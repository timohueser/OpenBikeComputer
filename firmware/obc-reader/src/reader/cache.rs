//! Caller-owned streamed-map geometry, index, and expanded-walk cache.

use core::cell::{RefCell, RefMut};

use heapless::Vec;
use obc_formats::cache::IndexBlockCache;
use obc_formats::io::{ByteSource, Error as IoError};
use obc_map_scene::BBox;

use super::{CacheError, MAX_CHUNK_BYTES};

/// Size of one geometry-chunk **cache** slot. A chunk this size or smaller is cached (kept
/// resident across the frame's two collect passes); a larger one — up to [`MAX_CHUNK_BYTES`] — is
/// decoded through the scratch without being cached. Matches the packer's default `chunk_size`
/// (4096) so the maps the device actually loads are fully cacheable.
const CACHE_SLOT_BYTES: usize = 4096;

/// Dedicated geometry-chunk buffers (each [`CACHE_SLOT_BYTES`]). The first 4 KiB of the oversized
/// decode scratch doubles as a fifth slot, so the common four/five-chunk riding views remain fully
/// resident without increasing [`MapCache`]. Replacement is scan-resistant RRIP: a view just over
/// capacity retains protected chunks instead of cyclically missing every one. Coarse overview
/// zooms can expose dozens of chunks and remain intentionally streaming on the LM20 RAM budget.
pub(crate) const MAP_CHUNK_SLOTS: usize = 4;

/// Windows of the shared [`IndexBlockCache`] the render walk gets. ≈3.5 KB: the leaf walk reads
/// 4-byte nodes whose siblings are adjacent in the file, so seven aligned windows coalesce a whole
/// walk into a handful of SD reads rather than one per node. The eighth former window's 520-byte
/// budget holds the expanded-view leaf cache below.
const INDEX_BLOCKS: usize = 7;

/// Two recent geometry walks (normally the one or two volume shards touching the viewport), each
/// retaining up to twelve `(chunk, leaf bbox)` results over a view widened by 1/8 on every side.
/// A moving camera can then reuse the list until it crosses that margin without reading the
/// quadtree again. Two 260-byte records exactly replace the eighth tagged index block: no net RAM.
const WALK_CACHE_SLOTS: usize = 2;
pub(in crate::reader) const WALK_CACHE_ENTRIES: usize = 12;

// A slot must fit any chunk it caches, and the scratch any chunk the reader accepts; `chunk_size`
// is a `u16`, so the accepted cap stays within range.
const _: () = assert!(CACHE_SLOT_BYTES <= MAX_CHUNK_BYTES, "a cached chunk must fit the scratch");
const _: () = assert!(MAX_CHUNK_BYTES <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// A snapshot of the [`Reader`](super::Reader)'s streaming counters: chunk-cache hit/miss tally and raw SD-read
/// overhead (read calls + bytes). The renderer reports the per-frame delta.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Geometry-chunk requests served from a resident slot (no SD read).
    pub chunk_hits: u32,
    /// Geometry-chunk requests that missed and read from the source.
    pub chunk_misses: u32,
    /// Total `read_at` calls to the source (index blocks + chunk fills).
    pub sd_reads: u32,
    /// Total bytes pulled from the source.
    pub bytes_read: u32,
}

/// Where `load_chunk` left a chunk's bytes: a cache slot (cacheable, ≤ `CACHE_SLOT_BYTES`) or the
/// shared decode scratch (an oversized chunk, read uncached).
#[derive(Clone, Copy)]
pub(in crate::reader) enum ChunkLoc {
    Slot(usize),
    /// The shared oversized-decode scratch doubles as a fifth 4-KiB cache slot while no oversized
    /// chunk is using it. `load_chunk` invalidates that tag before a larger read.
    Scratch,
}

#[derive(Clone, Copy)]
enum ChunkVictim {
    Slot(usize),
    Scratch,
}

