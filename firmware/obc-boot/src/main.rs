//! `obc-boot` — the 32 KB nRF54L bootloader (epic #615; S2 #617 boot chain, S3 #618 install
//! engine).
//!
//! Deliberately a dumb driver: read the BOOT_STATE RRAM page, let the host-tested `obc-dfu`
//! decode it (anything torn/blank/garbage ⇒ `Idle`) and run the install engine
//! (verify → flash → readback → trial/rollback — ALL sequencing lives in
//! `obc_dfu::engine`, unit-tested with mock IO), then act on the returned outcome: jump to the
//! app at [`APP_BASE`], reset into the freshly-written image, or park with an LED code. This
//! crate contributes only resource bring-up (SPI card, RRAMC, LED) via `sd`/`install`/`led`;
//! the bootloader itself must never be able to panic on any page content.
//!
//! LED0 is the entire UI (blink codes — table in the README). No executor, no timers, no FAT,
//! no FLPR — blocking embassy-nrf HAL only; the app starts the FLPR core itself.

#![no_std]
#![no_main]

mod install;
mod led;
mod sd;

use cortex_m_rt::entry;
#[cfg(feature = "rtt")]
use {defmt_rtt as _, panic_probe as _};

use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::rramc::Rramc;
use led::Led;
use obc_dfu::engine::{self, Outcome, Slot};
use obc_dfu::{decide, BootDecision, BootState, PAGE_LEN};

/// Base of the app slot — the app's vector table (see the repo layout in `memory.x`; the board
/// crate's `build.rs` links the app's `FLASH` at this exact origin, and its raw first words are
/// the initial MSP + reset vector this bootloader jumps through).
const APP_BASE: u32 = 0x0000_8000;

/// One past the end of the app slot — the BOOT_STATE page origin (`memory.x` mirrors the board
/// crate's layout). The install engine takes the slot size so IT owns the "padded image must
/// fit" gate, host-tested.
const APP_SLOT_END: u32 = 0x0017_B000;

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

/// Decide + install. Returns only when the machine should jump to the app; the `Installed`
/// (reset) and fatal (LED SOS park) outcomes diverge inside.
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

    // The rtt throughput meter reads the DWT cycle counter — start it (rtt builds only; the
    // shipping build carries none of this).
    #[cfg(feature = "rtt")]
    if let Some(mut cp) = cortex_m::Peripherals::take() {
        cp.DCB.enable_trace();
        cp.DWT.enable_cycle_counter();
    }

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
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD init failed — retrying in {=u32} ms", backoff);
                led.blink_code(3);
                led::delay_ms(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX_MS);
            }
            Some(blocks)
        }
        _ => None,
    };

    // Run the engine; a transient SD error mid-stream gets the same forever-retry treatment as
    // a missing card (state untouched by construction — host-tested), everything else is final.
    let slot = Slot { base: APP_BASE, len: APP_SLOT_END - APP_BASE };
    let mut buf = [0u8; INSTALL_BUF_LEN];
    let mut backoff = BACKOFF_MIN_MS;
    let outcome = loop {
        let mut io = install::BootIo::new(card.as_ref(), &mut rram, &mut led, boot_state_base());
        match engine::run(&state, &slot, &mut io, &mut buf) {
            Outcome::SdError => {
                #[cfg(feature = "rtt")]
                defmt::warn!("obc-boot: SD read failed mid-install — retrying in {=u32} ms", backoff);
                led.blink_code(3);
                led::delay_ms(backoff);
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
            led.blink_code(2);
        }
        // The slot holds the readback-verified image and the follow-up state is written: a
        // clean reset re-enters this bootloader, which sees Trial (or Idle after a rollback)
        // and jumps — the one trial boot.
        Outcome::Installed => {
            #[cfg(feature = "rtt")]
            defmt::info!("obc-boot: install complete — resetting into the new image");
            cortex_m::peripheral::SCB::sys_reset();
        }
        // Readback never matched (or the RRAM write path failed) after all retries. The state
        // page still holds the Armed record, so a power cycle retries the whole install; park
        // on SOS rather than reset-looping into a boot storm.
        Outcome::FlashError => {
            #[cfg(feature = "rtt")]
            defmt::error!("obc-boot: flash/readback failed after retries — halting (power cycle retries)");
            led.sos_forever();
        }
        // The retry loop above never breaks with SdError; keep the match total without a
        // panic path.
        Outcome::SdError => led.sos_forever(),
    }
}

/// Hand the machine to the app at [`APP_BASE`], matching the reset state as closely as
/// possible — the app was previously entered directly from reset and must not notice the
/// difference. Known deviations from a cold reset, both harmless: VTOR points at the app's
/// table instead of 0 (deliberate — that's the mechanism), and the FPU is already enabled
/// (this crate's own cortex-m-rt reset enabled CPACR; the app's reset handler re-enables it
/// idempotently).
fn jump_to_app() -> ! {
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
        (*cortex_m::peripheral::SCB::PTR).vtor.write(APP_BASE);
        cortex_m::asm::dsb();
        cortex_m::asm::isb();
        // 3.+4. `asm::bootload(APP_BASE)`: clears CONTROL.SPSEL (main stack — already the
        //    case here, matching reset), loads MSP from *(0x8000), and branches to
        //    *(0x8004)|1 (the app's reset vector, thumb bit set). Diverges — the bootloader
        //    is gone after this line.
        cortex_m::asm::bootload(APP_BASE as *const u32)
    }
}

/// The bootloader must never panic — every decode failure is an `Idle` by construction, the
/// engine is total over any page content (host-tested), and the IO adapters carry no unwraps.
/// If a panic happens anyway (a future logic bug), park the core rather than reset-looping
/// into a brick-flavoured boot storm; the LED staying dark after power-on is the field
/// symptom. (`rtt` builds swap this for panic-probe's printing handler.)
#[cfg(not(feature = "rtt"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
