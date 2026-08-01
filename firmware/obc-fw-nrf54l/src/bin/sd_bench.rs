//! **Throwaway** microSD speed harness — measure the real SPI/embedded-sdmmc read *and* write
//! paths on the DK against the actual `.obcm` on the card, and find the levers left on the table
//! (issues: renderer is SD-bound; map upload is SD-write-bound). Not shipped, not wired into
//! anything — delete when the numbers are in.
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
//! ## The write shapes (map upload, #889)
//!
//! A map upload writes 512 B per `VolumeManager::write` — one `BlockCache::write_back`, so one
//! CMD24 + program + CMD13 per block, which is why it lands ~10× under the wire. These rows price
//! the alternative:
//!
//! - `wr-fat  b1` — the shipping upload loop byte for byte (`usb::data_plane` → `object_store` →
//!   `vmgr.write`). The baseline to beat.
//! - `wr-raw  b1` — the same one-block write straight at the device. The gap against `wr-fat b1`
//!   is the FAT layer's bookkeeping (chain walk, cache, length fixups), not the card.
//! - `wr-raw  b8..b64` — multi-block writes, which `SdCard::write` turns into ACMD23 + CMD25.
//!   **Unreachable from the FAT layer today**: 0.9's `BlockCache` holds exactly one block, so
//!   `VolumeManager` can never hand the device more than one and the crate's own multi-block path
//!   is dead code from above. These rows are the ceiling a patched `write()` would unlock — run
//!   them before writing the patch, not after.
//!
//! The write shapes own a scratch file (`BENCH.TMP`, [`BENCH_BLOCKS`] × 512 B), created by the
//! `wr-fat` pass and deleted at the end; the map is never written to. Every pass rewrites the
//! **same** extents with a fresh salt, while a real upload lands on freshly allocated clusters —
//! so a card that treats a rewrite differently from a first write reads a little pessimistic
//! here. The ranking across batches is the number to trust.
//!
//! ## Integrity guard
//! An FNV-1a-64 of the whole file, taken once at the safe stock/`b8` pass (the golden), is
//! re-checked on every `seq` pass. A mismatch = that config corrupted bytes — printed loudly.
//! The write passes carry their own guard: every block is stamped with its file-block index and
//! the pass's salt, then read back and checked, so a multi-block write that lands one block off
//! or silently drops its tail is caught rather than averaged into a flattering number.
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
use embedded_sdmmc::{Block, BlockDevice, BlockIdx, Mode, SdCard, TimeSource, Timestamp, VolumeIdx, VolumeManager};
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
const FAST_HZ: Frequency = Frequency::M16;

/// Scratch depth (blocks) = the largest batch. 64 × 512 = 32 KB — lives in a `.bss` static (see
/// [`SCRATCH`]), not on the executor task's stack.
const SCRATCH_BLOCKS: usize = 64;

/// The 32 KB read buffer, out of line of the stack. Written once at the top of `main`.
static mut SCRATCH: core::mem::MaybeUninit<[Block; SCRATCH_BLOCKS]> = core::mem::MaybeUninit::uninit();

/// Scattered-read sample count per config — clears the GRTC's ~1 µs granularity by 1000×.
const RAND_SAMPLES: usize = 400;

/// Blocks in the write scratch file: 8192 × 512 B = 4 MB. Big enough to cross the card's
/// allocation units several times (so the numbers are steady-state, not the first-page special
/// case), small enough that the two unbatched passes finish in ~20 s each.
const BENCH_BLOCKS: u32 = 8192;

/// The scratch file the write shapes own — created by the `wr-fat` pass, deleted at the end.
/// Short 8.3 name so it needs no LFN entries.
const BENCH_FILE: &str = "BENCH.TMP";

/// Salt for the `wr-fat` pass's stamps (the `wr-raw` passes derive theirs from the batch), so a
/// pass that silently writes nothing fails the read-back against the *previous* pass's salt
/// instead of quietly passing on stale bytes.
const FAT_SALT: u32 = 0xF0F0_0001;

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

/// A NullTime for the `VolumeManager` — it locates the map's directory entry and stamps the
/// scratch file's. A zero timestamp on a file this bench also deletes is nobody's problem.
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

