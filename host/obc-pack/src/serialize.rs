//! OBCM serializer: lay out the `.obcm` bytes per `OBCM_Spec.md`.
//!
//! Deterministic: the same feature list and quadtree give the same output. Geometry arrives already
//! clipped and simplified; this module rounds lon/lat to microdegrees (round-half-to-even),
//! densifies long segments, delta-encodes rings, and lays out the chunk, offset-table, index,
//! LOD-table and header bytes. The POI and nav-graph sections reuse the geometry tree's BFS flatten
//! and `u32` node encoding.

use std::convert::Infallible;
use std::io::{self, Seek, SeekFrom, Write};

use obc_formats::obcm::{
    BRANCH_BIT, CHUNK_END, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, FEATURE_FLAG_WIDE,
    FEATURE_HEADER_COMPACT_LEN, MAGIC, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT,
    STYLE_PRIORITY_MASK, STYLE_RECORD_LEN, STYLE_TERRAIN_LAYER_BIT, STYLE_TICKED_BIT,
};

use obc_formats::obcm::{
    nav_edge_id, nav_index_padding, settlement_class_of, OffsetScale, UnitWriter, FILLER, HEADER_LEN, LOD_ENTRY_LEN,
    NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN, NAV_EDGE_MAX_CHUNKS, NAV_EDGE_MAX_RECORDS_PER_CHUNK,
    NAV_MAX_DEGREE, NAV_MAX_PROFILES, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN,
    NAV_PROFILE_RESERVED_LEN, NAV_SNAP_ANCHOR_GAP_M, NAV_SNAP_EDGE_MIN_M, NAV_SNAP_RECORD_LEN, POI_CAT_ENTRY_LEN,
    POI_CHUNK_SIZE, POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE, POI_NAME_LEN, POI_RECORD_LEN, SETTLEMENT_CATEGORY_ID,
    SUMMIT_CATEGORY_ID, SUMMIT_ELEVATION_UNKNOWN, SUMMIT_SUBTYPE_ID, VERSION as OBCM_VERSION,
};

/// The `Offset Scale` every `.obcm` this packer writes carries: `U = 16`, a 64 GiB addressable
/// interior. A constant rather than a knob, which pins the byte for determinism.
pub const SCALE: OffsetScale = OffsetScale::DEFAULT;

/// The next unit boundary at or after `cursor`. Every structure a header or directory offset
/// reaches begins on one; the bytes this rounds past are [`FILLER`].
///
/// The writers below reach their boundaries through [`UnitWriter::begin_section`]. This spelling is
/// for the two places that need the boundary without having a cursor there.
#[inline]
fn align_up(cursor: usize) -> usize {
    SCALE.align_up(cursor as u64).expect("a layout cursor never approaches u64::MAX") as usize
}

/// The `uint32` a scaled offset field stores for byte offset `at`.
///
/// A scaled offset cannot name a byte that is not a multiple of `U`, so a non-boundary argument is a
/// bug in the layout above it, not a rounding request — hence the panic rather than a silent round.
#[inline]
fn scaled(at: usize) -> u32 {
    SCALE
        .scaled(at as u64)
        .unwrap_or_else(|| panic!("byte {at} is not on a {}-byte unit boundary", SCALE.unit()))
        .units()
}

/// Lay bytes out through a [`UnitWriter`] over an in-memory buffer. `at` is the absolute file byte
/// the buffer's first byte lands on, so the cursor finds the boundaries where the reader will look
/// for them rather than where the buffer happens to start.
fn lay_out<T>(at: usize, build: impl FnOnce(&mut UnitWriter<'_, Infallible>) -> Result<T, Infallible>) -> (Vec<u8>, T) {
    let mut buf: Vec<u8> = Vec::new();
    let value = {
        let mut sink = |bytes: &[u8]| -> Result<(), Infallible> {
            buf.extend_from_slice(bytes);
            Ok(())
        };
        match build(&mut UnitWriter::new(SCALE, at as u64, &mut sink)) {
            Ok(value) => value,
            Err(never) => match never {},
        }
    };
    (buf, value)
}

/// Walk a layout with a sink that keeps nothing but the cursor: the projection of a section, run
/// through the very code that emits it. `place` sees exactly the byte lengths the write will, so
/// what it reports and what gets written cannot be two different layouts.
fn place<T>(at: usize, walk: impl FnOnce(&mut UnitWriter<'_, Infallible>) -> Result<T, Infallible>) -> T {
    let mut discard = |_: &[u8]| -> Result<(), Infallible> { Ok(()) };
    match walk(&mut UnitWriter::new(SCALE, at as u64, &mut discard)) {
        Ok(value) => value,
        Err(never) => match never {},
    }
}

use crate::config::LineStyle;
use crate::nav::{polyline_len_m, NavGraph};
use crate::poi::{table_row, Poi};
use obc_elevation::ElevationSource;
use obc_map_scene::ground_dist_m;

/// Max delta (microdegrees) before a segment is densified to keep deltas in 16-bit range.
/// Crate-visible so `geom::packed_size_budget` can count the midpoints `densify` will insert.
pub(crate) const MAX_SEGMENT: i64 = 30_000;

/// Largest safe first delta from a feature's exterior anchor to a hole vertex. Unlike a real ring
/// edge this jump must never be densified: inserted points would become part of the hole boundary.
/// It is the symmetric positive `i16` limit, because anchor selection reasons about unsigned
/// Chebyshev distance.
pub(crate) const MAX_HOLE_ANCHOR_DELTA: i64 = i16::MAX as i64;

// The serializer's blob length must equal `hours.rs`'s `Schedule::encode` width, or the pool bytes
// and the `POI_HOURS_BLOB_LEN` the directory advertises disagree.
const _: () = assert!(POI_HOURS_BLOB_LEN == crate::hours::BLOB_LEN, "hours blob length must match hours.rs");

// A cap-degree record must fit one chunk, or `pack_nav_chunk` would drop real junctions.
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);

/// Max endpoint-to-endpoint µdeg delta (lat or lon) a serialized adjacency piece may span, so every
/// neighbor entry's `dlat`/`dlon` fits `i16`. The serializer's long-edge split re-checks it, because
/// splitting a densified polyline into pieces can otherwise land a synthetic junction farther than
/// `i16` from its neighbor.
const NAV_MAX_NEIGHBOR_DELTA: i64 = 32_000;

// The reader treats `profile_count == 0` as malformed, so the packer must never write zero.
const _: () = assert!(NAV_MAX_PROFILES <= u8::MAX as usize);

/// Max polyline points of one serialized edge record: a record must never straddle a chunk
/// boundary, so it is bounded by the chunk itself. An edge whose densified polyline is longer, or
/// whose pieces would span more than [`NAV_MAX_NEIGHBOR_DELTA`], is split at a vertex into pieces
/// joined by synthetic degree-2 nodes — routing-neutral, and one chunk-sized read per edge.
pub const NAV_MAX_EDGE_PTS: usize = (NAV_CHUNK_SIZE - NAV_EDGE_FIXED_LEN) / 4 + 1;

/// A per-map routing profile ready to serialize: a display name plus the two multiplier tables in
/// `u8` fixed-point 1/16, indexed by highway class and surface class (`16` = 1.0x, `0` = forbidden).
/// Built and validated in [`crate::config`], where every non-zero multiplier is at least 16 so the
/// great-circle A* heuristic stays admissible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavProfile {
    /// Display name (UTF-8), truncated to [`NAV_PROFILE_NAME_LEN`] bytes on write, `0xFF`-padded.
    pub name: String,
    /// Multiplier per highway class (5-bit index, 0..=31). `16` = 1.0×, `0` = forbidden.
    pub highway: [u8; 32],
    /// Multiplier per surface class (3-bit index, 0..=7). Same encoding.
    pub surface: [u8; 8],
    /// Flat metres charged per metre of a neighbor entry's `Ascent M`. `0` = climb-blind. Needs no
    /// admissibility bound: the term is additive and non-negative.
    pub climb_weight: u8,
}

/// Largest `chunk_size` (bytes) that keeps every feature within the reader's
/// [`obc_reader::MAX_FEAT_PTS`] vertex cap. A feature's packed bytes are at least
/// `FEATURE_HEADER_COMPACT_LEN + 2 * (total_vertices - 1)`, so a chunk of `chunk_size` bytes carries
/// at most `(chunk_size - 7) / 2 + 1` vertices. Above this the reader silently truncates past-cap
/// vertices and the feature's fill or stroke is corrupt.
pub const MAX_SAFE_CHUNK_SIZE: usize = (obc_reader::MAX_FEAT_PTS - 1) * 2 + FEATURE_HEADER_COMPACT_LEN;

// The safe ceiling must itself fit the on-wire `u16` chunk_size field, or the bound is moot.
const _: () = assert!(MAX_SAFE_CHUNK_SIZE <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Smallest accepted `chunk_size` (bytes). The format decodes any positive size, but below this
/// even modest features exceed the chunk and [`pack_chunk`] drops them wholesale, so the pack
/// "succeeds" and the map is silently near-empty. Kept in lock-step with the schema's
/// `chunk_size.minimum`.
pub const MIN_CHUNK_SIZE: usize = 256;

/// Reject a `chunk_size` outside [`MIN_CHUNK_SIZE`]..=[`MAX_SAFE_CHUNK_SIZE`]: above the max the
/// reader silently truncates vertices, below the min features get dropped wholesale.
pub fn validate_chunk_size(chunk_size: usize) -> Result<(), String> {
    if chunk_size > MAX_SAFE_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} exceeds the safe maximum {MAX_SAFE_CHUNK_SIZE}: a single feature \
             could then pack more than {} vertices, which the device reader silently truncates \
             (issue #2). Lower chunk_size, or raise the LOD's simplify tolerance.",
            obc_reader::MAX_FEAT_PTS
        ));
    }
    if chunk_size < MIN_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} is below the minimum {MIN_CHUNK_SIZE}: features larger than the \
             chunk are dropped at pack time, so a tiny chunk_size produces a mostly-empty map."
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
}

/// A style record as packed into the Style Table (`pack_style_dict`).
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    /// Priority 1..=4; clamped to that range on pack.
    pub priority: u8,
    /// Line stroke style. Polygons ignore it.
    pub line_style: LineStyle,
    /// Optional RGB565 secondary color. `None` clears the flag bit and writes `0x0000`, which the
    /// reader ignores — black is a legit color, not a sentinel.
    pub color2: Option<u16>,
    /// Fixed width: `weight` is device pixels, off the renderer's zoom ramp.
    pub fixed_width: bool,
    /// Terrain layer: written here, consumed by the device's Settings toggle.
    pub terrain_layer: bool,
}

/// f64 lon/lat, rounded to microdegrees and densified here. `rings[0]` is the
/// exterior; `rings[1..]` are interior rings (polygons only). Lines carry one ring.
#[derive(Debug, Clone)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub rings: Vec<Vec<(f64, f64)>>,
}

/// Child bboxes are re-derived by the reader, so branches store only their four
/// children (order NW, NE, SW, SE); only leaf bboxes are kept (for anchors).
#[derive(Debug, Clone)]
pub enum Node {
    Leaf {
        /// (min_lon, min_lat, max_lon, max_lat) in microdegrees.
        bbox: (i64, i64, i64, i64),
        features: Vec<Feature>,
    },
    Branch(Box<[Node; 4]>),
}

/// One LOD layer to serialize: its quadtree root + per-layer chunk size and the
/// m/px upper bound (`None` ⇒ `+inf`, the coarsest layer).
#[derive(Debug, Clone)]
pub struct LodLayer {
    pub max_mpp: Option<f64>,
    pub chunk_size: usize,
    pub root: Node,
}

/// `v * 1e6` to microdegrees, round-half-to-even — NOT `f64::round` (half-away-from-zero), which
/// would shift vertices by a microdegree. The value is integer-valued before the `as i64`, so the
/// cast is exact.
#[inline]
pub(crate) fn to_udeg(v: f64) -> i64 {
    (v * 1e6).round_ties_even() as i64
}

/// Round one geometry ring exactly as [`pack_feature`] will and remove vertices that carry no
/// integer geometry. Crate-visible so the quadtree enforces the hole-anchor invariant on the exact
/// coordinates the serializer will see.
pub(crate) fn canonical_ring_udeg(ring: &[(f64, f64)], closed: bool) -> Vec<(i64, i64)> {
    let mut points: Vec<(i64, i64)> = ring.iter().map(|&(lon, lat)| (to_udeg(lon), to_udeg(lat))).collect();
    if closed && points.len() > 1 && points.first() == points.last() {
        points.pop();
    }
    remove_redundant_vertices(&mut points, closed);
    points
}

