//! Legacy FatFs storage for the nRF54L board's staged firmware update.
//!
//! This owns the concrete transport → [`VolumeManager`] stack retained for the staged updater and
//! the card-resident store epoch. Routes, trips, and rides are flat-store-only.
//!
//! The `Storage` implementation and every adapter below speak `embedded_sdmmc`'s `BlockDevice` /
//! `TimeSource` seams. FAT remains only for `/EPOCH.OBE`, `/UPDATE.BIN`, and `/ROLLBACK.BIN` until
//! the updater moves to the flat store.
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
//! [`crate::semmc::Semmc`]. Everything above it — [`Storage`], the FAT layer, the updater extent
//! resolver, and the boot-fault rule — is transport-agnostic and did not move.

#[cfg(feature = "sd-bench")]
use embassy_time::Instant;
use embedded_sdmmc::{
    Block, BlockCount, BlockDevice, BlockIdx, LfnBuffer, Mode, RawDirectory, RawFile, ShortFileName, TimeSource,
    Timestamp, VolumeManager,
};
use obc_app::store_meta::{decode_store_epoch, encode_store_epoch, STORE_EPOCH_LEN};
use obc_dfu::armer::{ExtentsError, ScanError, StageIo};
use obc_formats::io::ByteSource;
use obc_storage::shared_device::SharedBlockDevice;
use obc_storage::SdByteSource;

/// The store-epoch nonce file in the **card root** (protocol v2 #632 item 5; card-resident #776):
/// the `u32` id-era name the phone reads over the pre-pairing `protocolVersion` read. Kept in the
/// card **root** because the epoch names the *whole* store — so the SD card is the sole home of the
/// id-era name: a card swap
/// transplants the store's identity (swap back restores the old era, a card written by a *different*
/// device presents *its* epoch — its own scope, closing the foreign-card hole the retired RRAM line
/// left open). Minted/rewritten only at boot by the mint pass; a missing/torn file reads as "no
/// epoch" → the mint rule draws a fresh one. Codec + torn-line semantics live in `obc-app::settings`
/// (host-tested).
const EPOCH_FILE: &str = "EPOCH.OBE";

/// The staged firmware update in the **card root** (epic #615, locked: 8.3-safe, no LFN — the
/// same file contract the future LM20 USB-MSC epic exposes). Sideloaded by the user (or, S6, the
/// phone); the armer only ever reads it.
const UPDATE_BIN: &str = "UPDATE.BIN";

/// The armer's snapshot of the **running** image (epic #615 S4, #619), in the card root next to
/// [`UPDATE_BIN`]: a full OBCU container (64-byte header + raw image read straight out of RRAM),
/// truncated-and-reused per arm. The bootloader flashes it back if a trial boot goes unconfirmed.
const ROLLBACK_BIN: &str = "ROLLBACK.BIN";

/// The concrete SD stack for this board: [`SemmcCard`] — the card in native 4-bit mode on the FLPR
/// — under a 16-file/4-dir [`VolumeManager`].
///
/// The manager keeps the existing measured handle budget until this FAT stack is retired.
type Sd = SemmcCard;
/// What the retained legacy manager owns: the card by shared reference, leaving its raw handle
/// available to the DFU extent resolver.
type SdShared = SharedBlockDevice<'static, Sd>;
/// The open-handle budget (see the file-count note above) — one set of consts so the manager and
/// the `obc-platform` wrapper aliases below can never drift apart.
const SD_MAX_DIRS: usize = 4;
/// This is deliberately not resized while the FAT stack awaits deletion in FS11 (#1393): its 640 B
/// delta from the former six-handle budget is already included in the resource baseline.
///
/// The cost is measured, not guessed: the fork's `FileInfo` (`filesystem/files.rs`) is `RawFile`
/// 4 · `RawVolume` 4 · `current_cluster` 8 · `current_offset` 4 · `Mode` 1 · `DirEntry` 40 ·
/// `dirty` 1, i.e. **64 B** at `align 4` on thumbv8m. `6 → 16` is ten slots, **+640 B of `.bss`**
/// — the manager's `open_files` array is a `heapless::Vec<FileInfo, SD_MAX_FILES>` and nothing
/// else scales with it. Nothing on the stack changes.
const SD_MAX_FILES: usize = 16;
const SD_MAX_VOLUMES: usize = 1;
type Vmgr = VolumeManager<SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;

/// FAT timestamps need a clock; the device has none yet, so every file gets the epoch.
/// `pub(crate)` only because it surfaces in the adapter return types the loop names.
pub(crate) struct NullTime;
impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// The mounted legacy FAT card retained for the staged updater, store epoch, and free-space read.
pub struct Storage {
    vmgr: Vmgr,
    /// The raw card the manager's [`SdShared`] borrows — the extent path's direct read handle.
    card: &'static Sd,
    root: RawDirectory,
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
/// It is safe to block here: bring-up runs before the app loop, the BLE stack and the USB plane
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
    /// so updater scans don't repeat it. The iteration error is ignored — a partial scan still
    /// yields what it read, the same as the bare call did.
    fn iter_dir_lfn(&self, dir: RawDirectory, mut f: impl FnMut(&embedded_sdmmc::DirEntry, Option<&str>)) {
        let mut lfn_storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut lfn_storage);
        let _ = self.vmgr.iterate_dir_lfn(dir, &mut lfn, |e, long| f(e, long));
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
    /// (a swallowed epoch-write failure would let a clause-2 mint go permanently undetected: old
    /// valid epoch on card + freshly-written valid floor = steady state next boot — the exact aliasing the epoch
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
}

impl Storage {
    /// Whether a staged `/UPDATE.BIN` exists in the card root — the `installFw` `noStaged` cheap
    /// existence check (spec §4.4). Presence only (a directory scan, no read): the full CRC validation
    /// is the on-device confirm flow's, never a BLE command handler's.
    pub fn has_update_bin(&self) -> bool {
        ShortFileName::create_from_str(UPDATE_BIN).ok().and_then(|n| self.find_root_entry(&n)).is_some()
    }
}

// ==================== The DFU armer plane (epic #615 S4, #619) ====================
//
// The storage half of the app-side armer: locate + validate the staged `UPDATE.BIN` and write
// the `ROLLBACK.BIN` snapshot, both resolved to raw block extents through the same
// a bounded local FAT-chain walk. The *decision logic* — the scan
// matrix, the arm sequencing — is pure and host-tested in `obc_dfu::armer`; these methods are
// its thin `StageIo`/snapshot adapters over FatFs + the raw card. Everything here runs inside
// the app loop's drained request at shallow per-pass depth, in frames that pop on return —
// its small parsing block and the `StagedRef`s never sit resident.
impl Storage {
    /// Locate an 8.3 `name` in the card root, returning the entry facts the extent build needs:
    /// `(entry_block, entry_offset, byte length)` — the same public `DirEntry` capture as the
    /// root-directory scan.
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
    /// `/ROLLBACK.BIN` (truncate-and-reuse), then extent-resolved exactly
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
/// This is intentionally the only FAT-chain walk left: DFU's boot record needs physical runs, not
/// a reusable random-read source. Runs are written directly into the caller's fixed wire-cap
/// buffer, so there is no resident extent table or broader filesystem abstraction to keep alive.
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
