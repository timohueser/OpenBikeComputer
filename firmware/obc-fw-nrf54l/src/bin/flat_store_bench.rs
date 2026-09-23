//! Flat store bench: the whole of `obc_storage::flat` on the real card.
//!
//!     cargo run --release --bin flat_store_bench
//!
//! `FLAT_Store_Format.md` states five figures that only the board can check: a first boot "well
//! under a second", a mount "about 100 ms", a commit "about 15-20 ms at a few hundred entries", a
//! read path that is arithmetic rather than a chain walk, and a resident cost of the 8 KiB free
//! bitmap plus the ride tail. This binary measures each of them against a card in the slot. It is
//! RTT-driven, self-reporting, and never shipped: the app image does not link it.
//!
//! It brings up only the sEMMC card: no display, no app, no BLE, no sensors. The store owns the
//! raw card from LBA 0, with no partition table and no filesystem, so a run DESTROYS whatever was
//! on it, a FAT volume included. It refuses to touch a card that already carries a flat store
//! under someone else's `StoreId`.
//!
//! Phase one initializes a card and measures the mount, the commit ladder, the ride journal, the
//! read path and the resident cost, and it leaves a ride recording. Phase two runs after a
//! `probe-rs reset`: it recovers that ride through the store's own [`FlatStore::recovered_ride`],
//! finishes it, and compares the published object byte for byte with the bytes the recorder
//! generated. Every timed figure is reported through [`report_split`], which separates the card's
//! write half, its read half and the M33's residue.
//!
//! A `probe-rs reset` is a CPU reset, not a power cut. The card keeps its supply and never sees
//! the mid-page interruption the fault model is about, so nothing here says anything about
//! tearing. What the reset loop proves is that a mount reconstructs the catalog and the ride from
//! the card alone.
//!
//! `semmc.rs` is pulled in by path and has no `crate::` dependencies, so this binary owns its own
//! host instance and never touches the display mux. The M33 must be at CK128 and `VPR00` bound.
//!
#![no_std]
#![no_main]

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::Instant;
use obc_crc::Crc32;
use obc_formats::ride::{encode_footer, Footer, FOOTER_LEN, SAMPLE_LEN};
use obc_formats::track::encode_record;
use obc_ports::TrackPoint;
use obc_storage::flat::store::{MAX_OPEN_OBJECTS, MAX_RESERVATIONS};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId, ObjectKind,
    PutSource, Revision, RideCheckpoint, RideRecovery, Store as _, StoreId,
};

// The critical-section impl comes from linking nrf-mpsl. MPSL is never initialised here, and its
// implementation works from reset.
use nrf_mpsl as _;
use {defmt_rtt as _, panic_probe as _};

#[allow(dead_code)]
#[path = "../semmc.rs"]
mod semmc;

use semmc::{Semmc, SemmcError, BLOCK_BYTES};

/// sEMMC completion event: VEVIF event 20 is routed to `VPR00_IRQn` by the FLPR firmware.
#[interrupt]
unsafe fn VPR00() {
    semmc::on_vpr00_irq();
}

// The format constants this bench needs. `obc_storage::flat::layout` is `pub(crate)`, and a bench
// sits beside the store rather than above it, so it restates the few it needs instead of widening
// the seam for instrumentation. A wrong constant here shows up as an allocation the store refuses
// or a checkpoint that flushes a different number of pages than this bench predicted.

const EXTENT_SIZE: u64 = 1 << 20;
/// Where the extent area begins, in blocks.
const EXTENT_AREA: u64 = 4_096;
/// The media program page, and the granule ride payload is flushed in.
const PROGRAM_PAGE: usize = 16_384;
/// Bytes held by the shipping recorder: sixteen in-flight samples plus the fixed final footer.
/// The store owns the durable 16 KiB tail snapshot and reconstructs it media-to-media; the
/// recorder lends only the bytes appended since its last successful logical checkpoint.
const RIDE_DELTA_CAPACITY: usize = 16 * SAMPLE_LEN + FOOTER_LEN;
/// The resident free bitmap. Named here only to decompose the footprint report.
const FREE_BITMAP: usize = 8 * 1_024;

/// The plan figure: "boot is about 100 ms".
const PLAN_BOOT_US: u64 = 100_000;
/// The plan figure: "about 15-20 ms at a few hundred entries". The upper end, so a pass is clear.
const PLAN_COMMIT_US: u64 = 20_000;
/// The plan figure for initialization: "well under 1 s".
const PLAN_INIT_US: u64 = 1_000_000;

/// The bench's StoreId. A real initialization draws 128 CSPRNG bits; a fixed value here makes a
/// store from an earlier run recognisable as this bench's rather than a live one's.
const BENCH_STORE: StoreId =
    StoreId([0xF5, 0x04, 0xF1, 0xA7, 0x5B, 0xE2, 0x11, 0x30, 0x9C, 0x64, 0xAD, 0x77, 0x02, 0xEE, 0x38, 0x41]);

/// Flip to `true` for one flash to wipe a card that carries another store's `StoreId`, or to
/// force phase one over this bench's own recorded ride instead of recovering it.
const FORCE_REINIT: bool = false;
/// Explicit build-time maintenance mode. Unlike `FORCE_REINIT`, this stops
/// immediately after initialization instead of running the destructive corpus.
const RESET_ONLY: bool = cfg!(feature = "flat-store-reset");

/// Where the commit ladder reports its middle figure: a few hundred entries.
const LADDER_MID: u16 = 300;
/// Where it stops. One commit runs with the catalog holding exactly this many entries.
const LADDER_TOP: u16 = 1_024;
/// One ladder object's payload. One block, so the object costs the minimum one extent, and the
/// ladder's card cost is `LADDER_TOP` MiB rather than its payload.
const LADDER_PAYLOAD: usize = 512;

/// Samples taken at each reported catalog size. Three is enough to show whether a figure is
/// stable; the same commit moved ten per cent between two runs of this bench's first round.
const COMMIT_SAMPLES: usize = 3;

const RIDE_RESERVE: u64 = 32 * EXTENT_SIZE;
/// The shipping recorder checkpoints every ten seconds. At the minimum one-second fix cadence that
/// is ten exact ride-v4 records, not an arbitrary byte-growth surrogate.
const SAMPLES_PER_CHECKPOINT: u32 = 10;
const CHECKPOINT_SAMPLE_BYTES: usize = SAMPLES_PER_CHECKPOINT as usize * SAMPLE_LEN;
/// Enough ten-sample checkpoints to cross the 16 KiB boundary and turn the 16-slot ring repeatedly.
/// Checkpoint 82 crosses the page: the store consumes one proof sequence and one logical sequence.
const RIDE_CHECKPOINTS: u64 = 83;
const CONTINUATION_SAMPLES: u32 = 7;
const SHORT_SAMPLES: u32 = 10;
const RIDE_NAME: &str = "fs8-ride";
const SHORT_NAME: &str = "fs8-short";

/// What phase one's ride adds up to, from the constants it recorded with. Phase two anchors every
/// expectation here and never on what recovery reported.
const RIDE_LEN: u64 = RIDE_CHECKPOINTS * CHECKPOINT_SAMPLE_BYTES as u64;
/// Payload bytes the journal will have flushed into the ride's own extents at that point.
const RIDE_FLUSHED: u64 = RIDE_LEN / PROGRAM_PAGE as u64 * PROGRAM_PAGE as u64;
/// And what is left in the newest journal slot: deliberately not zero.
const RIDE_TAIL_LEN: u32 = (RIDE_LEN - RIDE_FLUSHED) as u32;
/// A page rollover consumes a proof sequence in addition to the logical checkpoint sequence.
const RIDE_RECOVERED_SEQUENCE: u64 = RIDE_CHECKPOINTS + RIDE_FLUSHED / PROGRAM_PAGE as u64;
const RIDE_POINTS: u32 = RIDE_CHECKPOINTS as u32 * SAMPLES_PER_CHECKPOINT;
const FINAL_POINTS: u32 = RIDE_POINTS + CONTINUATION_SAMPLES;
const FINAL_SAMPLE_BYTES: u64 = FINAL_POINTS as u64 * SAMPLE_LEN as u64;
const FINAL_RIDE_LEN: u64 = FINAL_SAMPLE_BYTES + FOOTER_LEN as u64;

/// The read-path object, if the card has room for it. Addressing is arithmetic over 1 MiB
/// extents, and only an object spanning thousands of them exercises it.
const BIG_TARGET: u64 = 2 * 1_024 * EXTENT_SIZE;
/// Below this the bench scales the object down and says so rather than skipping the read path.
const BIG_MINIMUM: u64 = 64 * EXTENT_SIZE;
/// One `Store::write` / `Store::read` call's span. 64 blocks per device command.
const CHUNK: usize = 32 * 1_024;
const RANDOM_READS: u32 = 512;
const RANDOM_LEN: usize = 4_096;

