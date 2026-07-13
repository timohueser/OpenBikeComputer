//! Extent-mapped **direct block reads** for one big read-only FatFs file — the #500 fix.
//!
//! The map `.obcm` is read with scattered `read_at`s (the A* nav planner and the render loop's
//! chunk cache both seek all over it), and `embedded-sdmmc`'s seek is O(offset): FAT is a
//! singly-linked list, every *backward* seek restarts from the file's first cluster, and each
//! `next_cluster` step goes through a **one-block** cache that the data reads also evict. On the
//! measured card that was ~109 FAT-sector reads per 2 KB nav chunk — 41k block reads for a 2 km
//! plan, ~100% of a 56 s route computation (issue #500).
//!
//! A FAT file's location on disk is static while it's open read-only, so the chain only ever
//! needs to be resolved **once**: [`ExtentTable::build`] walks the FAT a single time at map-open
//! and compresses the chain into runs of contiguous clusters. A file that was copied onto the
//! card in one go — the only way a map gets there — is one run, or a handful. Every later
//! [`read_at`](ExtentSource::read_at) is then pure arithmetic plus the data-block reads
//! themselves: **zero** FAT traffic, no `VolumeManager` seek state.
//!
//! Everything here uses only `embedded-sdmmc`'s public API — no fork: the board hands its
//! `VolumeManager` a [`SharedBlockDevice`] and keeps the raw `&SdCard` twin for this module (the
//! manager's own `device()` accessor can't return a `Result` in 0.9, so the share happens one
//! level up), and the file is located via the public
//! [`DirEntry::entry_block`]/[`entry_offset`](embedded_sdmmc::DirEntry::entry_offset) (this
//! module re-reads the 32-byte on-disk entry to get the first cluster, which `ClusterId` hides).
//! The volume geometry (partition start, FAT/data offsets) is re-derived from the MBR + BPB with
//! the same rules `open_raw_volume`/`parse_volume` use, so the two views can't disagree on a card
//! the manager successfully mounted. The caller should still verify the table against the normal
//! read path once at build time (read a block through both, compare) and fall back on any
//! mismatch — a geometry bug must degrade to *slow*, never to *wrong bytes*.
//!
//! Bounded RAM by design (the alternative — caching the whole chain — was measured, rejected,
//! and reverted in #500): [`MAX_EXTENTS`] runs. A file fragmented past the cap fails the build
//! with [`BuildError::TooFragmented`] and the caller keeps the plain [`SdByteSource`] path; the
//! fix for such a card is re-copying the map onto it (fresh FAT allocation is contiguous).

use core::cell::RefCell;

use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx};
use obc_route::{ByteSource, Error};

/// A [`BlockDevice`] by shared reference — what lets one card serve **both** the `VolumeManager`
/// (which takes its device by value) and this module's raw extent reads. The board parks its
/// `SdCard` in a `.bss` slot, hands the manager `SharedBlockDevice(&card)`, and keeps the same
/// `&card` for [`ExtentTable::build`]/[`ExtentSource`]. Interleaved (never re-entrant) access is
/// safe: `BlockDevice`'s methods take `&self`, and the single storage owner serialises calls.
pub struct SharedBlockDevice<'a, D: BlockDevice>(pub &'a D);

impl<D: BlockDevice> BlockDevice for SharedBlockDevice<'_, D> {
    type Error = D::Error;
    fn read(&self, blocks: &mut [Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.0.read(blocks, start_block_idx)
    }
    fn write(&self, blocks: &[Block], start_block_idx: BlockIdx) -> Result<(), Self::Error> {
        self.0.write(blocks, start_block_idx)
    }
    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        self.0.num_blocks()
    }
}

/// Extent-run budget. A file written in one streaming copy is 1 run, but real cards accumulate
/// churn: the #500 reference card's map measured **46 extents** (the first cap tried, 32, was
/// refused by the actual hardware). 128 runs = 1.5 KB — bounded and small against the nav/tile
/// budgets, with ~3× margin over the measured card; past it the build refuses (fall back to the
/// seek path, with the true count in the log) rather than growing.
pub const MAX_EXTENTS: usize = 128;

