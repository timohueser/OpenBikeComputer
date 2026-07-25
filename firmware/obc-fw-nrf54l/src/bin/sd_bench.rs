//! **Throwaway** microSD read-speed harness — measure the real SPI/embedded-sdmmc read path on
//! the DK against the actual `.obcm` on the card, and find the levers left on the table (issue:
//! renderer is SD-bound). Not shipped, not wired into anything — delete when the numbers are in.
//!
//!     cargo run --release --bin sd_bench
//!
//! ## What round 1 found (2026-07-15, on glass)
//!
//! - **`M16` on SERIAL22 == `M8`, byte for byte** (0.46 MB/s, 1106 µs/512 B block). The standard
//!   instance floors the prescaler at ÷2 — you cannot clock it past 8 MHz by asking. The real
//!   faster-than-8-MHz path is the unused high-speed SERIAL00/SPIM00 (32 MHz), which needs the SD rewired to
//!   the P2 high-speed pins. So the clock sweep is dropped here.
//! - **1106 µs to read ONE 512 B block at 8 MHz** — but the payload is only ~512 µs. Over half of
//!   every block is *clock-independent* overhead, and CMD18 batching bought +33 %.
//! - Disabling embedded-sdmmc's 10 µs poll backoff bought only ~4 %, so round 2 ruled that out as
//!   the main cost. The remaining floor is the per-block data-token handshake, CRC, and DMA setup.
//!
//! ## What the retained harness tests
//!
//! The bus stays at the proven 8 MHz ceiling and the negligible backoff stays disabled. Sequential
//! reads sweep batches 1/8/16/64 to show where CMD18's benefit plateaus; scattered reads compare
//! batch 1 with batch 8 for the shipping shapes.
//!
//! Shapes: `seq` (whole-file scan, throughput ceiling), `rand4k` (scattered 8-block reads = render
//! chunk misses), `rand512` (scattered 1-block reads = nav A*, #500 — always CMD17).
//!
//! ## Integrity guard
//! An FNV-1a-64 of the whole file, taken once at the safe stock/`b8` pass (the golden), is
//! re-checked on every `seq` pass. A mismatch = that config corrupted bytes — printed loudly.
//!
//! Card bring-up mirrors `sd::init` (250 kHz handshake, CS held LOW via the `NoCs` per-byte-CS
//! workaround); file location + extent resolve reuse the shipping `obc_storage` code, so the path
//! under the stopwatch is byte-for-byte the firmware's (only the delayer is swapped).
#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, Ordering};

