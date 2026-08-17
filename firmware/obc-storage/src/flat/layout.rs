//! Card geometry (`FLAT_Store_Format.md` §2), the extent area (§6) and the address arithmetic that
//! is the whole read path (§6.1).
//!
//! Every address the store computes is arithmetic on these constants. There is no partition table,
//! no filesystem, no indirection block and no chain walk.

use super::error::{DecodeError, Reason, Record, Result};

/// One block.
pub const BLOCK: usize = 512;
/// The media program page: a cut may corrupt blocks inside the page being programmed and no block
/// outside it (§1). Every region boundary and record stride below is a multiple of it.
pub const PROGRAM_PAGE: usize = 16_384;
/// Blocks in one program page.
#[cfg(any(test, feature = "std"))]
pub const PAGE_BLOCKS: u64 = (PROGRAM_PAGE / BLOCK) as u64;

/// Superblock copy A, and copy B (§2). The body is block 0 of the copy.
pub const SUPERBLOCK: [u64; 2] = [0, 32];
/// Catalog copy A, and copy B (§2).
pub const CATALOG: [u64; 2] = [64, 576];
/// Blocks in one catalog copy.
#[cfg(test)]
pub const CATALOG_BLOCKS: u64 = 512;
/// The gate block of a catalog copy, copy-relative (§5.1).
pub const CATALOG_GATE_BLOCK: u64 = 480;
/// Blocks of the entry array (§5.1).
pub const ENTRY_BLOCKS: usize = 480;
/// Entries in one block.
pub const ENTRIES_PER_BLOCK: usize = BLOCK / ENTRY_STRIDE;
/// One catalog entry (§5.3).
pub const ENTRY_STRIDE: usize = 128;
/// Entries one copy holds: `479 × 4` (§5.1).
pub const ENTRY_CAPACITY: usize = (ENTRY_BLOCKS - 1) * ENTRIES_PER_BLOCK;

/// The ride journal (§2), 16 slots of 32 KiB.
pub const JOURNAL: u64 = 1_088;
/// Blocks in one journal slot: two program pages.
pub const SLOT_BLOCKS: u64 = 64;
/// Journal slots (§7).
pub const SLOTS: usize = 16;

/// The extent area begins here (§6).
pub const EXTENT_AREA: u64 = 4_096;
/// One extent: 1 MiB.
pub const EXTENT_SIZE: u64 = 1 << 20;
/// Blocks in one extent.
pub const EXTENT_BLOCKS: u64 = EXTENT_SIZE / BLOCK as u64;
/// Extents a `u16` extent index can address (§6).
pub const MAX_EXTENTS: u32 = 65_536;
/// Extent ranges one object may have (§5.3).
pub const MAX_RANGES: usize = 8;

/// The first block of catalog copy `copy`'s gate.
pub fn catalog_gate(copy: usize) -> u64 {
    CATALOG[copy] + CATALOG_GATE_BLOCK
}

/// The first block of journal slot `slot` (§7).
pub fn slot_block(slot: usize) -> u64 {
    JOURNAL + SLOT_BLOCKS * slot as u64
}

/// Bytes the body of a catalog copy holding `entries` entries covers (§5.1). The store never needs
/// the whole body at once — `commit` counts blocks as it streams them — so this is what a reader of
/// one asks for.
#[cfg(any(test, feature = "std"))]
pub fn body_len(entries: u16) -> usize {
    BLOCK + entries as usize * ENTRY_STRIDE
}

/// §6's extent count, from the card's block count.
pub fn extent_count(total_blocks: u64) -> u32 {
    if total_blocks <= EXTENT_AREA {
        return 0;
    }
    let extents = (total_blocks - EXTENT_AREA) / EXTENT_BLOCKS;
    extents.min(MAX_EXTENTS as u64) as u32
}

/// Extents a payload of `bytes` needs.
pub fn extents_for(bytes: u64) -> u64 {
    bytes.div_ceil(EXTENT_SIZE)
}

/// One object's extents, in payload order: range `i` carries the payload bytes that follow range
/// `i-1` (§5.3).
///
/// This type is the format's extent vocabulary and never crosses the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ranges {
    ranges: [(u16, u16); MAX_RANGES],
    count: u8,
}

impl Ranges {
    /// Appends one range, coalescing with the previous one when it is adjacent — which is what keeps
    /// a first-fit walk over a contiguous free run inside one range instead of eight.
    #[must_use]
    pub fn push(&mut self, first: u16, extents: u16) -> Option<()> {
        if extents == 0 {
            return None;
        }
        if self.count > 0 {
            let last = &mut self.ranges[self.count as usize - 1];
            if last.0 as u32 + last.1 as u32 == first as u32 {
                last.1 += extents;
                return Some(());
            }
        }
        if self.count as usize == MAX_RANGES {
            return None;
        }
        self.ranges[self.count as usize] = (first, extents);
        self.count += 1;
        Some(())
    }

