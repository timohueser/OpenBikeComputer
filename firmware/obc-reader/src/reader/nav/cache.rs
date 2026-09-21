//! Caller-owned navigation tile and quadtree-index cache.

use super::super::QuadIndex;
use super::NAV_MAX_CHUNK_BYTES;
use obc_formats::cache::IndexBlockCache;
use obc_formats::io::{ByteSource, Error as IoError};

/// Graph-tile cache slots. Thirty-two: the frontier working set stays useful through 32 slots,
/// cutting node-chunk misses by roughly 1.5 to 2.5 times over 8 depending on density. The cache
/// lives in the route-only scratch-arena arm, which has the headroom, so the size costs no linked
/// resident RAM. Fully-associative round-robin is kept: 32 tag compares are negligible beside a
/// card command and leave no conflict misses.
const NAV_TILE_SLOTS: usize = 32;

/// Route-private aligned quadtree-index windows. Real nav indexes are about 8 KiB, and the render
/// cache's seven windows thrashed, because every settled node re-descends the tree. Sixteen
/// scan-resistant windows keep that working set inside the route arena and leave the renderer's
/// budgeted cache untouched.
const NAV_INDEX_BLOCKS: usize = 16;

/// Empty-slot tag: a chunk's absolute file offset never reaches `u64::MAX`, because its whole
/// extent must lie inside the source.
const NAV_TILE_EMPTY: u64 = u64::MAX;

/// A snapshot of the [`NavTileCache`] counters. These are logical `read_at` counts: a
/// sector-aligned current producer makes every full fill one physical command, while an older
/// unaligned map may need two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NavCacheStats {
    /// Nav-chunk requests served from a resident slot.
    pub hits: u32,
    /// Nav-chunk requests that missed and read from the source.
    pub misses: u32,
    /// Quadtree-index node reads served by a route-private window.
    pub index_hits: u32,
    /// Route-private index windows filled from the source.
    pub index_misses: u32,
    /// Junction records decoded by cached navigation walks.
    #[cfg(feature = "nav-metrics")]
    pub decoded_junctions: u64,
    /// Quadtree nodes requested, independent of index-window fills.
    #[cfg(feature = "nav-metrics")]
    pub quadtree_visits: u64,
}

impl NavCacheStats {
    /// Total logical source fills attributable to route traversal: graph chunks plus index
    /// windows, never resident hits. The scheduler's expensive unit.
    #[inline]
    pub const fn source_reads(self) -> u32 {
        self.misses.saturating_add(self.index_misses)
    }
}