/// The one sEMMC host. Single-threaded and never re-entered, which is what makes the `&mut` sound.
static mut SEMMC: Semmc = Semmc::new();

/// A 4-byte-aligned byte buffer. The sEMMC firmware's DMA requires 32-bit alignment.
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// The misaligned-span bounce. The store hands the card `[u8; 512]` locals and a 4,096-byte pad
/// out of rodata, neither of which carries an alignment attribute, so every buffer the driver
/// would refuse comes through here.
static mut BOUNCE: Aligned<4_096> = Aligned([0; 4_096]);

/// What the card was asked to do. `reads` and `writes` are calls; the `_blocks` fields are what
/// those calls covered, which is what makes an amplification ratio computable rather than inferred.
#[derive(Clone, Copy, Default)]
struct Counters {
    reads: u32,
    read_blocks: u32,
    /// Microseconds spent inside the driver on those reads.
    read_us: u64,
    writes: u32,
    write_blocks: u32,
    /// Microseconds spent inside the driver on those writes.
    write_us: u64,
    syncs: u32,
}

static mut COUNTERS: Counters =
    Counters { reads: 0, read_blocks: 0, read_us: 0, writes: 0, write_blocks: 0, write_us: 0, syncs: 0 };

/// Everything the card has been asked to do since [`arm`].
fn counters() -> Counters {
    // SAFETY: single-threaded, and no interrupt handler touches this.
    unsafe { *core::ptr::addr_of!(COUNTERS) }
}

/// Zeroes the counters. Named for what a caller does with it: arm, run one thing, read them.
fn arm() {
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(COUNTERS) = Counters::default() };
}

/// The [`BlockDevice`] over the sEMMC host. Zero-sized, because all the state is in [`SEMMC`],
/// which is what lets the counters be read while a [`FlatStore`] owns the device by value.
#[derive(Clone, Copy)]
struct Card;

impl Card {
    /// SAFETY: the caller must not be inside another `with` — this binary never is.
    fn with<R>(f: impl FnOnce(&mut Semmc) -> R) -> R {
        // SAFETY: single-threaded, non-re-entrant, and no interrupt handler touches the host state.
        f(unsafe { &mut *core::ptr::addr_of_mut!(SEMMC) })
    }

    fn count(f: impl FnOnce(&mut Counters)) {
        // SAFETY: as above.
        f(unsafe { &mut *core::ptr::addr_of_mut!(COUNTERS) })
    }

    /// The card addresses blocks in a `u32`; the store's seam is a `u64`.
    fn lba(lba: u64) -> Result<u32, SemmcError> {
        u32::try_from(lba).map_err(|_| SemmcError::OutOfRange)
    }
}

impl BlockDevice for Card {
    type Error = SemmcError;

    fn block_count(&self) -> Result<u64, SemmcError> {
        Card::with(|sd| sd.num_blocks()).map(u64::from)
    }

    fn read(&self, lba: u64, buf: &mut [u8]) -> Result<(), SemmcError> {
        let start = Card::lba(lba)?;
        let blocks = (buf.len() / BLOCK_BYTES) as u32;
        // The stopwatch is around the driver call and nothing else. An interval measured above the
        // seam is the card and the M33 together, and the two have different fixes.
        let started = Instant::now();
        let outcome = Card::with(|sd| {
            if (buf.as_ptr() as usize).is_multiple_of(4) {
                return sd.read_blocks(start, buf);
            }
            // SAFETY: sole borrow; nothing else touches the bounce inside this call.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            let mut done = 0usize;
            while done < buf.len() {
                let take = (buf.len() - done).min(bounce.0.len());
                sd.read_blocks(start + (done / BLOCK_BYTES) as u32, &mut bounce.0[..take])?;
                buf[done..done + take].copy_from_slice(&bounce.0[..take]);
                done += take;
            }
            Ok(())
        });
        let elapsed = us(started);
        Card::count(|c| {
            c.reads += 1;
            c.read_blocks += blocks;
            c.read_us += elapsed;
        });
        outcome
    }

    fn write(&self, lba: u64, buf: &[u8]) -> Result<(), SemmcError> {
        let start = Card::lba(lba)?;
        let blocks = (buf.len() / BLOCK_BYTES) as u32;
        let started = Instant::now();
        let outcome = Card::with(|sd| {
            if (buf.as_ptr() as usize).is_multiple_of(4) {
                return sd.write_blocks(start, buf);
            }
            // SAFETY: as in `read`.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            let mut done = 0usize;
            while done < buf.len() {
                let take = (buf.len() - done).min(bounce.0.len());
                bounce.0[..take].copy_from_slice(&buf[done..done + take]);
                sd.write_blocks(start + (done / BLOCK_BYTES) as u32, &bounce.0[..take])?;
                done += take;
            }
            Ok(())
        });
        let elapsed = us(started);
        Card::count(|c| {
            c.writes += 1;
            c.write_blocks += blocks;
            c.write_us += elapsed;
        });
        outcome
    }

    /// The one thing this transport gets for free.
    ///
    /// `Semmc::write_blocks` does not return until CMD13 says the card has left `prg`, so every
    /// write is already durable by the time the store's next statement runs and a sync has
    /// nothing left to do. The commit budget calls the three synchronizations the dominant term;
    /// on this card they are free, so the syncs are counted and the report can say so.
    fn sync(&self) -> Result<(), SemmcError> {
        Card::count(|c| c.syncs += 1);
        Ok(())
    }
}

/// The repeating pattern the read-path object is made of, and the buffer every `Store::write` of it
/// comes from.
static mut PATTERN: Aligned<CHUNK> = Aligned([0; CHUNK]);
static mut READBACK: Aligned<CHUNK> = Aligned([0; CHUNK]);
/// The recording caller's bounded append buffer. The durable tail snapshot remains store-owned.
static mut RIDE_DELTA: Aligned<RIDE_DELTA_CAPACITY> = Aligned([0; RIDE_DELTA_CAPACITY]);

/// One deterministic semantic sample. Encoding always goes through the production 20-byte codec;
/// the bench never substitutes a byte pattern for a ride point.
fn ride_sample(index: u32) -> [u8; SAMPLE_LEN] {
    let n = index as i32;
    encode_record(&TrackPoint {
        lon: 7_000_000 + n * 13,
        lat: 46_000_000 + n * 7,
        ele: 430 + (index % 900) as i16,
        t_ms: index.wrapping_mul(1_000),
        segment_start: index == 0 || index == RIDE_POINTS,
        hr: (!index.is_multiple_of(5)).then_some(120 + (index % 55) as u8),
        cadence: (!index.is_multiple_of(7)).then_some(70 + (index % 35) as u8),
        power: (!index.is_multiple_of(11)).then_some(150 + (index % 500) as u16),
    })
}

fn sample_byte(offset: u64) -> u8 {
    let index = (offset / SAMPLE_LEN as u64) as u32;
    ride_sample(index)[offset as usize % SAMPLE_LEN]
}

fn final_footer() -> [u8; FOOTER_LEN] {
    encode_footer(&Footer::new(
        RIDE_NAME,
        1_751_449_700,
        4_180,
        FINAL_POINTS - 1,
        500,
        321,
        FINAL_POINTS,
        Some(145),
        Some(178),
        Some(86),
        Some(235),
        Some(612),
    ))
}

fn short_footer() -> [u8; FOOTER_LEN] {
    encode_footer(&Footer::new(
        SHORT_NAME,
        1_751_449_800,
        45,
        SHORT_SAMPLES - 1,
        500,
        3,
        SHORT_SAMPLES,
        Some(130),
        Some(142),
        Some(82),
        Some(190),
        Some(260),
    ))
}

