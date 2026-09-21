//! `obc-boot`, the 32 KB nRF54L bootloader.
//!
//! Deliberately a dumb driver: read the BOOT_STATE RRAM page, let the host-tested `obc-dfu`
//! decode it (anything torn, blank, or garbage decodes to `Idle`) and run the install engine
//! (verify, flash, readback, trial or rollback), then act on the outcome. It jumps to the app at
//! [`app_slot_base`] and never resets, because a reset would re-enter here and roll the fresh
//! trial back. All sequencing lives in `obc_dfu::engine`; this crate contributes only resource
//! bring-up, and it must never be able to panic on any page content.
//!
//! LED0 is the entire UI (blink codes, table in the README): the bootloader never draws. Past
//! the fast path it does keep the panel alive. The app pre-paints an "Installing update" frame
//! that the Memory-in-Pixel glass holds, and `com.rs` parks the scan pins and keeps the
//! anti-DC-bias COM wave alternating under it. There is no executor, no timers, and no FAT, only
//! the blocking embassy-nrf HAL plus one coprocessor this crate cannot avoid: the card exists
//! only behind the sEMMC soft peripheral, so on the Install and Rollback paths `semmc.rs` boots
//! the armer-staged image on the FLPR, then parks the hart and resets the pads before the jump.
//! The app still owns the display blob's whole lifecycle.

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

/// The staging buffer's 32-bit-aligned home: the sEMMC firmware DMAs whole-block reads straight
/// into it, and a plain `[u8; N]` local has no alignment guarantee.
#[repr(C, align(4))]
struct AlignedBuf([u8; INSTALL_BUF_LEN]);

/// Initial retry backoff after an SD failure; doubles per attempt up to [`BACKOFF_MAX_MS`].
const BACKOFF_MIN_MS: u32 = 250;
const BACKOFF_MAX_MS: u32 = 8_000;

/// How many pre-erase card failures an `Armed` arm tolerates before the bootloader abandons it
/// and boots the intact old app. A round is one bring-up-or-verify failure followed by a
/// triple-blink and a backoff wait, so 10 rounds is about a minute. Only failures before the
/// engine's flash pass count, when the slot is provably untouched. A `Rollback` or a mid-flash
/// error is never abandoned; it parks forever.
const ARM_ABANDON_ROUNDS: u32 = 10;

/// The BOOT_STATE RRAM page, read in place. The address comes from the `__boot_state_base`
/// linker symbol, the same convention as the app's `__settings_base`: the magic address lives
/// only in the linker scripts. RRAM is memory-mapped for reads, so a shared borrow of the page
/// is sound, and the state is copied out before the install engine writes the page back.
fn boot_state_base() -> u32 {
    extern "C" {
        static __boot_state_base: u8;
    }
    core::ptr::addr_of!(__boot_state_base) as u32
}

fn boot_state_page() -> &'static [u8; PAGE_LEN] {
    unsafe { &*(boot_state_base() as *const [u8; PAGE_LEN]) }
}

/// Base of the app slot, which holds the app's vector table. It comes from the `__app_slot_base`
/// linker symbol, so the slot geometry never appears as a literal here. The slot's first two
/// words are the initial MSP and reset vector this bootloader jumps through. The slot ends at
/// [`semmc_stage_base`], so its length is `semmc_stage_base() - app_slot_base()`; the install
/// engine takes that size and owns the "padded image must fit" gate that keeps every flash clear
/// of the staged blob.
fn app_slot_base() -> u32 {
    extern "C" {
        static __app_slot_base: u8;
    }
    core::ptr::addr_of!(__app_slot_base) as u32
}

/// Base of the blob-stage carve, which is the app slot's end. `semmc.rs` reads the carve's
/// contents; this is only the slot bound.
fn semmc_stage_base() -> u32 {
    extern "C" {
        static __semmc_stage_base: u8;
    }
    core::ptr::addr_of!(__semmc_stage_base) as u32
}

#[entry]
fn main() -> ! {
    // HAL init: trims, debug unlock, glitch-detector off, default clocks (internal HF osc at
    // 64 MHz; the app raises itself to 128 MHz with its own config after the jump). With no
    // `gpiote` or `time-driver` feature compiled in, this enables no interrupt sources at all.
    // `FlprReset::Leave` keeps embassy off the FLPR: `semmc.rs` owns its lifecycle, parks it
    // again and resets the pads before the jump, and the app re-takes it from scratch.
    let mut config = embassy_nrf::config::Config::default();
    config.flpr_reset = embassy_nrf::config::FlprReset::Leave;
    let p = embassy_nrf::init(config);

    // Everything with a Drop (the LED pin, the parked panel pins) lives inside `boot`, so the
    // drops restore the pins to their reset state before the jump.
    boot(p);
    jump_to_app()
}

