//! OBCM **v5** format reader: header, style table, LOD table, and per-LOD
//! quadtree query + chunk decode.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job (see [`obc_render`]).
//!
//! The reader **streams** through a [`ByteSource`] (issue #37): only the small
//! header / style table / LOD table are read resident at [`Reader::new`]; the
//! quadtree index and geometry chunks are pulled on demand via `read_at`, so the
//! whole `.obcm` never has to fit in RAM (the nRF54L has 256 KB, no SDRAM). A
//! [`SliceSource`](crate::SliceSource) makes "the whole file is resident" a
//! one-line wrapper for the simulator and tests, exactly as the route reader does.
//!
//! Because `read_at` takes `&self`, the lazy reads go through an internal
//! [`MapCache`] behind a `RefCell` — a small **geometry-chunk cache** so the
//! renderer's per-priority-pass walk (`for_each_chunk` is re-run once per level)
//! does not re-read the same chunk from SD on every pass, plus a tiny block cache
//! that coalesces the 4-byte quadtree-node reads. The cache changes only *when* a
//! byte is read, never *what* decodes, so renders stay byte-identical.

use core::cell::{RefCell, RefMut};

use heapless::Vec;

use crate::byte_io::{ByteSource, Error as IoError};
use crate::codec::{rd_f32, rd_i32, rd_u16, rd_u32};
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

/// Absolute upper bound on a single map data chunk, in bytes — the size of the decode scratch,
/// so the reader can decode **any** valid chunk. A `chunk_size` is a `u16` in the format, so
/// this is the 65535-byte ceiling rounded up: no valid map can declare a larger chunk.
pub const MAX_CHUNK_BYTES: usize = 1 << 16;

/// Size of one geometry-chunk **cache** slot. A chunk this size or smaller is cached (kept
/// resident across the frame's priority passes); a larger one — the packer's default is 4096,
/// but the format permits up to 65535 — is decoded through [`MAX_CHUNK_BYTES`] scratch without
/// being cached. Covers the default and the headroom the packer uses for a max-size feature.
const CACHE_SLOT_BYTES: usize = 8192;

/// Geometry-chunk cache slots (each [`CACHE_SLOT_BYTES`]). The renderer makes four priority
/// passes over the same visible-chunk set per frame; sizing the cache to hold a riding-zoom
/// viewport's chunks turns passes 2–4 into cache hits instead of re-reads from SD. ≈64 KB at
/// the default; tune against the on-device RAM budget.
const MAP_CHUNK_SLOTS: usize = 8;

/// Block size + count of the quadtree-index cache. The leaf walk reads 4-byte nodes (siblings
/// adjacent in the file); caching a few aligned blocks coalesces those into a handful of SD
/// reads per walk rather than one read per node. ≈4 KB total.
const INDEX_BLOCK: usize = 512;
const INDEX_BLOCKS: usize = 8;

// A cache slot's length is stored in a `u16`-range chunk; the scratch must hold any u16 chunk.
const _: () = assert!(CACHE_SLOT_BYTES <= u16::MAX as usize, "chunk_size is a u16 in the format");
const _: () = assert!(MAX_CHUNK_BYTES > u16::MAX as usize, "scratch must hold any u16 chunk_size");

const BRANCH_BIT: u32 = 0x8000_0000;
const EMPTY_LEAF: u32 = 0x7FFF_FFFF;

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
    /// when `index_offset`/`node_count` come from a corrupt file.
    #[inline]
    fn data_start(&self) -> Option<usize> {
        self.node_count.checked_mul(4)?.checked_add(self.index_offset)
    }

    /// Byte range `[start, end)` of chunk `chunk_id` within the file, or `None`
    /// if `chunk_id` is out of range or any offset arithmetic overflows `usize`.
    /// `chunk_id` comes straight from a quadtree leaf, so a corrupt or hostile
    /// map can carry an arbitrary value; validating it against `chunk_count` and
    /// computing the offset with checked arithmetic keeps the 32-bit device from
    /// wrapping past the caller's file-length guard (it must not panic on a bad
    /// map file). The caller still bounds-checks `end` against the actual buffer.
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

