//! A host-only sparse block device carrying a real FAT volume, so the §13.1 adapter is tested
//! against the filesystem the board actually runs.
//!
//! The faulting harness in [`media`](super::media) models a *sector-addressed medium* and
//! deliberately says nothing about a FAT adapter — its own module documentation names clean flush,
//! chain-longer-than-length and the absent primitives as the three obligations it cannot prove.
//! This is the other half: a genuine `embedded_sdmmc` volume, mounted by the vendored fork, over a
//! device that records every sector it is asked to write.
//!
//! The disk is sparse — a `BTreeMap` of written blocks over an implicit sea of zeros — because the
//! smallest volume with enough clusters to be FAT32 is a gigabyte, and none of the tests care about
//! more than a few megabytes of it. A read of a block nobody wrote returns zeros, exactly as a
//! freshly formatted card does.
//!
//! The image builder produces the layout §1.1 names as conforming: an SD-Association-style 4 MiB
//! partition alignment with 16 KiB clusters. Its parameters exist so a test can also build the
//! *non*-conforming variants and watch the geometry probe refuse them.

use std::collections::BTreeMap;
use std::vec::Vec;

use core::cell::RefCell;

use embedded_sdmmc::{Block, BlockCount, BlockDevice, BlockIdx, TimeSource, Timestamp};

/// A block device over a sparse image: every block that was never written reads as zeros.
pub struct SparseDisk {
    blocks: RefCell<BTreeMap<u32, [u8; 512]>>,
    num_blocks: u32,
}

impl SparseDisk {
    /// An all-zero disk of `num_blocks` 512-byte sectors.
    pub fn blank(num_blocks: u32) -> Self {
        SparseDisk { blocks: RefCell::new(BTreeMap::new()), num_blocks }
    }

    /// Overwrites one sector.
    pub fn put(&self, lba: u32, bytes: [u8; 512]) {
        self.blocks.borrow_mut().insert(lba, bytes);
    }

    /// Reads one sector.
    pub fn get(&self, lba: u32) -> [u8; 512] {
        self.blocks.borrow().get(&lba).copied().unwrap_or([0u8; 512])
    }

    /// How many sectors have ever been written — the sparse image's occupancy.
    pub fn resident_blocks(&self) -> usize {
        self.blocks.borrow().len()
    }
}

impl BlockDevice for SparseDisk {
    type Error = core::convert::Infallible;

    fn read(&self, blocks: &mut [Block], start: BlockIdx) -> Result<(), Self::Error> {
        let image = self.blocks.borrow();
        for (offset, block) in blocks.iter_mut().enumerate() {
            let lba = start.0 + offset as u32;
            block.contents = image.get(&lba).copied().unwrap_or([0u8; 512]);
        }
        Ok(())
    }

    fn write(&self, blocks: &[Block], start: BlockIdx) -> Result<(), Self::Error> {
        let mut image = self.blocks.borrow_mut();
        for (offset, block) in blocks.iter().enumerate() {
            image.insert(start.0 + offset as u32, block.contents);
        }
        Ok(())
    }

    fn num_blocks(&self) -> Result<BlockCount, Self::Error> {
        Ok(BlockCount(self.num_blocks))
    }
}

/// The `TimeSource` the board uses: a zero timestamp. Its only effect here is that the fork stamps
/// the directory entry's mtime with it — which is precisely the byte that makes an entry rewrite
/// visible even when nothing else about the file changed.
pub struct NullTime;

impl TimeSource for NullTime {
    fn get_timestamp(&self) -> Timestamp {
        Timestamp { year_since_1970: 0, zero_indexed_month: 0, zero_indexed_day: 0, hours: 0, minutes: 0, seconds: 0 }
    }
}

/// How to lay a FAT32 volume out.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// The partition's first LBA. 8,192 is the SD Association's 4 MiB alignment.
    pub partition_start_lba: u32,
    /// Sectors per cluster. 32 is a 16 KiB cluster; 64 is 32 KiB.
    pub sectors_per_cluster: u8,
    /// The reserved region, FSInfo included. 32 is what every FAT32 formatter uses.
    pub reserved_sectors: u16,
    /// How many clusters the data region holds. 65,525 is the FAT32 minimum.
    pub clusters: u32,
}