/// The fill byte block `fb` carries under `salt` — cheap enough (one memset) that stamping never
/// shows up next to a millisecond-scale card write.
fn fill_byte(fb: u32, salt: u32) -> u8 {
    (fb ^ salt) as u8 ^ (fb >> 8) as u8
}

/// Stamp block `fb` for pass `salt`: the first 8 bytes carry the identity, the rest is the fill.
/// Identity in the payload is what turns "the bytes came back" into "the *right* bytes came back
/// from the *right* LBA" — the failure a multi-block write can have and a single-block one can't.
fn stamp(b: &mut Block, fb: u32, salt: u32) {
    b.contents.fill(fill_byte(fb, salt));
    b.contents[..4].copy_from_slice(&fb.to_le_bytes());
    b.contents[4..8].copy_from_slice(&salt.to_le_bytes());
}

/// Check a block read back against its stamp: identity, plus the fill at both ends of the payload
/// (a torn write that only landed its head fails the tail check).
fn check(b: &Block, fb: u32, salt: u32) -> bool {
    let fill = fill_byte(fb, salt);
    b.contents[..4] == fb.to_le_bytes()
        && b.contents[4..8] == salt.to_le_bytes()
        && b.contents[8] == fill
        && b.contents[Block::LEN - 1] == fill
}

/// Write file blocks `0..n_blocks` in `scratch`-sized batches (clamped to the disk run), stamping
/// each with `salt`. Returns the microseconds spent **inside** `BlockDevice::write` — the stamping
/// sits outside the stopwatch on purpose, so the row is the card's cost and not the CPU's.
/// `None` = the card refused a write (already logged).
fn write_span(card: &Sd, lay: &Layout, scratch: &mut [Block], n_blocks: u32, salt: u32) -> Option<u64> {
    let batch = scratch.len() as u32;
    let mut us = 0u64;
    let mut fb = 0u32;
    while fb < n_blocks {
        let (lba, run_left) = lay.map(fb)?;
        let take = batch.min(run_left).min(n_blocks - fb) as usize;
        let slice = &mut scratch[..take];
        for (i, b) in slice.iter_mut().enumerate() {
            stamp(b, fb + i as u32, salt);
        }
        let t0 = Instant::now();
        let failed = card.write(slice, BlockIdx(lba)).is_err();
        us += t0.elapsed().as_micros();
        if failed {
            info!("  WRITE ERROR at lba {=u32} (file block {=u32})", lba, fb);
            return None;
        }
        fb += take as u32;
    }
    Some(us)
}

