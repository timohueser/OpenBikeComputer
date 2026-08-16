//! **OBC2 media bench** — the §13.1 adapter and the §1.1 fault model, measured on the real card.
//!
//!     cargo run --release --bin obc2_media_bench
//!
//! `OBC2_Storage_Format.md` §13 lists the measurements DOS2 owes before the format's durability
//! argument can be believed on this hardware, and §1.1 ends with the blunt version: "DOS2 must
//! validate this assumption on the shipped media; a violation is a format-version matter, not a
//! runtime workaround." Everything the kernel does — gated slots, alternating checkpoints, a
//! journal whose torn tail is provably ignorable — rests on facts about a card that no host test
//! can establish. This binary establishes them.
//!
//! It brings up only the sEMMC card: no display, no app, no BLE, no sensors. It is **destructive**
//! — it deletes and recreates `/OBC2` — and it is a bench, never shipped.
//!
//! ## What it measures, in the order it runs
//!
//! 1. **Volume geometry (§1.1).** The two preconditions, computed from the MBR entry and the BPB of
//!    *this* card, with every input printed so the verdict can be checked by hand. A card that
//!    fails either is refused and nothing is written, exactly as a mount would.
//! 2. **Skeleton initialization (§12).** The `/OBC2` tree, 512 shard directories, and 4,636,672
//!    bytes of zero-filled metadata files, each stage timed. §13 predicts a multi-second first boot.
//! 3. **Clean flush (§13.1) — the important one.** A body write, a sync, a gate write and a sync on
//!    a fixed-length gated file, with every sector the FAT layer wrote recorded and classified. If
//!    a sync of an unchanged-length file rewrites FSInfo or the directory entry, OBC2's commit path
//!    risks a single-copy sector three times per commit and the C2 assumption is false. The same
//!    sequence is then repeated through the fork's own `flush_file` to show what that would cost.
//! 4. **Commit throughput.** The gated-record commit cycle — body write, sync, gate write, sync —
//!    timed per commit over 64 journal slots, beside a 16,384-byte full-stride write for the
//!    payload rate. These are the numbers DOS2's card-resident-catalog decision needs.
//! 5. **Recovery across resets.** Each boot scans all 256 journal slots, reports the contiguous
//!    valid prefix §6.3's replay would accept, checks every record against its physical slot, and
//!    appends exactly one more. Reset the board (`probe-rs reset`) and run it again: the prefix must
//!    grow by exactly one, with no stray valid slot beyond it.
//!
//! ## What it does NOT prove — read this before quoting the results
//!
//! A `probe-rs reset` is a **CPU reset, not a power cut**. The card keeps its supply, finishes any
//! program cycle it had started, and never sees the mid-page interruption §1.1's fault model is
//! about. So the recovery loop validates *the recovery decision over durable records* and nothing
//! at all about tearing: the program page `P` of the shipped media and the fault-isolation
//! assumption remain **unvalidated** and need a rig that cuts the card's own supply mid-write.
//!
//! ## Bring-up
//!
//! `semmc.rs` is pulled in by path and has no `crate::` dependencies, so this binary owns its own
//! host instance and never touches the display mux. The card is brought up once with
//! [`Semmc::start`]; the M33 must be at CK128 (the sEMMC clock divisors and the firmware's wait
//! slices are stated against it) and `VPR00` must be bound, which is done below.
#![no_std]
#![no_main]

use core::mem::MaybeUninit;

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_nrf::interrupt;
use embassy_nrf::interrupt::{InterruptExt, Priority};
use embassy_time::Instant;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, RawDirectory, RawFile, TimeSource, Timestamp, VolumeIdx, VolumeManager,
};
use obc_storage::fat_extents::SharedBlockDevice;
use obc_storage::obc2::adapter::{Adapter, AdapterError};
use obc_storage::obc2::blocklog::WriteLog;
use obc_storage::obc2::geometry::{self, FatType, Region, VolumeGeometry};
use obc_storage::obc2::init::InitRecord;
use obc_storage::obc2::journal::{Change, JournalBody, Mutation, RecordKind};
use obc_storage::obc2::limits::{
    CHECKPOINT_FILE_LEN, INITIALIZATION_ZERO_FILL, JOURNAL_BODY_LEN, JOURNAL_FILE_LEN, JOURNAL_GATE_OFFSET,
    JOURNAL_SLOTS, RIDE_FILE_LEN, SLOT_FILE_LEN, SLOT_STRIDE,
};
use obc_storage::obc2::{GenerationId, OperationId, StoreId};

