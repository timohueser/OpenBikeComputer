//! OBCM **v5** format reader: header, style table, LOD table, and per-LOD
//! quadtree query + chunk decode.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.
//!
//! The reader **streams** through a [`ByteSource`]: only the small header / style
//! table / LOD table are resident (parsed once in [`MapTables`]); the quadtree
//! index and geometry chunks are pulled on demand via `read_at`, so the whole
//! `.obcm` never has to fit in RAM. A [`SliceSource`](crate::SliceSource) makes
//! "the whole file is resident" a one-line wrapper for the sim and tests.
//!
//! Because `read_at` takes `&self`, the lazy reads go through an internal
//! [`MapCache`] behind a `RefCell`: a geometry-chunk cache (the renderer re-runs
//! `for_each_chunk` once per priority level, so this avoids re-reading a chunk per
//! pass) plus a small block cache coalescing the 4-byte quadtree-node reads. The
//! cache changes only *when* a byte is read, never *what* decodes, so renders stay
//! byte-identical.

use core::cell::{RefCell, RefMut};
use core::sync::atomic::{AtomicU32, Ordering};

use heapless::Vec;

use crate::byte_io::{ByteSource, Error as IoError};
use crate::codec::{rd_f32, rd_i32, rd_u16, rd_u32};
use crate::format::{
    BRANCH_BIT, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, STYLE_PRIORITY_MASK,
};
use crate::{BBox, Error};

/// Upper bound on the vertices of a single decoded feature — the capacity a caller
/// sizes the `points` scratch buffer to for [`Reader::for_each_feature`].
pub const MAX_FEAT_PTS: usize = 2048;
/// Upper bound on the rings (exterior + holes) of a single decoded feature — the
/// capacity for the `ring_lens` scratch buffer of [`Reader::for_each_feature`].
pub const MAX_FEAT_RINGS: usize = 32;

/// The header is fixed-size; everything after it is reached via explicit offsets.
pub const HEADER_LEN: usize = 32;
/// Each LOD table entry: `max_mpp f32, index_off u32, node_count u32, chunk_size u16, chunk_count u32`.
pub const LOD_ENTRY_LEN: usize = 18;

/// Upper bound on a single map data chunk, in bytes — the size of the decode scratch, and the
/// largest `chunk_size` the reader accepts ([`MapTables::parse`] rejects a bigger one). The
/// format stores `chunk_size` as a `u16` (≤ 65535) but real maps pack far smaller (the packer
/// defaults to 4096), so this caps the scratch below the format ceiling to save RAM. A chunk
/// between a cache slot and this decodes through the scratch, uncached.
///
/// `nrf-mem` halves the scratch (issue #270 — the map path must coexist with the BLE stack on
/// the 256 KB DK): a map packed with `chunk_size` past 8192 loads on the host/sim but is
/// rejected on the device. The packer default (4096) clears it with room; the 512 KB LM20
/// re-decides the cap.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_CHUNK_BYTES: usize = 16384;
#[cfg(feature = "nrf-mem")]
pub const MAX_CHUNK_BYTES: usize = 8192;

/// Size of one geometry-chunk **cache** slot. A chunk this size or smaller is cached (kept
/// resident across the frame's priority passes); a larger one — up to [`MAX_CHUNK_BYTES`] — is
/// decoded through the scratch without being cached. Matches the packer's default `chunk_size`
/// (4096) so the maps the device actually loads are fully cacheable.
const CACHE_SLOT_BYTES: usize = 4096;

/// Geometry-chunk cache slots (each [`CACHE_SLOT_BYTES`]). The renderer makes four priority
/// passes over the same visible-chunk set per frame, so the cache must hold the whole working set
/// or each pass re-reads every chunk (`miss ≈ chunks × 4`) and SD I/O dominates. Size it to the
/// **visible-chunk count**, not a fixed 16: the worst zooms (LOD1, ~3–4 m/px) put ~50 chunks in
/// view, so 64 slots keep them resident across all four passes and across frames (a slow pan
/// re-hits last frame's chunks). 64 × 4 KB = 256 KB.
///
/// The constrained `nrf-mem` profile drops to a single slot: a riding-zoom working set already
/// exceeded the previous 3 slots (measured: 0 hits, misses ≈ chunks × passes — the DK is
/// SD-bound either way), so extra slots bought nothing; what the cull buys is room for the
/// BLE stack next to the map path — and stack headroom under the combined build's deep ride
/// path — on the 256 KB part (issue #270). A one-chunk view still hits across passes and frames.
#[cfg(not(feature = "nrf-mem"))]
const MAP_CHUNK_SLOTS: usize = 64;
#[cfg(feature = "nrf-mem")]
const MAP_CHUNK_SLOTS: usize = 1;

/// Block size + count of the quadtree-index cache. The leaf walk reads 4-byte nodes (siblings
/// adjacent in the file); caching a few aligned blocks coalesces those into a handful of SD
/// reads per walk rather than one read per node. ≈4 KB total.
const INDEX_BLOCK: usize = 512;
const INDEX_BLOCKS: usize = 8;

// A slot must fit any chunk it caches, and the scratch any chunk the reader accepts; `chunk_size`
// is a `u16`, so the accepted cap stays within range.
const _: () = assert!(CACHE_SLOT_BYTES <= MAX_CHUNK_BYTES, "a cached chunk must fit the scratch");
const _: () = assert!(MAX_CHUNK_BYTES <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Hard cap on quadtree recursion depth in [`Reader::walk_leaves`]. A well-formed tree is far
/// shallower (the node bbox halves each level, bottoming out at the coordinate bit-width ≤32), so
/// this never rejects a real map. It matters for a *corrupt* one: once the node bbox subdivides to
/// a degenerate point the quadrants stop shrinking while `intersects(view)` stays true, so an
/// unbounded walk recurses forever → stack overflow → HardFault (no MMU guard page on the MCU).
const MAX_QUADTREE_DEPTH: u32 = 32;

#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16, // RGB565
    pub weight: u8,
    pub priority: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
}

/// One level of the LOD pyramid: a self-contained quadtree index + chunk set.
#[derive(Debug, Clone, Copy)]
pub struct Lod {
    /// Upper bound of the meters-per-pixel range this level covers; the coarsest
    /// level is `f32::INFINITY`. Strictly decreasing from coarse (0) to fine.
    pub max_mpp: f32,
    pub index_offset: usize,
    pub node_count: usize,
    pub chunk_size: usize,
    pub chunk_count: usize,
}

impl Lod {
    /// Byte offset where this level's data chunks begin (right after its index).
    /// `None` if the arithmetic overflows `usize` — reachable on the 32-bit MCU
    /// from a corrupt `index_offset`/`node_count`.
    #[inline]
    fn data_start(&self) -> Option<usize> {
        self.node_count.checked_mul(4)?.checked_add(self.index_offset)
    }

    /// Byte range `[start, end)` of chunk `chunk_id`, or `None` if `chunk_id` is out
    /// of range or any offset overflows `usize`. `chunk_id` comes straight from a
    /// quadtree leaf (arbitrary in a corrupt map), so validate against `chunk_count`
    /// with checked arithmetic to keep the 32-bit device from wrapping past the
    /// caller's file-length guard. The caller still bounds-checks `end` against the buffer.
    #[inline]
    fn chunk_range(&self, chunk_id: u32) -> Option<(usize, usize)> {
        let id = chunk_id as usize;
        if id >= self.chunk_count {
            return None;
        }
        let start = id.checked_mul(self.chunk_size)?.checked_add(self.data_start()?)?;
        let end = start.checked_add(self.chunk_size)?;
        Some((start, end))
    }
}

