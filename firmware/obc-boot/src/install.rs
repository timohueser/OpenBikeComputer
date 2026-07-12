//! The [`obc_dfu::engine::InstallIo`] wiring: real SPI block reads + RRAMC line writes behind
//! the host-tested install engine. This file is deliberately free of *sequencing* — ordering,
//! retries, and failure policy all live (and are unit-tested) in `obc-dfu`; everything here is
//! a one-line adapter plus the LED heartbeat and the `rtt` throughput meter.

use embassy_nrf::rramc::{Rramc, Unbuffered};
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use obc_dfu::engine::{InstallIo, IoError, Phase};
use obc_dfu::BootState;

use crate::com::Com;
use crate::led::Led;
use crate::sd::SdBlocks;
use crate::wdt::BootDog;

/// Toggle the LED every this many progress chunks (~4 KB each) while verifying — the *slow*
/// heartbeat (~1 s period at SD bulk speed; the exact rate scales with card throughput).
const VERIFY_TOGGLE_CHUNKS: u32 = 64;
/// Toggle every this many chunks while flashing / reading back — the *fast* heartbeat.
const FLASH_TOGGLE_CHUNKS: u32 = 8;

/// The real IO the install engine drives.
pub struct BootIo<'a> {
    /// The raw-block card — `None` for decisions that never stream extents (`AcceptAndClear`),
    /// which is why `read_blocks` on `None` is unreachable rather than a real error path.
    sd: Option<&'a SdBlocks>,
    rram: &'a mut Rramc<'static, Unbuffered>,
    led: &'a mut Led,
    /// The boot-chain watchdog (DR1, #729) — pet from [`progress`](InstallIo::progress), the
    /// one callback guaranteed to fire steadily through all three passes.
    dog: &'a mut BootDog,
    /// The panel's COM wave (`com.rs`) — polled from the same steady progress hook, so the
    /// glass under the app's pre-painted frame keeps alternating through the whole install
    /// (chunks land every few ms, well inside the ~8.3 ms half period).
    com: &'a mut Com,
    /// Address of the BOOT_STATE page (from the linker symbol — resolved once in `main`).
    state_addr: u32,
    chunks: u32,
    #[cfg(feature = "rtt")]
    meter: Meter,
}

impl<'a> BootIo<'a> {
    pub fn new(
        sd: Option<&'a SdBlocks>,
        rram: &'a mut Rramc<'static, Unbuffered>,
        led: &'a mut Led,
        dog: &'a mut BootDog,
        com: &'a mut Com,
        state_addr: u32,
    ) -> BootIo<'a> {
        BootIo {
            sd,
            rram,
            led,
            dog,
            com,
            state_addr,
            chunks: 0,
            #[cfg(feature = "rtt")]
            meter: Meter::new(),
        }
    }
}

impl InstallIo for BootIo<'_> {
    fn read_blocks(&mut self, start_block: u32, buf: &mut [u8]) -> Result<(), IoError> {
        self.sd.ok_or(IoError)?.read_blocks(start_block, buf).map_err(|_| IoError)
    }

    /// Blocking RRAMC line writes to the app slot. The engine only ever hands us 16-byte-aligned
    /// spans, matching `Rramc::write`'s `WRITE_SIZE`; the write path is the same idiom as the
    /// app's `RramSettingsStore` (`obc-fw-nrf54l/src/settings.rs`) — deliberate duplication, the
    /// crates don't share an RRAM module (same policy as the SD constants in `sd.rs`).
    fn write_lines(&mut self, addr: u32, data: &[u8]) -> Result<(), IoError> {
        self.rram.write(addr, data).map_err(|_| IoError)
    }

    /// Readback straight off the memory map (RRAM is XIP-readable; `Rramc::read` is exactly a
    /// volatile-free slice copy from the absolute address).
    fn read_flash(&mut self, addr: u32, buf: &mut [u8]) -> Result<(), IoError> {
        self.rram.read(addr, buf).map_err(|_| IoError)
    }

    /// Persist a boot state into the BOOT_STATE page. `EncodedPage` is a whole number of RRAM
    /// lines by construction, so this is one aligned multi-line write; stale bytes of a longer
    /// previous blob past the new `blob_len` are outside the CRC frame and ignored by decode.
    fn write_state(&mut self, state: &BootState) -> Result<(), IoError> {
        let page = state.encode();
        self.rram.write(self.state_addr, page.as_bytes()).map_err(|_| IoError)
    }

    fn progress(&mut self, phase: Phase, done: u32, total: u32) {
        // DR1 (#729): pet the adopted/inherited watchdog every chunk (one register write per
        // ≤4 KB — effectively free), so an install stretched past 24 s by a slow card, flash
        // retries, or a near-max image can never be dog-reset mid-flash.
        self.dog.pet();
        // Keep the panel's COM wave alternating with the same cadence (one compare + at most
        // three pin writes) — the held "Installing update" frame's anti-DC-bias contract.
        self.com.poll();
        self.chunks += 1;
        let period = match phase {
            Phase::Verify => VERIFY_TOGGLE_CHUNKS,
            Phase::Flash | Phase::Readback => FLASH_TOGGLE_CHUNKS,
        };
        if self.chunks.is_multiple_of(period) {
            self.led.toggle();
        }
        #[cfg(feature = "rtt")]
        self.meter.tick(phase, done, total);
        #[cfg(not(feature = "rtt"))]
        let _ = (done, total);
    }
}

/// Per-phase wall-time + throughput over the DWT cycle counter (`rtt` builds only — the size
/// budget is measured without it). Chunks are ≤4 KB, far inside the 32-bit counter's ~67 s wrap
/// at 64 MHz, so accumulating per-tick `wrapping_sub` deltas into a u64 is exact.
#[cfg(feature = "rtt")]
struct Meter {
    phase: Option<Phase>,
    last: u32,
    accum: u64,
    bytes: u32,
}

#[cfg(feature = "rtt")]
impl Meter {
    fn new() -> Meter {
        Meter { phase: None, last: 0, accum: 0, bytes: 0 }
    }

    fn tick(&mut self, phase: Phase, done: u32, total: u32) {
        let now = cortex_m::peripheral::DWT::cycle_count();
        if self.phase != Some(phase) {
            self.report();
            self.phase = Some(phase);
            self.accum = 0;
        } else {
            self.accum += u64::from(now.wrapping_sub(self.last));
        }
        self.last = now;
        self.bytes = total;
        if done == total && total > 0 {
            self.report();
            self.phase = None;
        }
    }

    fn report(&mut self) {
        let Some(phase) = self.phase else { return };
        let ms = self.accum / 64_000; // 64 MHz boot clock
        let kib = self.bytes / 1024;
        let kib_s = (u64::from(self.bytes) * 1000 / 1024).checked_div(ms).unwrap_or(0);
        let name = match phase {
            Phase::Verify => "verify",
            Phase::Flash => "flash",
            Phase::Readback => "readback",
        };
        defmt::info!("obc-boot: {=str} {=u32} KiB in {=u64} ms ({=u64} KiB/s)", name, kib, ms, kib_s);
    }
}