// The critical-section impl comes from linking nrf-mpsl (the default `ble` feature set); MPSL is
// never initialised here, and its impl works from reset — the same arrangement `display_test` uses.
use nrf_mpsl as _;
use {defmt_rtt as _, panic_probe as _};

#[allow(dead_code)]
#[path = "../semmc.rs"]
mod semmc;

use semmc::{Semmc, SemmcError, BLOCK_BYTES};

/// sEMMC completion event: VEVIF event 20 is routed to `VPR00_IRQn` by the FLPR firmware. Without
/// this the driver still works — `wait_completion` polls — but it warns on every transfer.
#[interrupt]
unsafe fn VPR00() {
    semmc::on_vpr00_irq();
}

// ── the block device ────────────────────────────────────────────────────────────────────────────

/// The one sEMMC host. This binary is single-threaded and never re-enters the driver, which is what
/// makes the `&mut` below sound; the app reaches the same instance through `flpr_mux`'s scheduler
/// because it also has a display to hand the coprocessor to.
static mut SEMMC: Semmc = Semmc::new();

/// A 4-byte-aligned byte buffer. The sEMMC firmware's DMA requires 32-bit alignment and
/// `embedded_sdmmc::Block` cannot promise it (`#[repr(transparent)]` over `[u8; 512]` forbids
/// `#[repr(align(4))]`), so every buffer this bench hands the driver carries the alignment itself.
#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);

/// The misaligned-span bounce, four blocks deep — the same shape `sd.rs` uses.
static mut BOUNCE: Aligned<2_048> = Aligned([0; 2_048]);
/// One 16,384-byte slot, staged out of line of the task frame.
static mut SLOT: Aligned<SLOT_STRIDE> = Aligned([0; SLOT_STRIDE]);
/// The zero-fill granule for full-length initialization: one 16 KiB cluster per write.
static mut ZEROS: Aligned<16_384> = Aligned([0; 16_384]);

/// The `BlockDevice` over the sEMMC host: zero-sized, because all the state is in [`SEMMC`].
#[derive(Clone, Copy)]
struct Card;

