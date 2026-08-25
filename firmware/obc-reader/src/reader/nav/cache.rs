//! Caller-owned navigation tile and quadtree-index cache.

use super::super::QuadIndex;
use super::NAV_MAX_CHUNK_BYTES;
use obc_formats::cache::IndexBlockCache;
use obc_formats::io::{ByteSource, Error as IoError};

/// Graph-tile cache slots. **Thirty-two**: the earlier 8-slot measurement covered one route, while
/// the 2026-08-08 physical-command study covered Grimsel, Monaco and failure/escalation paths. The
/// larger frontier working set remained useful through 32 slots, cutting node-chunk misses by
/// roughly 1.5–2.5× over 8 depending on density. The cache lives in the route-only scratch-arena arm,
/// which had ~69 KiB below the 128 KiB USB maximum, so this growth costs zero linked resident RAM.
/// Fully-associative round-robin is retained: 32 tag compares are negligible beside a card command
/// and preserve the measured hit rate without conflict misses.
const NAV_TILE_SLOTS: usize = 32;

/// Route-private aligned quadtree-index windows. Real nav indexes are about 8 KiB; the render cache's
/// seven windows repeatedly scanned and thrashed because every settled node re-descends the tree.
/// Sixteen scan-resistant RRIP windows keep that working set inside the route-only arena arm and
/// leave the renderer's carefully-budgeted seven-window cache untouched.
const NAV_INDEX_BLOCKS: usize = 16;

/// Empty-slot tag for [`NavTileCache`]: a chunk's absolute file offset never reaches `u64::MAX`
/// (its whole extent must lie inside the source, and §1.1 bounds a file at `2^32 × U`).
const NAV_TILE_EMPTY: u64 = u64::MAX;

/// A snapshot of the [`NavTileCache`] counters. These are **logical** `ByteSource::read_at` counts;
/// physical command counts are lower-level transport diagnostics. Sector-aligned current producers make
/// every full node/edge/index fill one physical command, while old unaligned maps may need two.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NavCacheStats {
    /// Nav-chunk requests served from a resident slot (no SD read).
    pub hits: u32,
    /// Nav-chunk requests that missed and read `chunk_size` bytes from the source.
    pub misses: u32,
    /// Quadtree-index node reads served by one of the route-private aligned windows.
    pub index_hits: u32,
    /// Route-private index windows filled from the source.
    pub index_misses: u32,
}

impl NavCacheStats {
    /// Total logical source fills attributable to route traversal. This is the scheduler's expensive
    /// unit: graph chunks plus index windows, never resident hits.
    #[inline]
    pub const fn source_reads(self) -> u32 {
        self.misses.saturating_add(self.index_misses)
    }
}