/// One geometry-chunk cache slot: a resident copy of a `chunk_size`-byte chunk, tagged with its
/// `(lod, chunk_id)` and an RRIP prediction in `used`. The validity bit makes the all-zero state a
/// valid *empty* slot, so [`MapCacheInner::new`] can zero-init the whole cache.
const CHUNK_META_LOD_MASK: u8 = 0x0f;
const CHUNK_META_VALID: u8 = 0x80;

#[repr(C)]
pub(in crate::reader) struct ChunkSlot {
    pub(in crate::reader) cid: u32,
    pub(in crate::reader) used: u32,
    /// Leaf anchor captured with the chunk fill. This lets pass B decode a resident winner without
    /// repeating the whole quadtree walk merely to reconstruct the same bbox.
    pub(in crate::reader) node: BBox,
    pub(in crate::reader) len: u16,
    /// Validity plus the four-bit LOD. Packing the former fields makes each slot four bytes smaller;
    /// across four slots that funds the scratch-cache tag below without growing `MapCache`.
    pub(in crate::reader) meta: u8,
    /// The byte the shard tag used to be (FS7.5, #1420). It is spelled out rather than left to the
    /// compiler because `ChunkSlot` is `repr(C)` with a pinned size: a map is one file, so the tag
    /// had nothing left to range over, and dropping it frees a byte the alignment was already
    /// paying for. Named padding keeps the const assert below reading as a fact about the layout.
    _pad: u8,
    pub(in crate::reader) buf: [u8; CACHE_SLOT_BYTES],
}

impl ChunkSlot {
    #[inline]
    pub(in crate::reader) fn valid(&self) -> bool {
        self.meta & CHUNK_META_VALID != 0
    }

    #[inline]
    pub(in crate::reader) fn lod(&self) -> u8 {
        self.meta & CHUNK_META_LOD_MASK
    }

    #[inline]
    pub(in crate::reader) fn commit(&mut self, lod: u8) {
        debug_assert!(lod <= CHUNK_META_LOD_MASK);
        self.meta = CHUNK_META_VALID | (lod & CHUNK_META_LOD_MASK);
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<ChunkSlot>() == CACHE_SLOT_BYTES + 28);

const SCRATCH_META_LOD_MASK: u8 = 0x0f;
const SCRATCH_META_RRPV_SHIFT: u8 = 4;
const SCRATCH_META_VALID: u8 = 0x80;

/// Tag for the fifth cache slot backed by the first 4 KiB of `MapCacheInner::scratch`. It needs no
/// length or bbox: a hit is checked only after `chunk_range` supplied the current length, and the
/// caller already carries the leaf bbox used for decode.
#[repr(C)]
struct ScratchSlot {
    cid: u32,
    meta: u8,
    _reserved: [u8; 3],
}

impl ScratchSlot {
    #[inline]
    pub(in crate::reader) fn valid(&self) -> bool {
        self.meta & SCRATCH_META_VALID != 0
    }

    #[inline]
    fn lod(&self) -> u8 {
        self.meta & SCRATCH_META_LOD_MASK
    }

    #[inline]
    pub(in crate::reader) fn rrpv(&self) -> u8 {
        (self.meta >> SCRATCH_META_RRPV_SHIFT) & 0x03
    }

    #[inline]
    pub(in crate::reader) fn set_rrpv(&mut self, rrpv: u8) {
        self.meta = (self.meta & !(0x03 << SCRATCH_META_RRPV_SHIFT)) | ((rrpv & 0x03) << SCRATCH_META_RRPV_SHIFT);
    }