/// One run of file-contiguous, disk-contiguous 512-byte blocks.
#[derive(Clone, Copy, Debug)]
struct Run {
    /// First block of the run, in **file** space (byte offset / 512, cluster-aligned).
    file_block: u32,
    /// Absolute LBA of that block on the card.
    lba: u32,
    /// Run length, blocks.
    blocks: u32,
}

/// Why a build was refused. Every variant means "keep the plain seek path", they differ only in
/// what the caller logs: `TooFragmented` is the expected/actionable one (re-copy the map file);
/// the rest indicate a card this module's FAT view can't safely describe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuildError {
    /// A raw block read failed.
    Io,
    /// MBR/BPB/dir-entry contents outside what this module understands (no MBR partition 0, a
    /// non-512-byte-sector or FAT12 volume, a corrupt directory entry…).
    Geometry,
    /// The FAT chain didn't cover the file's byte length (truncated chain, reserved/bad cluster
    /// id mid-chain, or a dir-entry size disagreeing with the open handle's length).
    Mismatch,
    /// More than [`MAX_EXTENTS`] runs — the bounded table refuses rather than growing. Carries
    /// the file's **true** extent count (the walk finishes for the count even once storage is
    /// full), so the refusal log states exactly how fragmented the file is.
    TooFragmented(u32),
}

/// The volume facts needed to turn a cluster id into an absolute LBA and to index the FAT —
/// parsed from the MBR + BPB with `parse_volume`'s own rules.
struct Geometry {
    /// Absolute LBA of the first FAT sector.
    fat_start: u32,
    /// Absolute LBA of cluster 2 (the first data cluster).
    data_start: u32,
    /// Blocks per cluster.
    spc: u32,
    /// 4-byte FAT entries (FAT32) vs 2-byte (FAT16).
    fat32: bool,
    /// Number of data clusters — valid chain ids are `2 .. 2 + cluster_count`.
    cluster_count: u32,
}

/// The resolved file: its extent runs plus a resident one-block bounce buffer for the unaligned
/// head/tail of a read. The bounce lives *here* — sized once, resident with the table — rather
/// than on the read path's stack: `read_at` is reached from the deepest render frames, where the
/// tight ride-stack budget has no spare 512 bytes (see the board crate's stack notes). For the
/// same reason the board keeps the whole table in a `.bss` slot and never moves it by value.
pub struct ExtentTable {
    runs: heapless::Vec<Run, MAX_EXTENTS>,
    len: u32,
    bounce: RefCell<Block>,
}

