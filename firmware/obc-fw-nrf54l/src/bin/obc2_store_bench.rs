//! **OBC2 store bench** — the kernel transaction over the real card, end to end.
//!
//!     cargo run --release --bin obc2_store_bench
//!
//! `obc2_media_bench` established what the **media** does: §1.1's volume preconditions, §13.1's
//! clean flush, the cost of a gated commit, and that §6.3's decision is correct over durable
//! records. It stopped at the adapter, deliberately — it writes no checkpoint, holds no projection
//! and knows nothing about a claim.
//!
//! This is the next layer, and the one #1359 owes: `obc_storage::obc2::fat` composing the §13.1
//! adapter into the [`KernelMedia`] the DOS3 kernel runs against, with the **whole** upload
//! lifecycle — claim, append, seal, validate, publish, query — driven through
//! [`KernelTransaction`] on a card. It is the same transaction the host suite drives against a
//! faulting simulation; only the media underneath is different, which is the point.
//!
//! It brings up only the sEMMC card: no display, no app, no BLE, no sensors. It is **destructive**
//! — it deletes `/OBC2`'s fixed files and rebuilds the store — and it is a bench, never shipped.
//! Nothing here touches the live v1 storage path: this binary owns its own volume manager and the
//! app image does not link it.
//!
//! ## What it measures, in the order it runs
//!
//! 1. **Volume geometry (§1.1)**, as the media bench does, because a card that fails it is never
//!    written to and every later number would be meaningless.
//! 2. **Mount (§12).** The survey: the `/OBC2` listing, both checkpoints, all 256 journal slots,
//!    and §12's classification of what that adds up to — timed, on a fresh card and on a populated
//!    one. This is the figure a boot pays.
//! 3. **Initialization (§12), with lazy shards.** The seven fixed files and their 4,636,672 bytes of
//!    zero-fill, and **no** shard tree. The owner's 2026-08-16 decision predicts ~1.7 s against the
//!    75 s the eager tree cost; this is where that is confirmed or not.
//! 4. **One upload lifecycle through the kernel.** Claim → append → seal → validate → publish →
//!    QueryOperation, each step timed, with the publish's sectors recorded and classified so
//!    §13.1's clean flush is checked *through the whole stack* rather than at the adapter alone.
//! 5. **Resident cost.** `size_of` of every value the store places, against §13's 19,848-byte RAM
//!    index budget, plus the stack high-water the whole run reached.
//! 6. **Reboot recovery.** Reset the board (`probe-rs reset`) and run it again: the store must
//!    remount from the card alone with the head, the payload bytes and the retained result intact,
//!    and then commit one more object.
//!
//! ## What it does NOT prove — read this before quoting the results
//!
//! A `probe-rs reset` is a **CPU reset, not a power cut**. The card keeps its supply and never sees
//! the mid-page interruption §1.1's fault model is about, so nothing here says anything about
//! tearing. What the reset loop validates is that a mount reconstructs the catalog from durable
//! records, which is a different claim and the one this bench is for.
//!
//! ## Bring-up
//!
//! `semmc.rs` is pulled in by path and has no `crate::` dependencies, so this binary owns its own
//! host instance and never touches the display mux. The M33 must be at CK128 and `VPR00` bound,
//! exactly as in `obc2_media_bench`.
#![no_std]
#![no_main]

use core::mem::MaybeUninit;

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::Instant;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, RawDirectory, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use obc_link::engine::Transaction as _;
use obc_link::engine::{ClaimIntent, ClaimOutcome, Command, OperationReport, Outcome as EngineOutcome, PrincipalScope};
use obc_link::frame::Opcode;
use obc_link::ids::{GenerationId, LogicalObjectId, OperationId};
use obc_link::registry::ObjectKind;
use obc_link::upload::Target;
use obc_storage::obc2::adapter::Adapter;
use obc_storage::obc2::blocklog::WriteLog;
use obc_storage::obc2::fat::{self, FatMedia, SlotTable, Stride, Survey, NO_SLOTS};
use obc_storage::obc2::generation::GenerationMedia as _;
use obc_storage::obc2::geometry::{self, FatType, Region, VolumeGeometry};
use obc_storage::obc2::index;
use obc_storage::obc2::limits::{INITIALIZATION_ZERO_FILL, SLOT_STRIDE, WORK_FILE_LEN};
use obc_storage::obc2::mount::{Outcome, CREATION_ORDER};
use obc_storage::obc2::transaction::KernelMedia as _;
use obc_storage::obc2::transaction::{AcceptEverything, KernelTransaction, NoHooks};
use obc_storage::obc2::StoreId;
use obc_storage::shared_device::SharedBlockDevice;

// The critical-section impl comes from linking nrf-mpsl (the default `ble` feature set); MPSL is
// never initialised here, and its impl works from reset — the same arrangement the media bench uses.
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

// ── the block device ────────────────────────────────────────────────────────────────────────────

/// The one sEMMC host. Single-threaded and never re-entered, which is what makes the `&mut` sound.
static mut SEMMC: Semmc = Semmc::new();

/// A 4-byte-aligned byte buffer. The sEMMC firmware's DMA requires 32-bit alignment and
/// `embedded_sdmmc::Block` cannot promise it, so every buffer handed to the driver carries it.
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// The misaligned-span bounce, four blocks deep — the same shape `sd.rs` uses.
static mut BOUNCE: Aligned<2_048> = Aligned([0; 2_048]);