    #[inline]
    pub(in crate::reader) fn commit(&mut self, lod: u8, rrpv: u8) {
        debug_assert!(lod <= SCRATCH_META_LOD_MASK);
        self.meta = SCRATCH_META_VALID | (lod & SCRATCH_META_LOD_MASK) | ((rrpv & 0x03) << SCRATCH_META_RRPV_SHIFT);
    }
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<ScratchSlot>() == 8);

#[derive(Clone, Copy)]
pub(in crate::reader) struct WalkEntry {
    pub(in crate::reader) cid: u32,
    pub(in crate::reader) node: BBox,
}

/// One complete expanded-view leaf result. The all-zero form is an empty cache record, like the
/// geometry/index slots. Field order and `repr(C)` pin this to 260 B on the 32-bit target; two
/// records replace one 520-byte index window exactly.
#[repr(C)]
struct WalkCache {
    cover: BBox,
    entries: [WalkEntry; WALK_CACHE_ENTRIES],
    valid: bool,
    lod: u8,
    len: u8,
}

// An index window is its 512-byte buffer plus 8 bytes of tag and alignment.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<WalkCache>() * WALK_CACHE_SLOTS == obc_formats::cache::INDEX_BLOCK + 8);

/// The streamed-map cache: a scan-resistant five-slot geometry working set (absorbing the
/// renderer's per-priority-pass re-reads), a small block cache for quadtree-node reads, and two
/// bounded expanded-walk results. Caller-owned and reusable across frames. ≈37 KB, dominated by
/// the geometry buffers and decode scratch.
///
/// Wraps its mutable state in a `RefCell` so a [`Reader`](super::Reader) can read through it on `&self` paths; the
/// borrows are tightly scoped (one index-node read, or one chunk load + decode) so they never overlap.
pub struct MapCache {
    pub(in crate::reader) inner: RefCell<MapCacheInner>,
}

/// **`MapCache` must stay 4-aligned, and that is a measurement rather than a preference.** The
/// board `ptr::write`s this ~37 KB value into a reserved static from the boot task. At alignment 4
/// LLVM folds [`MapCacheInner::new`]'s `zeroed()` into a `.bss` memset at that placement site; at
/// alignment 8 it stops folding and materialises the whole cache as a stack temporary instead —
/// measured on the nRF54LM20 image, the boot task's frame went **7,456 → 74,240 B against a 49 KB
/// residual stack**, a boot overflow and the exact failure mode of #1084/#1108. The one field that
/// can move it is the index driver's key, which is why
/// [`IndexBlockCache`](obc_formats::cache::IndexBlockCache) tags a window by a `u32` block number
/// rather than a `u64` byte offset. CI's `boot_chain_ceiling` and `task_frame_limit` are the
/// backstop; they fire late and diagnose badly, so this fires first.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::align_of::<MapCache>() == 4);

impl Default for MapCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MapCache {
    /// A fresh, empty cache. ≈37 KB of zeroed buffers — on the device, place it once in the
    /// reserved region (e.g. `ptr::write`, like the `App`) so it stays off the main stack.
    pub fn new() -> Self {
        MapCache { inner: RefCell::new(MapCacheInner::new()) }
    }

    /// Allocate a fresh, empty cache **directly on the heap**, never on the stack.
    ///
    /// The cache is ≈37 KB, so `Box::new(MapCache::new())` first builds the whole value on the
    /// stack and then copies it — and a debug build walks `MapCacheInner::new`'s `zeroed()`
    /// interior across the stack several more un-elided-copy times — a silent overflow on a
    /// small stack (the web demo's default 1 MiB wasm shadow stack, PR #661). Like `obc-route`'s
    /// `NavScratch::new_boxed`, this owns the crate-private invariant that a zeroed allocation
    /// *is* [`MapCache::new`]:
    /// - a zeroed `MapCacheInner` is exactly `MapCacheInner::new` (which zero-inits — see
    ///   the field-by-field argument there);
    /// - the `RefCell` around it keeps its borrow state in a `Cell<isize>` whose *not borrowed*
    ///   value is 0, so the zeroed wrapper reads as unborrowed. That is a `core` implementation
    ///   detail rather than a documented guarantee — the `new_boxed_is_a_fresh_unborrowed_cache`
    ///   test is the tripwire (its first `borrow_mut` panics if this ever changes).
    ///
    /// Host-only (`alloc` feature): the device `ptr::write`s its cache into a reserved region
    /// and never calls this.
    #[cfg(feature = "alloc")]
    pub fn new_boxed() -> alloc::boxed::Box<Self> {
        // SAFETY: an all-zero `MapCache` is bit-identical to a fresh `new()` — see above.
        unsafe { alloc::boxed::Box::<Self>::new_zeroed().assume_init() }
    }