/// Recorder-owned state that must survive with the same logical checkpoint as its bytes.
fn resume_image(points: u32) -> [u8; obc_storage::flat::RIDE_RESUME_LEN] {
    let mut resume = [0u8; obc_storage::flat::RIDE_RESUME_LEN];
    resume[..4].copy_from_slice(&points.to_le_bytes());
    for (index, byte) in resume[4..].iter_mut().enumerate() {
        *byte = points.wrapping_add(index as u32 * 17) as u8;
    }
    resume
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    {
        let mut config = embassy_nrf::config::Config::default();
        // Not optional: the sEMMC clock divisors and the firmware's wait slices are stated against
        // a 128 MHz core.
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config);
    }
    // SAFETY: arming the vector before the soft peripheral boots is what `main.rs` does too.
    unsafe {
        interrupt::VPR00.set_priority(Priority::P1);
        interrupt::VPR00.enable();
    }
    stackmeter::arm_limit();
    stackmeter::paint();
    info!("flat_store_bench: obc_storage::flat on the real card ({=str})", env!("OBC_FW_GIT"));
    info!("flat_store_bench: DESTRUCTIVE — the store owns the raw card from LBA 0, partition table included");

    let card = match Card::with(|sd| sd.start()) {
        Ok(card) => card,
        Err(error) => {
            error!("flat_store_bench: the card did not come up ({}) — nothing measured", error);
            park();
        }
    };
    info!(
        "CARD  rca=0x{=u16:04x} blocks={=u32} ({=u64} MiB) high_speed={=bool} read_clk={=u32} Hz",
        card.rca,
        card.blocks,
        (card.blocks as u64) * 512 / (1024 * 1024),
        card.high_speed,
        card.read_clk_hz
    );
    // The extent count, from the card's block count. The store computes the same thing from the
    // superblock; a mismatch would show up as an allocation it refuses.
    let extents = ((u64::from(card.blocks).saturating_sub(EXTENT_AREA)) / (EXTENT_SIZE / 512)).min(65_536);
    info!("CARD  §6 extent area: {=u64} extents of 1 MiB ({=u64} MiB addressable)", extents, extents);

    run();
    info!("STACK high-water across the whole run: {=usize} B of {=usize} B", stackmeter::used(), stackmeter::total());
    park();
}

/// The whole run, in a plain function rather than in the async `main`.
///
/// An async fn's locals are permanent poll-frame slots, and this one places stores whose type is
/// ten kilobytes. In an ordinary call the same locals are scoped to the frame and come back at
/// return, which is what makes the printed stack figure a measurement of the store's peak rather
/// than of the executor's permanent reservation.
#[inline(never)]
fn run() {
    report_footprint();

    if RESET_ONLY {
        info!("RESET ONLY: initializing one empty flat store; no benchmark phases will run");
        if initialize().is_some() {
            info!("RESET ONLY complete: the card has an empty flat catalog; flash the normal app firmware next");
        }
        return;
    }

    let boot = measure_boot("SURVEY", Some(PLAN_BOOT_US));
    let ours = boot.mode.readable() && boot.store_id == BENCH_STORE;
    if boot.mode.readable() && !ours && !FORCE_REINIT {
        error!("BOOT  this card carries another store's StoreId — REFUSING to wipe it (set FORCE_REINIT)");
        return;
    }
    if ours && boot.recording.is_some() && !FORCE_REINIT {
        phase_two(&boot);
        return;
    }
    phase_one();
}

fn phase_one() {
    info!("PHASE one: initialize, then measure boot, commit, ride and read on a card built from scratch");

    let Some(extents) = initialize() else { return };

    measure_boot("BOOT  empty catalog", Some(PLAN_BOOT_US));

    // How much of the card the three phases may take. The ladder is one extent per object, the
    // ride takes the 32 MiB reserve, and the read-path object takes what is left, up to 2 GiB.
    let ladder_top = if u32::from(LADDER_TOP) + 64 <= extents / 2 {
        LADDER_TOP
    } else {
        let scaled = (extents / 2).saturating_sub(64).min(u32::from(u16::MAX)) as u16;
        warn!("SIZE  the card has {=u32} extents — scaling the commit ladder to {=u16} entries", extents, scaled);
        scaled
    };
    let spare = u64::from(extents.saturating_sub(u32::from(ladder_top) + 40)) * EXTENT_SIZE;
    let big = BIG_TARGET.min(spare / EXTENT_SIZE * EXTENT_SIZE);

    let ladder_free = ladder(ladder_top);
    info!("LADDER left {=u32} free extents for the ride and the read-path object", ladder_free);
    ride();
    if big < BIG_MINIMUM {
        error!("READ  only {=u64} MiB spare after the ladder — the read path needs at least 64 MiB", big / EXTENT_SIZE);
    } else {
        if big < BIG_TARGET {
            warn!(
                "READ  SCALED: the card leaves {=u64} MiB, so the read-path object is {=u64} MiB rather than 2 GiB",
                spare / EXTENT_SIZE,
                big / EXTENT_SIZE
            );
        }
        read_path(big);
    }
    info!("PHASE one done. `probe-rs reset` to run phase two: the ride recovers from the card alone.");
}

/// Initialization: two superblocks invalidated, gate B invalidated, sixteen slot headers
/// invalidated, one empty catalog body, its gate, then both superblocks, which is five
/// synchronization points. The mount that follows is part of what this returns, along with the
/// free extents the card came up with.
///
/// It is a function of its own so the store it builds goes out of scope with it: a boot figure has
/// to be a mount's own cost, and a store left standing here would sit on the stack for the run.
#[inline(never)]
fn initialize() -> Option<u32> {
    arm();
    let started = Instant::now();
    let store = match FlatStore::initialize(Card, BENCH_STORE) {
        Ok(store) => store,
        Err(error) => {
            error!("INIT  §8 initialization failed ({})", defmt::Debug2Format(&error));
            return None;
        }
    };
    let init_us = us(started);
    let counted = counters();
    info!(
        "INIT  §8 initialization + the mount it returns: {=u64} us ({=u32} writes / {=u32} blocks, {=u32} reads / {=u32} blocks, {=u32} syncs)",
        init_us, counted.writes, counted.write_blocks, counted.reads, counted.read_blocks, counted.syncs
    );
    verdict("INIT  first boot", init_us, PLAN_INIT_US);
    info!(
        "INIT  the store came up {} with {=u32} free extents and commit sequence {=u64}",
        defmt::Debug2Format(&store.mode()),
        store.free_extents(),
        store.sequence()
    );
    Some(store.free_extents())
}

/// What one mount found, and what it cost.
struct Boot {
    mode: Mode,
    store_id: StoreId,
    entries: u16,
    us: u64,
    counters: Counters,
    recording: Option<EntryMeta>,
    recovered: Option<RideRecovery>,
}

/// One mount, timed, with the entry the journal half of the mount keys on.
///
/// The store is dropped before this returns: a boot figure is what a mount costs, and holding one
/// open would put a second ten-kilobyte store beside whichever one the caller already has.
///
/// `plan` is a parameter because a mount that recovered a ride is reported against no budget. The
/// ~100 ms figure is scoped to a card with no ride in progress, where a mount reads at most three
/// blocks plus the live catalog prefix. A mount that reads sixteen slot headers and CRCs a 16 KiB
/// slot does more than that figure covers, and the format states no figure for it.
#[inline(never)]
fn measure_boot(label: &str, plan: Option<u64>) -> Boot {
    arm();
    let started = Instant::now();
    let store = FlatStore::mount(Card);
    let elapsed = us(started);
    let counted = counters();

    let mut recording = None;
    for entry in store.entries() {
        if entry.flags.has(EntryFlags::RECORDING) {
            recording = Some(entry);
        }
    }
    let boot = Boot {
        mode: store.mode(),
        store_id: store.store_id(),
        entries: store.entry_count(),
        us: elapsed,
        counters: counted,
        recording,
        recovered: store.recovered_ride(),
    };
    info!(
        "{=str}: {=u64} us — {} at sequence {=u64}, {=u16} entries, {=u32} free extents, {=u32} reads / {=u32} blocks",
        label,
        boot.us,
        defmt::Debug2Format(&boot.mode),
        store.sequence(),
        boot.entries,
        store.free_extents(),
        boot.counters.reads,
        boot.counters.read_blocks
    );
    if let Some(recovered) = boot.recovered {
        info!(
            "{=str}: §7.3 recovered a ride — object {=u64} revision {=u64}, checkpoint {=u64}, {=u64} B flushed + {=u32} B tail",
            label,
            recovered.id.0,
            recovered.revision.0,
            recovered.checkpoint_sequence,
            recovered.flushed,
            recovered.tail_len
        );
    }
    report_split(label, boot.us, &boot.counters);
    match (plan, boot.recovered) {
        (_, Some(_)) => info!(
            "{=str}: reported against NO budget — §5.6's ~100 ms is scoped to a card with no ride in progress, and the spec states no figure for a mount that also recovers one",
            label
        ),
        (Some(plan), None) => verdict(label, boot.us, plan),
        (None, None) => {}
    }
    boot
}