/// A feature decoded into caller-owned scratch buffers, borrowed for one
/// [`Reader::for_each_feature`] callback. No per-feature allocation: `points`
/// holds every ring's vertices concatenated, `ring_lens[0]` is the exterior
/// length, the rest are holes. Coordinates are microdegrees.
#[derive(Debug, Clone, Copy)]
pub struct FeatureRef<'a> {
    pub style_id: u8,
    pub kind: Kind,
    points: &'a [(i32, i32)],
    ring_lens: &'a [usize],
    bbox: BBox,
}

impl<'a> FeatureRef<'a> {
    /// Axis-aligned bounds (microdegrees) of every vertex, computed during decode.
    /// Empty for a zero-vertex feature.
    #[inline]
    pub fn bbox(&self) -> BBox {
        self.bbox
    }

    /// The exterior ring's vertices.
    #[inline]
    pub fn exterior(&self) -> &'a [(i32, i32)] {
        let n = self.ring_lens.first().copied().unwrap_or(0);
        &self.points[..n]
    }

    /// Iterator over the interior (hole) rings, if any.
    #[inline]
    pub fn interiors(&self) -> Interiors<'a> {
        let start = self.ring_lens.first().copied().unwrap_or(0);
        let rest = if self.ring_lens.is_empty() { &[][..] } else { &self.ring_lens[1..] };
        Interiors { points: self.points, lens: rest, offset: start }
    }

    /// All rings' vertices, concatenated (exterior first); partition with [`FeatureRef::ring_lens`].
    #[inline]
    pub fn points(&self) -> &'a [(i32, i32)] {
        self.points
    }

    /// Per-ring vertex counts: `[0]` exterior, `[1..]` holes.
    #[inline]
    pub fn ring_lens(&self) -> &'a [usize] {
        self.ring_lens
    }
}

/// Iterator over a feature's hole rings (see [`FeatureRef::interiors`]).
pub struct Interiors<'a> {
    points: &'a [(i32, i32)],
    lens: &'a [usize],
    offset: usize,
}

impl<'a> Iterator for Interiors<'a> {
    type Item = &'a [(i32, i32)];
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        let (&len, rest) = self.lens.split_first()?;
        self.lens = rest;
        let s = self.offset;
        self.offset += len;
        Some(&self.points[s..s + len])
    }
}

/// The OBCM header fields that describe a map without touching any geometry — a "which map is
/// this?" probe. Read cache-free via [`read_header`] (no ≈277 KB [`MapCache`]); [`MapTables::parse`]
/// parses the same prefix on its way to the full tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MapHeader {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565); see [`Reader::marker_color`].
    pub marker_color: u16,
}

/// Decode + validate the fixed 32-byte OBCM header prefix (magic, version, bbox, marker color).
/// Shared by [`read_header`] and [`MapTables::parse`] so the byte layout lives in one place.
/// Offsets follow `obc-pack`'s header pack (see OBCM_Spec.md).
fn parse_header(h: &[u8; HEADER_LEN]) -> Result<MapHeader, Error> {
    if &h[0..4] != b"OBCM" {
        return Err(Error::BadMagic);
    }
    let version = h[4];
    if version != 5 {
        return Err(Error::BadVersion);
    }
    // Header field order: lat,lon,lat,lon (see `obc-pack`'s `serialize.rs` header pack).
    let min_lat = rd_i32(h, 5);
    let min_lon = rd_i32(h, 9);
    let max_lat = rd_i32(h, 13);
    let max_lon = rd_i32(h, 17);
    let marker_color = rd_u16(h, 30);
    Ok(MapHeader { version, bbox: BBox { min_lon, min_lat, max_lon, max_lat }, marker_color })
}

/// Read just the OBCM header (version + bbox + marker color) from `src` — no style/LOD table,
/// index, or geometry, so it allocates nothing and needs no [`MapCache`]. A map shorter than the
/// header, with the wrong magic, or an unsupported version is rejected exactly as parsing would.
pub fn read_header(src: &dyn ByteSource) -> Result<MapHeader, Error> {
    if (src.len() as usize) < HEADER_LEN {
        return Err(Error::TooShort);
    }
    let mut h = [0u8; HEADER_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::TooShort)?;
    parse_header(&h)
}

/// The session-resident, immutable map tables — everything [`Reader`] needs that doesn't change
/// frame to frame: header scalars, style table, LOD pyramid. Parsed **once** per `.obcm` by
/// [`MapTables::parse`], then borrowed by a cheap per-frame [`Reader::new`]. Keeping the per-frame
/// reader ~tens of bytes (no re-parse, no 1536-byte style scratch, no per-frame style-table SD
/// read) is what keeps the deep route-load render path inside the nRF's stack reserve.
pub struct MapTables {
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565), a global header property; resolved to a device pixel
    /// by the host's color policy like style colors.
    pub marker_color: u16,
    /// LOD layers ordered coarsest (0) → finest (N-1). Always at least one.
    lods: Vec<Lod, 16>,
    /// Styles indexed by id (0..=255) for O(1) lookup during rendering.
    styles: [Option<Style>; 256],
    /// The backdrop style (bottom of the paint order; see [`Reader::backdrop_style`]), resolved
    /// once at parse so the per-frame lookup is a field read, not a 256-slot scan.
    backdrop: Option<Style>,
    /// Session-unique parse identity, never 0 (a zeroed [`MapCache`] sits at generation 0 =
    /// "unowned"). [`Reader::new`] hands it to the cache, which self-clears when it last served a
    /// different parse — the structural guard against a map switch cross-serving stale chunks.
    generation: u32,
}

impl MapTables {
    /// Parse the header scalars + style table + LOD pyramid from `src`. The one expensive,
    /// allocating step (a 1536-byte style scratch plus the style/LOD-table SD reads), so do it
    /// **once** per map and hand the result to [`Reader::new`] each frame. A map shorter than the
    /// header, with the wrong magic / version, or with out-of-range table offsets is rejected. The
    /// magic / version / bbox / marker prefix goes through the shared (private) `parse_header` (so
    /// [`read_header`] validates identically); the style + LOD-table offsets are decoded here.
    pub fn parse(src: &dyn ByteSource) -> Result<MapTables, Error> {
        let total = src.len() as usize;
        if total < HEADER_LEN {
            return Err(Error::TooShort);
        }
        let mut header = [0u8; HEADER_LEN];
        src.read_at(0, &mut header).map_err(|_| Error::TooShort)?;
        let MapHeader { version, bbox, marker_color } = parse_header(&header)?;
        let style_offset = rd_u32(&header, 21) as usize;
        let lod_count = header[25] as usize;
        let lod_table_offset = rd_u32(&header, 26) as usize;

        if style_offset < HEADER_LEN || style_offset > total {
            return Err(Error::BadOffset);
        }
        if lod_count == 0 {
            return Err(Error::BadOffset);
        }
        // Checked: `lod_table_offset` is an arbitrary header u32, so on the 32-bit
        // target the table-end can wrap `usize` and slip past the guard below.
        let lod_table_end = lod_count
            .checked_mul(LOD_ENTRY_LEN)
            .and_then(|len| lod_table_offset.checked_add(len))
            .ok_or(Error::BadOffset)?;
        if lod_table_end > total {
            return Err(Error::BadOffset);
        }

        let mut styles = [None; 256];
        parse_styles(src, style_offset, total, &mut styles)?;
        let lods = parse_lod_table(src, lod_table_offset, lod_count, total)?;
        // Resolve the backdrop (lowest `z_index`, ties broken by lowest id) once here; the table is
        // immutable after parse, so `Reader::backdrop_style` never has to re-scan the 256 slots.
        let backdrop = styles.iter().filter_map(|s| s.as_ref()).min_by_key(|s| (s.z_index, s.id)).copied();
        // Stamp a session-unique generation. `fetch_add + 1` starts the first parse at 1, so 0 is
        // never live — a zero-initialized `MapCacheInner` must always read as "unowned". `Relaxed`
        // suffices: the counter is the only shared state and only uniqueness matters.
        static GEN: AtomicU32 = AtomicU32::new(0);
        let generation = GEN.fetch_add(1, Ordering::Relaxed) + 1;
        Ok(MapTables { version, bbox, marker_color, lods, styles, backdrop, generation })
    }
}

