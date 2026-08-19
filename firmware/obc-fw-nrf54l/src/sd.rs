//! Legacy FatFs storage for the nRF54L board: maps, ride logs, weather, and firmware updates.
//!
//! This owns the concrete transport → [`VolumeManager`] stack and reconciles the FAT filesystem to
//! the shared app's *intent*. Routes and trips are flat-store-only; this module retains the FAT
//! surfaces that have not yet moved. The reusable, board-agnostic adapters it hands the format code
//! live in [`obc_storage::sd`] ([`SdByteSource`]/[`SdByteSink`]/[`SdTrackSink`]); everything here is
//! nRF-specific.
//!
//! The `Storage` impl and every adapter below are generic over the concrete **block-device type**
//! (they speak `embedded_sdmmc`'s `BlockDevice` / `TimeSource`). The chosen map streams from the
//! card and the ride is logged to a temp `.obct` converted to the durable ride object on Finish.
//!
//! ## Card layout (FAT16/FAT32)
//!   `/<name>.obcm`   — a side-loaded map (long filename, dragged on from a computer)
//!   `/MP{id}.OBM`    — a map the device received over USB (issue #927): the durable object id lives
//!                      in the 8.3 name. `OBM` is the device-created 3-char twin of `.obcm` —
//!                      embedded-sdmmc creates short names only. The upload streams **straight into
//!                      this file** with its 4-byte magic held back, so a torn write leaves a
//!                      zero-magic file the scan refuses and the boot sweep reclaims.
//!   `/MAP.SEL`       — which map the renderer streams from (see `obc_app::store_meta`); absent or
//!                      torn = no preference, and the loader takes the first readable map
//!   `/tracks/`       — saved rides (created if absent); the in-progress log lives here as
//!                      `TRACK.OBT` and is deleted once converted. Each Finish writes **one**
//!                      artifact: the BLE ride object `RD{id}.ORD` (the durable ride object id
//!                      lives in the name). The device writes no GPX — the phone owns human-format
//!                      export after sync.
//!
//! ## The transport: **native 4-bit SD over Nordic's sEMMC soft peripheral** (epic #1158)
//!
//! The card is not on a SPI bus. The FLPR (VPR00) runs Nordic's sEMMC image and *is* the SD host
//! controller; [`crate::semmc`] is the M33-side driver and [`crate::flpr_mux`] decides whether the
//! coprocessor is currently drawing the panel or clocking the card. Wiring (fixed by the soft
//! peripheral, not by us):
//!
//! ```text
//!   P2.00 D3   P2.01 CLK   P2.02 D0   P2.03 D2   P2.04 D1   P2.05 CMD
//! ```
//!
//! Reads run 4-bit at **32 MHz** (14.7 MB/s measured, CMD18 × 256 blocks) and writes at 21.3 MHz
//! (8.2 MB/s, card-program-limited) — against 1.07 MB/s over the SPI transport this replaced. The
//! only thing this file does about any of that is [`SemmcCard`]: a `BlockDevice` over
//! [`crate::semmc::Semmc`]. Everything above it — [`Storage`], the FAT layer, the extent fast path,
//! the object store, the boot-fault rule — is transport-agnostic and did not move.

#[cfg(feature = "sd-bench")]
use embassy_time::Instant;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, LfnBuffer, Mode, RawDirectory, RawFile, ShortFileName, TimeSource,
    Timestamp, VolumeManager,
};
use heapless::{String, Vec};
use obc_app::ride::{decode_synced_rides, encode_synced_rides, SyncedRides, SYNCED_RIDES_MAX_LEN};
use obc_app::store_meta::{decode_store_epoch, encode_store_epoch, STORE_EPOCH_LEN};
use obc_app::{MAX_RIDES, UI_RIDES_CAP};
use obc_dfu::armer::{ExtentsError, ScanError, StageIo};
use obc_formats::io::ByteSource;
use obc_formats::obcr::NAME_CAP;
use obc_route::{ride_elevation_profile, ride_preview_polyline, track_to_ride, Profile, RideInfo, RideStats};
use obc_storage::shared_device::SharedBlockDevice;
use obc_storage::{SdByteSink, SdByteSource, SdTrackSink};

/// The in-progress ride log on the card — a header-less array of fixed track records (8.3
/// name). Truncated-and-reused per ride, converted to the `RD{id}.ORD` ride object, then
/// deleted on Finish.
const TRACK_TMP: &str = "TRACK.OBT";

/// The synced-ride sidecar in `/tracks` (epic #447 P7 / #454): the set of ride object ids the phone
/// has downloaded at least once, so the Rides screen can render an *unsynced* ride's delete footer
/// with the warning cue. In `/tracks` (not RRAM) so it survives a reflash and travels with the card
/// alongside the rides it flags. Rewritten on a download completion; parsed leniently (a torn/missing
/// file = "nothing synced"). Codec + torn-line semantics live in `obc-app::settings` (host-tested).
const SYNCED_SET: &str = "SYNCED.SET";

/// Which step of a CRC-framed sidecar rewrite failed (finding #876-5). A truncating rewrite is only
/// **durable** when open, write, flush, **and** close all succeed; a swallowed flush/close error is a
/// torn persist. Callers whose failure direction is safe by design (a torn retention/synced sidecar
/// decodes conservatively → nothing deletes) may treat this best-effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidecarWriteError {
    /// The directory was absent, or the file could not be opened for a truncating rewrite.
    Open,
    /// The payload write failed.
    Write,
    /// The flush after the write failed (bytes may not have reached the card).
    Flush,
    /// The close failed (the directory entry / length may not be committed).
    Close,
}

/// The store-epoch nonce file in the **card root** (protocol v2 #632 item 5; card-resident #776):
/// the `u32` id-era name the phone reads over the pre-pairing `protocolVersion` read. Kept in the
/// card **root** (not `/tracks`) because the epoch names the *whole* store, not rides specifically
/// — so the SD card is the sole home of the id-era name: a card swap
/// transplants the store's identity (swap back restores the old era, a card written by a *different*
/// device presents *its* epoch — its own scope, closing the foreign-card hole the retired RRAM line
/// left open). Minted/rewritten only at boot by the mint pass; a missing/torn file reads as "no
/// epoch" → the mint rule draws a fresh one. Codec + torn-line semantics live in `obc-app::settings`
/// (host-tested), the direct analogue of the `SYNCED.SET` sidecar.
const EPOCH_FILE: &str = "EPOCH.OBE";

/// The staged firmware update in the **card root** (epic #615, locked: 8.3-safe, no LFN — the
/// same file contract the future LM20 USB-MSC epic exposes). Sideloaded by the user (or, S6, the
/// phone); the armer only ever reads it.
const UPDATE_BIN: &str = "UPDATE.BIN";

/// The armer's snapshot of the **running** image (epic #615 S4, #619), in the card root next to
/// [`UPDATE_BIN`]: a full OBCU container (64-byte header + raw image read straight out of RRAM),
/// truncated-and-reused per arm like `TRACK.OBT`. The bootloader flashes it back if a trial boot
/// goes unconfirmed.
const ROLLBACK_BIN: &str = "ROLLBACK.BIN";

/// The concrete SD stack for this board: [`SemmcCard`] — the card in native 4-bit mode on the FLPR
/// — under a 16-file/4-dir [`VolumeManager`].
///
/// The manager keeps a larger-than-default handle budget for the retained ride and update paths.
type Sd = SemmcCard;
/// What the retained legacy manager owns: the card by shared reference, leaving its raw handle
/// available to the DFU extent resolver.
type SdShared = SharedBlockDevice<'static, Sd>;
/// The open-handle budget (see the file-count note above) — one set of consts so the manager and
/// the `obc-platform` wrapper aliases below can never drift apart.
const SD_MAX_DIRS: usize = 4;
/// **Why 16, and why it is not 6 again.** The pre-volume-set budget was 6: the 5-handle mid-ride
/// peak documented above plus one slot of headroom. A mounted volume set then needed one handle per
/// shard for the mount lifetime, and 16 is where that landed.
///
/// The set is gone (FS7.5-c3b, #1420) and this is **deliberately not reverted to 6**. The cost is
/// 640 B of `.bss` that the resource baseline already accounts for, and the reason to leave it is
/// that this whole module is FS11's to delete (#1393) — shrinking a budget inside a stack that is
/// being retired trades a measured, harmless allocation for a fresh chance to under-size the one
/// thing that fails *only* mid-ride. If FS11 slips, this is a two-line change with a number
/// already attached.
///
/// The cost is measured, not guessed: the fork's `FileInfo` (`filesystem/files.rs`) is `RawFile`
/// 4 · `RawVolume` 4 · `current_cluster` 8 · `current_offset` 4 · `Mode` 1 · `DirEntry` 40 ·
/// `dirty` 1, i.e. **64 B** at `align 4` on thumbv8m. `6 → 16` is ten slots, **+640 B of `.bss`**
/// — the manager's `open_files` array is a `heapless::Vec<FileInfo, SD_MAX_FILES>` and nothing
/// else scales with it. Nothing on the stack changes.
const SD_MAX_FILES: usize = 16;
const SD_MAX_VOLUMES: usize = 1;
type Vmgr = VolumeManager<SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;
/// [`SdTrackSink`] over this board's manager.
type TrackSinkT<'a> = SdTrackSink<'a, SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;