impl Card {
    /// SAFETY: the caller must not be inside another `with` — this binary never is.
    fn with<R>(f: impl FnOnce(&mut Semmc) -> R) -> R {
        // SAFETY: single-threaded, non-re-entrant, and no interrupt handler touches the host state
        // (`on_vpr00_irq` only stamps an atomic).
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
                for (block, src) in chunk.iter_mut().zip(bounce.0[..len].chunks_exact(BLOCK_BYTES)) {
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
                for (block, dst) in chunk.iter().zip(bounce.0[..len].chunks_exact_mut(BLOCK_BYTES)) {
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

/// The zero timestamp the board's storage uses. Its only effect here is the mtime the fork stamps
/// into a directory entry on flush — which is exactly the byte that makes an entry rewrite visible.
struct NullTime;

impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// The instrumented card: 192 recorded spans is more than any single measurement below produces.
type Log = WriteLog<Card, 192>;
/// §13's handle budget: four directory handles reach a `GEN`/`WORK` leaf, sixteen file handles.
type Vmgr = VolumeManager<SharedBlockDevice<'static, Log>, NullTime, 4, 16, 1>;
type Fat = Adapter<'static, SharedBlockDevice<'static, Log>, NullTime, 4, 16, 1>;

static mut LOG: MaybeUninit<Log> = MaybeUninit::uninit();
static mut VMGR: MaybeUninit<Vmgr> = MaybeUninit::uninit();

/// Places `value` in a `.bss` slot and hands back the `'static` reference — the warm-reset-safe
/// pattern the app uses, so nothing large lands on the executor's task frame.
///
/// SAFETY: called exactly once per slot, before anything reads it.
unsafe fn init_static<T>(slot: *mut MaybeUninit<T>, value: T) -> &'static mut T {
    let slot = &mut *slot;
    slot.write(value);
    slot.assume_init_mut()
}

// ── the store the bench writes ──────────────────────────────────────────────────────────────────

/// The bench's StoreId. A real initialization generates 128 CSPRNG bits (§12); a fixed value here
/// makes a record from an earlier run recognisable as this bench's rather than a live store's.
const BENCH_STORE: StoreId =
    StoreId::new([0xB2, 0x0C, 0xB2, 0x0C, 0xB2, 0x0C, 0xB2, 0x0C, 0xB2, 0x0C, 0xB2, 0x0C, 0xB2, 0x0C, 0xB2, 0x0C]);

/// How many commits the throughput pass times, and therefore how many records a fresh card ends
/// with. Each later boot adds one.
const COMMITS: u16 = 64;

/// Past this many valid slots the bench wipes and starts over rather than filling the journal.
const RESTART_ABOVE: usize = 240;

/// Flip to `true` for one flash to force the destructive first-cycle path — the wipe, the §12
/// skeleton and the initialization timings — on a card this bench has already initialized.
///
/// The recovery loop is the default because it is the one that wants many consecutive runs, and it
/// must not lose the records a reset is supposed to preserve.
const FORCE_REINIT: bool = false;

/// The fixed files of §3, in §12's creation order, with the lengths §13.1 requires them to reach.
const FIXED_FILES: [(&str, u32); 6] = [
    ("COMMIT.JNL", JOURNAL_FILE_LEN as u32),
    ("ARM0.HND", SLOT_FILE_LEN as u32),
    ("ARM1.HND", SLOT_FILE_LEN as u32),
    ("RIDE.ACT", RIDE_FILE_LEN as u32),
    ("CAT0.CHK", CHECKPOINT_FILE_LEN as u32),
    ("CAT1.CHK", CHECKPOINT_FILE_LEN as u32),
];

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let _p = {
        let mut config = embassy_nrf::config::Config::default();
        // Not optional: the sEMMC clock divisors and the firmware's wait slices are all stated
        // against a 128 MHz core.
        config.clock_speed = embassy_nrf::config::ClockSpeed::CK128;
        embassy_nrf::init(config)
    };
    // SAFETY: arming the vector before the soft peripheral boots is what `main.rs` does too.
    unsafe {
        interrupt::VPR00.set_priority(Priority::P1);
        interrupt::VPR00.enable();
    }
    info!("obc2_media_bench: OBC2 §13.1 adapter + §1.1 fault model on real media ({=str})", env!("OBC_FW_GIT"));
    info!("obc2_media_bench: DESTRUCTIVE — /OBC2 on this card is deleted and rebuilt");

    let card = match Card::with(|sd| sd.start()) {
        Ok(card) => card,
        Err(error) => {
            error!("obc2_media_bench: the card did not come up ({}) — nothing measured", error);
            park();
        }
    };
    let capacity_mib = (card.blocks as u64) * 512 / (1024 * 1024);
    info!(
        "CARD  rca=0x{=u16:04x} blocks={=u32} ({=u64} MiB) high_speed={=bool} read_clk={=u32} Hz",
        card.rca, card.blocks, capacity_mib, card.high_speed, card.read_clk_hz
    );

    let Some(geometry) = geometry_phase() else { park() };

    let log: &'static Log = unsafe { init_static(core::ptr::addr_of_mut!(LOG), WriteLog::new(Card)) };
    let vmgr: &'static Vmgr = unsafe {
        init_static(
            core::ptr::addr_of_mut!(VMGR),
            VolumeManager::new_with_limits(SharedBlockDevice(log), NullTime, 5_000),
        )
    };
    let fat: Fat = Adapter::new(vmgr);

    let Ok(volume) = vmgr.open_raw_volume(VolumeIdx(0)) else {
        error!("obc2_media_bench: the geometry admitted this volume but the FAT layer would not mount it");
        park();
    };
    let Ok(root) = vmgr.open_root_dir(volume) else {
        error!("obc2_media_bench: no root directory");
        park();
    };

    // A fresh card, a card this bench already initialised, or one whose journal is nearly full.
    //
    // The `/OBC2` handle is opened at most once and closed before initialization reopens it: the
    // §13 budget is four directory handles, and `make_dir_in_dir` needs a free slot even though it
    // keeps none, so root + two `/OBC2` handles + a role directory is exactly one too many.
    let existing = vmgr.open_dir(root, "OBC2").ok();
    let valid = match existing {
        Some(obc2) => fat
            .open_fixed(obc2, "COMMIT.JNL", JOURNAL_FILE_LEN as u32)
            .map(|file| {
                let count = scan_journal(&fat, file);
                let _ = vmgr.close_file(file);
                count
            })
            .unwrap_or(0),
        None => 0,
    };

    let obc2 = match existing {
        Some(obc2) if !FORCE_REINIT && valid > 0 && valid <= RESTART_ABOVE => {
            info!("RUN   recovery cycle: {=usize} durable records were found on this card", valid);
            append_and_verify(&fat, obc2, valid);
            park();
        }
        Some(obc2) => {
            if valid > RESTART_ABOVE {
                info!("INIT  the journal holds {=usize} valid slots — wiping and starting over", valid);
            }
            wipe(&fat, obc2);
            vmgr.close_dir(obc2).expect("close");
            initialize(&fat, root)
        }
        None => initialize(&fat, root),
    };

    clean_flush_phase(&fat, log, obc2, &geometry);
    throughput_phase(&fat, obc2, &geometry);
    info!("RUN   first cycle complete — `probe-rs reset` and run again to exercise recovery");
    park();
}

// ── 1. volume geometry (§1.1) ───────────────────────────────────────────────────────────────────

/// Reads the MBR and the BPB straight off the card and decides §1.1's two preconditions.
///
/// Deliberately before the FAT layer is mounted: §12 says the unsupported-filesystem class "is
/// decided before `/OBC2` is looked for", and a card that fails it is never written to.
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
        "GEOM  partition type=0x{=u8:02x} start_lba={=u32} sectors={=u32} ({=u64} MiB)",
        geometry.partition.kind,
        geometry.partition.start_lba,
        geometry.partition.sectors,
        geometry.volume_bytes() / (1024 * 1024)
    );
    info!(
        "GEOM  {=str} bytes/sector={=u16} sectors/cluster={=u8} cluster={=u32} B reserved={=u16} fats={=u8} fat_size={=u32} root_dir={=u32}",
        if geometry.fat_type == FatType::Fat32 { "FAT32" } else { "FAT16" },
        geometry.bytes_per_sector,
        geometry.sectors_per_cluster,
        geometry.cluster_bytes,
        geometry.reserved_sectors,
        geometry.fat_count,
        geometry.fat_size_sectors,
        geometry.root_dir_sectors
    );
    info!(
        "GEOM  data region: lba={=u32} byte={=u64} clusters={=u32} fsinfo_lba={=u32}",
        geometry.data_start_lba,
        geometry.data_start_byte,
        geometry.cluster_count,
        geometry.fs_info_lba.unwrap_or(0)
    );
    info!(
        "GEOM  §1.1 precondition 1 (cluster is a whole 16,384 B program page): {=bool} — {=u32} B / 16384 = {=u32} rem {=u32}",
        geometry.cluster_is_whole_pages(),
        geometry.cluster_bytes,
        geometry.cluster_bytes / 16_384,
        geometry.cluster_bytes % 16_384
    );
    info!(
        "GEOM  §1.1 precondition 2 (data region 16,384-aligned from physical LBA 0): {=bool} — {=u64} rem {=u64}",
        geometry.data_region_is_page_aligned(),
        geometry.data_start_byte,
        geometry.data_start_byte % 16_384
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

// ── 2. skeleton initialization (§12) ────────────────────────────────────────────────────────────

/// Deletes every OBC2-owned file under `/OBC2` (§12: "store reset … is defined as file deletion,
/// never directory deletion"). The directory skeleton survives and is reused in place.
fn wipe(fat: &Fat, obc2: RawDirectory) {
    let started = Instant::now();
    let vmgr = fat.volume_manager();
    let mut deleted = 0;
    for (name, _) in FIXED_FILES.iter().chain(core::iter::once(&("INIT.REC", 0))) {
        if vmgr.delete_file_in_dir(obc2, *name).is_ok() {
            deleted += 1;
        }
    }
    info!("INIT  wiped {=u32} OBC2 file(s) in {=u64} ms — the directory skeleton is reused", deleted, ms(started));
}

/// Creates the §12 tree in the order §12 fixes and times every stage.
fn initialize(fat: &Fat, root: RawDirectory) -> RawDirectory {
    let vmgr = fat.volume_manager();
    let total = Instant::now();

    fat.make_dir(root, "OBC2").expect("OBC2");
    let obc2 = vmgr.open_dir(root, "OBC2").expect("OBC2 opens");

    // INIT.REC first: §12 writes the incomplete-initialization witness "before it creates anything
    // else that could outlive a cut".
    let started = Instant::now();
    // SAFETY: sole borrow of the staging slot for the duration of this call.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(SLOT) };
    let zeros = unsafe { &mut *core::ptr::addr_of_mut!(ZEROS) };
    InitRecord { store: BENCH_STORE }.encode_slot_into(&mut slot.0).expect("a stride-sized buffer");
    let init_file = fat.create_fixed(obc2, "INIT.REC", SLOT_FILE_LEN as u32, &mut zeros.0).expect("INIT.REC");
    // Body first, then its gate — the ordering every gated record uses.
    fat.write_at(init_file, 0, &slot.0[..512]).expect("INIT body");
    fat.sync_fixed(init_file, SLOT_FILE_LEN as u32).expect("sync");
    let gate: [u8; 512] = slot.0[512..1_024].try_into().expect("512 bytes");
    fat.write_gate(init_file, 512, &gate).expect("INIT gate");
    fat.sync_fixed(init_file, SLOT_FILE_LEN as u32).expect("sync");
    vmgr.close_file(init_file).expect("close");
    info!("INIT  INIT.REC (16,384 B + witness gate): {=u64} ms", ms(started));

    // The two role trees: 256 shard directories each, in numeric order.
    for role in ["GEN", "WORK"] {
        let started = Instant::now();
        fat.make_dir(obc2, role).expect("role");
        let dir = vmgr.open_dir(obc2, role).expect("role opens");
        for shard in 0u16..256 {
            let mut name = [0u8; 2];
            name[0] = hex(shard >> 4);
            name[1] = hex(shard & 0xF);
            let name = core::str::from_utf8(&name).expect("ascii");
            if let Err(error) = fat.make_dir(dir, name) {
                error!("INIT  {=str}/{=str} failed ({})", role, name, defmt::Debug2Format(&error));
                break;
            }
        }
        vmgr.close_dir(dir).expect("close");
        info!("INIT  {=str}/ + 256 shard directories: {=u64} ms", role, ms(started));
    }
    fat.make_dir(obc2, "IMPORT").expect("IMPORT");

    // The fixed metadata files, each written to its full length in zeros (§13.1).
    let mut filled = 0u64;
    let fill_started = Instant::now();
    for (name, len) in FIXED_FILES {
        let started = Instant::now();
        match fat.create_fixed(obc2, name, len, &mut zeros.0) {
            Ok(file) => {
                let elapsed = us(started);
                info!(
                    "INIT  {=str} {=u32} B zero-filled in {=u64} ms ({=u64} kB/s)",
                    name,
                    len,
                    elapsed / 1_000,
                    rate(len as u64, elapsed)
                );
                filled += len as u64;
                vmgr.close_file(file).expect("close");
            }
            Err(error) => error!("INIT  {=str} failed ({})", name, defmt::Debug2Format(&error)),
        }
    }
    let fill_us = us(fill_started);
    info!(
        "INIT  metadata zero-fill {=u64} B in {=u64} ms ({=u64} kB/s); with INIT.REC's 16,384 B that is §13.1's {=usize}",
        filled,
        fill_us / 1_000,
        rate(filled, fill_us),
        INITIALIZATION_ZERO_FILL
    );
    // §12 deletes INIT.REC once the first checkpoint gate is durable. This bench writes no
    // checkpoint — the catalog codec is the store's job, not the adapter's — so the witness stays,
    // and its presence is the honest statement that this store was never born.
    info!("INIT  TOTAL first-boot initialization: {=u64} ms (no first checkpoint: this is a media bench)", ms(total));
    obc2
}

// ── 3. clean flush (§13.1) ──────────────────────────────────────────────────────────────────────

/// **The measurement that validates or falsifies OBC2's clean-flush assumption.**
///
/// §13.1: "Synchronizing a fixed-length gated file MUST NOT rewrite its directory entry and MUST
/// NOT rewrite FSInfo." The store's commit path performs three such syncs per commit, and the
/// sector at risk is the one holding every `/OBC2` directory entry — lose it and every metadata
/// file in the store becomes unreachable. So this records the exact LBAs the FAT layer writes, for
/// the adapter's clean path and for the fork's own `flush_file`, and names each one.
fn clean_flush_phase(fat: &Fat, log: &Log, obc2: RawDirectory, geometry: &VolumeGeometry) {
    let vmgr = fat.volume_manager();
    let entry_lba = directory_entry_lba(fat, obc2, "ARM0.HND");
    let Ok(file) = fat.open_fixed(obc2, "ARM0.HND", SLOT_FILE_LEN as u32) else {
        error!("FLUSH ARM0.HND is not at its full length — initialization did not complete");
        return;
    };
    info!(
        "FLUSH ARM0.HND is 16,384 B; its directory entry lives in LBA {=u32}, FSInfo in LBA {=u32}",
        entry_lba.unwrap_or(0),
        geometry.fs_info_lba.unwrap_or(0)
    );

    // The adapter's clean path: body, sync, gate, sync — the OBC2 commit shape.
    let body = [0xA5u8; 512];
    let gate = [0x5Au8; 512];
    log.arm();
    fat.write_at(file, 0, &body).expect("body");
    fat.sync_fixed(file, SLOT_FILE_LEN as u32).expect("sync");
    fat.write_gate(file, 512, &gate).expect("gate");
    fat.sync_fixed(file, SLOT_FILE_LEN as u32).expect("sync");
    log.disarm();
    let clean = report_spans(log, geometry, entry_lba, "FLUSH clean");

    // The same sequence through the fork's own flush, which is what §13.1 rules out.
    log.arm();
    fat.write_at(file, 0, &body).expect("body");
    fat.sync_metadata(file).expect("flush_file");
    log.disarm();
    let dirty = report_spans(log, geometry, entry_lba, "FLUSH forks flush_file");

    info!("FLUSH VERDICT: a clean sync wrote {=u32} metadata sector(s); the fork's flush wrote {=u32}", clean, dirty);
    if clean == 0 {
        info!("FLUSH §13.1 clean-flush obligation HOLDS on this card with the adapter's sync_fixed");
    } else {
        error!("FLUSH §13.1 clean-flush obligation VIOLATED — OBC2's C2 assumption does not hold here");
    }
    let _ = vmgr.close_file(file);
}

/// Prints every recorded span with the structure it landed in, and returns how many of those
/// sectors were single-copy metadata — FSInfo, the directory entry, the boot record or a FAT.
fn report_spans(log: &Log, geometry: &VolumeGeometry, entry_lba: Option<u32>, tag: &str) -> u32 {
    let mut metadata = 0;
    log.with_spans(|spans| {
        for span in spans {
            for offset in 0..span.blocks {
                let lba = span.start + offset;
                let region = geometry.region(lba);
                let is_entry = Some(lba) == entry_lba;
                let name = match (region, is_entry) {
                    (_, true) => "DIRECTORY ENTRY",
                    (Region::FsInfo, _) => "FSINFO",
                    (Region::Reserved, _) => "BOOT/RESERVED",
                    (Region::Fat(_), _) => "FAT",
                    (Region::RootDir, _) => "ROOT DIR",
                    (Region::Data, _) => "data",
                    (Region::BeforeVolume, _) => "BEFORE VOLUME",
                    (Region::BeyondVolume, _) => "BEYOND VOLUME",
                };
                if !matches!((region, is_entry), (Region::Data, false)) {
                    metadata += 1;
                }
                info!("{=str}: wrote LBA {=u32} — {=str}", tag, lba, name);
            }
        }
        if spans.is_empty() {
            info!("{=str}: wrote no sectors at all", tag);
        }
    });
    if log.dropped() > 0 {
        warn!("{=str}: {=u32} span(s) did not fit the log — the count above is a lower bound", tag, log.dropped());
    }
    metadata
}

/// The LBA of the sector holding `name`'s 32-byte directory entry, from the FAT layer's own view.
fn directory_entry_lba(fat: &Fat, dir: RawDirectory, name: &str) -> Option<u32> {
    let mut found = None;
    let _ = fat.volume_manager().iterate_dir(dir, |entry| {
        if found.is_none() && entry.name.base_name() == name.split('.').next().unwrap_or("").as_bytes() {
            found = Some(entry.entry_block.0);
        }
    });
    found
}

// ── 4. commit throughput ────────────────────────────────────────────────────────────────────────

/// Times the gated-record commit cycle and the full-stride payload write.
///
/// The commit cycle is what §13 asks for: one journal record is a 1,536-byte body write, a sync, a
/// 512-byte gate write and a sync. The full-stride write beside it is the payload rate at the
/// 16,384-byte granule the format's slots use, which is the number a card-resident catalog is
/// budgeted against.
fn throughput_phase(fat: &Fat, obc2: RawDirectory, geometry: &VolumeGeometry) {
    let vmgr = fat.volume_manager();
    let Ok(journal) = fat.open_fixed(obc2, "COMMIT.JNL", JOURNAL_FILE_LEN as u32) else {
        error!("RATE  COMMIT.JNL is not at its full length");
        return;
    };
    // SAFETY: sole borrow of the staging slot.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(SLOT) };

    let started = Instant::now();
    let mut worst = 0u64;
    for index in 0..COMMITS {
        let commit = Instant::now();
        if !write_record(fat, journal, slot, index) {
            break;
        }
        worst = worst.max(us(commit));
    }
    let elapsed = us(started);
    info!(
        "RATE  {=u16} gated commits (body 1,536 B + sync + gate 512 B + sync): {=u64} us total, {=u64} us mean, {=u64} us worst",
        COMMITS,
        elapsed,
        elapsed / COMMITS as u64,
        worst
    );
    info!("RATE  → {=u64} commits/s at the mean", 1_000_000 / (elapsed / COMMITS as u64).max(1));

    // The full-stride payload write, at the same 16,384-byte granule the slots use.
    let strides = 32u32;
    slot.0.fill(0x33);
    let started = Instant::now();
    for index in 0..strides {
        let offset = (JOURNAL_SLOTS as u32 - 1 - index) * SLOT_STRIDE as u32;
        if let Err(error) = fat.write_at(journal, offset, &slot.0) {
            error!("RATE  stride write at {=u32} failed ({})", offset, defmt::Debug2Format(&error));
            break;
        }
    }
    let elapsed = us(started);
    let bytes = strides as u64 * SLOT_STRIDE as u64;
    info!(
        "RATE  {=u32} x 16,384 B full-stride writes: {=u64} us ({=u64} us each, {=u64} kB/s)",
        strides,
        elapsed,
        elapsed / strides as u64,
        rate(bytes, elapsed)
    );

    // Two read shapes, because the difference is a product decision rather than a curiosity.
    //
    // The whole-slot scan is what a validator that also checks §6's zero pad pays: 16,384 B per
    // slot. The mount-shaped scan reads only the 2,048 bytes a record actually occupies — its
    // 1,536-byte body and its gate — which is what §12's "fixed number of bounded reads" costs if
    // the pad is left to a lazy check.
    for (label, span) in [("whole-slot", SLOT_STRIDE), ("mount-shaped", JOURNAL_GATE_OFFSET + 512)] {
        let started = Instant::now();
        let mut read = 0u64;
        for index in 0..JOURNAL_SLOTS as u32 {
            if fat.read_at(journal, index * SLOT_STRIDE as u32, &mut slot.0[..span]).is_err() {
                break;
            }
            read += span as u64;
        }
        let elapsed = us(started);
        info!(
            "RATE  {=str} journal scan ({=u64} B over 256 slots): {=u64} ms ({=u64} kB/s)",
            label,
            read,
            elapsed / 1_000,
            rate(read, elapsed)
        );
    }
    info!(
        "RATE  cluster is {=u32} B = {=u32} slot stride(s); the data region starts at LBA {=u32}",
        geometry.cluster_bytes,
        geometry.cluster_bytes / SLOT_STRIDE as u32,
        geometry.data_start_lba
    );

    // The stride writes above left 32 slots at the far end of the journal full of 0x33, which is
    // not a valid record and must not be mistaken for one. Zero them so the recovery scan sees the
    // clean tail §6.3 expects.
    slot.0.fill(0);
    for index in 0..strides {
        let offset = (JOURNAL_SLOTS as u32 - 1 - index) * SLOT_STRIDE as u32;
        let _ = fat.write_at(journal, offset, &slot.0);
    }
    let _ = vmgr.close_file(journal);
}

/// Writes one journal record the way §6 orders it: the whole body, synchronized, and then the gate.
///
/// The record is a retention record with an empty mutation — the smallest thing §6.1 admits without
/// an operation identity — carrying sequence `index + 1` at physical slot `index`, so a later scan
/// can check every slot against where it was found.
fn write_record(fat: &Fat, journal: RawFile, slot: &mut Aligned<SLOT_STRIDE>, index: u16) -> bool {
    let body = JournalBody {
        store: BENCH_STORE,
        epoch: 1,
        sequence: index as u64 + 1,
        slot: index,
        kind: RecordKind::Retention,
        operation: OperationId::ZERO,
        intent: [0u8; 32],
        mutation: Mutation { retained: Some(Change::Remove(GenerationId::new(index as u64))), ..Mutation::default() },
    };
    if body.encode_slot_into(&mut slot.0).is_err() {
        return false;
    }
    let base = index as u32 * SLOT_STRIDE as u32;
    let write = |offset: u32, bytes: &[u8]| -> Result<(), AdapterError> {
        fat.write_at(journal, base + offset, bytes)?;
        fat.sync_fixed(journal, JOURNAL_FILE_LEN as u32)
    };
    if let Err(error) = write(0, &slot.0[..JOURNAL_BODY_LEN]) {
        error!("RATE  body write at slot {=u16} failed ({})", index, defmt::Debug2Format(&error));
        return false;
    }
    if let Err(error) = write(JOURNAL_GATE_OFFSET as u32, &slot.0[JOURNAL_GATE_OFFSET..JOURNAL_GATE_OFFSET + 512]) {
        error!("RATE  gate write at slot {=u16} failed ({})", index, defmt::Debug2Format(&error));
        return false;
    }
    true
}

// ── 5. recovery across resets ───────────────────────────────────────────────────────────────────

/// Scans all 256 journal slots and reports the decision §6.3's replay would make.
///
/// Returns the length of the contiguous valid prefix. The scan is deliberately over *every* slot
/// rather than stopping at the first invalid one, because §6.3's all-slot scan is what "turns any
/// loss that does occur into a fail-closed mount rather than a silent rollback": a valid slot
/// beyond the prefix is a gap, and a gap is corruption.
fn scan_journal(fat: &Fat, journal: RawFile) -> usize {
    // SAFETY: sole borrow of the staging slot.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(SLOT) };
    let started = Instant::now();
    let mut prefix = 0usize;
    let mut prefix_open = true;
    let mut strays = 0usize;
    let mut mismatched = 0usize;
    for index in 0..JOURNAL_SLOTS {
        if fat.read_at(journal, index as u32 * SLOT_STRIDE as u32, &mut slot.0).is_err() {
            error!("SCAN  slot {=usize} unreadable", index);
            prefix_open = false;
            continue;
        }
        match JournalBody::validate_slot(&slot.0, index as u16) {
            Ok(body) => {
                if body.store != BENCH_STORE || body.sequence != index as u64 + 1 {
                    mismatched += 1;
                }
                if prefix_open {
                    prefix = index + 1;
                } else {
                    strays += 1;
                }
            }
            Err(_) => prefix_open = false,
        }
    }
    info!(
        "SCAN  256 slots in {=u64} ms: contiguous valid prefix {=usize}, stray valid slots beyond it {=usize}, sequence/store mismatches {=usize}",
        ms(started),
        prefix,
        strays,
        mismatched
    );
    if strays != 0 || mismatched != 0 {
        error!("SCAN  the journal is NOT a clean prefix — §6.3 would fail this mount closed");
    }
    prefix
}

/// The recovery cycle: verify what the last run left, append exactly one record, and prove it.
fn append_and_verify(fat: &Fat, obc2: RawDirectory, valid: usize) {
    let vmgr = fat.volume_manager();
    let Ok(journal) = fat.open_fixed(obc2, "COMMIT.JNL", JOURNAL_FILE_LEN as u32) else {
        error!("RCVR  COMMIT.JNL is not at its full length");
        return;
    };
    if valid >= JOURNAL_SLOTS {
        error!("RCVR  the journal is full");
        let _ = vmgr.close_file(journal);
        return;
    }
    // SAFETY: sole borrow of the staging slot.
    let slot = unsafe { &mut *core::ptr::addr_of_mut!(SLOT) };
    let started = Instant::now();
    let appended = write_record(fat, journal, slot, valid as u16);
    let commit_us = us(started);
    if !appended {
        error!("RCVR  the append failed");
        let _ = vmgr.close_file(journal);
        return;
    }
    let after = scan_journal(fat, journal);
    if after == valid + 1 {
        info!(
            "RCVR  OK — recovery found {=usize}, appended one in {=u64} us, and now finds {=usize}",
            valid, commit_us, after
        );
    } else {
        error!("RCVR  MISMATCH — expected {=usize} durable records after the append, found {=usize}", valid + 1, after);
    }
    info!("RCVR  soft reset only: `probe-rs reset` never removed the card's supply, so this says");
    info!("RCVR  nothing about §1.1 tearing. Reset and run again to add another cycle.");
    let _ = vmgr.close_file(journal);
}

// ── helpers ─────────────────────────────────────────────────────────────────────────────────────

fn hex(nibble: u16) -> u8 {
    match nibble {
        0..=9 => b'0' + nibble as u8,
        _ => b'A' + (nibble as u8 - 10),
    }
}

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
    info!("obc2_media_bench: done — parked");
    loop {
        cortex_m::asm::wfi();
    }
}