pub struct Reader<'a> {
    /// The byte source the index + geometry chunks stream from. `&dyn` (not a generic) so
    /// signatures holding a `&Reader` need no `<S>` parameter.
    src: &'a dyn ByteSource,
    /// Header scalars, **copied** from [`MapTables`] so `reader.version` / `.bbox` / `.marker_color`
    /// field access stays direct while the big tables stay borrowed.
    pub version: u8,
    pub bbox: BBox,
    pub marker_color: u16,
    /// The session-resident immutable tables (style table + LOD pyramid), parsed once and borrowed
    /// here so a per-frame `Reader` carries no styles/lods of its own.
    tables: &'a MapTables,
    /// Borrowed lazy-read cache for the streamed index + geometry — the ≈84 KB of buffers live in
    /// the caller's [`MapCache`], reusable across frames. It keeps its own `RefCell` because
    /// `read_at` takes `&self` but the cache mutates; the borrows are tightly scoped so the
    /// index-node read and the chunk decode never overlap.
    cache: &'a MapCache,
}

impl<'a> Reader<'a> {
    /// Build a per-frame reader over the pre-parsed [`MapTables`], a fresh `src`, and a `cache` the
    /// geometry + index reads stream through. **Cheap**: borrows the tables and copies only the
    /// header scalars (no parse, no SD read). The cache is caller-owned and reusable across frames;
    /// pass a fresh [`MapCache::new`] if you don't keep one. The cache *adopts* these tables' parse
    /// generation here: building a reader over a different map's tables auto-clears the stale
    /// slots, so a map switch (a re-`parse`) can never cross-serve the old map's chunks — no manual
    /// [`MapCache::clear`] required.
    pub fn new(src: &'a dyn ByteSource, tables: &'a MapTables, cache: &'a MapCache) -> Reader<'a> {
        cache.adopt(tables.generation);
        Reader { src, version: tables.version, bbox: tables.bbox, marker_color: tables.marker_color, tables, cache }
    }

    /// Snapshot of the chunk-cache + streaming counters. Cumulative over the cache's life, so the
    /// renderer reports the per-frame delta.
    #[inline]
    pub fn chunk_cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// The parsed LOD pyramid (coarsest first).
    #[inline]
    pub fn lods(&self) -> &[Lod] {
        &self.tables.lods
    }

    #[inline]
    pub fn style(&self, id: u8) -> Option<&Style> {
        self.tables.styles.get(id as usize).and_then(|s| s.as_ref())
    }

    /// The backdrop style: the one at the bottom of the paint order (lowest
    /// `z_index`, ties broken by lowest id). By convention the map's sea/
    /// background style sits here, so its color fills the screen before any
    /// geometry is drawn. Resolved once in [`MapTables::parse`]; returns `None`
    /// only for an empty style table.
    pub fn backdrop_style(&self) -> Option<&Style> {
        self.tables.backdrop.as_ref()
    }

    /// Pick the finest LOD whose range still covers `mpp` (meters/pixel). The
    /// coarsest level (`max_mpp == +inf`) always qualifies, so the result is a
    /// valid index in `0..lods().len()`.
    pub fn select_lod_for_mpp(&self, mpp: f32) -> usize {
        let mut chosen = 0;
        for (i, lod) in self.tables.lods.iter().enumerate() {
            if lod.max_mpp >= mpp {
                chosen = i;
            }
        }
        chosen
    }

    /// Read quadtree node `idx` of `lod` (a `u32`), streamed through the index block cache. `None`
    /// on a read failure — the walk then skips that subtree. `idx < node_count` and the index
    /// region lies within the file (both guaranteed by `walk_leaves`/`parse_lod_table`), so the
    /// offset never overflows `u32`.
    #[inline]
    fn read_node(&self, lod: &Lod, idx: usize) -> Option<u32> {
        let off = (lod.index_offset + idx * 4) as u32;
        let mut b = [0u8; 4];
        self.cache.borrow_mut().index_read(self.src, off, &mut b).ok()?;
        Some(u32::from_le_bytes(b))
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf in `lod` overlapping `view`, in
    /// quadtree order. `lod` indexes [`Reader::lods`]; out-of-range visits nothing. Unlike a
    /// capacity-bounded collect, this streams through a callback with **no upper bound** on the
    /// chunk count — the renderer relies on this so a wide viewport never silently drops chunks.
    /// The walk only reads the index (bbox tests over `u32` nodes), so re-running it once per
    /// priority pass is cheap relative to decoding.
    pub fn for_each_chunk(&self, lod: usize, view: &BBox, mut visit: impl FnMut(u32, BBox)) {
        if let Some(l) = self.tables.lods.get(lod) {
            if l.node_count > 0 {
                self.walk_leaves(l, 0, self.bbox, view, 0, &mut visit);
            }
        }
    }

    fn walk_leaves<F: FnMut(u32, BBox)>(
        &self,
        lod: &Lod,
        idx: usize,
        node: BBox,
        view: &BBox,
        depth: u32,
        visit: &mut F,
    ) {
        // The depth cap is the hard stack bound against a corrupt cyclic branch (see
        // `MAX_QUADTREE_DEPTH`); a well-formed tree never reaches it.
        if idx >= lod.node_count || depth > MAX_QUADTREE_DEPTH || !node.intersects(view) {
            return;
        }
        // Read the node *before* descending/visiting so the index-cache borrow is released by the
        // time a leaf's `visit` triggers a geometry-chunk read (no nested `RefCell` borrow).
        let val = match self.read_node(lod, idx) {
            Some(v) => v,
            None => return,
        };
        if val & BRANCH_BIT == 0 {
            if val != EMPTY_LEAF {
                visit(val, node);
            }
            return;
        }
        let child = (val & !BRANCH_BIT) as usize;
        // The packer flattens the quadtree breadth-first, so a branch's children always lie after
        // it: `child > idx` is a well-formed-map invariant. A back-/self-reference (`child <= idx`)
        // only appears in a corrupt map and would re-enter a node already on the stack; reject it
        // (the depth cap above is the backstop, this stops the most direct cycle at its source).
        if child <= idx {
            return;
        }
        // Floor-division midpoints (`div_euclid` floors toward −∞) — must match the packer's
        // `quadtree.rs` split so reader and writer agree on every node bbox.
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        // NW, NE, SW, SE
        let kids = [
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.walk_leaves(lod, child + i, *kb, view, depth + 1, visit);
        }
    }

    /// Decode every feature in a chunk of `lod`, invoking `visit` once per feature with a
    /// [`FeatureRef`] borrowing the caller's `points`/`ring_lens` scratch. Allocation-free: the
    /// buffers grow to the largest feature once and are reused across features/chunks/frames.
    /// `node` is the leaf bbox yielded by [`Reader::for_each_chunk`].
    ///
    /// # Reentrancy
    ///
    /// The internal cache `RefCell` borrow is held while `visit` runs. A callback may read the
    /// resident tables ([`Reader::style`], [`Reader::backdrop_style`], [`Reader::lods`],
    /// [`Reader::select_lod_for_mpp`]) but must **not** call any `Reader` method that streams from
    /// the source — [`Reader::for_each_chunk`] or another `for_each_feature*` — which re-borrows
    /// the cache and panics at runtime with a borrow error.
    pub fn for_each_feature<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        visit: impl FnMut(FeatureRef),
    ) {
        self.for_each_feature_filtered(lod, chunk_id, node, points, ring_lens, |_| true, visit);
    }

    /// Like [`Reader::for_each_feature`], but `should_decode` is consulted with each feature's
    /// style id **before** its coordinates are decoded: `false` skips the geometry cheaply
    /// (advancing past its bytes with no coordinate math), `true` decodes it and hands a
    /// [`FeatureRef`] to `visit`. The renderer uses this so each priority pass decodes only its own
    /// features — across all passes a feature's coordinates decode **at most once per frame**.
    ///
    /// # Reentrancy
    ///
    /// The internal cache `RefCell` borrow is held while `should_decode` and `visit` run. A
    /// callback may read the resident tables ([`Reader::style`], [`Reader::backdrop_style`],
    /// [`Reader::lods`], [`Reader::select_lod_for_mpp`]) but must **not** call any `Reader` method
    /// that streams from the source — [`Reader::for_each_chunk`] or another `for_each_feature*` —
    /// which re-borrows the cache and panics at runtime with a borrow error.
    #[allow(clippy::too_many_arguments)]
    pub fn for_each_feature_filtered<const P: usize, const R: usize>(
        &self,
        lod: usize,
        chunk_id: u32,
        node: &BBox,
        points: &mut Vec<(i32, i32), P>,
        ring_lens: &mut Vec<usize, R>,
        should_decode: impl Fn(u8) -> bool,
        visit: impl FnMut(FeatureRef),
    ) {
        let l = match self.tables.lods.get(lod) {
            Some(l) => l,
            None => return,
        };
        // `chunk_id` is unvalidated file data: reject an out-of-range id or an offset overflowing
        // `usize` (32-bit on device) instead of panicking or decoding an adjacent region.
        let (start, end) = match l.chunk_range(chunk_id) {
            Some(range) => range,
            None => return,
        };
        if end > self.src.len() as usize {
            return;
        }
        // `chunk_size` was capped at `MAX_CHUNK_BYTES` in `parse`; this defensive check keeps a
        // corrupt LOD from indexing past the decode scratch.
        let len = end - start;
        if len > MAX_CHUNK_BYTES {
            return;
        }
        // Pull the chunk through the cache, then decode from the resident bytes. The borrow is held
        // across `decode_chunk_into` — safe because `should_decode`/`visit` only touch
        // `self.tables.styles`, never the cache.
        let mut cache = self.cache.borrow_mut();
        let loc = match cache.load_chunk(self.src, lod as u8, chunk_id, start as u32, len) {
            Ok(loc) => loc,
            Err(_) => return,
        };
        let chunk = match loc {
            ChunkLoc::Slot(i) => &cache.chunks[i].buf[..len],
            ChunkLoc::Scratch => &cache.scratch[..len],
        };
        decode_chunk_into(chunk, node, points, ring_lens, should_decode, visit);
    }
}

/// Running vertex bounds, accumulated as a feature decodes so its bbox is ready
/// with no extra pass over the points. Seeded inverted; widened per vertex.
#[derive(Clone, Copy)]
struct Bounds {
    min_lon: i32,
    min_lat: i32,
    max_lon: i32,
    max_lat: i32,
}

impl Bounds {
    #[inline]
    fn new() -> Self {
        Bounds { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN }
    }
    #[inline]
    fn add(&mut self, x: i32, y: i32) {
        if x < self.min_lon {
            self.min_lon = x;
        }
        if x > self.max_lon {
            self.max_lon = x;
        }
        if y < self.min_lat {
            self.min_lat = y;
        }
        if y > self.max_lat {
            self.max_lat = y;
        }
    }
    #[inline]
    fn to_bbox(self) -> BBox {
        BBox { min_lon: self.min_lon, min_lat: self.min_lat, max_lon: self.max_lon, max_lat: self.max_lat }
    }
}

/// Walk a single chunk's bytes, decoding each feature into the shared `points`/`ring_lens` buffers
/// (cleared and refilled per feature) and handing a [`FeatureRef`] to `visit`.
fn decode_chunk_into<const P: usize, const R: usize>(
    chunk: &[u8],
    node: &BBox,
    points: &mut Vec<(i32, i32), P>,
    ring_lens: &mut Vec<usize, R>,
    should_decode: impl Fn(u8) -> bool,
    mut visit: impl FnMut(FeatureRef),
) {
    let cs = chunk.len();
    let anchor_base = (node.min_lon, node.min_lat);
    let mut off = 0usize;

    while off + 12 <= cs {
        if chunk[off] == 0xFF {
            break;
        }
        let style_id = chunk[off];
        let ext_pt_count = rd_u16(chunk, off + 1) as usize;
        let ax = rd_i32(chunk, off + 3);
        let ay = rd_i32(chunk, off + 7);
        let flags = chunk[off + 11];
        off += 12;

        let is_16 = flags & FEATURE_FLAG_16BIT != 0;
        let is_poly = flags & FEATURE_FLAG_POLYGON != 0;
        let has_holes = flags & FEATURE_FLAG_HOLES != 0;
        let dsize = if is_16 { 2 } else { 1 };

        // Skip path: the caller doesn't want this style this pass, so advance
        // past the geometry without decoding. `skip_ring` mirrors `read_ring`'s
        // offset arithmetic exactly — the two must stay byte-for-byte in sync.
        if !should_decode(style_id) {
            off = skip_ring(chunk, off, ext_pt_count, false, dsize);
            if is_poly && has_holes {
                off = for_each_hole(chunk, off, |c, o, hpc| skip_ring(c, o, hpc, true, dsize));
            }
            continue;
        }

        let anchor = (anchor_base.0 + ax, anchor_base.1 + ay);

        points.clear();
        ring_lens.clear();
        let mut bounds = Bounds::new();

        off = read_ring(chunk, off, ext_pt_count, anchor, is_16, dsize, false, points, &mut bounds);
        let _ = ring_lens.push(points.len());

        if is_poly && has_holes {
            off = for_each_hole(chunk, off, |c, o, hpc| {
                let before = points.len();
                let o = read_ring(c, o, hpc, anchor, is_16, dsize, true, points, &mut bounds);
                let _ = ring_lens.push(points.len() - before);
                o
            });
        }

        visit(FeatureRef {
            style_id,
            kind: if is_poly { Kind::Polygon } else { Kind::Line },
            points,
            ring_lens,
            bbox: bounds.to_bbox(),
        });
    }
}

/// Walk a polygon's hole list: read the 1-byte hole count at `off`, then per hole read its `u16`
/// point count and hand `(chunk, off, hpc)` to `ring` (which decodes or skips and returns the
/// post-ring offset); returns the offset past the whole block. The skip and decode paths share this
/// framing so their byte arithmetic can't drift. No-op when `off` is already at the chunk end.
#[inline]
fn for_each_hole(chunk: &[u8], mut off: usize, mut ring: impl FnMut(&[u8], usize, usize) -> usize) -> usize {
    let cs = chunk.len();
    if off >= cs {
        return off;
    }
    let hole_count = chunk[off] as usize;
    off += 1;
    for _ in 0..hole_count {
        if off + 2 > cs {
            break;
        }
        let hpc = rd_u16(chunk, off) as usize;
        off += 2;
        off = ring(chunk, off, hpc);
    }
    off
}

/// Advance `off` past one ring's encoded deltas without decoding, mirroring [`read_ring`]'s offset
/// arithmetic exactly so skip and decode stay byte-aligned. `is_hole` selects the hole encoding
/// (every point a delta) vs the exterior encoding (first point is the anchor, not stored).
fn skip_ring(chunk: &[u8], off: usize, pt_count: usize, is_hole: bool, dsize: usize) -> usize {
    if pt_count == 0 {
        return off;
    }
    let num_deltas = if is_hole { pt_count } else { pt_count - 1 };
    let step = dsize * 2;
    // Common case: the whole ring fits in the chunk — one multiply, no division.
    let want = num_deltas * step;
    let remain = chunk.len().saturating_sub(off);
    if want <= remain {
        return off + want;
    }
    // Truncated ring: advance by whole delta steps only — mirrors the old loop's
    // `off + step > len ⇒ break`, so skip and decode stay byte-for-byte aligned.
    off + (remain / step) * step
}

#[allow(clippy::too_many_arguments)]
fn read_ring<const P: usize>(
    chunk: &[u8],
    mut off: usize,
    pt_count: usize,
    anchor: (i32, i32),
    is_16: bool,
    dsize: usize,
    is_hole: bool,
    out: &mut Vec<(i32, i32), P>,
    bounds: &mut Bounds,
) -> usize {
    if pt_count == 0 {
        return off;
    }
    let (mut px, mut py) = anchor;
    let num_deltas = if is_hole {
        // holes store all points as deltas (first relative to anchor)
        pt_count
    } else {
        let _ = out.push(anchor);
        bounds.add(anchor.0, anchor.1);
        pt_count - 1
    };
    for _ in 0..num_deltas {
        if off + dsize * 2 > chunk.len() {
            break;
        }
        let (dx, dy) = if is_16 {
            (
                i16::from_le_bytes([chunk[off], chunk[off + 1]]) as i32,
                i16::from_le_bytes([chunk[off + 2], chunk[off + 3]]) as i32,
            )
        } else {
            (chunk[off] as i8 as i32, chunk[off + 1] as i8 as i32)
        };
        off += dsize * 2;
        px += dx;
        py += dy;
        let _ = out.push((px, py));
        bounds.add(px, py);
    }
    off
}

/// Parse the style table, read resident from `src` at `style_offset` (file is `total` bytes) into
/// the caller's `styles` (cleared first). The table is small (≤ `1 + 256*6` bytes) so it's pulled
/// in two reads (count byte, then records). A truncated *table* is tolerated — the `o + 6 > want`
/// break stops at the last whole record rather than reading past it — but a failed *read* (flaky
/// card) or a `style_offset` at/past EOF (corrupt header) is [`Error::BadOffset`]: an all-`None`
/// table would let the map load "fine" and render nothing, with no error to surface.
///
/// Out-param + `inline(never)`, deliberately: with the array in the return value this single-call-
/// site function inlined its ~3.5 KB of scratch (the 1.5 KB record buffer plus the `Result` array
/// temporaries) into `MapTables::parse` and on into the device `main`'s **permanent** frame — every
/// stack watermark rose by ~3.8 KB and the DK's ride path overflowed (HardFault). The scratch must
/// stay in a frame that pops before `run_app` starts.
#[inline(never)]
fn parse_styles(
    src: &dyn ByteSource,
    style_offset: usize,
    total: usize,
    styles: &mut [Option<Style>; 256],
) -> Result<(), Error> {
    styles.fill(None);
    // `MapTables::parse`'s header guard admits `style_offset == total`; there is no count byte to
    // read there, so treat it as the corrupt header it is rather than a silently-empty table.
    if style_offset >= total {
        return Err(Error::BadOffset);
    }
    let mut cb = [0u8; 1];
    src.read_at(style_offset as u32, &mut cb).map_err(|_| Error::BadOffset)?;
    let count = cb[0] as usize;
    // `count*6` record bytes follow the count, clamped to what the file holds so the `o + 6 > want`
    // break below stops at the last whole record in a truncated table.
    let avail = total - (style_offset + 1);
    let want = (count * 6).min(avail);
    let mut buf = [0u8; 256 * 6];
    if want > 0 {
        src.read_at((style_offset + 1) as u32, &mut buf[..want]).map_err(|_| Error::BadOffset)?;
    }
    let mut o = 0usize;
    for _ in 0..count {
        if o + 6 > want {
            break;
        }
        let id = buf[o];
        let z_index = buf[o + 1] as i8;
        let color = rd_u16(&buf, o + 2);
        let weight = buf[o + 4];
        let flags = buf[o + 5];
        let priority = (flags & STYLE_PRIORITY_MASK) + 1;
        styles[id as usize] = Some(Style { id, z_index, color, weight, priority });
        o += 6;
    }
    Ok(())
}

/// Parse the `lod_count` LOD-table entries (resident from `src`); validates each layer's
/// index/chunk region lies within the file (`total` bytes) so `for_each_chunk`/`decode_chunk` can skip
/// bounds math, and that its `chunk_size` fits the decode scratch ([`MAX_CHUNK_BYTES`]).
fn parse_lod_table(src: &dyn ByteSource, offset: usize, lod_count: usize, total: usize) -> Result<Vec<Lod, 16>, Error> {
    let mut lods = Vec::new();
    let mut e = [0u8; LOD_ENTRY_LEN];
    for k in 0..lod_count {
        let o = offset + k * LOD_ENTRY_LEN;
        src.read_at(o as u32, &mut e).map_err(|_| Error::BadOffset)?;
        let lod = Lod {
            max_mpp: rd_f32(&e, 0),
            index_offset: rd_u32(&e, 4) as usize,
            node_count: rd_u32(&e, 8) as usize,
            chunk_size: rd_u16(&e, 12) as usize,
            chunk_count: rd_u32(&e, 14) as usize,
        };
        // Checked: a corrupt entry's `node_count`/`chunk_count`/`chunk_size` products can wrap
        // `usize` on the 32-bit target, so an unchecked `chunks_end` could land below `total` and
        // admit a layer indexing out of the file.
        let chunks_end = lod
            .data_start()
            .and_then(|start| lod.chunk_count.checked_mul(lod.chunk_size).and_then(|len| start.checked_add(len)))
            .ok_or(Error::BadOffset)?;
        if lod.index_offset < HEADER_LEN || chunks_end > total {
            return Err(Error::BadOffset);
        }
        // A chunk decodes into the resident scratch, so reject a `chunk_size` over
        // [`MAX_CHUNK_BYTES`] rather than silently dropping its geometry at render time.
        if lod.chunk_size > MAX_CHUNK_BYTES {
            return Err(Error::BadOffset);
        }
        let _ = lods.push(lod);
    }
    Ok(lods)
}

/// A snapshot of the [`Reader`]'s streaming counters: chunk-cache hit/miss tally and raw SD-read
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
enum ChunkLoc {
    Slot(usize),
    Scratch,
}

/// One geometry-chunk cache slot: a resident copy of a `chunk_size`-byte chunk, tagged with its
/// `(lod, chunk_id)` and a recency stamp for LRU eviction. `valid` (over `Option`) makes the
/// all-zero state a valid *empty* slot, so [`MapCacheInner::new`] can zero-init the whole cache.
struct ChunkSlot {
    valid: bool,
    lod: u8,
    cid: u32,
    len: usize,
    used: u32,
    buf: [u8; CACHE_SLOT_BYTES],
}

/// One quadtree-index cache block: a resident, block-aligned window of the index region. `valid`
/// plays the same all-zero-is-empty role as in [`ChunkSlot`].
struct IndexBlock {
    valid: bool,
    off: u32,
    len: usize,
    used: u32,
    buf: [u8; INDEX_BLOCK],
}

/// The streamed-map cache: an LRU set of geometry-chunk slots (absorbing the renderer's
/// per-priority-pass re-reads) plus a small block cache for the quadtree-node reads, with the
/// streaming counters. Caller-owned and reusable across frames (a chunk read one frame can hit the
/// next). ≈277 KB, dominated by the slots + decode scratch; tune the slot count / `CACHE_SLOT_BYTES`
/// against the on-device RAM budget.
///
/// Wraps its mutable state in a `RefCell` so a [`Reader`] can read through it on `&self` paths; the
/// borrows are tightly scoped (one index-node read, or one chunk load + decode) so they never overlap.
pub struct MapCache {
    inner: RefCell<MapCacheInner>,
}

impl Default for MapCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MapCache {
    /// A fresh, empty cache. ≈277 KB of zeroed buffers — on the device, place it once in the
    /// reserved region (e.g. `ptr::write`, like the `App`) so it stays off the 192 KB main stack.
    pub fn new() -> Self {
        MapCache { inner: RefCell::new(MapCacheInner::new()) }
    }