/// FAT timestamps need a clock; the device has none yet (see [`obc_ports::TrackPoint::t_ms`]),
/// so every file gets the epoch. Real dates wait on a clock source.
/// `pub(crate)` only because it surfaces in the adapter return types the loop names.
pub(crate) struct NullTime;
impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// The mounted legacy FAT card: ride, update and weather state retained until their flat slices land.
pub struct Storage {
    vmgr: Vmgr,
    /// The raw card the manager's [`SdShared`] borrows — the extent path's direct read handle.
    card: &'static Sd,
    root: RawDirectory,
    /// `/tracks` (created on mount if absent), or `None` if it couldn't be opened/created
    /// (rides then can't be saved, but the rest still works).
    tracks_dir: Option<RawDirectory>,
    /// 8.3 filename of each *ride* catalog entry, parallel to the ride order
    /// [`scan_rides_into`](Storage::scan_rides_into) last returned — so a ride's durable object id resolves back
    /// to the `RD{id}.ORD` file for detail reads and object-store deletes.
    ride_files: Vec<ShortFileName, UI_RIDES_CAP>,
    /// Each ride catalog entry's **durable object id**, parallel to [`ride_files`](Storage::ride_files)
    /// — filename-encoded (`RD{id}.ORD`), the identity the app's ride-menu remap and the phone's
    /// synced/tombstone sets key on.
    ride_ids: Vec<u16, UI_RIDES_CAP>,
    /// The open ride log for the current tracking session.
    open_track: Option<OpenTrack>,
    /// A finished ride whose log → ride-object conversion hasn't run yet. Finish only closes the
    /// log and stashes this; the ride loop runs [`run_pending_save`](Storage::run_pending_save)
    /// once the confirm animation has left the glass, so the save's blocking SD stretch never
    /// freezes the hold bulge (the "finishing a ride is laggy" bug).
    pending_save: Option<PendingSave>,
    /// A ride object landed on the card this pass ([`run_pending_save`](Storage::run_pending_save)
    /// committed an `RD{id}.ORD`) and the store edge hasn't been raised yet. Set by **every** save
    /// path (the quiet-glass deferred run *and* the back-to-back flush inside
    /// [`begin_track`](Storage::begin_track)/[`reconcile_track`](Storage::reconcile_track)), drained
    /// once per ride-loop pass via [`take_ride_saved`](Storage::take_ride_saved) — which is what
    /// makes a freshly-finished ride reach the Rides menu (and, on `ble`, the phone's catalog)
    /// without a reboot.
    ride_saved: bool,
}

/// One open `.obct` ride log: the session it belongs to, its file handle, and the save name
/// (the route name, frozen at begin, so a later route change can't rename a finished file).
struct OpenTrack {
    session: u32,
    file: RawFile,
    name: String<NAME_CAP>,
}

/// A Finish waiting for its deferred log → `RD{id}.ORD` conversion: the save name and the ride
/// totals snapshotted on the Finish frame. The log itself is already flushed + closed on the card
/// as [`TRACK_TMP`], so a power cut before the conversion loses nothing a crash mid-ride wouldn't.
struct PendingSave {
    name: String<NAME_CAP>,
    stats: RideStats,
}

/// **Bring the card up, and mount nothing on it.** Boot the sEMMC soft peripheral and identify the
/// card (4-bit, High Speed, 32 MHz reads).
///
/// Boot brings the card up once and hands its raw blocks directly to the flat store. Cards without
/// a valid flat superblock are rejected by the boot composition; no filesystem fallback follows.
///
/// Card identification is the slow part — the ACMD41 power-up poll is bounded at 1.5 s.
/// [`flpr_mux::bring_up_storage`](crate::flpr_mux::bring_up_storage) is what holds the FLPR in
/// storage mode across the whole of it (see its doc, and PR #1160's `Semmc::start` contract).
///
/// ## ⚠️ Synchronous on purpose — the async-fn frame trap (#677, #1108)
///
/// The obvious shape for this is `async fn`, and it was one for a while: `Semmc::start` yields at
/// the card settle, the CMD8 deliver-and-abort and each ACMD41 poll. Awaited from `main`, that
/// chain's coroutine flattens into `main`'s **task-body poll frame**, whose slot set is allocated on
/// entry on *every* poll for the life of the program — measured **6,912 → 13,376 B** for a function
/// that runs once at boot, and `#[inline(never)]` does not fix it (it governs the future
/// constructor, not the coroutine body). Synchronous — the shape that ships — the same build
/// measures **7,328 B**, which is what the resource guard pins as `task_frame_measured`.
///
/// It is safe to block here: bring-up runs before the ride loop, the BLE stack and the USB plane
/// exist, and the panel's anti-DC-bias COM wave is on the P3 `InterruptExecutor` (or, on `com-hw`,
/// on TIMER + DPPI + GPIOTE), so it preempts thread mode rather than competing with it. See
/// `Semmc::start`'s note for the full accounting.
pub fn bring_up_card() -> Result<(), obc_app::BootFault> {
    let info = match crate::flpr_mux::bring_up_storage() {
        Ok(info) => info,
        Err(e) => return Err(bring_up_fault(e)),
    };
    defmt::info!(
        "SD: card up over sEMMC — {=u32} MB, 4-bit, {=u32} MHz reads (high-speed {=bool}), RCA 0x{=u16:04x}; FLPR mode: {=str}",
        (info.blocks >> 11),
        info.read_clk_hz / 1_000_000,
        info.high_speed,
        info.rca,
        crate::flpr_mux::mode_name()
    );
    Ok(())
}

/// **Which fault screen a failed bring-up earns** — the honesty rule (#1163 review, P3): a fault
/// line the rider can act on, not one catch-all.
///
/// Three classes, because the driver can genuinely tell them apart:
///
/// - [`SemmcError::NoCard`] is the only one that means what "NO SD CARD" says — the host came up and
///   identification found nothing (empty socket, dead card, broken bus).
/// - [`SemmcError::UnsupportedCard`] means a working card that is SDSC. Dropping SDSC is deliberate
///   (byte-addressed, ≤2 GB, no map fits), but a card that worked over the retired SPI path now
///   fails, and "NO SD CARD" would send its owner hunting for a card that is already inserted.
/// - everything else is the storage subsystem itself — the soft peripheral that would not boot, a
///   barrier that never echoed, a transport error during identification. None of those are evidence
///   about whether a card is present, so they read as the honest superset.
fn bring_up_fault(e: crate::semmc::SemmcError) -> obc_app::BootFault {
    use crate::semmc::SemmcError;
    match e {
        SemmcError::NoCard => {
            defmt::warn!("SD: card identification found no card — NO SD CARD");
            obc_app::BootFault::NoCard
        }
        SemmcError::UnsupportedCard => {
            defmt::error!("SD: card is SDSC (CSD v1, <=2 GB) — rejected; CARD UNSUPPORTED");
            obc_app::BootFault::CardUnsupported
        }
        other => {
            defmt::error!("SD: the sEMMC host did not come up ({}) — STORAGE FAULT, not a missing card", other);
            obc_app::BootFault::StorageFault
        }
    }
}

// ═══════════════════════════ the block device ═══════════════════════════