    /// Drop every resident chunk, index block, and expanded walk. Slots are keyed only inside one
    /// parse generation, so [`Reader::new`](super::Reader::new) also guards map switches via `MapCache::adopt`. Cheap
    /// — only validity metadata and counters are touched, not the backing buffers.
    pub fn clear(&self) -> Result<(), CacheError> {
        self.inner.try_borrow_mut().map_err(|_| CacheError::Busy)?.clear();
        Ok(())
    }

    /// Bind the cache to a [`MapTables`](super::MapTables) parse `generation`, running the [`MapCache::clear`] logic
    /// first if it last served a different one. Called by [`Reader::new`](super::Reader::new), which is what makes the
    /// forgotten-`clear()`-on-map-switch cross-serve impossible by construction. A zeroed cache
    /// sits at generation 0 — never a live generation — so the first adopt after boot clears an
    /// already-empty cache (harmless).
    pub(in crate::reader) fn adopt(&self, generation: u32) -> Result<(), CacheError> {
        let mut inner = self.inner.try_borrow_mut().map_err(|_| CacheError::Busy)?;
        if inner.generation != generation {
            inner.clear();
            inner.generation = generation;
        }
        Ok(())
    }

    #[inline]
    pub(in crate::reader) fn stats(&self) -> Result<CacheStats, CacheError> {
        Ok(self.inner.try_borrow().map_err(|_| CacheError::Busy)?.stats())
    }

    #[inline]
    pub(in crate::reader) fn try_borrow_mut(&self) -> Result<RefMut<'_, MapCacheInner>, CacheError> {
        self.inner.try_borrow_mut().map_err(|_| CacheError::Busy)
    }
}

/// The cache's mutable interior (see [`MapCache`]). `tick` counts geometry fills and supplies the
/// occasional protected RRIP insertion used to resist a repeated scan just over capacity.
pub(in crate::reader) struct MapCacheInner {
    /// The [`MapTables::parse`](super::MapTables::parse) generation the resident slots belong to; 0 (the zero-init state)
    /// means "unowned". Written only by [`MapCache::adopt`] — deliberately *not* reset by `clear`,
    /// which empties the slots and so is safe under any generation.
    pub(in crate::reader) generation: u32,
    tick: u32,
    pub(in crate::reader) chunks: [ChunkSlot; MAP_CHUNK_SLOTS],
    /// The shared index-block driver, seven windows wide. Its own hit/miss counters are the eight
    /// bytes the chunk tags' packing had been holding in reserve for a future field — spent here,
    /// exactly as intended, so `map_cache` stays byte-identical. They are not reported: `CacheStats`
    /// keeps its shape, and `sd_reads` already counts every fill this cache makes.
    index: IndexBlockCache<INDEX_BLOCKS>,
    walks: [WalkCache; WALK_CACHE_SLOTS],
    scratch_slot: ScratchSlot,
    /// Decode buffer for a chunk too large to cache (`> CACHE_SLOT_BYTES`, up to the accepted
    /// `MAX_CHUNK_BYTES`). Its first 4 KiB are also the fifth ordinary-chunk slot; an oversized
    /// load invalidates that tag before overwriting it.
    pub(in crate::reader) scratch: [u8; MAX_CHUNK_BYTES],
    pub(in crate::reader) chunk_hits: u32,
    chunk_misses: u32,
    sd_reads: u32,
    bytes_read: u32,
}

impl MapCacheInner {
    fn new() -> Self {
        // Zero-init via `zeroed()` (all-zero is a valid empty state for every field). This lowers
        // to a `memset` / `.bss`, whereas a struct literal zeroing the ~36 KB of buffers emits them
        // as a `.rodata` const that is then `memcpy`'d — which overflowed flash on the device.
        //
        // SAFETY: `MapCacheInner` is inhabited and valid for the all-zero bit pattern — it has no
        // references, no enums with a non-zero discriminant, and no `bool` that must be non-zero
        // (the only `bool`s are the `valid` flags, false at zero). No padding is read.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
    }

