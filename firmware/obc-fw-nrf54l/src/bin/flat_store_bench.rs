//! **Flat store bench** — the whole of `obc_storage::flat` on the real card (FS4, #1386).
//!
//!     cargo run --release --bin flat_store_bench
//!
//! `FLAT_Store_Format.md` states five figures the board is the only place to check: a first boot
//! "well under a second", a mount "about 100 ms", a commit "about 15–20 ms at a few hundred
//! entries", a read path that is arithmetic rather than a chain walk, and a resident cost of the
//! 8 KiB free bitmap plus the ride tail. This binary produces a measured number for each of them
//! against a card in the slot, in the shape #1375 and #1379 established for OBC2: RTT-driven,
//! destructive, self-reporting, and never shipped — the app image does not link it.
//!
//! It brings up only the sEMMC card: no display, no app, no BLE, no sensors. The store owns the
//! **raw card from LBA 0** — there is no partition table and no filesystem — so a run destroys
//! whatever was on it, including a FAT volume. It refuses to touch a card that already carries a
//! flat store under someone else's `StoreId`.
//!
//! ## What it measures, in the order it runs
//!
//! Phase one, on a card this bench may destroy:
//!
//! 1. **Initialization (§8).** Two superblocks, one gate, sixteen slot headers, one empty catalog.
//! 2. **Mount (§5.6)**, at four catalog shapes: empty, [`LADDER_MID`] entries, [`LADDER_TOP`]
//!    entries, and with a ride recording (which is the only shape that reads the journal).
//! 3. **The commit ladder (§5.5).** One create per step, and at 0, [`LADDER_MID`] and
//!    [`LADDER_TOP`] entries a create/remove pair repeated [`COMMIT_SAMPLES`] times, so the reported
//!    figure carries a spread rather than being one observation. Also [`measure_opens`]: what §5.3's
//!    lookup costs twelve times over, which is a rendered set coming up.
//! 4. **The ride journal (§7.2).** One checkpoint every [`RIDE_GROWTH`] bytes, timed, split by
//!    whether it flushed a payload page.
//! 5. **The read path (§6.1)** into a multi-GiB object: one sequential sweep and three random
//!    passes, each with the **read amplification** — device blocks read over payload blocks
//!    required — which is the flat store's version of #1379's read-ratio check.
//! 6. **Resident cost** as a build assertion (see [`RESIDENT`]), plus the stack high-water the whole
//!    run reached.
//!
//! Every timed figure is reported through [`report_split`]: the card's write half, its read half and
//! the M33's residue, measured *inside* the block-device adapter. A commit is not one number — at 300
//! entries it writes 79 blocks and reads 156, and attributing the reads to the program cycle is how
//! the first round of this bench got its headline wrong.
//!
//! Phase two, after `probe-rs reset`, on the ride phase one left recording:
//!
//! 7. **Recovery (§7.3)**, through the store's own [`FlatStore::recovered_ride`] — not a
//!    reimplementation of it — and then §7.2's ride end, after which the ride is read back byte for
//!    byte against the payload phase one generated.
//!
//! ## What it does NOT prove — read this before quoting the results
//!
//! A `probe-rs reset` is a **CPU reset, not a power cut**. The card keeps its supply and never sees
//! the mid-page interruption §1's fault model is about, so nothing here says anything about tearing;
//! that is FS1's rig (#1383). What the reset loop validates is that a mount reconstructs the catalog
//! and the ride from the card alone, which is a different claim and the one this bench is for.
//!
//! ## Bring-up
//!
//! `semmc.rs` is pulled in by path and has no `crate::` dependencies, so this binary owns its own
//! host instance and never touches the display mux. The M33 must be at CK128 and `VPR00` bound,
//! exactly as in `obc2_store_bench`.
#![no_std]
#![no_main]

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::Instant;
use obc_crc::Crc32;
use obc_storage::flat::store::{MAX_OPEN_OBJECTS, MAX_RESERVATIONS};
use obc_storage::flat::{
    BlockDevice, DisplayName, EntryFlags, EntryMeta, FlatStore, Handle, Mode, Mutation, ObjectId, ObjectKind,
    PutSource, Revision, RideCheckpoint, RideRecovery, Store as _, StoreId,
};