impl ExtentTable {
    /// Resolve the file's FAT chain into an extent table, reading raw blocks off `dev` (the
    /// shared twin of the manager's [`SharedBlockDevice`]). `entry_block`/`entry_offset` locate
    /// the file's 32-byte directory entry (absolute, from the public
    /// [`DirEntry`](embedded_sdmmc::DirEntry) fields); `expected_len` is the open handle's byte
    /// length and must match the entry's.
    pub fn build<D: BlockDevice>(
        dev: &D,
        entry_block: BlockIdx,
        entry_offset: u32,
        expected_len: u32,
    ) -> Result<Self, BuildError> {
        let mut block = Block::new();

        // ── Volume geometry: MBR partition 0 → BPB, exactly `open_raw_volume`'s rules ──
        read_block(dev, 0, &mut block)?;
        if read_u16(&block.contents, 510) != 0xAA55 {
            return Err(BuildError::Geometry);
        }
        let part = &block.contents[446..462];
        // Only 0x80/0x00 status and the FAT partition types the manager itself mounts.
        if (part[0] & 0x7F) != 0 || !matches!(part[4], 0x01 | 0x04 | 0x06 | 0x0B | 0x0C | 0x0E) {
            return Err(BuildError::Geometry);
        }
        let part_lba = read_u32(part, 8);

        read_block(dev, part_lba, &mut block)?;
        let bpb = &block.contents;
        if read_u16(bpb, 510) != 0xAA55 || read_u16(bpb, 11) as usize != 512 {
            return Err(BuildError::Geometry);
        }
        let spc = bpb[13] as u32;
        let reserved = read_u16(bpb, 14) as u32;
        let num_fats = bpb[16] as u32;
        let root_entries = read_u16(bpb, 17) as u32;
        let total_blocks = match read_u16(bpb, 19) {
            0 => read_u32(bpb, 32),
            n => n as u32,
        };
        let fat_size = match read_u16(bpb, 22) {
            0 => read_u32(bpb, 36),
            n => n as u32,
        };
        if spc == 0 || fat_size == 0 {
            return Err(BuildError::Geometry);
        }
        // FAT type is decided by cluster count (the BPB's own rule — mirrors `Bpb::create_from_bytes`).
        let root_dir_blocks = (root_entries * 32).div_ceil(512);
        let non_data = reserved + num_fats * fat_size + root_dir_blocks;
        let cluster_count = total_blocks.checked_sub(non_data).ok_or(BuildError::Geometry)? / spc;
        if cluster_count < 4085 {
            return Err(BuildError::Geometry); // FAT12 — unsupported, like the manager itself
        }
        let geo = Geometry {
            fat_start: part_lba + reserved,
            data_start: part_lba + non_data,
            spc,
            fat32: cluster_count >= 65525,
            cluster_count,
        };

        // ── The file's first cluster, from its raw 32-byte directory entry ──
        read_block(dev, entry_block.0, &mut block)?;
        let off = entry_offset as usize;
        let entry = block.contents.get(off..off + 32).ok_or(BuildError::Geometry)?;
        if entry[11] == 0x0F || entry[11] & 0x10 != 0 {
            return Err(BuildError::Geometry); // an LFN fragment or a directory, not a file entry
        }
        if read_u32(entry, 28) != expected_len {
            return Err(BuildError::Mismatch);
        }
        let hi = if geo.fat32 { read_u16(entry, 20) as u32 } else { 0 };
        let first_cluster = (hi << 16) | read_u16(entry, 26) as u32;

        // ── One walk of the chain, compressed into runs ──
        let bytes_per_cluster = geo.spc * 512;
        let clusters_needed = expected_len.div_ceil(bytes_per_cluster);
        let mut runs: heapless::Vec<Run, MAX_EXTENTS> = heapless::Vec::new();
        let mut file_block = 0u32;
        // Run bookkeeping independent of storage, so an overflowing walk still finishes and
        // reports the file's *true* extent count — the actionable number in the refusal (and
        // #500's fragmentation measurement even when the table can't be kept).
        let mut run_count = 0u32;
        let mut next_lba = u32::MAX; // where the current run would continue; MAX = no run yet
                                     // The FAT sector under the walk cursor — one resident block, like the manager's own
                                     // cache, but nothing else contends for it mid-walk, so a contiguous chain reads each FAT
                                     // sector exactly once.
        let mut cached_fat_lba = u32::MAX;
        let mut cluster = first_cluster;
        for i in 0..clusters_needed {
            if cluster < 2 || cluster >= 2 + geo.cluster_count {
                // EOC/bad/free mid-chain: the chain is shorter than the file claims.
                return Err(BuildError::Mismatch);
            }
            let lba = geo.data_start + (cluster - 2) * geo.spc;
            if lba == next_lba {
                // Continues the current run — extend the stored twin only while storage still
                // tracks the walk (past the cap the last stored run is an *earlier* run).
                if run_count as usize == runs.len() {
                    if let Some(r) = runs.last_mut() {
                        r.blocks += geo.spc;
                    }
                }
            } else {
                run_count += 1;
                // Past the cap the walk keeps going for the count alone; the stored runs are
                // dead either way (the build returns `TooFragmented` below).
                let _ = runs.push(Run { file_block, lba, blocks: geo.spc });
            }
            next_lba = lba + geo.spc;
            file_block += geo.spc;
            if i + 1 < clusters_needed {
                let (fat_lba, ent_off) = if geo.fat32 {
                    (geo.fat_start + cluster * 4 / 512, (cluster * 4 % 512) as usize)
                } else {
                    (geo.fat_start + cluster * 2 / 512, (cluster * 2 % 512) as usize)
                };
                if fat_lba != cached_fat_lba {
                    read_block(dev, fat_lba, &mut block)?;
                    cached_fat_lba = fat_lba;
                }
                cluster = if geo.fat32 {
                    read_u32(&block.contents, ent_off) & 0x0FFF_FFFF
                } else {
                    read_u16(&block.contents, ent_off) as u32
                };
            }
        }
        if run_count as usize > MAX_EXTENTS {
            return Err(BuildError::TooFragmented(run_count));
        }
        Ok(ExtentTable { runs, len: expected_len, bounce: RefCell::new(Block::new()) })
    }