    pub fn len(&self) -> usize {
        self.count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The ranges, in payload order.
    pub fn iter(&self) -> impl Iterator<Item = (u16, u16)> + '_ {
        self.ranges[..self.count as usize].iter().copied()
    }

    /// Extents this object owns.
    pub fn extents(&self) -> u32 {
        self.iter().map(|(_, count)| count as u32).sum()
    }

    /// True when one of these ranges covers `extent`.
    pub fn names(&self, extent: u16) -> bool {
        self.iter().any(|(first, count)| extent >= first && extent - first < count)
    }

    /// Drops the tail beyond `extents`, and reports the extents it gave up so the caller can free
    /// them — §5.3's "every other entry is trimmed to its payload at the commit that publishes it".
    pub fn trim_to(&mut self, extents: u32) -> Ranges {
        let mut freed = Ranges::default();
        let mut kept = Ranges::default();
        let mut remaining = extents;
        for (first, count) in self.iter() {
            let keep = remaining.min(count as u32) as u16;
            if keep > 0 {
                let _ = kept.push(first, keep);
                remaining -= keep as u32;
            }
            if keep < count {
                let _ = freed.push(first + keep, count - keep);
            }
        }
        *self = kept;
        freed
    }

    /// §6.1: the block holding payload offset `offset`, the byte inside it, and how many payload
    /// bytes remain contiguous on the card from there.
    pub fn locate(&self, offset: u64) -> Option<Located> {
        let mut start = 0u64;
        for (first, count) in self.iter() {
            let span = count as u64 * EXTENT_SIZE;
            if offset < start + span {
                let inner = offset - start;
                return Some(Located {
                    block: EXTENT_AREA + EXTENT_BLOCKS * first as u64 + inner / BLOCK as u64,
                    offset: (inner % BLOCK as u64) as usize,
                    contiguous: span - inner,
                });
            }
            start += span;
        }
        None
    }

    /// Decodes §5.3's eight `(u16 first, u16 count)` pairs. `range_count` live ranges, each nonzero
    /// and inside the extent area, and the rest all zero.
    pub fn decode(field: &[u8], range_count: u8, extent_count: u32) -> Result<Self> {
        let err = |reason| DecodeError::new(Record::Entry, reason);
        if range_count == 0 || range_count as usize > MAX_RANGES {
            return Err(err(Reason::Count));
        }
        let mut ranges = Ranges::default();
        for index in 0..MAX_RANGES {
            let first = super::raw::u16_at(field, index * 4);
            let count = super::raw::u16_at(field, index * 4 + 2);
            if index >= range_count as usize {
                if first != 0 || count != 0 {
                    return Err(err(Reason::Reserved));
                }
                continue;
            }
            if count == 0 || first as u32 + count as u32 > extent_count {
                return Err(err(Reason::Ranges));
            }
            // Adjacent ranges would coalesce, so a decoded array with more ranges than the writer
            // could have produced is still accepted: the payload mapping is the same either way.
            ranges.ranges[index] = (first, count);
            ranges.count += 1;
        }
        Ok(ranges)
    }

    /// Encodes the 32-byte field.
    pub fn encode(&self) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (index, (first, count)) in self.iter().enumerate() {
            super::raw::put_u16(&mut out, index * 4, first);
            super::raw::put_u16(&mut out, index * 4 + 2, count);
        }
        out
    }
}

