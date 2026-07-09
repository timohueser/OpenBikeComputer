//! `obc-boot` — the 32 KB nRF54L bootloader (epic #615, S2 #617).
//!
//! Deliberately a dumb driver: read the BOOT_STATE RRAM page, let the host-tested `obc-dfu`
//! decode it (anything torn/blank/garbage ⇒ `Idle`) and decide, then hand the machine to the
//! app at [`APP_BASE`]. **Every decision resolves to a jump in S2** — the install engine
//! (verify → flash → trial → rollback) is S3 (#618). All format/decision logic stays upstream
//! in `obc-dfu` so this `main` stays small enough that review IS the verification; the
//! bootloader itself must never be able to panic on any page content.
//!
//! One short LED0 blink on entry is the visible proof the bootloader ran (S3 extends it into
//! blink codes). No executor, no timers, no FLPR — the app starts the FLPR core itself.

#![no_std]
#![no_main]

use cortex_m_rt::entry;
#[cfg(feature = "rtt")]
use {defmt_rtt as _, panic_probe as _};

use embassy_nrf::gpio::{Level, Output, OutputDrive};
use obc_dfu::{decide, BootDecision, BootState, PAGE_LEN};

/// Base of the app slot — the app's vector table (see the repo layout in `memory.x`; the board
/// crate's `build.rs` links the app's `FLASH` at this exact origin, and its raw first words are
/// the initial MSP + reset vector this bootloader jumps through).
const APP_BASE: u32 = 0x0000_8000;

/// LED0 (P2_09, active-HIGH — the DK's on-board LED, same one the app blinks per frame) held
/// on for ~100 ms at the default 64 MHz boot clock. Crude `asm::delay` busy-wait — the
/// bootloader has no timers on purpose.
const BLINK_CYCLES: u32 = 6_400_000;

/// The BOOT_STATE RRAM page, read in place. The address comes from the `__boot_state_base`
/// linker symbol (`ORIGIN(BOOT_STATE)` in `memory.x` — the app-side `build.rs` PROVIDEs the
/// same symbol for the armer), the same convention as the app's `__settings_base`: the magic
/// address lives only in the linker scripts. RRAM is plain memory-mapped for reads, so a
/// shared borrow of the page is sound; the bootloader never writes it in S2.
fn boot_state_page() -> &'static [u8; PAGE_LEN] {
    extern "C" {
        static __boot_state_base: u8;
    }
    unsafe { &*(core::ptr::addr_of!(__boot_state_base) as *const [u8; PAGE_LEN]) }
}

#[entry]
fn main() -> ! {
    // HAL init: trims, debug unlock, glitch-detector off, default clocks (internal HF osc,
    // 64 MHz — the app raises itself to 128 MHz with its own config after the jump). With no
    // `gpiote`/`time-driver` features compiled in, this enables **no interrupt sources**.
    // `FlprReset::Leave`: the bootloader never touches the FLPR coprocessor — the app owns
    // its whole lifecycle (its own init resets it, then loads + starts the panel blob).
    let mut config = embassy_nrf::config::Config::default();
    config.flpr_reset = embassy_nrf::config::FlprReset::Leave;
    let p = embassy_nrf::init(config);

    // One short blink: visible proof the bootloader ran, even with nothing else working.
    // Scoped so the `Output` drops before the jump — embassy-nrf's GPIO drop returns the pin
    // to its reset state (disconnected input), so the app inherits clean pins.
    {
        let mut led = Output::new(p.P2_09, Level::High, OutputDrive::Standard);
        cortex_m::asm::delay(BLINK_CYCLES);
        led.set_low();
    }

    // The whole decision matrix is pure, host-tested obc-dfu logic; garbage decodes to Idle.
    let state = BootState::decode(boot_state_page());
    #[cfg(feature = "rtt")]
    defmt::info!("obc-boot: generation={} → deciding", state.generation());
    match decide(&state) {
        BootDecision::Jump => {}
        // S3 (#618): verify the staged image over its extents, flash the app slot, write
        // Trial, reset. Until the install engine lands, fall through to the app — NEVER
        // todo!()/panic: a stray Armed page on an S2 bootloader must still boot the device.
        BootDecision::Install(_) => {}
        // S3 (#618): flash the rollback snapshot back, write Idle, reset.
        BootDecision::Rollback(_) => {}
        // S3 (#618): accept the running image (first-install case) and clear to Idle.
        BootDecision::AcceptAndClear => {}
    }

    jump_to_app()
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
        //    a bootloader HAL touched can vector into the app before it is ready. (With S2's
        //    feature set nothing was ever enabled — this pins the invariant for S3, whose
        //    RRAMC/SPIM drivers may enable lines.) PRIMASK is deliberately left CLEAR: we
        //    never set it, the app's cortex-m-rt entry does not re-enable interrupts, and at
        //    reset PRIMASK is clear — masking here would hand the app a dead interrupt system.
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

/// The bootloader must never panic — every decode failure is an `Idle` by construction and
/// every S2 decision is a jump. If a panic happens anyway (a future logic bug), park the core
/// rather than reset-looping into a brick-flavoured boot storm; the LED staying dark after
/// power-on is the field symptom. (`rtt` builds swap this for panic-probe's printing handler.)
#[cfg(not(feature = "rtt"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        cortex_m::asm::wfi();
    }
}
