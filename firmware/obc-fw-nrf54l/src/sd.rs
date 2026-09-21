//! Legacy FatFs storage for the nRF54L board's staged firmware update.
//!
//! This owns the transport-to-[`VolumeManager`] stack that the staged updater and the
//! card-resident store epoch still need. Routes, trips and rides are flat-store-only, and FAT
//! remains only for `/EPOCH.OBE`, `/UPDATE.BIN` and `/ROLLBACK.BIN`.
//!
//! The card is not on a SPI bus. The FLPR (VPR00) runs Nordic's sEMMC image and is the SD host
//! controller; [`crate::semmc`] is the M33-side driver, and [`crate::flpr_mux`] decides whether the
//! coprocessor draws the panel or clocks the card. The soft peripheral fixes the wiring:
//!
//! ```text
//!   P2.00 D3   P2.01 CLK   P2.02 D0   P2.03 D2   P2.04 D1   P2.05 CMD
//! ```
//!
//! Reads run 4-bit at 32 MHz and writes at 21.3 MHz, which the card's program time limits. The only
//! part of that this file owns is [`SemmcCard`], a `BlockDevice` over [`crate::semmc::Semmc`];
//! everything above it is transport-agnostic.

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

/// The store-epoch nonce file in the card root: the `u32` id-era name the phone reads before it
/// pairs. It is in the root because the epoch names the whole store, so a card swap transplants the
/// store's identity. It is minted at boot; a missing or torn file reads as "no epoch", and the mint
/// rule then draws a fresh one.
const EPOCH_FILE: &str = "EPOCH.OBE";

/// The staged firmware update in the card root: 8.3-safe, no LFN. The user sideloads it, and the
/// armer only reads it.
const UPDATE_BIN: &str = "UPDATE.BIN";

/// The armer's snapshot of the running image, in the card root beside [`UPDATE_BIN`]: a full OBCU
/// container, truncated and reused per arm. The bootloader flashes it back if a trial boot goes
/// unconfirmed.
const ROLLBACK_BIN: &str = "ROLLBACK.BIN";

/// The concrete SD stack for this board: the card in native 4-bit mode on the FLPR, under a
/// [`VolumeManager`].
type Sd = SemmcCard;
/// What the legacy manager owns: the card by shared reference, which leaves its raw handle
/// available to the DFU extent resolver.
type SdShared = SharedBlockDevice<'static, Sd>;
const SD_MAX_DIRS: usize = 4;
const SD_MAX_FILES: usize = 16;
const SD_MAX_VOLUMES: usize = 1;
type Vmgr = VolumeManager<SdShared, NullTime, SD_MAX_DIRS, SD_MAX_FILES, SD_MAX_VOLUMES>;

/// FAT timestamps need a clock; the device has none, so every file gets the epoch.
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

/// Bring the card up, and mount nothing on it: boot the sEMMC soft peripheral and identify the
/// card (4-bit, High Speed, 32 MHz reads). Boot hands the raw blocks to the flat store, and a card
/// without a valid flat superblock is rejected; no filesystem fallback follows.
///
/// Card identification is the slow part, and the ACMD41 power-up poll is bounded at 1.5 s.
/// [`flpr_mux::bring_up_storage`](crate::flpr_mux::bring_up_storage) holds the FLPR in storage mode
/// across the whole of it.
///
/// Synchronous on purpose. As an `async fn`, this chain's coroutine flattens into `main`'s
/// task-body poll frame, whose slot set is allocated on entry on every poll for the life of the
/// program: 6,912 to 13,376 B for a function that runs once at boot. `#[inline(never)]` does not
/// fix it, because it governs the future constructor and not the coroutine body.
///
/// It is safe to block here: bring-up runs before the app loop, the BLE stack and the USB plane
/// exist, and the panel's anti-DC-bias COM wave preempts thread mode rather than competing with
/// it.
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

/// Which fault screen a failed bring-up earns. The driver can tell three classes apart, and the
/// rider needs a line they can act on.
///
/// [`SemmcError::NoCard`] is the only one that means what "NO SD CARD" says.
/// [`SemmcError::UnsupportedCard`] is a working card that is SDSC, which this firmware rejects;
/// "NO SD CARD" would send its owner hunting for a card that is already inserted. Everything else
/// is the storage subsystem itself, and is no evidence about whether a card is present.
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