/// A feature decoded into caller-owned scratch buffers, borrowed for the
/// duration of one [`Reader::for_each_feature`] callback. No per-feature
/// allocation: `points` holds every ring's vertices concatenated and `ring_lens`
/// records each ring's length (`ring_lens[0]` is the exterior, the rest are
/// holes). Coordinates are microdegrees.
#[derive(Debug, Clone, Copy)]
pub struct FeatureRef<'a> {
    pub style_id: u8,
    pub kind: Kind,
    points: &'a [(i32, i32)],
    ring_lens: &'a [usize],
    bbox: BBox,
}

impl<'a> FeatureRef<'a> {
    /// Axis-aligned bounds (microdegrees) of every vertex, computed during decode
    /// (no extra pass over the points). Empty for a zero-vertex feature.
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

    /// All rings' vertices, concatenated (exterior first). Partition with
    /// [`FeatureRef::ring_lens`].
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

pub struct Reader<'a> {
    /// The byte source the index + geometry chunks stream from. `&dyn` (not a generic) so the
    /// renderer / app / screen signatures that hold a `&Reader` need no `<S>` parameter — the
    /// same monomorphic shape the route [`RouteReader`](../../obc_route/struct.RouteReader.html)
    /// uses.
    src: &'a dyn ByteSource,
    pub version: u8,
    pub bbox: BBox,
    /// User-position marker color (RGB565), a global map-presentation property
    /// stored in the header — resolved to a device pixel by the host's color
    /// policy just like style colors, then drawn by [`obc_render`].
    pub marker_color: u16,
    /// LOD layers ordered coarsest (0) → finest (N-1). Always at least one.
    lods: Vec<Lod, 16>,
    /// Styles indexed by id (0..=255) for O(1) lookup during rendering.
    styles: [Option<Style>; 256],
    /// Borrowed lazy-read cache for the streamed index + geometry. **Borrowed**, not owned, so
    /// the ≈130 KB of buffers live in a caller-provided [`MapCache`] (the device places it once
    /// in SDRAM and rebuilds the small `Reader` per frame, reusing the cache across frames; the
    /// host just makes one on the stack). `MapCache` keeps its own `RefCell` because `read_at`
    /// (and so `for_each_chunk`/`for_each_feature_filtered`) take `&self` but the cache mutates;
    /// the borrows are tightly scoped so the index-node read and the chunk decode never overlap.
    cache: &'a MapCache,
}

impl<'a> Reader<'a> {
    /// Parse the resident header / styles / LOD table from `src`, pairing the result with a
    /// `cache` the geometry + index reads stream through. The cache is caller-owned and reusable
    /// across frames (a chunk read last frame stays resident); pass a fresh [`MapCache::new`] if
    /// you don't keep one around. Both borrows live as long as the `Reader`.
    pub fn new(src: &'a dyn ByteSource, cache: &'a MapCache) -> Result<Reader<'a>, Error> {
        let total = src.len() as usize;
        if total < HEADER_LEN {
            return Err(Error::TooShort);
        }
        // The header is the one read that's fixed-size and always present; a short read here is
        // the streamed equivalent of the old `data.len() < HEADER_LEN`.
        let mut header = [0u8; HEADER_LEN];
        src.read_at(0, &mut header).map_err(|_| Error::TooShort)?;
        if &header[0..4] != b"OBCM" {
            return Err(Error::BadMagic);
        }
        let version = header[4];
        if version != 5 {
            return Err(Error::BadVersion);
        }
        // Header field order: lat,lon,lat,lon (see serialize.py header pack).
        let min_lat = rd_i32(&header, 5);
        let min_lon = rd_i32(&header, 9);
        let max_lat = rd_i32(&header, 13);
        let max_lon = rd_i32(&header, 17);
        let style_offset = rd_u32(&header, 21) as usize;
        let lod_count = header[25] as usize;
        let lod_table_offset = rd_u32(&header, 26) as usize;
        let marker_color = rd_u16(&header, 30);

        if style_offset < HEADER_LEN || style_offset > total {
            return Err(Error::BadOffset);
        }
        if lod_count == 0 {
            return Err(Error::BadOffset);
        }
        // Checked: `lod_table_offset` is an arbitrary u32 from the header, so on
        // the 32-bit target the table-end can wrap and slip past this guard.
        let lod_table_end = lod_count
            .checked_mul(LOD_ENTRY_LEN)
            .and_then(|len| lod_table_offset.checked_add(len))
            .ok_or(Error::BadOffset)?;
        if lod_table_end > total {
            return Err(Error::BadOffset);
        }

