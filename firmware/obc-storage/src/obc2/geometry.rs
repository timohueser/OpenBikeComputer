//! The §1.1 volume geometry preconditions, decided before anything on the card is trusted.
//!
//! `OBC2_Storage_Format.md` §1.1 turns a slot stride into a physical program page with exactly two
//! normative facts about how the card was prepared:
//!
//! 1. the cluster size — `bytes_per_sector × sectors_per_cluster` — is a whole multiple of the
//!    16,384-byte program page;
//! 2. the first byte of the FAT data region is 16,384-aligned relative to the card's physical
//!    LBA 0, which is computable from the partition entry and the BPB alone.
//!
//! Under them every 16,384-aligned file offset is physically page-aligned, which is the whole
//! isolation argument the gated-slot layout rests on. A volume that fails either one mounts
//! **unsupported filesystem** — a class distinct from an unrecognised filesystem type — and nothing
//! is written to it. This module is the decision, not the mount: it parses the two sectors a caller
//! read and classifies them, so the same code runs on the board and in a host test.
//!
//! It deliberately does not use `embedded_sdmmc`'s parser. §1.1's check is over the *physical* LBA,
//! and the adapter's volume type exposes neither the partition start nor the reserved-region
//! arithmetic; reading 512 bytes twice and doing the arithmetic here is both smaller and exactly
//! what the spec writes down.

use super::limits::PROGRAM_PAGE;

/// The MBR/BPB boot signature, at offset 510 of both sectors.
const BOOT_SIGNATURE: u16 = 0xAA55;
/// The first partition entry.
const PARTITION_TABLE: usize = 446;
/// One MBR partition entry.
const PARTITION_ENTRY_LEN: usize = 16;
/// The largest volume §1.1 admits.
const MAX_VOLUME_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;

/// The partition types §1.1 admits: FAT16 and FAT32 in their CHS and LBA spellings.
pub const ADMITTED_PARTITION_TYPES: [u8; 5] = [0x04, 0x06, 0x0B, 0x0C, 0x0E];

/// Why a volume is not one OBC2 can host.
///
/// Every variant is the **unsupported filesystem** mount class of §12's table (value `1`). They are
/// distinguished only so a diagnostic can say which precondition the card failed; none of them is a
/// repair instruction, because the device never formats a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// LBA 0 carries no MBR signature: a partitionless superfloppy or a GPT disk.
    NotPartitioned,
    /// The partition entry is empty (type `0x00`).
    NoSuchPartition,
    /// The entry's status byte is neither bootable nor non-bootable.
    BadPartitionStatus(u8),
    /// The partition type is outside [`ADMITTED_PARTITION_TYPES`] — `0x07` (exFAT/NTFS) included.
    PartitionType(u8),
    /// The volume boot record carries no signature.
    NotFat,
    /// exFAT: the same partition type as NTFS, told apart by the BPB's `EXFAT   ` marker.
    ExFat,
    /// A sector size other than 512. The adapter is 512-only and so is this format.
    BytesPerSector(u16),
    /// A cluster of zero sectors, or one that is not a power of two.
    SectorsPerCluster(u8),
    /// Neither one nor two FATs.
    FatCount(u8),
    /// The BPB's own arithmetic does not close: a region larger than the volume that holds it.
    Inconsistent,
    /// Too few clusters to be FAT16 — FAT12, which this format does not admit.
    Fat12,
    /// §1.1 precondition 1: the cluster is not a whole number of program pages.
    ClusterNotWholePages(u32),
    /// §1.1 precondition 2: the data region does not begin at a physical multiple of the page.
    DataRegionMisaligned(u64),
    /// A volume larger than 2 TiB.
    VolumeTooLarge(u64),
}

/// Which FAT the volume is, decided the way the FAT specification decides it: by cluster count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatType {
    /// 4,085..65,525 clusters.
    Fat16,
    /// 65,525 clusters or more.
    Fat32,
}

/// One MBR partition entry, as read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    /// The zero-based entry index in the table.
    pub index: usize,
    /// The entry's type byte.
    pub kind: u8,
    /// The partition's first physical LBA.
    pub start_lba: u32,
    /// The partition's length in sectors.
    pub sectors: u32,
}