/// **The card as an `embedded_sdmmc::BlockDevice`** — the whole transport (epic #1158).
///
/// A zero-sized handle: the driver state is the one [`Semmc`](crate::semmc::Semmc) in
/// [`crate::flpr_mux`], which also decides whether the FLPR is currently drawing the panel or
/// clocking the card. Every method here is one `flpr_mux::with_storage` call — mode ensured, driver
/// borrowed, transfer issued — so there is exactly one place that can get the ordering wrong.
///
/// Blocking, like the SPI transport before it: `BlockDevice` is a synchronous trait and everything
/// above it (FAT, extents, the object store) is built on that. The transfers are ~30× shorter now.
#[derive(Clone, Copy)]
pub(crate) struct SemmcCard;

const BLOCK_LEN: usize = Block::LEN;

/// `Block` is `#[repr(transparent)]` over `[u8; 512]`, which is what makes the byte views below
/// sound. Pinned here because the whole bounce/fast-path split is built on it.
const _: () = assert!(core::mem::size_of::<Block>() == BLOCK_LEN);

fn log_transfer_error(op: &'static str, lba: u32, blocks: usize, e: crate::semmc::SemmcError) {
    match e {
        crate::semmc::SemmcError::Aborted(status) => defmt::warn!(
            "SD: {=str} of {=usize} block(s) @ {=u32} aborted — {=str} (STATUS 0x{=u32:08x})",
            op,
            blocks,
            lba,
            crate::semmc::SemmcError::abort_reason(status),
            status
        ),
        other => {
            defmt::warn!("SD: {=str} of {=usize} block(s) @ {=u32} failed — {}", op, blocks, lba, other)
        }
    }
}

impl BlockDevice for SemmcCard {
    type Error = crate::semmc::SemmcError;

    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, n) = (blocks.as_ptr() as usize, blocks.len());
        #[cfg(feature = "sd-bench")]
        let bench_started = Instant::now();
        let r = crate::flpr_mux::with_storage(|sd| {
            if addr.is_multiple_of(4) {
                // SAFETY: `Block` is `#[repr(transparent)]` over `[u8; 512]` (asserted above), so a
                // `&mut [Block]` is exactly this byte span, exclusively borrowed for the call.
                let buf = unsafe { core::slice::from_raw_parts_mut(blocks.as_mut_ptr().cast::<u8>(), n * BLOCK_LEN) };
                return sd.read_blocks(start_block_idx.0, buf);
            }
            // SAFETY: sole borrow — this runs inside `with_storage`, which is non-re-entrant.
            unsafe {
                crate::card_io::with_bounce(addr, |bounce| {
                    let bounce_blocks = bounce.len() / BLOCK_LEN;
                    for (i, chunk) in blocks.chunks_mut(bounce_blocks).enumerate() {
                        let len = chunk.len() * BLOCK_LEN;
                        sd.read_blocks(start_block_idx.0 + (i * bounce_blocks) as u32, &mut bounce[..len])?;
                        for (b, src) in chunk.iter_mut().zip(bounce[..len].chunks_exact(BLOCK_LEN)) {
                            b.contents.copy_from_slice(src);
                        }
                    }
                    Ok(())
                })
            }
        })?;
        #[cfg(feature = "sd-bench")]
        crate::card_io::note_read_perf(bench_started, addr, n);
        if let Err(e) = r {
            log_transfer_error("read", start_block_idx.0, n, e);
        }
        r
    }

    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        if blocks.is_empty() {
            return Ok(());
        }
        let (addr, n) = (blocks.as_ptr() as usize, blocks.len());
        let r = crate::flpr_mux::with_storage(|sd| {
            if addr.is_multiple_of(4) {
                // SAFETY: as in `read` — `Block` is `#[repr(transparent)]` over `[u8; 512]`, and the
                // shared borrow covers the whole span for the call.
                let buf = unsafe { core::slice::from_raw_parts(blocks.as_ptr().cast::<u8>(), n * BLOCK_LEN) };
                return sd.write_blocks(start_block_idx.0, buf);
            }
            // SAFETY: as in `read`.
            unsafe {
                crate::card_io::with_bounce(addr, |bounce| {
                    let bounce_blocks = bounce.len() / BLOCK_LEN;
                    for (i, chunk) in blocks.chunks(bounce_blocks).enumerate() {
                        let len = chunk.len() * BLOCK_LEN;
                        for (b, dst) in chunk.iter().zip(bounce[..len].chunks_exact_mut(BLOCK_LEN)) {
                            dst.copy_from_slice(&b.contents);
                        }
                        sd.write_blocks(start_block_idx.0 + (i * bounce_blocks) as u32, &bounce[..len])?;
                    }
                    Ok(())
                })
            }
        })?;
        if let Err(e) = r {
            log_transfer_error("write", start_block_idx.0, n, e);
        }
        r
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        crate::flpr_mux::with_storage(|sd| sd.num_blocks())?.map(BlockCount)
    }
}