#[inline]
pub(crate) fn coordinate_delta(a: (i64, i64), b: (i64, i64)) -> i64 {
    (b.0 - a.0).abs().max((b.1 - a.1).abs())
}

/// Find the exterior vertex that minimizes the worst first-delta distance to all holes. Closed rings
/// are cyclic, so choosing a different first vertex is lossless. The returned distance is exact:
/// every hole is rotated to its own closest vertex after the exterior is rotated to `index`.
pub(crate) fn best_exterior_anchor(exterior: &[(i64, i64)], interiors: &[Vec<(i64, i64)>]) -> Option<(usize, i64)> {
    exterior
        .iter()
        .enumerate()
        .map(|(index, &anchor)| {
            let worst = interiors
                .iter()
                .map(|hole| hole.iter().map(|&point| coordinate_delta(anchor, point)).min().unwrap_or(i64::MAX))
                .max()
                .unwrap_or(0);
            (index, worst)
        })
        .min_by_key(|&(index, worst)| (worst, index))
}

/// Rotate a closed ring to the vertex needing the shortest first delta from the feature anchor.
/// Rotation is geometry-preserving and often avoids a quadtree split for a hole near one side of a
/// large exterior. Returns that shortest Chebyshev delta in microdegrees.
fn rotate_ring_near_anchor(points: &mut [(i64, i64)], anchor: (i64, i64)) -> i64 {
    let Some((index, distance)) = points
        .iter()
        .enumerate()
        .map(|(index, &point)| (index, coordinate_delta(anchor, point)))
        .min_by_key(|&(_, distance)| distance)
    else {
        return i64::MAX;
    };
    points.rotate_left(index);
    distance
}

/// Append intermediate points between `p1` and `p2` (then `p2`) so no single (dx, dy) step exceeds
/// the 16-bit delta range, using an integer step count and banker's-rounded midpoints.
fn densify(p1: (i64, i64), p2: (i64, i64), out: &mut Vec<(i64, i64)>) {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    let max_dist = dx.abs().max(dy.abs());
    if max_dist > MAX_SEGMENT {
        let steps = max_dist / MAX_SEGMENT + 1;
        for step in 1..steps {
            let t = step as f64 / steps as f64;
            out.push((
                (p1.0 as f64 + dx as f64 * t).round_ties_even() as i64,
                (p1.1 as f64 + dy as f64 * t).round_ties_even() as i64,
            ));
        }
    }
    out.push(p2);
}

#[inline]
fn push_deltas(data: &mut Vec<u8>, deltas: &[i64], is16: bool) {
    if is16 {
        for &d in deltas {
            data.extend_from_slice(&(d as i16).to_le_bytes());
        }
    } else {
        for &d in deltas {
            data.push(d as i8 as u8);
        }
    }
}

/// Pack the style table: `Count(u8)` then one record per style, sorted by id. Bit 7 of the flags
/// stays reserved and is written `0`.
pub fn pack_style_dict(styles: &[Style]) -> Vec<u8> {
    let mut styles = styles.to_vec();
    styles.sort_by_key(|s| s.id);
    let mut data = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    data.push(styles.len() as u8);
    for s in &styles {
        let priority = (s.priority as i32).clamp(1, 4);
        let mut flags = (priority - 1) as u8 & STYLE_PRIORITY_MASK;
        match s.line_style {
            LineStyle::Solid => {}
            LineStyle::Dashed => flags |= STYLE_DASHED_BIT,
            LineStyle::Ticked => flags |= STYLE_TICKED_BIT,
        }
        if s.color2.is_some() {
            flags |= STYLE_HAS_COLOR2_BIT;
        }
        if s.fixed_width {
            flags |= STYLE_FIXED_WIDTH_BIT;
        }
        if s.terrain_layer {
            flags |= STYLE_TERRAIN_LAYER_BIT;
        }
        data.push(s.id);
        data.push(s.z_index as u8);
        data.extend_from_slice(&s.color.to_le_bytes());
        data.push(s.weight);
        data.push(flags);
        data.extend_from_slice(&s.color2.unwrap_or(0).to_le_bytes());
    }
    data
}

/// Pack one feature: the header plus delta-encoded rings. `node_bbox` is the containing leaf's
/// bbox; the exterior's first point becomes the anchor, stored relative to the leaf min corner.
///
/// The header is the 7-byte compact form whenever the exterior holds `1..=255` vertices and both
/// anchor components land in `0..=65535`, else the 12-byte wide form. A leaf-relative anchor is
/// small at fine LODs but genuinely is not at coarse ones, where one leaf can span far more than
/// 65 535 µdeg.
pub fn pack_feature(f: &Feature, node_bbox: (i64, i64, i64, i64)) -> Vec<u8> {
    let is_polygon = f.kind == Kind::Polygon;
    let mut flags: u8 = 0;
    if is_polygon {
        flags |= FEATURE_FLAG_POLYGON;
        if f.rings.len() > 1 {
            flags |= FEATURE_FLAG_HOLES;
        }
    }

    let mut anchor_lon = 0i64;
    let mut anchor_lat = 0i64;
    let mut max_delta = 0i64;
    let mut packed_rings: Vec<(usize, Vec<i64>)> = Vec::with_capacity(f.rings.len());

    let mut raw_rings: Vec<Vec<(i64, i64)>> =
        f.rings.iter().map(|ring| canonical_ring_udeg(ring, is_polygon)).collect();
    assert!(!raw_rings.is_empty() && !raw_rings[0].is_empty(), "feature has no encodable exterior vertices");
    if is_polygon {
        let (index, distance) = best_exterior_anchor(&raw_rings[0], &raw_rings[1..])
            .expect("non-empty polygon exterior has an anchor candidate");
        assert!(
            distance <= MAX_HOLE_ANCHOR_DELTA,
            "polygon hole is {distance} µdeg from its best exterior anchor; build it through the quadtree before packing"
        );
        raw_rings[0].rotate_left(index);
    }

    let mut feature_anchor = (0i64, 0i64);
    for (i, raw_pts) in raw_rings.iter_mut().enumerate() {
        // Polygon rings close implicitly in the format and the renderer, so serializing the
        // repeated first vertex spends a frame point without adding geometry. Lines keep their last
        // point even when they are loops: their stroke path is not implicitly closed.
        let start_ref = if i == 0 {
            anchor_lon = raw_pts[0].0 - node_bbox.0;
            anchor_lat = raw_pts[0].1 - node_bbox.1;
            feature_anchor = raw_pts[0];
            feature_anchor
        } else {
            // OBCM gives holes no independent anchor: their first delta is relative to the
            // exterior anchor. Rotating a ring is lossless and minimizes that jump. If even the
            // nearest vertex is too far away, bridge vertices would turn the hole into a long
            // wedge, so the quadtree must split first.
            let distance = rotate_ring_near_anchor(raw_pts, feature_anchor);
            debug_assert!(
                distance <= MAX_HOLE_ANCHOR_DELTA,
                "best exterior-anchor calculation disagreed with hole rotation"
            );
            feature_anchor
        };

        // Walk actual ring edges only. Never densify the exterior-anchor to hole jump: those
        // intermediate coordinates would become vertices of the hole, and the implicit closing edge
        // would turn the bridge into a triangular cutout.
        let mut pts: Vec<(i64, i64)> = vec![raw_pts[0]];
        for &p2 in &raw_pts[1..] {
            let last = *pts.last().unwrap();
            densify(last, p2, &mut pts);
        }

        // Exterior: first point is the anchor; deltas start at the 2nd vertex.
        // Hole: every vertex is a delta, the first relative to the anchor.
        let (mut prev, delta_pts): ((i64, i64), &[(i64, i64)]) =
            if i == 0 { (pts[0], &pts[1..]) } else { (start_ref, &pts[..]) };

        let mut deltas: Vec<i64> = Vec::with_capacity(delta_pts.len() * 2);
        for &(x, y) in delta_pts {
            let dx = x - prev.0;
            let dy = y - prev.1;
            deltas.push(dx);
            deltas.push(dy);
            max_delta = max_delta.max(dx.abs()).max(dy.abs());
            prev = (x, y);
        }
        packed_rings.push((pts.len(), deltas));
    }

    assert!(max_delta <= i16::MAX as i64, "quadtree/serializer delta invariant exceeded i16");

    let is16 = max_delta > 127;
    if is16 {
        flags |= FEATURE_FLAG_16BIT;
    }

    // The reader decodes a whole feature (exterior plus holes) into one `MAX_FEAT_PTS` buffer; past
    // that `heapless` silently drops vertices.
    debug_assert!(
        packed_rings.iter().map(|(n, _)| *n).sum::<usize>() <= obc_reader::MAX_FEAT_PTS,
        "feature vertex count exceeds the reader's MAX_FEAT_PTS — chunk_size too large?"
    );

    // The reader discards a whole feature past `MAX_FEAT_RINGS`; the quadtree splits (or
    // floor-trims) features over the cap before they reach here.
    debug_assert!(
        packed_rings.len() <= obc_reader::MAX_FEAT_RINGS,
        "feature ring count exceeds the reader's MAX_FEAT_RINGS — quadtree cap enforcement missed it"
    );

    debug_assert!(packed_rings[0].0 <= u16::MAX as usize, "exterior pt_count overflows the u16 field");
    let ext_pt_count = packed_rings[0].0;
    // Compact iff every compact field can hold its value; the anchor test is on the packed anchor,
    // which densification cannot move (it is the exterior's first vertex).
    const ANCHOR_COMPACT: core::ops::RangeInclusive<i64> = 0..=u16::MAX as i64;
    let wide = ext_pt_count > u8::MAX as usize
        || !ANCHOR_COMPACT.contains(&anchor_lon)
        || !ANCHOR_COMPACT.contains(&anchor_lat);
    if wide {
        flags |= FEATURE_FLAG_WIDE;
    }

    let mut data = Vec::new();
    data.push(f.style_id);
    data.push(flags);
    if wide {
        data.extend_from_slice(&(ext_pt_count as u16).to_le_bytes());
        data.extend_from_slice(&(anchor_lon as i32).to_le_bytes());
        data.extend_from_slice(&(anchor_lat as i32).to_le_bytes());
    } else {
        data.push(ext_pt_count as u8);
        data.extend_from_slice(&(anchor_lon as u16).to_le_bytes());
        data.extend_from_slice(&(anchor_lat as u16).to_le_bytes());
    }

    push_deltas(&mut data, &packed_rings[0].1, is16);

    if flags & FEATURE_FLAG_HOLES != 0 {
        data.push((packed_rings.len() - 1) as u8);
        for (pt_count, deltas) in &packed_rings[1..] {
            debug_assert!(*pt_count <= u16::MAX as usize, "hole pt_count overflows the u16 field");
            data.extend_from_slice(&(*pt_count as u16).to_le_bytes());
            push_deltas(&mut data, deltas, is16);
        }
    }
    data
}

/// Canonicalize integer geometry before delta encoding. Floating-point topology work can leave
/// distinct coordinates that round onto the same microdegree, and clipping can split a straight edge
/// with an exactly collinear middle vertex. Neither carries geometry once coordinates are integer.
fn remove_redundant_vertices(points: &mut Vec<(i64, i64)>, closed: bool) {
    points.dedup();
    if closed {
        while points.len() >= 4 {
            let n = points.len();
            let Some(index) =
                (0..n).find(|&i| redundant_between(points[(i + n - 1) % n], points[i], points[(i + 1) % n]))
            else {
                break;
            };
            points.remove(index);
        }
    } else {
        let mut out = Vec::with_capacity(points.len());
        for &point in points.iter() {
            while out.len() >= 2 && redundant_between(out[out.len() - 2], out[out.len() - 1], point) {
                out.pop();
            }
            out.push(point);
        }
        *points = out;
    }
}