/// Everything §1.1's two preconditions are computed from, plus the answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VolumeGeometry {
    /// The partition this volume lives in.
    pub partition: Partition,
    /// FAT16 or FAT32, by cluster count.
    pub fat_type: FatType,
    /// `BPB_BytsPerSec`.
    pub bytes_per_sector: u16,
    /// `BPB_SecPerClus`.
    pub sectors_per_cluster: u8,
    /// `BPB_RsvdSecCnt`.
    pub reserved_sectors: u16,
    /// `BPB_NumFATs`.
    pub fat_count: u8,
    /// `BPB_FATSz16` or `BPB_FATSz32`, whichever this volume uses.
    pub fat_size_sectors: u32,
    /// The FAT16 fixed root-directory region, zero on FAT32.
    pub root_dir_sectors: u32,
    /// `BPB_TotSec16` or `BPB_TotSec32`, whichever this volume uses.
    pub total_sectors: u32,
    /// Clusters in the data region.
    pub cluster_count: u32,
    /// `bytes_per_sector × sectors_per_cluster`.
    pub cluster_bytes: u32,
    /// The first data-region sector, as a **physical** LBA.
    pub data_start_lba: u32,
    /// The first data-region byte, physically. §1.1's precondition 2 is a fact about this number.
    pub data_start_byte: u64,
    /// The FAT32 FSInfo sector as a physical LBA, when the volume has one.
    pub fs_info_lba: Option<u32>,
    /// The first FAT16 root-directory sector, physically. `None` on FAT32, where the root is an
    /// ordinary cluster chain in the data region.
    pub root_dir_lba: Option<u32>,
}

/// Which structure of the volume a physical LBA belongs to.
///
/// §1.1 divides the volume into exactly the structures whose loss it classifies: "the FAT boot
/// sector, the FSInfo sector, and directory sectors are single-copy structures outside this model;
/// only the FAT itself is mirrored". A write log is only evidence about the clean-flush obligation
/// once each written LBA is named, which is what this does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    /// Ahead of the partition — the MBR and the alignment gap behind it.
    BeforeVolume,
    /// The volume boot record, the FSInfo sector and the rest of the reserved region.
    Reserved,
    /// The FSInfo sector specifically: single-copy, and §13.1 forbids rewriting it on every sync.
    FsInfo,
    /// One of the FATs, by zero-based copy index.
    Fat(u8),
    /// The FAT16 fixed root-directory region.
    RootDir,
    /// The data region: file contents and, on FAT32, every directory.
    Data,
    /// Past the end of the partition.
    BeyondVolume,
}

impl VolumeGeometry {
    /// §1.1 precondition 1.
    ///
    /// The spec adds "so it is exactly 16,384 or 32,768 bytes", which is what the multiple works out
    /// to at the cluster sizes FAT actually uses; the normative statement is the multiple, and that
    /// is what this checks.
    pub fn cluster_is_whole_pages(&self) -> bool {
        self.cluster_bytes != 0 && (self.cluster_bytes as usize).is_multiple_of(PROGRAM_PAGE)
    }

    /// §1.1 precondition 2.
    pub fn data_region_is_page_aligned(&self) -> bool {
        self.data_start_byte.is_multiple_of(PROGRAM_PAGE as u64)
    }

    /// Names the structure a physical LBA belongs to.
    pub fn region(&self, lba: u32) -> Region {
        let start = self.partition.start_lba;
        if lba < start {
            return Region::BeforeVolume;
        }
        if lba >= start.saturating_add(self.total_sectors) {
            return Region::BeyondVolume;
        }
        if Some(lba) == self.fs_info_lba {
            return Region::FsInfo;
        }
        let reserved_end = start + u32::from(self.reserved_sectors);
        if lba < reserved_end {
            return Region::Reserved;
        }
        for copy in 0..self.fat_count {
            let fat_start = reserved_end + u32::from(copy) * self.fat_size_sectors;
            if lba >= fat_start && lba - fat_start < self.fat_size_sectors {
                return Region::Fat(copy);
            }
        }
        if lba < self.data_start_lba {
            return Region::RootDir;
        }
        Region::Data
    }

    /// The volume's size in bytes, from the partition entry.
    pub fn volume_bytes(&self) -> u64 {
        u64::from(self.partition.sectors) * u64::from(self.bytes_per_sector)
    }

    /// Both §1.1 preconditions plus the remaining volume preconditions, in the order §1.1 states
    /// them. `Ok` is the only thing that admits a write to this card.
    pub fn admit(&self) -> Result<(), Unsupported> {
        if !self.cluster_is_whole_pages() {
            return Err(Unsupported::ClusterNotWholePages(self.cluster_bytes));
        }
        if !self.data_region_is_page_aligned() {
            return Err(Unsupported::DataRegionMisaligned(self.data_start_byte));
        }
        if self.volume_bytes() > MAX_VOLUME_BYTES {
            return Err(Unsupported::VolumeTooLarge(self.volume_bytes()));
        }
        Ok(())
    }
}