impl Storage {
    /// Iterate `dir`'s entries with their long filenames, running `f` per entry. Wraps
    /// `iterate_dir_lfn`'s [`LfnBuffer`] scratch setup (a 256-byte buffer is ample for an 8.3 dir),
    /// so the `.obcm`/`.obcr` scans below don't each repeat it. The iteration error is ignored —
    /// a partial scan still yields what it read, the same as the bare call did.
    fn iter_dir_lfn(&self, dir: RawDirectory, mut f: impl FnMut(&embedded_sdmmc::DirEntry, Option<&str>)) {
        let mut lfn_storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut lfn_storage);
        let _ = self.vmgr.iterate_dir_lfn(dir, &mut lfn, |e, long| f(e, long));
    }

    /// Scan `/tracks` for stored ride objects into the app's Rides menu (epic #447 P7 / #454):
    /// [`RideSummary`](obc_app::RideSummary) per `RD{id}.ORD` — the **newest [`UI_RIDES_CAP`]**
    /// (by `start_time`), newest first — each stamped with its `synced` flag from the
    /// [`SYNCED_SET`] sidecar (read once here, not per file). Fills the parallel
    /// [`ride_files`](Storage::ride_files)/[`ride_ids`](Storage::ride_ids) tables so a
    /// hold-to-delete can resolve a durable id back to its file.
    ///
    /// **Stack discipline** (this fn hard-faulted the 256 KB part at boot, twice, before it
    /// respected the budget): the first cut stacked an ~8 KB aligned sort temp on the ~6 KB
    /// 128-cap catalog (>16 KB one frame); the second kept a 128-cap catalog whose *resident*
    /// twin in `App`+`Storage` ate the deep-render path's last ~1.6 KB of margin — statics and
    /// stack are zero-sum on this part. Now the catalog is [`UI_RIDES_CAP`]-capped (~1.4 KB) and
    /// ordering is a bounded **top-K insertion** as summaries are read — no sort temp at all.
    /// Fills the caller's catalog rather than returning a large buffer by value.
    pub fn scan_rides_into(&mut self, catalog: &mut Vec<obc_app::RideSummary, UI_RIDES_CAP>) {
        catalog.clear();
        self.ride_files.clear();
        self.ride_ids.clear();
        let synced = self.load_synced_set();
        let Some(dir) = self.tracks_dir else { return };

        // Collect (id, name) for every RD{id}.ORD.
        let mut entries: Vec<(u16, ShortFileName), MAX_RIDES> = Vec::new();
        let mut overflow = false;
        self.iter_dir_lfn(dir, |e, _| {
            if let Some(id) = stored_ride_id(&e.name) {
                if entries.push((id, e.name.clone())).is_err() {
                    overflow = true;
                }
            }
        });
        if overflow {
            defmt::warn!("SD: scan: more than {=usize} ride files — the excess is not listed", MAX_RIDES);
        }

        // Read each header and keep the newest UI_RIDES_CAP via bounded insertion: find the
        // summary's slot by descending start_time; a full catalog drops the oldest (or skips the
        // candidate when it is the oldest). The three parallel tables move together on every
        // insert/evict, staying aligned.
        for (id, n) in &entries {
            let file = match self.vmgr.open_file_in_dir(dir, n, Mode::ReadOnly) {
                Ok(f) => f,
                Err(_) => {
                    defmt::warn!("SD: scan: cannot open ride {} — not listed", defmt::Debug2Format(n));
                    continue;
                }
            };
            let len = self.vmgr.file_length(file).unwrap_or(0);
            match RideInfo::read(&SdByteSource::new(&self.vmgr, file, len)) {
                Ok(info) => {
                    let sum = obc_app::RideSummary::from_info(&info, synced.contains(*id), synced.synced_at(*id));
                    let pos = catalog.iter().position(|c| sum.start_time > c.start_time).unwrap_or(catalog.len());
                    // A full catalog evicts its oldest for a newer candidate; a candidate older
                    // than everything listed is simply not one of the newest UI_RIDES_CAP.
                    if catalog.is_full() && pos < catalog.len() {
                        let _ = catalog.pop();
                        let _ = self.ride_files.pop();
                        let _ = self.ride_ids.pop();
                    }
                    if pos <= catalog.len() && !catalog.is_full() {
                        let _ = catalog.insert(pos, sum);
                        let _ = self.ride_files.insert(pos, n.clone());
                        let _ = self.ride_ids.insert(pos, *id);
                    }
                }
                Err(_) => defmt::warn!("SD: scan: ride {} unreadable — not listed", defmt::Debug2Format(n)),
            }
            let _ = self.vmgr.close_file(file);
        }

        if entries.len() > catalog.len() {
            defmt::info!("SD: rides menu lists the newest {=usize} of {=usize} stored", catalog.len(), entries.len());
        }
        defmt::info!("SD: {=usize} ride(s) in /tracks", catalog.len());
    }

    /// Each ride catalog entry's durable object id, parallel to the catalog
    /// [`scan_rides_into`](Storage::scan_rides_into) last returned — the second argument to
    /// [`App::set_rides`](obc_app::App::set_rides).
    pub fn ride_ids(&self) -> &[u16] {
        &self.ride_ids
    }

    /// Read the synced-ride sidecar (`/tracks/SYNCED.SET`) into a [`SyncedRides`] set. A missing,
    /// torn, or malformed sidecar decodes to the **empty** set ("nothing synced") — never a panic
    /// (the codec + torn-line semantics are host-tested in `obc-app::settings`). One file read.
    pub fn load_synced_set(&self) -> SyncedRides {
        let Some(dir) = self.tracks_dir else { return SyncedRides::new() };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, SYNCED_SET, Mode::ReadOnly) else {
            return SyncedRides::new(); // absent = nothing synced
        };
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_synced_rides(&buf[..n])
    }

    /// Record a batch of ride ids as synced at `synced_at` in **one** sidecar read-modify-write (the
    /// `ackRides` command can carry dozens of ids — a per-id rewrite would be that many file
    /// round-trips). Returns how many ids were **newly** flagged; `0` = the sidecar was not
    /// rewritten. Ids already flagged (or dropped by a full set) count as nothing-new.
    pub fn mark_rides_synced(&mut self, ids: impl Iterator<Item = u16>, synced_at: u32) -> usize {
        let mut set = self.load_synced_set();
        // The merge rule itself lives in `SyncedRides::ack` (obc-app), where it is host-tested:
        // add-only, idempotent, first-stamp-wins — which is what lets a desktop ack and a phone
        // heal commute (E1, #911). This function only owns the read-modify-write around it.
        let added = set.ack(ids, synced_at);
        if added > 0 {
            let _ = self.write_synced_set(&set);
        }
        added
    }

    /// The **full** compact ride-retention inventory (finding #876-2): every synced ride's
    /// `id + synced + synced_at`, up to [`MAX_RIDES`], read straight off the synced-set sidecar — so
    /// the auto-delete sweep + the eager `synced_at` stamp reach a synced+expired ride even when it
    /// sits below the newest-[`UI_RIDES_CAP`] the Rides menu shows. An unsynced ride carries no
    /// retention state and is never in the set. One file read; the board hands this to
    /// [`App::set_ride_retention_inventory`](obc_app::App::set_ride_retention_inventory) after each
    /// ride rescan.
    pub fn ride_retention_inventory(&self) -> Vec<obc_app::RideRetentionRecord, MAX_RIDES> {
        let mut out = Vec::new();
        for (id, synced_at) in self.load_synced_set().entries() {
            let _ =
                out.push(obc_app::RideRetentionRecord { id: u64::from(id), synced: true, synced_at_utc: synced_at });
        }
        out
    }

    /// Stamp a **legacy** synced-without-timestamp ride's `synced_at` (auto-expiry epic #638, S3 —
    /// the sweep's [`StampRideSynced`](obc_app::HostCommand::StampRideSynced) countdown-start). Only
    /// ever fills a `0` stamp; rewrites the sidecar only when it changed.
    pub fn stamp_ride_synced_at(&mut self, id: u16, synced_at: u32) {
        let mut set = self.load_synced_set();
        if set.stamp_synced_at(id, synced_at) {
            let _ = self.write_synced_set(&set);
        }
    }

    /// Retire ride `id`'s synced flag from the sidecar (a deleted ride — ids never reuse, so this is
    /// belt-and-braces tidiness). Rewrites the sidecar only when the flag was present.
    pub fn forget_ride_synced(&mut self, id: u16) {
        let mut set = self.load_synced_set();
        if set.remove(id) {
            let _ = self.write_synced_set(&set);
        }
    }

    /// The centralized CRC-framed sidecar rewrite (finding #876-5): open (truncating) → write →
    /// flush → close, checking **every** step, and returning the first that failed. The file is
    /// always flushed + closed even after a write error so the open-file budget is never leaked, and
    /// the failing step is named in the log (the consequence line lives at the call site, which knows
    /// whether the failure is safe-by-design or must surface to the phone).
    fn rewrite_sidecar(
        &mut self,
        dir: Option<RawDirectory>,
        name: &str,
        bytes: &[u8],
    ) -> Result<(), SidecarWriteError> {
        let Some(dir) = dir else { return Err(SidecarWriteError::Open) };
        let file = self
            .vmgr
            .open_file_in_dir(dir, name, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| SidecarWriteError::Open)?;
        // Write, then flush + close **unconditionally** to release the handle, then report the first
        // failing step.
        let wrote = self.vmgr.write(file, bytes).is_ok();
        let flushed = self.vmgr.flush_file(file).is_ok();
        let closed = self.vmgr.close_file(file).is_ok();
        let step = if !wrote {
            Some(SidecarWriteError::Write)
        } else if !flushed {
            Some(SidecarWriteError::Flush)
        } else if !closed {
            Some(SidecarWriteError::Close)
        } else {
            None
        };
        match step {
            Some(e) => {
                defmt::warn!(
                    "SD: sidecar rewrite failed (write {=bool} flush {=bool} close {=bool})",
                    wrote,
                    flushed,
                    closed
                );
                Err(e)
            }
            None => Ok(()),
        }
    }

    /// Read the card-resident store-epoch nonce (`/EPOCH.OBE`, protocol v2 #632 item 5 / #776), or
    /// `None` when the file is **absent** (a fresh/foreign-formatted card) or torn/foreign — "no
    /// epoch", which the boot mint rule ([`obc_app::store_meta::store_epoch_mint`]) treats as clause 1
    /// (draw a fresh nonce). Never panics on malformed input (the codec is host-tested). One file
    /// read; the card **root** is always open on a mounted card.
    pub fn load_card_epoch(&self) -> Option<u32> {
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, EPOCH_FILE, Mode::ReadOnly) else {
            return None; // absent = no epoch (the mint pass draws a fresh one)
        };
        let mut buf = [0u8; STORE_EPOCH_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_store_epoch(&buf[..n])
    }

    /// Overwrite the store-epoch file (truncating) with `epoch`. Called once, from the boot mint
    /// pass, when the mint rule fires — so the write rate is negligible. Returns `true` only when
    /// **every** step — open, write, flush, close — succeeded: a discarded flush/close error is a
    /// torn persist, and the mint pass gates the id-marks write and the served epoch on this result
    /// (unlike the other sidecar writers, whose failure direction is safe by design, a swallowed
    /// epoch-write failure would let a clause-2 mint go permanently undetected: old valid epoch on
    /// card + freshly-written valid floor = steady state next boot — the exact aliasing the epoch
    /// exists to catch). Whole persist within the call (open, write truncating, flush, close), so it
    /// never counts against the open-file budget across an `await`.
    #[must_use]
    pub fn save_card_epoch(&mut self, epoch: u32) -> bool {
        let bytes = encode_store_epoch(epoch);
        let file = match self.vmgr.open_file_in_dir(self.root, EPOCH_FILE, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => file,
            Err(e) => {
                defmt::warn!("SD: cannot open store-epoch file: {}", defmt::Debug2Format(&e));
                return false;
            }
        };
        let wrote = self.vmgr.write(file, &bytes).is_ok();
        let flushed = self.vmgr.flush_file(file).is_ok();
        let closed = self.vmgr.close_file(file).is_ok();
        let ok = wrote && flushed && closed;
        if !ok {
            // The consequence log lives at the mint site (which knows whether this was a clause-2
            // mint); here just name which step tore.
            defmt::warn!(
                "SD: store-epoch persist failed (write {=bool} flush {=bool} close {=bool})",
                wrote,
                flushed,
                closed
            );
        }
        ok
    }

    /// Overwrite the synced-ride sidecar (truncating), returning whether the whole rewrite reached
    /// the card (finding #876-5). A torn write is safe by design — a ride reads as unsynced next boot
    /// (the safe default: never deleted, and the phone re-acks on reconnect) — so the synced-flag
    /// callers treat it best-effort; the helper logs the failing step.
    fn write_synced_set(&mut self, set: &SyncedRides) -> Result<(), SidecarWriteError> {
        let mut buf = [0u8; SYNCED_RIDES_MAX_LEN];
        let n = encode_synced_rides(set, &mut buf);
        self.rewrite_sidecar(self.tracks_dir, SYNCED_SET, &buf[..n])
    }

    /// Build the stored ride `id`'s recorded-track elevation [`Profile`] — the Ride detail's band
    /// fill (epic #678 T2 / #680), answering
    /// [`App::ride_track_request`](obc_app::App::ride_track_request). Resolves the id
    /// through the scan-parallel [`ride_ids`](Storage::ride_ids)/[`ride_files`](Storage::ride_files)
    /// tables and streams the `RD{id}.ORD` once through the shared `ride_elevation_profile`
    /// (~448 B per SD read, no whole-track buffer — the ~36 KB stack budget's discipline; the
    /// returned `Profile` is the nrf-mem ~3 KB build). `None` = unknown id / unopenable / torn file
    /// — the caller parks the failure so the read isn't ground against every pass.
    pub fn ride_profile_by_id(&mut self, id: u16) -> Option<Profile> {
        let pos = self.ride_ids.iter().position(|&x| x == id)?;
        let name = self.ride_files[pos].clone();
        let dir = self.tracks_dir?;
        let file = match self.vmgr.open_file_in_dir(dir, &name, Mode::ReadOnly) {
            Ok(f) => f,
            Err(_) => {
                defmt::warn!("SD: ride profile: cannot open {} — band stays empty", defmt::Debug2Format(&name));
                return None;
            }
        };
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let profile = ride_elevation_profile(&SdByteSource::new(&self.vmgr, file, len)).ok();
        let _ = self.vmgr.close_file(file);
        profile
    }

    /// Build the stored ride `id`'s decimated recorded-track shape polyline (#678 rework 3) —
    /// the preview half of the Ride detail's track-request answer, `ride_profile_by_id`'s twin:
    /// the same id resolution, one forward streaming pass through the shared
    /// `ride_preview_polyline` (~448 B blocks, no whole-track
    /// buffer, no backward seeks — the #502 FAT lesson). Empty = unknown id / unopenable / torn
    /// file — the detail's track page just leaves its slot blank.
    pub fn ride_preview_by_id(&mut self, id: u16) -> heapless::Vec<(i32, i32), { obc_app::NAV_PREVIEW_MAX }> {
        let Some(pos) = self.ride_ids.iter().position(|&x| x == id) else { return heapless::Vec::new() };
        let name = self.ride_files[pos].clone();
        let Some(dir) = self.tracks_dir else { return heapless::Vec::new() };
        let file = match self.vmgr.open_file_in_dir(dir, &name, Mode::ReadOnly) {
            Ok(f) => f,
            Err(_) => {
                defmt::warn!("SD: ride preview: cannot open {} — track page stays empty", defmt::Debug2Format(&name));
                return heapless::Vec::new();
            }
        };
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let pts = ride_preview_polyline(&SdByteSource::new(&self.vmgr, file, len)).unwrap_or_default();
        let _ = self.vmgr.close_file(file);
        pts
    }

    /// Free space on the SD card in bytes (T8 item 6) — a bounded **FAT free-cluster** read: the
    /// FAT32 FSInfo sector's cached free-cluster count × cluster size. Three single-block CMD17s (the
    /// MBR partition entry, the volume BPB, then FSInfo) — never a full FAT walk, so it's cheap enough
    /// to run on the System screen's on-entry request. Returns `None` unless the card is MBR + FAT32
    /// with a valid FSInfo free count (the screen then keeps `--`).
    pub fn card_free_bytes(&self) -> Option<u64> {
        use embedded_sdmmc::{Block, BlockDevice, BlockIdx};
        let read = |lba: u32, blk: &mut Block| self.card.read(core::slice::from_mut(blk), BlockIdx(lba)).ok();
        let rd_u16 = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let rd_u32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

        let mut blk = Block::new();
        // Sector 0: an MBR (partition 0 start LBA at +8 of the 446-offset entry) or, on a
        // "superfloppy" (a BPB directly at LBA 0), the boot sector itself — detected by the FAT jump
        // + "FAT32" type string, in which case the partition starts at LBA 0.
        read(0, &mut blk)?;
        let superfloppy = (blk.contents[0] == 0xEB || blk.contents[0] == 0xE9) && &blk.contents[82..87] == b"FAT32";
        let part_lba = if superfloppy { 0 } else { rd_u32(&blk.contents, 446 + 8) };

        // The volume BPB.
        read(part_lba, &mut blk)?;
        let bytes_per_sec = rd_u16(&blk.contents, 11) as u64;
        let sec_per_clus = blk.contents[13] as u64;
        let fsinfo_sec = rd_u16(&blk.contents, 48) as u32;
        if bytes_per_sec == 0 || sec_per_clus == 0 || fsinfo_sec == 0 {
            return None;
        }

        // The FSInfo sector — the FAT's cached free-cluster count (lead sig 0x41615252 @0, struct sig
        // 0x61417272 @484). `0xFFFFFFFF` = unknown / uncomputed.
        read(part_lba + fsinfo_sec, &mut blk)?;
        if rd_u32(&blk.contents, 0) != 0x4161_5252 || rd_u32(&blk.contents, 484) != 0x6141_7272 {
            return None;
        }
        let free_clusters = rd_u32(&blk.contents, 488);
        if free_clusters == 0xFFFF_FFFF {
            return None;
        }
        Some(free_clusters as u64 * sec_per_clus * bytes_per_sec)
    }

    /// Reconcile the open ride log to the app's tracking intent — call once per frame *before*
    /// ticking, mirroring the sim's `TrackStore::reconcile`. Drains the one-shot disposition
    /// first (finalising / abandoning the current log), then opens a fresh log when the session
    /// id changes. `name` is the active route's name (the save filename); `stats` is the app's
    /// ride totals + wall-clock anchor, read the same frame — the ride object's header. `marks`
    /// is the RRAM id high-water store (#450), threaded through for the ride-id allocation a
    /// back-to-back begin's early flush may need.
    pub fn reconcile_track(
        &mut self,
        action: Option<obc_app::TrackAction>,
        session: Option<u32>,
        name: &str,
        stats: Option<&RideStats>,
        marks: &mut crate::settings::RramSettingsStore,
    ) {
        use obc_app::TrackAction;
        match action {
            Some(TrackAction::Save) => self.finalize_track(stats),
            Some(TrackAction::Discard) => self.abandon_track(),
            None => {}
        }
        match session {
            Some(id) if self.open_track.as_ref().map(|o| o.session) != Some(id) => self.begin_track(id, name, marks),
            None => self.abandon_track(), // no session → ensure nothing is left open
            _ => {}                       // same session → keep appending
        }
    }

    /// The [`TrackSink`](obc_ports::TrackSink) for the open log, or `None` when not recording.
    pub fn track_sink(&self) -> Option<TrackSinkT<'_>> {
        self.open_track.as_ref().map(|o| SdTrackSink::new(&self.vmgr, o.file))
    }

    /// Open (truncating) a fresh `TRACK.OBT` for session `id`, to be saved as `name`.
    fn begin_track(&mut self, id: u32, name: &str, marks: &mut crate::settings::RramSettingsStore) {
        // A still-deferred previous Finish must convert **before** the truncate below destroys its
        // log — the rare back-to-back case (Finish, then a new ride within the same quiet moment)
        // pays the blocking save up front rather than losing the ride.
        self.run_pending_save(marks);
        self.abandon_track(); // close any previous handle first
        let Some(dir) = self.tracks_dir else { return };
        match self.vmgr.open_file_in_dir(dir, TRACK_TMP, Mode::ReadWriteCreateOrTruncate) {
            Ok(file) => {
                let mut nm = String::new();
                let _ = nm.push_str(name);
                self.open_track = Some(OpenTrack { session: id, file, name: nm });
                defmt::info!("SD: recording ride → {=str}", name);
            }
            Err(e) => defmt::warn!("SD: cannot open ride log: {}", defmt::Debug2Format(&e)),
        }
    }

    /// Finish the ride log: flush + close the temp and **stash** the log → ride-object conversion
    /// as a [`PendingSave`] — deliberately doing no bulk SD work here. The Finish gesture lands
    /// mid-confirm-animation, and the conversion is the longest blocking SD stretch the ride loop
    /// has; the loop runs [`run_pending_save`](Self::run_pending_save) once the glass is quiet.
    /// With no ride totals to head the object (`stats == None` — can't happen on a real Finish)
    /// the temp is kept unconverted rather than writing a headerless object.
    fn finalize_track(&mut self, stats: Option<&RideStats>) {
        let Some(ot) = self.open_track.take() else { return };
        let _ = self.vmgr.flush_file(ot.file);
        let _ = self.vmgr.close_file(ot.file);
        let Some(stats) = stats else {
            defmt::warn!("SD: finish without ride stats — kept TRACK.OBT unconverted");
            return;
        };
        self.pending_save = Some(PendingSave { name: ot.name, stats: *stats });
    }

    /// Whether a finished ride still awaits its deferred conversion — the ride loop keeps its
    /// short wake cadence while this is true, so the save actually runs.
    pub fn has_pending_save(&self) -> bool {
        self.pending_save.is_some()
    }

    /// Run a deferred Finish, if any: one streaming [`track_to_ride`] pass over the closed
    /// `TRACK.OBT` into a confirmed-free `RD{id}.ORD`, deleting the temp **only once the ride is
    /// safely in the object**. Any path that can't guarantee a clean save — no confirmed-free
    /// name, the object won't open, or the conversion errors — keeps `TRACK.OBT` so the ride
    /// isn't lost to a transient SD glitch (a card-pull can still recover it; a fresh ride
    /// truncates it, as before).
    ///
    /// `track_to_ride` holds the version byte back and patches it in as its final write, so a
    /// power cut mid-save leaves a file every reader rejects (and whose id-in-name still reserves
    /// the id — a later ride can't alias it for the app's synced-set).
    ///
    /// This is the ride path's one long blocking SD stretch, which is exactly why it is deferred:
    /// the ride loop calls it only when the hold bulge is quiet, and [`begin_track`](Self::begin_track)
    /// flushes it early if a new ride would otherwise truncate the unconverted temp.
    ///
    /// `marks` is the RRAM id high-water store (#450): the ride id is allocated as
    /// `max(scan_max + 1, stored floor)` and the floor is bumped past it **before** the object is
    /// written — so once device-side ride deletion exists, a deleted id can never be re-issued to
    /// a later ride and alias it in the phone's synced/tombstone sets. A blank/torn floor line
    /// decodes to "no floor" and this degrades to exactly the old scan-max + 1.
    pub fn run_pending_save(&mut self, marks: &mut crate::settings::RramSettingsStore) {
        let Some(ps) = self.pending_save.take() else { return };
        let Some(dir) = self.tracks_dir else { return };
        let Ok(src_file) = self.vmgr.open_file_in_dir(dir, TRACK_TMP, Mode::ReadOnly) else {
            defmt::warn!("SD: pending save: cannot reopen TRACK.OBT — kept for a card-pull");
            return;
        };
        let len = self.vmgr.file_length(src_file).unwrap_or(0);

        let mut m = marks.load_id_marks().unwrap_or_default();
        let id = m.alloc_ride(self.next_ride_id(dir));
        marks.save_id_marks(&m); // one 16-byte RRAM line per ride finish — the durable never-reuse floor
        let saved = match self.fresh_object_name(dir, "RD", id, "ORD") {
            Some(file_name) => match self.vmgr.open_file_in_dir(dir, &file_name, Mode::ReadWriteCreateOrTruncate) {
                Ok(dst_file) => {
                    let source = SdByteSource::new(&self.vmgr, src_file, len);
                    let mut sink = SdByteSink::new(&self.vmgr, dst_file);
                    let ok = match track_to_ride(&source, &ps.name, &ps.stats, &mut sink) {
                        Ok(()) => {
                            defmt::info!("SD: saved ride → tracks/RD{=u16}.ORD", id);
                            true
                        }
                        Err(e) => {
                            defmt::warn!("SD: ride-object write failed: {} — kept TRACK.OBT", defmt::Debug2Format(&e));
                            false
                        }
                    };
                    let _ = self.vmgr.flush_file(dst_file);
                    let _ = self.vmgr.close_file(dst_file);
                    ok
                }
                Err(e) => {
                    defmt::warn!("SD: cannot open ride object: {} — kept TRACK.OBT", defmt::Debug2Format(&e));
                    false
                }
            },
            None => {
                defmt::warn!("SD: ride-object name RD{=u16}.ORD unavailable — kept TRACK.OBT (no overwrite)", id);
                false
            }
        };
        let _ = self.vmgr.close_file(src_file);
        // Drop the temp only after the ride is confirmed written; otherwise keep it.
        if saved {
            let _ = self.vmgr.delete_file_in_dir(dir, TRACK_TMP);
            // Raise the saved-ride flag for the ride loop's per-pass drain (`take_ride_saved`) —
            // the edge that gets the fresh `RD{id}.ORD` into the Rides menu (and the phone's
            // catalog) without a reboot. Set here, at the single commit point, so every caller
            // (deferred run and back-to-back flush alike) raises it.
            self.ride_saved = true;
        }
    }

    /// Drain the saved-ride flag: `true` exactly once after [`run_pending_save`] committed a ride
    /// object. The ride loop checks this once per pass and raises the store edge from it — on `ble`
    /// by posting [`crate::object_store::note_ride_saved`] (the BLE plane re-scans its catalog and
    /// bumps the revision, so the phone's `storeChanged`/digest and the Rides menu learn from the
    /// same edge).
    pub fn take_ride_saved(&mut self) -> bool {
        core::mem::take(&mut self.ride_saved)
    }

    /// One past the highest ride object id stored in `/tracks` (0 on a virgin card) — the **scan
    /// half** of the ride-id allocation. On its own it would resurrect a deleted id; the caller
    /// (`run_pending_save`) maxes it against the persisted RRAM high-water floor (#450), which is
    /// what keeps ids unique across deletes and reboots.
    fn next_ride_id(&self, dir: RawDirectory) -> u16 {
        let mut next = 0u16;
        self.iter_dir_lfn(dir, |e, _| {
            if let Some(id) = stored_ride_id(&e.name) {
                next = next.max(id.saturating_add(1));
            }
        });
        next
    }

    /// Drop the open log without saving (Discard, or a no-session reconcile), deleting the temp.
    fn abandon_track(&mut self) {
        if let Some(ot) = self.open_track.take() {
            let _ = self.vmgr.close_file(ot.file);
            if let Some(dir) = self.tracks_dir {
                let _ = self.vmgr.delete_file_in_dir(dir, TRACK_TMP);
            }
        }
    }

    /// Whether `name` is **confirmed absent** in `dir` — i.e. safe to create without overwriting.
    /// Only `embedded_sdmmc::Error::NotFound` counts as free — the *only* answer that proves a
    /// name is unused. A present entry, or **any other error** (a transient `DeviceError` on the
    /// flaky breadboard link), is treated as "taken", so a glitch can never green-light reusing a
    /// name and truncate an existing object.
    fn name_is_free(&self, dir: RawDirectory, name: &str) -> bool {
        matches!(self.vmgr.find_directory_entry(dir, name), Err(embedded_sdmmc::Error::NotFound))
    }
}