/// The commit ladder, from an empty catalog to [`LADDER_TOP`] entries, one create per step.
///
/// Every step is `allocate`, `write`, `commit`, which is the whole publication path. Only the
/// commit is on the clock, because it is the step the format puts a figure on. Returns the free
/// extents left behind.
fn ladder(top: u16) -> u32 {
    let store = FlatStore::mount(Card);
    let mut total = 0u64;
    let mut worst = 0u64;
    let mut worst_at = 0u16;
    let mut publish_total = 0u64;
    for _ in 0..=top {
        let entries = store.entry_count();
        // At a reported catalog size the figure is sampled, not taken once: the same commit moved
        // ten per cent between two runs of this bench, so one observation is an anecdote rather
        // than a cost. The ladder's own commit below still feeds the aggregate.
        if entries == 0 || entries == LADDER_MID || entries == top {
            sample_commits(&store, entries);
            if entries > 0 {
                measure_opens(&store, entries);
            }
        }
        let started = Instant::now();
        let Some(commit) = create_once(&store) else { return store.free_extents() };
        let commit_us = commit.elapsed;
        publish_total += us(started);
        total += commit_us;
        if commit_us > worst {
            worst = commit_us;
            worst_at = entries;
        }
        if entries == LADDER_MID || entries == top {
            let label = if entries == LADDER_MID { "BOOT  300 entries" } else { "BOOT  full ladder" };
            measure_boot(label, Some(PLAN_BOOT_US));
        }
    }
    let steps = u64::from(top) + 1;
    info!(
        "LADDER {=u64} commits from an empty catalog to {=u16} entries: {=u64} us mean, {=u64} us worst (at {=u16} entries)",
        steps, top, total / steps, worst, worst_at
    );
    info!(
        "LADDER the whole publication — write + commit — averaged {=u64} us, so {=u64} objects/s sustained",
        publish_total / steps,
        1_000_000 / (publish_total / steps).max(1)
    );
    store.free_extents()
}

/// One timed commit: what it cost, what the card did, and which catalog copy it landed on.
struct Commit {
    elapsed: u64,
    counted: Counters,
    /// A commit writes the copy that is not being served and then serves it, so the copies
    /// strictly alternate from initialization, which serves copy 0 at sequence 1. A commit that
    /// produced sequence `s` therefore wrote copy `1 - (s % 2)`, and that survives any number of
    /// mounts in between, because a mount serves whichever copy carries the greater sequence.
    copy: usize,
    id: ObjectId,
}

/// Which catalog copy the commit that produced `sequence` wrote.
fn copy_of(sequence: u64) -> usize {
    1 - (sequence % 2) as usize
}

/// One create: allocate, write the payload, commit. Only the commit is on the clock.
fn create_once(store: &FlatStore<Card>) -> Option<Commit> {
    let entries = store.entry_count();
    let payload = [0x5Au8; LADDER_PAYLOAD];
    let mut allocation = match store.allocate(LADDER_PAYLOAD as u64) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!("LADDER allocate at {=u16} entries refused ({})", entries, defmt::Debug2Format(&error));
            return None;
        }
    };
    if let Err(error) = store.write(&mut allocation, &payload) {
        error!("LADDER write at {=u16} entries refused ({})", entries, defmt::Debug2Format(&error));
        store.cancel(allocation);
        return None;
    }
    let meta = EntryMeta {
        added_at_utc: 0,
        id: store.next_object_id(),
        revision: Revision(1),
        kind: ObjectKind::Route,
        flags: EntryFlags::NONE,
        payload_len: LADDER_PAYLOAD as u64,
        payload_crc: obc_crc::crc32(&payload),
        name: DisplayName::new("fs4-ladder").unwrap_or_default(),
    };
    let id = meta.id;
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let elapsed = us(started);
    let counted = counters();
    let sequence = match outcome {
        Ok(sequence) => sequence,
        Err(error) => {
            error!("LADDER commit at {=u16} entries refused ({})", entries, defmt::Debug2Format(&error));
            return None;
        }
    };
    Some(Commit { elapsed, counted, copy: copy_of(sequence), id })
}

/// One removal, timed: the same whole-prefix rewrite a create pays, without a payload write.
fn remove_once(store: &FlatStore<Card>, id: ObjectId) -> Option<Commit> {
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Remove { id, revision: Revision(1) }]);
    let elapsed = us(started);
    let counted = counters();
    let sequence = match outcome {
        Ok(sequence) => sequence,
        Err(error) => {
            error!("LADDER remove of object {=u64} refused ({})", id.0, defmt::Debug2Format(&error));
            return None;
        }
    };
    Some(Commit { elapsed, counted, copy: copy_of(sequence), id })
}

/// The commit figure at one catalog size, sampled [`COMMIT_SAMPLES`] times per catalog copy.
///
/// The shape of this is the whole point. Commits alternate copies, so a create-then-remove sample
/// sends every create to one copy and every remove to the other, and the gap that appears between
/// the two is a copy difference wearing a mutation-kind costume. Two creates in a row land on the
/// two copies with everything else equal, and the two removals that undo them do the same, so each
/// sample yields four figures and every sample repeats the same assignment. The entry count moves
/// by one between the paired commits, which changes no block count at any size this bench reports.
fn sample_commits(store: &FlatStore<Card>, entries: u16) {
    let mut creates = [[0u64; COMMIT_SAMPLES]; 2];
    let mut removes = [[0u64; COMMIT_SAMPLES]; 2];
    let mut last = [Counters::default(); 2];
    for index in 0..COMMIT_SAMPLES {
        let Some(first) = create_once(store) else { return };
        let Some(second) = create_once(store) else { return };
        for commit in [&first, &second] {
            creates[commit.copy][index] = commit.elapsed;
            last[commit.copy] = commit.counted;
        }
        let Some(third) = remove_once(store, first.id) else { return };
        let Some(fourth) = remove_once(store, second.id) else { return };
        for commit in [&third, &fourth] {
            removes[commit.copy][index] = commit.elapsed;
        }
    }
    // The format's commit: `ceil(n/4) + 3` block writes, which are the body's `1 + ceil(n/4)`
    // blocks (header included), the gate invalidation and the gate itself, plus three syncs.
    let predicted = 1 + (u32::from(entries) + 1).div_ceil(4) + 2;
    let mut cross = 0u64;
    for copy in 0..2 {
        let (mean, median, least, greatest) = spread(&creates[copy]);
        cross += mean;
        info!(
            "COMMIT at {=u16} entries, create on catalog copy {=usize} ({=usize} samples): {=u64} us mean, {=u64} median, {=u64}..{=u64} — {=u32} blocks, §5.5 predicts {=u32}",
            entries, copy, COMMIT_SAMPLES, mean, median, least, greatest, last[copy].write_blocks, predicted
        );
        report_split("COMMIT create (last sample)", creates[copy][COMMIT_SAMPLES - 1], &last[copy]);
        let (mean, median, least, greatest) = spread(&removes[copy]);
        info!(
            "COMMIT at {=u16} entries, remove on catalog copy {=usize}: {=u64} us mean, {=u64} median, {=u64}..{=u64}",
            entries, copy, mean, median, least, greatest
        );
    }
    // The figure a device actually pays: commits alternate, so consecutive ones pay one copy each.
    let cross = cross / 2;
    info!(
        "COMMIT at {=u16} entries: CROSS-COPY create mean {=u64} us — this is the figure to quote, because §5.5 alternates and no caller gets to pick the cheaper copy",
        entries, cross
    );
    info!(
        "COMMIT at {=u16} entries: the {=u32} syncs are no-ops on this transport — `write_blocks` already waited out the program cycle, so none of the time above is theirs",
        entries, last[0].syncs
    );
    verdict("COMMIT", cross, PLAN_COMMIT_US);
}

/// What `open` costs: a binary search over the live prefix, then the hold row.
///
/// Twelve of them, because [`MAX_OPEN_OBJECTS`] is sized for the eleven map shards a rendered set
/// mounts plus one transfer, so this is the figure a renderer pays to bring a set up. The same
/// search is `find`, which every commit's `resolve` runs once per mutation, so it is also half of
/// why the commit figures above carry the read time they do.
fn measure_opens(store: &FlatStore<Card>, entries: u16) {
    let mut ids = [ObjectId::NONE; MAX_OPEN_OBJECTS];
    let step = (entries as usize / MAX_OPEN_OBJECTS).max(1);
    let mut found = 0usize;
    for (index, meta) in store.entries().enumerate() {
        if index.is_multiple_of(step) && found < MAX_OPEN_OBJECTS {
            ids[found] = meta.id;
            found += 1;
        }
    }
    if found == 0 {
        return;
    }
    let mut handles: [Option<Handle>; MAX_OPEN_OBJECTS] = core::array::from_fn(|_| None);
    arm();
    let started = Instant::now();
    for (slot, id) in ids[..found].iter().enumerate() {
        handles[slot] = store.open(*id, None).ok();
    }
    let elapsed = us(started);
    let counted = counters();
    let opened = handles.iter().flatten().count().max(1) as u64;
    info!(
        "OPEN  {=u64} objects spread across {=u16} entries: {=u64} us total, {=u64} us each — {=u32} entry-block reads, {=u64} per open",
        opened,
        entries,
        elapsed,
        elapsed / opened,
        counted.read_blocks,
        u64::from(counted.read_blocks) / opened
    );
    report_split("OPEN ", elapsed, &counted);
    for handle in handles.into_iter().flatten() {
        store.close(handle);
    }
}