    /// Drop every resident chunk + index slot. Slots are keyed only by `(lod, chunk_id, len)` /
    /// index offset, *not* by which map produced them, so a slot left resident across a map switch
    /// would cross-serve as a (wrong-geometry) hit — but [`Reader::new`] guards that structurally
    /// via [`MapCache::adopt`], so calling this on a switch is still correct, just no longer
    /// load-bearing. Cheap — only the `valid` flags + counters are touched, not the buffers.
    pub fn clear(&self) {
        self.inner.borrow_mut().clear();
    }

    /// Bind the cache to a [`MapTables`] parse `generation`, running the [`MapCache::clear`] logic
    /// first if it last served a different one. Called by [`Reader::new`], which is what makes the
    /// forgotten-`clear()`-on-map-switch cross-serve impossible by construction. A zeroed cache
    /// sits at generation 0 — never a live generation — so the first adopt after boot clears an
    /// already-empty cache (harmless).
    fn adopt(&self, generation: u32) {
        let mut inner = self.inner.borrow_mut();
        if inner.generation != generation {
            inner.clear();
            inner.generation = generation;
        }
    }

    #[inline]
    fn borrow_mut(&self) -> RefMut<'_, MapCacheInner> {
        self.inner.borrow_mut()
    }

    #[inline]
    fn stats(&self) -> CacheStats {
        self.inner.borrow().stats()
    }
}