/// A tiny caller-owned cache of whole nav chunks (node **and** edge-pool — both are `chunk_size`
/// ≤ [`NAV_MAX_CHUNK_BYTES`] bytes, spec §8.1), keyed by the chunk's absolute file offset so the
/// two chunk spaces can't collide. [`crate::Reader::for_each_nav_node_cached`] and
/// [`crate::Reader::nav_edge_oriented`] stream through it so the router's per-settle spatial re-fetch
/// doesn't re-read the same leaf from the SD (epic #116's named risk). Round-robin eviction: across
/// the `NAV_TILE_SLOTS` slots the measured hit rate matches LRU's within noise (the frontier's
/// live-leaf set has no strong recency skew), so the cheaper cursor is kept.
///
/// ~25 KB, owned by the caller (the device puts it in the route-only scratch-arena arm); `new()` is
/// `const` so a static/arena initialization stays deterministic. The tags are only meaningful
/// against one map/source — the router resets it per plan, so a map switch cannot cross-serve stale
/// graph or index bytes.
pub struct NavTileCache {
    slots: [[u8; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
    /// Absolute file offset of the chunk each slot holds, or [`NAV_TILE_EMPTY`].
    tags: [u64; NAV_TILE_SLOTS],
    /// Round-robin eviction cursor.
    next: u8,
    hits: u32,
    misses: u32,
    /// The shared index-block driver, sixteen windows wide; its own counters are this cache's
    /// `index_hits`/`index_misses`.
    index: IndexBlockCache<NAV_INDEX_BLOCKS>,
}

// On-device: 32 graph sectors + tags/counters and sixteen 520-byte index windows. It was 24,852 B
// while a slot tag was a `u32`; the u64 read seam adds 128 B of tags and eight of alignment. Unlike
// `MapCache`, this cache may take the `u64`'s 8-byte alignment: it lives in the scratch arena's
// route arm rather than in a `.bss` slot the boot task fills, so no placement of it is on a poll
// frame — the distinction `MapCache`'s `align_of` assert documents, checked here by measurement.
#[cfg(target_pointer_width = "32")]
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
        }
    }

    /// Invalidate every slot and zero the counters — call before a fresh route computation (or
    /// after a map switch) so stale tags can't serve another file's bytes and the counters read
    /// as "this run's I/O".
    pub fn reset(&mut self) {
        self.tags = [NAV_TILE_EMPTY; NAV_TILE_SLOTS];
        self.next = 0;
        self.hits = 0;
        self.misses = 0;
        self.index.reset();
    }

    /// Snapshot of the hit/miss counters since the last [`NavTileCache::reset`].
    #[inline]
    pub fn stats(&self) -> NavCacheStats {
        NavCacheStats {
            hits: self.hits,
            misses: self.misses,
            index_hits: self.index.hits(),
            index_misses: self.index.misses(),
        }
    }

    /// The `len`-byte chunk at absolute `offset`, from a resident slot or (on miss) read from
    /// `src` into the round-robin victim. `None` on a read failure — the victim's tag is cleared
    /// *before* the read so a short/failed fill can never leave a stale tag over garbage bytes.
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
        let byte_index = (idx as u64).checked_mul(4).ok_or(IoError::BadOffset)?;
        let off = index.index_offset().checked_add(byte_index).ok_or(IoError::BadOffset)?;
        let mut word = [0u8; 4];
        self.index_read(src, off, &mut word)?;
        Ok(u32::from_le_bytes(word))
    }

    /// Read through the route-private index working set.
    ///
    /// The bimodal insertion decision stays this cache's and stays spelled as it always was: its
    /// own miss counter, sampled *after* this fill is counted — one step later than the render
    /// cache's phase. `fill` is that post-increment count.
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
    /// round-robin eviction drops the **oldest** on the next miss.
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

        // Re-touch all slots — every one still resident, so no new read.
        for i in 0..NAV_TILE_SLOTS {
            assert_eq!(cache.chunk(&src, off(i), LEN).unwrap()[0], i as u8);
        }
        assert_eq!(
            cache.stats(),
            NavCacheStats { hits: NAV_TILE_SLOTS as u32, misses: NAV_TILE_SLOTS as u32, ..NavCacheStats::default() }
        );

        // One more distinct chunk evicts the oldest (round-robin cursor = slot 0 = chunk 0).
        assert_eq!(cache.chunk(&src, off(NAV_TILE_SLOTS), LEN).unwrap()[0], NAV_TILE_SLOTS as u8);
        assert_eq!(cache.stats().misses, NAV_TILE_SLOTS as u32 + 1);

        // Chunk 1 survived the eviction ⇒ hits; chunk 0 was evicted ⇒ re-reads. (Order matters: the
        // chunk-0 re-read then evicts the next round-robin victim, so check the hit first.)
        let s = cache.stats();
        cache.chunk(&src, off(1), LEN).unwrap();
        assert_eq!(cache.stats().hits, s.hits + 1, "a still-resident chunk hits");
        let s = cache.stats();
        cache.chunk(&src, off(0), LEN).unwrap();
        assert_eq!(cache.stats().misses, s.misses + 1, "the evicted oldest re-reads");
    }

    /// A route re-descends the same quadtree for every settled node. The private index cache is
    /// deliberately scan-resistant: a cycle one sector larger than capacity should churn one
    /// probation slot, not evict the entire warm index as LRU would.
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