/// The ride journal's write half: start a ride, then one checkpoint per ten encoded samples,
/// timed. The ride is left recording on purpose, because it is what phase two recovers.
fn ride() {
    let store = FlatStore::mount(Card);
    let id = store.next_object_id();

    let allocation = match store.allocate(RIDE_RESERVE) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!("RIDE  the 32 MiB reserve was refused ({})", defmt::Debug2Format(&error));
            return;
        }
    };
    // A `RECORDING` entry holds slack: it owns more extents than its payload needs, which is the
    // one thing that lets a ride grow without a commit per page.
    let meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision: Revision(1),
        kind: ObjectKind::Ride,
        flags: EntryFlags::RECORDING,
        payload_len: 0,
        payload_crc: 0,
        name: DisplayName::default(),
    };
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let start_us = us(started);
    let counted = counters();
    if let Err(error) = outcome {
        error!("RIDE  the start commit was refused ({})", defmt::Debug2Format(&error));
        return;
    }
    info!(
        "RIDE  start (32 MiB reserve + one commit at {=u16} entries): {=u64} us, {=u32} writes / {=u32} blocks",
        store.entry_count() - 1,
        start_us,
        counted.writes,
        counted.write_blocks
    );

    // SAFETY: sole borrow of the append buffer; nothing else reads it.
    let delta = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_DELTA)).0 };
    let mut payload_len = 0u64;
    let mut flushed = 0u64;
    let mut digest = Crc32::new();
    let mut plain_total = 0u64;
    let mut plain = 0u64;
    let mut flush_total = 0u64;
    let mut flushes = 0u64;
    let mut worst = 0u64;
    for checkpoint_number in 1..=RIDE_CHECKPOINTS {
        let mut delta_len = 0usize;
        for _ in 0..SAMPLES_PER_CHECKPOINT {
            let point = (payload_len / SAMPLE_LEN as u64) as u32;
            let sample = ride_sample(point);
            delta[delta_len..delta_len + SAMPLE_LEN].copy_from_slice(&sample);
            delta_len += SAMPLE_LEN;
            digest.update(&sample);
            payload_len += SAMPLE_LEN as u64;
        }
        debug_assert_eq!(delta_len, CHECKPOINT_SAMPLE_BYTES);
        let resume = resume_image((payload_len / SAMPLE_LEN as u64) as u32);
        let checkpoint = RideCheckpoint {
            id,
            revision: Revision(1),
            append: &delta[..delta_len],
            payload_crc: digest.finalize(),
            resume: &resume,
        };
        arm();
        let started = Instant::now();
        let outcome = store.journal(checkpoint);
        let elapsed = us(started);
        let counted = counters();
        if let Err(error) = outcome {
            error!("RIDE  checkpoint {=u64} refused ({})", checkpoint_number, defmt::Debug2Format(&error));
            return;
        }
        // Storage owns the accumulated tail snapshot. The caller consumes exactly this interval's
        // delta after success; a page boundary is visible only in the measured I/O and sequence.
        let next_flushed = payload_len / PROGRAM_PAGE as u64 * PROGRAM_PAGE as u64;
        let pages = ((next_flushed - flushed) / PROGRAM_PAGE as u64) as usize;
        if pages > 0 {
            flushed = next_flushed;
            flush_total += elapsed;
            flushes += 1;
            info!(
                "RIDE  checkpoint {=u64} flushed {=usize} payload page(s): {=u64} us, {=u32} writes / {=u32} blocks",
                checkpoint_number, pages, elapsed, counted.writes, counted.write_blocks
            );
        } else {
            plain_total += elapsed;
            plain += 1;
            if checkpoint_number == 1 {
                info!(
                    "RIDE  checkpoint 1 (ten 20-byte samples into one 16 KiB slot, no page rollover): {=u64} us, {=u32} writes / {=u32} blocks, {=u32} syncs",
                    elapsed, counted.writes, counted.write_blocks, counted.syncs
                );
            }
        }
        worst = worst.max(elapsed);
    }
    info!(
        "RIDE  {=u64} checkpoints × {=u32} exact samples × {=usize} B: {=u64} us mean without a page rollover ({=u64}), {=u64} us mean with one ({=u64}), {=u64} us worst",
        RIDE_CHECKPOINTS,
        SAMPLES_PER_CHECKPOINT,
        SAMPLE_LEN,
        plain_total / plain.max(1),
        plain,
        flush_total / flushes.max(1),
        flushes,
        worst
    );
    info!(
        "RIDE  §9's cadence is one checkpoint per 10 s, so the journal costs {=u64} ppm of the ride's wall time",
        (plain_total + flush_total) / RIDE_CHECKPOINTS / 10
    );
    info!(
        "RIDE  left recording for reset: object {=u64}, {=u32} samples / {=u64} B, {=u64} B flushed, logical sequence {=u64}, crc 0x{=u32:08x}",
        id.0,
        RIDE_POINTS,
        payload_len,
        flushed,
        RIDE_RECOVERED_SEQUENCE,
        digest.finalize()
    );
}

/// The read path over a multi-GiB object: write it, sweep it, then hit it at random.
fn read_path(bytes: u64) {
    let store = FlatStore::mount(Card);
    // SAFETY: sole borrows of the two payload slots.
    let pattern = unsafe { &mut (*core::ptr::addr_of_mut!(PATTERN)).0 };
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    for (at, byte) in pattern.iter_mut().enumerate() {
        *byte = (at * 7 + 11) as u8;
    }

    let id = store.next_object_id();
    let mut allocation = match store.allocate(bytes) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!(
                "READ  the {=u64} MiB allocation was refused ({})",
                bytes / EXTENT_SIZE,
                defmt::Debug2Format(&error)
            );
            return;
        }
    };

    // The write, with the CRC fold on its own clock: rolling it into the write's rate would report
    // the M33 rather than the card. The board enables obc-crc's slicing-by-8 implementation, so
    // this is also the acceptance measurement for the upload pipeline's CRC worker.
    arm();
    let mut digest = Crc32::new();
    let mut written = 0u64;
    let mut write_us = 0u64;
    let mut crc_us = 0u64;
    while written < bytes {
        let started = Instant::now();
        if let Err(error) = store.write(&mut allocation, pattern) {
            error!("READ  write at {=u64} B refused ({})", written, defmt::Debug2Format(&error));
            store.cancel(allocation);
            return;
        }
        write_us += us(started);
        let started = Instant::now();
        digest.update(pattern);
        crc_us += us(started);
        written += CHUNK as u64;
    }
    let crc = digest.finalize();
    let counted = counters();
    info!(
        "READ  wrote {=u64} MiB in {=usize} B calls: {=u64} ms, {=u64} kB/s ({=u32} device writes / {=u32} blocks)",
        bytes / EXTENT_SIZE,
        CHUNK,
        write_us / 1_000,
        rate(bytes, write_us),
        counted.writes,
        counted.write_blocks
    );
    info!(
        "READ  the CRC-32 fold over the same bytes cost {=u64} ms ({=u64} kB/s on the M33)",
        crc_us / 1_000,
        rate(bytes, crc_us)
    );

    let meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision: Revision(1),
        kind: ObjectKind::MapShard,
        flags: EntryFlags::NONE,
        payload_len: bytes,
        payload_crc: crc,
        name: DisplayName::new("fs4-read-path").unwrap_or_default(),
    };
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Fresh(allocation) }]);
    let commit_us = us(started);
    if let Err(error) = outcome {
        error!("READ  the publishing commit was refused ({})", defmt::Debug2Format(&error));
        return;
    }
    info!("READ  published {=u64} MiB as object {=u64} in one commit: {=u64} us", bytes / EXTENT_SIZE, id.0, commit_us);

    let handle = match store.open(id, None) {
        Ok(handle) => handle,
        Err(error) => {
            error!("READ  open refused ({})", defmt::Debug2Format(&error));
            return;
        }
    };

    // The sequential sweep. Every byte is compared against the pattern and folded into a CRC, both
    // off the clock, so the rate is the store's and the verdict is still byte-exact.
    arm();
    let mut digest = Crc32::new();
    let mut offset = 0u64;
    let mut read_us = 0u64;
    let mut mismatch = None;
    while offset < bytes {
        let started = Instant::now();
        let got = match store.read(&handle, offset, readback) {
            Ok(got) => got,
            Err(error) => {
                error!("READ  sweep at {=u64} B refused ({})", offset, defmt::Debug2Format(&error));
                return;
            }
        };
        read_us += us(started);
        if got != CHUNK {
            error!("READ  sweep at {=u64} B came back short ({=usize} B)", offset, got);
            return;
        }
        if mismatch.is_none() && readback[..] != pattern[..] {
            mismatch = Some(offset);
        }
        digest.update(&readback[..got]);
        offset += got as u64;
    }
    let counted = counters();
    if let Some(at) = mismatch {
        error!("READ  the sweep read back bytes the write never put there, from {=u64} B", at);
    } else if digest.finalize() != crc {
        error!("READ  the sweep's CRC does not match the published one");
    } else {
        info!(
            "READ  the whole {=u64} MiB read back byte for byte, crc 0x{=u32:08x} confirmed",
            bytes / EXTENT_SIZE,
            crc
        );
    }
    info!(
        "READ  sequential sweep in {=usize} B calls: {=u64} ms, {=u64} kB/s ({=u64} us per call)",
        CHUNK,
        read_us / 1_000,
        rate(bytes, read_us),
        read_us / (bytes / CHUNK as u64).max(1)
    );
    amplification("READ  sequential", &counted, bytes / 512, read_us);

    random_pass(&store, &handle, bytes, 0, "aligned");
    random_pass(&store, &handle, bytes, 1, "byte-offset");
    random_pass(&store, &handle, bytes, 511, "block-straddling");
    store.close(handle);
}