/// Reads partition entry `index` out of an MBR sector.
pub fn partition(mbr: &[u8; 512], index: usize) -> Result<Partition, Unsupported> {
    if u16_le(mbr, 510) != BOOT_SIGNATURE {
        return Err(Unsupported::NotPartitioned);
    }
    if index >= 4 {
        return Err(Unsupported::NoSuchPartition);
    }
    let at = PARTITION_TABLE + index * PARTITION_ENTRY_LEN;
    let status = mbr[at];
    if status & 0x7F != 0 {
        return Err(Unsupported::BadPartitionStatus(status));
    }
    let kind = mbr[at + 4];
    if kind == 0x00 {
        return Err(Unsupported::NoSuchPartition);
    }
    if !ADMITTED_PARTITION_TYPES.contains(&kind) {
        return Err(Unsupported::PartitionType(kind));
    }
    Ok(Partition { index, kind, start_lba: u32_le(mbr, at + 8), sectors: u32_le(mbr, at + 12) })
}

/// Computes the geometry from a partition entry and that partition's volume boot record.
///
/// The caller reads `bpb` from `partition.start_lba`. Structural failures are reported here;
/// [`VolumeGeometry::admit`] decides the two preconditions separately so a diagnostic can print the
/// geometry of a card it is about to refuse.
pub fn geometry(partition: Partition, bpb: &[u8; 512]) -> Result<VolumeGeometry, Unsupported> {
    if u16_le(bpb, 510) != BOOT_SIGNATURE {
        return Err(Unsupported::NotFat);
    }
    if &bpb[3..11] == b"EXFAT   " {
        return Err(Unsupported::ExFat);
    }
    let bytes_per_sector = u16_le(bpb, 11);
    if bytes_per_sector != 512 {
        return Err(Unsupported::BytesPerSector(bytes_per_sector));
    }
    let sectors_per_cluster = bpb[13];
    if sectors_per_cluster == 0 || !sectors_per_cluster.is_power_of_two() {
        return Err(Unsupported::SectorsPerCluster(sectors_per_cluster));
    }
    let fat_count = bpb[16];
    if fat_count != 1 && fat_count != 2 {
        return Err(Unsupported::FatCount(fat_count));
    }
    let reserved_sectors = u16_le(bpb, 14);
    if reserved_sectors == 0 {
        return Err(Unsupported::Inconsistent);
    }
    let root_entries = u16_le(bpb, 17);
    let root_dir_sectors = (u32::from(root_entries) * 32).div_ceil(u32::from(bytes_per_sector));
    let fat_size_16 = u32::from(u16_le(bpb, 22));
    let fat_size_sectors = if fat_size_16 != 0 { fat_size_16 } else { u32_le(bpb, 36) };
    let total_16 = u32::from(u16_le(bpb, 19));
    let total_sectors = if total_16 != 0 { total_16 } else { u32_le(bpb, 32) };

    let non_data = u32::from(reserved_sectors)
        .checked_add(u32::from(fat_count).checked_mul(fat_size_sectors).ok_or(Unsupported::Inconsistent)?)
        .and_then(|sectors| sectors.checked_add(root_dir_sectors))
        .ok_or(Unsupported::Inconsistent)?;
    if non_data == 0 || total_sectors <= non_data {
        return Err(Unsupported::Inconsistent);
    }
    let cluster_count = (total_sectors - non_data) / u32::from(sectors_per_cluster);
    let fat_type = if cluster_count < 4_085 {
        return Err(Unsupported::Fat12);
    } else if cluster_count < 65_525 {
        FatType::Fat16
    } else {
        FatType::Fat32
    };
    // FAT32 has no fixed root-directory region; a nonzero root-entry count there is a malformed BPB
    // rather than a region, and `non_data` above would have counted sectors that do not exist.
    if fat_type == FatType::Fat32 && root_dir_sectors != 0 {
        return Err(Unsupported::Inconsistent);
    }

    let data_start_lba = partition.start_lba.checked_add(non_data).ok_or(Unsupported::Inconsistent)?;
    let fs_info_lba = match fat_type {
        FatType::Fat32 => match u16_le(bpb, 48) {
            0 | 0xFFFF => None,
            sector => Some(partition.start_lba + u32::from(sector)),
        },
        FatType::Fat16 => None,
    };
    let root_dir_lba = (root_dir_sectors != 0).then(|| data_start_lba - root_dir_sectors);
    Ok(VolumeGeometry {
        partition,
        fat_type,
        bytes_per_sector,
        sectors_per_cluster,
        reserved_sectors,
        fat_count,
        fat_size_sectors,
        root_dir_sectors,
        total_sectors,
        cluster_count,
        cluster_bytes: u32::from(bytes_per_sector) * u32::from(sectors_per_cluster),
        data_start_lba,
        data_start_byte: u64::from(data_start_lba) * u64::from(bytes_per_sector),
        fs_info_lba,
        root_dir_lba,
    })
}

