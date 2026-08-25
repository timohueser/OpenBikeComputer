//! The resident tile cache: `N` × 512 B of terrain, plus the one-entry directory memo that keeps a
//! bilinear query from re-reading the directory for every corner.
//!
//! **This value must never live on a stack.** At the v1 `N = 4` it is ≈ 2.1 KB, and the nRF54L's
//! async tasks run on ~36 KB (issues #419 / #501: the 26.5 kB `nav_step` frame that had to be
//! hunted down). Place it once — a `static`, or the reserved region the device `ptr::write`s its
//! `App` into — and hand out `&mut` from there. [`TileCache::new`] is `const` precisely so a
//! `static CACHE: TileCache<4> = TileCache::new();` lands in `.bss` and is never built anywhere else.
//!
//! Recency is a monotonic tick stamped on each access and eviction picks the lowest stamp — real
//! LRU rather than a clock hand, because a single bilinear sample can touch four tiles at a cell
//! corner and a round-robin hand would evict the tile the *next* corner needs.

use obc_formats::cache::lru_victim;
use obc_formats::obct::TILE_BYTES;

/// A slot holds one tile keyed by its **absolute byte offset in the file**. `0` is the empty key:
/// no tile can start at 0 because the header does.
const EMPTY: u32 = 0;

/// `N` tile slots plus the last-resolved cell offset. Bound to one [`TerrainReader`] parse by
/// [`adopt`](TileCache::adopt), so a cache carried across a terrain-file switch cannot cross-serve
/// another file's bytes — the same generation guard `obc-reader`'s `MapCache` uses, and for the same
/// reason: the keys are file offsets, which mean different things in different files.
///
/// [`TerrainReader`]: crate::TerrainReader
pub struct TileCache<const N: usize> {
    tiles: [[u8; TILE_BYTES]; N],
    /// Absolute file offset of the tile in each slot, or [`EMPTY`].
    keys: [u32; N],
    stamps: [u32; N],
    tick: u32,
    /// The parse generation the resident slots belong to; `0` (the zero-init state) is never a live
    /// generation, so a fresh cache adopts on first use.
    generation: u32,
    /// `(cell_i, cell_j, cell_offset)` of the last directory lookup that found a present cell.
    /// A bilinear query reads up to four corners out of the same cell, and a route or a rendered
    /// profile walks in a straight line — so one entry removes essentially every repeat directory
    /// read without any of the residency machinery a real directory cache would need.
    cell_memo: Option<(u32, u32, u32)>,
    hits: u32,
    misses: u32,
}

impl<const N: usize> Default for TileCache<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> TileCache<N> {
    /// A fresh, empty cache. `const` so it can initialize a `static` directly (see the type docs).
    pub const fn new() -> Self {
        TileCache {
            tiles: [[0u8; TILE_BYTES]; N],
            keys: [EMPTY; N],
            stamps: [0; N],
            tick: 0,
            generation: 0,
            cell_memo: None,
            hits: 0,
            misses: 0,
        }
    }

    /// Drop every resident tile and the directory memo. Touches only the keys and counters, never
    /// the `N` × 512 B of buffers.
    pub fn clear(&mut self) {
        self.keys = [EMPTY; N];
        self.stamps = [0; N];
        self.tick = 0;
        self.cell_memo = None;
        self.hits = 0;
        self.misses = 0;
    }

    /// Bind to a parse generation, clearing first if the cache last served a different one.
    /// Called by every sample, which is what makes the forgotten-`clear()`-on-file-switch
    /// cross-serve impossible by construction rather than by discipline.
    pub(crate) fn adopt(&mut self, generation: u32) {
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
    }

    /// The resident tile at absolute file offset `key`, or `None` on a miss.
    pub(crate) fn get(&mut self, key: u32) -> Option<&[u8; TILE_BYTES]> {
        let slot = self.keys.iter().position(|&k| k == key && k != EMPTY)?;
        self.tick = self.tick.wrapping_add(1);
        self.stamps[slot] = self.tick;
        self.hits += 1;
        Some(&self.tiles[slot])
    }

