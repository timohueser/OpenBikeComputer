//! `obc-boot` — the 32 KB nRF54L bootloader (epic #615; S2 #617 boot chain, S3 #618 install
//! engine).
//!
//! Deliberately a dumb driver: read the BOOT_STATE RRAM page, let the host-tested `obc-dfu`
//! decode it (anything torn/blank/garbage ⇒ `Idle`) and run the install engine
//! (verify → flash → readback → trial/rollback — ALL sequencing lives in
//! `obc_dfu::engine`, unit-tested with mock IO), then act on the returned outcome: jump to the
//! app at [`app_slot_base`] (after an install, that IS the freshly-written image's one trial boot —
//! never a reset, which would re-enter here and roll the trial back), or park with an LED code. This
//! crate contributes only resource bring-up (the sEMMC card transport, RRAMC, LED) via
//! `semmc`/`install`/`led`; the bootloader itself must never be able to panic on any page content.
//!
//! LED0 is the entire UI (blink codes — table in the README): the bootloader never draws.
//! On the slow paths it does keep the *panel* alive — the app pre-paints an "Installing
//! update" frame the Memory-in-Pixel glass holds, and `com.rs` parks the scan pins + keeps
//! the anti-DC-bias COM wave alternating under it. No executor, no timers, no FAT —
//! blocking embassy-nrf HAL plus, since the storage pivot (#1158), the one coprocessor use
//! this crate cannot avoid: the card only exists behind the sEMMC soft peripheral, so on the
//! Install/Rollback paths `semmc.rs` boots the **armer-staged** image on the FLPR
//! (`OBCU_Spec.md` §3) and parks the hart + resets the pads again before the jump. The app
//! still owns the display blob's whole lifecycle.

#![no_std]
#![no_main]

mod com;
mod install;
mod led;
mod semmc;
mod wdt;

use cortex_m_rt::entry;
#[cfg(feature = "rtt")]
use {defmt_rtt as _, panic_probe as _};

use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::rramc::Rramc;
use led::Led;
use obc_dfu::engine::{self, Outcome, Slot};
use obc_dfu::{decide, BootDecision, BootState, PAGE_LEN};

/// The engine's SD↔RRAM staging buffer: 8 whole SD blocks between card reads and line writes.
const INSTALL_BUF_LEN: usize = 4096;

/// The staging buffer's 32-bit-aligned home — the sEMMC firmware DMAs whole-block reads straight
/// into it (`semmc.rs` documents the alignment rule; a plain `[u8; N]` local has no guarantee).
#[repr(C, align(4))]
struct AlignedBuf([u8; INSTALL_BUF_LEN]);

/// Initial retry backoff after an SD failure; doubles per attempt up to [`BACKOFF_MAX_MS`].
const BACKOFF_MIN_MS: u32 = 250;
const BACKOFF_MAX_MS: u32 = 8_000;

/// DR3 (#731): how many **pre-erase** card failures an `Armed` arm tolerates before the bootloader
/// abandons it and boots the intact old app. A "round" is one bring-up-or-verify failure followed
/// by a triple-blink + backoff wait; with the backoff ladder (250, 500, 1000, 2000, 4000, then
/// 8000 ms) 10 rounds is ~48 s of backoff plus the blink time — on the order of a minute, as the
/// issue asks. Only failures **before** the engine's flash pass begins count (the slot is provably
/// untouched then); a `Rollback` or a mid-flash error is never abandoned (it parks forever).
const ARM_ABANDON_ROUNDS: u32 = 10;

/// The BOOT_STATE RRAM page, read in place. The address comes from the `__boot_state_base`
/// linker symbol (`ORIGIN(BOOT_STATE)` in `memory.x` — the app-side `build.rs` PROVIDEs the
/// same symbol for the armer), the same convention as the app's `__settings_base`: the magic
/// address lives only in the linker scripts. RRAM is plain memory-mapped for reads, so a
/// shared borrow of the page is sound; the borrow is released (the state copied out) before
/// the install engine can write the page back.
fn boot_state_base() -> u32 {
    extern "C" {
        static __boot_state_base: u8;
    }
    core::ptr::addr_of!(__boot_state_base) as u32
}

