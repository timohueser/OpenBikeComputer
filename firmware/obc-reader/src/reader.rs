//! OBCM **v5** format reader: header, style table, LOD table, and per-LOD
//! quadtree query + chunk decode.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job (see [`obc_render`]).
//!
//! The reader is immutable: instead of a stateful "active layer" it exposes the
//! parsed [`Lod`] table and takes a LOD index on every `query`/`decode_chunk`,
//! so the same `Reader` can serve different zoom levels without interior
//! mutability — friendlier for the MCU and for concurrent reads.

use heapless::Vec;

use crate::{BBox, Error};

pub const MAX_FEAT_PTS: usize = 2048;
pub const MAX_FEAT_RINGS: usize = 32;

/// The header is fixed-size; everything after it is reached via explicit offsets.
pub const HEADER_LEN: usize = 32;
/// Each LOD table entry: `max_mpp f32, index_off u32, node_count u32, chunk_size u16, chunk_count u32`.
pub const LOD_ENTRY_LEN: usize = 18;

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

/// A decoded feature owning its geometry (microdegrees). Convenience type for
/// callers that want owned data; the rendering path uses the allocation-free
/// [`Reader::for_each_feature`] instead.
#[derive(Debug, Clone)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub exterior: Vec<(i32, i32), MAX_FEAT_PTS>,
    pub interiors: Vec<Vec<(i32, i32), MAX_FEAT_PTS>, MAX_FEAT_RINGS>,
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

#[inline]
fn rd_u16(d: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([d[o], d[o + 1]])
}
#[inline]
fn rd_u32(d: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
#[inline]
fn rd_i32(d: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}
#[inline]
fn rd_f32(d: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]])
}

pub struct Reader<'a> {
    data: &'a [u8],
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
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Result<Reader<'a>, Error> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort);
        }
        if &data[0..4] != b"OBCM" {
            return Err(Error::BadMagic);
        }
        let version = data[4];
        if version != 5 {
            return Err(Error::BadVersion);
        }
        // Header field order: lat,lon,lat,lon (see serialize.py header pack).
        let min_lat = rd_i32(data, 5);
        let min_lon = rd_i32(data, 9);
        let max_lat = rd_i32(data, 13);
        let max_lon = rd_i32(data, 17);
        let style_offset = rd_u32(data, 21) as usize;
        let lod_count = data[25] as usize;
        let lod_table_offset = rd_u32(data, 26) as usize;
        let marker_color = rd_u16(data, 30);

        if style_offset < HEADER_LEN || style_offset > data.len() {
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
        if lod_table_end > data.len() {
            return Err(Error::BadOffset);
        }

        let styles = parse_styles(data, style_offset);
        let lods = parse_lod_table(data, lod_table_offset, lod_count)?;

        Ok(Reader {
            data,
            version,
            bbox: BBox { min_lon, min_lat, max_lon, max_lat },
            marker_color,
            lods,
            styles,
        })
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
        self.styles
            .iter()
            .filter_map(|s| s.as_ref())
            .min_by_key(|s| (s.z_index, s.id))
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

    #[inline]
    fn node(&self, lod: &Lod, idx: usize) -> u32 {
        rd_u32(self.data, lod.index_offset + idx * 4)
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
    pub fn query_into<const C: usize>(&self, lod: usize, view: &BBox, out: &mut Vec<(u32, BBox), C>) {
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

    fn walk_leaves<F: FnMut(u32, BBox)>(&self, lod: &Lod, idx: usize, node: BBox, view: &BBox, visit: &mut F) {
        if idx >= lod.node_count || !node.intersects(view) {
            return;
        }
        let val = self.node(lod, idx);
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
            BBox { min_lon: node.min_lon, min_lat: mid_lat, max_lon: mid_lon, max_lat: node.max_lat },
            BBox { min_lon: mid_lon, min_lat: mid_lat, max_lon: node.max_lon, max_lat: node.max_lat },
            BBox { min_lon: node.min_lon, min_lat: node.min_lat, max_lon: mid_lon, max_lat: mid_lat },
            BBox { min_lon: mid_lon, min_lat: node.min_lat, max_lon: node.max_lon, max_lat: mid_lat },
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
        if end > self.data.len() {
            return;
        }
        decode_chunk_into(&self.data[start..end], node, points, ring_lens, should_decode, visit);
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

fn parse_styles(data: &[u8], style_offset: usize) -> [Option<Style>; 256] {
    let mut styles = [None; 256];
    if style_offset >= data.len() {
        return styles;
    }
    let count = data[style_offset] as usize;
    let mut o = style_offset + 1;
    for _ in 0..count {
        if o + 6 > data.len() {
            break;
        }
        let id = data[o];
        let z_index = data[o + 1] as i8;
        let color = rd_u16(data, o + 2);
        let weight = data[o + 4];
        let flags = data[o + 5];
        let priority = (flags & 0x03) + 1;
        styles[id as usize] = Some(Style { id, z_index, color, weight, priority });
        o += 6;
    }
    styles
}

/// Parse the `lod_count` LOD-table entries; validates each layer's index/chunk
/// region lies within the file so `query`/`decode_chunk` can skip bounds math.
fn parse_lod_table(data: &[u8], offset: usize, lod_count: usize) -> Result<Vec<Lod, 16>, Error> {
    let mut lods = Vec::new();
    for k in 0..lod_count {
        let o = offset + k * LOD_ENTRY_LEN;
        let lod = Lod {
            max_mpp: rd_f32(data, o),
            index_offset: rd_u32(data, o + 4) as usize,
            node_count: rd_u32(data, o + 8) as usize,
            chunk_size: rd_u16(data, o + 12) as usize,
            chunk_count: rd_u32(data, o + 14) as usize,
        };
        // Checked: a corrupt entry can advertise a `node_count`/`chunk_count`/
        // `chunk_size` whose products wrap `usize` on the 32-bit target, so an
        // unchecked `chunks_end` could land below `data.len()` and admit a layer
        // that indexes far out of the file.
        let chunks_end = lod
            .data_start()
            .and_then(|start| {
                lod.chunk_count.checked_mul(lod.chunk_size).and_then(|len| start.checked_add(len))
            })
            .ok_or(Error::BadOffset)?;
        if lod.index_offset < HEADER_LEN || chunks_end > data.len() {
            return Err(Error::BadOffset);
        }
        let _ = lods.push(lod);
    }
    Ok(lods)
}