#[inline]
fn redundant_between(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> bool {
    let (abx, aby) = (b.0 as i128 - a.0 as i128, b.1 as i128 - a.1 as i128);
    let (bcx, bcy) = (c.0 as i128 - b.0 as i128, c.1 as i128 - b.1 as i128);
    abx * bcy == aby * bcx && abx * bcx + aby * bcy >= 0
}

/// Pack features into one tight chunk: the packed features back to back, then exactly one `0xFF`
/// [`CHUNK_END`] sentinel. `chunk_size` is the capacity bound, not a stride. A feature that would
/// overflow the chunk (and every feature after it) is dropped; the second return value is how many,
/// so callers can warn instead of losing map content silently.
pub fn pack_chunk(features: &[Feature], node_bbox: (i64, i64, i64, i64), chunk_size: usize) -> (Vec<u8>, usize) {
    let mut data = Vec::new();
    let mut kept = 0usize;
    for f in features {
        let packed = pack_feature(f, node_bbox);
        // The `+ 1` reserves the sentinel byte, so the sealed chunk still fits the capacity, which
        // is what the reader validates an offset-table length against.
        if data.len() + packed.len() + 1 > chunk_size {
            break;
        }
        data.extend_from_slice(&packed);
        kept += 1;
    }
    data.push(CHUNK_END);
    (data, features.len() - kept)
}

/// A resident quadtree whose branches have four NW/NE/SW/SE children. Geometry, POI, graph-node and
/// snap-anchor trees share this traversal contract; their leaf framing stays separate.
trait TreeWalk: Sized {
    fn children(&self) -> Option<&[Self; 4]>;
}

/// A quadtree whose leaf owns at most one chunk. Geometry and POI trees share this framing; graph
/// and snap trees use first-fit leaf binning instead.
trait FlattenTree: TreeWalk {
    /// Pack a leaf's payload into its chunk: `None` for an empty leaf, else `(chunk_bytes,
    /// dropped)` where `dropped` is the chunk-overflow count.
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)>;
}

/// Flatten any [`FlattenTree`] into `(index_bytes, node_count, chunks, dropped)` via BFS. Child
/// order and chunk-id assignment are BFS, which fixes the byte layout: a branch's four children are
/// appended contiguously, so its first-child index is the node count at the moment it is expanded
/// (`child > idx` always, the invariant the reader's `walk_leaves` relies on).
///
/// Chunks come back one `Vec` per chunk, not concatenated, because the two consumers frame them
/// differently: POI chunks are a fixed stride and just get joined, while geometry chunks are tight
/// and need their lengths to build the offset table.
fn flatten_tree<N: FlattenTree>(root: &N, chunk_size: usize) -> (Vec<u8>, u32, Vec<Vec<u8>>, usize) {
    let (nodes, first_child) = obc_tree_walk::breadth_first(root, TreeWalk::children);

    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut chunks: Vec<Vec<u8>> = Vec::new();
    let mut dropped: usize = 0;
    for (idx, node) in nodes.iter().enumerate() {
        match node.children() {
            None => match node.pack_leaf(chunk_size) {
                None => index.push(EMPTY_LEAF),
                Some((chunk, chunk_dropped)) => {
                    let chunk_id = chunks.len() as u32;
                    chunks.push(chunk);
                    dropped += chunk_dropped;
                    index.push(chunk_id & !BRANCH_BIT);
                }
            },
            Some(_) => index.push(first_child[idx] as u32 | BRANCH_BIT),
        }
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, dropped)
}

impl TreeWalk for Node {
    fn children(&self) -> Option<&[Node; 4]> {
        match self {
            Node::Leaf { .. } => None,
            Node::Branch(children) => Some(children),
        }
    }
}

impl FlattenTree for Node {
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)> {
        match self {
            Node::Leaf { bbox, features } if !features.is_empty() => Some(pack_chunk(features, *bbox, chunk_size)),
            _ => None,
        }
    }
}

/// Flatten one geometry quadtree via BFS and frame the chunks as the chunk-data region: a
/// `chunk_count + 1` entry `uint32` offset table, filler, then the chunks, each ending in its one
/// `0xFF` sentinel and padded to the next unit boundary.
///
/// The offsets are scaled and region-relative: entry `e` names `data_start + e * U`, chunk `k` spans
/// `offsets[k]..offsets[k+1]`, and the last entry is the region's total chunk units (the reader
/// keeps it resident as its bound). The table is written even for a chunkless LOD, where it is the
/// single `0` entry, and the region ends on a unit boundary, so the next LOD's index starts on one
/// without the caller aligning anything.
pub fn serialize_tree(root: &Node, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    let (index_bytes, node_count, chunks, dropped) = flatten_tree(root, chunk_size);
    // The table is region-relative, so it is built from the chunks' spans rather than from a
    // cursor: a chunk's span is its content rounded up to a unit.
    let mut table = Vec::with_capacity((chunks.len() + 1) * 4);
    let mut span_total = 0usize;
    table.extend_from_slice(&scaled(span_total).to_le_bytes());
    for c in &chunks {
        span_total += align_up(c.len());
        table.extend_from_slice(&scaled(span_total).to_le_bytes());
    }
    // The table above and the chunk run below are the one place in this file where a boundary is
    // computed twice — `align_up` per span there, `begin_section` per chunk here — because a table
    // entry is region-relative and the cursor is not. The assert ties them together: the cursor must
    // land exactly `offsets[Chunk Count]` units past `data_start`, or the table the reader indexes
    // with describes a region the writer did not lay out.
    let (data, ()) = lay_out(index_bytes.len(), |w| {
        w.put(&table)?;
        let data_start = w.begin_section()?;
        for c in &chunks {
            w.put(c)?;
            w.begin_section()?;
        }
        debug_assert_eq!(
            w.at() - data_start,
            span_total as u64,
            "the §5.1 offset table and the chunks it addresses must end at the same byte"
        );
        Ok(())
    });
    (index_bytes, node_count, data, chunks.len() as u32, dropped)
}

/// A POI record's absolute microdegree coordinates plus the fields packed into its record. Owned so
/// the tree can move records into leaves. The trailer holds a service-hours reference, a summit
/// height or a population.
struct PoiPoint {
    metadata: obc_formats::obcm::PoiMetadata,
    lon_udeg: i32,
    lat_udeg: i32,
    subtype: u8,
    name: Option<String>,
    payload: u16,
}

/// One node of a category's POI quadtree, mirroring the geometry [`Node`] shape so the shared
/// [`flatten_tree`] serializes it to the identical index layout.
enum PoiNode {
    Leaf(Vec<PoiPoint>),
    Branch(Box<[PoiNode; 4]>),
}

impl TreeWalk for PoiNode {
    fn children(&self) -> Option<&[PoiNode; 4]> {
        match self {
            PoiNode::Leaf(_) => None,
            PoiNode::Branch(children) => Some(children),
        }
    }
}

impl FlattenTree for PoiNode {
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)> {
        match self {
            PoiNode::Leaf(points) if !points.is_empty() => Some(pack_poi_chunk(points, chunk_size)),
            _ => None,
        }
    }
}

/// Pack one POI record. Names truncate only at UTF-8 character boundaries.
fn pack_poi_record(p: &PoiPoint) -> [u8; POI_RECORD_LEN] {
    let mut rec = [CHUNK_END; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat_udeg.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon_udeg.to_le_bytes());
    rec[8] = p.subtype;
    let name = p.name.as_deref().unwrap_or("");
    let bytes = name.as_bytes();
    let mut len = bytes.len().min(POI_NAME_LEN);
    while !name.is_char_boundary(len) {
        len -= 1;
    }
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 34] stays 0xFF (name pad).
    rec[34..36].copy_from_slice(&p.payload.to_le_bytes());
    rec[36..64].copy_from_slice(&p.metadata.encode());
    rec
}

/// Pack a leaf's POI records into one `chunk_size`-byte chunk: as many fixed records as fit, back to
/// back, then a `0xFF` subtype sentinel and `0xFF` padding. `build_poi_tree` splits a leaf before it
/// exceeds the capacity, so `dropped` is the safety net for the one case the tree cannot split away:
/// more POIs than a chunk holds inside the ~1 m recursion floor. Truncating loudly beats corrupting
/// the chunk.
fn pack_poi_chunk(points: &[PoiPoint], chunk_size: usize) -> (Vec<u8>, usize) {
    let capacity = chunk_size / POI_RECORD_LEN;
    let kept = points.len().min(capacity);
    let mut data = Vec::with_capacity(chunk_size);
    for p in &points[..kept] {
        data.extend_from_slice(&pack_poi_record(p));
    }
    // The 0xFF subtype byte ends the records (mirroring the geometry chunk's style-id sentinel);
    // `resize` writes it and the rest of the padding in one go.
    data.resize(chunk_size, CHUNK_END);
    (data, points.len() - kept)
}

/// Build one category's POI quadtree over the global bbox, splitting a leaf once it holds more
/// points than one chunk can carry. Split geometry matches the reader's `walk_leaves` exactly:
/// floor-division midpoints, NW/NE/SW/SE order, and a 10-µdeg recursion guard so a dense cluster
/// cannot recurse forever.
fn build_poi_tree(points: Vec<PoiPoint>, bbox: (i64, i64, i64, i64), capacity: usize) -> PoiNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    // Fits a chunk, or the box is too small to subdivide. The guard matches the geometry
    // quadtree's 10-µdeg floor so both agree on when to stop.
    if points.len() <= capacity || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return PoiNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    // West is `lon < mid` and South is `lat < mid`, so a point exactly on a midline lands in the
    // East or North child — inside that child's bbox either way.
    let mut nw = Vec::new();
    let mut ne = Vec::new();
    let mut sw = Vec::new();
    let mut se = Vec::new();
    for p in points {
        let west = (p.lon_udeg as i64) < mid_lon;
        let south = (p.lat_udeg as i64) < mid_lat;
        match (west, south) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    PoiNode::Branch(Box::new([
        build_poi_tree(nw, (min_lon, mid_lat, mid_lon, max_lat), capacity), // NW
        build_poi_tree(ne, (mid_lon, mid_lat, max_lon, max_lat), capacity), // NE
        build_poi_tree(sw, (min_lon, min_lat, mid_lon, mid_lat), capacity), // SW
        build_poi_tree(se, (mid_lon, min_lat, max_lon, mid_lat), capacity), // SE
    ]))
}

/// Serialize the full POI section: the directory, then each category's quadtree index and data
/// chunks, then the shared hours-pool section at the tail. `pois` is the deduped classified list,
/// bucketed by its subtype's category. Every category gets a directory entry, empty or not; summits
/// and settlements add theirs only when the map holds one. `section_offset` is the section's
/// absolute byte offset, because the directory's offsets are file-absolute.
///
/// The hours pool is built once over the whole list: identical weekly-schedule blobs collapse to
/// one, and each POI's `hours_ref` is stamped onto its record before tree-building, so it travels
/// into the right leaf.
pub fn serialize_poi_section(
    pois: &[Poi],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
) -> io::Result<Vec<u8>> {
    // `refs[k]` is POI k's 0-based pool index (or `None` for no hours), aligned to `pois`.
    let (pool, refs) =
        crate::hours::build_hours_pool(pois, |p| has_hours(p.subtype).then_some(p.hours.as_ref()).flatten());
    serialize_poi_pool(pois, global_bbox, section_offset, &pool, &refs)
}

/// Only a service place carries an hours reference in its payload. A summit holds an elevation there
/// and a settlement a population, so neither joins the pool.
fn has_hours(subtype: u8) -> bool {
    subtype != SUMMIT_SUBTYPE_ID && settlement_class_of(subtype).is_none()
}