    /// Drop every resident slot and zero the counters, touching only the `valid` flags + counters,
    /// not the ≈36 KB of backing buffers. See [`MapCache::clear`].
    fn clear(&mut self) {
        for s in &mut self.chunks {
            s.meta = 0;
        }
        self.scratch_slot.meta = 0;
        self.index.reset();
        for walk in &mut self.walks {
            walk.valid = false;
        }
        self.tick = 0;
        self.chunk_hits = 0;
        self.chunk_misses = 0;
        self.sd_reads = 0;
        self.bytes_read = 0;
    }

    #[inline]
    pub(in crate::reader) fn stats(&self) -> CacheStats {
        CacheStats {
            chunk_hits: self.chunk_hits,
            chunk_misses: self.chunk_misses,
            sd_reads: self.sd_reads,
            bytes_read: self.bytes_read,
        }
    }

    #[inline]
    fn count_read(&mut self, bytes: usize) {
        count_source_read(&mut self.sd_reads, &mut self.bytes_read, bytes);
    }

    pub(in crate::reader) fn cached_walk(&self, lod: u8, query: &BBox) -> Option<Vec<WalkEntry, WALK_CACHE_ENTRIES>> {
        let slot = self.walks.iter().find(|slot| slot.valid && slot.lod == lod && bbox_contains(&slot.cover, query))?;
        let mut out = Vec::new();
        for entry in slot.entries.iter().take(slot.len as usize) {
            let _ = out.push(*entry);
        }
        Some(out)
    }

    pub(in crate::reader) fn store_walk(&mut self, lod: u8, cover: BBox, entries: &Vec<WalkEntry, WALK_CACHE_ENTRIES>) {
        let i = self
            .walks
            .iter()
            .position(|slot| slot.valid && slot.lod == lod)
            .or_else(|| self.walks.iter().position(|slot| !slot.valid))
            // Neither a matching nor an empty slot: overwrite the first. It used to key off the
            // shard index, which is what made the choice look considered; with one file there is
            // nothing to spread over and the LOD-match arm above is what does the real work.
            .unwrap_or(0);
        let slot = &mut self.walks[i];
        slot.valid = false;
        slot.cover = cover;
        for (dst, src) in slot.entries.iter_mut().zip(entries.iter()) {
            *dst = *src;
        }
        slot.lod = lod;
        slot.len = entries.len() as u8;
        slot.valid = true;
    }