/// The `BlockDevice` over the sEMMC host: zero-sized, because all the state is in [`SEMMC`].
#[derive(Clone, Copy)]
struct Card;

impl Card {
    /// SAFETY: the caller must not be inside another `with` — this binary never is.
    fn with<R>(f: impl FnOnce(&mut Semmc) -> R) -> R {
        // SAFETY: single-threaded, non-re-entrant, and no interrupt handler touches the host state.
        f(unsafe { &mut *core::ptr::addr_of_mut!(SEMMC) })
    }
}

impl BlockDevice for Card {
    type Error = SemmcError;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, count) = (blocks.as_ptr() as usize, blocks.len());
        Card::with(|sd| {
            if addr.is_multiple_of(4) {
                // SAFETY: `Block` is `#[repr(transparent)]` over `[u8; 512]`, so a `&mut [Block]` is
                // exactly this byte span, exclusively borrowed for the call.
                let buf =
                    unsafe { core::slice::from_raw_parts_mut(blocks.as_mut_ptr().cast::<u8>(), count * BLOCK_BYTES) };
                return sd.read_blocks(start.0, buf);
            }
            // SAFETY: sole borrow; nothing else touches the bounce inside this call.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            for (chunk_index, chunk) in blocks.chunks_mut(4).enumerate() {
                let len = chunk.len() * BLOCK_BYTES;
                sd.read_blocks(start.0 + (chunk_index * 4) as u32, &mut bounce.0[..len])?;
                for (block, src) in chunk.iter_mut().zip(bounce.0[..len].as_chunks::<BLOCK_BYTES>().0) {
                    block.contents.copy_from_slice(src);
                }
            }
            Ok(())
        })
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, count) = (blocks.as_ptr() as usize, blocks.len());
        Card::with(|sd| {
            if addr.is_multiple_of(4) {
                // SAFETY: as in `read`, shared for the duration of the call.
                let buf = unsafe { core::slice::from_raw_parts(blocks.as_ptr().cast::<u8>(), count * BLOCK_BYTES) };
                return sd.write_blocks(start.0, buf);
            }
            // SAFETY: as in `read`.
            let bounce = unsafe { &mut *core::ptr::addr_of_mut!(BOUNCE) };
            for (chunk_index, chunk) in blocks.chunks(4).enumerate() {
                let len = chunk.len() * BLOCK_BYTES;
                for (block, dst) in chunk.iter().zip(bounce.0[..len].as_chunks_mut::<BLOCK_BYTES>().0) {
                    dst.copy_from_slice(&block.contents);
                }
                sd.write_blocks(start.0 + (chunk_index * 4) as u32, &bounce.0[..len])?;
            }
            Ok(())
        })
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        Card::with(|sd| sd.num_blocks()).map(BlockCount)
    }
}

/// The zero timestamp the board's storage uses.
struct NullTime;

impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// The instrumented card: 256 recorded spans is more than any single measurement below produces.
type Log = WriteLog<Card, 256>;
/// §13's handle budget: four directory handles reach a `GEN`/`WORK` leaf, sixteen file handles.
type Vmgr = VolumeManager<SharedBlockDevice<'static, Log>, NullTime, 4, 16, 1>;
type Fat = Adapter<'static, SharedBlockDevice<'static, Log>, NullTime, 4, 16, 1>;
type Media = FatMedia<'static, SharedBlockDevice<'static, Log>, NullTime, 4, 16, 1>;
type Store = KernelTransaction<Media, AcceptEverything, NoHooks>;

static mut LOG: MaybeUninit<Log> = MaybeUninit::uninit();
static mut VMGR: MaybeUninit<Vmgr> = MaybeUninit::uninit();

/// One 16,384-byte slot stride: a journal record, a `WORK` slot, or a zero-fill granule.
static mut STRIDE: Aligned<SLOT_STRIDE> = Aligned([0; SLOT_STRIDE]);
/// One whole checkpoint file. §13 budgets a *commit*'s staging at one journal body; a **mount**
/// validates and decodes the checkpoint as one slice, so this 65,536-byte buffer is the mount-time
/// figure and is reported as its own number rather than folded into the commit budget.
/// The 256 slot observations one all-slot scan produces: 10,240 bytes, in `.bss` and never on a
/// task frame.
static mut SLOTS: SlotTable = NO_SLOTS;
/// The kernel transaction, projection included. It is the largest single value this board places.
static mut STORE: MaybeUninit<Store> = MaybeUninit::uninit();
/// The scratch an engine command reads or writes through. No command this bench issues uses more.
static mut SCRATCH: Aligned<1_024> = Aligned([0; 1_024]);
/// The payload one lifecycle uploads.
static mut PAYLOAD: Aligned<PAYLOAD_LEN> = Aligned([0; PAYLOAD_LEN]);
/// Where a published head is read back for the byte comparison.
static mut READBACK: Aligned<PAYLOAD_LEN> = Aligned([0; PAYLOAD_LEN]);

/// Places `value` in a `.bss` slot and hands back the `'static` reference.
///
/// # Safety
///
/// The caller must call this at most once per slot, before anything reads it — a second call would
/// hand out a second `&'static mut` to the same storage.
unsafe fn init_static<T>(slot: *mut MaybeUninit<T>, value: T) -> &'static mut T {
    let slot = &mut *slot;
    slot.write(value);
    slot.assume_init_mut()
}

// ── the store this bench writes ─────────────────────────────────────────────────────────────────