fn boot_state_page() -> &'static [u8; PAGE_LEN] {
    unsafe { &*(boot_state_base() as *const [u8; PAGE_LEN]) }
}

/// Base of the app slot — the app's vector table (`0x8000`). Comes from the `__app_slot_base`
/// linker symbol (`memory.x` PROVIDEs it as `ORIGIN(FLASH) + LENGTH(FLASH)`, i.e. one past the
/// bootloader's own region), the same convention as [`boot_state_base`] and mirroring the board
/// crate's `__app_slot_base`: the slot geometry lives only in the linker scripts, never as a
/// literal here. The board crate's `build.rs` links the app's `FLASH` at this exact origin, and
/// its raw first words are the initial MSP + reset vector this bootloader jumps through. The
/// slot *end* is [`semmc_stage_base`] (#1158 — the blob-stage carve sits between the slot and
/// the BOOT_STATE page), so the slot length is `semmc_stage_base() - app_slot_base()` — the
/// install engine takes that size and owns the "padded image must fit" gate, host-tested; that
/// same gate is what keeps every possible flash clear of the staged blob.
fn app_slot_base() -> u32 {
    extern "C" {
        static __app_slot_base: u8;
    }
    core::ptr::addr_of!(__app_slot_base) as u32
}

/// Base of the blob-stage carve (`OBCU_Spec.md` §3) — the app slot's end. Same linker-symbol
/// convention as the rest; `semmc.rs` reads the carve's *contents*, this is only the slot bound.
fn semmc_stage_base() -> u32 {
    extern "C" {
        static __semmc_stage_base: u8;
    }
    core::ptr::addr_of!(__semmc_stage_base) as u32
}

#[entry]
fn main() -> ! {
    // HAL init: trims, debug unlock, glitch-detector off, default clocks (internal HF osc,
    // 64 MHz — the app raises itself to 128 MHz with its own config after the jump; semmc.rs
    // documents what the halved clock means for the card rates). With no `gpiote`/`time-driver`
    // features compiled in this enables no interrupt sources at all — the sEMMC transport is
    // polled, vectorless MMIO (and `jump_to_app` quiesces the NVIC regardless).
    // `FlprReset::Leave`: embassy must not touch the FLPR behind our back — its lifecycle is
    // explicit now: parked + booted by `semmc.rs` on the card paths, parked again and pads
    // reset before the jump; the app then re-takes it from scratch for the display blob.
    let mut config = embassy_nrf::config::Config::default();
    config.flpr_reset = embassy_nrf::config::FlprReset::Leave;
    let p = embassy_nrf::init(config);

    // Everything with a Drop (LED pin, parked panel pins) lives inside `boot` — when it returns
    // ("run the app"), the drops restore the pins to their reset state before the jump.
    boot(p);
    jump_to_app()
}