/// Where a payload offset lives on the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    /// The block holding it.
    pub block: u64,
    /// The byte inside that block.
    pub offset: usize,
    /// Payload bytes that follow it contiguously on the card, this range's tail included.
    pub contiguous: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §2's table: no two regions share a program page, and the extent area starts on a 1 MiB
    /// boundary.
    #[test]
    fn every_region_boundary_is_page_aligned() {
        for boundary in [SUPERBLOCK[0], SUPERBLOCK[1], CATALOG[0], CATALOG[1], JOURNAL, EXTENT_AREA] {
            assert_eq!(boundary % PAGE_BLOCKS, 0, "block {boundary} is not page aligned");
        }
        assert_eq!(CATALOG[0], SUPERBLOCK[1] + 32);
        assert_eq!(CATALOG[1], CATALOG[0] + CATALOG_BLOCKS);
        assert_eq!(JOURNAL, CATALOG[1] + CATALOG_BLOCKS);
        assert_eq!(SLOT_BLOCKS * SLOTS as u64, 1_024);
        assert_eq!(EXTENT_AREA - (JOURNAL + SLOT_BLOCKS * SLOTS as u64), 1_984);
        assert_eq!(EXTENT_AREA * BLOCK as u64 % EXTENT_SIZE, 0);
        assert_eq!(catalog_gate(0), 544);
        assert_eq!(catalog_gate(1), 1_056);
        assert_eq!(slot_block(3), 1_280);
    }

    /// §5.1: `512 + 1916 × 128 = 245,760` fills blocks `0..480` exactly.
    #[test]
    fn the_entry_array_fills_its_blocks_exactly() {
        assert_eq!(ENTRY_CAPACITY, 1_916);
        assert_eq!(body_len(ENTRY_CAPACITY as u16), 245_760);
        assert_eq!(body_len(ENTRY_CAPACITY as u16), ENTRY_BLOCKS * BLOCK);
        // §5.5 step 2's write count: one header block plus `ceil(n / 4)`.
        for (entries, blocks) in [(0u16, 1usize), (1, 2), (4, 2), (5, 3), (ENTRY_CAPACITY as u16, ENTRY_BLOCKS)] {
            assert_eq!(body_len(entries).div_ceil(BLOCK), blocks);
        }
    }

    /// §4.1's card: 62,914,560 blocks recompute to 30,718 extents. And §6's cap holds above 64 GiB.
    #[test]
    fn extent_count_is_section_6s_formula() {
        assert_eq!(extent_count(62_914_560), 30_718);
        assert_eq!(extent_count(EXTENT_AREA), 0);
        assert_eq!(extent_count(EXTENT_AREA + EXTENT_BLOCKS - 1), 0);
        assert_eq!(extent_count(EXTENT_AREA + EXTENT_BLOCKS), 1);
        assert_eq!(extent_count(u64::MAX), MAX_EXTENTS);
    }

    /// §6.1's worked example: the route entry's range `(12, 1)`, payload offset 40,960.
    #[test]
    fn addressing_matches_the_section_6_1_example() {
        let mut ranges = Ranges::default();
        ranges.push(12, 1).unwrap();
        let located = ranges.locate(40_960).unwrap();
        assert_eq!(located.block, 28_752);
        assert_eq!(located.offset, 0);
        assert_eq!(located.contiguous, EXTENT_SIZE - 40_960);
        assert!(ranges.locate(EXTENT_SIZE).is_none());
    }

    /// §6.1: any payload offset that is a multiple of 16,384 maps to a page-aligned block, which is
    /// what §7.2's payload-page flush relies on.
    #[test]
    fn page_aligned_payload_offsets_map_to_page_aligned_blocks() {
        let mut ranges = Ranges::default();
        ranges.push(13, 32).unwrap();
        for page in 0..(32 * EXTENT_SIZE / PROGRAM_PAGE as u64) {
            let located = ranges.locate(page * PROGRAM_PAGE as u64).unwrap();
            assert_eq!(located.offset, 0);
            assert_eq!(located.block % PAGE_BLOCKS, 0, "payload page {page} is not page aligned");
        }
    }

    #[test]
    fn ranges_coalesce_and_span_the_payload_in_order() {
        let mut ranges = Ranges::default();
        ranges.push(4, 2).unwrap();
        ranges.push(6, 1).unwrap();
        assert_eq!(ranges.len(), 1, "adjacent ranges did not coalesce");
        ranges.push(9, 1).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges.extents(), 4);

        // The second range carries the payload bytes that follow the first.
        let located = ranges.locate(3 * EXTENT_SIZE).unwrap();
        assert_eq!(located.block, EXTENT_AREA + EXTENT_BLOCKS * 9);
        assert_eq!(located.contiguous, EXTENT_SIZE);
    }

    #[test]
    fn a_ninth_range_and_an_empty_range_are_refused() {
        let mut ranges = Ranges::default();
        for index in 0..MAX_RANGES {
            ranges.push(index as u16 * 2, 1).unwrap();
        }
        assert_eq!(ranges.len(), MAX_RANGES);
        assert!(ranges.push(100, 1).is_none());
        assert!(Ranges::default().push(0, 0).is_none());
    }

    #[test]
    fn trimming_keeps_payload_order_and_reports_the_freed_tail() {
        let mut ranges = Ranges::default();
        ranges.push(4, 2).unwrap();
        ranges.push(20, 3).unwrap();
        let freed = ranges.trim_to(3);
        assert_eq!(ranges.iter().collect::<heapless::Vec<_, 8>>()[..], [(4, 2), (20, 1)]);
        assert_eq!(freed.iter().collect::<heapless::Vec<_, 8>>()[..], [(21, 2)]);
    }

    #[test]
    fn a_range_leaving_the_extent_area_is_refused() {
        let mut field = [0u8; 32];
        super::super::raw::put_u16(&mut field, 0, 30_717);
        super::super::raw::put_u16(&mut field, 2, 2);
        assert_eq!(Ranges::decode(&field, 1, 30_718).unwrap_err().reason, Reason::Ranges);
        super::super::raw::put_u16(&mut field, 2, 1);
        assert!(Ranges::decode(&field, 1, 30_718).is_ok());

        // Ranges beyond the live count must be zero, and the count itself is bounded.
        super::super::raw::put_u16(&mut field, 4, 1);
        assert_eq!(Ranges::decode(&field, 1, 30_718).unwrap_err().reason, Reason::Reserved);
        assert_eq!(Ranges::decode(&field, 0, 30_718).unwrap_err().reason, Reason::Count);
        assert_eq!(Ranges::decode(&field, 9, 30_718).unwrap_err().reason, Reason::Count);
    }
}