/// The cache's mutable interior (see [`MapCache`]). Recency is a monotonic `tick` stamped on each
/// access; eviction picks the lowest stamp.
struct MapCacheInner {
    /// The [`MapTables::parse`] generation the resident slots belong to; 0 (the zero-init state)
    /// means "unowned". Written only by [`MapCache::adopt`] — deliberately *not* reset by `clear`,
    /// which empties the slots and so is safe under any generation.
    generation: u32,
    tick: u32,
    chunks: [ChunkSlot; MAP_CHUNK_SLOTS],
    index: [IndexBlock; INDEX_BLOCKS],
    /// Decode buffer for a chunk too large to cache (`> CACHE_SLOT_BYTES`, up to the accepted
    /// `MAX_CHUNK_BYTES`); never keyed, so such a chunk is re-read every pass.
    scratch: [u8; MAX_CHUNK_BYTES],
    chunk_hits: u32,
    chunk_misses: u32,
    sd_reads: u32,
    bytes_read: u32,
}

impl MapCacheInner {
    fn new() -> Self {
        // Zero-init via `zeroed()` (all-zero is a valid empty state for every field). This lowers
        // to a `memset` / `.bss`, whereas a struct literal zeroing the ~84 KB of buffers emits them
        // as a `.rodata` const that is then `memcpy`'d — which overflowed flash on the device.
        //
        // SAFETY: `MapCacheInner` is inhabited and valid for the all-zero bit pattern — it has no
        // references, no enums with a non-zero discriminant, and no `bool` that must be non-zero
        // (the only `bool`s are the `valid` flags, false at zero). No padding is read.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
    }