// The critical-section impl comes from linking nrf-mpsl (the default `ble` feature set); MPSL is
// never initialised here, and its impl works from reset — the same arrangement the OBC2 benches use.
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

// ── the format constants this bench needs ───────────────────────────────────────────────────────
//
// `obc_storage::flat::layout` is `pub(crate)` — an LBA, an extent and a program page are the store's
// business and nothing above the seam may name them. A bench sits beside the store rather than above
// it, so it restates the four it needs from `FLAT_Store_Format.md` §2, §6, §7.1 and §9 rather than
// widening the seam for instrumentation. Each one is checked against the store's own behaviour by the
// measurements below: a wrong constant here shows up as an allocation the store refuses or a
// checkpoint that flushes a different number of pages than this bench predicted.

/// One extent (§6).
const EXTENT_SIZE: u64 = 1 << 20;
/// The extent area begins here, in blocks (§6).
const EXTENT_AREA: u64 = 4_096;
/// The media program page, and the granule §7.2 flushes ride payload in (§1, §2).
const PROGRAM_PAGE: usize = 16_384;
/// Tail bytes one journal slot carries (§7.1) — the ceiling a recording caller's buffer must meet.
const TAIL_CAPACITY: usize = 32_256;
/// The resident free bitmap (§9). Named here only to decompose the footprint report.
const FREE_BITMAP: usize = 8 * 1_024;

// ── the plan figures this bench is checking ─────────────────────────────────────────────────────

/// §5.6: "boot is about 100 ms".
const PLAN_BOOT_US: u64 = 100_000;
/// §5.5: "about 15–20 ms at a few hundred entries". The upper end, so a pass is unambiguous.
const PLAN_COMMIT_US: u64 = 20_000;
/// §8's initialization: "well under 1 s" (#1386).
const PLAN_INIT_US: u64 = 1_000_000;
/// The epic's resident figure: §9's 8 KiB bitmap, the 32,256-byte ride tail a recording caller
/// holds, and the store's rows — about 42 KiB together.
const PLAN_RESIDENT: usize = 42 * 1_024;

// ── the bench's own shape ───────────────────────────────────────────────────────────────────────

/// The bench's StoreId. A real initialization draws 128 CSPRNG bits (§4); a fixed value here makes a
/// store from an earlier run recognisable as this bench's rather than a live one's.
const BENCH_STORE: StoreId =
    StoreId([0xF5, 0x04, 0xF1, 0xA7, 0x5B, 0xE2, 0x11, 0x30, 0x9C, 0x64, 0xAD, 0x77, 0x02, 0xEE, 0x38, 0x41]);

/// Flip to `true` for one flash to wipe a card that carries **another** store's `StoreId`, or to
/// force phase one over this bench's own recorded ride instead of recovering it.
const FORCE_REINIT: bool = false;

/// Where the commit ladder reports its middle figure — §5.5's "a few hundred entries".
const LADDER_MID: u16 = 300;
/// Where it stops. One commit runs with the catalog holding exactly this many entries.
const LADDER_TOP: u16 = 1_024;
/// One ladder object's payload. One block, so the object costs the minimum §6 can allocate — one
/// extent — and the ladder's card cost is `LADDER_TOP` MiB rather than its payload.
const LADDER_PAYLOAD: usize = 512;

/// Samples taken at each reported catalog size. Three is enough to show whether a figure is stable;
/// the same commit moved ten per cent between two runs of the first round of this bench.
const COMMIT_SAMPLES: usize = 3;

/// The ride reserve §9 budgets: 32 MiB.
const RIDE_RESERVE: u64 = 32 * EXTENT_SIZE;
/// Payload bytes one checkpoint interval adds. A recorded ride is a few hundred bytes a second and
/// §9's cadence is 10 s, so this is one interval of a real ride rounded to something a reader can
/// multiply: nine checkpoints fill a program page, so the page flush lands mid-run rather than on a
/// boundary the bench chose.
const RIDE_GROWTH: usize = 2_048;
/// Checkpoints phase one writes. Enough to flush several payload pages and to wrap past the sixteen
/// slots, so the slot §7.3 selects is not the first one written — and deliberately not a multiple of
/// eight, so the ride is left with a partial page in its slot and phase two's ride end has to move
/// those bytes into the extents rather than finding everything already flushed.
const RIDE_CHECKPOINTS: u64 = 23;