    /// Ensure chunk `(lod, cid)` — the `len` bytes at `start` — is resident, returning where its
    /// bytes are. A chunk that fits a cache slot is cached across the four dedicated buffers plus
    /// the otherwise-idle decode scratch. A larger chunk invalidates that fifth tag and uses the
    /// scratch uncached. Both paths count source fills as misses and resident service as hits.
    #[allow(clippy::too_many_arguments)]
    pub(in crate::reader) fn load_chunk(
        &mut self,
        src: &dyn ByteSource,
        lod: u8,
        cid: u32,
        start: u64,
        len: usize,
        node: &BBox,
    ) -> Result<ChunkLoc, IoError> {
        if len > CACHE_SLOT_BYTES {
            self.scratch_slot.meta = 0;
            src.read_at(start, &mut self.scratch[..len])?;
            self.chunk_misses = self.chunk_misses.saturating_add(1);
            self.count_read(len);
            return Ok(ChunkLoc::Scratch);
        }
        if let Some(i) =
            self.chunks.iter().position(|s| s.valid() && s.lod() == lod && s.cid == cid && s.len as usize == len)
        {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            self.chunks[i].used = 0;
            return Ok(ChunkLoc::Slot(i));
        }
        if self.scratch_slot.valid() && self.scratch_slot.lod() == lod && self.scratch_slot.cid == cid {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            self.scratch_slot.set_rrpv(0);
            return Ok(ChunkLoc::Scratch);
        }
        let (victim, empty) = if let Some(i) = self.chunks.iter().position(|slot| !slot.valid()) {
            (ChunkVictim::Slot(i), true)
        } else if !self.scratch_slot.valid() {
            (ChunkVictim::Scratch, true)
        } else {
            (chunk_rrip_victim(&mut self.chunks, &mut self.scratch_slot), false)
        };
        // Invalidate before the read: a flaky source can fail partway, half-overwriting the buffer.
        // Committing `valid`/keys only after the read succeeds means a failed read leaves an empty
        // slot, not a poisoned one keyed to the old chunk (which would serve as a corrupt hit).
        self.tick = self.tick.wrapping_add(1);
        let rrpv = if empty || self.tick.is_multiple_of(5) { 2 } else { 3 };
        let loc = match victim {
            ChunkVictim::Slot(i) => {
                self.chunks[i].meta = 0;
                src.read_at(start, &mut self.chunks[i].buf[..len])?;
                self.chunks[i].cid = cid;
                self.chunks[i].len = len as u16;
                self.chunks[i].node = *node;
                self.chunks[i].used = rrpv;
                self.chunks[i].commit(lod);
                ChunkLoc::Slot(i)
            }
            ChunkVictim::Scratch => {
                self.scratch_slot.meta = 0;
                src.read_at(start, &mut self.scratch[..len])?;
                self.scratch_slot.cid = cid;
                self.scratch_slot.commit(lod, rrpv as u8);
                ChunkLoc::Scratch
            }
        };
        self.chunk_misses = self.chunk_misses.saturating_add(1);
        self.count_read(len);
        Ok(loc)
    }

    /// Fill `out` from index-region offset `off` through the shared index-block driver, which
    /// assembles it from resident windows and reads any that are missing.
    ///
    /// The bimodal insertion decision is this cache's, not the driver's, and it is spelled exactly
    /// as it always was: sample `sd_reads` — the counter that also counts geometry fills — *before*
    /// this fill is counted. The router samples a different counter one step later; those two
    /// phases are what a merged driver with a counter of its own would silently erase.
    pub(in crate::reader) fn index_read(
        &mut self,
        src: &dyn ByteSource,
        off: u64,
        out: &mut [u8],
    ) -> Result<(), IoError> {
        let Self { index, sd_reads, bytes_read, .. } = self;
        index.read(src, off, out, &mut |bytes, _fill| {
            let protected = sd_reads.is_multiple_of(8);
            count_source_read(sd_reads, bytes_read, bytes);
            protected
        })
    }
}

/// Bump the raw source-read counters. A free function so [`MapCacheInner::index_read`] can count
/// through a field-split borrow while the index driver holds `&mut self.index`.
#[inline]
fn count_source_read(sd_reads: &mut u32, bytes_read: &mut u32, bytes: usize) {
    *sd_reads = sd_reads.saturating_add(1);
    *bytes_read = bytes_read.saturating_add(bytes as u32);
}

fn chunk_rrip_victim(slots: &mut [ChunkSlot], scratch: &mut ScratchSlot) -> ChunkVictim {
    loop {
        if let Some(i) = slots.iter().position(|slot| slot.used >= 3) {
            return ChunkVictim::Slot(i);
        }
        if scratch.rrpv() >= 3 {
            return ChunkVictim::Scratch;
        }
        for slot in slots.iter_mut() {
            slot.used = (slot.used + 1).min(3);
        }
        scratch.set_rrpv((scratch.rrpv() + 1).min(3));
    }
}

#[inline]
fn bbox_contains(outer: &BBox, inner: &BBox) -> bool {
    outer.min_lon <= inner.min_lon
        && outer.min_lat <= inner.min_lat
        && outer.max_lon >= inner.max_lon
        && outer.max_lat >= inner.max_lat
}