    /// Drop every resident slot and zero the counters, touching only the `valid` flags + counters,
    /// not the ≈84 KB of backing buffers. See [`MapCache::clear`].
    fn clear(&mut self) {
        for s in &mut self.chunks {
            s.valid = false;
        }
        for b in &mut self.index {
            b.valid = false;
        }
        self.tick = 0;
        self.chunk_hits = 0;
        self.chunk_misses = 0;
        self.sd_reads = 0;
        self.bytes_read = 0;
    }

    #[inline]
    fn stats(&self) -> CacheStats {
        CacheStats {
            chunk_hits: self.chunk_hits,
            chunk_misses: self.chunk_misses,
            sd_reads: self.sd_reads,
            bytes_read: self.bytes_read,
        }
    }

    #[inline]
    fn touch(&mut self) -> u32 {
        self.tick = self.tick.wrapping_add(1);
        self.tick
    }

    #[inline]
    fn count_read(&mut self, bytes: usize) {
        self.sd_reads = self.sd_reads.saturating_add(1);
        self.bytes_read = self.bytes_read.saturating_add(bytes as u32);
    }

    /// Ensure chunk `(lod, cid)` — the `len` bytes at `start` — is resident, returning where its
    /// bytes are. A chunk that fits a cache slot is cached: a hit bumps recency + the hit
    /// counter, a miss evicts the LRU slot and reads from the source. A chunk larger than a slot
    /// is read into the uncached scratch every call (counted as a miss + a read).
    fn load_chunk(
        &mut self,
        src: &dyn ByteSource,
        lod: u8,
        cid: u32,
        start: u32,
        len: usize,
    ) -> Result<ChunkLoc, IoError> {
        if len > CACHE_SLOT_BYTES {
            src.read_at(start, &mut self.scratch[..len])?;
            self.chunk_misses = self.chunk_misses.saturating_add(1);
            self.count_read(len);
            return Ok(ChunkLoc::Scratch);
        }
        if let Some(i) = self.chunks.iter().position(|s| s.valid && s.lod == lod && s.cid == cid && s.len == len) {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            let t = self.touch();
            self.chunks[i].used = t;
            return Ok(ChunkLoc::Slot(i));
        }
        let i = lru(self.chunks.iter().map(|s| (!s.valid, s.used)));
        // Invalidate before the read: a flaky source can fail partway, half-overwriting the buffer.
        // Committing `valid`/keys only after the read succeeds means a failed read leaves an empty
        // slot, not a poisoned one keyed to the old chunk (which would serve as a corrupt hit).
        self.chunks[i].valid = false;
        src.read_at(start, &mut self.chunks[i].buf[..len])?;
        self.chunks[i].valid = true;
        self.chunks[i].lod = lod;
        self.chunks[i].cid = cid;
        self.chunks[i].len = len;
        let t = self.touch();
        self.chunks[i].used = t;
        self.chunk_misses = self.chunk_misses.saturating_add(1);
        self.count_read(len);
        Ok(ChunkLoc::Slot(i))
    }