use defmt::info;
use embassy_embedded_hal::SetConfig;
use embassy_executor::Spawner;
use embassy_nrf::gpio::{Level, Output, OutputDrive};
use embassy_nrf::spim::{Config as SpiConfig, Frequency, Spim};
use embassy_nrf::{bind_interrupts, peripherals, spim};
use embassy_time::{Delay, Instant};
use embedded_hal::delay::DelayNs;
use embedded_hal_bus::spi::ExclusiveDevice;
use embedded_sdmmc::{Block, BlockDevice, BlockIdx, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
use obc_storage::fat_extents::{ExtentTable, SharedBlockDevice};
use {defmt_rtt as _, panic_probe as _};

// Linking nrf-mpsl (via the default `ble` feature) provides the critical-section impl; the
// bench never inits MPSL — the impl works from reset.
use nrf_mpsl as _;

bind_interrupts!(struct Irqs {
    SERIAL00 => spim::InterruptHandler<peripherals::SERIAL00>;
});

/// The init-handshake clock config. SPIM00's 7-bit prescaler off the 128 MHz PLL floors at
/// ~1.008 MHz (÷127) — `K250`/`K500`/`M1` all truncate to a zero divisor and stop SCK dead (the
/// app's `sd.rs` has the full story). `M2` is a valid placeholder; the ÷127 floor is poked below.
const INIT_HZ: Frequency = Frequency::M2;
/// SPIM00's true minimum: 128 MHz / 127 ≈ 1.008 MHz, poked directly into PRESCALER for the acquire.
const INIT_DIVISOR: u8 = 127;

/// The bulk clock under test on SPIM00 — the referee for `sd::SD_FAST_HZ` (the FNV integrity
/// guard decides). M32 failed on-glass over DK jumpers (2026-07-24, app-level reads); bench at
/// M8 = the shipping setting. Hand-bump to M16/M32 here to probe the ceiling (esp. on the
/// production PCB's short traces).
const FAST_HZ: Frequency = Frequency::M8;

/// Scratch depth (blocks) = the largest batch. 64 × 512 = 32 KB — lives in a `.bss` static (see
/// [`SCRATCH`]), not on the executor task's stack.
const SCRATCH_BLOCKS: usize = 64;

/// The 32 KB read buffer, out of line of the stack. Written once at the top of `main`.
static mut SCRATCH: core::mem::MaybeUninit<[Block; SCRATCH_BLOCKS]> = core::mem::MaybeUninit::uninit();

/// Scattered-read sample count per config — clears the GRTC's ~1 µs granularity by 1000×.
const RAND_SAMPLES: usize = 400;

/// Whether the card's poll-loop delayer injects embedded-sdmmc's stock 10 µs backoff. Flipped
/// between matrix halves; the whole point of the round.
static BACKOFF: AtomicBool = AtomicBool::new(true);

/// The card's poll delayer. When [`BACKOFF`] is set it blocks the requested time (the stock
/// behaviour — `read_data`/`card_command`/`wait_not_busy` call `delay_us(10)` per idle poll);
/// when clear it's a no-op, so the poll spins as fast as the SPI byte-reads themselves.
struct PollDelay;
impl DelayNs for PollDelay {
    fn delay_ns(&mut self, ns: u32) {
        if BACKOFF.load(Ordering::Relaxed) {
            Delay.delay_ns(ns);
        }
    }
}

/// A NullTime for the `VolumeManager` (used only to locate the map's directory entry; no writes).
struct NullTime;
impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// The no-op chip-select — the real CS (P2_10) is held LOW for the session, exactly as `sd::NoCs`.
struct NoCs;
impl embedded_hal::digital::ErrorType for NoCs {
    type Error = core::convert::Infallible;
}
impl embedded_hal::digital::OutputPin for NoCs {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

type SdDev = ExclusiveDevice<Spim<'static>, NoCs, Delay>;
type Sd = SdCard<SdDev, PollDelay>;

/// A file's extent layout, flattened from [`ExtentTable::runs`] into `(file_block_start, lba,
/// blocks)` so a file-block index maps to an absolute LBA with a run-boundary-aware read span.
struct Layout {
    runs: heapless::Vec<(u32, u32, u32), { obc_storage::fat_extents::MAX_EXTENTS }>,
    file_blocks: u32,
}

impl Layout {
    fn from_table(t: &ExtentTable) -> Self {
        let mut runs = heapless::Vec::new();
        let mut file_start = 0u32;
        for (lba, blocks) in t.runs() {
            let _ = runs.push((file_start, lba, blocks));
            file_start += blocks;
        }
        Layout { runs, file_blocks: file_start }
    }

    /// The absolute LBA of `file_block`, and how many blocks remain contiguous on disk from it
    /// (to the end of its extent run) — the cap on one CMD18 batch.
    fn map(&self, file_block: u32) -> Option<(u32, u32)> {
        let i = self.runs.partition_point(|&(fs, _, _)| fs <= file_block).checked_sub(1)?;
        let (fs, lba, blocks) = self.runs[i];
        let into = file_block - fs;
        (into < blocks).then(|| (lba + into, blocks - into))
    }
}

/// Read `n_blocks` from file block `start`, in `batch`-block reads (clamped to the disk run and
/// scratch), folding bytes through FNV-1a-64 so reads can't be elided and `seq` can be checked.
fn read_span(card: &Sd, lay: &Layout, scratch: &mut [Block], start: u32, n_blocks: u32) -> u64 {
    let batch = scratch.len().min(SCRATCH_BLOCKS) as u32;
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut fb = start;
    let end = start + n_blocks;
    while fb < end {
        let Some((lba, run_left)) = lay.map(fb) else { break };
        let take = batch.min(run_left).min(end - fb) as usize;
        let slice = &mut scratch[..take];
        if card.read(slice, BlockIdx(lba)).is_err() {
            info!("  READ ERROR at lba {=u32}", lba);
            break;
        }
        for b in slice.iter() {
            for &byte in b.contents.iter() {
                hash = (hash ^ byte as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        fb += take as u32;
    }
    hash
}

/// MB/s ×100 as an integer (bytes/µs == MB/s).
fn mbps_x100(bytes: u64, us: u64) -> u64 {
    bytes.saturating_mul(100).checked_div(us).unwrap_or_default()
}

/// Re-clock the bulk bus (the `SetConfig` seam `sd::init` uses), `orc = 0xFF`.
fn reclock(card: &Sd, freq: Frequency) {
    card.spi(|dev| {
        let mut cfg = SpiConfig::default();
        cfg.frequency = freq;
        cfg.orc = 0xFF;
        let _ = dev.bus_mut().set_config(&cfg);
    });
}

/// A tiny deterministic xorshift64 PRNG — reproducible scatter without `rand`/std.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn block(&mut self, blocks: u32, span: u32) -> u32 {
        let hi = blocks.saturating_sub(span).max(1);
        (self.next() % hi as u64) as u32
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let p = {
        let mut config = embassy_nrf::config::Config::default();
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
    info!("sd_bench: bring-up");

    let mut sd_cfg = SpiConfig::default();
    sd_cfg.frequency = INIT_HZ;
    sd_cfg.orc = 0xFF;
    let mut spi = Spim::new(p.SERIAL00, Irqs, p.P2_06, p.P2_09, p.P2_08, sd_cfg);
    // The ~1 MHz init floor (see INIT_DIVISOR): poked raw — no embassy Frequency reaches it.
    embassy_nrf::pac::SPIM00.prescaler().write(|w| w.set_divisor(INIT_DIVISOR));
    // Mirror main.rs's fast-pad setup: 32 MHz needs extra-high drive (E0/E1) on SCK/MOSI + the
    // highest HS-pad slew — without it the timing budget caps near 16 MHz.
    {
        use embassy_nrf::pac;
        use embassy_nrf::pac::gpio::vals::Drive;
        for pin in [6usize, 8, 9] {
            pac::P2_S.pin_cnf(pin).modify(|w| {
                w.set_drive0(Drive::E);
                w.set_drive1(Drive::E);
            });
        }
        pac::GPIOHSPADCTRL_S.bias().modify(|w| w.set_hsbias(3));
    }
    let mut cs = Output::new(p.P2_10, Level::High, OutputDrive::Standard);
    cs.set_high();
    let _ = spi.blocking_write(&[0xFFu8; 10]);
    cs.set_low();
    // `NoCs`'s error is `Infallible`, so the device build can't fail.
    let Ok(dev) = ExclusiveDevice::new(spi, NoCs, Delay);
    let card: Sd = SdCard::new(dev, PollDelay);
    match card.num_bytes() {
        Ok(bytes) => info!("sd_bench: card up, {=u64} MB", bytes >> 20),
        Err(_) => {
            info!("sd_bench: no card / init failed");
            return;
        }
    }
    reclock(&card, FAST_HZ);

    // Locate the map (first root file with short-ext `OBC`) and resolve its FAT chain to extents.
    let vmgr: VolumeManager<SharedBlockDevice<Sd>, NullTime, 4, 4, 1> =
        VolumeManager::new_with_limits(SharedBlockDevice(&card), NullTime, 5000);
    let Ok(volume) = vmgr.open_raw_volume(VolumeIdx(0)) else {
        info!("sd_bench: no FAT volume");
        return;
    };
    let Ok(root) = vmgr.open_root_dir(volume) else {
        info!("sd_bench: no root dir");
        return;
    };
    let mut facts: Option<(BlockIdx, u32, u32)> = None;
    let _ = vmgr.iterate_dir(root, |e| {
        if facts.is_none() && !e.attributes.is_directory() && e.name.extension() == b"OBC" {
            facts = Some((e.entry_block, e.entry_offset, e.size));
        }
    });
    let Some((eb, eo, len)) = facts else {
        info!("sd_bench: no .obcm in root");
        return;
    };
    info!("sd_bench: map found, {=u32} bytes", len);
    let table = match ExtentTable::build(&card, eb, eo, len) {
        Ok(t) => t,
        Err(e) => {
            info!("sd_bench: extent build refused: {}", defmt::Debug2Format(&e));
            return;
        }
    };
    info!("sd_bench: {=usize} extent run(s)", table.extent_count());
    let lay = Layout::from_table(&table);
    // SAFETY: sole access to SCRATCH; single-threaded bench, written once here before any use.
    let scratch: &mut [Block; SCRATCH_BLOCKS] = unsafe {
        let p = core::ptr::addr_of_mut!(SCRATCH);
        (*p).write(core::array::from_fn(|_| Block::new()));
        (*p).assume_init_mut()
    };
    let total_blocks = lay.file_blocks;
    let total_bytes = total_blocks as u64 * 512;

    // Backoff proven negligible in round 2 (~4 %); run with it off and hold it fixed. The point
    // of this round is the batch **asymptote** — how far CMD18 can push before a hard per-block
    // floor (payload + data-token handshake) stops it.
    BACKOFF.store(false, Ordering::Relaxed);

    // Golden: whole file at batch 8.
    let golden = read_span(&card, &lay, &mut scratch[..8], 0, total_blocks);
    info!("sd_bench: golden fnv = {=u64:x}, {=u32} blocks @ 8 MHz", golden, total_blocks);
    info!("");
    info!("shape    batch   MB/s      per-read   integrity");

    // seq — whole-file scan, climbing the CMD18 batch to find the floor. b1 = CMD17/block.
    for &batch in &[1usize, 8, 16, 64] {
        let t0 = Instant::now();
        let hash = read_span(&card, &lay, &mut scratch[..batch], 0, total_blocks);
        let us = t0.elapsed().as_micros();
        let integ = if hash == golden { "OK" } else { "CORRUPT!" };
        let m = mbps_x100(total_bytes, us);
        let per = us / (total_blocks as u64).max(1);
        info!(
            "seq      b{=usize}    {=u64}.{=u64:02} MB/s  {=u64} us/blk  {=str}",
            batch,
            m / 100,
            m % 100,
            per,
            integ
        );
    }

    // rand4k — scattered 8-block (4 KB) reads (render chunk misses): CMD17/block vs one CMD18.
    for &batch in &[1usize, 8] {
        let mut rng = Rng(0x1234_5678_9abc_def0);
        let span = 8u32;
        let t0 = Instant::now();
        for _ in 0..RAND_SAMPLES {
            let fb = rng.block(total_blocks, span);
            let _ = read_span(&card, &lay, &mut scratch[..batch], fb, span);
        }
        let us = t0.elapsed().as_micros();
        let bytes = RAND_SAMPLES as u64 * span as u64 * 512;
        let m = mbps_x100(bytes, us);
        let per = us / RAND_SAMPLES as u64;
        info!("rand4k   b{=usize}    {=u64}.{=u64:02} MB/s  {=u64} us/chunk", batch, m / 100, m % 100, per);
    }

    // rand512 — scattered single-block reads (nav A*, #500). Always CMD17.
    {
        let mut rng = Rng(0x0f0f_0f0f_1111_2222);
        let t0 = Instant::now();
        for _ in 0..RAND_SAMPLES {
            let fb = rng.block(total_blocks, 1);
            let _ = read_span(&card, &lay, &mut scratch[..1], fb, 1);
        }
        let us = t0.elapsed().as_micros();
        let m = mbps_x100(RAND_SAMPLES as u64 * 512, us);
        let per = us / RAND_SAMPLES as u64;
        info!("rand512  b1    {=u64}.{=u64:02} MB/s  {=u64} us/read", m / 100, m % 100, per);
    }

    info!("sd_bench: done");
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(3600)).await;
    }
}