    /// How many extent runs the file resolved to — 1 = fully contiguous. The number #500's open
    /// fragmentation question asked for; the board logs it at map-open.
    pub fn extent_count(&self) -> usize {
        self.runs.len()
    }

    /// The file's byte length the table was built for.
    pub fn len(&self) -> u32 {
        self.len
    }

    /// The resolved runs as `(absolute start LBA, blocks)`, in file order — what the DFU armer
    /// (S4, #619) copies into the boot-state page's `StagedRef` extents (`OBCU_Spec.md` §2.3):
    /// the bootloader replays exactly these block runs with no FAT of its own.
    pub fn runs(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.runs.iter().map(|r| (r.lba, r.blocks))
    }

    /// Whether the table is empty (a zero-length file — never built in practice).
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Absolute LBA of file block `file_block`, or `None` past the mapped extents.
    fn lba_of(&self, file_block: u32) -> Option<u32> {
        // Runs are sorted by construction; binary-search the covering run.
        let i = self.runs.partition_point(|r| r.file_block <= file_block).checked_sub(1)?;
        let r = &self.runs[i];
        (file_block - r.file_block < r.blocks).then(|| r.lba + (file_block - r.file_block))
    }
}

/// A [`ByteSource`] serving `read_at` straight off the card through an [`ExtentTable`] — the
/// fast twin of [`SdByteSource`](crate::SdByteSource), same construction pattern (borrow, rebuild
/// per use, hold no seek state).
pub struct ExtentSource<'a, D: BlockDevice> {
    dev: &'a D,
    table: &'a ExtentTable,
}

impl<'a, D: BlockDevice> ExtentSource<'a, D> {
    /// A source over `table`, reading raw blocks off `dev`. The caller keeps the underlying file
    /// open for the table's lifetime (an open handle is what pins the chain).
    pub fn new(dev: &'a D, table: &'a ExtentTable) -> Self {
        ExtentSource { dev, table }
    }
}

impl<D: BlockDevice> ByteSource for ExtentSource<'_, D> {
    // `inline(never)`: called from the deepest render/nav frames — keep this body's locals out
    // of them permanently, whatever the inliner decides later (deep-frame discipline; on-glass
    // stack peaks have moved with inlining before). Measured free: one out-of-line call per
    // multi-ms SD read.
    #[inline(never)]
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        let end = offset.checked_add(buf.len() as u32).ok_or(Error::BadOffset)?;
        if end > self.table.len {
            return Err(Error::BadOffset);
        }
        // Block-at-a-time through the table's resident bounce buffer (see its field doc for why
        // it isn't a stack local here). Single-block CMD17s already cut the measured per-chunk
        // cost ~30× (the FAT walk was the cost, not the data blocks); batching contiguous spans
        // into one CMD18 is a further ~2× left on the table if a read path ever needs it.
        let mut bounce = self.table.bounce.borrow_mut();
        let mut off = offset;
        let mut done = 0usize;
        while done < buf.len() {
            let lba = self.table.lba_of(off / 512).ok_or(Error::BadOffset)?;
            self.dev.read(core::slice::from_mut(&mut *bounce), BlockIdx(lba)).map_err(|_| Error::Io)?;
            let in_block = (off % 512) as usize;
            let n = (512 - in_block).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&bounce.contents[in_block..in_block + n]);
            done += n;
            off += n as u32;
        }
        Ok(())
    }

    fn len(&self) -> u32 {
        self.table.len
    }
}