    /// Fill `out` from index-region offset `off`, assembling from cached blocks (reading any
    /// missing block from the source). A node read is 4 bytes and may straddle a block edge, so
    /// this loops over blocks.
    fn index_read(&mut self, src: &dyn ByteSource, off: u32, out: &mut [u8]) -> Result<(), IoError> {
        let mut filled = 0usize;
        while filled < out.len() {
            let cur = off + filled as u32;
            let block_off = cur - cur % INDEX_BLOCK as u32;
            let slot = self.index_block(src, block_off)?;
            let within = (cur - block_off) as usize;
            let blen = self.index[slot].len;
            if within >= blen {
                return Err(IoError::BadOffset);
            }
            let take = (blen - within).min(out.len() - filled);
            out[filled..filled + take].copy_from_slice(&self.index[slot].buf[within..within + take]);
            filled += take;
        }
        Ok(())
    }

    /// Ensure the `INDEX_BLOCK`-aligned block at `block_off` is resident, returning its slot.
    fn index_block(&mut self, src: &dyn ByteSource, block_off: u32) -> Result<usize, IoError> {
        if let Some(i) = self.index.iter().position(|b| b.valid && b.off == block_off) {
            let t = self.touch();
            self.index[i].used = t;
            return Ok(i);
        }
        let want = ((src.len() - block_off) as usize).min(INDEX_BLOCK);
        if want == 0 {
            return Err(IoError::BadOffset);
        }
        let i = lru(self.index.iter().map(|b| (!b.valid, b.used)));
        // Invalidate before the read (see `load_chunk`): a partial read failure must not leave a
        // poisoned slot still keyed to the old block offset.
        self.index[i].valid = false;
        src.read_at(block_off, &mut self.index[i].buf[..want])?;
        self.index[i].valid = true;
        self.index[i].off = block_off;
        self.index[i].len = want;
        let t = self.touch();
        self.index[i].used = t;
        self.count_read(want);
        Ok(i)
    }
}