impl Storage {
    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Presence only (a directory scan, no read): the full CRC validation
    /// is the on-device confirm flow's, never a BLE command handler's.
    pub fn has_update_bin(&self) -> bool {
        ShortFileName::create_from_str(UPDATE_BIN).ok().and_then(|n| self.find_root_entry(&n)).is_some()
    }

    /// A confirmed-free `{prefix}{id}.{ext}` 8.3 name for a durable-id ride object file: only a
    /// proven-absent name (see [`name_is_free`](Self::name_is_free)) is handed back, so a squatting
    /// foreign file or an unproven check fails the save rather than risk an overwrite.
    fn fresh_object_name(&self, dir: RawDirectory, prefix: &str, id: u16, ext: &str) -> Option<ShortFileName> {
        let mut s: String<12> = String::new();
        let _ = core::fmt::write(&mut s, format_args!("{prefix}{id}.{ext}"));
        if self.name_is_free(dir, s.as_str()) {
            return ShortFileName::create_from_str(s.as_str()).ok();
        }
        None
    }

    /// Visit every stored ride object in `/tracks` (the `RD{id}.ORD` files) with its filename-encoded
    /// durable id. An in-progress `TRACK.OBT` (or any foreign file) never matches.
    pub fn for_each_ride_file(&self, mut f: impl FnMut(u16, &ShortFileName)) {
        let Some(dir) = self.tracks_dir else { return };
        self.iter_dir_lfn(dir, |e, _| {
            if let Some(id) = stored_ride_id(&e.name) {
                f(id, &e.name);
            }
        });
    }

