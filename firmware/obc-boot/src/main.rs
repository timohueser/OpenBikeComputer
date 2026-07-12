//! `obc-boot` — the 32 KB nRF54L bootloader (epic #615; S2 #617 boot chain, S3 #618 install
//! engine).
//!
//! Deliberately a dumb driver: read the BOOT_STATE RRAM page, let the host-tested `obc-dfu`
//! decode it (anything torn/blank/garbage ⇒ `Idle`) and run the install engine
//! (verify → flash → readback → trial/rollback — ALL sequencing lives in
//! `obc_dfu::engine`, unit-tested with mock IO), then act on the returned outcome: jump to the
//! app at [`app_slot_base`] (after an install, that IS the freshly-written image's one trial boot —
//! never a reset, which would re-enter here and roll the trial back), or park with an LED code. This
//! crate contributes only resource bring-up (SPI card, RRAMC, LED) via `sd`/`install`/`led`;
//! the bootloader itself must never be able to panic on any page content.
//!
//! LED0 is the entire UI (blink codes — table in the README): the bootloader never draws.
//! On the slow paths it does keep the *panel* alive — the app pre-paints an "Installing
//! update" frame the Memory-in-Pixel glass holds, and `com.rs` parks the scan pins + keeps
//! the anti-DC-bias COM wave alternating under it. No executor, no timers, no FAT, no FLPR —
//! blocking embassy-nrf HAL only; the app starts the FLPR core itself.

#![no_std]
#![no_main]

mod com;
mod install;
mod led;
mod sd;
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

/// Initial retry backoff after an SD failure; doubles per attempt up to [`BACKOFF_MAX_MS`].
const BACKOFF_MIN_MS: u32 = 250;
const BACKOFF_MAX_MS: u32 = 8_000;

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
/// slot *end* is [`boot_state_base`] (the app slot runs right up to the BOOT_STATE page), so the
/// slot length is `boot_state_base() - app_slot_base()` — the install engine takes that size and
/// owns the "padded image must fit" gate, host-tested.
fn app_slot_base() -> u32 {
    extern "C" {
        static __app_slot_base: u8;
    }
    core::ptr::addr_of!(__app_slot_base) as u32
}