/// [`RANDOM_READS`] reads of [`RANDOM_LEN`] at random offsets, `skew` bytes off a block boundary.
fn random_pass(store: &FlatStore<Card>, handle: &Handle, bytes: u64, skew: u64, label: &str) {
    // SAFETY: sole borrow; the sweep is finished with it.
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    let span = bytes - RANDOM_LEN as u64 - 512;
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut elapsed = 0u64;
    let mut blocks = 0u64;
    arm();
    for _ in 0..RANDOM_READS {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let offset = (state % (span / 512)) * 512 + skew;
        let started = Instant::now();
        let got = store.read(handle, offset, &mut readback[..RANDOM_LEN]);
        elapsed += us(started);
        match got {
            Ok(got) if got == RANDOM_LEN => {}
            other => {
                error!("READ  {=str} read at {=u64} B failed ({})", label, offset, defmt::Debug2Format(&other));
                return;
            }
        }
        // The blocks this read's payload span actually occupies, which is what an amplification of
        // 1.00 means: the store read those and not one more.
        blocks += ((offset % 512) + RANDOM_LEN as u64).div_ceil(512);
    }
    let counted = counters();
    info!(
        "READ  {=str} random: {=u32} x {=usize} B in {=u64} ms — {=u64} us each, {=u64} reads/s, {=u64} kB/s",
        label,
        RANDOM_READS,
        RANDOM_LEN,
        elapsed / 1_000,
        elapsed / u64::from(RANDOM_READS),
        1_000_000 / (elapsed / u64::from(RANDOM_READS)).max(1),
        rate(u64::from(RANDOM_READS) * RANDOM_LEN as u64, elapsed)
    );
    amplification("READ  random", &counted, blocks, elapsed);
}

/// The read-ratio check, in the shape a flat store makes it mean something.
///
/// Device reads per block is only a batching figure here: the store issues one command per
/// contiguous run, so it is well under 1.00 by design and says only how long the runs were. The
/// claim worth checking is the other ratio, blocks read per block the payload occupies. There is
/// no chain to walk and no indirection block to fetch, so a read that does not amplify is exactly
/// 1.00, and anything above it is the store reading something it did not need.
fn amplification(label: &str, counted: &Counters, required: u64, elapsed: u64) {
    let ratio = u64::from(counted.read_blocks) * 100 / required.max(1);
    info!(
        "{=str}: {=u32} device blocks read for {=u64} payload blocks = {=u64}/100 — {=u32} commands ({=u64}/100 per block), {=u64} us per command",
        label,
        counted.read_blocks,
        required,
        ratio,
        counted.reads,
        u64::from(counted.reads) * 100 / required.max(1),
        elapsed / u64::from(counted.reads.max(1))
    );
    if ratio == 100 {
        info!("{=str}: amplification is exactly 1.00 — §6.1's arithmetic read no block it did not need", label);
    } else {
        error!(
            "{=str}: amplification is {=u64}/100, not 1.00 — the read path fetched blocks it did not need",
            label, ratio
        );
    }
}

/// The resident total used by this bench. The format fixes the 8 KiB free bitmap and each
/// card-resident slot's 16 KiB payload, but states no combined RAM budget, so the caller addend
/// below, the shipping recorder's bounded append buffer, is reported as a fact rather than against
/// a plan figure that does not exist.
const RESIDENT: usize = core::mem::size_of::<FlatStore<Card>>() + RIDE_DELTA_CAPACITY;

const _: () = assert!(core::mem::size_of::<FlatStore<Card>>() > FREE_BITMAP);

fn report_footprint() {
    let store = core::mem::size_of::<FlatStore<Card>>();
    let handle = core::mem::size_of::<obc_storage::flat::Handle>();
    info!(
        "RAM   [addend 1] FlatStore<Card> {=usize} B = the {=usize} B free bitmap (§6.2) + {=usize} reservation row(s) + {=usize} hold row(s) + the mounted rows",
        store, FREE_BITMAP, MAX_RESERVATIONS, MAX_OPEN_OBJECTS
    );
    info!(
        "RAM   [addend 2] shipping recording delta: {=usize} B = sixteen × {=usize} B in-flight samples + {=usize} B final footer",
        RIDE_DELTA_CAPACITY, SAMPLE_LEN, FOOTER_LEN
    );
    info!("RAM   [component] one open Handle: {=usize} B; the entry array is never resident (§5.1)", handle);
    info!("RAM   [component] this bench's own buffers, which are not the store's: {=usize} B", CHUNK * 2 + 4_096);
    info!(
        "RAM   TOTAL for one mounted store plus the shipping ride delta: {=usize} B. §9 states the component geometry, not a combined RAM budget; this is reported without a fabricated plan verdict",
        RESIDENT
    );
}

/// Everything recorded before reset must come back from the card alone. Recording then continues,
/// appends the footer as final bytes, and one commit publishes that exact byte string.
fn phase_two(boot: &Boot) {
    info!("PHASE two: the card already carries this bench's store with a ride recording");
    let Some(entry) = boot.recording else { return };
    let Some(recovered) = boot.recovered else {
        error!("RCVR  §5.6 found a RECORDING entry but §7.3 recovered no checkpoint — a ride with no slot at all");
        return;
    };

    // Every expectation below is anchored on the constants phase one wrote with, not on anything
    // the store just said. Deriving the expected length from `checkpoint_sequence` makes the check
    // self-fulfilling: a store that silently selected the prior logical sequence would hand back a
    // shorter ride and a CRC over that shorter ride, and both would match.
    let mut digest = Crc32::new();
    for offset in 0..RIDE_LEN {
        digest.update(&[sample_byte(offset)]);
    }
    let expected_crc = digest.finalize();
    let mut ok = recovered.id == entry.id && recovered.revision == entry.revision;
    if recovered.checkpoint_sequence != RIDE_RECOVERED_SEQUENCE {
        error!(
            "RCVR  §7.3 selected logical sequence {=u64}, expected {=u64} from {=u64} checkpoints plus the rollover proof — a checkpoint was lost or a stale slot won",
            recovered.checkpoint_sequence, RIDE_RECOVERED_SEQUENCE, RIDE_CHECKPOINTS
        );
        ok = false;
    }
    if (recovered.flushed, recovered.tail_len) != (RIDE_FLUSHED, RIDE_TAIL_LEN) {
        error!(
            "RCVR  recovery says {=u64} B flushed + {=u32} B tail; {=u64} checkpoints of {=usize} B is {=u64} + {=u32}",
            recovered.flushed,
            recovered.tail_len,
            RIDE_CHECKPOINTS,
            CHECKPOINT_SAMPLE_BYTES,
            RIDE_FLUSHED,
            RIDE_TAIL_LEN
        );
        ok = false;
    }
    if recovered.payload_len() != RIDE_LEN {
        error!("RCVR  recovery says {=u64} B of ride; phase one recorded {=u64} B", recovered.payload_len(), RIDE_LEN);
        ok = false;
    }
    if recovered.payload_crc != expected_crc {
        error!(
            "RCVR  the recovered payload CRC is 0x{=u32:08x}, not 0x{=u32:08x}",
            recovered.payload_crc, expected_crc
        );
        ok = false;
    }
    let expected_resume = resume_image(RIDE_POINTS);
    if recovered.resume != expected_resume {
        error!("RCVR  the recorder-owned continuation image is not the one paired with the final phase-one samples");
        ok = false;
    }
    info!(
        "RCVR  §7.3 selected logical sequence {=u64} (physical slot {=u64} of 16 after wrap): {=u64} B flushed + {=u32} B tail = {=u64} B, crc 0x{=u32:08x}",
        recovered.checkpoint_sequence,
        recovered.checkpoint_sequence % 16,
        recovered.flushed,
        recovered.tail_len,
        recovered.payload_len(),
        recovered.payload_crc
    );
    if !ok {
        error!("RCVR  recovery did not match the independently regenerated checkpoint; refusing to continue or finalise it");
        return;
    }
    info!(
        "RCVR  recovery matches all {=u64} phase-one checkpoints — identity, wrapped sequence, flush point, exact sample bytes' CRC, and recorder continuation state",
        RIDE_CHECKPOINTS
    );

    let long = ride_end(&entry, recovered);
    let short = short_ride();
    if let (Some(long), Some(short)) = (long, short) {
        report_finish_census(short, long);
    }
    measure_boot("BOOT  after the ride ended", Some(PLAN_BOOT_US));
    info!("PHASE two done. `probe-rs reset` now runs phase one again — the card has no ride left to recover.");
}