fn serialize_poi_pool(
    pois: &[Poi],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
    pool: &[[u8; POI_HOURS_BLOB_LEN]],
    refs: &[Option<u16>],
) -> io::Result<Vec<u8>> {
    if pool.len() >= POI_HOURS_REF_NONE as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "hours pool exceeds its record limit"));
    }
    let mut category_ids: Vec<_> = obc_formats::obcm::PoiCategory::ALL.iter().map(|c| c.id()).collect();
    if pois.iter().any(|p| p.subtype == SUMMIT_SUBTYPE_ID) {
        category_ids.push(SUMMIT_CATEGORY_ID);
    }
    if pois.iter().any(|p| settlement_class_of(p.subtype).is_some()) {
        category_ids.push(SETTLEMENT_CATEGORY_ID);
    }
    category_ids.sort_unstable();
    let category_count = category_ids.len();
    let mut by_cat: Vec<Vec<PoiPoint>> = (0..=SETTLEMENT_CATEGORY_ID).map(|_| Vec::new()).collect();
    for (p, hours_ref) in pois.iter().zip(refs.iter()) {
        let cat = table_row(p.subtype).category() as usize;
        by_cat[cat].push(PoiPoint {
            metadata: p.metadata,
            lon_udeg: p.lon_udeg,
            lat_udeg: p.lat_udeg,
            subtype: p.subtype,
            name: p.name.clone(),
            payload: match p.subtype {
                SUMMIT_SUBTYPE_ID => p.elevation_m.unwrap_or(SUMMIT_ELEVATION_UNKNOWN) as u16,
                s if settlement_class_of(s).is_some() => crate::poi::settlement_payload(p.population),
                _ => hours_ref.unwrap_or(POI_HOURS_REF_NONE),
            },
        });
    }

    // Records per chunk = chunk_size / record_len, so a leaf holds at most that many before the
    // tree splits.
    let capacity = POI_CHUNK_SIZE / POI_RECORD_LEN;

    // Flatten every category's tree first; the directory's index offsets are then laid out
    // sequentially after the fixed-size directory itself.
    struct CatBlock {
        cat_id: u8,
        index: Vec<u8>,
        node_count: u32,
        chunks: Vec<u8>,
        chunk_count: u32,
    }
    let mut blocks = Vec::with_capacity(category_count);
    for cat_id in category_ids {
        let pts = std::mem::take(&mut by_cat[cat_id as usize]);
        if pts.is_empty() {
            blocks.push(CatBlock { cat_id, index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0 });
            continue;
        }
        let root = build_poi_tree(pts, global_bbox, capacity);
        let (index, node_count, chunks, dropped) = flatten_tree(&root, POI_CHUNK_SIZE);
        if dropped != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "POI density exceeds the chunk capacity"));
        }
        // POI chunks keep the fixed stride and have no offset table, so the reader's chunk range
        // stays the plain `k * chunk_size`.
        let chunk_count = chunks.len() as u32;
        blocks.push(CatBlock { cat_id, index, node_count, chunks: chunks.concat(), chunk_count });
    }

    // Directory size: count byte, chunk_size u16, one entry per category, and the hours-pool
    // offset and count.
    let dir_len = 1 + 2 + category_count * POI_CAT_ENTRY_LEN + 4 + 2;

    // Categories are laid out sequentially after the directory: [index][filler][chunks] each, with
    // empties contributing nothing but their directory entry. Every `Index Offset` is scaled, so
    // each index starts on a unit boundary. 512 is a multiple of `U` at every legal scale, so the
    // chunks need no filler between them and the region ends aligned for the next category, which
    // is why every `begin_section` in the loop below is a no-op after the first.
    //
    // The cursor starts past the directory, because the directory's own bytes cannot be written
    // until this walk has resolved the offsets they carry.
    let (payload, (cat_entries, hours_pool_offset)) = lay_out(section_offset + dir_len, |w| {
        let mut cat_entries = Vec::with_capacity(category_count);
        for b in &blocks {
            cat_entries.push((b.cat_id, scaled(w.begin_section()? as usize), b.node_count, b.chunk_count));
            w.put(&b.index)?;
            w.begin_section()?;
            w.put(&b.chunks)?;
        }
        // The hours pool, then the run that leaves the nav directory behind it nameable.
        let hours_pool_offset = w.begin_section()? as usize;
        w.put(&pack_hours_pool(pool))?;
        w.begin_section()?;
        Ok((cat_entries, hours_pool_offset))
    });

    let mut out = Vec::with_capacity(dir_len + payload.len());
    out.push(category_count as u8);
    out.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
    for (cat_id, index_offset, node_count, chunk_count) in cat_entries {
        out.push(cat_id);
        out.extend_from_slice(&index_offset.to_le_bytes());
        out.extend_from_slice(&node_count.to_le_bytes());
        out.extend_from_slice(&chunk_count.to_le_bytes());
    }
    out.extend_from_slice(&scaled(hours_pool_offset).to_le_bytes());
    out.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    debug_assert_eq!(out.len(), dir_len);
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Pack the hours-pool section: `count u16` then the blobs back to back. An empty pool is just the
/// `0` count.
fn pack_hours_pool(pool: &[[u8; POI_HOURS_BLOB_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + pool.len() * POI_HOURS_BLOB_LEN);
    out.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    for blob in pool {
        out.extend_from_slice(blob);
    }
    out
}

/// One adjacency entry of a junction record, holding the neighbor's absolute µdeg coords (the
/// serializer turns them into `i16` deltas from the owning record at pack time), the resolved wire
/// `edge_id`, the edge's ground `cost_m`, its `way_kind` class byte, and `ascent_m` — the climb of
/// riding the edge toward this neighbor, the one field the two sides of an edge legitimately
/// disagree on.
struct WireNeighbor {
    id: u32,
    lat: i32,
    lon: i32,
    edge_id: u32,
    cost_m: u32,
    way_kind: u8,
    ascent_m: u16,
}

/// A junction node ready to serialize: absolute µdeg coords, its dense id, and its capped neighbor
/// list.
struct NavPoint {
    lat: i32,
    lon: i32,
    id: u32,
    neighbors: Vec<WireNeighbor>,
}

impl NavPoint {
    fn record_len(&self) -> usize {
        NAV_NODE_FIXED_LEN + self.neighbors.len() * NAV_NEIGHBOR_LEN
    }
}

/// One node of the nav quadtree, mirroring [`PoiNode`] so the shared [`flatten_tree`] gives the
/// identical index layout. Leaves split on packed bytes, not record count.
enum NavTreeNode {
    Leaf(Vec<NavPoint>),
    Branch(Box<[NavTreeNode; 4]>),
}

/// Pack one junction record. Coordinates are absolute µdeg in the head, lat first; each neighbor's
/// coord is an `i16` delta from this record's own lat/lon (the edge splits guarantee the delta
/// fits), so relaxation reconstructs `neighbor = node + delta` exactly.
fn pack_nav_record(p: &NavPoint, out: &mut Vec<u8>) {
    out.extend_from_slice(&p.lat.to_le_bytes());
    out.extend_from_slice(&p.lon.to_le_bytes());
    out.extend_from_slice(&p.id.to_le_bytes());
    debug_assert!(p.neighbors.len() <= NAV_MAX_DEGREE, "degree capped before packing");
    out.push(p.neighbors.len() as u8);
    for n in &p.neighbors {
        let dlat = n.lat as i64 - p.lat as i64;
        let dlon = n.lon as i64 - p.lon as i64;
        debug_assert!(
            (-NAV_MAX_NEIGHBOR_DELTA..=NAV_MAX_NEIGHBOR_DELTA).contains(&dlat)
                && (-NAV_MAX_NEIGHBOR_DELTA..=NAV_MAX_NEIGHBOR_DELTA).contains(&dlon),
            "N1 + the serializer's split guarantee neighbor deltas fit i16 ({dlat},{dlon})"
        );
        debug_assert!(n.cost_m <= u16::MAX as u32, "N1 guarantees cost_m ≤ 60 000, fits u16 ({})", n.cost_m);
        out.extend_from_slice(&n.id.to_le_bytes());
        out.extend_from_slice(&(dlat as i16).to_le_bytes());
        out.extend_from_slice(&(dlon as i16).to_le_bytes());
        out.extend_from_slice(&n.edge_id.to_le_bytes());
        out.extend_from_slice(&(n.cost_m.min(u16::MAX as u32) as u16).to_le_bytes());
        out.push(n.way_kind);
        out.extend_from_slice(&n.ascent_m.to_le_bytes());
    }
}

/// Bin-pack the tree's leaves into 512-byte node chunks. A leaf's record block goes into the first
/// already-open chunk with room, so a small leaf back-fills the slack a larger one left.
///
/// Distinct leaves may therefore share a chunk id, and because first-fit reaches back they can be
/// spatially distant: a walk that visits several leaves of one chunk decodes that chunk once per
/// leaf and hands a consumer the same junction more than once. The reference consumers are
/// idempotent, so this is the section's contract, not a bug. A single leaf's records never straddle
/// chunks; a leaf larger than one chunk keeps what fits and counts the rest in `dropped`.
///
/// Returns the same shape as [`flatten_tree`], so the directory-writing code is shared.
fn flatten_nav_tree(root: &NavTreeNode) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    flatten_binned_tree(root, NAV_CHUNK_SIZE)
}

/// A resident tree whose leaf records are first-fit packed without splitting a leaf across chunks.
/// Graph nodes and snap anchors share the traversal and binning, and keep their own record sizing
/// and wire encoding.
trait FlattenBinnedTree: TreeWalk {
    type Record;

    fn records(&self) -> Option<&[Self::Record]>;
    fn record_len(record: &Self::Record) -> usize;
    fn pack_record(record: &Self::Record, out: &mut Vec<u8>);
}

fn flatten_binned_tree<N: FlattenBinnedTree>(root: &N, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    let (nodes, first_child) = obc_tree_walk::breadth_first(root, TreeWalk::children);
    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut bins: Vec<Vec<u8>> = Vec::new();
    let mut dropped: usize = 0;
    for (idx, node) in nodes.iter().enumerate() {
        let records = match node.records() {
            None => {
                index.push(first_child[idx] as u32 | BRANCH_BIT);
                continue;
            }
            Some(records) if !records.is_empty() => records,
            Some(_) => {
                index.push(EMPTY_LEAF);
                continue;
            }
        };
        let leaf_len: usize = records.iter().map(N::record_len).sum();
        // First-fit: the first open chunk whose remaining space holds the whole leaf, else a new
        // one. A leaf larger than a whole chunk opens a fresh chunk and drops its overflow.
        let bin = match bins.iter().position(|b| b.len() + leaf_len <= chunk_size) {
            Some(c) => c,
            None => {
                bins.push(Vec::with_capacity(chunk_size));
                bins.len() - 1
            }
        };
        index.push((bin as u32) & !BRANCH_BIT);
        for record in records {
            if bins[bin].len() + N::record_len(record) > chunk_size {
                dropped += 1;
                continue; // co-located overflow inside one leaf — effectively impossible in real OSM
            }
            N::pack_record(record, &mut bins[bin]);
        }
    }

    // Concatenate the bins, each 0xFF-padded to a full chunk. The padding's first byte lands on a
    // `degree` slot, which gives the reader its end-of-chunk sentinel.
    let chunk_count = bins.len() as u32;
    let mut chunks: Vec<u8> = Vec::with_capacity(bins.len() * chunk_size);
    for mut b in bins {
        b.resize(chunk_size, CHUNK_END);
        chunks.extend_from_slice(&b);
    }

    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count, dropped)
}

impl TreeWalk for NavTreeNode {
    fn children(&self) -> Option<&[NavTreeNode; 4]> {
        match self {
            NavTreeNode::Leaf(_) => None,
            NavTreeNode::Branch(children) => Some(children),
        }
    }
}

impl FlattenBinnedTree for NavTreeNode {
    type Record = NavPoint;

    fn records(&self) -> Option<&[NavPoint]> {
        match self {
            NavTreeNode::Leaf(points) => Some(points),
            NavTreeNode::Branch(_) => None,
        }
    }

    fn record_len(record: &NavPoint) -> usize {
        record.record_len()
    }

    fn pack_record(record: &NavPoint, out: &mut Vec<u8>) {
        pack_nav_record(record, out);
    }
}

/// Build the node quadtree over the global bbox, splitting a leaf once its packed records exceed one
/// chunk. Split geometry is identical to [`build_poi_tree`], so the reader's `walk_leaves` resolves
/// it verbatim.
fn build_nav_tree(points: Vec<NavPoint>, bbox: (i64, i64, i64, i64), chunk_size: usize) -> NavTreeNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let packed: usize = points.iter().map(NavPoint::record_len).sum();
    if packed <= chunk_size || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return NavTreeNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    // Same midline rule as POIs: a point exactly on a midline lands East / North.
    let mut nw = Vec::new();
    let mut ne = Vec::new();
    let mut sw = Vec::new();
    let mut se = Vec::new();
    for p in points {
        let west = (p.lon as i64) < mid_lon;
        let south = (p.lat as i64) < mid_lat;
        match (west, south) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    NavTreeNode::Branch(Box::new([
        build_nav_tree(nw, (min_lon, mid_lat, mid_lon, max_lat), chunk_size), // NW
        build_nav_tree(ne, (mid_lon, mid_lat, max_lon, max_lat), chunk_size), // NE
        build_nav_tree(sw, (min_lon, min_lat, mid_lon, mid_lat), chunk_size), // SW
        build_nav_tree(se, (mid_lon, min_lat, max_lon, mid_lat), chunk_size), // SE
    ]))
}

