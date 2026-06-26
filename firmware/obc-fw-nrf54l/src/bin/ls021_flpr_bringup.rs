//! **LS021 FLPR bring-up F0** (issue #150, epic #149) — dual-core build + boot path.
//!
//! The successor to the M33-direct LS021 bring-up (epic #139, `ls021_bringup`): the panel
//! protocol is cracked and holds on glass, so the FLPR epic now moves the waveform
//! generation onto the nRF54L15's **FLPR** (the VPR RISC-V coprocessor). F0 isolates the
//! riskiest unknown — *can we build, boot, and drive a pin from the FLPR at all* — **before**
//! any panel signal. No `PanelBus`, no COM, no glass; the M33-direct path is untouched.
//!
//! The M33 here is a tiny launcher:
//!   1. copies the freestanding **C blob** (`src/flpr/`, built by `build.rs`, `include_bytes!`'d
//!      below) into the FLPR's reserved SRAM region;
//!   2. writes a **magic** (`0xDEAD_BEEF`) into the shared handshake word, then releases the
//!      FLPR by setting `VPR00.INITPC` + `VPR00.CPURUN`;
//!   3. **polls** the handshake word and logs `FLPR alive` over RTT when the FLPR overwrites
//!      it with `0xA11E` — proof the FLPR booted, ran code, and reached shared SRAM.
//!
//! The FLPR blob then toggles **on-board LED0 (P2.09)** forever (visible by eye + on the
//! logic analyzer), while the M33 blinks **LED1 (P1.10)** as its own heartbeat — so two
//! blinking LEDs = both cores running concurrently. LED0 lives on **port P2**, the FLPR's
//! dedicated pin domain and the exact port the LS021 source bus uses, so a clean toggle here
//! also de-risks F2. See `firmware/docs/ls021-flpr.md` for the memory map + boot spec.
//!
//! Build/flash (the bin only compiles with its feature; needs a RISC-V gcc — `brew install
//! riscv64-elf-gcc`):
//! ```sh
//! cargo run --release --bin ls021_flpr_bringup --features ls021-flpr
//! ```

#![no_std]
#![no_main]

use defmt::{info, warn};
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_time::Timer;
use {defmt_rtt as _, panic_probe as _};

/// The FLPR program image, cross-compiled by `build.rs` into `$OUT_DIR/flpr.bin`.
static FLPR_BLOB: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/flpr.bin"));

// ── FLPR memory map (must match src/flpr/flpr.ld + the carved memory.x in build.rs) ──
/// FLPR execution base: the M33 copies the blob here and points `INITPC` at it.
const FLPR_RAM_BASE: usize = 0x2003_8000;
/// Cross-core handshake word: M33 writes `MAGIC`, FLPR overwrites with `ALIVE`.
const SHARED_HANDSHAKE: *mut u32 = 0x2003_F000 as *mut u32;
const HANDSHAKE_MAGIC: u32 = 0xDEAD_BEEF; // M33 → "core not started yet"
const HANDSHAKE_ALIVE: u32 = 0x0000_A11E; // FLPR → "alive" (matches flpr_blink.c)

// ── VPR00 control (secure alias 0x5004_C000; offsets from the nRF54L15 PAC) ──
const VPR00_INITPC: *mut u32 = 0x5004_C808 as *mut u32; // initial PC at core start
const VPR00_CPURUN: *mut u32 = 0x5004_C800 as *mut u32; // CPURUN.EN bit0 = run

/// Copy the blob into FLPR RAM, set the boot vector, and release the core from reset.
/// `INITPC` = the entry address; `CPURUN.EN = 1` starts execution there.
fn start_flpr() {
    unsafe {
        core::ptr::copy_nonoverlapping(FLPR_BLOB.as_ptr(), FLPR_RAM_BASE as *mut u8, FLPR_BLOB.len());
        // Make the blob + magic visible to the other core before we release it.
        cortex_m::asm::dsb();
        VPR00_INITPC.write_volatile(FLPR_RAM_BASE as u32);
        VPR00_CPURUN.write_volatile(1); // EN = 1 → FLPR runs from INITPC
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // Match the rest of the nRF firmware: run the M33 at its full 128 MHz.
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };

    // The M33 *owns the pin configuration* for LED0 (P2.09): set it to a driven output, low.
    // The FLPR then only ever pulses OUTSET/OUTCLR — atomic set/clear, never an OUT
    // read-modify-write — so the two cores never collide on shared port P2. Kept alive for the
    // life of the program (main never returns) so the pin stays configured as the FLPR drives it.
    let _led0 = Output::new(p.P2_09, Level::Low, OutputDrive::Standard);
    // LED1 (P1.10) is the M33's own heartbeat — proves the M33 keeps running alongside the FLPR.
    let mut led1 = Output::new(p.P1_10, Level::Low, OutputDrive::Standard);

    info!("LS021 FLPR F0: launcher up — blob is {=usize} bytes", FLPR_BLOB.len());

    // Arm the handshake, then release the FLPR.
    unsafe { SHARED_HANDSHAKE.write_volatile(HANDSHAKE_MAGIC) };
    start_flpr();
    info!("LS021 FLPR F0: FLPR released (INITPC=0x{=u32:08x}) — polling handshake", FLPR_RAM_BASE as u32);

    // Poll the handshake word (~1 s budget). The FLPR writes ALIVE as its first action, so this
    // normally fires within a few ms; a miss means the core never booted / can't reach shared RAM.
    let mut alive = false;
    for _ in 0..200 {
        if unsafe { SHARED_HANDSHAKE.read_volatile() } == HANDSHAKE_ALIVE {
            alive = true;
            break;
        }
        Timer::after_millis(5).await;
    }
    if alive {
        info!("LS021 FLPR F0: FLPR alive — handshake 0x{=u32:08x}. LED0 (P2.09) should be blinking.", HANDSHAKE_ALIVE);
    } else {
        warn!(
            "LS021 FLPR F0: no handshake (word=0x{=u32:08x}) — FLPR didn't boot or can't reach shared RAM; check INITPC / memory map (ls021-flpr.md)",
            unsafe { SHARED_HANDSHAKE.read_volatile() }
        );
    }

    // M33 heartbeat: blink LED1 forever. Two blinking LEDs (P2.09 by the FLPR, P1.10 here) =
    // both cores live and independent.
    loop {
        led1.toggle();
        Timer::after_millis(500).await;
    }
}