#[entry]
fn main() -> ! {
    // HAL init: trims, debug unlock, glitch-detector off, default clocks (internal HF osc,
    // 64 MHz — the app raises itself to 128 MHz with its own config after the jump). With no
    // `gpiote`/`time-driver` features compiled in, this enables no interrupt sources beyond
    // what the SPIM bring-up registers (and `jump_to_app` quiesces the NVIC regardless).
    // `FlprReset::Leave`: the bootloader never touches the FLPR coprocessor — the app owns
    // its whole lifecycle (its own init resets it, then loads + starts the panel blob).
    let mut config = embassy_nrf::config::Config::default();
    config.flpr_reset = embassy_nrf::config::FlprReset::Leave;
    let p = embassy_nrf::init(config);

    // Everything with a Drop (LED pin, SPI bus, CS) lives inside `boot` — when it returns
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
    let mut led = Led::new(Output::new(p.P2_09, Level::High, OutputDrive::Standard));
    led.pulse_ms(100);

    // The whole decision matrix is pure, host-tested obc-dfu logic; garbage decodes to Idle.
    let state = BootState::decode(boot_state_page());
    let decision = decide(&state);
    #[cfg(feature = "rtt")]
    defmt::info!("obc-boot: generation={=u32} → deciding", state.generation());

    // Fast path: nothing pending — no SPI, no RRAMC, no DWT; straight to the app.
    if matches!(decision, BootDecision::Jump) {
        return;
    }

    // The DWT cycle counter paces the panel's COM wave (`com.rs`) — and feeds the rtt
    // throughput meter. If the take somehow fails the COM poll just never fires (CYCCNT
    // stays 0 — a no-op, never a hang).
    if let Some(mut cp) = cortex_m::Peripherals::take() {
        cp.DCB.enable_trace();
        cp.DWT.enable_cycle_counter();
    }

    // Panel keep-alive for everything past the fast path (see `com.rs`): park the LS021's
    // gate + source lines driven-low so nothing floats into the glass while the app slot is
    // rewritten, and free-run the COM wave on the three COM pins so the app's pre-painted
    // "Installing update" frame survives the install without a DC bias. Pins are copied from
    // the app's bring-up (`obc-fw-nrf54l/src/main.rs`, the FLPR pin block) — deliberate
    // duplication, the same policy as the SD pins in `sd.rs`. All of it drops back to the
    // reset state when `boot` returns, exactly like the SPI/LED pins.
    let _panel_pins = [
        Output::new(p.P1_00, Level::Low, OutputDrive::Standard), // GSP
        Output::new(p.P1_01, Level::Low, OutputDrive::Standard), // GCK
        Output::new(p.P1_12, Level::Low, OutputDrive::Standard), // GEN
        Output::new(p.P1_10, Level::Low, OutputDrive::Standard), // INTB
        Output::new(p.P1_14, Level::Low, OutputDrive::Standard), // BSP
        Output::new(p.P2_06, Level::Low, OutputDrive::Standard), // BCK
        Output::new(p.P2_00, Level::Low, OutputDrive::Standard), // R0 (odd)
        Output::new(p.P2_01, Level::Low, OutputDrive::Standard), // R1 (even)
        Output::new(p.P2_02, Level::Low, OutputDrive::Standard), // G0
        Output::new(p.P2_03, Level::Low, OutputDrive::Standard), // G1
        Output::new(p.P2_04, Level::Low, OutputDrive::Standard), // B0
        Output::new(p.P2_05, Level::Low, OutputDrive::Standard), // B1
    ];
    // The COM electrodes are a 56–77 nF load → high-drive, like the app's COM pins. These are
    // the DK's COM pins (the app's M33 `com` driver); the production board reroutes COM onto
    // GPIOTE-capable pins for the app's `com-hw` feature (P1_04/05/15 today) — when that board
    // lands, mirror its routing here too.
    let mut com = com::Com::start(
        Output::new(p.P2_07, Level::Low, OutputDrive::HighDrive), // VCOM
        Output::new(p.P2_08, Level::Low, OutputDrive::HighDrive), // VB
        Output::new(p.P2_10, Level::Low, OutputDrive::HighDrive), // VA
    );

    // The watchdog across the boot chain (DR1, #729 — the policy lives on `wdt::BootDog`): the
    // arm path enters here through a warm reset that carries the app's live 24 s dog, so adopt
    // and pet it through everything below; a cold power-on stays dog-less until the trial jump.
    // Constructed only past the fast path — a plain `Idle` boot never touches the WDT.
    let mut dog = wdt::BootDog::take(p.WDT0);

    let mut rram = Rramc::new(p.RRAMC);

    // The card is needed only when the engine will stream extents. Bring-up retries FOREVER
    // with a triple-blink + growing backoff: the card is life-support (the device is a
    // paperweight without its maps), so "park until the card is back, then a power cycle
    // recovers" is the designed worst case — never proceed toward an erase without a card.
    let mut card = match decision {
        BootDecision::Install(_) | BootDecision::Rollback(_) => {
            let mut blocks = sd::SdBlocks::new(p.SERIAL22, p.P1_11, p.P1_07, p.P1_06, p.P0_00);
            let mut backoff = BACKOFF_MIN_MS;
            while !blocks.try_init() {
                // An adopted dog must not convert this park into a reset storm — pet per lap
                // (a lap is ≤ ~9 s: three blinks + the ≤8 s backoff, far inside 24 s), so the
                // loop keeps waiting for the card exactly as designed.
                dog.pet();
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD init failed — retrying in {=u32} ms", backoff);
                led.blink_code(3, &mut com);
                com.delay_ms(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
            }
            Some(blocks)
        }
        _ => None,
    };

    // Run the engine; a transient SD error mid-stream gets the same forever-retry treatment as
    // a missing card (state untouched by construction — host-tested), everything else is final.
    let app_base = app_slot_base();
    let slot = Slot { base: app_base, len: boot_state_base() - app_base };
    let mut buf = [0u8; INSTALL_BUF_LEN];
    let mut backoff = BACKOFF_MIN_MS;
    let outcome = loop {
        let mut io = install::BootIo::new(card.as_ref(), &mut rram, &mut led, &mut dog, &mut com, boot_state_base());
        match engine::run(&state, &slot, &mut io, &mut buf) {
            Outcome::SdError => {
                // Same pet-per-lap as the init loop above; inside the engine the progress
                // hook pets every chunk (`install.rs`).
                dog.pet();
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD read failed mid-install — retrying in {=u32} ms", backoff);
                led.blink_code(3, &mut com);
                com.delay_ms(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                if let Some(card) = card.as_mut() {
                    let _ = card.try_init();
                }
            }
            outcome => break outcome,
        }
    };
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
            led.blink_code(2, &mut com);
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
            if matches!(decision, BootDecision::Install(_)) {
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
            led.sos_forever(&mut com, || dog.pet());
        }
        // The retry loop above never breaks with SdError; keep the match total without a
        // panic path.
        Outcome::SdError => led.sos_forever(&mut com, || dog.pet()),
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
        // 1. Quiesce the NVIC: disable + clear-pend every external interrupt line, so nothing
        //    a bootloader HAL touched (the S3 SPIM bring-up registers SERIAL22) can vector
        //    into the app before it is ready. PRIMASK is deliberately left CLEAR: we never
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