        let styles = parse_styles(src, style_offset, total);
        let lods = parse_lod_table(src, lod_table_offset, lod_count, total)?;

        Ok(Reader {
            src,
            version,
            bbox: BBox { min_lon, min_lat, max_lon, max_lat },
            marker_color,
            lods,
            styles,
            cache,
        })
    }

    /// A snapshot of the geometry-chunk cache + streaming counters of the paired [`MapCache`].
    /// The renderer reports the per-frame delta, so the host stats panel / device log can show
    /// the chunk-cache hit rate and the SD-read overhead this frame — the measured deliverables
    /// of issue #37. (Counters are cumulative over the cache's life, hence the renderer's delta.)
    #[inline]
    pub fn chunk_cache_stats(&self) -> CacheStats {
        self.cache.stats()
    }

    /// The parsed LOD pyramid (coarsest first).
    #[inline]
    pub fn lods(&self) -> &[Lod] {
        &self.lods
    }

    #[inline]
    pub fn style(&self, id: u8) -> Option<&Style> {
        self.styles.get(id as usize).and_then(|s| s.as_ref())
    }

    /// The backdrop style: the one at the bottom of the paint order (lowest
    /// `z_index`, ties broken by lowest id). By convention the map's sea/
    /// background style sits here, so its color fills the screen before any
    /// geometry is drawn. Returns `None` only for an empty style table.
    pub fn backdrop_style(&self) -> Option<&Style> {
        self.styles.iter().filter_map(|s| s.as_ref()).min_by_key(|s| (s.z_index, s.id))
    }

    /// Pick the finest LOD whose range still covers `mpp` (meters/pixel). The
    /// coarsest level (`max_mpp == +inf`) always qualifies, so the result is a
    /// valid index in `0..lods().len()`.
    pub fn select_lod_for_mpp(&self, mpp: f32) -> usize {
        let mut chosen = 0;
        for (i, lod) in self.lods.iter().enumerate() {
            if lod.max_mpp >= mpp {
                chosen = i;
            }
        }
        chosen
    }

    /// Read quadtree node `idx` of `lod` (a `u32`), streamed through the index block cache.
    /// `None` on a read failure — the walk then skips that subtree (the streamed equivalent of
    /// the old in-bounds-by-construction direct read). `idx < node_count` and the index region
    /// lies within the file (both guaranteed by `walk_leaves`/`parse_lod_table`), so the offset
    /// never overflows `u32`.
    #[inline]
    fn read_node(&self, lod: &Lod, idx: usize) -> Option<u32> {
        let off = (lod.index_offset + idx * 4) as u32;
        let mut b = [0u8; 4];
        self.cache.borrow_mut().index_read(self.src, off, &mut b).ok()?;
        Some(u32::from_le_bytes(b))
    }

    /// Collect (chunk_id, node_bbox) for every non-empty leaf in `lod` that
    /// overlaps `view`. `lod` indexes [`Reader::lods`]; out-of-range yields empty.
    ///
    /// Bounded by the buffer capacity `C`: if more leaves overlap than fit, the
    /// extras are dropped. The renderer uses [`Reader::for_each_chunk`] instead,
    /// which streams leaves through a callback with no such cap.
    pub fn query<const C: usize>(&self, lod: usize, view: &BBox) -> Vec<(u32, BBox), C> {
        let mut out = Vec::new();
        self.query_into(lod, view, &mut out);
        out
    }

    /// Like [`Reader::query`] but appends into a caller-owned buffer (cleared
    /// first), so a caller can reuse one allocation across calls.
    pub fn query_into<const C: usize>(
        &self,
        lod: usize,
        view: &BBox,
        out: &mut Vec<(u32, BBox), C>,
    ) {
        out.clear();
        self.for_each_chunk(lod, view, |cid, node| {
            let _ = out.push((cid, node));
        });
    }

    /// Visit `(chunk_id, node_bbox)` for every non-empty leaf in `lod` that
    /// overlaps `view`, in quadtree order. Unlike [`Reader::query`] this streams
    /// through a callback and so has **no upper bound** on the number of chunks:
    /// the renderer relies on this to avoid silently dropping chunks — and the
    /// high-priority features they hold — when a wide viewport overlaps many
    /// leaves. The walk only reads the index (bbox tests over `u32` nodes), so
    /// re-running it once per priority pass is cheap relative to decoding.
    pub fn for_each_chunk(&self, lod: usize, view: &BBox, mut visit: impl FnMut(u32, BBox)) {
        if let Some(l) = self.lods.get(lod) {
            if l.node_count > 0 {
                self.walk_leaves(l, 0, self.bbox, view, &mut visit);
            }
        }
    }

    fn walk_leaves<F: FnMut(u32, BBox)>(
        &self,
        lod: &Lod,
        idx: usize,
        node: BBox,
        view: &BBox,
        visit: &mut F,
    ) {
        if idx >= lod.node_count || !node.intersects(view) {
            return;
        }
        // Read the node *before* descending/visiting so the index-cache borrow is released by
        // the time a leaf's `visit` triggers a geometry-chunk read (no nested `RefCell` borrow).
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
        // floor-division midpoints to match the Python packer's `//`.
        let mid_lon = (node.min_lon + node.max_lon).div_euclid(2);
        let mid_lat = (node.min_lat + node.max_lat).div_euclid(2);
        // NW, NE, SW, SE
        let kids = [
            BBox {
                min_lon: node.min_lon,
                min_lat: mid_lat,
                max_lon: mid_lon,
                max_lat: node.max_lat,
            },
            BBox {
                min_lon: mid_lon,
                min_lat: mid_lat,
                max_lon: node.max_lon,
                max_lat: node.max_lat,
            },
            BBox {
                min_lon: node.min_lon,
                min_lat: node.min_lat,
                max_lon: mid_lon,
                max_lat: mid_lat,
            },
            BBox {
                min_lon: mid_lon,
                min_lat: node.min_lat,
                max_lon: node.max_lon,
                max_lat: mid_lat,
            },
        ];
        for (i, kb) in kids.iter().enumerate() {
            self.walk_leaves(lod, child + i, *kb, view, visit);
        }
    }

    /// Decode every feature in a chunk of `lod`, invoking `visit` once per
    /// feature with a [`FeatureRef`] borrowing the caller's `points`/`ring_lens`
    /// scratch buffers. This is the allocation-free path: the buffers grow to the
    /// largest feature once and are reused for every feature, chunk and frame, so
    /// steady-state rendering does no heap work here. `node` is the leaf bbox
    /// from [`Reader::query`].
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

    /// Like [`Reader::for_each_feature`], but `should_decode` is consulted with
    /// each feature's style id **before** its coordinates are decoded: return
    /// `false` to skip the feature's geometry cheaply — advancing past its bytes
    /// with no coordinate math or buffer writes — or `true` to decode it and hand
    /// a [`FeatureRef`] to `visit`. The renderer uses this so each priority pass
    /// decodes only the features at its level: across all passes, a feature's
    /// coordinates are decoded **at most once per frame** rather than once per
    /// pass.
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
        let l = match self.lods.get(lod) {
            Some(l) => l,
            None => return,
        };
        // `chunk_id` is unvalidated file data: reject an out-of-range id or any
        // offset that overflows `usize` (32-bit on device) instead of panicking
        // or decoding an adjacent region.
        let (start, end) = match l.chunk_range(chunk_id) {
            Some(range) => range,
            None => return,
        };
        if end > self.src.len() as usize {
            return;
        }
        // `chunk_size` (== `end - start`) is a `u16`, so it always fits the decode scratch; this
        // defensive check just keeps a corrupt LOD from indexing past it.
        let len = end - start;
        if len > MAX_CHUNK_BYTES {
            return;
        }
        // Pull the chunk through the cache (a hit if a prior priority pass read it this frame),
        // then decode from the resident bytes. The borrow is held across `decode_chunk_into` —
        // safe because `should_decode`/`visit` only touch `self.styles`, never the cache.
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
        BBox {
            min_lon: self.min_lon,
            min_lat: self.min_lat,
            max_lon: self.max_lon,
            max_lat: self.max_lat,
        }
    }
}