/// A caller-owned cache of whole nav chunks, node and edge-pool alike, keyed by the chunk's
/// absolute file offset so the two chunk spaces cannot collide. The cached walks stream through it,
/// so the router's per-settle spatial re-fetch does not re-read the same leaf from SD. Round-robin
/// eviction: the measured hit rate matches LRU's within noise, because the frontier's live-leaf set
/// has no strong recency skew.
///
/// About 25 KB, owned by the caller, and `new()` is `const` so an arena initialization stays
/// deterministic. The tags mean something only against one source, and the router resets it per
/// plan, so a map switch cannot cross-serve stale bytes.
pub struct NavTileCache {
    slots: [[u8; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
    /// Absolute file offset of the chunk each slot holds, or [`NAV_TILE_EMPTY`].
    tags: [u64; NAV_TILE_SLOTS],
    /// Round-robin eviction cursor.
    next: u8,
    hits: u32,
    misses: u32,
    /// The shared index-block driver, sixteen windows wide; its counters are this cache's index
    /// hit and miss counts.
    index: IndexBlockCache<NAV_INDEX_BLOCKS>,
    #[cfg(feature = "nav-metrics")]
    pub(super) decoded_junctions: u64,
    #[cfg(feature = "nav-metrics")]
    quadtree_visits: u64,
}

// Unlike `MapCache`, this cache may take the `u64`'s 8-byte alignment: it lives in the scratch
// arena's route arm rather than in a `.bss` slot the boot task fills, so no placement of it sits
// on a poll frame.
#[cfg(all(target_pointer_width = "32", not(feature = "nav-metrics")))]
const _: () = assert!(core::mem::size_of::<NavTileCache>() == 24_984);

impl NavTileCache {
    pub const fn new() -> Self {
        NavTileCache {
            slots: [[0; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
            tags: [NAV_TILE_EMPTY; NAV_TILE_SLOTS],
            next: 0,
            hits: 0,
            misses: 0,
            index: IndexBlockCache::new(),
            #[cfg(feature = "nav-metrics")]
            decoded_junctions: 0,
            #[cfg(feature = "nav-metrics")]
            quadtree_visits: 0,
        }
    }

    /// Invalidate every slot and zero the counters. Call it before a fresh route computation, so
    /// stale tags cannot serve another file's bytes and the counters read as this run's I/O.
    pub fn reset(&mut self) {
        self.tags = [NAV_TILE_EMPTY; NAV_TILE_SLOTS];
        self.next = 0;
        self.hits = 0;
        self.misses = 0;
        self.index.reset();
        #[cfg(feature = "nav-metrics")]
        {
            self.decoded_junctions = 0;
            self.quadtree_visits = 0;
        }
    }

    /// Snapshot of the hit/miss counters since the last [`NavTileCache::reset`].
    #[inline]
    pub fn stats(&self) -> NavCacheStats {
        NavCacheStats {
            hits: self.hits,
            misses: self.misses,
            index_hits: self.index.hits(),
            index_misses: self.index.misses(),
            #[cfg(feature = "nav-metrics")]
            decoded_junctions: self.decoded_junctions,
            #[cfg(feature = "nav-metrics")]
            quadtree_visits: self.quadtree_visits,
        }
    }

    /// The `len`-byte chunk at absolute `offset`, from a resident slot or read into the
    /// round-robin victim. `None` on a read failure: the victim's tag is cleared before the read,
    /// so a failed fill can never leave a stale tag over garbage bytes.
    pub(in crate::reader) fn chunk(&mut self, src: &dyn ByteSource, offset: u64, len: usize) -> Option<&[u8]> {
        debug_assert!(len <= NAV_MAX_CHUNK_BYTES);
        for i in 0..NAV_TILE_SLOTS {
            if self.tags[i] == offset {
                self.hits += 1;
                return Some(&self.slots[i][..len]);
            }
        }
        let i = self.next as usize % NAV_TILE_SLOTS;
        self.tags[i] = NAV_TILE_EMPTY;
        src.read_at(offset, &mut self.slots[i][..len]).ok()?;
        self.tags[i] = offset;
        self.next = self.next.wrapping_add(1);
        self.misses += 1;
        Some(&self.slots[i][..len])
    }

    /// Read one quadtree node through the route-private aligned index working set.
    pub(super) fn index_node(
        &mut self,
        src: &dyn ByteSource,
        index: &dyn QuadIndex,
        idx: usize,
    ) -> Result<u32, IoError> {
        #[cfg(feature = "nav-metrics")]
        {
            self.quadtree_visits += 1;
        }
        let byte_index = (idx as u64).checked_mul(4).ok_or(IoError::BadOffset)?;
        let off = index.index_offset().checked_add(byte_index).ok_or(IoError::BadOffset)?;
        let mut word = [0u8; 4];
        self.index_read(src, off, &mut word)?;
        Ok(u32::from_le_bytes(word))
    }

    /// Read through the route-private index working set. The bimodal insertion decision is this
    /// cache's: its own miss counter, sampled after this fill is counted, one step later than the
    /// render cache's phase.
    pub(in crate::reader) fn index_read(
        &mut self,
        src: &dyn ByteSource,
        off: u64,
        out: &mut [u8],
    ) -> Result<(), IoError> {
        self.index.read(src, off, out, &mut |_bytes, fill| fill.is_multiple_of(8))
    }
}

impl Default for NavTileCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;
    use obc_formats::cache::INDEX_BLOCK;
    use obc_formats::obcm::NAV_CHUNK_SIZE;

    /// The graph-tile cache holds [`NAV_TILE_SLOTS`] distinct chunks resident at once, and
    /// round-robin eviction drops the oldest on the next miss.
    #[test]
    fn nav_tile_cache_holds_the_full_working_set_and_evicts_round_robin() {
        const LEN: usize = NAV_CHUNK_SIZE; // 512, = one pinned v9 nav chunk
                                           // NAV_TILE_SLOTS + 1 distinct chunks; every byte of chunk k is `k`, so contents are checkable.
        let mut data = [0u8; (NAV_TILE_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k / LEN) as u8;
        }
        let src = SliceSource(&data);
        let mut cache = NavTileCache::new();
        let off = |i: usize| (i * LEN) as u64;

        // Prime every slot: misses only, contents correct.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(cache.stats(), NavCacheStats { hits: 0, misses: NAV_TILE_SLOTS as u32, ..NavCacheStats::default() });

        // Re-touch all slots: every one is still resident, so there is no new read.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(
            cache.stats(),
            NavCacheStats { hits: NAV_TILE_SLOTS as u32, misses: NAV_TILE_SLOTS as u32, ..NavCacheStats::default() }
        );

        // One more distinct chunk evicts the oldest.
        assert_eq!(cache.chunk(&src, off(NAV_TILE_SLOTS), LEN).unwrap()[0], NAV_TILE_SLOTS as u8);
        assert_eq!(cache.stats().misses, NAV_TILE_SLOTS as u32 + 1);

        // Chunk 1 survived the eviction and hits; chunk 0 was evicted and re-reads. Order matters,
        // because the chunk-0 re-read evicts the next victim.
        let s = cache.stats();
        cache.chunk(&src, off(1), LEN).unwrap();
        assert_eq!(cache.stats().hits, s.hits + 1, "a still-resident chunk hits");
        let s = cache.stats();
        cache.chunk(&src, off(0), LEN).unwrap();
        assert_eq!(cache.stats().misses, s.misses + 1, "the evicted oldest re-reads");
    }

    /// A route re-descends the same quadtree for every settled node, so the private index cache is
    /// scan-resistant: a cycle one sector larger than capacity churns one probation slot rather
    /// than evicting the whole warm index.
    #[test]
    fn nav_index_cache_resists_a_repeated_scan_larger_than_capacity() {
        const WORKING_BLOCKS: usize = NAV_INDEX_BLOCKS + 1;
        let data = [0u8; WORKING_BLOCKS * INDEX_BLOCK];
        let src = SliceSource(&data);
        let mut cache = NavTileCache::new();
        let mut word = [0u8; 4];

        for block in 0..WORKING_BLOCKS {
            cache.index_read(&src, (block * INDEX_BLOCK) as u64, &mut word).unwrap();
        }
        assert_eq!(cache.stats().index_misses, WORKING_BLOCKS as u32);

        for block in 0..WORKING_BLOCKS {
            cache.index_read(&src, (block * INDEX_BLOCK) as u64, &mut word).unwrap();
        }
        assert_eq!(cache.stats().index_hits, (WORKING_BLOCKS - 2) as u32);
        assert_eq!(cache.stats().index_misses, (WORKING_BLOCKS + 2) as u32);
    }
}