/// One raw block read off the shared device.
fn read_block<D: BlockDevice>(dev: &D, lba: u32, block: &mut Block) -> Result<(), BuildError> {
    dev.read(core::slice::from_mut(block), BlockIdx(lba)).map_err(|_| BuildError::Io)
}

fn read_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

// The tests hand-build minimal-but-valid empty FAT16/FAT32 images in RAM, then create the actual
// files through `embedded-sdmmc` itself — so the manager's own mount + write path vouches for the
// image, and every extent read is differential-tested against the manager's seek+read of the same
// bytes. Fragmentation is produced the honest way: two files appended in alternation, so the FAT
// allocator interleaves their clusters.
#[cfg(test)]
mod tests {
    extern crate std;

    use std::boxed::Box;
    use std::vec;
    use std::vec::Vec;

    use embedded_sdmmc::{Mode, TimeSource, Timestamp, VolumeIdx, VolumeManager};

    use super::*;

    struct RamDisk(RefCell<Vec<u8>>);

    impl BlockDevice for RamDisk {
        type Error = ();
        fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), ()> {
            let data = self.0.borrow();
            for (i, b) in blocks.iter_mut().enumerate() {
                let off = (start.0 as usize + i) * 512;
                b.contents.copy_from_slice(&data[off..off + 512]);
            }
            Ok(())
        }
        fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), ()> {
            let mut data = self.0.borrow_mut();
            for (i, b) in blocks.iter().enumerate() {
                let off = (start.0 as usize + i) * 512;
                data[off..off + 512].copy_from_slice(&b.contents);
            }
            Ok(())
        }
        fn num_blocks(&self) -> Result<embedded_sdmmc::BlockCount, ()> {
            Ok(embedded_sdmmc::BlockCount((self.0.borrow().len() / 512) as u32))
        }
    }

    struct Epoch;
    impl TimeSource for Epoch {
        fn get_timestamp(&self) -> Timestamp {
            Timestamp {
                year_since_1970: 0,
                zero_indexed_month: 0,
                zero_indexed_day: 0,
                hours: 0,
                minutes: 0,
                seconds: 0,
            }
        }
    }

    const PART_START: u32 = 64;

    fn put_u16(img: &mut [u8], off: usize, v: u16) {
        img[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put_u32(img: &mut [u8], off: usize, v: u32) {
        img[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }

    /// An empty FAT32 volume: 1-block clusters (so single-block appends fragment maximally),
    /// 65,600 clusters (the FAT32 floor is 65,525), one FAT.
    fn mkfs_fat32() -> RamDisk {
        let (reserved, fat_size, clusters) = (2u32, 513u32, 65_600u32);
        let total = reserved + fat_size + clusters;
        let mut img = vec![0u8; ((PART_START + total) * 512) as usize];
        // MBR: partition 0 = FAT32-LBA at PART_START.
        img[446] = 0x00;
        img[446 + 4] = 0x0C;
        put_u32(&mut img, 446 + 8, PART_START);
        put_u32(&mut img, 446 + 12, total);
        put_u16(&mut img, 510, 0xAA55);
        // BPB.
        let b = (PART_START * 512) as usize;
        put_u16(&mut img, b + 11, 512);
        img[b + 13] = 1; // blocks per cluster
        put_u16(&mut img, b + 14, reserved as u16);
        img[b + 16] = 1; // one FAT
        img[b + 21] = 0xF8;
        put_u32(&mut img, b + 32, total);
        put_u32(&mut img, b + 36, fat_size);
        put_u32(&mut img, b + 44, 2); // root dir = cluster 2
        put_u16(&mut img, b + 48, 1); // FsInfo block
        put_u16(&mut img, b + 510, 0xAA55);
        // FsInfo (free count/next free left "unknown").
        let i = b + 512;
        put_u32(&mut img, i, 0x4161_5252);
        put_u32(&mut img, i + 484, 0x6141_7272);
        put_u32(&mut img, i + 488, 0xFFFF_FFFF);
        put_u32(&mut img, i + 492, 0xFFFF_FFFF);
        put_u16(&mut img, i + 510, 0xAA55);
        // FAT: media/EOC reserved entries + the root dir's single-cluster chain.
        let f = ((PART_START + reserved) * 512) as usize;
        put_u32(&mut img, f, 0x0FFF_FFF8);
        put_u32(&mut img, f + 4, 0x0FFF_FFFF);
        put_u32(&mut img, f + 8, 0x0FFF_FFFF);
        RamDisk(RefCell::new(img))
    }

    /// An empty FAT16 volume: 5,000 1-block clusters (within FAT16's [4085, 65525) window), a
    /// 32-block fixed root directory region.
    fn mkfs_fat16() -> RamDisk {
        let (reserved, fat_size, root_blocks, clusters) = (1u32, 20u32, 32u32, 5_000u32);
        let total = reserved + fat_size + root_blocks + clusters;
        let mut img = vec![0u8; ((PART_START + total) * 512) as usize];
        img[446] = 0x00;
        img[446 + 4] = 0x06;
        put_u32(&mut img, 446 + 8, PART_START);
        put_u32(&mut img, 446 + 12, total);
        put_u16(&mut img, 510, 0xAA55);
        let b = (PART_START * 512) as usize;
        put_u16(&mut img, b + 11, 512);
        img[b + 13] = 1;
        put_u16(&mut img, b + 14, reserved as u16);
        img[b + 16] = 1;
        put_u16(&mut img, b + 17, (root_blocks * 512 / 32) as u16);
        put_u16(&mut img, b + 19, total as u16);
        img[b + 21] = 0xF8;
        put_u16(&mut img, b + 22, fat_size as u16);
        put_u16(&mut img, b + 510, 0xAA55);
        let f = ((PART_START + reserved) * 512) as usize;
        put_u16(&mut img, f, 0xFFF8);
        put_u16(&mut img, f + 2, 0xFFFF);
        RamDisk(RefCell::new(img))
    }

    /// The manager over the shared-reference device — the same split the board uses, so the
    /// tests exercise interleaved manager + raw access on one card.
    type TestMgr = VolumeManager<SharedBlockDevice<'static, RamDisk>, Epoch, 4, 4, 1>;

    /// A recognisable position-dependent byte pattern, so any block-mapping slip shows up as a
    /// content mismatch (not just a length one).
    fn pattern(file_tag: u8, off: usize, len: usize) -> Vec<u8> {
        (off..off + len).map(|i| (i as u8) ^ file_tag).collect()
    }

    /// A mounted test volume: the raw disk (the board's `&SdCard` twin), the manager over its
    /// shared reference, and the open root — held for the whole test (the manager refuses a
    /// second `open_raw_volume`, so every helper works off this one handle).
    struct Fs {
        disk: &'static RamDisk,
        vmgr: TestMgr,
        root: embedded_sdmmc::RawDirectory,
    }

    /// Mount `disk`, create `names` in the root, and append `appends` × 512 bytes to each in
    /// round-robin order — alternation is what makes the FAT allocator interleave (fragment)
    /// their clusters; a single name allocates contiguously.
    fn setup(disk: RamDisk, names: &[&str], appends: usize) -> Fs {
        let disk: &'static RamDisk = Box::leak(Box::new(disk));
        let vmgr: TestMgr = VolumeManager::new_with_limits(SharedBlockDevice(disk), Epoch, 5000);
        let volume = vmgr.open_raw_volume(VolumeIdx(0)).unwrap();
        let root = vmgr.open_root_dir(volume).unwrap();
        let files: Vec<_> =
            names.iter().map(|n| vmgr.open_file_in_dir(root, *n, Mode::ReadWriteCreate).unwrap()).collect();
        for round in 0..appends {
            for (tag, f) in files.iter().enumerate() {
                vmgr.write(*f, &pattern(tag as u8, round * 512, 512)).unwrap();
            }
        }
        for f in files {
            vmgr.close_file(f).unwrap();
        }
        Fs { disk, vmgr, root }
    }

    impl Fs {
        /// `name`'s `(entry_block, entry_offset, byte length)` from a root-dir iteration — the
        /// same public facts the board's map-open scan captures.
        fn entry_facts(&self, name: &str) -> (BlockIdx, u32, u32) {
            let want = embedded_sdmmc::ShortFileName::create_from_str(name).unwrap();
            let mut found = None;
            self.vmgr
                .iterate_dir(self.root, |e| {
                    if e.name == want {
                        found = Some((e.entry_block, e.entry_offset, e.size));
                    }
                })
                .unwrap();
            found.expect("file not found in root")
        }
    }

    #[test]
    fn contiguous_file_is_one_extent_and_reads_match() {
        run_matrix(mkfs_fat32(), &["MAP.BIN"], 40, 1);
    }

    #[test]
    fn fragmented_file_reads_match() {
        // Two interleaved files: each ends up with `appends` single-cluster extents.
        run_matrix(mkfs_fat32(), &["MAP.BIN", "OTHER.BIN"], 20, 20);
    }

    #[test]
    fn fat16_fragmented_reads_match() {
        run_matrix(mkfs_fat16(), &["MAP.BIN", "OTHER.BIN"], 12, 12);
    }

    #[test]
    fn over_fragmented_build_is_refused_with_the_true_count() {
        let fs = setup(mkfs_fat32(), &["MAP.BIN", "OTHER.BIN"], MAX_EXTENTS + 3);
        let (eb, eo, len) = fs.entry_facts("MAP.BIN");
        assert_eq!(
            ExtentTable::build(fs.disk, eb, eo, len).err().expect("build should refuse"),
            BuildError::TooFragmented((MAX_EXTENTS + 3) as u32),
            "the refusal reports the file's true extent count, not just 'past the cap'"
        );
    }

    #[test]
    fn wrong_length_is_refused_and_eof_is_bad_offset() {
        let fs = setup(mkfs_fat32(), &["MAP.BIN"], 4);
        let (eb, eo, len) = fs.entry_facts("MAP.BIN");
        assert_eq!(
            ExtentTable::build(fs.disk, eb, eo, len + 1).err().expect("build should refuse"),
            BuildError::Mismatch
        );

        let table = ExtentTable::build(fs.disk, eb, eo, len).unwrap();
        let src = ExtentSource::new(fs.disk, &table);
        let mut buf = [0u8; 8];
        assert_eq!(src.read_at(len - 4, &mut buf).unwrap_err(), Error::BadOffset);
    }

    /// The shared body: build the image, create the files, build MAP.BIN's table, assert the
    /// extent count, then differential-test `read_at` against the manager's own seek+read on a
    /// window matrix (aligned, unaligned, cross-block, cross-extent, whole-file).
    fn run_matrix(disk: RamDisk, names: &[&str], appends: usize, want_extents: usize) {
        let fs = setup(disk, names, appends);
        let (eb, eo, len) = fs.entry_facts("MAP.BIN");
        assert_eq!(len as usize, appends * 512);

        let table = ExtentTable::build(fs.disk, eb, eo, len).unwrap();
        assert_eq!(table.extent_count(), want_extents);

        let src = ExtentSource::new(fs.disk, &table);
        let file = fs.vmgr.open_file_in_dir(fs.root, "MAP.BIN", Mode::ReadOnly).unwrap();
        let windows: &[(u32, usize)] =
            &[(0, 512), (0, len as usize), (1, 511), (7, 1300), (509, 8), (512, 512), (len - 700, 700), (len - 1, 1)];
        for &(off, n) in windows {
            let mut got = vec![0u8; n];
            src.read_at(off, &mut got).unwrap();
            // Against the manager's own read of the same window…
            let mut want = vec![0u8; n];
            fs.vmgr.file_seek_from_start(file, off).unwrap();
            let mut done = 0;
            while done < n {
                done += fs.vmgr.read(file, &mut want[done..]).unwrap();
            }
            assert_eq!(got, want, "window ({off}, {n}) diverged from the manager's read");
            // …and against the ground-truth pattern (tag 0 = MAP.BIN).
            assert_eq!(got, pattern(0, off as usize, n), "window ({off}, {n}) diverged from the pattern");
        }
        fs.vmgr.close_file(file).unwrap();
    }
}
