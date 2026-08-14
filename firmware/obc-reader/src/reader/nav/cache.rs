//! Caller-owned navigation tile and quadtree-index cache.

use super::super::cache::{rrip_victim, IndexBlock, INDEX_BLOCK};
use super::super::QuadIndex;
use super::NAV_MAX_CHUNK_BYTES;
use obc_formats::io::{ByteSource, Error as IoError};

/// Graph-tile cache slots. **Thirty-two**: the earlier 8-slot measurement covered one route, while
/// the 2026-08-08 physical-command study covered Grimsel, Monaco and failure/escalation paths. The
/// larger frontier working set remained useful through 32 slots, cutting node-chunk misses by
/// roughly 1.5–2.5× over 8 depending on density. The cache lives in the route-only scratch-arena arm,
/// which had ~69 KiB below the 128 KiB USB maximum, so this growth costs zero linked resident RAM.
/// Fully-associative round-robin is retained: 32 tag compares are negligible beside a card command
/// and preserve the measured hit rate without conflict misses.
pub(in crate::reader) const NAV_TILE_SLOTS: usize = 32;

/// Route-private aligned quadtree-index windows. Real nav indexes are about 8 KiB; the render cache's
/// seven windows repeatedly scanned and thrashed because every settled node re-descends the tree.
/// Sixteen scan-resistant RRIP windows keep that working set inside the route-only arena arm and
/// leave the renderer's carefully-budgeted seven-window cache untouched.
pub(in crate::reader) const NAV_INDEX_BLOCKS: usize = 16;

/// Empty-slot tag for [`NavTileCache`]: a chunk's absolute file offset never reaches `u32::MAX`
/// (its whole extent must lie inside a `u32`-addressed source).
const NAV_TILE_EMPTY: u32 = u32::MAX;

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
/// the [`NAV_TILE_SLOTS`] slots the measured hit rate matches LRU's within noise (the frontier's
/// live-leaf set has no strong recency skew), so the cheaper cursor is kept.
///
/// ~25 KB, owned by the caller (the device puts it in the route-only scratch-arena arm); `new()` is
/// `const` so a static/arena initialization stays deterministic. The tags are only meaningful
/// against one map/source — the router resets it per plan, so a map switch cannot cross-serve stale
/// graph or index bytes.
pub struct NavTileCache {
    slots: [[u8; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
    /// Absolute file offset of the chunk each slot holds, or [`NAV_TILE_EMPTY`].
    tags: [u32; NAV_TILE_SLOTS],
    /// Round-robin eviction cursor.
    next: u8,
    hits: u32,
    misses: u32,
    index: [IndexBlock; NAV_INDEX_BLOCKS],
    index_hits: u32,
    index_misses: u32,
}

// On-device: 32 graph sectors + tags/counters and sixteen 520-byte index windows.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<NavTileCache>() == 24_852);

impl NavTileCache {
    pub const fn new() -> Self {
        NavTileCache {
            slots: [[0; NAV_MAX_CHUNK_BYTES]; NAV_TILE_SLOTS],
            tags: [NAV_TILE_EMPTY; NAV_TILE_SLOTS],
            next: 0,
            hits: 0,
            misses: 0,
            index: [IndexBlock::EMPTY; NAV_INDEX_BLOCKS],
            index_hits: 0,
            index_misses: 0,
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
        for block in &mut self.index {
            block.meta = 0;
        }
        self.index_hits = 0;
        self.index_misses = 0;
    }

    /// Snapshot of the hit/miss counters since the last [`NavTileCache::reset`].
    #[inline]
    pub fn stats(&self) -> NavCacheStats {
        NavCacheStats {
            hits: self.hits,
            misses: self.misses,
            index_hits: self.index_hits,
            index_misses: self.index_misses,
        }
    }

    /// The `len`-byte chunk at absolute `offset`, from a resident slot or (on miss) read from
    /// `src` into the round-robin victim. `None` on a read failure — the victim's tag is cleared
    /// *before* the read so a short/failed fill can never leave a stale tag over garbage bytes.
    pub(in crate::reader) fn chunk(&mut self, src: &dyn ByteSource, offset: u32, len: usize) -> Option<&[u8]> {
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
        file: u8,
        index: &dyn QuadIndex,
        idx: usize,
    ) -> Result<u32, IoError> {
        let byte_index = idx.checked_mul(4).ok_or(IoError::BadOffset)?;
        let off = u32::try_from(index.index_offset().checked_add(byte_index).ok_or(IoError::BadOffset)?)
            .map_err(|_| IoError::BadOffset)?;
        let mut word = [0u8; 4];
        self.index_read(src, file, off, &mut word)?;
        Ok(u32::from_le_bytes(word))
    }

    pub(in crate::reader) fn index_read(
        &mut self,
        src: &dyn ByteSource,
        file: u8,
        off: u32,
        out: &mut [u8],
    ) -> Result<(), IoError> {
        let mut filled = 0usize;
        while filled < out.len() {
            let cur = off.checked_add(filled as u32).ok_or(IoError::BadOffset)?;
            let block_off = cur - cur % INDEX_BLOCK as u32;
            let slot = self.index_block(src, file, block_off)?;
            let within = (cur - block_off) as usize;
            let blen = self.index[slot].len as usize;
            if within >= blen {
                return Err(IoError::BadOffset);
            }
            let take = (blen - within).min(out.len() - filled);
            out[filled..filled + take].copy_from_slice(&self.index[slot].buf[within..within + take]);
            filled += take;
        }
        Ok(())
    }

    fn index_block(&mut self, src: &dyn ByteSource, file: u8, block_off: u32) -> Result<usize, IoError> {
        if let Some(i) = self.index.iter().position(|b| b.valid() && b.file() == file && b.off == block_off) {
            self.index[i].set_rrpv(0);
            self.index_hits = self.index_hits.saturating_add(1);
            return Ok(i);
        }
        let remaining = src.len().checked_sub(block_off).ok_or(IoError::BadOffset)? as usize;
        let want = remaining.min(INDEX_BLOCK);
        if want == 0 {
            return Err(IoError::BadOffset);
        }
        let empty = self.index.iter().position(|b| !b.valid());
        let i = empty.unwrap_or_else(|| rrip_victim(&mut self.index));
        self.index[i].meta = 0;
        src.read_at(block_off, &mut self.index[i].buf[..want])?;
        self.index[i].off = block_off;
        self.index[i].len = want as u16;
        self.index_misses = self.index_misses.saturating_add(1);
        let rrpv = if empty.is_some() || self.index_misses.is_multiple_of(8) { 2 } else { 3 };
        self.index[i].commit(file, rrpv);
        Ok(i)
    }
}

impl Default for NavTileCache {
    fn default() -> Self {
        Self::new()
    }
}