    /// Reserve the slot a tile at `key` will be filled into: the empty slot if there is one, else
    /// the least recently used. Returns the buffer to fill; the key is stamped immediately, so a
    /// failed fill must be undone with [`invalidate`](Self::invalidate).
    pub(crate) fn reserve(&mut self, key: u32) -> (usize, &mut [u8; TILE_BYTES]) {
        // `tick` wraps only after 4 · 10⁹ accesses, and the worst a wrap can do is evict a warm
        // tile once, so the shared rule needs no wrap handling here.
        let slot = lru_victim(self.keys.iter().zip(&self.stamps).map(|(&k, &stamp)| (k == EMPTY, stamp)));
        self.tick = self.tick.wrapping_add(1);
        self.stamps[slot] = self.tick;
        self.keys[slot] = key;
        self.misses += 1;
        (slot, &mut self.tiles[slot])
    }

    /// Drop a reserved slot whose fill failed, so a short or errored read can never be served as
    /// terrain.
    pub(crate) fn invalidate(&mut self, slot: usize) {
        self.keys[slot] = EMPTY;
        self.stamps[slot] = 0;
    }

    /// The tile in a slot filled by [`reserve`](Self::reserve).
    pub(crate) fn tile(&self, slot: usize) -> &[u8; TILE_BYTES] {
        &self.tiles[slot]
    }

    /// The memoized offset of cell `(i, j)`, if it is the last one resolved.
    pub(crate) fn memo(&self, cell_i: u32, cell_j: u32) -> Option<u32> {
        match self.cell_memo {
            Some((i, j, offset)) if i == cell_i && j == cell_j => Some(offset),
            _ => None,
        }
    }

    /// Remember a resolved **present** cell. Absent cells are deliberately not memoized: they cost
    /// one directory read and caching them would need a second sentinel for no measurable gain.
    pub(crate) fn remember(&mut self, cell_i: u32, cell_j: u32, offset: u32) {
        self.cell_memo = Some((cell_i, cell_j, offset));
    }

    /// Resident-tile hits and misses since the last [`clear`](Self::clear) — the number that tells
    /// an emit-time caller whether its walk has the locality the 4-slot budget assumes.
    pub fn stats(&self) -> (u32, u32) {
        (self.hits, self.misses)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh cache is empty, and a filled slot serves back exactly what was written into it.
    #[test]
    fn a_filled_slot_hits_on_its_own_key() {
        let mut cache = TileCache::<4>::new();
        assert!(cache.get(1024).is_none());
        let (slot, buf) = cache.reserve(1024);
        buf[0] = 0xAB;
        assert_eq!(cache.tile(slot)[0], 0xAB);
        assert_eq!(cache.get(1024).map(|t| t[0]), Some(0xAB));
        assert_eq!(cache.stats(), (1, 1));
    }

    /// Eviction takes the least recently *used* slot, not the least recently filled — the property
    /// a clock hand would not have.
    #[test]
    fn eviction_takes_the_least_recently_used_slot() {
        let mut cache = TileCache::<2>::new();
        cache.reserve(512).1[0] = 1;
        cache.reserve(1024).1[0] = 2;
        // Touch the older tile, then overflow: the *newer* one must be the victim.
        assert_eq!(cache.get(512).map(|t| t[0]), Some(1));
        cache.reserve(1536).1[0] = 3;
        assert_eq!(cache.get(512).map(|t| t[0]), Some(1), "the recently used tile survived");
        assert!(cache.get(1024).is_none(), "the idle tile was evicted");
        assert_eq!(cache.get(1536).map(|t| t[0]), Some(3));
    }

    #[test]
    fn a_failed_fill_leaves_no_servable_tile() {
        let mut cache = TileCache::<2>::new();
        let (slot, _) = cache.reserve(512);
        cache.invalidate(slot);
        assert!(cache.get(512).is_none());
    }

    /// The generation guard, the reason a cache can outlive a terrain file safely.
    #[test]
    fn adopting_a_new_generation_drops_the_previous_files_tiles() {
        let mut cache = TileCache::<2>::new();
        cache.adopt(7);
        cache.reserve(512).1[0] = 1;
        cache.remember(3, 4, 512);
        cache.adopt(7);
        assert!(cache.get(512).is_some(), "the same generation keeps its tiles");
        assert_eq!(cache.memo(3, 4), Some(512));
        cache.adopt(8);
        assert!(cache.get(512).is_none(), "a different file's offsets never cross-serve");
        assert_eq!(cache.memo(3, 4), None);
    }

    #[test]
    fn the_cell_memo_answers_only_for_the_cell_it_holds() {
        let mut cache = TileCache::<2>::new();
        assert_eq!(cache.memo(1, 1), None);
        cache.remember(1, 1, 2048);
        assert_eq!(cache.memo(1, 1), Some(2048));
        assert_eq!(cache.memo(1, 2), None);
    }
}