/// The whole §1.1 decision over the two sectors: parse, compute, admit.
pub fn admit(mbr: &[u8; 512], bpb: &[u8; 512], index: usize) -> Result<VolumeGeometry, Unsupported> {
    let geometry = geometry(partition(mbr, index)?, bpb)?;
    geometry.admit()?;
    Ok(geometry)
}

fn u16_le(bytes: &[u8; 512], at: usize) -> u16 {
    u16::from_le_bytes([bytes[at], bytes[at + 1]])
}

fn u32_le(bytes: &[u8; 512], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An SD-Association-formatted 8 GiB card: a 4 MiB-aligned partition at LBA 8192 with 32 KiB
    /// clusters, which §1.1 names as satisfying both preconditions.
    fn sd_association_card() -> ([u8; 512], [u8; 512]) {
        let mbr = mbr_with(0x0C, 8_192, 15_523_840);
        let bpb = bpb_fat32(64, 5_392, 15_523_840, 32);
        (mbr, bpb)
    }

    fn mbr_with(kind: u8, start_lba: u32, sectors: u32) -> [u8; 512] {
        let mut mbr = [0u8; 512];
        let at = PARTITION_TABLE;
        mbr[at] = 0x00;
        mbr[at + 4] = kind;
        mbr[at + 8..at + 12].copy_from_slice(&start_lba.to_le_bytes());
        mbr[at + 12..at + 16].copy_from_slice(&sectors.to_le_bytes());
        mbr[510..512].copy_from_slice(&BOOT_SIGNATURE.to_le_bytes());
        mbr
    }

    fn bpb_fat32(sectors_per_cluster: u8, fat_size: u32, total: u32, reserved: u16) -> [u8; 512] {
        let mut bpb = [0u8; 512];
        bpb[3..11].copy_from_slice(b"MSDOS5.0");
        bpb[11..13].copy_from_slice(&512u16.to_le_bytes());
        bpb[13] = sectors_per_cluster;
        bpb[14..16].copy_from_slice(&reserved.to_le_bytes());
        bpb[16] = 2;
        bpb[32..36].copy_from_slice(&total.to_le_bytes());
        bpb[36..40].copy_from_slice(&fat_size.to_le_bytes());
        bpb[510..512].copy_from_slice(&BOOT_SIGNATURE.to_le_bytes());
        bpb
    }

    #[test]
    fn the_sd_association_layout_satisfies_both_preconditions() {
        let (mbr, bpb) = sd_association_card();
        let geometry = admit(&mbr, &bpb, 0).unwrap();
        assert_eq!(geometry.fat_type, FatType::Fat32);
        assert_eq!(geometry.cluster_bytes, 32_768);
        assert_eq!(geometry.data_start_lba, 8_192 + 32 + 2 * 5_392);
        assert_eq!(geometry.data_start_byte % PROGRAM_PAGE as u64, 0);
    }

    /// Precondition 1: a 4 KiB cluster is a legal FAT volume and not a legal OBC2 one.
    #[test]
    fn a_cluster_smaller_than_the_program_page_is_unsupported() {
        let mbr = mbr_with(0x0C, 8_192, 15_523_840);
        let bpb = bpb_fat32(8, 15_136, 15_523_840, 32);
        let geometry = geometry(partition(&mbr, 0).unwrap(), &bpb).unwrap();
        assert_eq!(geometry.cluster_bytes, 4_096);
        assert!(geometry.data_region_is_page_aligned(), "only precondition 1 is under test here");
        assert_eq!(geometry.admit(), Err(Unsupported::ClusterNotWholePages(4_096)));
    }

    /// Precondition 2: the same card with one reserved sector fewer puts the data region on an odd
    /// sector, and every slot stride in the store would then straddle two program pages.
    #[test]
    fn a_misaligned_data_region_is_unsupported() {
        let mbr = mbr_with(0x0C, 8_192, 15_523_840);
        let bpb = bpb_fat32(64, 5_392, 15_523_840, 31);
        let geometry = geometry(partition(&mbr, 0).unwrap(), &bpb).unwrap();
        assert!(!geometry.data_region_is_page_aligned());
        assert!(matches!(geometry.admit(), Err(Unsupported::DataRegionMisaligned(_))));
    }

    /// A partition that begins on a 32-sector boundary but not a page one fails too: §1.1's check is
    /// against physical LBA 0, not against the partition.
    #[test]
    fn alignment_is_measured_from_physical_lba_zero() {
        let mbr = mbr_with(0x0C, 8_200, 15_523_840);
        let bpb = bpb_fat32(64, 5_392, 15_523_840, 32);
        let geometry = geometry(partition(&mbr, 0).unwrap(), &bpb).unwrap();
        assert!(matches!(geometry.admit(), Err(Unsupported::DataRegionMisaligned(_))));
    }

    /// §1.1: "The check is filesystem-type-neutral — a FAT16 volume that satisfies both is admitted".
    #[test]
    fn a_conforming_fat16_volume_is_admitted() {
        let mbr = mbr_with(0x06, 8_192, 2_097_152);
        let mut bpb = [0u8; 512];
        bpb[3..11].copy_from_slice(b"MSDOS5.0");
        bpb[11..13].copy_from_slice(&512u16.to_le_bytes());
        bpb[13] = 32; // 16 KiB clusters
                      // 32 reserved sectors rather than FAT16's customary one: the reserved region, both FATs and
                      // the fixed root directory together have to be a whole number of program pages.
        bpb[14..16].copy_from_slice(&32u16.to_le_bytes());
        bpb[16] = 2;
        bpb[17..19].copy_from_slice(&512u16.to_le_bytes()); // 32 root-dir sectors
        bpb[22..24].copy_from_slice(&256u16.to_le_bytes());
        bpb[32..36].copy_from_slice(&2_097_152u32.to_le_bytes());
        bpb[510..512].copy_from_slice(&BOOT_SIGNATURE.to_le_bytes());
        let geometry = admit(&mbr, &bpb, 0).unwrap();
        assert_eq!(geometry.fat_type, FatType::Fat16);
        assert_eq!(geometry.root_dir_sectors, 32);
        assert_eq!(geometry.cluster_bytes, 16_384);
    }

    #[test]
    fn exfat_and_unpartitioned_volumes_are_told_apart() {
        let mut bpb = [0u8; 512];
        bpb[3..11].copy_from_slice(b"EXFAT   ");
        bpb[510..512].copy_from_slice(&BOOT_SIGNATURE.to_le_bytes());
        // exFAT's own partition type is 0x07, which never reaches the BPB check…
        let mbr = mbr_with(0x07, 2_048, 1_000);
        assert_eq!(partition(&mbr, 0), Err(Unsupported::PartitionType(0x07)));
        // …but a volume mislabelled 0x0C does, and the marker catches it.
        assert_eq!(geometry(partition(&mbr_with(0x0C, 2_048, 1_000), 0).unwrap(), &bpb), Err(Unsupported::ExFat));
        // A superfloppy: a BPB at LBA 0 with no partition table.
        let mut superfloppy = [0u8; 512];
        superfloppy[510..512].copy_from_slice(&BOOT_SIGNATURE.to_le_bytes());
        superfloppy[PARTITION_TABLE] = 0xEB;
        assert_eq!(partition(&superfloppy, 0), Err(Unsupported::BadPartitionStatus(0xEB)));
        assert_eq!(partition(&[0u8; 512], 0), Err(Unsupported::NotPartitioned));
    }

    #[test]
    fn a_fat12_volume_and_a_nonsense_bpb_are_refused() {
        let mbr = mbr_with(0x06, 2_048, 8_192);
        let bpb = bpb_fat32(32, 2, 8_192, 4);
        assert_eq!(geometry(partition(&mbr, 0).unwrap(), &bpb), Err(Unsupported::Fat12));
        let bpb = bpb_fat32(0, 2, 8_192, 4);
        assert_eq!(geometry(partition(&mbr, 0).unwrap(), &bpb), Err(Unsupported::SectorsPerCluster(0)));
        let mut bpb = bpb_fat32(32, 2, 8_192, 4);
        bpb[11..13].copy_from_slice(&4_096u16.to_le_bytes());
        assert_eq!(geometry(partition(&mbr, 0).unwrap(), &bpb), Err(Unsupported::BytesPerSector(4_096)));
    }
}