/// Read the whole span back and check every block's stamp. Batched hard — verification is not
/// under measurement, and at b1 it would double the pass.
fn verify_span(card: &Sd, lay: &Layout, scratch: &mut [Block], n_blocks: u32, salt: u32) -> bool {
    let batch = scratch.len() as u32;
    let mut fb = 0u32;
    while fb < n_blocks {
        let Some((lba, run_left)) = lay.map(fb) else { return false };
        let take = batch.min(run_left).min(n_blocks - fb) as usize;
        let slice = &mut scratch[..take];
        if card.read(slice, BlockIdx(lba)).is_err() {
            info!("  READBACK ERROR at lba {=u32}", lba);
            return false;
        }
        for (i, b) in slice.iter().enumerate() {
            if !check(b, fb + i as u32, salt) {
                info!("  MISMATCH at file block {=u32} (lba {=u32})", fb + i as u32, lba);
                return false;
            }
        }
        fb += take as u32;
    }
    true
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

/// Re-clock to an arbitrary PLL divisor — the same raw PRESCALER poke `sd::init` uses for the
/// ~1 MHz floor, because embassy's `Frequency` enum only names the powers of two. This is what
/// opens the two rungs between M16 and M32: ÷6 = 21.3 MHz and **÷5 = 25.6 MHz** — the latter
/// right at the SD spec's 25 MHz SPI ceiling, where M32 (÷4) is a straight overclock (and the
/// prime suspect for its 2026-07-24 on-glass failure).
fn reclock_div(card: &Sd, divisor: u8) {
    reclock(card, Frequency::M2); // any valid config; the poke below overrides the divisor
    embassy_nrf::pac::SPIM00.prescaler().write(|w| w.set_divisor(divisor));
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

    // Blocks per cluster, straight out of the BPB. A `VolumeManager::write` can never batch wider
    // than one cluster — that is the largest span the FAT guarantees is contiguous without walking
    // the chain — so this number says which `wr-raw` row below is the *reachable* one and which is
    // merely the card's ceiling. Read before the clock goes fast, while the bus is still gentle.
    {
        let mut b = [Block::new()];
        let mut spc = 0u8;
        if card.read(&mut b, BlockIdx(0)).is_ok() {
            // MBR partition entry 0 (offset 0x1BE): first LBA at +8, little-endian.
            let e = &b[0].contents[0x1BE..];
            let lba = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
            if card.read(&mut b, BlockIdx(lba)).is_ok() {
                let vbr = &b[0].contents;
                // Trust the field only from something that looks like a boot record.
                if vbr[510] == 0x55 && vbr[511] == 0xAA && vbr[0x0D].is_power_of_two() {
                    spc = vbr[0x0D];
                }
            }
        }
        if spc == 0 {
            info!("sd_bench: cluster size unreadable — treat every wr-raw row as a ceiling");
        } else {
            info!(
                "sd_bench: {=u8} blocks/cluster ({=u32} KB) = the widest batch a vmgr can issue",
                spc,
                spc as u32 / 2
            );
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
    // (No frequency in this line: the bulk bus is whatever `FAST_HZ` is set to, and a hardcoded
    // "8 MHz" outlived the move to SPIM00 once already.)
    info!("sd_bench: golden fnv = {=u64:x}, {=u32} blocks", golden, total_blocks);
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

    // ==================== write shapes (map upload, #889) ====================
    // Last on purpose: a write pass that wedges the card must not cost the read numbers.
    info!("");
    info!("shape     batch   MB/s      per-blk    integrity");

    let bench_bytes = BENCH_BLOCKS as u64 * 512;

    // wr-fat b1 — the shipping upload loop. Creating the scratch file *is* the measurement, so
    // the 4 MB is not written twice. `close_file` flushes the dir entry after the stopwatch: that
    // cost is one-off per upload, not per block, and folding it in would flatter the batches.
    let Ok(file) = vmgr.open_file_in_dir(root, BENCH_FILE, Mode::ReadWriteCreateOrTruncate) else {
        info!("sd_bench: cannot create {=str} — write shapes skipped", BENCH_FILE);
        return;
    };
    let mut fat_us = 0u64;
    let mut fat_ok = true;
    for fb in 0..BENCH_BLOCKS {
        stamp(&mut scratch[0], fb, FAT_SALT);
        let t0 = Instant::now();
        let failed = vmgr.write(file, &scratch[0].contents).is_err();
        fat_us += t0.elapsed().as_micros();
        if failed {
            info!("  WRITE ERROR at file block {=u32}", fb);
            fat_ok = false;
            break;
        }
    }
    let _ = vmgr.close_file(file);
    if !fat_ok {
        let _ = vmgr.delete_file_in_dir(root, BENCH_FILE);
        return;
    }

    // Resolve the scratch file's chain, so the raw passes know which LBAs are theirs to scribble
    // on. Same code the map takes — a raw write outside these extents would corrupt the volume.
    let mut bfacts: Option<(BlockIdx, u32, u32)> = None;
    let _ = vmgr.iterate_dir(root, |e| {
        if bfacts.is_none() && !e.attributes.is_directory() && e.name.base_name() == b"BENCH" {
            bfacts = Some((e.entry_block, e.entry_offset, e.size));
        }
    });
    let Some((beb, beo, blen)) = bfacts else {
        info!("sd_bench: scratch file vanished after write");
        return;
    };
    let Ok(btable) = ExtentTable::build(&card, beb, beo, blen) else {
        info!("sd_bench: scratch extents refused — raw write shapes skipped");
        let _ = vmgr.delete_file_in_dir(root, BENCH_FILE);
        return;
    };
    let blay = Layout::from_table(&btable);

    {
        let m = mbps_x100(bench_bytes, fat_us);
        let integ = if verify_span(&card, &blay, &mut scratch[..], BENCH_BLOCKS, FAT_SALT) { "OK" } else { "CORRUPT!" };
        info!(
            "wr-fat    b1     {=u64}.{=u64:02} MB/s  {=u64} us/blk  {=str}",
            m / 100,
            m % 100,
            fat_us / BENCH_BLOCKS as u64,
            integ
        );
    }

    // wr-raw — the same blocks straight at the device. b1 isolates the FAT bookkeeping; b8..b64
    // are ACMD23+CMD25, the path `VolumeManager` cannot currently reach.
    for &batch in &[1usize, 8, 32, 64] {
        let salt = 0x5A5A_0000 ^ batch as u32;
        let Some(us) = write_span(&card, &blay, &mut scratch[..batch], BENCH_BLOCKS, salt) else { break };
        let integ = if verify_span(&card, &blay, &mut scratch[..], BENCH_BLOCKS, salt) { "OK" } else { "CORRUPT!" };
        let m = mbps_x100(bench_bytes, us);
        info!(
            "wr-raw    b{=usize}    {=u64}.{=u64:02} MB/s  {=u64} us/blk  {=str}",
            batch,
            m / 100,
            m % 100,
            us / BENCH_BLOCKS as u64,
            integ
        );
    }

    // ==================== clock sweep (the ÷5 / ÷4 question) ====================
    // The two rungs embassy's `Frequency` can't name, plus the known M32 overclock, each guarded:
    // a whole-map `seq` read checked against the golden FNV, and a `wr-raw b32` pass into the
    // scratch file whose stamps are verified back at the *proven* ÷8 clock — so a corrupt verify
    // indicts the write clock, not the read-back. A clock that corrupts either is disqualified;
    // the sweep continues so one flash still prices every rung.
    // ⚠️ **Even divisors only.** ÷5 (25.6 MHz) hard-wedged the bus on glass (2026-07-30): an odd
    // divisor cannot make a symmetric SCK, and at speed the asymmetry stops the transfer's END
    // event ever firing — the blocking spim spins forever, no recovery short of a power cycle.
    // (The ÷127 init floor survives being odd because ~1 MHz doesn't care about duty cycle.)
    // So the ladder above ÷8 is ÷6 = 21.3 (passed both guards, 2026-07-30) and ÷4 = 32.
    info!("");
    info!("clock sweep (reads vs golden fnv; writes verified at /8)");
    for &(label, div) in &[("21.3MHz(/6)", 6u8), ("32MHz(/4)", 4)] {
        reclock_div(&card, div);
        let t0 = Instant::now();
        let hash = read_span(&card, &lay, &mut scratch[..64], 0, total_blocks);
        let us = t0.elapsed().as_micros();
        let m = mbps_x100(total_bytes, us);
        let integ = if hash == golden { "OK" } else { "CORRUPT!" };
        info!(
            "{=str}  seq-rd b64   {=u64}.{=u64:02} MB/s  {=u64} us/blk  {=str}",
            label,
            m / 100,
            m % 100,
            us / (total_blocks as u64).max(1),
            integ
        );

        let salt = 0xC10C_0000 ^ div as u32;
        let wrote = write_span(&card, &blay, &mut scratch[..32], BENCH_BLOCKS, salt);
        reclock_div(&card, 8); // verify on the proven clock, whatever the write did
        match wrote {
            Some(us) => {
                let ok = verify_span(&card, &blay, &mut scratch[..], BENCH_BLOCKS, salt);
                let m = mbps_x100(bench_bytes, us);
                let integ = if ok { "OK" } else { "CORRUPT!" };
                info!(
                    "{=str}  wr-raw b32   {=u64}.{=u64:02} MB/s  {=u64} us/blk  {=str}",
                    label,
                    m / 100,
                    m % 100,
                    us / BENCH_BLOCKS as u64,
                    integ
                );
            }
            None => info!("{=str}  wr-raw b32   write refused — clock disqualified", label),
        }
    }

    // The raw passes wrote under the FAT layer, so the vmgr's one-block cache may hold a stale
    // *data* block of the scratch file — the delete only touches its dir entry and the FAT, which
    // no raw write went near.
    let _ = vmgr.delete_file_in_dir(root, BENCH_FILE);

    info!("sd_bench: done");
    loop {
        embassy_time::Timer::after(embassy_time::Duration::from_secs(3600)).await;
    }
}