#[derive(Clone, Copy)]
struct FinishCensus {
    payload_len: u64,
    elapsed: u64,
    counters: Counters,
}

/// Continue the reset ride from its recovered CRC, append the fixed footer, issue the one final
/// commit that clears `RECORDING`, and prove the ordinary object is exactly the bytes recorded.
#[inline(never)]
fn ride_end(entry: &EntryMeta, recovered: RideRecovery) -> Option<FinishCensus> {
    let store = FlatStore::mount(Card);
    if store.recovered_ride() != Some(recovered) {
        error!("RCVR  a second mount did not select the same durable ride checkpoint; refusing continuation");
        return None;
    }
    // SAFETY: sole borrows.
    let delta = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_DELTA)).0 };
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };

    // Continue from the recovered checksum rather than re-hashing the prefix. This is the board's
    // reset path: durable tail bytes stay store-owned, and only new sample and footer bytes occupy
    // the recorder's bounded append buffer before the final checkpoint.
    let mut digest = Crc32::from_checksum(recovered.payload_crc);
    let mut held = 0;
    for point in RIDE_POINTS..FINAL_POINTS {
        let sample = ride_sample(point);
        delta[held..held + SAMPLE_LEN].copy_from_slice(&sample);
        held += SAMPLE_LEN;
        digest.update(&sample);
    }
    let footer = final_footer();
    delta[held..held + FOOTER_LEN].copy_from_slice(&footer);
    held += FOOTER_LEN;
    digest.update(&footer);
    let final_crc = digest.finalize();
    let resume = resume_image(FINAL_POINTS);

    arm();
    let started = Instant::now();
    if let Err(error) = store.journal(RideCheckpoint {
        id: entry.id,
        revision: entry.revision,
        append: &delta[..held],
        payload_crc: final_crc,
        resume: &resume,
    }) {
        error!("RCVR  continuation/footer checkpoint was refused ({})", defmt::Debug2Format(&error));
        return None;
    }
    let checkpoint_us = us(started);
    let checkpoint_io = counters();
    info!(
        "RCVR  continued with {=u32} samples and checkpointed the {=usize} B footer last: {=u64} us, {=u32} writes / {=u32} blocks",
        CONTINUATION_SAMPLES,
        FOOTER_LEN,
        checkpoint_us,
        checkpoint_io.writes,
        checkpoint_io.write_blocks
    );

    let meta = EntryMeta {
        added_at_utc: 0,
        id: entry.id,
        revision: entry.revision,
        kind: ObjectKind::Ride,
        flags: EntryFlags::NONE,
        payload_len: FINAL_RIDE_LEN,
        payload_crc: final_crc,
        name: DisplayName::new(RIDE_NAME).unwrap_or_default(),
    };
    arm();
    let started = Instant::now();
    if let Err(error) = store.commit(&[Mutation::Put { meta, source: PutSource::Amend }]) {
        error!("RCVR  the one final commit clearing RECORDING was refused ({})", defmt::Debug2Format(&error));
        return None;
    }
    let census = FinishCensus { payload_len: FINAL_RIDE_LEN, elapsed: us(started), counters: counters() };
    info!(
        "RCVR  FINAL COMMIT cleared RECORDING for {=u64} B: {=u64} us, {=u32} reads / {=u32} blocks, {=u32} writes / {=u32} blocks, {=u32} syncs",
        census.payload_len,
        census.elapsed,
        census.counters.reads,
        census.counters.read_blocks,
        census.counters.writes,
        census.counters.write_blocks,
        census.counters.syncs
    );
    report_split("FINISH long", census.elapsed, &census.counters);

    // The GET claim: open the now-ordinary object and compare every served byte with the same
    // production encoders used above. There is no finish-time conversion oracle in the middle.
    match store.open(entry.id, None) {
        Ok(handle) => {
            let mut served_crc = Crc32::new();
            let mut offset = 0u64;
            let mut bad = None;
            while offset < FINAL_RIDE_LEN {
                let want = ((FINAL_RIDE_LEN - offset) as usize).min(readback.len());
                let Ok(got) = store.read(&handle, offset, &mut readback[..want]) else {
                    error!("RCVR  the finalised ride would not read at {=u64} B", offset);
                    store.close(handle);
                    return None;
                };
                if got == 0 {
                    error!("RCVR  ordinary GET ended early at {=u64} B", offset);
                    store.close(handle);
                    return None;
                }
                bad = bad.or_else(|| {
                    readback[..got].iter().enumerate().find_map(|(at, byte)| {
                        let absolute = offset + at as u64;
                        let expected = if absolute < FINAL_SAMPLE_BYTES {
                            sample_byte(absolute)
                        } else {
                            footer[(absolute - FINAL_SAMPLE_BYTES) as usize]
                        };
                        (*byte != expected).then_some(absolute)
                    })
                });
                served_crc.update(&readback[..got]);
                offset += got as u64;
            }
            match bad {
                None if served_crc.finalize() == final_crc => info!(
                    "RCVR  ordinary GET served all {=u32} × 20-byte samples plus the {=usize}-byte footer unchanged ({=u64} B exact)",
                    FINAL_POINTS,
                    FOOTER_LEN,
                    FINAL_RIDE_LEN
                ),
                None => {
                    error!("RCVR  the ordinary GET bytes have the wrong final CRC");
                    store.close(handle);
                    return None;
                }
                Some(at) => {
                    error!("RCVR  the ordinary GET differs from recorded bytes at offset {=u64}", at);
                    store.close(handle);
                    return None;
                }
            }
            store.close(handle);
        }
        Err(error) => error!("RCVR  the finalised ride would not open ({})", defmt::Debug2Format(&error)),
    }

    // The read-path object phase one published, spot-checked rather than swept: the sweep is phase
    // one's measurement, and phase two asks only whether it survived the reset.
    let big = store.entries().find(|entry| entry.kind == ObjectKind::MapShard);
    if let Some(meta) = big {
        spot_check(&store, &meta);
    }
    Some(census)
}

/// Build and finish a small comparison ride after the recovered long one. Its final tail occupies
/// one card block just like the long ride's, while its durable prefix is orders of magnitude shorter.
fn short_ride() -> Option<FinishCensus> {
    let store = FlatStore::mount(Card);
    let id = store.next_object_id();
    let allocation = match store.allocate(RIDE_RESERVE) {
        Ok(allocation) => allocation,
        Err(error) => {
            error!("FINISH short 32 MiB reserve was refused ({})", defmt::Debug2Format(&error));
            return None;
        }
    };
    let recording = EntryMeta {
        added_at_utc: 0,
        id,
        revision: Revision(1),
        kind: ObjectKind::Ride,
        flags: EntryFlags::RECORDING,
        payload_len: 0,
        payload_crc: 0,
        name: DisplayName::default(),
    };
    if let Err(error) = store.commit(&[Mutation::Put { meta: recording, source: PutSource::Fresh(allocation) }]) {
        error!("FINISH short start was refused ({})", defmt::Debug2Format(&error));
        return None;
    }

    // SAFETY: sole borrow after the long finish has returned.
    let delta = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_DELTA)).0 };
    let mut digest = Crc32::new();
    let mut held = 0usize;
    for point in 0..SHORT_SAMPLES {
        let sample = ride_sample(point);
        delta[held..held + SAMPLE_LEN].copy_from_slice(&sample);
        held += SAMPLE_LEN;
        digest.update(&sample);
    }
    let footer = short_footer();
    delta[held..held + FOOTER_LEN].copy_from_slice(&footer);
    held += FOOTER_LEN;
    digest.update(&footer);
    let payload_crc = digest.finalize();
    let resume = resume_image(SHORT_SAMPLES);
    if let Err(error) = store.journal(RideCheckpoint {
        id,
        revision: Revision(1),
        append: &delta[..held],
        payload_crc,
        resume: &resume,
    }) {
        error!("FINISH short checkpoint was refused ({})", defmt::Debug2Format(&error));
        return None;
    }

    let final_meta = EntryMeta {
        added_at_utc: 0,
        id,
        revision: Revision(1),
        kind: ObjectKind::Ride,
        flags: EntryFlags::NONE,
        payload_len: held as u64,
        payload_crc,
        name: DisplayName::new(SHORT_NAME).unwrap_or_default(),
    };
    arm();
    let started = Instant::now();
    if let Err(error) = store.commit(&[Mutation::Put { meta: final_meta, source: PutSource::Amend }]) {
        error!("FINISH short final commit was refused ({})", defmt::Debug2Format(&error));
        return None;
    }
    let census = FinishCensus { payload_len: held as u64, elapsed: us(started), counters: counters() };
    report_split("FINISH short", census.elapsed, &census.counters);
    Some(census)
}