/// The bench's StoreId. A real initialization generates 128 CSPRNG bits (§12); a fixed value here
/// makes a store from an earlier run recognisable as this bench's rather than a live one's.
const BENCH_STORE: StoreId =
    StoreId::new([0xB2, 0x57, 0xB2, 0x57, 0xB2, 0x57, 0xB2, 0x57, 0xB2, 0x57, 0xB2, 0x57, 0xB2, 0x57, 0xB2, 0x57]);

/// The principal every claim here is made under. One client, one scope.
const BENCH_PRINCIPAL: PrincipalScope = PrincipalScope::new([0x5B; 16]);

/// The canonical-intent digest §11 compares byte for byte. Fixed, so a replayed OperationId is the
/// *same* intent rather than a conflict.
const BENCH_DIGEST: [u8; 32] = [0x9E; 32];

/// The payload one lifecycle uploads: a few KB, which is what a route actually is.
const PAYLOAD_LEN: usize = 4_096;

/// How many bytes one `Append` command carries. Four appends per upload, so the per-append cost is
/// visible separately from the seal's.
const APPEND_CHUNK: usize = 1_024;

/// Past this many committed objects the bench reinitializes rather than filling the journal: §6.3's
/// compaction trigger is 192 records and the pass that would clear it is a later slice.
const REINITIALIZE_ABOVE: u64 = 60;

/// Flip to `true` for one flash to force the destructive path — the wipe, the §12 skeleton and the
/// initialization timings — on a card this bench has already initialized.
const FORCE_REINIT: bool = false;

/// Flip to `true` for one flash to run §6.3's compaction pass at the end of the cycle and time it.
///
/// A commit runs the pass itself once the journal reaches §6.3's 192-record trigger, which is the
/// cost a client's `FinishUpload` pays on the boot that crosses it. Waiting for a real crossing
/// costs 80-odd upload lifecycles of card time, and the pass's duration does not depend on how it
/// was reached — so this calls it directly and reports what it had to materialize alongside, which
/// is what makes the figure extrapolable to a full 256-head epoch instead of only true for this
/// store's shape.
///
/// It is **not** destructive: the pass writes the inactive checkpoint and advances the epoch, which
/// is an ordinary thing for the store to do and a later boot mounts normally.
const FORCE_COMPACT: bool = false;

