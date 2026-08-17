//! The free-extent bitmap (`FLAT_Store_Format.md` §6.2): the complement of the catalog, recomputed
//! at mount.
//!
//! There is no free list on the card and nothing to reconcile — an extent is free exactly when no
//! entry names it. The resident cost is the 8 KiB §6 budgets for it, which is what the entry's `u16`
//! extent index buys.

use super::layout::{Ranges, MAX_EXTENTS};

/// Words of the resident bitmap: 65,536 bits.
const WORDS: usize = MAX_EXTENTS as usize / 64;

/// One bit per extent, set when an entry — or a live reservation — names it.
///
/// Extents past the card's own count are marked used at [`reset`](FreeMap::reset), so a first-fit
/// walk can never propose one and a smaller card needs no separate bound check.
#[derive(Debug, Clone)]
pub struct FreeMap {
    words: [u64; WORDS],
    extents: u32,
}

impl Default for FreeMap {
    fn default() -> Self {
        // A card of no extents, none of them free — which is what a store that is serving no catalog
        // has to answer, rather than the whole address space.
        FreeMap { words: [u64::MAX; WORDS], extents: 0 }
    }
}

impl FreeMap {
    /// Every extent of a card with `extents` extents free, and every address above it unavailable.
    pub fn reset(&mut self, extents: u32) {
        self.extents = extents;
        self.words = [0; WORDS];
        for index in (extents as usize).div_ceil(64)..WORDS {
            self.words[index] = u64::MAX;
        }
        if !(extents as usize).is_multiple_of(64) {
            let word = extents as usize / 64;
            self.words[word] = u64::MAX << (extents as usize % 64);
        }
    }

    pub fn is_used(&self, extent: u32) -> bool {
        self.words[extent as usize / 64] & (1 << (extent % 64)) != 0
    }

    /// Marks every extent of `ranges` used, and reports the first extent already taken — which at
    /// mount is §5.3's overlap rule failing, and a structural failure of that copy.
    pub fn claim(&mut self, ranges: &Ranges) -> Result<(), u32> {
        for (first, count) in ranges.iter() {
            for extent in first as u32..first as u32 + count as u32 {
                if self.is_used(extent) {
                    return Err(extent);
                }
                self.words[extent as usize / 64] |= 1 << (extent % 64);
            }
        }
        Ok(())
    }

    /// Returns every extent of `ranges` to the allocator.
    pub fn release(&mut self, ranges: &Ranges) {
        for (first, count) in ranges.iter() {
            for extent in first as u32..first as u32 + count as u32 {
                self.release_one(extent);
            }
        }
    }

    /// Returns one extent to the allocator.
    pub fn release_one(&mut self, extent: u32) {
        self.words[extent as usize / 64] &= !(1 << (extent % 64));
    }

    /// Free extents.
    pub fn free(&self) -> u32 {
        MAX_EXTENTS - self.words.iter().map(|word| word.count_ones()).sum::<u32>()
    }

    /// §6.2's allocation: first-fit over the free bitmap in ascending extent order, at most eight
    /// ranges. `None` is the refusal — the caller sees it, never a partial object.
    ///
    /// Nothing is marked here: the caller claims the result, so a refusal leaves the map untouched.
    pub fn first_fit(&self, extents: u32) -> Option<Ranges> {
        let mut out = Ranges::default();
        let mut remaining = extents;
        let mut extent = 0u32;
        while remaining > 0 && extent < self.extents {
            if self.is_used(extent) {
                extent += 1;
                continue;
            }
            let start = extent;
            while extent < self.extents && !self.is_used(extent) && extent - start < remaining {
                extent += 1;
            }
            out.push(start as u16, (extent - start) as u16)?;
            remaining -= extent - start;
        }
        (remaining == 0).then_some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(extents: u32) -> FreeMap {
        let mut map = FreeMap::default();
        map.reset(extents);
        map
    }

    #[test]
    fn the_resident_bitmap_is_the_eight_kilobytes_section_6_budgets() {
        assert_eq!(core::mem::size_of::<[u64; WORDS]>(), 8_192);
    }

    #[test]
    fn a_fresh_card_is_entirely_free_and_its_tail_is_unavailable() {
        let map = map(30_718);
        assert_eq!(map.free(), 30_718);
        assert!(!map.is_used(30_717));
        assert!(map.is_used(30_718), "an extent past the card's count is not marked unavailable");
        assert!(map.is_used(MAX_EXTENTS - 1));
        // Eight ranges is the cap, so the whole card only fits in one when it is one run.
        assert_eq!(map.first_fit(30_718).unwrap().extents(), 30_718);
        assert!(map.first_fit(30_719).is_none());

        // A count that lands exactly on a word boundary takes the other branch of reset.
        let mut aligned = FreeMap::default();
        aligned.reset(64);
        assert_eq!(aligned.free(), 64);
        assert!(aligned.is_used(64));
    }

    #[test]
    fn claiming_twice_reports_the_overlapping_extent() {
        let mut map = map(64);
        let mut ranges = Ranges::default();
        ranges.push(4, 2).unwrap();
        map.claim(&ranges).unwrap();
        assert_eq!(map.free(), 62);
        assert_eq!(map.claim(&ranges), Err(4));
        map.release(&ranges);
        assert_eq!(map.free(), 64);
    }

    /// First fit, ascending, and a hole is filled before the tail is touched.
    #[test]
    fn allocation_is_first_fit_over_the_holes() {
        let mut map = map(64);
        let mut used = Ranges::default();
        used.push(0, 2).unwrap();
        used.push(4, 1).unwrap();
        map.claim(&used).unwrap();

        let allocated = map.first_fit(3).unwrap();
        assert_eq!(allocated.iter().collect::<heapless::Vec<_, 8>>()[..], [(2, 2), (5, 1)]);
        assert_eq!(allocated.extents(), 3);
    }

    /// Fragmentation's worst case is a refused allocation, never corruption: nine holes cannot be
    /// expressed in eight ranges.
    #[test]
    fn an_allocation_needing_nine_ranges_is_refused_without_changing_the_map() {
        let mut map = map(64);
        // Eight ranges is the cap on one entry, so the setup itself is two claims.
        let mut first = Ranges::default();
        let mut second = Ranges::default();
        for hole in 0..9u16 {
            if hole < 5 {
                first.push(hole * 2 + 1, 1).unwrap();
            } else {
                second.push(hole * 2 + 1, 1).unwrap();
            }
        }
        map.claim(&first).unwrap();
        map.claim(&second).unwrap();
        let free_before = map.free();
        assert!(map.first_fit(9).is_none());
        assert_eq!(map.free(), free_before);
        assert!(map.first_fit(8).is_some());
    }
}