    /// Whether a stored ride file is an **interrupted save** — the held-back version byte still
    /// zeroed because [`track_to_ride`]'s final patch never ran. Only that exact signature is
    /// sweepable; a merely unreadable file must be kept.
    pub fn is_aborted_ride_object(&self, name: &ShortFileName) -> bool {
        let Some(dir) = self.tracks_dir else { return false };
        let Ok(file) = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly) else {
            return false;
        };
        let mut version = [0xFFu8; 1];
        let zeroed = matches!(self.vmgr.read(file, &mut version), Ok(1)) && version[0] == 0;
        let _ = self.vmgr.close_file(file);
        zeroed
    }

    /// Delete a stored ride object file (the boot sweep of interrupted saves).
    pub fn delete_ride_file(&mut self, name: &ShortFileName) -> bool {
        let Some(dir) = self.tracks_dir else { return false };
        self.vmgr.delete_file_in_dir(dir, name).is_ok()
    }

    /// A stored ride object's byte length + the header facts its `rideList` entry serves. One header
    /// read; `None` when the file doesn't validate as a ride object v1 (incl. an interrupted save's
    /// held-back version byte — see [`track_to_ride`]).
    pub fn ride_object_info(&self, name: &ShortFileName) -> Option<(u32, RideInfo)> {
        let dir = self.tracks_dir?;
        let file = self.vmgr.open_file_in_dir(dir, name, Mode::ReadOnly).ok()?;
        let len = self.vmgr.file_length(file).unwrap_or(0);
        let info = RideInfo::read(&SdByteSource::new(&self.vmgr, file, len)).ok();
        let _ = self.vmgr.close_file(file);
        Some((len, info?))
    }
}