/// Pick a slot to (re)fill: the first empty slot if any, else the least-recently-used. Input is
/// `(is_empty, used)` per slot in order; returns the chosen index. Never empty in practice (the
/// cache always has ≥ 1 slot).
fn lru(slots: impl Iterator<Item = (bool, u32)>) -> usize {
    let mut best = 0usize;
    let mut best_used = u32::MAX;
    for (i, (empty, used)) in slots.enumerate() {
        if empty {
            return i;
        }
        if used < best_used {
            best_used = used;
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    /// A `ByteSource` reproducing a flaky-SD partial-overwrite: the read at offset `fail_at` copies
    /// `partial` bytes into the destination and then returns `Err` (like `SdByteSource` filling
    /// block-by-block). Every other read is filled from `data`. `SliceSource` copies in one shot and
    /// can't reproduce this.
    struct FlakySource<'a> {
        data: &'a [u8],
        fail_at: u32,
        partial: usize,
    }

    impl ByteSource for FlakySource<'_> {
        fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), IoError> {
            let start = offset as usize;
            let end = start.checked_add(buf.len()).ok_or(IoError::BadOffset)?;
            let bytes = self.data.get(start..end).ok_or(IoError::BadOffset)?;
            if offset == self.fail_at {
                let n = self.partial.min(buf.len());
                buf[..n].copy_from_slice(&bytes[..n]); // partial write, then fail
                return Err(IoError::Io);
            }
            buf.copy_from_slice(bytes);
            Ok(())
        }

        fn len(&self) -> u32 {
            self.data.len() as u32
        }
    }

    /// A read that fails partway must leave the evicted slot *empty*, not poisoned with the old key
    /// over a half-overwritten buffer — otherwise a later request for the old key is served as a
    /// corrupt hit.
    #[test]
    fn partial_read_failure_does_not_poison_evicted_slot() {
        const LEN: usize = 64;
        // One LEN-chunk per slot, plus one more past them for the failing eviction read — sized off
        // MAP_CHUNK_SLOTS so the test tracks the cache size rather than a hard-coded buffer length.
        let mut data = [0u8; (MAP_CHUNK_SLOTS + 1) * LEN];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (k as u8).wrapping_mul(31).wrapping_add(7); // distinct, offset-derived bytes
        }
        // The eviction read (K_new) lives past the primed chunks (one per slot) and fails partway.
        let fail_at = (MAP_CHUNK_SLOTS as u32) * LEN as u32;
        let src = FlakySource { data: &data, fail_at, partial: 8 };

        let cache = MapCache::new();
        let mut inner = cache.borrow_mut();

        // Prime all slots, oldest first — so the LRU victim of the next miss is slot 0 (cid 0).
        for cid in 0..MAP_CHUNK_SLOTS as u32 {
            let loc = inner.load_chunk(&src, 0, cid, cid * LEN as u32, LEN).unwrap();
            assert!(matches!(loc, ChunkLoc::Slot(_)));
        }
        let primed = inner.stats();
        assert_eq!(primed.chunk_misses, MAP_CHUNK_SLOTS as u32);
        assert_eq!(primed.chunk_hits, 0);

        // The true bytes of K_old (cid 0), for an uncorrupted-content check after re-read.
        let mut k_old = [0u8; LEN];
        src.read_at(0, &mut k_old).unwrap();

        // Eviction read of K_new fails partway through filling slot 0's buffer.
        assert!(matches!(inner.load_chunk(&src, 0, 99, fail_at, LEN), Err(IoError::Io)));

        // Request K_old again: it must be a *miss* (re-read), not a hit on the poisoned slot.
        let before = inner.stats();
        let loc = inner.load_chunk(&src, 0, 0, 0, LEN).unwrap();
        let after = inner.stats();
        assert_eq!(after.chunk_hits, before.chunk_hits, "K_old must not hit the poisoned slot");
        assert_eq!(after.chunk_misses, before.chunk_misses + 1, "K_old must be re-read");

        // …and the re-read returns the real K_old bytes, not the half-written K_new.
        match loc {
            ChunkLoc::Slot(i) => assert_eq!(&inner.chunks[i].buf[..LEN], &k_old[..]),
            ChunkLoc::Scratch => panic!("a slot-sized chunk should land in a slot"),
        }
    }

    /// Two maps sharing a chunk key `(lod, cid, len)` but holding *different* bytes must not
    /// cross-serve through a shared cache. Slots are keyed only by `(lod, cid, len)`, not the
    /// source — the guarantee lives in the generation adopt inside `Reader::new`, so this drives a
    /// map switch through the public path **without ever calling `clear()`** and asserts the
    /// same-key load misses and serves the new map's bytes.
    #[test]
    fn map_switch_without_clear_cannot_cross_serve() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};

        // Two byte-identical layouts (same style table / index / chunk_size ⇒ same chunk key and
        // offsets) whose one feature differs only in its delta — the decoded endpoint tells the
        // maps apart.
        let build = |delta: (i8, i8)| {
            build_file(
                (0, 0, 1000, 1000),
                &[(1, 3, 0xF800, 2, 3)],
                &[LodSpec {
                    max_mpp: f32::INFINITY,
                    index: vec![0],
                    chunks: vec![pad(pack_line(1, 10, 10, &[delta]), 64)],
                    chunk_size: 64,
                }],
            )
        };
        let a = build((1, 1));
        let b = build((2, 2));

        /// Decode chunk `(0, 0)` and return the feature's last exterior point + the cache stats.
        fn last_point(r: &Reader) -> ((i32, i32), CacheStats) {
            let mut points = Vec::<(i32, i32), 8>::new();
            let mut ring_lens = Vec::<usize, 2>::new();
            let mut last = (0, 0);
            let node = r.bbox;
            r.for_each_feature(0, 0, &node, &mut points, &mut ring_lens, |f| {
                last = *f.exterior().last().unwrap();
            });
            (last, r.chunk_cache_stats())
        }

        let sa = SliceSource(&a);
        let sb = SliceSource(&b);
        let ta = MapTables::parse(&sa).unwrap();
        let tb = MapTables::parse(&sb).unwrap();
        assert_ne!(ta.generation, 0, "0 must stay the unowned sentinel");
        assert_ne!(ta.generation, tb.generation, "each parse gets its own generation");

        // Map A through the shared cache: a miss that leaves A's chunk resident under key (0,0,64).
        let cache = MapCache::new();
        let ra = Reader::new(&sa, &ta, &cache);
        let (pa, stats_a) = last_point(&ra);
        assert_eq!(pa, (11, 11), "map A's geometry");
        assert_eq!((stats_a.chunk_hits, stats_a.chunk_misses), (0, 1));

        // Map B over the same cache, same chunk key, *no* `clear()`: `Reader::new` adopts B's
        // generation, so the load must miss (not hit A's resident slot) and serve B's bytes.
        let rb = Reader::new(&sb, &tb, &cache);
        let (pb, stats_b) = last_point(&rb);
        assert_eq!(pb, (12, 12), "map B's geometry, not stale A bytes");
        assert_eq!((stats_b.chunk_hits, stats_b.chunk_misses), (0, 1), "the switch must miss + re-read");
    }

    /// A style-table read that *fails* (flaky card) must surface as a parse error, not an
    /// all-`None` table that loads "fine" and renders nothing. Exercises both reads — the count
    /// byte and the record block — via a `FlakySource` failing at exactly that offset. (A
    /// physically *truncated* table is still tolerated; see `extremes.rs`.)
    #[test]
    fn failed_style_table_read_errors_map_parse() {
        use obcm_testkit::{build_file, pack_line, pad, LodSpec};
        let bytes = build_file(
            (0, 0, 1000, 1000),
            &[(1, 3, 0xF800, 2, 3)],
            &[LodSpec {
                max_mpp: f32::INFINITY,
                index: vec![0],
                chunks: vec![pad(pack_line(1, 10, 10, &[(1, 1)]), 64)],
                chunk_size: 64,
            }],
        );
        let style_off = u32::from_le_bytes(bytes[21..25].try_into().unwrap());
        // The count-byte read (at style_off), then the record-block read (at style_off + 1).
        for fail_at in [style_off, style_off + 1] {
            let src = FlakySource { data: &bytes, fail_at, partial: 0 };
            assert!(
                matches!(MapTables::parse(&src), Err(Error::BadOffset)),
                "a failed style read at {fail_at} must error the parse"
            );
        }
        // Control: the same bytes through a healthy source parse fine.
        assert!(MapTables::parse(&SliceSource(&bytes)).is_ok());
    }

    /// The index-block cache keys a block by its absolute offset into the index region, which means
    /// nothing across maps — so `clear()` must drop index blocks too, or a switched map's quadtree
    /// read at the same offset would hit a stale block. Detected via the source-read counters: a
    /// post-clear read of the same offset must re-read (a hit reads nothing).
    #[test]
    fn clear_invalidates_index_blocks() {
        let data = [0x5Au8; 1024];
        let src = SliceSource(&data);

        let cache = MapCache::new();
        let mut inner = cache.borrow_mut();

        // Resident, then a hit (no source read).
        inner.index_block(&src, 0).unwrap();
        let before = inner.stats();
        inner.index_block(&src, 0).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads, "a resident block must hit, not re-read");
        drop(inner);

        // After clear the same offset must miss and re-read from the source.
        cache.clear();
        let mut inner = cache.borrow_mut();
        let before = inner.stats();
        inner.index_block(&src, 0).unwrap();
        assert_eq!(inner.stats().sd_reads, before.sd_reads + 1, "post-clear index read must re-read");
    }

    /// A minimal 32-byte OBCM header with the given bbox/marker, enough for the cache-free
    /// [`read_header`] (no style/LOD tables, which it doesn't touch).
    fn synth_header(min_lon: i32, min_lat: i32, max_lon: i32, max_lat: i32, marker: u16) -> [u8; HEADER_LEN] {
        let mut h = [0u8; HEADER_LEN];
        h[0..4].copy_from_slice(b"OBCM");
        h[4] = 5;
        h[5..9].copy_from_slice(&min_lat.to_le_bytes()); // field order is lat,lon,lat,lon
        h[9..13].copy_from_slice(&min_lon.to_le_bytes());
        h[13..17].copy_from_slice(&max_lat.to_le_bytes());
        h[17..21].copy_from_slice(&max_lon.to_le_bytes());
        h[30..32].copy_from_slice(&marker.to_le_bytes());
        h
    }

    /// `read_header` pulls version + bbox + marker out of the header alone.
    #[test]
    fn read_header_decodes_bbox_and_marker() {
        let h = synth_header(-34, 12, 78, 56, 0xBEEF);
        let got = read_header(&SliceSource(&h)).unwrap();
        assert_eq!(
            got,
            MapHeader {
                version: 5,
                bbox: BBox { min_lon: -34, min_lat: 12, max_lon: 78, max_lat: 56 },
                marker_color: 0xBEEF
            }
        );
    }

    /// The same magic / version / length guards as `MapTables::parse` — a bad card never decodes to
    /// a bogus bbox.
    #[test]
    fn read_header_rejects_short_bad_magic_and_version() {
        assert_eq!(read_header(&SliceSource(&[0u8; HEADER_LEN - 1])), Err(Error::TooShort));
        let mut h = synth_header(0, 0, 1, 1, 0);
        h[0..4].copy_from_slice(b"NOPE");
        assert_eq!(read_header(&SliceSource(&h)), Err(Error::BadMagic));
        h[0..4].copy_from_slice(b"OBCM");
        h[4] = 4; // unsupported version
        assert_eq!(read_header(&SliceSource(&h)), Err(Error::BadVersion));
    }
}