/// A working edge on its way into the pool: endpoints (dense node ids), the densified polyline, the
/// cost carried into both endpoints' records, and the parent way's `kind` class byte, which every
/// split piece inherits.
struct WorkEdge {
    a: u32,
    b: u32,
    polyline: Vec<(i32, i32)>,
    cost_m: u32,
    kind: u8,
}

/// One interior lookup anchor. It is deliberately not a graph node: the coordinate only gets the
/// router to a small candidate set, after which the full edge geometry is projected exactly and
/// connected virtually to the edge's real endpoints.
struct SnapPoint {
    lat: i32,
    lon: i32,
    edge_id: u32,
}

enum SnapTreeNode {
    Leaf(Vec<SnapPoint>),
    Branch(Box<[SnapTreeNode; 4]>),
}

/// Place evenly-spaced interior anchors on one final serialized edge piece. Endpoints need no
/// records, because they already live in the node quadtree. `ceil(length / 300)` intervals keep
/// every gap at most 300 m even when the edge's rounded wire cost differs from the sum of its
/// floating segment lengths.
fn append_snap_points(edge: &WorkEdge, edge_id: u32, out: &mut Vec<SnapPoint>) {
    let seg_lens: Vec<f32> = edge.polyline.windows(2).map(|w| ground_dist_m(w[0], w[1])).collect();
    let length: f32 = seg_lens.iter().sum();
    if length <= NAV_SNAP_EDGE_MIN_M as f32 {
        return;
    }
    let intervals = (length / NAV_SNAP_ANCHOR_GAP_M as f32).ceil() as usize;
    let mut segment = 0usize;
    let mut before = 0.0f32;
    for i in 1..intervals {
        let target = length * i as f32 / intervals as f32;
        while segment + 1 < seg_lens.len() && before + seg_lens[segment] < target {
            before += seg_lens[segment];
            segment += 1;
        }
        let a = edge.polyline[segment];
        let b = edge.polyline[segment + 1];
        let t = ((target - before) / seg_lens[segment].max(f32::EPSILON)).clamp(0.0, 1.0);
        // Interpolate the small delta rather than the absolute microdegree coordinate: at real
        // latitudes an f32 cannot represent every i32 microdegree, while the per-segment delta can.
        let lon = a.0.saturating_add(((b.0 - a.0) as f32 * t).round() as i32);
        let lat = a.1.saturating_add(((b.1 - a.1) as f32 * t).round() as i32);
        out.push(SnapPoint { lat, lon, edge_id });
    }
}

fn build_snap_tree(points: Vec<SnapPoint>, bbox: (i64, i64, i64, i64)) -> SnapTreeNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    if points.len() * NAV_SNAP_RECORD_LEN <= NAV_CHUNK_SIZE || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return SnapTreeNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    let (mut nw, mut ne, mut sw, mut se) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for p in points {
        match ((p.lon as i64) < mid_lon, (p.lat as i64) < mid_lat) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    SnapTreeNode::Branch(Box::new([
        build_snap_tree(nw, (min_lon, mid_lat, mid_lon, max_lat)),
        build_snap_tree(ne, (mid_lon, mid_lat, max_lon, max_lat)),
        build_snap_tree(sw, (min_lon, min_lat, mid_lon, mid_lat)),
        build_snap_tree(se, (mid_lon, min_lat, max_lon, mid_lat)),
    ]))
}

/// Flatten the anchor quadtree with the node tree's first-fit leaf binning. Records keep absolute
/// coordinates, because distinct spatial leaves may share one chunk.
fn flatten_snap_tree(root: &SnapTreeNode) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    flatten_binned_tree(root, NAV_CHUNK_SIZE)
}

impl TreeWalk for SnapTreeNode {
    fn children(&self) -> Option<&[SnapTreeNode; 4]> {
        match self {
            SnapTreeNode::Leaf(_) => None,
            SnapTreeNode::Branch(children) => Some(children),
        }
    }
}

impl FlattenBinnedTree for SnapTreeNode {
    type Record = SnapPoint;

    fn records(&self) -> Option<&[SnapPoint]> {
        match self {
            SnapTreeNode::Leaf(points) => Some(points),
            SnapTreeNode::Branch(_) => None,
        }
    }

    fn record_len(_: &SnapPoint) -> usize {
        NAV_SNAP_RECORD_LEN
    }

    fn pack_record(record: &SnapPoint, out: &mut Vec<u8>) {
        out.extend_from_slice(&record.lat.to_le_bytes());
        out.extend_from_slice(&record.lon.to_le_bytes());
        out.extend_from_slice(&record.edge_id.to_le_bytes());
    }
}

/// Densify one nav polyline so every `(dlat, dlon)` fits an `i16` — the same [`densify`] the
/// geometry rings use, over the same 30 000-µdeg threshold.
fn densify_polyline(pts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let mut out64: Vec<(i64, i64)> = Vec::with_capacity(pts.len());
    out64.push((pts[0].0 as i64, pts[0].1 as i64));
    for w in pts.windows(2) {
        let last = *out64.last().unwrap();
        densify(last, (w[1].0 as i64, w[1].1 as i64), &mut out64);
    }
    // Midpoints interpolate between in-range i32 endpoints, so the cast is exact.
    out64.into_iter().map(|(x, y)| (x as i32, y as i32)).collect()
}

/// Pack one edge record. The polyline is already densified, so every delta fits `i16`.
fn pack_edge_record(e: &WorkEdge, elevation_complete: bool, out: &mut Vec<u8>) {
    out.extend_from_slice(&e.cost_m.to_le_bytes());
    out.extend_from_slice(&((e.polyline.len() as u16) | if elevation_complete { 0x8000 } else { 0 }).to_le_bytes());
    out.push(e.kind);
    out.extend_from_slice(&e.polyline[0].1.to_le_bytes()); // anchor lat
    out.extend_from_slice(&e.polyline[0].0.to_le_bytes()); // anchor lon
    for w in e.polyline.windows(2) {
        let dlat = w[1].1 - w[0].1;
        let dlon = w[1].0 - w[0].0;
        debug_assert!(i16::try_from(dlat).is_ok() && i16::try_from(dlon).is_ok(), "densified deltas must fit i16");
        out.extend_from_slice(&(dlat as i16).to_le_bytes());
        out.extend_from_slice(&(dlon as i16).to_le_bytes());
    }
}

/// Pack the profile table: one 56-byte record per profile. The name is UTF-8 truncated and
/// `0xFF`-padded (the POI-name convention); the reserved tail is zero, not `0xFF`, because it is a
/// reserved field and not a padded string.
fn pack_profile_table(profiles: &[NavProfile]) -> Vec<u8> {
    debug_assert!((1..=NAV_MAX_PROFILES).contains(&profiles.len()), "1..=8 profiles");
    let mut out = Vec::with_capacity(profiles.len() * NAV_PROFILE_LEN);
    for p in profiles {
        let name = p.name.as_bytes();
        let n = name.len().min(NAV_PROFILE_NAME_LEN);
        out.extend_from_slice(&name[..n]);
        out.resize(out.len() + (NAV_PROFILE_NAME_LEN - n), CHUNK_END); // 0xFF-pad the name field
        out.extend_from_slice(&p.highway);
        out.extend_from_slice(&p.surface);
        out.push(p.climb_weight);
        out.resize(out.len() + NAV_PROFILE_RESERVED_LEN, 0);
    }
    debug_assert_eq!(out.len(), profiles.len() * NAV_PROFILE_LEN);
    out
}

/// Mints `Edge Id`s as records are placed in the edge pool: the id is a per-chunk record counter.
///
/// The ordinal restarts whenever the chunk does, which is not the same as whenever a record was
/// pushed past a boundary. A record whose length divides the space left in its chunk exactly ends
/// flush with it, and the next record then opens the next chunk with no filler and no push. Deriving
/// both halves from the byte the record lands on makes that case fall out.
#[derive(Default)]
struct EdgeIds {
    chunk: u32,
    ordinal: u32,
}

impl EdgeIds {
    /// The id of a record starting at pool byte `at`, which must already have been advanced past
    /// any no-straddle filler.
    fn mint(&mut self, at: usize) -> u32 {
        let chunk = (at / NAV_CHUNK_SIZE) as u32;
        if chunk != self.chunk {
            self.chunk = chunk;
            self.ordinal = 0;
        }
        // The producer cap, and the reason `0xFFFFFFFF` is an impossible id. The 19-byte minimum
        // record puts the real maximum at 26, so this never binds today; it is asserted so that it
        // stays true if a future record shrinks.
        assert!(
            (self.ordinal as usize) < NAV_EDGE_MAX_RECORDS_PER_CHUNK,
            "an edge chunk may hold at most {NAV_EDGE_MAX_RECORDS_PER_CHUNK} records"
        );
        let id = nav_edge_id(chunk, self.ordinal).expect("chunk index and ordinal are both in field");
        self.ordinal += 1;
        id
    }
}

/// The regions the nav directory has to name, as [`walk_nav_section`] found them.
struct NavOffsets {
    profile_table_offset: usize,
    index_offset: usize,
    edge_pool_offset: usize,
    snap_index_offset: usize,
}

/// Everything behind a populated graph's profile table, already in wire form.
struct NavBody<'a> {
    index: &'a [u8],
    node_count: u32,
    chunks: &'a [u8],
    chunk_count: u32,
    pool: &'a [u8],
    edge_chunk_count: u32,
    snap_index: &'a [u8],
    snap_node_count: u32,
    snap_chunks: &'a [u8],
    snap_chunk_count: u32,
}

/// Walk the nav section through `w`, returning the offsets its directory has to state.
///
/// This runs twice and it is the same walk both times: once over a sink that keeps nothing but the
/// cursor, to find the offsets the 40-byte directory carries, and once over the real buffer with
/// that directory in hand. A projection and an emission that were two pieces of code could disagree;
/// two runs of one piece cannot. It is affordable here because the whole body is already resident.
fn walk_nav_section<E>(
    w: &mut UnitWriter<'_, E>,
    directory: &[u8],
    profile_table: &[u8],
    body: Option<&NavBody<'_>>,
) -> Result<NavOffsets, E> {
    debug_assert_eq!(directory.len(), NAV_DIR_LEN);
    // The profile table sits behind the 40-byte directory, at the first unit boundary past it, so
    // the bytes between them are filler.
    w.put(directory)?;
    let profile_table_offset = w.begin_section()? as usize;
    w.put(profile_table)?;
    let Some(b) = body else {
        // Empty graph: the directory and the always-present profile table are the whole section.
        // The zero-length regions still have to be nameable, so all three point at the first unit
        // boundary past the table rather than at its last byte.
        let at = w.begin_section()? as usize;
        return Ok(NavOffsets { profile_table_offset, index_offset: at, edge_pool_offset: at, snap_index_offset: at });
    };

    // `nav_index_padding` chooses each alignment run so that two things hold at once: the index
    // starts on a unit boundary (or no scaled offset could name it), and the fixed 512-byte chunks
    // behind it start on a sector boundary, so a full-chunk read is one card command. The edge pool
    // stays sector-aligned because the node region is whole 512-byte chunks.
    //
    // Every gap here is `0xFF`: one fill byte, one rule — a gap is `0xFF` and a reserved field
    // is `0`.
    w.pad(index_pad(w.at(), b.index.len() as u64))?;
    let index_offset = w.at() as usize;
    w.put(b.index)?;
    w.begin_section()?;
    w.put(b.chunks)?;
    let edge_pool_offset = w.at() as usize;
    w.put(b.pool)?;
    // A snap index of no nodes has no sector to reconcile, so its region only has to be nameable.
    if b.snap_node_count == 0 {
        w.begin_section()?;
    } else {
        w.pad(index_pad(w.at(), b.snap_index.len() as u64))?;
    }
    let snap_index_offset = w.at() as usize;
    w.put(b.snap_index)?;
    w.begin_section()?;
    w.put(b.snap_chunks)?;
    debug_assert_eq!(w.at(), align_up(w.at() as usize) as u64, "the file tail stays aligned");
    Ok(NavOffsets { profile_table_offset, index_offset, edge_pool_offset, snap_index_offset })
}

/// The alignment run before a quadtree index of `index_len` bytes starting, unpadded, at `at`.
#[inline]
fn index_pad(at: u64, index_len: u64) -> u64 {
    nav_index_padding(SCALE, at, index_len).expect("a nav index length never approaches u64::MAX") as u64
}