// ==================== The DFU armer plane (epic #615 S4, #619) ====================
//
// The storage half of the app-side armer: locate + validate the staged `UPDATE.BIN` and write
// the `ROLLBACK.BIN` snapshot, both resolved to raw block extents through the same
// a bounded local FAT-chain walk. The *decision logic* — the scan
// matrix, the arm sequencing — is pure and host-tested in `obc_dfu::armer`; these methods are
// its thin `StageIo`/snapshot adapters over FatFs + the raw card. Everything here runs inside
// the ride loop's drained request at shallow per-pass depth, in frames that pop on return —
// its small parsing block and the `StagedRef`s never sit resident.
impl Storage {
    /// Locate an 8.3 `name` in the card root, returning the entry facts the extent build needs:
    /// `(entry_block, entry_offset, byte length)` — the same public `DirEntry` capture as the
    /// map-open scan.
    fn find_root_entry(&self, name: &ShortFileName) -> Option<(embedded_sdmmc::BlockIdx, u32, u32)> {
        let mut found = None;
        self.iter_dir_lfn(self.root, |e, _| {
            if found.is_none() && !e.attributes.is_directory() && e.name == *name {
                found = Some((e.entry_block, e.entry_offset, e.size));
            }
        });
        found
    }

    /// The staging scan (#619 §1, signed in #997): find `UPDATE.BIN` in the card root, decode +
    /// validate its OBCU header, run the **full CRC-32 pass and the Ed25519 verification** over the
    /// image body in one pass through the byte source, gate the size, and resolve the whole-file
    /// extent chain (spec §2.3 — the header is part of the chain). Typed errors surface verbatim to
    /// the debug link now and S5's UI later. Read-only: a failed scan costs nothing.
    ///
    /// The trusted key is [`obc_dfu::RELEASE_PUBKEY`] — the production key compiled into this image
    /// (`firmware/obc-dfu/keys/obcu-release.pub`). This is the *only* place the firmware names it;
    /// `obc_dfu::armer::scan` takes it as a parameter so tests inject their own key without any
    /// build-flag surgery on the shipping path.
    pub fn dfu_scan_update(&mut self) -> Result<obc_dfu::StagedRef, ScanError> {
        let name = ShortFileName::create_from_str(UPDATE_BIN).map_err(|_| ScanError::Io)?;
        let Some((entry_block, entry_offset, len)) = self.find_root_entry(&name) else {
            return Err(ScanError::Missing);
        };
        let file = self.vmgr.open_file_in_dir(self.root, UPDATE_BIN, Mode::ReadOnly).map_err(|_| ScanError::Io)?;
        let mut stage = SdStage { vmgr: &self.vmgr, card: self.card, file, len, entry_block, entry_offset };
        // The CRC/signature staging buffer matches this module's transfer idiom
        // (`copy_with_held_magic`'s 512-byte stack chunk) — no new resident statics; the frame pops
        // with the scan, verifier state (~200 B) included.
        let mut chunk = [0u8; 512];
        let result = obc_dfu::armer::scan(&mut stage, &mut chunk, &obc_dfu::RELEASE_PUBKEY);
        let _ = self.vmgr.close_file(file);
        result
    }

    /// Write the rollback snapshot (#619 §2): `installed`'s raw image — `image`, the caller's
    /// memory-mapped view of the app slot — re-wrapped as a full OBCU container at
    /// `/ROLLBACK.BIN` (truncate-and-reuse, the `TRACK.OBT` idiom), then extent-resolved exactly
    /// like the update file (whole-file chain, spec §2.3).
    ///
    /// `Ok(None)` = the slot's bytes no longer CRC-match the installed header (a dev SWD reflash
    /// since the last install) — a snapshot would record a rollback the bootloader must reject,
    /// so none is taken and any stale `ROLLBACK.BIN` is removed. Errors abort the arm.
    pub fn dfu_write_rollback(
        &mut self,
        installed: &obc_dfu::ImageHeader,
        image: &[u8],
    ) -> Result<Option<obc_dfu::StagedRef>, ScanError> {
        debug_assert_eq!(image.len() as u32, installed.image_len);
        let crc = obc_dfu::crc32(image);
        if crc != installed.image_crc32 {
            defmt::warn!("dfu: running image doesn't match the installed record (SWD reflash?) — no rollback");
            let _ = self.vmgr.delete_file_in_dir(self.root, ROLLBACK_BIN); // don't leave a stale snapshot
            return Ok(None);
        }

        let file = self
            .vmgr
            .open_file_in_dir(self.root, ROLLBACK_BIN, Mode::ReadWriteCreateOrTruncate)
            .map_err(|_| ScanError::Io)?;
        // Header, then the raw image straight from the memory-mapped slot (embedded-sdmmc chunks
        // the long write into blocks itself). Flush before the extent resolve — the chain must
        // be final on card.
        // The snapshot is an **unsigned** container (`ImageHeader::unsigned`): the device cannot
        // re-create the release signature from slot bytes, and nothing verifies this file — it never
        // passes through `armer::scan`, and the bootloader's rollback path checks it by CRC. Writing
        // a signed *marker* with no trailer behind it would be a lie in a file `obc-mkimage inspect`
        // reads. The recorded `StagedRef` below carries the same unsigned header, so the installer's
        // header-equality check still matches the bytes on card.
        let snapshot_header = installed.unsigned();
        let ok = self.vmgr.write(file, &snapshot_header.encode()).is_ok()
            && self.vmgr.write(file, image).is_ok()
            && self.vmgr.flush_file(file).is_ok();
        let _ = self.vmgr.close_file(file);
        if !ok {
            defmt::warn!("dfu: rollback snapshot write failed — arm aborted");
            let _ = self.vmgr.delete_file_in_dir(self.root, ROLLBACK_BIN);
            return Err(ScanError::Io);
        }

        // Resolve the fresh file's chain off its directory entry, exactly like the update file.
        let name = ShortFileName::create_from_str(ROLLBACK_BIN).map_err(|_| ScanError::Io)?;
        let Some((entry_block, entry_offset, len)) = self.find_root_entry(&name) else {
            return Err(ScanError::Io);
        };
        let mut extents = [obc_dfu::Extent::default(); obc_dfu::MAX_EXTENTS];
        let count = resolve_extents(self.card, entry_block, entry_offset, len, &mut extents).map_err(|e| match e {
            ExtentsError::TooFragmented { extents } => ScanError::TooFragmented { extents },
            ExtentsError::Io => ScanError::Io,
        })?;
        defmt::info!(
            "dfu: rollback snapshot written ({=u32} B raw image, {=usize} extent(s))",
            installed.image_len,
            count
        );
        obc_dfu::StagedRef::new(snapshot_header, installed.image_len, crc, &extents[..count])
            .map(Some)
            .ok_or(ScanError::TooFragmented { extents: count as u32 })
    }
}