/// Walk a single chunk's bytes, decoding each feature into the shared
/// `points`/`ring_lens` buffers and handing a [`FeatureRef`] to `visit`. The
/// buffers are cleared and refilled per feature, so the same allocation serves
/// every feature in the chunk.
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

        let is_16 = flags & 0x01 != 0;
        let is_poly = flags & 0x02 != 0;
        let has_holes = flags & 0x04 != 0;
        let dsize = if is_16 { 2 } else { 1 };

        // Skip path: the caller doesn't want this style this pass, so advance
        // past the geometry without decoding. `skip_ring` mirrors `read_ring`'s
        // offset arithmetic exactly — the two must stay byte-for-byte in sync.
        if !should_decode(style_id) {
            off = skip_ring(chunk, off, ext_pt_count, false, dsize);
            if is_poly && has_holes && off < cs {
                let hole_count = chunk[off] as usize;
                off += 1;
                for _ in 0..hole_count {
                    if off + 2 > cs {
                        break;
                    }
                    let hpc = rd_u16(chunk, off) as usize;
                    off += 2;
                    off = skip_ring(chunk, off, hpc, true, dsize);
                }
            }
            continue;
        }

        let anchor = (anchor_base.0 + ax, anchor_base.1 + ay);

        points.clear();
        ring_lens.clear();
        let mut bounds = Bounds::new();

        off = read_ring(chunk, off, ext_pt_count, anchor, is_16, dsize, false, points, &mut bounds);
        let _ = ring_lens.push(points.len());

        if is_poly && has_holes && off < cs {
            let hole_count = chunk[off] as usize;
            off += 1;
            for _ in 0..hole_count {
                if off + 2 > cs {
                    break;
                }
                let hpc = rd_u16(chunk, off) as usize;
                off += 2;
                let before = points.len();
                off = read_ring(chunk, off, hpc, anchor, is_16, dsize, true, points, &mut bounds);
                let _ = ring_lens.push(points.len() - before);
            }
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

/// Advance `off` past one ring's encoded deltas without decoding them, mirroring
/// [`read_ring`]'s offset arithmetic exactly so the skip and decode paths stay
/// byte-for-byte aligned. `is_hole` selects the hole encoding (every point is a
/// delta) vs the exterior encoding (the first point is the anchor, not stored).
fn skip_ring(chunk: &[u8], mut off: usize, pt_count: usize, is_hole: bool, dsize: usize) -> usize {
    if pt_count == 0 {
        return off;
    }
    let num_deltas = if is_hole { pt_count } else { pt_count - 1 };
    for _ in 0..num_deltas {
        if off + dsize * 2 > chunk.len() {
            break;
        }
        off += dsize * 2;
    }
    off
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

/// Parse the style table, read resident from `src` at `style_offset` (the file is `total`
/// bytes). The table is small (≤ `1 + 256*6` bytes) so it's pulled in two reads — the count
/// byte, then the records — and parsed exactly as the old in-memory path (same per-record
/// in-bounds break, so a truncated table yields the same styles).
fn parse_styles(src: &dyn ByteSource, style_offset: usize, total: usize) -> [Option<Style>; 256] {
    let mut styles = [None; 256];
    if style_offset >= total {
        return styles;
    }
    let mut cb = [0u8; 1];
    if src.read_at(style_offset as u32, &mut cb).is_err() {
        return styles;
    }
    let count = cb[0] as usize;
    // `count*6` record bytes follow the count, clamped to what the file actually holds — so the
    // `o + 6 > want` break below fires at the same record the old `o + 6 > data.len()` did.
    let avail = total - (style_offset + 1);
    let want = (count * 6).min(avail);
    let mut buf = [0u8; 256 * 6];
    if want > 0 && src.read_at((style_offset + 1) as u32, &mut buf[..want]).is_err() {
        return styles;
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
        let priority = (flags & 0x03) + 1;
        styles[id as usize] = Some(Style { id, z_index, color, weight, priority });
        o += 6;
    }
    styles
}

/// Parse the `lod_count` LOD-table entries (read resident from `src`); validates each layer's
/// index/chunk region lies within the file (`total` bytes) so `query`/`decode_chunk` can skip
/// bounds math, and that its `chunk_size` fits a cache slot ([`MAX_CHUNK_BYTES`], issue #37).
fn parse_lod_table(
    src: &dyn ByteSource,
    offset: usize,
    lod_count: usize,
    total: usize,
) -> Result<Vec<Lod, 16>, Error> {
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
        // Checked: a corrupt entry can advertise a `node_count`/`chunk_count`/
        // `chunk_size` whose products wrap `usize` on the 32-bit target, so an
        // unchecked `chunks_end` could land below `total` and admit a layer
        // that indexes far out of the file.
        let chunks_end = lod
            .data_start()
            .and_then(|start| {
                lod.chunk_count.checked_mul(lod.chunk_size).and_then(|len| start.checked_add(len))
            })
            .ok_or(Error::BadOffset)?;
        if lod.index_offset < HEADER_LEN || chunks_end > total {
            return Err(Error::BadOffset);
        }
        // No `chunk_size` ceiling check: it's a `u16`, so it always fits the [`MAX_CHUNK_BYTES`]
        // decode scratch — a chunk larger than a cache slot is decoded uncached, not rejected.
        let _ = lods.push(lod);
    }
    Ok(lods)
}

/// A snapshot of the [`Reader`]'s streaming counters: the geometry-chunk cache hit/miss tally
/// and the raw SD-read overhead (read calls + bytes). Issue #37's measured deliverables — the
/// renderer reports the per-frame delta to the host stats panel / device log.
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
/// `(lod, chunk_id)` and a recency stamp for LRU eviction. `valid` distinguishes a loaded slot
/// from an empty one — chosen over `Option` so the all-zero state is a valid *empty* slot, which
/// lets [`MapCacheInner::new`] zero-init the whole cache (see there).
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

/// The streamed-map cache: an LRU set of geometry-chunk slots (the issue-#37 chunk cache that
/// absorbs the renderer's per-priority-pass re-reads) plus a small block cache for the
/// quadtree-node reads, with the streaming counters. Caller-owned and reusable across frames —
/// the device places one in SDRAM for the whole session and rebuilds the small [`Reader`] each
/// frame against it (so a chunk read one frame can hit the next), while the host just makes one
/// per render. ≈130 KB, dominated by the slots + the decode scratch; tune the slot count /
/// `CACHE_SLOT_BYTES` against the on-device RAM budget.
///
/// Wraps its mutable state in a `RefCell` so a [`Reader`] can borrow it (`&MapCache`) yet read
/// through it on `&self` paths; the borrows are tightly scoped (one index-node read, or one chunk
/// load + decode) so they never overlap.
pub struct MapCache {
    inner: RefCell<MapCacheInner>,
}

impl Default for MapCache {
    fn default() -> Self {
        Self::new()
    }
}

impl MapCache {
    /// A fresh, empty cache. ≈130 KB of zeroed buffers — on the device, place it once in SDRAM
    /// (e.g. `ptr::write`, like the `App`) so it stays off the 192 KB main stack.
    pub fn new() -> Self {
        MapCache { inner: RefCell::new(MapCacheInner::new()) }
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
    tick: u32,
    chunks: [ChunkSlot; MAP_CHUNK_SLOTS],
    index: [IndexBlock; INDEX_BLOCKS],
    /// Decode buffer for a chunk too large to cache (`> CACHE_SLOT_BYTES`). Sized to the format
    /// ceiling so any valid map decodes; never keyed, so such a chunk is re-read every pass.
    scratch: [u8; MAX_CHUNK_BYTES],
    chunk_hits: u32,
    chunk_misses: u32,
    sd_reads: u32,
    bytes_read: u32,
}

impl MapCacheInner {
    fn new() -> Self {
        // Zero-init the whole thing. Every field is valid when all-zero — `valid: false` (empty
        // slot), integer 0, and write-before-read byte buffers — and `zeroed()` lowers to a
        // `memset` / `.bss`, whereas a struct literal that zeroes the ~130 KB of buffers emits
        // them as a `.rodata` const that is then `memcpy`'d. On the device that const overflowed
        // flash; the simulator/tests are unaffected either way (the value is identical).
        //
        // SAFETY: `MapCacheInner` is inhabited and valid for the all-zero bit pattern — it has no
        // references, no enums with a non-zero discriminant, and no `bool` that must be non-zero
        // (the only `bool`s are the `valid` flags, false at zero). No padding is read.
        unsafe { core::mem::MaybeUninit::zeroed().assume_init() }
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
        if let Some(i) =
            self.chunks.iter().position(|s| s.valid && s.lod == lod && s.cid == cid && s.len == len)
        {
            self.chunk_hits = self.chunk_hits.saturating_add(1);
            let t = self.touch();
            self.chunks[i].used = t;
            return Ok(ChunkLoc::Slot(i));
        }
        let i = lru(self.chunks.iter().map(|s| (!s.valid, s.used)));
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
    fn index_read(
        &mut self,
        src: &dyn ByteSource,
        off: u32,
        out: &mut [u8],
    ) -> Result<(), IoError> {
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
            out[filled..filled + take]
                .copy_from_slice(&self.index[slot].buf[within..within + take]);
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