/// The card as an `embedded_sdmmc::BlockDevice`.
///
/// A zero-sized handle: the driver state is the one [`Semmc`](crate::semmc::Semmc) in
/// [`crate::flpr_mux`], which also decides whether the FLPR draws the panel or clocks the card.
/// Every method here is one `flpr_mux::with_storage` call, so exactly one place can get the
/// ordering wrong. The calls block, because `BlockDevice` is a synchronous trait.
#[derive(Clone, Copy)]
pub(crate) struct SemmcCard;

const BLOCK_LEN: usize = Block::LEN;

/// `Block` is `#[repr(transparent)]` over `[u8; 512]`, which is what makes the byte views below
/// sound.
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
                        for (b, src) in chunk.iter_mut().zip(bounce[..len].as_chunks::<BLOCK_LEN>().0) {
                            b.contents.copy_from_slice(src);
                        }
                    }
                    Ok(())
                })
            }
        });
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
                        for (b, dst) in chunk.iter().zip(bounce[..len].as_chunks_mut::<BLOCK_LEN>().0) {
                            dst.copy_from_slice(&b.contents);
                        }
                        sd.write_blocks(start_block_idx.0 + (i * bounce_blocks) as u32, &bounce[..len])?;
                    }
                    Ok(())
                })
            }
        });
        if let Err(e) = r {
            log_transfer_error("write", start_block_idx.0, n, e);
        }
        r
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        crate::flpr_mux::with_storage(|sd| sd.num_blocks()).map(BlockCount)
    }
}

impl Storage {
    /// Iterate `dir`'s entries with their long filenames, running `f` per entry. The iteration
    /// error is ignored: a partial scan still yields what it read.
    fn iter_dir_lfn(&self, dir: RawDirectory, mut f: impl FnMut(&embedded_sdmmc::DirEntry, Option<&str>)) {
        let mut lfn_storage = [0u8; 256];
        let mut lfn = LfnBuffer::new(&mut lfn_storage);
        let _ = self.vmgr.iterate_dir_lfn(dir, &mut lfn, |e, long| f(e, long));
    }

    /// Read the card-resident store-epoch nonce, or `None` when the file is absent, torn or
    /// foreign. The boot mint rule treats `None` as "draw a fresh nonce".
    pub fn load_card_epoch(&self) -> Option<u32> {
        let Ok(file) = self.vmgr.open_file_in_dir(self.root, EPOCH_FILE, Mode::ReadOnly) else {
            return None; // absent = no epoch (the mint pass draws a fresh one)
        };
        let mut buf = [0u8; STORE_EPOCH_LEN];
        let n = self.vmgr.read(file, &mut buf).unwrap_or(0);
        let _ = self.vmgr.close_file(file);
        decode_store_epoch(&buf[..n])
    }

    /// Overwrite the store-epoch file with `epoch`. Returns `true` only when open, write, flush
    /// and close all succeeded, because a discarded flush or close error is a torn persist and the
    /// mint pass gates the id-marks write and the served epoch on this result. The whole persist is
    /// inside the call, so it never holds a file handle across an `await`.
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
            // The consequence log lives at the mint site, which knows what kind of mint this was.
            // Here, name the step that tore.
            defmt::warn!(
                "SD: store-epoch persist failed (write {=bool} flush {=bool} close {=bool})",
                wrote,
                flushed,
                closed
            );
        }
        ok
    }

    /// Free space on the card in bytes: a bounded FAT free-cluster read of the FAT32 FSInfo
    /// sector's cached count, times the cluster size. Three single-block CMD17s and never a full
    /// FAT walk. `None` unless the card is MBR plus FAT32 with a valid FSInfo free count.
    pub fn card_free_bytes(&self) -> Option<u64> {
        use embedded_sdmmc::{Block, BlockDevice, BlockIdx};
        let read = |lba: u32, blk: &mut Block| self.card.read(core::slice::from_mut(blk), BlockIdx(lba)).ok();
        let rd_u16 = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
        let rd_u32 = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);

        let mut blk = Block::new();
        // Sector 0 is an MBR (partition 0's start LBA at +8 of the entry at 446) or, on a
        // superfloppy with a BPB directly at LBA 0, the boot sector itself. The FAT jump and the
        // "FAT32" type string tell them apart; a superfloppy's partition starts at LBA 0.
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

        // The FSInfo sector holds the FAT's cached free-cluster count. `0xFFFFFFFF` is unknown.
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
    /// Whether a staged `/UPDATE.BIN` exists in the card root: presence only, through a directory
    /// scan. The full CRC validation belongs to the on-device confirm flow.
    pub fn has_update_bin(&self) -> bool {
        ShortFileName::create_from_str(UPDATE_BIN).ok().and_then(|n| self.find_root_entry(&n)).is_some()
    }
}