impl Default for Layout {
    fn default() -> Self {
        Layout { partition_start_lba: 8_192, sectors_per_cluster: 32, reserved_sectors: 32, clusters: 65_525 }
    }
}

impl Layout {
    /// Sectors one FAT copy occupies, from the cluster count.
    pub fn fat_sectors(&self) -> u32 {
        (self.clusters + 2).div_ceil(128)
    }

    /// The partition's length in sectors.
    pub fn total_sectors(&self) -> u32 {
        u32::from(self.reserved_sectors) + 2 * self.fat_sectors() + self.clusters * u32::from(self.sectors_per_cluster)
    }

    /// The first data-region sector, physically.
    pub fn data_start_lba(&self) -> u32 {
        self.partition_start_lba + u32::from(self.reserved_sectors) + 2 * self.fat_sectors()
    }
}

/// Builds a mountable FAT32 card: MBR, volume boot record, FSInfo, both FATs and an empty root.
pub fn fat32_card(layout: Layout) -> SparseDisk {
    let total = layout.total_sectors();
    let disk = SparseDisk::blank(layout.partition_start_lba + total);

    let mut mbr = [0u8; 512];
    let at = 446;
    mbr[at] = 0x00; // non-bootable
    mbr[at + 4] = 0x0C; // FAT32 LBA
    mbr[at + 8..at + 12].copy_from_slice(&layout.partition_start_lba.to_le_bytes());
    mbr[at + 12..at + 16].copy_from_slice(&total.to_le_bytes());
    mbr[510..512].copy_from_slice(&0xAA55u16.to_le_bytes());
    disk.put(0, mbr);

    let fat_sectors = layout.fat_sectors();
    let mut bpb = [0u8; 512];
    bpb[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bpb[3..11].copy_from_slice(b"MSDOS5.0");
    bpb[11..13].copy_from_slice(&512u16.to_le_bytes());
    bpb[13] = layout.sectors_per_cluster;
    bpb[14..16].copy_from_slice(&layout.reserved_sectors.to_le_bytes());
    bpb[16] = 2; // two FATs
    bpb[21] = 0xF8;
    bpb[28..32].copy_from_slice(&layout.partition_start_lba.to_le_bytes());
    bpb[32..36].copy_from_slice(&total.to_le_bytes());
    bpb[36..40].copy_from_slice(&fat_sectors.to_le_bytes());
    bpb[44..48].copy_from_slice(&2u32.to_le_bytes()); // root directory cluster
    bpb[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
    bpb[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
    bpb[64] = 0x80;
    bpb[66] = 0x29;
    bpb[71..82].copy_from_slice(b"OBC2 SIM   ");
    bpb[82..90].copy_from_slice(b"FAT32   ");
    bpb[510..512].copy_from_slice(&0xAA55u16.to_le_bytes());
    disk.put(layout.partition_start_lba, bpb);

    let mut fsinfo = [0u8; 512];
    fsinfo[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    fsinfo[488..492].copy_from_slice(&(layout.clusters - 1).to_le_bytes()); // free clusters
    fsinfo[492..496].copy_from_slice(&3u32.to_le_bytes()); // next free
    fsinfo[508..512].copy_from_slice(&0xAA55_0000u32.to_le_bytes());
    disk.put(layout.partition_start_lba + 1, fsinfo);

    // FAT[0] media descriptor, FAT[1] end marker, FAT[2] the root directory's one-cluster chain.
    let mut fat0 = [0u8; 512];
    fat0[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    fat0[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    fat0[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    let fat_start = layout.partition_start_lba + u32::from(layout.reserved_sectors);
    disk.put(fat_start, fat0);
    disk.put(fat_start + fat_sectors, fat0);

    disk
}

/// The two sectors a §1.1 geometry probe reads, as a host test needs them.
pub fn geometry_sectors(disk: &SparseDisk, partition_start_lba: u32) -> ([u8; 512], [u8; 512]) {
    (disk.get(0), disk.get(partition_start_lba))
}

/// Every LBA a recorded span covers, flattened — the shape a per-sector assertion wants.
pub fn touched(spans: &[super::blocklog::Span]) -> Vec<u32> {
    let mut out = Vec::new();
    for span in spans {
        for offset in 0..span.blocks {
            out.push(span.start + offset);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}