/// The armer's [`StageIo`] over the open `UPDATE.BIN`: byte reads through the manager's seek
/// path (a scan is one forward pass — the extent-mapped fast path matters for the bootloader's
/// reads, not this one) and the whole-file extent resolve off the raw card.
struct SdStage<'a> {
    vmgr: &'a Vmgr,
    card: &'static Sd,
    file: RawFile,
    len: u32,
    entry_block: embedded_sdmmc::BlockIdx,
    entry_offset: u32,
}

impl StageIo for SdStage<'_> {
    fn stage_len(&mut self) -> Option<u32> {
        Some(self.len)
    }

    fn read_stage(&mut self, offset: u32, buf: &mut [u8]) -> Result<(), obc_dfu::engine::IoError> {
        SdByteSource::new(self.vmgr, self.file, self.len)
            .read_at(offset.into(), buf)
            .map_err(|_| obc_dfu::engine::IoError)
    }

    fn stage_extents(&mut self, out: &mut [obc_dfu::Extent; obc_dfu::MAX_EXTENTS]) -> Result<usize, ExtentsError> {
        resolve_extents(self.card, self.entry_block, self.entry_offset, self.len, out)
    }
}

/// Resolve one legacy FAT staging file into the bootloader's raw-block extents.
///
/// This is intentionally the only FAT-chain walk left after the flat-only map cutover: DFU's boot
/// record needs physical runs, not a reusable random-read source. Runs are written directly into
/// the caller's fixed wire-cap buffer, so there is no resident extent table or storage-crate FAT
/// abstraction to keep alive for an unreachable map fallback.
fn resolve_extents(
    card: &'static Sd,
    entry_block: embedded_sdmmc::BlockIdx,
    entry_offset: u32,
    len: u32,
    out: &mut [obc_dfu::Extent],
) -> Result<usize, ExtentsError> {
    fn read(card: &Sd, block: &mut Block, lba: u32) -> Result<(), ExtentsError> {
        card.read(core::slice::from_mut(block), BlockIdx(lba)).map_err(|_| ExtentsError::Io)
    }
    let mut block = Block::new();
    let u16_at = |bytes: &[u8], at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]);
    let u32_at = |bytes: &[u8], at: usize| u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);

    read(card, &mut block, 0)?;
    if u16_at(&block.contents, 510) != 0xAA55 {
        return Err(ExtentsError::Io);
    }
    let part = &block.contents[446..462];
    if (part[0] & 0x7f) != 0 || !matches!(part[4], 0x01 | 0x04 | 0x06 | 0x0b | 0x0c | 0x0e) {
        return Err(ExtentsError::Io);
    }
    let part_lba = u32_at(part, 8);
    read(card, &mut block, part_lba)?;
    let bpb = &block.contents;
    if u16_at(bpb, 510) != 0xAA55 || u16_at(bpb, 11) != 512 {
        return Err(ExtentsError::Io);
    }
    let spc = u32::from(bpb[13]);
    let reserved = u32::from(u16_at(bpb, 14));
    let fats = u32::from(bpb[16]);
    let root_entries = u32::from(u16_at(bpb, 17));
    let total = match u16_at(bpb, 19) {
        0 => u32_at(bpb, 32),
        n => u32::from(n),
    };
    let fat_size = match u16_at(bpb, 22) {
        0 => u32_at(bpb, 36),
        n => u32::from(n),
    };
    if spc == 0 || fats == 0 || fat_size == 0 {
        return Err(ExtentsError::Io);
    }
    let root_blocks = root_entries.checked_mul(32).ok_or(ExtentsError::Io)?.div_ceil(512);
    let non_data = fats
        .checked_mul(fat_size)
        .and_then(|n| n.checked_add(reserved))
        .and_then(|n| n.checked_add(root_blocks))
        .ok_or(ExtentsError::Io)?;
    let cluster_count = total.checked_sub(non_data).ok_or(ExtentsError::Io)? / spc;
    if cluster_count < 4085 {
        return Err(ExtentsError::Io);
    }
    let fat32 = cluster_count >= 65_525;
    let entries_per_block = if fat32 { 128 } else { 256 };
    let cluster_count =
        cluster_count.min(fat_size.saturating_mul(entries_per_block).saturating_sub(2)).min(0x0fff_fff5);
    let fat_start = part_lba.checked_add(reserved).ok_or(ExtentsError::Io)?;
    let data_start = part_lba.checked_add(non_data).ok_or(ExtentsError::Io)?;

    read(card, &mut block, entry_block.0)?;
    let at = usize::try_from(entry_offset).map_err(|_| ExtentsError::Io)?;
    let entry = block.contents.get(at..at.checked_add(32).ok_or(ExtentsError::Io)?).ok_or(ExtentsError::Io)?;
    if entry[11] == 0x0f || entry[11] & 0x10 != 0 || u32_at(entry, 28) != len {
        return Err(ExtentsError::Io);
    }
    let high = if fat32 { u32::from(u16_at(entry, 20)) } else { 0 };
    let mut cluster = (high << 16) | u32::from(u16_at(entry, 26));
    let needed = len.div_ceil(spc * Block::LEN as u32);
    let mut count = 0usize;
    let mut next_lba = u32::MAX;
    let mut cached_fat_lba = u32::MAX;
    for i in 0..needed {
        if cluster < 2 || cluster >= 2 + cluster_count {
            return Err(ExtentsError::Io);
        }
        let lba = data_start + (cluster - 2) * spc;
        if lba == next_lba {
            if count <= out.len() {
                out[count - 1].blocks += spc;
            }
        } else {
            count += 1;
            if count <= out.len() {
                out[count - 1] = obc_dfu::Extent { start_block: lba, blocks: spc };
            }
        }
        next_lba = lba + spc;
        if i + 1 == needed {
            continue;
        }
        let width = if fat32 { 4 } else { 2 };
        let byte = cluster.checked_mul(width).ok_or(ExtentsError::Io)?;
        let fat_lba = fat_start.checked_add(byte / Block::LEN as u32).ok_or(ExtentsError::Io)?;
        if fat_lba != cached_fat_lba {
            read(card, &mut block, fat_lba)?;
            cached_fat_lba = fat_lba;
        }
        let off = (byte % Block::LEN as u32) as usize;
        cluster =
            if fat32 { u32_at(&block.contents, off) & 0x0fff_ffff } else { u32::from(u16_at(&block.contents, off)) };
    }
    if count > out.len() {
        Err(ExtentsError::TooFragmented { extents: count as u32 })
    } else {
        Ok(count)
    }
}

/// The **durable ride object id** in a stored ride's filename — `RD{id}.ORD` → `id`. The app's
/// synced-set and tombstones key on these ids across device reboots.
pub fn stored_ride_id(name: &ShortFileName) -> Option<u16> {
    id_in_name(name, b"RD", b"ORD")
}

/// Parse `{prefix}{decimal u16}.{ext}` from an 8.3 name; `None` for anything else.
fn id_in_name(name: &ShortFileName, prefix: &[u8], ext: &[u8]) -> Option<u16> {
    if name.extension() != ext {
        return None;
    }
    let digits = name.base_name().strip_prefix(prefix)?;
    if digits.is_empty() {
        return None;
    }
    let mut id: u32 = 0;
    for &b in digits {
        if !b.is_ascii_digit() {
            return None;
        }
        id = id * 10 + (b - b'0') as u32;
        if id > u16::MAX as u32 {
            return None;
        }
    }
    Some(id as u16)
}