/// The 40-byte nav directory, over the offsets [`walk_nav_section`]'s first pass resolved.
fn nav_directory(offsets: &NavOffsets, body: Option<&NavBody<'_>>, profile_count: usize) -> Vec<u8> {
    let (node_count, node_chunks, edge_chunks, snap_node_count, snap_chunks) = match body {
        Some(b) => (b.node_count, b.chunk_count, b.edge_chunk_count, b.snap_node_count, b.snap_chunk_count),
        None => (0, 0, 0, 0, 0),
    };
    let mut dir = Vec::with_capacity(NAV_DIR_LEN);
    dir.extend_from_slice(&scaled(offsets.index_offset).to_le_bytes());
    dir.extend_from_slice(&node_count.to_le_bytes());
    dir.extend_from_slice(&node_chunks.to_le_bytes());
    dir.extend_from_slice(&scaled(offsets.edge_pool_offset).to_le_bytes());
    dir.extend_from_slice(&edge_chunks.to_le_bytes());
    dir.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes()); // chunk_size (pinned 512)
    dir.extend_from_slice(&scaled(offsets.profile_table_offset).to_le_bytes());
    dir.push(profile_count as u8);
    dir.push(0u8); // reserved — a field, so `0`, unlike a gap
    dir.extend_from_slice(&scaled(offsets.snap_index_offset).to_le_bytes());
    dir.extend_from_slice(&snap_node_count.to_le_bytes());
    dir.extend_from_slice(&snap_chunks.to_le_bytes());
    debug_assert_eq!(dir.len(), NAV_DIR_LEN);
    dir
}

/// Lay the section out at `section_offset`: the placeholder walk that resolves the directory's
/// offsets, then the identical walk that writes the bytes.
fn emit_nav_section(
    section_offset: usize,
    profile_table: &[u8],
    body: Option<&NavBody<'_>>,
    profiles: usize,
) -> Vec<u8> {
    let placeholder = [0u8; NAV_DIR_LEN];
    let offsets = place(section_offset, |w| walk_nav_section(w, &placeholder, profile_table, body));
    let directory = nav_directory(&offsets, body, profiles);
    let (out, _) = lay_out(section_offset, |w| walk_nav_section(w, &directory, profile_table, body));
    out
}