/// What phase one's ride adds up to, from the constants it recorded with. Phase two anchors every
/// expectation here and never on what recovery reported — see the comment in [`phase_two`].
const RIDE_LEN: u64 = RIDE_CHECKPOINTS * RIDE_GROWTH as u64;
/// Payload bytes §7.2 will have flushed into the ride's own extents at that point.
const RIDE_FLUSHED: u64 = RIDE_LEN / PROGRAM_PAGE as u64 * PROGRAM_PAGE as u64;
/// And what is left in the newest journal slot: deliberately not zero.
const RIDE_TAIL_LEN: u32 = (RIDE_LEN - RIDE_FLUSHED) as u32;

/// The read-path object, if the card has room for it. `FLAT_Store_Format.md` §6.1's addressing is
/// arithmetic over 1 MiB extents, and only an object spanning thousands of them exercises it.
const BIG_TARGET: u64 = 2 * 1_024 * EXTENT_SIZE;
/// Below this the bench scales the object down and says so rather than skipping the read path.
const BIG_MINIMUM: u64 = 64 * EXTENT_SIZE;
/// One `Store::write` / `Store::read` call's span. 64 blocks per device command.
const CHUNK: usize = 32 * 1_024;
/// Random reads per pass.
const RANDOM_READS: u32 = 512;
/// One random read's length.
const RANDOM_LEN: usize = 4_096;

// ── the card ────────────────────────────────────────────────────────────────────────────────────

/// The one sEMMC host. Single-threaded and never re-entered, which is what makes the `&mut` sound.
static mut SEMMC: Semmc = Semmc::new();

/// A 4-byte-aligned byte buffer. The sEMMC firmware's DMA requires 32-bit alignment.
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// The misaligned-span bounce. The store hands the card `[u8; 512]` locals and a 4,096-byte pad out
/// of rodata, neither of which carries an alignment attribute, so every buffer the driver would
/// refuse comes through here.
static mut BOUNCE: Aligned<4_096> = Aligned([0; 4_096]);

/// What the card was asked to do. `reads` and `writes` are calls; the `_blocks` fields are what those
/// calls covered, which is what makes an amplification ratio computable rather than inferred.
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

/// Zeroes the counters. Named for what a caller does with it: arm, run one thing, read the counters.
fn arm() {
    // SAFETY: as above.
    unsafe { *core::ptr::addr_of_mut!(COUNTERS) = Counters::default() };
}

/// The [`BlockDevice`] over the sEMMC host: zero-sized, because all the state is in [`SEMMC`], which
/// is what lets the counters be read while a [`FlatStore`] owns the device by value.
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
        // seam is the card *and* the M33, and the two have different fixes: attributing one to the
        // other is exactly the error this bench's first round made.
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

    /// **The one thing this transport gets for free.**
    ///
    /// `Semmc::write_blocks` does not return until CMD13 says the card has left `prg` — the program
    /// cycle *is* its completion signal — so every write is already durable by the time the store's
    /// next statement runs. There is nothing left for a sync to do, and it costs nothing.
    ///
    /// This matters for reading §5.5's commit budget, which calls the three synchronizations the
    /// dominant term: on this card they are free and the commit's whole cost is its block writes. A
    /// transport with a write-back cache would move that cost back here, and the numbers this bench
    /// reports for a commit would move with it. The syncs are counted so the report can say so.
    fn sync(&self) -> Result<(), SemmcError> {
        Card::count(|c| c.syncs += 1);
        Ok(())
    }
}

// ── the payloads ────────────────────────────────────────────────────────────────────────────────

/// The repeating pattern the read-path object is made of, and the buffer every `Store::write` of it
/// comes from.
static mut PATTERN: Aligned<CHUNK> = Aligned([0; CHUNK]);
/// Where a read comes back for the byte comparison.
static mut READBACK: Aligned<CHUNK> = Aligned([0; CHUNK]);
/// The recording caller's tail buffer: §7.1's ceiling, which is the figure §9's resident budget
/// carries and not the couple of pages a real ride is ever holding.
static mut RIDE_TAIL: Aligned<TAIL_CAPACITY> = Aligned([0; TAIL_CAPACITY]);