/// Decide and install. Returns whenever the machine should jump to the app: the fast `Idle`
/// path, a rejected stage, and a completed install all end in the same jump. Only the fatal
/// outcomes (the LED SOS park) diverge inside.
fn boot(p: embassy_nrf::Peripherals) {
    // One short blink: visible proof the bootloader ran, even with nothing else working.
    let mut led = Led::new(Output::new(p.P1_25, Level::High, OutputDrive::Standard));
    led.pulse_ms(100);

    let state = BootState::decode(boot_state_page());
    let decision = decide(&state);
    #[cfg(feature = "rtt")]
    defmt::info!("obc-boot: generation={=u32} → deciding", state.generation());

    // Fast path: nothing pending — no FLPR, no RRAMC, no DWT; straight to the app.
    if matches!(decision, BootDecision::Jump) {
        return;
    }

    // The DWT cycle counter paces the panel's COM wave, bounds every sEMMC deadline, and feeds
    // the rtt throughput meter. The card transport's "every wait is bounded" promise would be a
    // lie on a frozen counter, so the card is built only when the counter really runs. Without
    // it the decision takes the same abandon-or-park path as an unvalidatable blob.
    let cyccnt_ok = if let Some(mut cp) = cortex_m::Peripherals::take() {
        cp.DCB.enable_trace();
        cp.DWT.enable_cycle_counter();
        true
    } else {
        false
    };

    // Panel keep-alive for everything past the fast path (see `com.rs`): park the LS021's gate
    // and source lines driven-low so nothing floats into the glass while the app slot is
    // rewritten, and free-run the COM wave so the app's pre-painted frame survives the install
    // without a DC bias. The pins are copied from the app's bring-up. All of it drops back to
    // the reset state when `boot` returns, exactly like the LED pin.
    //
    // The two source lines B0/B1 are deliberately not parked: they live on P2.00/P2.04, which
    // are card pads (D3/D1) during the install. SD traffic wiggles them under the held frame,
    // which is harmless because with BCK parked low the panel never latches a source bit.
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
    // The COM electrodes are a 56–77 nF load, so these are high-drive, like the app's COM pins.
    // These are the DK's COM pins; the production board reroutes COM onto GPIOTE-capable pins
    // for the app's `com-hw` feature, so mirror that routing here when the board lands.
    let mut com = com::Com::start(
        Output::new(p.P1_22, Level::Low, OutputDrive::HighDrive), // VCOM
        Output::new(p.P1_23, Level::Low, OutputDrive::HighDrive), // VB
        Output::new(p.P1_24, Level::Low, OutputDrive::HighDrive), // VA
    );

    // The watchdog across the boot chain (the policy lives on `wdt::BootDog`): the arm path
    // enters here through a warm reset that carries the app's live 24 s dog, so adopt and pet it
    // through everything below. A cold power-on stays dog-less until the trial jump, and a plain
    // `Idle` boot never touches the WDT.
    let mut dog = wdt::BootDog::take(p.WDT0);

    let mut rram = Rramc::new(p.RRAMC);

    let app_base = app_slot_base();
    let slot = Slot { base: app_base, len: semmc_stage_base() - app_base };
    let mut buf = AlignedBuf([0u8; INSTALL_BUF_LEN]);

    // The card is life-support (no maps means no device), so a card that will not read gets a
    // triple-blink and a growing backoff rather than a hard failure. Two cases differ:
    //
    // - `Rollback`, or any mid-flash SD error: the trial image in the slot is the only bootable
    //   thing, or the slot is already being rewritten. Never abandon a touched slot, so retry
    //   forever; the designed worst case is that the rider reinserts the card and power-cycles.
    // - A pre-erase `Armed` arm: nothing is touched and the old app is intact. After
    //   `ARM_ABANDON_ROUNDS` failures the arm is cleared to `Idle` (the engine records
    //   `ArmAbandoned`, so the app can show the abandon card) and the old app boots, instead of
    //   stranding a device that still holds good firmware.
    let abandonable = matches!(decision, BootDecision::Install { .. });
    let mut backoff = BACKOFF_MIN_MS;
    let mut pre_erase_rounds = 0u32;

    // Only decisions that stream extents need the card at all, and the card exists only behind
    // the armer-staged sEMMC blob, validated before the FLPR runs a byte of it. An unvalidatable
    // carve is not a retryable card wobble, so it skips the backoff loop: an untouched `Armed`
    // arm is abandoned, and a `Rollback` parks on SOS for a power cycle to retry.
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
        // Bring the card up (or back up after a wobble) before the engine touches it. A
        // bring-up failure is inherently pre-erase, so an `Armed` arm here is abandonable. A lap
        // is at most about 9 s, far inside the 24 s dog window.
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
                // The next loop iteration re-inits the card at the top.
                dog.pet();
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD read failed mid-install — retrying in {=u32} ms", backoff);
                led.blink_code(3, &mut com);
                com.delay_ms(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
                // Only a pre-erase failure of an abandonable arm counts toward the budget. Once
                // the flash pass has begun we retry forever, because abandoning a half-written
                // slot would brick the device.
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
    // Hand the FLPR back before the jump: hart parked, card pads reset, so the app's own
    // bring-up starts from what reset would have given it.
    if let Some(card) = card.as_mut() {
        card.shutdown();
    }
    finish(outcome, abandonable, &mut led, &mut com, &mut dog);
}

/// Map the engine's outcome onto the LED and the jump-or-park endgame. `install_decision` marks
/// an `Install` decision, the only one whose trial jump starts the watchdog.
fn finish(outcome: Outcome, install_decision: bool, led: &mut Led, com: &mut com::Com, dog: &mut wdt::BootDog) {
    led.off();

    match outcome {
        // Nothing pending, or an accepted first-install trial: run the app.
        Outcome::Jump => {}
        Outcome::StageRejected => {
            #[cfg(feature = "rtt")]
            defmt::warn!("obc-boot: staged image invalid — arm cleared, booting the old app");
            led.blink_code(2, com);
        }
        // Same end state as a rejected stage, arm cleared and old app intact, so the same
        // 2-blink code. The cause (unreadable card or bad image) reaches the rider through the
        // app's verdict card, not the LED.
        Outcome::ArmAbandoned => {
            #[cfg(feature = "rtt")]
            defmt::warn!("obc-boot: arm abandoned (storage unreachable) — booting the old app");
            led.blink_code(2, com);
        }
        // The slot holds the readback-verified image and the follow-up state is written, so
        // jump straight into it. A reset here would re-enter this bootloader, which reads the
        // just-written `Trial` as unconfirmed and undoes the install before the new image ever
        // ran. This jump is the one trial boot.
        Outcome::Installed => {
            // On the Install decision this jump is the one trial boot of a freshly flashed
            // image, so make sure it runs under the dog. A trial that wedges before the app's
            // own WDT setup is dog-reset back into this bootloader, which rolls it back. A
            // rollback's jump re-enters the previously confirmed image, the same trust level as
            // a plain boot, so it stays dog-less on a cold boot.
            if install_decision {
                dog.start_for_trial();
            }
            #[cfg(feature = "rtt")]
            defmt::info!("obc-boot: install complete — jumping into the new image (trial boot)");
        }
        // Readback never matched, or the RRAM write path failed, after all retries. The state
        // page still holds the `Armed` record, so a power cycle retries the whole install. Park
        // on SOS rather than reset-looping, and pet an adopted dog so the park stays a park.
        Outcome::FlashError => {
            #[cfg(feature = "rtt")]
            defmt::error!("obc-boot: flash/readback failed after retries — halting (power cycle retries)");
            led.sos_forever(com, || dog.pet());
        }
        // The retry loop never breaks with SdError, so this arm only keeps the match total.
        Outcome::SdError { .. } => led.sos_forever(com, || dog.pet()),
    }
}

/// Hand the machine to the app at [`app_slot_base`] in a state as close to reset as possible,
/// because a non-bootloader build enters the app directly from reset and it must not notice the
/// difference. Two deviations are deliberate and harmless: VTOR points at the app's table instead
/// of 0, and the FPU is already enabled.
fn jump_to_app() -> ! {
    let app_base = app_slot_base();
    unsafe {
        // Quiesce the NVIC: disable and clear-pend every external interrupt line. Nothing here
        // arms one today, but this stays as the reset contract for whatever a future HAL init
        // does. PRIMASK is deliberately left CLEAR: it is clear at reset and the app's entry
        // does not re-enable interrupts, so masking here would hand the app a dead interrupt
        // system.
        let nvic = &*cortex_m::peripheral::NVIC::PTR;
        for i in 0..nvic.icer.len() {
            nvic.icer[i].write(0xFFFF_FFFF); // disable 32 lines
            nvic.icpr[i].write(0xFFFF_FFFF); // clear their pending bits
        }
        // Point VTOR at the app's vector table before the jump, so the app's handlers serve any
        // exception from its first instruction. `asm::bootload` does not write VTOR, so this
        // write is load-bearing. DSB and ISB order it against the NVIC writes above and the
        // jump below.
        (*cortex_m::peripheral::SCB::PTR).vtor.write(app_base);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        // `asm::bootload` clears CONTROL.SPSEL (the main stack, as at reset), loads MSP from
        // *(app base), and branches to *(app base + 4) with the thumb bit set. It diverges.
        cortex_m::asm::bootload(app_base as *const u32)
    }
}

/// The bootloader must never panic: every decode failure is an `Idle` by construction, the
/// engine is total over any page content, and the IO adapters carry no unwraps. If a panic
/// happens anyway, park the core rather than reset-looping into a boot storm; the LED staying
/// dark after power-on is the field symptom. (`rtt` builds use panic-probe's handler instead.)
///
/// On the warm-reset install path an adopted watchdog is live and nothing pets it here, so the
/// park is bounded at one WDT period. That reset re-enters this bootloader with the state page
/// unchanged, and a dog-fired reset does not carry the dog over, so a deterministic panic parks
/// for good on the second entry. Cold-boot panics have no dog and park forever.
#[cfg(not(feature = "rtt"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