/// Decide + install. Returns whenever the machine should jump to the app — the fast `Idle`
/// path, a rejected stage (old app intact), and a completed install (the freshly-written
/// image's trial boot) all end in the same jump; only the fatal outcomes (LED SOS park)
/// diverge inside.
fn boot(p: embassy_nrf::Peripherals) {
    // One short blink: visible proof the bootloader ran, even with nothing else working.
    let mut led = Led::new(Output::new(p.P1_25, Level::High, OutputDrive::Standard));
    led.pulse_ms(100);

    // The whole decision matrix is pure, host-tested obc-dfu logic; garbage decodes to Idle.
    let state = BootState::decode(boot_state_page());
    let decision = decide(&state);
    #[cfg(feature = "rtt")]
    defmt::info!("obc-boot: generation={=u32} → deciding", state.generation());

    // Fast path: nothing pending — no FLPR, no RRAMC, no DWT; straight to the app.
    if matches!(decision, BootDecision::Jump) {
        return;
    }

    // The DWT cycle counter paces the panel's COM wave (`com.rs`), bounds every sEMMC deadline
    // (`semmc.rs`), and feeds the rtt throughput meter. If the take somehow fails the COM poll
    // just never fires (CYCCNT stays 0 — a no-op, never a hang) — but the card transport's
    // "every wait is bounded" promise would be a lie on a frozen counter, so the card is only
    // constructed when the counter is genuinely running (`cyccnt_ok` below); without it the
    // decision falls into the same abandon/park path as an unvalidatable blob.
    let cyccnt_ok = if let Some(mut cp) = cortex_m::Peripherals::take() {
        cp.DCB.enable_trace();
        cp.DWT.enable_cycle_counter();
        true
    } else {
        false
    };

    // Panel keep-alive for everything past the fast path (see `com.rs`): park the LS021's
    // gate + source lines driven-low so nothing floats into the glass while the app slot is
    // rewritten, and free-run the COM wave on the three COM pins so the app's pre-painted
    // "Installing update" frame survives the install without a DC bias. Pins are copied from
    // the app's bring-up (`obc-fw-nrf54l/src/main.rs`, the FLPR pin block, post-#1159 rehome) —
    // deliberate duplication, the same policy as the card pads in `semmc.rs`. All of it drops
    // back to the reset state when `boot` returns, exactly like the LED pin.
    //
    // The two source lines NOT parked here are the point of the whole pivot: B0/B1 live on
    // P2.00/P2.04, which are card pads (D3/D1) during the install. SD traffic wiggles them
    // under the held frame — harmless, because with BCK parked low the panel never latches a
    // source bit; the same reasoning as the app's storage/display mux (#1158).
    let _panel_pins = [
        Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_11, Level::Low, OutputDrive::Standard), // GCK (P1.01 is NFC on the LM20-DK)
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_13, Level::Low, OutputDrive::Standard), // INTB
        Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_07, Level::Low, OutputDrive::Standard), // BCK
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // R0
        Output::new(p.P2_08, Level::Low, OutputDrive::Standard), // R1
        Output::new(p.P2_09, Level::Low, OutputDrive::Standard), // G0
        Output::new(p.P2_10, Level::Low, OutputDrive::Standard), // G1
    ];
    // The COM electrodes are a 56–77 nF load → high-drive, like the app's COM pins. These are
    // the DK's COM pins (the app's M33 `com` driver); the production board reroutes COM onto
    // GPIOTE-capable pins for the app's `com-hw` feature (P1_04/05/15 today) — when that board
    // lands, mirror its routing here too.
    let mut com = com::Com::start(
        Output::new(p.P1_22, Level::Low, OutputDrive::HighDrive), // VCOM
        Output::new(p.P1_23, Level::Low, OutputDrive::HighDrive), // VB
        Output::new(p.P1_24, Level::Low, OutputDrive::HighDrive), // VA
    );

    // The watchdog across the boot chain (DR1, #729 — the policy lives on `wdt::BootDog`): the
    // arm path enters here through a warm reset that carries the app's live 24 s dog, so adopt
    // and pet it through everything below; a cold power-on stays dog-less until the trial jump.
    // Constructed only past the fast path — a plain `Idle` boot never touches the WDT.
    let mut dog = wdt::BootDog::take(p.WDT0);

    let mut rram = Rramc::new(p.RRAMC);

    let app_base = app_slot_base();
    // The slot ends at the blob-stage carve (#1158) — the engine's padded-fit gate is what keeps
    // every flash clear of the staged sEMMC image and the BOOT_STATE page beyond it.
    let slot = Slot { base: app_base, len: semmc_stage_base() - app_base };
    // 32-bit aligned so the engine's whole-block reads DMA straight into it (the sEMMC firmware's
    // requirement; `semmc.rs` bounces the engine's unaligned mid-buffer slices per-block).
    let mut buf = AlignedBuf([0u8; INSTALL_BUF_LEN]);

    // The card is life-support (no maps ⇒ no device), so a card that won't read is retried with a
    // triple-blink + growing backoff rather than a hard failure. Two cases differ (DR3, #731):
    //
    // - `Rollback`, or **any mid-flash** SD error: the slot's trial image is the only bootable
    //   thing, or the app slot is already being rewritten — never abandon a touched slot. Retry
    //   FOREVER (the designed worst case: reinsert the card and power-cycle).
    // - **pre-erase `Armed`**: nothing has been touched — the old app is intact at `app_base`. A
    //   card that died in the drawer, a swapped-in fresh maps card, a card lost on a trip: after
    //   `ARM_ABANDON_ROUNDS` pre-erase failures (~a minute) ABANDON the arm — clear it to `Idle`
    //   (the engine records `ArmAbandoned` so the app shows the abandon card) and boot the old app,
    //   instead of stranding a device that still holds perfectly good firmware.
    let abandonable = matches!(decision, BootDecision::Install { .. });
    let mut backoff = BACKOFF_MIN_MS;
    let mut pre_erase_rounds = 0u32;

    // Only decisions that stream extents need the card at all — and since #1158 the card only
    // exists behind the armer-staged sEMMC blob, validated (CRC frame + the image's own metadata,
    // OBCU_Spec.md §3.4) before the FLPR runs a byte of it. An unvalidatable carve is not a
    // retryable card wobble — nothing on the card side can heal it — so it skips the backoff
    // loop entirely: an untouched `Armed` arm is abandoned like DR3's unreadable card (the app
    // then shows the abandon card), and a `Rollback` parks on SOS (a power cycle retries; the
    // armer's stage-before-arm ordering makes this near-unreachable short of RRAM decay).
    let mut card = match decision {
        BootDecision::Install { .. } | BootDecision::Rollback { .. } => {
            match semmc::staged_blob().filter(|_| cyccnt_ok) {
                Some((blob, geom)) => Some(semmc::BootSemmc::new(blob, geom)),
                None => {
                    #[cfg(feature = "rtt")]
                    defmt::warn!("obc-boot: no valid staged sEMMC blob — storage is unreachable");
                    if abandonable {
                        let mut io =
                            install::BootIo::new(None, &mut rram, &mut led, &mut dog, &mut com, boot_state_base());
                        let outcome = engine::abandon_arm(&state, &mut io);
                        finish(outcome, abandonable, &mut led, &mut com, &mut dog);
                        return;
                    }
                    led.sos_forever(&mut com, || dog.pet())
                }
            }
        }
        _ => None,
    };

    let outcome = loop {
        // Bring the card up (or back up after a wobble) before the engine touches it. A bring-up
        // failure is inherently pre-erase — nothing has streamed — so an `Armed` arm here is
        // abandonable. Pet the adopted dog per lap (a lap is ≤ ~9 s: three blinks + the ≤8 s
        // backoff, far inside 24 s) so the wait stays a wait, never a reset storm.
        if let Some(blocks) = card.as_mut() {
            if !blocks.try_init() {
                dog.pet();
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD init failed — retrying in {=u32} ms", backoff);
                led.blink_code(3, &mut com);
                com.delay_ms(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                if abandonable {
                    pre_erase_rounds += 1;
                    if pre_erase_rounds >= ARM_ABANDON_ROUNDS {
                        #[cfg(feature = "rtt")]
                        defmt::warn!(
                            "obc-boot: card unreadable after {=u32} rounds — abandoning the arm, booting the old app",
                            pre_erase_rounds
                        );
                        let mut io =
                            install::BootIo::new(None, &mut rram, &mut led, &mut dog, &mut com, boot_state_base());
                        break engine::abandon_arm(&state, &mut io);
                    }
                }
                continue;
            }
        }

        let mut io = install::BootIo::new(card.as_mut(), &mut rram, &mut led, &mut dog, &mut com, boot_state_base());
        match engine::run(&state, &slot, &mut io, &mut buf.0) {
            Outcome::SdError { pre_erase } => {
                // Same pet-per-lap as the bring-up above; inside the engine the progress hook pets
                // every chunk (`install.rs`). The next loop iteration re-inits the card at the top.
                dog.pet();
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD read failed mid-install — retrying in {=u32} ms", backoff);
                led.blink_code(3, &mut com);
                com.delay_ms(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                // Only a pre-erase failure of an abandonable (`Armed`) arm counts toward the budget;
                // once the flash pass has begun (`pre_erase == false`) we retry forever — abandoning
                // a half-written slot would brick. The engine owns that distinction, host-tested.
                if abandonable && pre_erase {
                    pre_erase_rounds += 1;
                    if pre_erase_rounds >= ARM_ABANDON_ROUNDS {
                        #[cfg(feature = "rtt")]
                        defmt::warn!(
                            "obc-boot: card unreadable after {=u32} rounds — abandoning the arm, booting the old app",
                            pre_erase_rounds
                        );
                        let mut io =
                            install::BootIo::new(None, &mut rram, &mut led, &mut dog, &mut com, boot_state_base());
                        break engine::abandon_arm(&state, &mut io);
                    }
                }
            }
            outcome => break outcome,
        }
    };
    // Hand the FLPR back before the jump: hart parked, card pads reset — the app's own bring-up
    // re-takes the coprocessor from scratch and must start from what reset would have given it.
    if let Some(card) = card.as_mut() {
        card.shutdown();
    }
    finish(outcome, abandonable, &mut led, &mut com, &mut dog);
}

/// Map the engine's outcome onto the LED + the jump/park endgame — shared by the normal
/// install path and the invalid-blob early abandon. `install_decision` = the decision was
/// `Install` (only that outcome's trial jump starts the watchdog).
fn finish(outcome: Outcome, install_decision: bool, led: &mut Led, com: &mut com::Com, dog: &mut wdt::BootDog) {
    led.off();

    match outcome {
        // Nothing pending (unreachable here — handled by the fast path) or an accepted
        // first-install trial: run the app.
        Outcome::Jump => {}
        // The staged image failed verification; the arm is cleared and the old app is intact.
        // Two blinks so a watcher can tell "update refused" from a plain boot, then run it.
        Outcome::StageRejected => {
            #[cfg(feature = "rtt")]
            defmt::warn!("obc-boot: staged image invalid — arm cleared, booting the old app");
            led.blink_code(2, com);
        }
        // DR3 (#731): the card never came up for an untouched `Armed` arm within the retry budget
        // — or (#1158) the staged sEMMC blob failed validation, which no retry heals — so it was
        // cleared to `Idle` and the intact old app is booted. Same end state as a rejected stage
        // — arm cleared, old app intact — so the same 2-blink code; the *cause* (unreadable card
        // vs. bad image) is surfaced to the rider by the app's verdict card, not the LED.
        Outcome::ArmAbandoned => {
            #[cfg(feature = "rtt")]
            defmt::warn!("obc-boot: arm abandoned (storage unreachable) — booting the old app");
            led.blink_code(2, com);
        }
        // The slot holds the readback-verified image and the follow-up state (`Trial` after an
        // install, `Idle` after a rollback) is written: jump straight into it. Never reset
        // here — a reset would re-enter this bootloader with the just-written `Trial`, which
        // `decide` reads as an *unconfirmed* trial (`Rollback`), undoing the install before the
        // new image ever ran. The jump IS the one trial boot; a later bootloader entry that
        // still sees `Trial` then genuinely means the trial went unconfirmed.
        Outcome::Installed => {
            // On the Install decision this jump IS the one trial boot of a freshly-flashed
            // image — make sure it runs under the dog (Gap B of #729): started here on a cold
            // boot, already live and inherited on the warm-reset path; either way the app
            // adopts it at its own WDT setup, and a trial that wedges before getting there is
            // dog-reset back into this bootloader → unconfirmed `Trial` → rollback. A
            // *rollback's* jump (also `Installed`) re-enters the previously confirmed image —
            // the same trust level as a plain `Idle` boot, so it deliberately stays dog-less
            // on a cold boot, like the fast path.
            if install_decision {
                dog.start_for_trial();
            }
            #[cfg(feature = "rtt")]
            defmt::info!("obc-boot: install complete — jumping into the new image (trial boot)");
        }
        // Readback never matched (or the RRAM write path failed) after all retries. The state
        // page still holds the Armed record, so a power cycle retries the whole install; park
        // on SOS rather than reset-looping into a boot storm. Petting an adopted dog keeps the
        // park a park (#729): letting it fire would turn this into a 24 s reset-and-rehammer
        // cycle against a card that just failed readback — the opposite of the documented
        // "halt until a human power-cycles" design.
        Outcome::FlashError => {
            #[cfg(feature = "rtt")]
            defmt::error!("obc-boot: flash/readback failed after retries — halting (power cycle retries)");
            led.sos_forever(com, || dog.pet());
        }
        // The retry loop never breaks with SdError (a pre-erase one is retried then abandoned;
        // a mid-flash one is retried forever); keep the match total without a panic path.
        Outcome::SdError { .. } => led.sos_forever(com, || dog.pet()),
    }
}

/// Hand the machine to the app at [`app_slot_base`], matching the reset state as closely as
/// possible — the app was previously entered directly from reset and must not notice the
/// difference. Known deviations from a cold reset, both harmless: VTOR points at the app's
/// table instead of 0 (deliberate — that's the mechanism), and the FPU is already enabled
/// (this crate's own cortex-m-rt reset enabled CPACR; the app's reset handler re-enables it
/// idempotently).
fn jump_to_app() -> ! {
    let app_base = app_slot_base();
    unsafe {
        // 1. Quiesce the NVIC: disable + clear-pend every external interrupt line — nothing
        //    here arms one today (the sEMMC transport is polled, vectorless MMIO), but this
        //    stays as the belt-and-braces reset contract for whatever a future HAL init does.
        //    PRIMASK is deliberately left CLEAR: we never
        //    set it, the app's cortex-m-rt entry does not re-enable interrupts, and at reset
        //    PRIMASK is clear — masking here would hand the app a dead interrupt system.
        let nvic = &*cortex_m::peripheral::NVIC::PTR;
        for i in 0..nvic.icer.len() {
            nvic.icer[i].write(0xFFFF_FFFF); // disable 32 lines
            nvic.icpr[i].write(0xFFFF_FFFF); // clear their pending bits
        }
        // 2. Point VTOR at the app's vector table BEFORE the jump, so the app's handlers
        //    serve any exception from its very first instruction. cortex-m 0.7's
        //    `asm::bootload` does NOT write VTOR (verified against its source — it only loads
        //    MSP and branches), so this write is load-bearing. DSB+ISB order the write against
        //    both the NVIC writes above and the jump below.
        (*cortex_m::peripheral::SCB::PTR).vtor.write(app_base);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        // 3.+4. `asm::bootload(app_base)`: clears CONTROL.SPSEL (main stack — already the
        //    case here, matching reset), loads MSP from *(app base), and branches to
        //    *(app base + 4)|1 (the app's reset vector, thumb bit set). Diverges — the
        //    bootloader is gone after this line.
        cortex_m::asm::bootload(app_base as *const u32)
    }
}

/// The bootloader must never panic — every decode failure is an `Idle` by construction, the
/// engine is total over any page content (host-tested), and the IO adapters carry no unwraps.
/// If a panic happens anyway (a future logic bug), park the core rather than reset-looping
/// into a brick-flavoured boot storm; the LED staying dark after power-on is the field
/// symptom. (`rtt` builds swap this for panic-probe's printing handler.)
///
/// One caveat since DR1 (#729): on the warm-reset install path an **adopted watchdog is live**
/// and nothing pets it here (deliberate — this handler has no access to the handle, and a
/// panicking bootloader shouldn't keep itself alive), so the park is bounded at one WDT period
/// (≤ 24 s) before the dog resets the machine. That reset re-enters this bootloader with the
/// state page unchanged — and, per the app's WDT notes (`obc-fw-nrf54l/src/main.rs`), a
/// dog-fired reset does *not* carry the dog over, so a deterministic panic parks for good on
/// the second entry: one bounded retry, then the designed dark-LED park — still no reset loop.
/// Cold-boot panics (no dog) park forever exactly as before.
#[cfg(not(feature = "rtt"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