fn report_finish_census(short: FinishCensus, long: FinishCensus) {
    info!(
        "FINISH O(1) CENSUS short {=u64} B: {=u64} us, R {=u32}/{=u32} blocks, W {=u32}/{=u32} blocks, S {=u32}; long {=u64} B: {=u64} us, R {=u32}/{=u32} blocks, W {=u32}/{=u32} blocks, S {=u32}",
        short.payload_len,
        short.elapsed,
        short.counters.reads,
        short.counters.read_blocks,
        short.counters.writes,
        short.counters.write_blocks,
        short.counters.syncs,
        long.payload_len,
        long.elapsed,
        long.counters.reads,
        long.counters.read_blocks,
        long.counters.writes,
        long.counters.write_blocks,
        long.counters.syncs
    );
    // One additional catalog row can move a streamed catalog prefix across one block boundary. No
    // operation may scale with the ride's recorded prefix, and both final tails fit one block.
    let bounded = short.counters.read_blocks.abs_diff(long.counters.read_blocks) <= 1
        && short.counters.write_blocks.abs_diff(long.counters.write_blocks) <= 1
        && short.counters.syncs == long.counters.syncs;
    if bounded {
        info!("FINISH O(1) PASS: long-prefix finish I/O is the short-prefix census ± one catalog block");
    } else {
        error!("FINISH O(1) FAIL: final commit I/O changed by more than the one-block catalog-shape allowance");
    }
}

/// A few pages of a published object, read at random and compared with the pattern that wrote it.
fn spot_check(store: &FlatStore<Card>, meta: &EntryMeta) {
    // SAFETY: sole borrows.
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    let Ok(handle) = store.open(meta.id, None) else {
        error!("SPOT  object {=u64} would not open after the reset", meta.id.0);
        return;
    };
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut bad = 0u32;
    for _ in 0..32 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let offset = (state % (meta.payload_len / CHUNK as u64)) * CHUNK as u64;
        let Ok(got) = store.read(&handle, offset, readback) else {
            error!("SPOT  object {=u64} would not read at {=u64} B", meta.id.0, offset);
            store.close(handle);
            return;
        };
        if readback[..got].iter().enumerate().any(|(at, byte)| *byte != (at * 7 + 11) as u8) {
            bad += 1;
        }
    }
    if bad == 0 {
        info!(
            "SPOT  object {=u64} ({=u64} MiB, published before the reset): 32 random pages read back intact",
            meta.id.0,
            meta.payload_len / EXTENT_SIZE
        );
    } else {
        error!("SPOT  object {=u64}: {=u32} of 32 random pages came back wrong", meta.id.0, bad);
    }
    store.close(handle);
}

/// Where a measured interval went: the card's write half, its read half, and what was left over
/// for the M33.
///
/// Every figure this bench reports carries this line, because the halves have different causes and
/// different fixes. A commit that spends 79 blocks of writing and 156 blocks of reading is not one
/// figure: the reads are `merge` streaming the live prefix twice plus `find`'s binary search, and
/// dividing the total by the blocks written attributes all of it to the program cycle.
fn report_split(label: &str, elapsed: u64, counted: &Counters) {
    info!(
        "{=str}: {=u64} us WRITE {=u64} us ({=u32} calls / {=u32} blocks = {=u64} us per block) · READ {=u64} us ({=u32} / {=u32} = {=u64} us per block) · M33 {=u64} us",
        label,
        elapsed,
        counted.write_us,
        counted.writes,
        counted.write_blocks,
        counted.write_us / u64::from(counted.write_blocks.max(1)),
        counted.read_us,
        counted.reads,
        counted.read_blocks,
        counted.read_us / u64::from(counted.read_blocks.max(1)),
        elapsed.saturating_sub(counted.read_us + counted.write_us)
    );
}

/// The mean, median, least and greatest of a sample set, so a single-sample figure is never quoted
/// as if it were the cost.
///
/// The median earns its place: this card produces occasional multi-second commits (1.2 s and 3.3 s
/// have both been seen mid-ladder), and one of those inside a three-sample set moves the mean by
/// more than every effect this bench is trying to measure.
fn spread(samples: &[u64]) -> (u64, u64, u64, u64) {
    let mean = samples.iter().sum::<u64>() / samples.len() as u64;
    let mut sorted = [0u64; COMMIT_SAMPLES];
    sorted[..samples.len()].copy_from_slice(samples);
    sorted[..samples.len()].sort_unstable();
    let median = sorted[samples.len() / 2];
    (mean, median, sorted[0], sorted[samples.len() - 1])
}

/// One measured figure against its plan figure: within plan, a miss, or past 2x a miss that goes
/// back to the epic.
fn verdict(label: &str, measured: u64, plan: u64) {
    let ratio = measured * 100 / plan.max(1);
    if measured <= plan {
        info!("{=str}: WITHIN PLAN — {=u64} us against {=u64} us ({=u64}/100x)", label, measured, plan, ratio);
    } else if ratio <= 200 {
        warn!("{=str}: MISS — {=u64} us against {=u64} us ({=u64}/100x, inside 2x)", label, measured, plan, ratio);
    } else {
        error!(
            "{=str}: MISS >2x — {=u64} us against {=u64} us ({=u64}/100x) — flag on #1256",
            label, measured, plan, ratio
        );
    }
}

fn us(since: Instant) -> u64 {
    Instant::now().duration_since(since).as_micros()
}

/// Bytes per microsecond scaled to kB/s, saturating rather than dividing by zero.
fn rate(bytes: u64, elapsed_us: u64) -> u64 {
    if elapsed_us == 0 {
        return 0;
    }
    bytes * 1_000 / elapsed_us
}

/// Everything is measured; hold here so the RTT session stays attached and a `probe-rs reset`
/// starts the next cycle cleanly.
fn park() -> ! {
    info!("flat_store_bench: done — parked");
    loop {
        cortex_m::asm::wfi();
    }
}

/// Stack high-water: paint the free stack with a sentinel at boot, then find the lowest word that
/// is still painted. The scan must run bottom-up to the first non-painted word, because a frame
/// does not write every word it covers and a top-down scan under-reports by whole buffers.
mod stackmeter {
    const PAINT: u32 = 0xC0DE_DEAD;

    extern "C" {
        static _stack_start: u32;
        static _stack_end: u32;
    }

    fn top() -> usize {
        core::ptr::addr_of!(_stack_start) as usize
    }

    fn bottom() -> usize {
        core::ptr::addr_of!(_stack_end) as usize
    }

    /// Paints everything below the current SP (less a margin) down to the stack bottom.
    pub fn paint() {
        let sp = cortex_m::register::msp::read() as usize;
        let mut at = bottom();
        let stop = sp.saturating_sub(512);
        while at < stop {
            // SAFETY: the range is inside this binary's own stack region and below the live frame.
            unsafe { (at as *mut u32).write_volatile(PAINT) };
            at += 4;
        }
    }

    /// Bytes of stack used at the deepest point reached so far.
    pub fn used() -> usize {
        let (top, bottom) = (top(), bottom());
        let mut at = bottom;
        while at < top {
            // SAFETY: reading this binary's own stack region.
            if unsafe { (at as *const u32).read_volatile() } != PAINT {
                break;
            }
            at += 4;
        }
        top - at
    }

    /// Total usable stack.
    pub fn total() -> usize {
        top() - bottom()
    }

    /// Arms the ARMv8-M `MSPLIM`, so an overflow faults at the moment of overflow instead of
    /// silently smashing whatever static tops `.bss`.
    pub fn arm_limit() {
        const HANDLER_MARGIN: usize = 512;
        // SAFETY: raising a fault on genuine overflow is strictly safer than silent corruption.
        unsafe { cortex_m::register::msplim::write((bottom() + HANDLER_MARGIN) as u32) };
    }
}