/// Serialize the full nav-graph section at absolute byte `section_offset`:
/// `[directory][profile table][node quadtree index][node chunks][edge pool][snap index][snap
/// chunks]`. The profile table is written immediately after the directory, before the node index, so
/// even an empty graph carries its profiles; the section is always present.
///
/// Graph normalizations happen here, on working copies, so the caller's [`NavGraph`] is untouched.
/// Polylines are densified, and an edge whose record would overflow one chunk, or whose piece would
/// span more than [`NAV_MAX_NEIGHBOR_DELTA`], is split at a vertex into pieces joined by synthetic
/// degree-2 nodes, so no record straddles a chunk and every neighbor delta fits `i16`. A node keeps
/// its first [`NAV_MAX_DEGREE`] adjacency entries in edge-pool order and the packer warns about the
/// rest. A self-loop edge contributes one adjacency entry, not two.
///
/// `terrain` is where `Ascent M` comes from. It is sampled after every split, over each final
/// piece's own polyline, because an edge's total climb cannot be divided among its pieces after the
/// fact: the dead-band is a fold over samples, not a length. Hand it
/// [`NullElevation`](obc_elevation::NullElevation) and every entry gets `0`.
pub fn serialize_nav_section(
    graph: &NavGraph,
    profiles: &[NavProfile],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
    terrain: &mut dyn ElevationSource,
) -> Vec<u8> {
    let profile_table = pack_profile_table(profiles);
    if graph.nodes.is_empty() {
        return emit_nav_section(section_offset, &profile_table, None, profiles.len());
    }

    // R1 assigns dense ids in push order — the serializer indexes `coords` by id.
    debug_assert!(graph.nodes.iter().enumerate().all(|(i, n)| n.id as usize == i), "node ids are dense");
    let mut coords: Vec<(i32, i32)> = graph.nodes.iter().map(|n| n.coord).collect();

    // Densify, then split anything over one chunk's worth of points or whose piece would span more
    // than the i16-delta bound. Splitting appends synthetic nodes, so it runs before adjacency.
    let mut edges: Vec<WorkEdge> = Vec::with_capacity(graph.edges.len());
    for e in &graph.edges {
        let poly = densify_polyline(&e.polyline);
        // Fast path: a short polyline whose endpoints already fit the i16 neighbor delta. A
        // hand-built graph may not, so the span is re-checked rather than trusted.
        let (a, z) = (poly[0], *poly.last().unwrap());
        let span_ok = (a.0 as i64 - z.0 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA
            && (a.1 as i64 - z.1 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA;
        if poly.len() <= NAV_MAX_EDGE_PTS && span_ok {
            edges.push(WorkEdge { a: e.a, b: e.b, polyline: poly, cost_m: e.length_m, kind: e.kind });
            continue;
        }
        // Walk the long polyline in pieces bounded by both the chunk point cap and the i16 endpoint
        // span; each interior cut vertex becomes a synthetic junction. Densify caps each segment
        // below the span bound, so a piece always advances at least one vertex and the loop
        // terminates. Pieces are re-measured, so their costs sum to the original within rounding.
        let mut start = 0usize;
        let mut from = e.a;
        while start < poly.len() - 1 {
            let max_end = (start + NAV_MAX_EDGE_PTS - 1).min(poly.len() - 1);
            let mut end = start + 1;
            while end < max_end {
                let dlon = (poly[end + 1].0 as i64 - poly[start].0 as i64).abs();
                let dlat = (poly[end + 1].1 as i64 - poly[start].1 as i64).abs();
                if dlon > NAV_MAX_NEIGHBOR_DELTA || dlat > NAV_MAX_NEIGHBOR_DELTA {
                    break;
                }
                end += 1;
            }
            let piece = poly[start..=end].to_vec();
            let to = if end == poly.len() - 1 {
                e.b
            } else {
                let id = coords.len() as u32;
                coords.push(poly[end]);
                id
            };
            let cost_m = polyline_len_m(&piece);
            edges.push(WorkEdge { a: from, b: to, polyline: piece, cost_m, kind: e.kind });
            from = to;
            start = end;
        }
    }

    // Edge pool: records back to back in `edges` order, each pushed to the next chunk start if it
    // would straddle a boundary.
    let edge_facts: Vec<_> = edges.iter().map(|e| crate::nav::integrate_edge_facts(&e.polyline, terrain)).collect();
    let mut pool: Vec<u8> = Vec::new();
    let mut edge_ids: Vec<u32> = Vec::with_capacity(edges.len());
    let mut ids = EdgeIds::default();
    for (e, facts) in edges.iter().zip(&edge_facts) {
        let rec_len = NAV_EDGE_FIXED_LEN + (e.polyline.len() - 1) * 4;
        debug_assert!(rec_len <= NAV_CHUNK_SIZE, "split bounded every record to one chunk");
        let within = pool.len() % NAV_CHUNK_SIZE;
        if within + rec_len > NAV_CHUNK_SIZE {
            pool.resize(pool.len() + (NAV_CHUNK_SIZE - within), FILLER);
        }
        edge_ids.push(ids.mint(pool.len()));
        pack_edge_record(e, facts.2, &mut pool);
    }
    pool.resize(pool.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE, FILLER);
    let edge_chunk_count = (pool.len() / NAV_CHUNK_SIZE) as u32;
    assert!(
        edge_chunk_count as u64 <= NAV_EDGE_MAX_CHUNKS,
        "the edge pool's {edge_chunk_count} chunks exceed the {NAV_EDGE_MAX_CHUNKS} an Edge Id can name (§8.4)"
    );

    // Adjacency with inline neighbor coords, capped at NAV_MAX_DEGREE.
    let mut adj: Vec<Vec<WireNeighbor>> = (0..coords.len()).map(|_| Vec::new()).collect();
    let mut truncated = 0usize;
    for ((e, &edge_id), &(ascent_ab, ascent_ba, _)) in edges.iter().zip(&edge_ids).zip(&edge_facts) {
        // The two entries of an edge differ in exactly one field: `a->b` books the climb of riding
        // the polyline forwards, `b->a` the climb of riding it backwards. A self-loop writes the
        // forward one only, matching its single entry.
        let mut push = |from: u32, to: u32, ascent_m: u16| {
            let list = &mut adj[from as usize];
            if list.len() >= NAV_MAX_DEGREE {
                truncated += 1;
                return;
            }
            let (lon, lat) = coords[to as usize];
            list.push(WireNeighbor { id: to, lat, lon, edge_id, cost_m: e.cost_m, way_kind: e.kind, ascent_m });
        };
        push(e.a, e.b, ascent_ab);
        if e.a != e.b {
            push(e.b, e.a, ascent_ba);
        }
    }
    if truncated > 0 {
        eprintln!("warning: {truncated} adjacency entrie(s) dropped at the degree cap ({NAV_MAX_DEGREE})");
    }

    let points: Vec<NavPoint> = coords
        .iter()
        .zip(adj)
        .enumerate()
        .map(|(id, (&(lon, lat), neighbors))| NavPoint { lat, lon, id: id as u32, neighbors })
        .collect();
    let root = build_nav_tree(points, global_bbox, NAV_CHUNK_SIZE);
    let (index, node_count, chunks, chunk_count, dropped) = flatten_nav_tree(&root);
    if dropped > 0 {
        // Co-located junctions inside the 10-µdeg split floor: effectively impossible in real
        // OSM, but never silent.
        eprintln!("warning: {dropped} nav node record(s) dropped (leaf overflow at the split floor)");
    }

    // The lookup-only interior anchors name the final pool ids, so they are generated after every
    // geometry split and after pool placement. A short edge contributes none: its endpoints in the
    // node quadtree already provide the same 300 m spacing contract.
    let mut snap_points = Vec::new();
    for (edge, &edge_id) in edges.iter().zip(&edge_ids) {
        append_snap_points(edge, edge_id, &mut snap_points);
    }
    let (snap_index, snap_node_count, snap_chunks, snap_chunk_count, snap_dropped) = if snap_points.is_empty() {
        (Vec::new(), 0, Vec::new(), 0, 0)
    } else {
        flatten_snap_tree(&build_snap_tree(snap_points, global_bbox))
    };
    if snap_dropped > 0 {
        eprintln!("warning: {snap_dropped} nav snap anchor(s) dropped (leaf overflow at the split floor)");
    }

    let body = NavBody {
        index: &index,
        node_count,
        chunks: &chunks,
        chunk_count,
        pool: &pool,
        edge_chunk_count,
        snap_index: &snap_index,
        snap_node_count,
        snap_chunks: &snap_chunks,
        snap_chunk_count,
    };
    emit_nav_section(section_offset, &profile_table, Some(&body), profiles.len())
}

/// The byte offset of the style table in every file this packer writes: the first unit boundary at
/// or after the header, which at the default `U = 16` is `80`. Reading the field rather than
/// assuming the table follows the header is what it was always for.
const STYLE_OFFSET: usize = 80;
// 80 is a derivation with two halves, and both are asserted.
const _: () = assert!(STYLE_OFFSET >= HEADER_LEN, "the style table cannot start inside the header");
const _: () = assert!(
    (STYLE_OFFSET as u64).is_multiple_of(SCALE.unit()),
    "and it must be a unit boundary a scaled offset can name"
);
const _: () =
    assert!(((STYLE_OFFSET - HEADER_LEN) as u64) < SCALE.unit(), "…the *first* such boundary, so the gap is one unit");

/// The OBCM header. Every offset field here is scaled, and both serializers share it.
///
/// `obc-pack` writes a map with no embedded terrain, so the terrain pair is `(0, 0)`: unambiguous
/// absence, since the header occupies byte `0` and no region can begin there. The producer of a
/// terrain region is `obcm-assemble`, which is what splices the catalog's terrain cells. `--terrain`
/// on this side is an `ElevationSource` for the per-edge climb, and never a carried raster.
fn header_bytes(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
    poi_section_offset: usize,
    nav_section_offset: usize,
    dark_style_offset: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&MAGIC);
    out.push(OBCM_VERSION);
    out.extend_from_slice(&(global_bbox.1 as i32).to_le_bytes()); // min_lat
    out.extend_from_slice(&(global_bbox.0 as i32).to_le_bytes()); // min_lon
    out.extend_from_slice(&(global_bbox.3 as i32).to_le_bytes()); // max_lat
    out.extend_from_slice(&(global_bbox.2 as i32).to_le_bytes()); // max_lon
    out.extend_from_slice(&scaled(STYLE_OFFSET).to_le_bytes());
    out.push(lod_count as u8);
    out.extend_from_slice(&scaled(lod_table_offset).to_le_bytes());
    out.extend_from_slice(&marker_color.to_le_bytes());
    out.extend_from_slice(&scaled(poi_section_offset).to_le_bytes());
    out.extend_from_slice(&scaled(nav_section_offset).to_le_bytes());
    out.push(SCALE.log2());
    out.extend_from_slice(&0u32.to_le_bytes()); // terrain offset — no embedded raster
    out.extend_from_slice(&0u32.to_le_bytes()); // terrain length, `0` exactly when the offset is
    out.extend_from_slice(&[0; 16]); // optional landmark and peak sections
    out.extend_from_slice(&scaled(dark_style_offset).to_le_bytes());
    out.extend_from_slice(&marker_color.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

/// Where the file's fixed prefix puts the LOD table and the first LOD's index: each is named by a
/// scaled offset, so each begins on the first unit boundary past the structure before it. Both
/// serializers need these before they write the header that states them, which is why this is
/// arithmetic rather than a cursor.
fn prefix_offsets(style_len: usize, lod_count: usize) -> (usize, usize) {
    let lod_table_offset = align_up(STYLE_OFFSET + style_len);
    (lod_table_offset, align_up(lod_table_offset + lod_count * LOD_ENTRY_LEN))
}

/// Append one LOD-table entry. `None` max_mpp is `+inf`, the coarsest layer; `cs` is the chunk
/// capacity bound rather than a stride.
fn push_lod_entry(table: &mut Vec<u8>, max_mpp: Option<f64>, index_offset: u32, nc: u32, cs: usize, cc: u32) {
    let mpp_f: f32 = max_mpp.map_or(f32::INFINITY, |v| v as f32);
    table.extend_from_slice(&mpp_f.to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&nc.to_le_bytes());
    table.extend_from_slice(&(cs as u16).to_le_bytes());
    table.extend_from_slice(&cc.to_le_bytes());
}

/// Not an entry point — the in-memory parity oracle for [`serialize_lods_streaming`].
///
/// It lays out the same complete `.obcm` byte stream the obvious way: build every LOD's bytes, then
/// concatenate. Every production caller writes through the streaming twin instead, which holds one
/// tree at a time; this one exists so `streaming_matches_in_memory` can assert the two are
/// byte-identical, and because a corpus-building test outside this crate wants a map in a `Vec<u8>`.
///
/// The second return value is the total chunk-overflow feature drops (see [`pack_chunk`]).
///
// Eight positional arguments, one past clippy's default. A struct would move the same eight names
// one indirection away and force every caller to name a type to say "no styles, no POIs, an empty
// graph"; the streaming twin carries the same list, and the two must stay in lockstep.
#[doc(hidden)]
#[allow(clippy::too_many_arguments)]
pub fn serialize_lods(
    lods: &[LodLayer],
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    nav: &NavGraph,
    profiles: &[NavProfile],
    terrain: &mut dyn ElevationSource,
) -> (Vec<u8>, usize) {
    let style_data = pack_style_dict(styles);
    let lod_count = lods.len();
    let (lod_table_offset, payload_start) = prefix_offsets(style_data.len(), lod_count);

    struct Block {
        ib: Vec<u8>,
        nc: u32,
        cb: Vec<u8>,
        cc: u32,
        cs: usize,
        mpp: Option<f64>,
    }
    let mut blocks = Vec::with_capacity(lod_count);
    let mut dropped = 0usize;
    for lod in lods {
        let (ib, nc, cb, cc, lod_dropped) = serialize_tree(&lod.root, lod.chunk_size);
        dropped += lod_dropped;
        blocks.push(Block { ib, nc, cb, cc, cs: lod.chunk_size, mpp: lod.max_mpp });
    }

    // Each LOD's index is named by a scaled `Index Offset`, so it starts on a unit boundary. The
    // LOD table's own end is rounded up once and every region behind it ends aligned by
    // construction, so no further alignment is needed here.
    let mut cursor = payload_start;
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    for b in &blocks {
        push_lod_entry(&mut table, b.mpp, scaled(cursor), b.nc, b.cs, b.cc);
        cursor += b.ib.len() + b.cb.len();
    }

    // The POI section starts right after the last LOD's chunks; the nav section follows it.
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset)
        .expect("POI section must fit before emitting the in-memory map");
    let nav_section_offset = poi_section_offset + poi_section.len();
    let nav_section = serialize_nav_section(nav, profiles, global_bbox, nav_section_offset, terrain);
    let dark_style_offset = align_up(nav_section_offset + nav_section.len());

    let (out, ()) = lay_out(0, |w| {
        w.put(&header_bytes(
            lod_count,
            marker_color,
            global_bbox,
            lod_table_offset,
            poi_section_offset,
            nav_section_offset,
            dark_style_offset,
        ))?;
        let at = w.begin_section()?; // → the style table
        debug_assert_eq!(at, STYLE_OFFSET as u64);
        w.put(&style_data)?;
        let at = w.begin_section()?; // → the LOD table
        debug_assert_eq!(at, lod_table_offset as u64, "the header names the table the cursor reached");
        w.put(&table)?;
        let at = w.begin_section()?; // → the first LOD's index
        debug_assert_eq!(at, payload_start as u64, "the first LOD entry names the index the cursor reached");
        for b in &blocks {
            w.put(&b.ib)?;
            w.put(&b.cb)?;
        }
        w.put(&poi_section)?;
        w.put(&nav_section)?;
        let at = w.begin_section()?;
        debug_assert_eq!(at, dark_style_offset as u64);
        w.put(&style_data)
    });
    check_scale_covers(out.len() as u64);
    (out, dropped)
}

/// The one producer rule: the scale must cover the file it writes. A file whose bytes reach past
/// what its scale can address is malformed, and the producer that laid it out is the only party
/// positioned to notice — a reader that never resolves the last section never sees anything wrong.
fn check_scale_covers(total: u64) {
    assert!(
        SCALE.covers(total),
        "a {total}-byte map does not fit the {}-byte-unit interior this packer writes (§1.1)",
        SCALE.unit()
    );
}

/// The production writer, and the streaming counterpart to [`serialize_lods`]: the same byte stream,
/// but it builds, serializes and drops one LOD tree at a time, so peak memory is about one tree plus
/// one LOD's chunk bytes.
///
/// The POI and nav section offsets are not known until every LOD is sized, so the header and a
/// zeroed LOD table go out with placeholders, and step 5 seeks back and patches them. Returns
/// `(bytes_written, dropped_features)`, the latter counting chunk-overflow drops so the CLI can
/// warn.
///
/// `build(i)` yields LOD `i`'s `(root, chunk_size, max_mpp)`, called once per level in order; each
/// tree is dropped before the next call. A `None` root writes an empty region: no index, no chunk,
/// and the single-`0` offset table a chunkless LOD needs. A cell artifact depends on that, because
/// it writes the complete ladder with its out-of-band levels empty so that band membership never
/// appears in the bytes, and `Index Node Count == 0` is the predicate a reader caches at mount to
/// skip a level with no I/O at all.
#[allow(clippy::too_many_arguments)]
pub fn serialize_lods_streaming<W, F>(
    w: &mut W,
    lod_count: usize,
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
    landmarks: &[crate::landmark_map::Landmark],
    peaks: &crate::peak_map::Peaks,
    nav: &NavGraph,
    profiles: &[NavProfile],
    terrain: &mut dyn ElevationSource,
    mut build: F,
) -> io::Result<(u64, usize)>
where
    W: Write + Seek,
    F: FnMut(usize) -> (Option<Node>, usize, Option<f64>),
{
    let style_data = pack_style_dict(styles);
    let (lod_table_offset, payload_start) = prefix_offsets(style_data.len(), lod_count);

    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    let mut dropped = 0usize;
    let schedules: Vec<_> = pois
        .iter()
        .map(|p| has_hours(p.subtype).then_some(p.hours.as_ref()).flatten())
        .chain(landmarks.iter().map(|p| p.hours.as_ref()))
        .collect();
    let (pool, refs) = crate::hours::build_hours_pool(&schedules, |schedule| *schedule);
    if pool.len() >= POI_HOURS_REF_NONE as usize {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "hours pool exceeds its record limit"));
    }
    let landmark_refs: Vec<_> = refs[pois.len()..].iter().map(|index| index.unwrap_or(POI_HOURS_REF_NONE)).collect();
    let landmark_bytes = crate::landmark_map::serialize(landmarks, &landmark_refs)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let peak_bytes = crate::peak_map::serialize(peaks).map_err(io::Error::other)?;
    let (
        poi_section_offset,
        nav_section_offset,
        landmark_offset,
        landmark_len,
        peak_offset,
        peak_len,
        dark_style_offset,
        cursor,
    ) = {
        let mut sink = |bytes: &[u8]| w.write_all(bytes);
        let mut u = UnitWriter::new(SCALE, 0, &mut sink);

        // 1. Header, then filler to the style table's boundary. The POI and nav offsets are not
        // known until the LODs are sized, so write `STYLE_OFFSET` placeholders (any unit-aligned
        // byte will do; `scaled` refuses a non-boundary) and patch them in step 5.
        u.put(&header_bytes(
            lod_count,
            marker_color,
            global_bbox,
            lod_table_offset,
            STYLE_OFFSET,
            STYLE_OFFSET,
            STYLE_OFFSET,
        ))?;
        let at = u.begin_section()?; // → the style table
        debug_assert_eq!(at, STYLE_OFFSET as u64);

        // 2. Style table, then a zeroed LOD table patched in step 5. The cursor arriving where
        // `prefix_offsets` said it would is what keeps the header's claim and the bytes one
        // statement.
        u.put(&style_data)?;
        let at = u.begin_section()?; // → the LOD table
        debug_assert_eq!(at, lod_table_offset as u64, "the header names the table the cursor reached");
        u.put(&vec![0u8; lod_count * LOD_ENTRY_LEN])?;
        let at = u.begin_section()?; // → the first LOD's index
        debug_assert_eq!(at, payload_start as u64, "the first LOD entry names the index the cursor reached");

        // 3. Per-LOD: build → serialize → stream payload → drop the tree.
        for i in 0..lod_count {
            let (root, chunk_size, max_mpp) = build(i);
            // Serialized before its LOD-table entry, because the entry states this region's counts;
            // the entry's offset is simply where the cursor stands.
            let region = match root {
                Some(root) => {
                    let out = serialize_tree(&root, chunk_size);
                    drop(root); // free the tree before writing this LOD / building the next
                    Some(out)
                }
                None => None,
            };
            let (nc, cc) = region.as_ref().map_or((0, 0), |&(_, nc, _, cc, _)| (nc, cc));
            push_lod_entry(&mut table, max_mpp, scaled(u.at() as usize), nc, chunk_size, cc);
            match region {
                Some((ib, _, cb, _, lod_dropped)) => {
                    dropped += lod_dropped;
                    u.put(&ib)?;
                    u.put(&cb)?;
                }
                // Empty region: no index, no chunk, the mandatory single-`0` offset table, then the
                // boundary the LOD behind it has to start on.
                None => {
                    u.put(&0u32.to_le_bytes())?;
                    u.begin_section()?;
                }
            }
        }

        // 4. The POI section begins at the current cursor; the nav section follows it.
        let poi_section_offset = u.at() as usize;
        u.put(&serialize_poi_pool(pois, global_bbox, poi_section_offset, &pool, &refs[..pois.len()])?)?;
        let nav_section_offset = u.at() as usize;
        u.put(&serialize_nav_section(nav, profiles, global_bbox, nav_section_offset, terrain))?;
        let (landmark_offset, landmark_len) = if landmark_bytes.is_empty() {
            (0, 0)
        } else {
            let start = u.begin_section()? as usize;
            u.put(&landmark_bytes)?;
            let end = u.begin_section()? as usize;
            (start, end - start)
        };
        let (peak_offset, peak_len) = if peak_bytes.is_empty() {
            (0, 0)
        } else {
            let start = u.begin_section()? as usize;
            u.put(&peak_bytes)?;
            let end = u.begin_section()? as usize;
            (start, end - start)
        };
        let dark_style_offset = u.begin_section()? as usize;
        u.put(&style_data)?;
        (
            poi_section_offset,
            nav_section_offset,
            landmark_offset,
            landmark_len,
            peak_offset,
            peak_len,
            dark_style_offset,
            u.at() as usize,
        )
    };

    // 5. Back-patch the LOD table and the header's two section-offset fields, then leave the cursor
    // at EOF. Both fields are scaled, like every offset the header carries.
    check_scale_covers(cursor as u64);
    w.seek(SeekFrom::Start(lod_table_offset as u64))?;
    w.write_all(&table)?;
    w.seek(SeekFrom::Start(32))?;
    w.write_all(&scaled(poi_section_offset).to_le_bytes())?;
    w.write_all(&scaled(nav_section_offset).to_le_bytes())?;
    w.seek(SeekFrom::Start(obc_formats::obcm::HEADER_LANDMARK_OFFSET_OFF as u64))?;
    w.write_all(&scaled(landmark_offset).to_le_bytes())?;
    w.write_all(&scaled(landmark_len).to_le_bytes())?;
    w.seek(SeekFrom::Start(obc_formats::obcm::HEADER_PEAK_OFFSET_OFF as u64))?;
    w.write_all(&scaled(peak_offset).to_le_bytes())?;
    w.write_all(&scaled(peak_len).to_le_bytes())?;
    w.seek(SeekFrom::Start(obc_formats::obcm::HEADER_DARK_STYLE_OFFSET_OFF as u64))?;
    w.write_all(&scaled(dark_style_offset).to_le_bytes())?;
    w.seek(SeekFrom::Start(cursor as u64))?;
    Ok((cursor as u64, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_elevation::NullElevation;
    use obc_formats::obcm::{nav_edge_id_ordinal, FEATURE_HEADER_WIDE_LEN};

    /// The one case a push-driven ordinal gets wrong: a record that ends flush with its chunk. The
    /// record behind it opens the next chunk with no filler and no push, so an ordinal reset on the
    /// push would carry straight over the boundary and mint a duplicate id.
    #[test]
    fn an_edge_ordinal_restarts_with_the_chunk_not_with_the_push() {
        // 19 + 19 + 19 + 455 = 512 exactly: the fourth record ends on the boundary.
        let flush = [19usize, 19, 19, 455, 19, 19];
        let mut ids = EdgeIds::default();
        let mut at = 0usize;
        let mut minted = Vec::new();
        for len in flush {
            let within = at % NAV_CHUNK_SIZE;
            if within + len > NAV_CHUNK_SIZE {
                at += NAV_CHUNK_SIZE - within;
            }
            minted.push(ids.mint(at));
            at += len;
        }
        let decoded: Vec<(u32, u32)> =
            minted.iter().map(|&id| (obc_formats::obcm::nav_edge_id_chunk(id), nav_edge_id_ordinal(id))).collect();
        assert_eq!(decoded, [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)]);
        // Distinctness is the property that matters, and what a push-driven counter would break.
        let mut sorted = minted.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), minted.len(), "every record gets its own id");
    }

    #[test]
    fn an_edge_pushed_past_a_boundary_opens_the_next_chunk_at_ordinal_zero() {
        let mut ids = EdgeIds::default();
        let mut at = 0usize;
        let mut minted = Vec::new();
        for len in [500usize, 19, 19] {
            let within = at % NAV_CHUNK_SIZE;
            if within + len > NAV_CHUNK_SIZE {
                at += NAV_CHUNK_SIZE - within;
            }
            minted.push(ids.mint(at));
            at += len;
        }
        let decoded: Vec<(u32, u32)> =
            minted.iter().map(|&id| (obc_formats::obcm::nav_edge_id_chunk(id), nav_edge_id_ordinal(id))).collect();
        assert_eq!(decoded, [(0, 0), (1, 0), (1, 1)]);
    }

    #[test]
    fn rounding_is_ties_even_not_away() {
        // Pins the mode `to_udeg` relies on: `round_ties_even` vs `f64::round` (half-away-from-zero).
        assert_eq!(0.5_f64.round_ties_even(), 0.0);
        assert_eq!(1.5_f64.round_ties_even(), 2.0);
        assert_eq!(2.5_f64.round_ties_even(), 2.0);
        assert_eq!(3.5_f64.round_ties_even(), 4.0);
        assert_eq!((-1.5_f64).round_ties_even(), -2.0);
        // The wrong (away) mode would give 1.0 and 3.0 here — guard against it.
        assert_eq!(0.5_f64.round(), 1.0);
        assert_eq!(2.5_f64.round(), 3.0);
        assert_eq!(to_udeg(1.0), 1_000_000);
        assert_eq!(to_udeg(1.0001), 1_000_100);
    }

    #[test]
    fn densify_steps_long_segments() {
        // A 55000-µdeg jump → steps = 55000//30000 + 1 = 2, so exactly one
        // banker's-rounded midpoint, then the endpoint.
        let mut out = Vec::new();
        densify((0, 0), (55000, 0), &mut out);
        assert_eq!(out, vec![(27500, 0), (55000, 0)]);

        // Just under the threshold: no midpoint, just the endpoint.
        let mut out2 = Vec::new();
        densify((0, 0), (100, 200), &mut out2);
        assert_eq!(out2, vec![(100, 200)]);

        // Exactly at the threshold (30000) is NOT densified (`> MAX_SEGMENT`).
        let mut out3 = Vec::new();
        densify((0, 0), (30000, -30000), &mut out3);
        assert_eq!(out3, vec![(30000, -30000)]);
    }

    #[test]
    fn max_safe_chunk_size_keeps_features_within_reader_cap() {
        let n = obc_reader::MAX_FEAT_PTS;
        // `n` points 1 µdeg apart in a tiny zig-zag: tiny deltas ⇒ densest 8-bit encoding (2
        // bytes/vertex), no densification, while the lossless collinear cleanup cannot collapse
        // this fixture to its two endpoints.
        let coords: Vec<(f64, f64)> = (0..n).map(|i| (i as f64 * 1e-6, (i % 2) as f64 * 1e-6)).collect();
        let f = Feature { style_id: 1, kind: Kind::Line, rings: vec![coords] };
        let packed = pack_feature(&f, (0, 0, n as i64, 1));

        // 2048 vertices overflow the compact `pt_count u8`, so this is the wide header — read the
        // count from where the wide layout puts it.
        assert_eq!(packed[1] & FEATURE_FLAG_WIDE, FEATURE_FLAG_WIDE, "a cap-sized feature needs the wide header");
        let ext_pt_count = u16::from_le_bytes([packed[2], packed[3]]) as usize;
        assert_eq!(ext_pt_count, n, "a cap-sized feature keeps every vertex");
        assert!(ext_pt_count <= obc_reader::MAX_FEAT_PTS, "must not exceed the reader cap");

        // The bound runs the other way: a chunk of `MAX_SAFE_CHUNK_SIZE` cannot hold a cap-sized
        // feature at all, so the reader is never handed one it would silently truncate. The bound
        // itself is the loosest encoding, a compact-header line at `2*V + 5` bytes.
        // `MAX_SAFE_CHUNK_SIZE` cannot hold a cap-sized feature at all, so the reader is never handed
        // one it would silently truncate. The bound itself is the loosest encoding — a compact-header
        // line at `2·V + 5` bytes — which is why it sits above these 4106.
        assert!(packed.len() > MAX_SAFE_CHUNK_SIZE, "a cap-sized feature does not fit the safe-max chunk");
        assert_eq!(packed.len(), FEATURE_HEADER_WIDE_LEN + (n - 1) * 2, "wide header + 2 bytes per delta");
        assert_eq!(MAX_SAFE_CHUNK_SIZE, 2 * n + FEATURE_HEADER_COMPACT_LEN - 2, "the 2·V + 5 arithmetic");
    }

    #[test]
    fn validate_chunk_size_accepts_safe_rejects_oversize() {
        assert!(validate_chunk_size(4096).is_ok(), "the packer default must pass");
        assert!(validate_chunk_size(MAX_SAFE_CHUNK_SIZE).is_ok(), "the boundary is inclusive");
        assert!(validate_chunk_size(MAX_SAFE_CHUNK_SIZE + 1).is_err(), "one over the cap is rejected");
        assert!(validate_chunk_size(8192).is_err(), "a chunk_size that could truncate is rejected (#2)");
    }

    #[test]
    fn validate_chunk_size_rejects_degenerate_minimum() {
        assert!(validate_chunk_size(MIN_CHUNK_SIZE).is_ok(), "the minimum is inclusive");
        assert!(validate_chunk_size(MIN_CHUNK_SIZE - 1).is_err(), "one under the floor is rejected");
        assert!(validate_chunk_size(1).is_err(), "a chunk_size that drops every feature is rejected");
    }

    #[test]
    fn streaming_matches_in_memory() {
        // Streaming output must be byte-identical to `serialize_lods` for the same 2-LOD pyramid
        // and POI + nav sections, so the fixture carries POIs of a few categories (two sharing one
        // pooled schedule) and a small nav graph.
        use crate::nav::{Edge, Node as NavNode};
        use crate::poi::Poi;
        use std::io::Cursor;

        let bbox = (0, 0, 1_000_000, 1_000_000);
        let styles = vec![Style {
            id: 1,
            z_index: 0,
            color: 0x1234,
            weight: 2,
            priority: 1,
            line_style: LineStyle::Solid,
            color2: None,
            fixed_width: false,
            terrain_layer: false,
        }];
        let lods = vec![
            LodLayer {
                max_mpp: Some(100.0),
                chunk_size: 256,
                root: Node::Leaf {
                    bbox,
                    features: vec![Feature {
                        style_id: 1,
                        kind: Kind::Line,
                        rings: vec![vec![(0.1, 0.1), (0.9, 0.9)]],
                    }],
                },
            },
            LodLayer {
                max_mpp: None,
                chunk_size: 256,
                root: Node::Leaf {
                    bbox,
                    features: vec![Feature {
                        style_id: 1,
                        kind: Kind::Line,
                        rings: vec![vec![(0.2, 0.2), (0.8, 0.8), (0.5, 0.1)]],
                    }],
                },
            },
        ];
        let poi = |subtype, lon, lat, name: Option<&str>, hours: Option<&str>| Poi {
            metadata: obc_formats::obcm::PoiMetadata {
                source: obc_formats::obcm::SourceId::osm(1, subtype as u64),
                approach: None,
            },
            access_nodes: Vec::new(),
            wikidata: None,
            wikipedia: None,
            subtype,
            lon_udeg: lon,
            lat_udeg: lat,
            name: name.map(String::from),
            from_node: true,
            hours: hours.and_then(crate::hours::parse),
            elevation_m: None,
            population: None,
        };
        let pois = vec![
            poi(1, 100_000, 100_000, Some("Brunnen"), None),
            poi(5, 200_000, 200_000, None, Some("Mo-Fr 08:00-18:00")),
            poi(17, 300_000, 300_000, Some("Apotheke"), Some("Mo-Fr 08:00-18:00")),
            poi(18, 400_000, 400_000, Some("Velowerkstatt"), Some("24/7")),
        ];
        let nav = NavGraph {
            nodes: vec![NavNode { id: 0, coord: (100_000, 100_000) }, NavNode { id: 1, coord: (200_000, 200_000) }],
            edges: vec![Edge {
                a: 0,
                b: 1,
                polyline: vec![(100_000, 100_000), (150_000, 160_000), (200_000, 200_000)],
                length_m: 15_700,
                kind: 0,
            }],
        };

        // Two profiles so the profile table is non-trivial and both paths must agree on its bytes.
        let profiles = vec![
            NavProfile { name: "Road".into(), highway: [16; 32], surface: [16; 8], climb_weight: 10 },
            NavProfile { name: "Gravel".into(), highway: [24; 32], surface: [32; 8], climb_weight: 8 },
        ];

        let (reference, ref_dropped) =
            serialize_lods(&lods, &styles, 0xABCD, bbox, &pois, &nav, &profiles, &mut NullElevation);
        assert_eq!(ref_dropped, 0, "nothing overflows in this fixture");

        let mut cur = Cursor::new(Vec::new());
        let (total, dropped) = serialize_lods_streaming(
            &mut cur,
            lods.len(),
            &styles,
            0xABCD,
            bbox,
            &pois,
            &[],
            &crate::peak_map::Peaks::default(),
            &nav,
            &profiles,
            &mut NullElevation,
            |i| (Some(lods[i].root.clone()), lods[i].chunk_size, lods[i].max_mpp),
        )
        .unwrap();

        assert_eq!(cur.into_inner(), reference, "streaming output must be byte-identical");
        assert_eq!(total as usize, reference.len());
        assert_eq!(dropped, 0, "and reports the same (zero) drop count");
    }
}