/// The OperationId of the `index`-th object this bench commits. Deterministic, so a boot after a
/// reset can ask about the one the previous boot published.
fn operation_of(index: u64) -> OperationId {
    let mut bytes = [0xC1u8; 16];
    bytes[8..16].copy_from_slice(&index.to_be_bytes());
    OperationId::new(bytes)
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
    info!("obc2_store_bench: the OBC2 kernel transaction on real media ({=str})", env!("OBC_FW_GIT"));
    info!("obc2_store_bench: DESTRUCTIVE — this bench's /OBC2 store is rebuilt when it has to be");

    let card = match Card::with(|sd| sd.start()) {
        Ok(card) => card,
        Err(error) => {
            error!("obc2_store_bench: the card did not come up ({}) — nothing measured", error);
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

    let Some(geometry) = geometry_phase() else { park() };
    report_footprint();
    info!("STACK usable {=usize} B, {=usize} B used at boot", stackmeter::total(), stackmeter::used());

    let log: &'static Log = unsafe { init_static(core::ptr::addr_of_mut!(LOG), WriteLog::new(Card)) };
    let vmgr: &'static Vmgr = unsafe {
        init_static(
            core::ptr::addr_of_mut!(VMGR),
            VolumeManager::new_with_limits(SharedBlockDevice(log), NullTime, 9_000),
        )
    };
    let Ok(volume) = vmgr.open_raw_volume(VolumeIdx(0)) else {
        error!("obc2_store_bench: the geometry admitted this volume but the FAT layer would not mount it");
        park();
    };
    let Ok(root) = vmgr.open_root_dir(volume) else {
        error!("obc2_store_bench: no root directory");
        park();
    };

    run(vmgr, log, root, &geometry);
    info!("STACK high-water across the whole run: {=usize} B of {=usize} B", stackmeter::used(), stackmeter::total());
    park();
}

/// The whole run, in a plain function rather than in the async `main`.
///
/// Deliberate: an async fn's locals are permanent poll-frame slots, and this one places a
/// transaction whose type is tens of kilobytes. In an ordinary call the same locals are scoped to
/// the frame and come back at return — which is what makes the stack figure this bench prints a
/// measurement of the store's peak rather than of the executor's permanent reservation.
#[inline(never)]
fn run(vmgr: &'static Vmgr, log: &'static Log, root: RawDirectory, geometry: &VolumeGeometry) {
    let fat: Fat = Adapter::new(vmgr);
    // SAFETY: sole borrows. This binary is single-threaded and nothing else reads these statics.
    let stride: &'static mut Stride = unsafe { &mut (*core::ptr::addr_of_mut!(STRIDE)).0 };
    let slots: &'static mut SlotTable = unsafe { &mut *core::ptr::addr_of_mut!(SLOTS) };

    let started = Instant::now();
    let survey = fat::survey(&fat, root, None, stride, slots);
    report_survey("MOUNT", &survey, ms(started));

    // Initialization **deletes the seven fixed files**, so the conditions under which this bench is
    // allowed to run it are the narrow ones. §12 hands back three pre-birth verdicts and a
    // fail-closed one, and they are not equally safe to overwrite:
    //
    // - `Initialize` — `/OBC2` is absent or empty. Nothing to destroy.
    // - `RestartPreBirth` — an ungated prefix with no witness, which is exactly what §12 authorizes
    //   deleting. Safe, and the verdict itself is the authorization.
    // - `ResumeInitialization` — a valid `INIT.REC` naming a StoreId. If that StoreId is not this
    //   bench's, the card belongs to a real initialization that was cut, and §12 says to *resume* it
    //   under its own identity rather than restart under a new one. Wiping it would destroy the one
    //   fact that makes resuming possible.
    // - `RecoveryFailed` — evidence §12 says to preserve. Never wiped without an explicit flag.
    let refuses_wipe = match survey.outcome {
        Outcome::ResumeInitialization { store } if store != BENCH_STORE => {
            Some("a valid INIT.REC names another store — resuming it is §12's answer, not wiping it")
        }
        Outcome::RecoveryFailed(_) => Some("§12 mounted recovery-failed and says to preserve the evidence"),
        Outcome::Unsupported(_) => Some("§1.1 refused this volume, so nothing may be written to it"),
        _ => None,
    };
    let survey = if survey.is_mountable() && !FORCE_REINIT {
        survey
    } else if let Some(reason) = refuses_wipe.filter(|_| !FORCE_REINIT) {
        error!("MOUNT REFUSING to initialize: {=str} (set FORCE_REINIT to override)", reason);
        return;
    } else {
        match initialize_phase(&fat, root, stride, slots) {
            Some(survey) => survey,
            None => return,
        }
    };

    let free = geometry.volume_bytes() / 4;
    let started = Instant::now();
    let media = match fat::attach(Adapter::new(vmgr), root, &survey, stride, free) {
        Ok(media) => media,
        Err(error) => {
            error!("MOUNT attach refused ({})", defmt::Debug2Format(&error));
            return;
        }
    };
    // The transaction is placed field by field into `.bss`. The by-value `mount` measured
    // **206,080 B of transient stack** on this board for exactly this step — a 56 KiB projection
    // moved into a 73 KiB value that is then copied into the static — against a 252 KiB stack here
    // and a 51.6 KiB residual main stack in the shipping image. `mount_in_place` never materializes
    // the value at all, and the projection is decoded straight into its own field below.
    // SAFETY: the slot is written exactly once, before anything reads it.
    let store = unsafe {
        KernelTransaction::mount_in_place(
            &mut *core::ptr::addr_of_mut!(STORE),
            media,
            AcceptEverything,
            NoHooks,
            BENCH_STORE,
        )
    };
    let (media, index) = store.media_and_index_mut();
    // §13: the checkpoint is *streamed* into the bounded index, so this step stages the 16 KiB
    // stride and nothing else. It used to read the whole 65,536-byte file into `.bss` first.
    let epoch_base = match media.load_index(&survey, index) {
        Ok(base) => base,
        Err(error) => {
            error!("MOUNT the index would not load ({})", defmt::Debug2Format(&error));
            return;
        }
    };
    store.rebind(epoch_base);
    info!("MOUNT attach + index (streamed checkpoint + suffix replay): {=u64} us", us(started));
    if store.store_id() != BENCH_STORE {
        error!("MOUNT this store is not this bench's — REFUSING to write to it");
        return;
    }
    mark("after attach");

    // Whatever a previous boot published must still be there, byte for byte, before this boot
    // publishes anything of its own.
    let committed = store.retained_results() as u64;
    if committed > 0 {
        verify_recovery(store, committed - 1);
    } else {
        info!("RCVR  a fresh store: nothing to recover yet — reset the board and run again");
    }

    shard_phase(store, committed);

    if committed >= REINITIALIZE_ABOVE {
        info!("RUN   {=u64} committed objects — reinitialize (flip FORCE_REINIT) before the journal fills", committed);
        return;
    }
    lifecycle(store, log, geometry, committed);
}

// ── 1. volume geometry (§1.1) ───────────────────────────────────────────────────────────────────

/// Reads the MBR and the BPB straight off the card and decides §1.1's two preconditions, before the
/// FAT layer is mounted: §12 decides the unsupported-filesystem class "before `/OBC2` is looked for".
fn geometry_phase() -> Option<VolumeGeometry> {
    let mut sector = [Block::new()];
    if let Err(error) = Card.read(&mut sector, BlockIdx(0)) {
        error!("GEOM  LBA 0 unreadable ({})", error);
        return None;
    }
    let mbr = sector[0].contents;
    let partition = match geometry::partition(&mbr, 0) {
        Ok(partition) => partition,
        Err(reason) => {
            error!("GEOM  unsupported filesystem: no usable partition ({})", defmt::Debug2Format(&reason));
            return None;
        }
    };
    if let Err(error) = Card.read(&mut sector, BlockIdx(partition.start_lba)) {
        error!("GEOM  the volume boot record at LBA {=u32} is unreadable ({})", partition.start_lba, error);
        return None;
    }
    let geometry = match geometry::geometry(partition, &sector[0].contents) {
        Ok(geometry) => geometry,
        Err(reason) => {
            error!("GEOM  unsupported filesystem: {}", defmt::Debug2Format(&reason));
            return None;
        }
    };
    info!(
        "GEOM  {=str} cluster={=u32} B data region LBA {=u32} = byte {=u64}; FSInfo LBA {=u32}",
        if geometry.fat_type == FatType::Fat32 { "FAT32" } else { "FAT16" },
        geometry.cluster_bytes,
        geometry.data_start_lba,
        geometry.data_start_byte,
        geometry.fs_info_lba.unwrap_or(0)
    );
    match geometry.admit() {
        Ok(()) => {
            info!("GEOM  VERDICT: this card satisfies every §1.1 volume precondition");
            Some(geometry)
        }
        Err(reason) => {
            error!("GEOM  VERDICT: UNSUPPORTED FILESYSTEM ({}) — nothing is written", defmt::Debug2Format(&reason));
            None
        }
    }
}

// ── 2. mount (§12) ──────────────────────────────────────────────────────────────────────────────

fn report_survey(tag: &str, survey: &Survey, elapsed_ms: u64) {
    info!(
        "{=str} §12 class {=u8} ({}) in {=u64} ms — {=usize} /OBC2 entries, checkpoints [{=bool},{=bool}], {=usize} valid journal slots, witness {=bool}",
        tag,
        survey.class as u8,
        defmt::Debug2Format(&survey.outcome),
        elapsed_ms,
        survey.entries,
        survey.checkpoints_valid[0],
        survey.checkpoints_valid[1],
        survey.valid_slots,
        survey.witness.is_some()
    );
}

// ── 3. initialization (§12), with lazy shards ───────────────────────────────────────────────────

/// Wipes this bench's store and rebuilds it, timing every stage.
fn initialize_phase(fat: &Fat, root: RawDirectory, stride: &mut Stride, slots: &mut SlotTable) -> Option<Survey> {
    let vmgr = fat.volume_manager();
    // §12: "store reset … is defined as file deletion, never directory deletion". The skeleton — and
    // whatever shard directories an earlier bench left behind — survives and is reused in place.
    if let Ok(obc2) = vmgr.open_dir(root, fat::ROOT_DIRECTORY) {
        let started = Instant::now();
        let mut deleted = 0u32;
        for file in CREATION_ORDER {
            if vmgr.delete_file_in_dir(obc2, file.name).is_ok() {
                deleted += 1;
            }
        }
        let _ = vmgr.close_dir(obc2);
        info!("INIT  wiped {=u32} OBC2 file(s) in {=u64} ms", deleted, ms(started));
    }

    let started = Instant::now();
    let report = match fat::initialize(fat, root, BENCH_STORE, stride) {
        Ok(report) => report,
        Err(error) => {
            error!("INIT  initialization failed ({})", defmt::Debug2Format(&error));
            return None;
        }
    };
    let elapsed = us(started);
    info!(
        "INIT  §12 initialization: {=u64} directories, {=u64} B zero-filled (§13.1 says {=usize}) — {=u64} ms total",
        report.directories as u64,
        report.zero_filled,
        INITIALIZATION_ZERO_FILL,
        elapsed / 1_000
    );
    info!(
        "INIT  LAZY SHARDS: 0 shard directories created at initialization (the eager tree cost 73.5 s of a 75 s first boot)"
    );
    info!("INIT  zero-fill rate {=u64} kB/s over the seven fixed files", rate(report.zero_filled, elapsed));

    let started = Instant::now();
    let survey = fat::survey(fat, root, None, stride, slots);
    report_survey("MOUNT", &survey, ms(started));
    if !survey.is_mountable() {
        error!("INIT  the store did not mount after initialization");
        return None;
    }
    Some(survey)
}

/// Times §12's lazy-shard obligation on its own, because the claim pays it and the claim's total
/// hides it.
///
/// §12 and §13 say a shard's `make_dir` "costs about 140 ms the first time a shard is used and
/// nothing afterwards". The first half was measured in #1354; the second half is an assumption, and
/// this is where it is tested: the same shard is asked for twice in a row, and a role directory here
/// already holds all 256 shards, which is the state a mature card is in.
fn shard_phase(store: &mut Store, index: u64) {
    // A generation this store will never write. `ensure_shards` creates directories and nothing
    // else, so an unused id costs a `make_dir` on a present directory and leaves no file behind.
    let scratch = GenerationId::new(0xFFFF_0000 | (index & 0xFF));
    let media = store.media_mut();
    let started = Instant::now();
    let first = media.ensure_shards(scratch);
    let first_us = us(started);
    let started = Instant::now();
    let second = media.ensure_shards(scratch);
    let second_us = us(started);
    if first.is_err() || second.is_err() {
        error!("SHARD ensure_shards refused — the lazy-shard obligation is not satisfiable here");
        return;
    }
    info!(
        "SHARD ensure_shards (GEN/xx + WORK/xx, both already present): first {=u64} us, again {=u64} us",
        first_us, second_us
    );
    info!("SHARD §12 says a present shard costs 'nothing afterwards' — every claim pays the figure above");

    // The rest of what a claim pays, isolated. A claim is `ensure_shards`, then `open_generation`
    // (which creates the 32,768-byte `WORK` file at its full length, §13.1's `BeginWork` zero-fill,
    // and opens the payload), then §7's rewind, then one journal record. Only the last of those was
    // measured in #1354, and the claim's total is four times it.
    let started = Instant::now();
    if media.open_generation(scratch).is_err() {
        error!("SHARD open_generation refused");
        return;
    }
    let open_us = us(started);
    let started = Instant::now();
    let rewound = media.truncate_payload().and_then(|()| media.sync_payload());
    let rewind_us = us(started);
    let started = Instant::now();
    let appended = media.write_payload(0, &[0x5A; 1_024]).and_then(|()| media.sync_payload());
    let append_us = us(started);
    let started = Instant::now();
    let collected = media.collect_generation(scratch);
    let collect_us = us(started);
    if rewound.is_err() || appended.is_err() || collected.is_err() {
        error!("SHARD a generation primitive refused");
        return;
    }
    info!(
        "SHARD open_generation (create {=usize} B WORK + open payload) {=u64} us; §7 rewind {=u64} us; 1 KiB append+sync {=u64} us; collect {=u64} us",
        WORK_FILE_LEN, open_us, rewind_us, append_us, collect_us
    );
}

// ── 4. one upload lifecycle through the kernel ──────────────────────────────────────────────────

/// Claim → append → seal → validate → publish → query, each step timed.
fn lifecycle(store: &mut Store, log: &'static Log, geometry: &VolumeGeometry, index: u64) {
    let operation = operation_of(index);
    // SAFETY: sole borrows of the payload and scratch slots.
    let payload = unsafe { &mut (*core::ptr::addr_of_mut!(PAYLOAD)).0 };
    let scratch = unsafe { &mut (*core::ptr::addr_of_mut!(SCRATCH)).0 };
    for (at, byte) in payload.iter_mut().enumerate() {
        *byte = (at as u8) ^ (index as u8);
    }
    let crc = obc_crc::crc32(&payload[..]);

    let claim = ClaimIntent {
        operation_id: operation,
        principal: BENCH_PRINCIPAL,
        opcode: Opcode::StartUpload,
        digest: BENCH_DIGEST,
        kind: ObjectKind::Route,
        target: Target::Create,
        declared_length: PAYLOAD_LEN as u64,
        expected_crc: crc,
        target_operation_id: None,
    };

    // The claim: §11's admission, the lazy shard `make_dir`s, and the first durable record.
    let started = Instant::now();
    let outcome = store.execute(Command::Claim(claim), scratch);
    let claim_us = us(started);
    let logical = match outcome {
        EngineOutcome::Claim(ClaimOutcome::Claimed { logical_object_id, .. }) => logical_object_id,
        other => {
            error!("LIFE  the claim was refused ({})", defmt::Debug2Format(&other));
            return;
        }
    };
    info!(
        "LIFE  claim (admission + lazy shards + one journal record): {=u64} us → logical id {=u64}",
        claim_us,
        logical.get()
    );
    mark("after claim");

    // The appends: payload bytes only. The restart-only profile acknowledges no offset, so nothing
    // here is synchronized and nothing here is durable.
    let started = Instant::now();
    let mut appends = 0u32;
    for (at, chunk) in payload.chunks(APPEND_CHUNK).enumerate() {
        let offset = (at * APPEND_CHUNK) as u64;
        match store.execute(Command::Append { operation_id: operation, offset, bytes: chunk }, scratch) {
            EngineOutcome::Appended => appends += 1,
            other => {
                error!("LIFE  the append at {=u64} was refused ({})", offset, defmt::Debug2Format(&other));
                return;
            }
        }
    }
    let append_us = us(started);
    info!(
        "LIFE  {=u32} appends of {=usize} B ({=usize} B total): {=u64} us, {=u64} kB/s",
        appends,
        APPEND_CHUNK,
        PAYLOAD_LEN,
        append_us,
        rate(PAYLOAD_LEN as u64, append_us)
    );

    // The seal: the payload sync, the length and CRC proof, and the sealed `WORK` slot.
    let started = Instant::now();
    match store.execute(
        Command::Seal { operation_id: operation, declared_length: PAYLOAD_LEN as u64, expected_crc: crc },
        scratch,
    ) {
        EngineOutcome::Sealed => {}
        other => {
            error!("LIFE  the seal was refused ({})", defmt::Debug2Format(&other));
            return;
        }
    }
    info!("LIFE  seal (payload sync + {=usize} B WORK slot, gated): {=u64} us", WORK_FILE_LEN, us(started));

    let started = Instant::now();
    match store.execute(Command::Validate { operation_id: operation }, scratch) {
        EngineOutcome::Validated => {}
        other => {
            error!("LIFE  validation refused ({})", defmt::Debug2Format(&other));
            return;
        }
    }
    info!("LIFE  validate (the typed-validator seam; no domain rules yet): {=u64} us", us(started));

    // The publication: one journal record carrying the head, the revision and the retained result
    // together. Its sectors are recorded so §13.1's clean flush is checked through the whole stack.
    let entry_lba = directory_entry_lba(store, "COMMIT");

    log.arm();
    let started = Instant::now();
    let published = store.execute(Command::Publish { operation_id: operation }, scratch);
    let publish_us = us(started);
    log.disarm();
    match published {
        EngineOutcome::Published(_) => {}
        other => {
            error!("LIFE  publication refused ({})", defmt::Debug2Format(&other));
            return;
        }
    }
    info!("LIFE  publish (one catalog commit: head + revision + retained result): {=u64} us", publish_us);
    mark("after publish");
    report_commit_sectors(log, geometry, entry_lba);

    match store.head(ObjectKind::Route, logical) {
        Some((revision, length, stored)) if length == PAYLOAD_LEN as u64 && stored == crc => {
            info!("LIFE  head revision {=u64}, {=u64} B, crc 0x{=u32:08x}", revision.get(), length, stored)
        }
        other => {
            error!("LIFE  the head is not what was published ({})", defmt::Debug2Format(&other));
            return;
        }
    }

    // §8.1: the query answers from the durable ledger.
    let started = Instant::now();
    let report =
        store.execute(Command::QueryOperation { operation_id: operation, principal: BENCH_PRINCIPAL }, scratch);
    match report {
        EngineOutcome::OperationReport(OperationReport::Committed(_)) => {
            info!("LIFE  QueryOperation → Committed in {=u64} us — the lifecycle is durable", us(started))
        }
        other => error!("LIFE  QueryOperation answered {} — the result is not retained", defmt::Debug2Format(&other)),
    }
    if FORCE_COMPACT {
        compaction_phase(store);
    }
    info!("LIFE  reset the board (`probe-rs reset`) and run again: object {=u64} must come back intact", index);
}

/// §6.3's compaction pass, timed on the real card, with the shape it ran over.
///
/// The pass is a single forward pass over 127 sectors of the inactive checkpoint, and for each
/// occupied entry it sources the two card-resident head fields and the 208-byte result bodies from
/// §6.3's newest source. That source is a **whole 16,384-byte journal stride** for anything a record
/// replayed since the active checkpoint carries, and a bounded checkpoint read for everything else —
/// so the counts below are what turn one duration into a rate.
fn compaction_phase(store: &mut Store) {
    let index = store.index();
    let (heads, results, journal_heads, journal_results) = (
        index.heads.len(),
        index.results.len(),
        index.heads.iter().filter(|head| head.journal_slot != obc_storage::obc2::index::NO_JOURNAL_SLOT).count(),
        index.results.iter().filter(|row| row.journal_slot != obc_storage::obc2::index::NO_JOURNAL_SLOT).count(),
    );
    let epoch = index.epoch;
    info!(
        "CMPCT §6.3 pass over epoch {=u64}: {=usize} heads ({=usize} journal-carried), {=usize} results ({=usize} journal-carried)",
        epoch, heads, journal_heads, results, journal_results
    );
    let started = Instant::now();
    let outcome = store.compact();
    let elapsed = us(started);
    match outcome {
        Ok(()) => info!(
            "CMPCT §6.3 steps 2-4 (invalidate + 65,024 B streamed body + gate): {=u64} us — epoch {=u64} now, {=usize} card-sourced entries",
            elapsed,
            store.index().epoch,
            journal_heads + journal_results
        ),
        Err(error) => error!("CMPCT the pass refused ({})", defmt::Debug2Format(&error)),
    }
}

/// What one armed window wrote, named by the structure it landed in.
fn report_commit_sectors(log: &'static Log, geometry: &VolumeGeometry, entry_lba: Option<u32>) {
    let mut total = 0u32;
    let mut metadata = 0u32;
    log.with_spans(|recorded| {
        for span in recorded {
            for offset in 0..span.blocks {
                let lba = span.start + offset;
                let region = geometry.region(lba);
                let is_entry = Some(lba) == entry_lba;
                total += 1;
                if !matches!((region, is_entry), (Region::Data, false)) {
                    metadata += 1;
                    let name = match (region, is_entry) {
                        (_, true) => "DIRECTORY ENTRY",
                        (Region::FsInfo, _) => "FSINFO",
                        (Region::Reserved, _) => "BOOT/RESERVED",
                        (Region::Fat(_), _) => "FAT",
                        (Region::RootDir, _) => "ROOT DIR",
                        _ => "other",
                    };
                    warn!("FLUSH publish wrote LBA {=u32} — {=str}", lba, name);
                }
            }
        }
    });
    if log.dropped() > 0 {
        error!(
            "FLUSH INDETERMINATE — {=u32} span(s) did not fit the log, so this window proves nothing",
            log.dropped()
        );
        return;
    }
    if entry_lba.is_none() {
        error!(
            "FLUSH INDETERMINATE — COMMIT.JNL's directory-entry sector is unknown, so a rewrite would look like data"
        );
        return;
    }
    if metadata == 0 {
        info!(
            "FLUSH §13.1 clean flush HOLDS through the kernel: a publish wrote {=u32} sector(s), 0 of them metadata",
            total
        );
    } else {
        error!(
            "FLUSH §13.1 clean flush VIOLATED through the kernel: {=u32} of {=u32} sector(s) were single-copy metadata",
            metadata, total
        );
    }
}

/// The LBA of the sector holding `base`'s 32-byte directory entry, from the FAT layer's own view.
///
/// `None` is "I do not know", and the caller must not turn that into a verdict: on FAT32 a directory
/// entry lives in the data region, so an unknown entry sector would silently classify an entry
/// rewrite as an ordinary record write.
fn directory_entry_lba(store: &mut Store, base: &str) -> Option<u32> {
    let media = store.media_mut();
    let mut found = None;
    media
        .adapter()
        .volume_manager()
        .iterate_dir(media.obc2(), |entry| {
            if found.is_none() && entry.name.base_name() == base.as_bytes() {
                found = Some(entry.entry_block.0);
            }
        })
        .ok()?;
    found
}

// ── 5. resident cost ────────────────────────────────────────────────────────────────────────────

/// The `size_of` table, decomposed against §13's budget.
///
/// §13: "RAM holds a bounded index, not the projection … The measured figure at these capacities is
/// **19,848 bytes**. DOS2 sizes its arena from that figure."
///
/// Still three separate figures rather than one ratio, because they are three different questions:
///
/// 1. **The resident catalog.** §13's 19,848 B *is* the budget, and this is now the same design —
///    a bounded index with envelopes, resolution generations and result bodies re-read from card.
///    It is the one line that compares like with like.
/// 2. **The transaction around it.** The index plus the 16 KiB seal stride and the resident tables.
///    §13 budgets none of this explicitly; it is the kernel's own working set, and after the swap it
///    is dominated by the stride rather than by catalog state.
/// 3. **Mount staging.** A slot stride and the observation table. The 65,536-byte checkpoint image
///    is gone: §13's mount streams, so the largest thing a mount touches is the stride it already
///    needed for the journal scan.
const RAM_INDEX_BUDGET: usize = 19_848;

fn report_footprint() {
    let resident = index::resident_bytes();
    let store = core::mem::size_of::<Store>();
    let media = core::mem::size_of::<Media>();
    let slots = core::mem::size_of::<SlotTable>();
    let staging = SLOT_STRIDE + slots;
    // Two of these are addends and two are components, and the total is the sum of the addends
    // alone. `KernelTransaction` *contains* the media and the index by value, so adding either to
    // the total would count it twice — which is exactly the arithmetic a reader checks first.
    info!(
        "RAM   [component] resident catalog: RamIndex + lease table {=usize} B vs §13's {=usize} B budget — {=u32}/100x",
        resident,
        RAM_INDEX_BUDGET,
        (resident * 100 / RAM_INDEX_BUDGET) as u32
    );
    info!("RAM   [component] FatMedia inside it (handles + one staging reference): {=usize} B", media);
    info!(
        "RAM   [addend 1] transaction: KernelTransaction {=usize} B — the two components above plus a {=usize} B seal stride and the resident tables; §13 budgets no figure for this",
        store, SLOT_STRIDE
    );
    info!(
        "RAM   [addend 2] mount staging: {=usize} B = {=usize} B stride + {=usize} B slot table; no checkpoint image — §13's mount streams",
        staging, SLOT_STRIDE, slots
    );
    info!("RAM   TOTAL placed in .bss for one mounted store (addends only): {=usize} B", store + staging);
}

// ── 6. reboot recovery ──────────────────────────────────────────────────────────────────────────

/// Everything a previous boot published must come back from the card alone.
fn verify_recovery(store: &mut Store, index: u64) {
    let operation = operation_of(index);
    // SAFETY: sole borrows.
    let scratch = unsafe { &mut (*core::ptr::addr_of_mut!(SCRATCH)).0 };
    let readback = unsafe { &mut (*core::ptr::addr_of_mut!(READBACK)).0 };
    let expected = unsafe { &mut (*core::ptr::addr_of_mut!(PAYLOAD)).0 };
    for (at, byte) in expected.iter_mut().enumerate() {
        *byte = (at as u8) ^ (index as u8);
    }

    let mut ok = true;
    match store.execute(Command::QueryOperation { operation_id: operation, principal: BENCH_PRINCIPAL }, scratch) {
        EngineOutcome::OperationReport(OperationReport::Committed(_)) => {
            info!("RCVR  QueryOperation for object {=u64} → Committed after the reset", index)
        }
        other => {
            error!(
                "RCVR  object {=u64} answered {} — the retained result did not survive",
                index,
                defmt::Debug2Format(&other)
            );
            ok = false;
        }
    }

    // §5.3's logical identity is assigned by the repository, and object `index` is the `index`-th
    // route this store published — the identities start at one and never repeat.
    let logical = LogicalObjectId::new(index + 1);
    match store.head(ObjectKind::Route, logical) {
        Some((revision, length, crc)) => {
            let read = store.read_head(ObjectKind::Route, logical, &mut readback[..PAYLOAD_LEN]);
            let matched = read == Some(PAYLOAD_LEN) && readback[..PAYLOAD_LEN] == expected[..PAYLOAD_LEN];
            let recomputed = obc_crc::crc32(&readback[..PAYLOAD_LEN]);
            if matched && recomputed == crc && length == PAYLOAD_LEN as u64 {
                info!(
                    "RCVR  head {=u64} revision {=u64}: {=u64} B read back byte-for-byte, crc 0x{=u32:08x} confirmed",
                    logical.get(),
                    revision.get(),
                    length,
                    crc
                );
            } else {
                error!(
                    "RCVR  head {=u64} did not read back intact (read {=usize} B)",
                    logical.get(),
                    read.unwrap_or(0)
                );
                ok = false;
            }
        }
        None => {
            error!("RCVR  no head at logical id {=u64} after the reset", logical.get());
            ok = false;
        }
    }
    if ok {
        info!("RCVR  OK — the store remounted from the card alone with claim, head, payload and result intact");
        info!("RCVR  soft reset only: `probe-rs reset` never removed the card's supply, so this says");
        info!("RCVR  nothing about §1.1 tearing.");
    }
}

/// One stack high-water reading, tagged.
fn mark(tag: &str) {
    info!("STACK {=str}: {=usize} B of {=usize} B", tag, stackmeter::used(), stackmeter::total());
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

// ── helpers ─────────────────────────────────────────────────────────────────────────────────────

fn us(since: Instant) -> u64 {
    Instant::now().duration_since(since).as_micros()
}

fn ms(since: Instant) -> u64 {
    Instant::now().duration_since(since).as_millis()
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
    info!("obc2_store_bench: done — parked");
    loop {
        cortex_m::asm::wfi();
    }
}