// The storage half of the app-side armer: locate and validate the staged `UPDATE.BIN`, and write
// the `ROLLBACK.BIN` snapshot, both resolved to raw block extents through a bounded FAT-chain
// walk. The decision logic is pure and host-tested in `obc_dfu::armer`; these methods are its thin
// `StageIo` and snapshot adapters over FatFs and the raw card.
impl Storage {
    /// Locate an 8.3 `name` in the card root, returning the facts the extent build needs:
    /// `(entry_block, entry_offset, byte length)`.
    fn find_root_entry(&self, name: &ShortFileName) -> Option<(embedded_sdmmc::BlockIdx, u32, u32)> {
        let mut found = None;
        self.iter_dir_lfn(self.root, |e, _| {
            if found.is_none() && !e.attributes.is_directory() && e.name == *name {
                found = Some((e.entry_block, e.entry_offset, e.size));
            }
        });
        found
    }

    /// The staging scan: find `UPDATE.BIN` in the card root, decode and validate its OBCU header,
    /// run the full CRC-32 pass and the Ed25519 verification over the image body in one pass through
    /// the byte source, gate the size, and resolve the whole-file extent chain, header included.
    /// Read-only, so a failed scan costs nothing.
    ///
    /// The trusted key is [`obc_dfu::RELEASE_PUBKEY`], and this is the only place the firmware names
    /// it. `obc_dfu::armer::scan` takes the key as a parameter, so tests inject their own.
    pub fn dfu_scan_update(&mut self) -> Result<obc_dfu::StagedRef, ScanError> {
        let name = ShortFileName::create_from_str(UPDATE_BIN).map_err(|_| ScanError::Io)?;
        let Some((entry_block, entry_offset, len)) = self.find_root_entry(&name) else {
            return Err(ScanError::Missing);
        };
        let file = self.vmgr.open_file_in_dir(self.root, UPDATE_BIN, Mode::ReadOnly).map_err(|_| ScanError::Io)?;
        let mut stage = SdStage { vmgr: &self.vmgr, card: self.card, file, len, entry_block, entry_offset };
        // The CRC and signature staging buffer is a stack chunk: no new resident statics, and the
        // frame pops with the scan, verifier state included.
        let mut chunk = [0u8; 512];
        let result = obc_dfu::armer::scan(&mut stage, &mut chunk, &obc_dfu::RELEASE_PUBKEY);
        let _ = self.vmgr.close_file(file);
        result
    }

    /// Write the rollback snapshot: `installed`'s raw image, re-wrapped as a full OBCU container at
    /// `/ROLLBACK.BIN`, then extent-resolved like the update file.
    ///
    /// `Ok(None)` means the slot's bytes no longer CRC-match the installed header, so a snapshot
    /// would record a rollback the bootloader must reject. None is taken, and any stale
    /// `ROLLBACK.BIN` is removed. Errors abort the arm.
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
        // Header, then the raw image straight from the memory-mapped slot. Flush before the extent
        // resolve: the chain must be final on card.
        //
        // The snapshot is an unsigned container. The device cannot re-create the release signature
        // from slot bytes, and nothing verifies this file: the bootloader's rollback path checks it
        // by CRC. A signed marker with no trailer behind it would be a lie in a file
        // `obc-mkimage inspect` reads.
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

/// The armer's [`StageIo`] over the open `UPDATE.BIN`: byte reads through the manager's seek path,
/// because a scan is one forward pass, and the whole-file extent resolve off the raw card.
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
/// This is the only FAT-chain walk left: DFU's boot record needs physical runs, not a reusable
/// random-read source. Runs go straight into the caller's fixed buffer, so no extent table stays
/// resident.
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
