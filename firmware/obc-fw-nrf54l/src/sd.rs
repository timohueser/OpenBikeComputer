//! Legacy FatFs storage for the nRF54L board's card-resident store epoch.
//!
//! This owns the transport-to-[`VolumeManager`] stack that the store epoch and the free-space read
//! still need. Everything else is flat-store-only, and FAT remains only for `/EPOCH.OBE`.
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
    Block, BlockCount, BlockDevice, BlockIdx, Mode, RawDirectory, TimeSource, Timestamp, VolumeManager,
};
use obc_app::store_meta::{decode_store_epoch, encode_store_epoch, STORE_EPOCH_LEN};
use obc_storage::shared_device::SharedBlockDevice;

/// The store-epoch nonce file in the card root: the `u32` id-era name the phone reads before it
/// pairs. It is in the root because the epoch names the whole store, so a card swap transplants the
/// store's identity. It is minted at boot; a missing or torn file reads as "no epoch", and the mint
/// rule then draws a fresh one.
const EPOCH_FILE: &str = "EPOCH.OBE";

/// The concrete SD stack for this board: the card in native 4-bit mode on the FLPR, under a
/// [`VolumeManager`].
type Sd = SemmcCard;
/// What the legacy manager owns: the card by shared reference, which leaves its raw handle
/// available to the free-space read.
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

/// The mounted legacy FAT card retained for the store epoch and the free-space read.
pub struct Storage {
    vmgr: Vmgr,
    /// The raw card the manager's [`SdShared`] borrows — the free-space read's direct handle.
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
                        for (b, dst) in chunk.iter().zip(bounce[..len].as_chunks_mut::<BLOCK_LEN>().0) {
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