/// The ride's payload byte at `offset`. Deterministic, so the boot after a reset can regenerate
/// exactly what the boot before it recorded and compare byte for byte.
fn ride_byte(offset: u64) -> u8 {
    (offset.wrapping_mul(7).wrapping_add(11) ^ (offset >> 11)) as u8
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = {
        let mut config = embassy_nrf::config::Config::default();
        // Not optional: the sEMMC clock divisors and the firmware's wait slices are stated against
        // a 128 MHz core.
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
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
    // §6's extent count, from the card's block count. The store computes the same thing from the
    // superblock; a mismatch would show up as an allocation it refuses.
    let extents = ((u64::from(card.blocks).saturating_sub(EXTENT_AREA)) / (EXTENT_SIZE / 512)).min(65_536);
    info!("CARD  §6 extent area: {=u64} extents of 1 MiB ({=u64} MiB addressable)", extents, extents);

    run();
    info!("STACK high-water across the whole run: {=usize} B of {=usize} B", stackmeter::used(), stackmeter::total());
    park();
}

/// The whole run, in a plain function rather than in the async `main`.
///
/// Deliberate, and the #1379 lesson restated: an async fn's locals are permanent poll-frame slots,
/// and this one places stores whose type is ten kilobytes. In an ordinary call the same locals are
/// scoped to the frame and come back at return — which is what makes the stack figure this bench
/// prints a measurement of the store's peak rather than of the executor's permanent reservation.
#[inline(never)]
fn run() {
    report_footprint();

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

// ── phase one ───────────────────────────────────────────────────────────────────────────────────

fn phase_one() {
    info!("PHASE one: initialize, then measure boot, commit, ride and read on a card built from scratch");

    let Some(extents) = initialize() else { return };

    measure_boot("BOOT  empty catalog", Some(PLAN_BOOT_US));

    // How much of the card the three phases may take. The ladder is one extent per object; the ride
    // takes §9's 32 MiB reserve; the read-path object takes whatever is left, up to 2 GiB.
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

/// §8: two superblocks invalidated, gate B invalidated, sixteen slot headers invalidated, one empty
/// catalog body, its gate, then both superblocks — five synchronization points, and the mount that
/// follows is part of what `initialize` returns. Hands back the free extents the card came up with.
///
/// It is a function of its own so the store it builds goes out of scope with it: a boot figure has to
/// be a mount's own cost, and a store left standing here would sit on the stack for the whole run.
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

// ── 2 and 3: mount, and the commit ladder ───────────────────────────────────────────────────────

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

/// One mount, timed, with the entry the journal half of §5.6 keys on.
///
/// The store is dropped before this returns: a boot figure is what a mount costs, and holding one of
/// these open would put a second ten-kilobyte store beside whichever one the caller already has.
///
/// `plan` is a parameter and a mount that recovered a ride is reported against **no** budget,
/// because §5.6's ~100 ms is scoped to its own sentence: "on a card with no ride in progress a mount
/// reads at most 3 blocks plus the live catalog prefix". A mount that reads sixteen slot headers and
/// CRCs a 32 KiB slot is doing more than that figure covers, and the spec states no figure for it.
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

/// §5.5's commit, from an empty catalog to [`LADDER_TOP`] entries, one create per step.
///
/// Every step is `allocate` → `write` → `commit`, which is the whole publication path; only the
/// commit is on the clock, because it is the one §5.5 puts a figure on. Returns the free extents
/// left behind.
fn ladder(top: u16) -> u32 {
    let store = FlatStore::mount(Card);
    let mut total = 0u64;
    let mut worst = 0u64;
    let mut worst_at = 0u16;
    let mut publish_total = 0u64;
    for _ in 0..=top {
        let entries = store.entry_count();
        // At a reported catalog size the figure is **sampled**, not taken once: the same commit moved
        // ten per cent between two runs of this bench, so one observation is an anecdote rather than
        // a cost. The ladder's own commit below still feeds the aggregate.
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

/// One timed commit: what it cost, what the card did, and **which catalog copy it landed on**.
struct Commit {
    elapsed: u64,
    counted: Counters,
    /// §5.5 writes the copy that is not being served and then serves it, so the copies strictly
    /// alternate from initialization — which serves copy 0 at sequence 1. A commit that produced
    /// sequence `s` therefore wrote copy `1 - (s % 2)`, and that survives any number of mounts in
    /// between because a mount serves whichever copy carries the greater sequence.
    copy: usize,
    id: ObjectId,
}

/// Which catalog copy the commit that produced `sequence` wrote.
fn copy_of(sequence: u64) -> usize {
    1 - (sequence % 2) as usize
}

/// One create: allocate, write the payload, commit. Only the commit is on the clock, because it is
/// the step §5.5 puts a figure on.
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

/// §5.5's figure at one catalog size, sampled [`COMMIT_SAMPLES`] times **per catalog copy**.
///
/// The shape of this is the whole point, and the first version of it was confounded. §5.5 alternates
/// copies, so a create-then-remove sample sends every create to one copy and every remove to the
/// other: the 22% gap that appeared between the two was a *copy* difference wearing a mutation-kind
/// costume. Two creates in a row fix it — they land on the two copies with everything else equal —
/// and the two removals that undo them do the same, so each sample yields four figures:
///
/// | | copy A | copy B |
/// | create | first  | second |
/// | remove | third  | fourth |
///
/// Four commits per sample keeps the parity, so every sample repeats the same assignment. The entry
/// count moves by one between the paired commits, which changes no block count at any size this
/// bench reports (`1 + ceil(n/4) + 2` is flat across `n` and `n+1` at 0, 300 and 1024).
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
    // §5.5: `ceil(n/4) + 3` block writes — the body's `1 + ceil(n/4)` blocks (header included), the
    // gate invalidation and the gate itself — and three synchronizations.
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
    // The figure a device actually pays: §5.5 alternates, so consecutive commits pay one copy each.
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

/// What `open` costs — §5.3's binary search over the live prefix, then the hold row.
///
/// Twelve of them, because [`MAX_OPEN_OBJECTS`] is sized for the eleven map shards a rendered set
/// mounts plus one transfer: this is the figure a renderer pays to bring a set up. The same search is
/// `find`, which every commit's `resolve` runs once per mutation — so it is also half of why the
/// commit figures above carry the read time they do.
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

// ── 4: the ride journal ─────────────────────────────────────────────────────────────────────────

/// §7.2's write half: start a ride, then one checkpoint per [`RIDE_GROWTH`] bytes, timed.
///
/// The ride is left **recording** on purpose. It is what phase two recovers.
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
    // §5.3: a `RECORDING` entry holds slack — it owns more extents than its payload needs — which is
    // the one thing that lets a ride grow without a commit per page.
    let meta = EntryMeta {
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

    // SAFETY: sole borrow of the tail slot; nothing else reads it.
    let tail = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_TAIL)).0 };
    let mut payload_len = 0u64;
    let mut flushed = 0u64;
    let mut digest = Crc32::new();
    let mut plain_total = 0u64;
    let mut plain = 0u64;
    let mut flush_total = 0u64;
    let mut flushes = 0u64;
    let mut worst = 0u64;
    for sequence in 1..=RIDE_CHECKPOINTS {
        for step in 0..RIDE_GROWTH {
            let offset = payload_len + step as u64;
            let byte = ride_byte(offset);
            tail[(offset - flushed) as usize] = byte;
            digest.update(&[byte]);
        }
        payload_len += RIDE_GROWTH as u64;
        let held = (payload_len - flushed) as usize;
        let checkpoint =
            RideCheckpoint { id, revision: Revision(1), tail: &tail[..held], payload_crc: digest.finalize() };
        arm();
        let started = Instant::now();
        let outcome = store.journal(checkpoint);
        let elapsed = us(started);
        let counted = counters();
        if let Err(error) = outcome {
            error!("RIDE  checkpoint {=u64} refused ({})", sequence, defmt::Debug2Format(&error));
            return;
        }
        // §7.2 flushes whole payload pages out of the front of the tail, and the caller drops exactly
        // those bytes from its own — the one bookkeeping the seam leaves to the rider.
        let pages = held / PROGRAM_PAGE;
        if pages > 0 {
            tail.copy_within(pages * PROGRAM_PAGE..held, 0);
            flushed += (pages * PROGRAM_PAGE) as u64;
            flush_total += elapsed;
            flushes += 1;
            info!(
                "RIDE  checkpoint {=u64} flushed {=usize} payload page(s): {=u64} us, {=u32} writes / {=u32} blocks",
                sequence, pages, elapsed, counted.writes, counted.write_blocks
            );
        } else {
            plain_total += elapsed;
            plain += 1;
            if sequence == 1 {
                info!(
                    "RIDE  checkpoint 1 (one 32 KiB slot, no page flush): {=u64} us, {=u32} writes / {=u32} blocks, {=u32} syncs",
                    elapsed, counted.writes, counted.write_blocks, counted.syncs
                );
            }
        }
        worst = worst.max(elapsed);
    }
    info!(
        "RIDE  {=u64} checkpoints at {=usize} B each: {=u64} us mean without a page flush ({=u64} of them), {=u64} us mean with one ({=u64}), {=u64} us worst",
        RIDE_CHECKPOINTS,
        RIDE_GROWTH,
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
        "RIDE  left recording: object {=u64}, {=u64} B recorded, {=u64} B flushed, crc 0x{=u32:08x}",
        id.0,
        payload_len,
        flushed,
        digest.finalize()
    );
}

// ── 5: the read path ────────────────────────────────────────────────────────────────────────────

/// §6.1 over a multi-GiB object: write it, sweep it, then hit it at random.
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

    // The write, with the CRC fold on its own clock: `obc-crc` is a byte-at-a-time table fold, and
    // rolling it into the write's rate would report the M33 rather than the card.
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

/// #1379's read-ratio check, in the shape a flat store makes it mean something.
///
/// OBC2's figure was **device reads per block**, and 1.00 was the diagnosis: the FAT layer's cache
/// could never hand the card more than one block, so a scan paid a command per 512 bytes. Here that
/// number is a batching figure and nothing more — the store issues one command per contiguous run,
/// so it is well under 1.00 by design and says only how long the runs were.
///
/// The claim §6.1 actually makes is the other ratio: **blocks read per block the payload occupies**.
/// There is no chain to walk and no indirection block to fetch, so a read that does not amplify is
/// exactly 1.00, and anything above it would be the store reading something it did not need.
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

// ── 6: resident cost ────────────────────────────────────────────────────────────────────────────

/// The resident total, and the build assertion that keeps it under §9's figure.
const RESIDENT: usize = core::mem::size_of::<FlatStore<Card>>() + TAIL_CAPACITY;

// **An assertion, not a measurement.** `size_of` is a compile-time fact, and the ~42 KiB plan figure
// is the sum of the same two addends — reporting one against the other as a ratio dresses an
// identity up as a result, which is what the first round of this bench did. So it is stated the
// honest way: the build fails if the store plus a recording caller's tail ever leaves §9's budget,
// and the number below is reported without a verdict. Nothing here is in `obc-storage`; the
// constants are §9's, restated at the top of this file.
const _: () = assert!(RESIDENT <= PLAN_RESIDENT, "the flat store plus §7.1's tail no longer fits §9's resident budget");
const _: () = assert!(core::mem::size_of::<FlatStore<Card>>() > FREE_BITMAP);

fn report_footprint() {
    let store = core::mem::size_of::<FlatStore<Card>>();
    let handle = core::mem::size_of::<obc_storage::flat::Handle>();
    info!(
        "RAM   [addend 1] FlatStore<Card> {=usize} B = the {=usize} B free bitmap (§6.2) + {=usize} reservation row(s) + {=usize} hold row(s) + the mounted rows",
        store, FREE_BITMAP, MAX_RESERVATIONS, MAX_OPEN_OBJECTS
    );
    info!(
        "RAM   [addend 2] the recording caller's tail buffer: {=usize} B — §7.1's ceiling, not the ~18 KiB a real ride holds",
        TAIL_CAPACITY
    );
    info!("RAM   [component] one open Handle: {=usize} B; the entry array is never resident (§5.1)", handle);
    info!("RAM   [component] this bench's own buffers, which are not the store's: {=usize} B", CHUNK * 2 + 4_096);
    info!(
        "RAM   TOTAL for one mounted store with a ride recording: {=usize} B. The build ASSERTS this stays within §9's {=usize} B — it is a compile-time identity, so it is reported without a verdict",
        RESIDENT, PLAN_RESIDENT
    );
}

// ── 7: phase two, the recovery half ─────────────────────────────────────────────────────────────

/// Everything the ride recorded before the reset must come back from the card alone, and then §7.2's
/// ride end must publish exactly those bytes.
fn phase_two(boot: &Boot) {
    info!("PHASE two: the card already carries this bench's store with a ride recording");
    let Some(entry) = boot.recording else { return };
    let Some(recovered) = boot.recovered else {
        error!("RCVR  §5.6 found a RECORDING entry but §7.3 recovered no checkpoint — a ride with no slot at all");
        return;
    };

    // **Every expectation below is anchored on the constants phase one wrote with, not on anything
    // the store just said.** Deriving the expected length from `checkpoint_sequence` — as the first
    // round of this bench did — makes the check self-fulfilling: a store that silently selected slot
    // 22 instead of 23 would hand back a shorter ride and a CRC over the shorter ride, and both would
    // "match". §7.4's loss cap is exactly the claim that would go unchecked.
    let mut digest = Crc32::new();
    for offset in 0..RIDE_LEN {
        digest.update(&[ride_byte(offset)]);
    }
    let expected_crc = digest.finalize();
    let mut ok = recovered.id == entry.id && recovered.revision == entry.revision;
    if recovered.checkpoint_sequence != RIDE_CHECKPOINTS {
        error!(
            "RCVR  §7.3 selected checkpoint {=u64}, but phase one wrote {=u64} — a checkpoint was lost or a stale slot won",
            recovered.checkpoint_sequence, RIDE_CHECKPOINTS
        );
        ok = false;
    }
    if (recovered.flushed, recovered.tail_len) != (RIDE_FLUSHED, RIDE_TAIL_LEN) {
        error!(
            "RCVR  recovery says {=u64} B flushed + {=u32} B tail; {=u64} checkpoints of {=usize} B is {=u64} + {=u32}",
            recovered.flushed, recovered.tail_len, RIDE_CHECKPOINTS, RIDE_GROWTH, RIDE_FLUSHED, RIDE_TAIL_LEN
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
    info!(
        "RCVR  §7.3 selected checkpoint {=u64} of 16 slots: {=u64} B flushed + {=u32} B tail = {=u64} B, crc 0x{=u32:08x}",
        recovered.checkpoint_sequence,
        recovered.flushed,
        recovered.tail_len,
        recovered.payload_len(),
        recovered.payload_crc
    );
    if ok {
        info!(
            "RCVR  the recovery matches the {=u64} checkpoints phase one wrote — sequence, flush point, length and CRC, all against the bench's own constants",
            RIDE_CHECKPOINTS
        );
    }

    ride_end(&entry, expected_crc);
    measure_boot("BOOT  after the ride ended", Some(PLAN_BOOT_US));
    info!("PHASE two done. `probe-rs reset` now runs phase one again — the card has no ride left to recover.");
}

/// §7.2's ride end and what it makes readable, with the store scoped to this call.
#[inline(never)]
fn ride_end(entry: &EntryMeta, expected_crc: u32) {
    let store = FlatStore::mount(Card);
    // SAFETY: sole borrows.
    let tail = unsafe { &mut (*core::ptr::addr_of_mut!(RIDE_TAIL)).0 };
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };

    // The tail the store hands back is the part of the payload that is in a journal slot rather than
    // in the ride's extents. It has to be the tail of what phase one generated.
    match store.recovered_tail(&mut tail[..RIDE_TAIL_LEN as usize]) {
        Ok(len) => {
            let bad = tail[..len]
                .iter()
                .enumerate()
                .find(|(at, byte)| **byte != ride_byte(RIDE_FLUSHED + *at as u64))
                .map(|(at, _)| at);
            match bad {
                None => info!("RCVR  the {=usize} B tail came back out of the slot byte for byte", len),
                Some(at) => error!("RCVR  the recovered tail differs from the ride at byte {=usize}", at),
            }
        }
        Err(error) => error!("RCVR  the tail would not come back ({})", defmt::Debug2Format(&error)),
    }

    // §7.2's ride end: one commit clears RECORDING, trims the reserve to the payload, and moves the
    // last partial page out of the slot and into the ride's own extents.
    let meta = EntryMeta {
        id: entry.id,
        revision: entry.revision,
        kind: ObjectKind::Ride,
        flags: EntryFlags::NONE,
        // The constants again, not the recovery's own numbers: if §7.3 had selected a stale slot,
        // this length would not match the tail that slot holds and the store would refuse the commit
        // — which is the loud failure a self-anchored expectation quietly turns into a pass.
        payload_len: RIDE_LEN,
        payload_crc: expected_crc,
        name: DisplayName::new("fs4-ride").unwrap_or_default(),
    };
    arm();
    let started = Instant::now();
    let outcome = store.commit(&[Mutation::Put { meta, source: PutSource::Amend }]);
    let end_us = us(started);
    let counted = counters();
    if let Err(error) = outcome {
        error!("RCVR  the ride end was refused ({})", defmt::Debug2Format(&error));
        return;
    }
    info!(
        "RCVR  §7.2 ride end (tail out of the slot, ranges trimmed, 16 slot headers zeroed): {=u64} us, {=u32} writes / {=u32} blocks",
        end_us, counted.writes, counted.write_blocks
    );

    // And now the whole ride is an ordinary object, so it reads back through the ordinary path.
    match store.open(entry.id, None) {
        Ok(handle) => {
            let mut digest = Crc32::new();
            let mut offset = 0u64;
            let mut bad = None;
            while offset < RIDE_LEN {
                let want = ((RIDE_LEN - offset) as usize).min(readback.len());
                let Ok(got) = store.read(&handle, offset, &mut readback[..want]) else {
                    error!("RCVR  the finalised ride would not read at {=u64} B", offset);
                    return;
                };
                bad = bad.or_else(|| {
                    readback[..got]
                        .iter()
                        .enumerate()
                        .find(|(at, byte)| **byte != ride_byte(offset + *at as u64))
                        .map(|(at, _)| offset + at as u64)
                });
                digest.update(&readback[..got]);
                offset += got as u64;
            }
            match bad {
                None if digest.finalize() == expected_crc => info!(
                    "RCVR  the whole {=u64} B ride read back byte for byte through the ordinary read path",
                    RIDE_LEN
                ),
                None => error!("RCVR  the finalised ride's CRC is not the one phase one recorded"),
                Some(at) => error!("RCVR  the finalised ride differs from what was recorded at byte {=u64}", at),
            }
            store.close(handle);
        }
        Err(error) => error!("RCVR  the finalised ride would not open ({})", defmt::Debug2Format(&error)),
    }

    // The read-path object phase one published, spot-checked rather than swept: the sweep is phase
    // one's measurement, and what phase two is asking is only whether it survived the reset.
    let big = store.entries().find(|entry| entry.kind == ObjectKind::MapShard);
    if let Some(meta) = big {
        spot_check(&store, &meta);
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

// ── verdicts and helpers ────────────────────────────────────────────────────────────────────────

/// Where a measured interval went: the card's write half, its read half, and what was left over for
/// the M33.
///
/// Every figure this bench reports carries this line, because the halves have different causes and
/// different fixes. A commit that spends 79 blocks of writing and 156 blocks of *reading* is not one
/// figure — the reads are `merge` streaming the live prefix twice plus `find`'s binary search, and
/// dividing the total by the blocks written attributes all of it to the program cycle. That is the
/// error the first round of this bench made, and this function is the fix.
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
/// The median earns its place here: this card produces occasional multi-second commits (1.2 s and
/// 3.3 s have both been seen mid-ladder, presumably its own housekeeping), and one of those inside a
/// three-sample set moves the mean by more than every effect this bench is trying to measure.
fn spread(samples: &[u64]) -> (u64, u64, u64, u64) {
    let mean = samples.iter().sum::<u64>() / samples.len() as u64;
    let mut sorted = [0u64; COMMIT_SAMPLES];
    sorted[..samples.len()].copy_from_slice(samples);
    sorted[..samples.len()].sort_unstable();
    let median = sorted[samples.len() / 2];
    (mean, median, sorted[0], sorted[samples.len() - 1])
}

/// One measured figure against its plan figure, in the form #1386 asks for: within plan, or a miss,
/// and past 2× a miss that goes back to the epic.
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

// ── the stack meter ─────────────────────────────────────────────────────────────────────────────

/// Stack high-water: paint the free stack with a sentinel at boot, then find the lowest word that
/// is still painted. The scan must run bottom-up to the first non-painted word — a frame does not
/// write every word it covers, so a top-down scan under-reports by whole buffers.
///
/// The same measurement `main.rs` makes, restated here because this binary links none of it.
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
