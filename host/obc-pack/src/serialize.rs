//! OBCM v13 serializer — lay out the `.obcm` bytes per `OBCM_Spec.md`.
//!
//! Deterministic: same feature list + quadtree → same output. Geometry arrives
//! already clipped + simplified; this module rounds lon/lat to microdegrees
//! (round-half-to-even), densifies long segments, delta-encodes rings, and lays out
//! the chunk / offset-table / index / LOD-table / header bytes. Geometry chunks are
//! packed **tight** and addressed by a per-LOD offset table (v11 §5,
//! [`serialize_tree`]); POI and nav chunks keep their fixed strides. The **POI section** (§7 of the
//! spec) is a per-category quadtree over fixed 36-byte point records (each carrying
//! a `hours_ref` u16 into the shared hours-pool section), reusing the same
//! BFS-flatten + u32 node encoding as the geometry tree. The trailing **nav-graph
//! section** (v8, §8) tiles the routable graph ([`crate::nav`]): a node quadtree
//! (§4 encoding again) over variable-length junction records with inline neighbor
//! coords, plus a chunked edge pool addressed by pool-relative byte offset.

use std::io::{self, Seek, SeekFrom, Write};

use obc_formats::obcm::{
    BRANCH_BIT, CHUNK_END, EMPTY_LEAF, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, FEATURE_FLAG_WIDE,
    FEATURE_HEADER_COMPACT_LEN, MAGIC, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT,
    STYLE_PRIORITY_MASK, STYLE_RECORD_LEN, STYLE_TERRAIN_LAYER_BIT,
};

// The OBCM constants the serializer lays out are owned by `obc-formats`; imported here (the
// `VERSION as OBCM_VERSION` rename is a module-local readability alias). Not re-exported.
use obc_formats::obcm::{
    nav_edge_id, nav_index_padding, OffsetScale, FILLER, HEADER_LEN, LOD_ENTRY_LEN, NAV_CHUNK_SIZE, NAV_DIR_LEN,
    NAV_EDGE_FIXED_LEN, NAV_EDGE_MAX_CHUNKS, NAV_EDGE_MAX_RECORDS_PER_CHUNK, NAV_MAX_DEGREE, NAV_MAX_PROFILES,
    NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN, NAV_PROFILE_RESERVED_LEN,
    NAV_SNAP_ANCHOR_GAP_M, NAV_SNAP_EDGE_MIN_M, NAV_SNAP_RECORD_LEN, POI_CATEGORY_COUNT, POI_CAT_ENTRY_LEN,
    POI_CHUNK_SIZE, POI_HOURS_BLOB_LEN, POI_HOURS_REF_NONE, POI_NAME_LEN, POI_RECORD_LEN, VERSION as OBCM_VERSION,
};

/// The `Offset Scale` every `.obcm` this packer writes carries (§1.1): `U = 16`, a 64 GiB
/// addressable interior. Writing it as a constant rather than a knob is itself a byte-determinism
/// pin — two bakes of one input agree on this byte, and a map past 64 GiB becomes a different value
/// here rather than a version bump.
pub const SCALE: OffsetScale = OffsetScale::DEFAULT;

/// The next unit boundary at or after `cursor` (§1.2's `align_up`). Every structure a header or
/// directory offset reaches begins on one; the `0..U-1` bytes this rounds past are [`FILLER`].
#[inline]
fn align_up(cursor: usize) -> usize {
    SCALE.align_up(cursor as u64).expect("a layout cursor never approaches u64::MAX") as usize
}

/// The filler run [`align_up`] implies at `cursor` — `0..U-1` bytes of `0xFF`.
#[inline]
fn filler_len(cursor: usize) -> usize {
    align_up(cursor) - cursor
}

/// The `uint32` a scaled offset field stores for byte offset `at`.
///
/// A scaled offset **cannot** name a byte that is not a multiple of `U`, so a non-boundary argument
/// here is a bug in the layout above it, not a rounding request — hence the panic rather than a
/// silent round. Every call site has just aligned the cursor it passes.
#[inline]
fn scaled(at: usize) -> u32 {
    SCALE
        .scaled(at as u64)
        .unwrap_or_else(|| panic!("byte {at} is not on a {}-byte unit boundary", SCALE.unit()))
        .units()
}

use crate::nav::{polyline_len_m, NavGraph};
use crate::poi::{table_row, Poi};
use obc_elevation::ElevationSource;
use obc_map_scene::ground_dist_m;

/// Max delta (microdegrees) before a segment is densified to keep deltas in
/// 16-bit range. Crate-visible so `geom::packed_size_budget` can count the
/// midpoints `densify` will insert.
pub(crate) const MAX_SEGMENT: i64 = 30_000;

/// Largest safe first delta from a feature's exterior anchor to a hole vertex. Unlike a real ring
/// edge this jump must never be densified: inserted points would become part of the hole boundary.
/// Keep it at the symmetric positive `i16` limit (rather than accepting the lone `-32768` value)
/// because anchor selection reasons about unsigned Chebyshev distance.
pub(crate) const MAX_HOLE_ANCHOR_DELTA: i64 = i16::MAX as i64;

// The serializer's blob length must equal `hours.rs`'s `Schedule::encode` width, or
// the pool bytes and the `POI_HOURS_BLOB_LEN` the directory advertises disagree.
const _: () = assert!(POI_HOURS_BLOB_LEN == crate::hours::BLOB_LEN, "hours blob length must match hours.rs");

// A cap-degree record must fit one chunk, or `pack_nav_chunk` would drop real junctions.
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);

/// Max endpoint-to-endpoint µdeg delta (lat **or** lon) a serialized adjacency piece may span, so
/// every neighbor entry's `dlat`/`dlon` fits `i16`. Mirrors `nav::MAX_ENDPOINT_DELTA_UDEG` (N1
/// bounds whole edges to this); the serializer's long-edge split re-checks it because splitting a
/// densified polyline into ≤ [`NAV_MAX_EDGE_PTS`] pieces can otherwise land a synthetic junction
/// farther than `i16` from its neighbor. `32 000 < i16::MAX (32 767)` keeps a margin.
const NAV_MAX_NEIGHBOR_DELTA: i64 = 32_000;

// Every non-empty v9 map carries at least one profile; the packer must never write zero (the
// reader treats `profile_count == 0` as malformed).
const _: () = assert!(NAV_MAX_PROFILES <= u8::MAX as usize);

/// Max polyline points of one serialized edge record: the record must never
/// straddle a chunk boundary (§8.4), so it is bounded by the chunk itself. An edge
/// whose **densified** polyline is longer (or whose pieces would span more than
/// [`NAV_MAX_NEIGHBOR_DELTA`]) is split at a vertex into pieces joined by synthetic
/// degree-2 nodes — routing-neutral, and it keeps the reader to one chunk-sized read
/// per edge. `(512 − 15) / 4 + 1 = 125`, unchanged from v8 (the +1-byte `way_kind`
/// head doesn't cross a 4-byte delta boundary).
pub const NAV_MAX_EDGE_PTS: usize = (NAV_CHUNK_SIZE - NAV_EDGE_FIXED_LEN) / 4 + 1;

/// A per-map routing profile ready to serialize into §8.6: a display name plus the two multiplier
/// tables in `u8` fixed-point 1/16 (indexed by way-kind's highway class 0..=31 / surface class
/// 0..=7; `16` = 1.0×, `0` = forbidden). Built + validated in [`crate::config`] (every non-zero
/// multiplier ≥ 16 so the great-circle A* heuristic stays admissible); the serializer only writes
/// the bytes. Kept small and `Clone` so the four defaults can be handed around cheaply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavProfile {
    /// Display name (UTF-8), truncated to [`NAV_PROFILE_NAME_LEN`] bytes on write, `0xFF`-padded.
    pub name: String,
    /// Multiplier per highway class (5-bit index, 0..=31). `16` = 1.0×, `0` = forbidden.
    pub highway: [u8; 32],
    /// Multiplier per surface class (3-bit index, 0..=7). Same encoding.
    pub surface: [u8; 8],
    /// §8.6 `Climb Weight` (v12): flat metres charged per metre of a neighbor entry's `Ascent M`.
    /// `0` = climb-blind. Needs no admissibility bound — the term is additive and non-negative.
    pub climb_weight: u8,
}

/// Largest `chunk_size` (bytes) that keeps every feature within the reader's
/// [`obc_reader::MAX_FEAT_PTS`] vertex cap. Any feature's packed bytes are at least
/// `FEATURE_HEADER_COMPACT_LEN + 2 · (total_vertices − 1)`: the smallest header v11
/// can write is the 7-byte compact one, and the densest geometry is 8-bit deltas at
/// 2 bytes per vertex after the anchor (holes and the wide header only add bytes).
/// So a chunk of `chunk_size` bytes carries at most `(chunk_size − 7) / 2 + 1`
/// vertices, and `4101` is where that hits 2048. Above this the reader **silently
/// truncates** past-cap vertices (`heapless` push fails, no error either side),
/// corrupting the feature's fill/stroke. (v10's bound was 4106 off the 12-byte
/// header; the compact header is what tightened it by 5 bytes.)
pub const MAX_SAFE_CHUNK_SIZE: usize = (obc_reader::MAX_FEAT_PTS - 1) * 2 + FEATURE_HEADER_COMPACT_LEN;

// The safe ceiling must itself fit the on-wire `u16` chunk_size field, or the bound is moot.
const _: () = assert!(MAX_SAFE_CHUNK_SIZE <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Smallest accepted `chunk_size` (bytes). The format decodes any positive size,
/// but below this even modest features exceed the chunk and [`pack_chunk`] drops
/// them wholesale — the pack "succeeds" and the map is silently near-empty. The
/// value is a judgment call (a few dozen mid-size features per chunk), kept in
/// lock-step with the schema's `chunk_size.minimum`.
pub const MIN_CHUNK_SIZE: usize = 256;

/// Reject a `chunk_size` outside [`MIN_CHUNK_SIZE`]..=[`MAX_SAFE_CHUNK_SIZE`]:
/// above the max the reader silently truncates vertices, below the min features
/// get dropped wholesale. Either way a misconfigured pack fails loudly.
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
    /// v10: dashed line style (flag bit 2). Ignored for polygons by the renderer.
    pub dashed: bool,
    /// v10: optional RGB565 secondary color (flag bit 3 + the trailing u16). `None` ⇒ bit clear and
    /// `0x0000` on the wire (which the reader ignores — black is a legit color, not a sentinel).
    pub color2: Option<u16>,
    /// #1095: fixed width (flag bit 4) — `weight` is device pixels, off the renderer's zoom ramp.
    pub fixed_width: bool,
    /// #1095: terrain layer (flag bit 5) — written here, consumed by the device's Settings toggle.
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

/// `v * 1e6` to microdegrees, **round-half-to-even** — NOT `f64::round`
/// (half-away-from-zero), which would shift vertices by a microdegree. The value is
/// integer-valued before the `as i64`, so the cast is exact. Crate-visible so
/// `geom::packed_size_budget` rounds exactly like the packer does.
#[inline]
pub(crate) fn to_udeg(v: f64) -> i64 {
    (v * 1e6).round_ties_even() as i64
}

/// Round one geometry ring exactly as [`pack_feature`] will and remove vertices that carry no
/// integer geometry. Kept crate-visible so the quadtree can enforce the hole-anchor invariant on
/// the exact coordinates the serializer will see, rather than on approximately equivalent floats.
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

/// Find the exterior vertex that minimizes the worst first-delta distance to all holes. Closed
/// rings are cyclic, so choosing a different first vertex is lossless. The returned distance is
/// exact for the serializer: every hole is independently rotated to its closest vertex after the
/// exterior is rotated to `index`.
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

/// Rotate a closed ring to the vertex requiring the shortest first delta from the feature anchor.
/// Ring rotation is geometry-preserving; it often avoids a quadtree split for a hole near one side
/// of a large exterior. Returns that shortest Chebyshev delta in microdegrees.
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

/// Append intermediate points between `p1` and `p2` (then `p2`) so no single
/// (dx, dy) step exceeds the 16-bit delta range, using an integer step count and
/// banker's-rounded midpoints.
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

/// Pack the style table (OBCM §2): `Count(u8)` then, sorted by id, `<BbHBBH>` per style — `id,
/// z_index, color, weight, flags, color2`. `flags = (priority-1) & STYLE_PRIORITY_MASK`, plus
/// `STYLE_DASHED_BIT` when `dashed`, `STYLE_HAS_COLOR2_BIT` when `color2` is `Some`,
/// `STYLE_FIXED_WIDTH_BIT` when `fixed_width` and `STYLE_TERRAIN_LAYER_BIT` when `terrain_layer`
/// (#1095). `color2` writes its RGB565 value when present, else `0x0000` (which the reader ignores,
/// bit 3 being clear). Bits 6-7 stay reserved and written `0`.
pub fn pack_style_dict(styles: &[Style]) -> Vec<u8> {
    let mut styles = styles.to_vec();
    styles.sort_by_key(|s| s.id);
    let mut data = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    data.push(styles.len() as u8);
    for s in &styles {
        let priority = (s.priority as i32).clamp(1, 4);
        let mut flags = (priority - 1) as u8 & STYLE_PRIORITY_MASK;
        if s.dashed {
            flags |= STYLE_DASHED_BIT;
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

/// Pack one feature: the v11 §5 header + delta-encoded rings. `node_bbox` is the containing leaf's
/// bbox; the exterior's first point becomes the anchor, stored relative to the leaf min corner.
///
/// The header is the **7-byte compact** form (`<BBBHH>`: style, flags, `pt_count u8`, `anchor u16`
/// ×2) whenever the exterior holds `1..=255` vertices *and* both anchor components land in
/// `0..=65535`, else the **12-byte wide** form (`<BBHii>`) with [`FEATURE_FLAG_WIDE`] set. A
/// leaf-relative anchor is small at fine LODs but genuinely isn't at coarse ones, where one leaf can
/// span far more than 65 535 µdeg — the wide form is that escape (and covers a negative anchor, which
/// clipping should never produce but the encoding must not silently mangle).
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
        // Polygon rings close implicitly in both the format and renderer. GEOS and the baker's
        // topology stages conventionally repeat the first vertex at the end, but serializing that
        // duplicate spends one frame point per ring without adding geometry. Lines keep their last
        // point even when they happen to be loops: their stroke path is not implicitly closed.
        let start_ref = if i == 0 {
            anchor_lon = raw_pts[0].0 - node_bbox.0;
            anchor_lat = raw_pts[0].1 - node_bbox.1;
            feature_anchor = raw_pts[0];
            feature_anchor
        } else {
            // OBCM gives holes no independent anchor: their first delta is relative to the
            // exterior anchor. Rotating a ring is lossless and minimizes that jump. If even the
            // nearest vertex is too far away, inserting bridge vertices would change the hole into
            // a long wedge (the PR #1299 coarse-map artifact). The quadtree must split first.
            let distance = rotate_ring_near_anchor(raw_pts, feature_anchor);
            debug_assert!(
                distance <= MAX_HOLE_ANCHOR_DELTA,
                "best exterior-anchor calculation disagreed with hole rotation"
            );
            feature_anchor
        };

        // Walk actual ring edges only. In particular, never densify the exterior-anchor → hole
        // jump: those intermediate coordinates would become vertices of the hole and the implicit
        // closing edge would turn the bridge into a triangular cutout.
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

    // The reader decodes a whole feature (exterior + holes) into one `MAX_FEAT_PTS`
    // buffer; past that `heapless` silently drops vertices. `validate_chunk_size`
    // guards it, but assert the real invariant in debug.
    debug_assert!(
        packed_rings.iter().map(|(n, _)| *n).sum::<usize>() <= obc_reader::MAX_FEAT_PTS,
        "feature vertex count exceeds the reader's MAX_FEAT_PTS — chunk_size too large?"
    );

    // The reader buffers ring lengths in a `MAX_FEAT_RINGS` heapless vec and
    // discards the whole feature past it; the quadtree splits (or floor-trims)
    // features over the cap before they reach here. (Also keeps the hole-count
    // byte below from ever wrapping u8.)
    debug_assert!(
        packed_rings.len() <= obc_reader::MAX_FEAT_RINGS,
        "feature ring count exceeds the reader's MAX_FEAT_RINGS — quadtree cap enforcement missed it"
    );

    debug_assert!(packed_rings[0].0 <= u16::MAX as usize, "exterior pt_count overflows the u16 field");
    let ext_pt_count = packed_rings[0].0;
    // Compact iff every compact field can hold its value; the anchor test is on the *packed* anchor,
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
/// distinct coordinates that round onto the same microdegree, and clipping can split a straight
/// edge with an exactly collinear middle vertex. Neither carries geometry once OBCM coordinates
/// are integer, so remove both before they consume file bytes and renderer frame points.
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

/// Pack features into one **tight** v11 chunk: the packed features back to back, then exactly one
/// `0xFF` [`CHUNK_END`] sentinel — no padding to `chunk_size`, which is now only the capacity bound
/// (§3). A feature that would overflow the chunk (and every feature after it) is dropped; the second
/// return value is the number dropped, so callers can warn instead of losing map content silently.
pub fn pack_chunk(features: &[Feature], node_bbox: (i64, i64, i64, i64), chunk_size: usize) -> (Vec<u8>, usize) {
    let mut data = Vec::new();
    let mut kept = 0usize;
    for f in features {
        let packed = pack_feature(f, node_bbox);
        // The `+ 1` reserves the sentinel byte, so the *sealed* chunk still fits the capacity —
        // which is what the reader validates an offset-table length against.
        if data.len() + packed.len() + 1 > chunk_size {
            break;
        }
        data.extend_from_slice(&packed);
        kept += 1;
    }
    data.push(CHUNK_END);
    (data, features.len() - kept)
}

/// A resident quadtree whose branches have four NW/NE/SW/SE children. Geometry,
/// POI, graph-node, and snap-anchor trees all share this traversal contract; their
/// leaf framing remains deliberately separate.
trait TreeWalk: Sized {
    fn children(&self) -> Option<&[Self; 4]>;
}

/// A quadtree whose leaf owns at most one chunk. Geometry and POI trees share this
/// framing; graph and snap trees use first-fit leaf binning instead.
trait FlattenTree: TreeWalk {
    /// Pack a leaf's payload into its chunk: `None` for an empty leaf (no chunk),
    /// else `(chunk_bytes, dropped)` where `dropped` is the chunk-overflow count.
    fn pack_leaf(&self, chunk_size: usize) -> Option<(Vec<u8>, usize)>;
}

/// Flatten any [`FlattenTree`] into `(index_bytes, node_count, chunks, dropped)` via BFS. Child
/// order and chunk-id assignment order are BFS, which fixes the byte layout: a branch's four
/// children are appended contiguously, so its first-child index is the node count at the moment it
/// is expanded (`child > idx` always — the invariant the reader's `walk_leaves` relies on).
/// `dropped` is the total chunk-overflow drop count across all leaves.
///
/// Chunks come back **one `Vec` per chunk**, not concatenated, because the two consumers frame them
/// differently: POI chunks are a fixed stride and just get joined ([`serialize_poi_section`]),
/// while geometry chunks are tight and need their lengths to build the v11 offset table
/// ([`serialize_tree`]).
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

/// Flatten one geometry quadtree into `(index_bytes, node_count, data_bytes, chunk_count,
/// dropped_features)` via BFS (the shared [`flatten_tree`]), framing the chunks as the §5
/// **chunk-data region**: a `chunk_count + 1` entry `uint32` offset table, `0..U-1` bytes of
/// [`FILLER`], then the chunks — each ending in its one `0xFF` sentinel and padded to the next unit
/// boundary.
///
/// v14 makes those offsets **scaled** (§5.1): entry `e` names byte `data_start + e * U`, where
/// `data_start = align_up(index_start + node_count * 4 + (chunk_count + 1) * 4, U)`. That last
/// rounding step is the only thing between the table and the chunks, and it is computable here
/// because `index_start` is itself a unit boundary — so the gap depends on the two counts alone.
/// `offsets[0]` is `0`, chunk `k` spans `offsets[k]..offsets[k+1]`, and `offsets[chunk_count]` is
/// the region's total chunk **units** (the reader keeps that last entry resident as its bound).
///
/// The table is written even for a chunkless LOD, where it is the single `0` entry. Because the
/// region ends on a unit boundary, the next LOD's index starts on one without the caller aligning
/// anything. `dropped_features` counts chunk-overflow drops across all leaves (see [`pack_chunk`]).
pub fn serialize_tree(root: &Node, chunk_size: usize) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    let (index_bytes, node_count, chunks, dropped) = flatten_tree(root, chunk_size);
    let table_len = (chunks.len() + 1) * 4;
    // `index_start` is a unit boundary, so only the two lengths behind it decide the filler run.
    let gap = filler_len(index_bytes.len() + table_len);
    let spans: Vec<usize> = chunks.iter().map(|c| align_up(c.len())).collect();
    let total: usize = spans.iter().sum();
    let mut data = Vec::with_capacity(table_len + gap + total);
    let mut offset = 0usize;
    data.extend_from_slice(&scaled(offset).to_le_bytes());
    for span in &spans {
        offset += span;
        data.extend_from_slice(&scaled(offset).to_le_bytes());
    }
    data.resize(data.len() + gap, FILLER);
    for (c, span) in chunks.iter().zip(&spans) {
        let end = data.len() + span;
        data.extend_from_slice(c);
        data.resize(end, FILLER);
    }
    debug_assert_eq!(offset, total, "the last offset is the region's chunk-byte total");
    debug_assert_eq!(data.len(), table_len + gap + total);
    (index_bytes, node_count, data, chunks.len() as u32, dropped)
}

// --- POI section (v7, spec §7) ------------------------------------------------

/// A POI record's absolute microdegree coordinates + the fields packed into its
/// 36-byte record (§7.3). Owned so the tree can move records into leaves.
/// `hours_ref` is the 0-based index into the map's hours-pool section (§7.5), or
/// [`POI_HOURS_REF_NONE`] when the POI has no pooled hours — resolved before
/// tree-building so it travels with the record.
struct PoiPoint {
    lon_udeg: i32,
    lat_udeg: i32,
    subtype: u8,
    name: Option<String>,
    hours_ref: u16,
}

/// One node of a category's POI quadtree, mirroring the geometry [`Node`] shape so
/// the shared [`flatten_tree`] serializes it to the identical index layout. A leaf
/// carries the points that fall inside its bbox; a branch its four NW/NE/SW/SE
/// children.
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

/// Pack one 36-byte POI record (§7.3): absolute `int32 lat, int32 lon`, `u8
/// subtype`, `u8 name_len`, a 24-byte `0xFF`-padded name, and a `u16 hours_ref`
/// (0-based hours-pool index, [`POI_HOURS_REF_NONE`] = none). The name is already
/// ASCII-folded + ≤ 24 bytes at ingest ([`crate::poi::normalize_name`]); truncate
/// defensively so a stray long name can never overrun the fixed field.
fn pack_poi_record(p: &PoiPoint) -> [u8; POI_RECORD_LEN] {
    let mut rec = [CHUNK_END; POI_RECORD_LEN];
    rec[0..4].copy_from_slice(&p.lat_udeg.to_le_bytes());
    rec[4..8].copy_from_slice(&p.lon_udeg.to_le_bytes());
    rec[8] = p.subtype;
    let name = p.name.as_deref().unwrap_or("");
    let bytes = name.as_bytes();
    let len = bytes.len().min(POI_NAME_LEN);
    rec[9] = len as u8;
    rec[10..10 + len].copy_from_slice(&bytes[..len]);
    // rec[10 + len .. 34] stays 0xFF (name pad).
    rec[34..36].copy_from_slice(&p.hours_ref.to_le_bytes());
    rec
}

/// Pack a leaf's POI records into one `chunk_size`-byte chunk (§7.3): as many fixed
/// 36-byte records as fit, back-to-back, then a `0xFF` **subtype** sentinel + `0xFF`
/// padding to `chunk_size`. Returns `(bytes, dropped)`. `build_poi_tree` splits a
/// leaf before it exceeds the chunk capacity, so `dropped` is 0 in practice; the cap
/// is the safety net for the one case the tree can't split away — more than
/// `chunk_size / 36` distinct POIs inside the 10-µdeg (~1 m) recursion floor, which
/// dedup makes effectively impossible. Truncating loudly beats corrupting the chunk.
fn pack_poi_chunk(points: &[PoiPoint], chunk_size: usize) -> (Vec<u8>, usize) {
    let capacity = chunk_size / POI_RECORD_LEN;
    let kept = points.len().min(capacity);
    let mut data = Vec::with_capacity(chunk_size);
    for p in &points[..kept] {
        data.extend_from_slice(&pack_poi_record(p));
    }
    // A 0xFF subtype byte ends the records (mirrors the geometry chunk's style-id
    // sentinel). `data.resize` writes it and the rest of the padding in one go —
    // never a truncation, since `kept * 32 <= chunk_size` by construction.
    data.resize(chunk_size, CHUNK_END);
    (data, points.len() - kept)
}

/// Build one category's POI quadtree over the **global bbox** (§7.2), splitting a
/// leaf once it holds more points than one chunk can carry. Split geometry matches
/// the reader's `walk_leaves` exactly: floor-division midpoints (`div_euclid(2)`),
/// NW/NE/SW/SE order, and a 10-µdeg recursion guard (identical to the geometry
/// `quadtree.rs`) so a dense cluster can't recurse forever. A point on a midline is
/// assigned deterministically (east of / north of the midpoint is `>= mid`), which
/// keeps it inside its leaf's bbox for the query.
fn build_poi_tree(points: Vec<PoiPoint>, bbox: (i64, i64, i64, i64), capacity: usize) -> PoiNode {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    // Fits a chunk, or the box is too small to subdivide — a leaf. The guard
    // matches the geometry quadtree's 10-µdeg floor so both agree on when to stop.
    if points.len() <= capacity || max_lon - min_lon < 10 || max_lat - min_lat < 10 {
        return PoiNode::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    // Partition into the four quadrants. West is `lon < mid`, South is `lat < mid`,
    // so a point exactly on a midline lands in the East / North child — inside that
    // child's bbox either way (the reader's boxes share the midline edge).
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

/// Serialize the full POI section (spec §7): the directory followed by each
/// category's quadtree index + data chunks, then the shared **hours-pool section**
/// (§7.5) at the tail. `pois` is the deduped classified list; each is bucketed by
/// its subtype's category ([`crate::poi::table_row`]). Category ids are
/// `1..=POI_CATEGORY_COUNT` and every one gets a directory entry, empty or not
/// (§7.1) — a map with no POIs writes six empty entries, never a zero offset.
/// `section_offset` is the section's absolute byte offset in the file, needed so the
/// directory's per-category `index_offset` fields and the `hours_pool_offset` are
/// file-absolute.
///
/// The hours pool is built **once** over the whole `pois` slice ([`build_hours_pool`]):
/// identical 29-byte weekly-schedule blobs collapse to one, and each POI's 0-based
/// `hours_ref` (or [`POI_HOURS_REF_NONE`]) is stamped onto its record **before**
/// tree-building so it travels into the right leaf. The pool bytes (`count u16` +
/// `count × 29-byte blobs`) are appended after every category's index+chunks, and
/// the directory records the pool's absolute offset + count.
pub fn serialize_poi_section(pois: &[Poi], global_bbox: (i64, i64, i64, i64), section_offset: usize) -> Vec<u8> {
    // Dedup the weekly schedules into a pool once over the whole list; `refs[k]` is
    // POI k's 0-based pool index (or `None` ⇒ no hours). Aligned to `pois`.
    let (pool, refs) = crate::hours::build_hours_pool(pois, |p| p.hours.as_ref());

    // Bucket points by category (id 1..=6). Index 0 is unused (no category 0). Each
    // point carries its resolved `hours_ref` so it survives tree-building + chunking.
    let mut by_cat: Vec<Vec<PoiPoint>> = (0..=POI_CATEGORY_COUNT as usize).map(|_| Vec::new()).collect();
    for (p, hours_ref) in pois.iter().zip(refs.iter()) {
        let cat = table_row(p.subtype).category() as usize;
        by_cat[cat].push(PoiPoint {
            lon_udeg: p.lon_udeg,
            lat_udeg: p.lat_udeg,
            subtype: p.subtype,
            name: p.name.clone(),
            hours_ref: hours_ref.unwrap_or(POI_HOURS_REF_NONE),
        });
    }

    // Records per chunk = chunk_size / record_len (512 / 36 = 14), so a leaf holds
    // at most that many before the tree splits.
    let capacity = POI_CHUNK_SIZE / POI_RECORD_LEN;

    // Flatten every category's tree first; the directory's index offsets are then
    // laid out sequentially after the fixed-size directory itself.
    struct CatBlock {
        cat_id: u8,
        index: Vec<u8>,
        node_count: u32,
        chunks: Vec<u8>,
        chunk_count: u32,
    }
    let mut blocks = Vec::with_capacity(POI_CATEGORY_COUNT as usize);
    for cat_id in 1..=POI_CATEGORY_COUNT {
        let pts = std::mem::take(&mut by_cat[cat_id as usize]);
        if pts.is_empty() {
            blocks.push(CatBlock { cat_id, index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0 });
            continue;
        }
        let root = build_poi_tree(pts, global_bbox, capacity);
        let (index, node_count, chunks, dropped) = flatten_tree(&root, POI_CHUNK_SIZE);
        debug_assert_eq!(dropped, 0, "fixed-size POI records never overflow a split leaf");
        // POI chunks keep the fixed `POI_CHUNK_SIZE` stride (§7.3) — no offset table, so the reader's
        // `PoiCatEntry::chunk_range` stays the plain `k * chunk_size` it has been since v6.
        let chunk_count = chunks.len() as u32;
        blocks.push(CatBlock { cat_id, index, node_count, chunks: chunks.concat(), chunk_count });
    }

    // Directory size: count byte + chunk_size u16 + one entry per category + the two
    // v7 hours-pool fields (offset u32 + count u16).
    let dir_len = 1 + 2 + POI_CATEGORY_COUNT as usize * POI_CAT_ENTRY_LEN + 4 + 2;

    // Lay categories out sequentially after the directory: [index][filler][chunks] per category,
    // empties contributing nothing but their directory entry. Every `Index Offset` is scaled, so
    // each index starts on a unit boundary, and a category's chunks begin at
    // `align_up(Index Offset * U + Index Node Count * 4, U)` — §7.1's one rounding step, the same
    // convention §3 and §8.1 use. 512 is a multiple of `U` at every legal scale, so the chunks
    // themselves need no filler between them and the region ends aligned for the next category.
    let mut payload = Vec::new();
    let mut cursor = align_up(section_offset + dir_len);
    let dir_gap = cursor - (section_offset + dir_len);
    payload.resize(dir_gap, FILLER);
    let mut cat_entries = Vec::with_capacity(POI_CATEGORY_COUNT as usize);
    for b in &blocks {
        cat_entries.push((b.cat_id, scaled(cursor), b.node_count, b.chunk_count));
        payload.extend_from_slice(&b.index);
        cursor += b.index.len();
        let gap = filler_len(cursor);
        payload.resize(payload.len() + gap, FILLER);
        cursor += gap;
        payload.extend_from_slice(&b.chunks);
        cursor += b.chunks.len();
    }

    // The hours-pool section begins at the first unit boundary at or after the last category's
    // chunks — those are whole 512-byte strides, so in practice `cursor` is already one.
    let gap = filler_len(cursor);
    payload.resize(payload.len() + gap, FILLER);
    let hours_pool_offset = cursor + gap;
    let hours_pool = pack_hours_pool(&pool);
    payload.extend_from_slice(&hours_pool);

    // Now emit the directory with the resolved offsets + pool fields.
    let mut dir = Vec::with_capacity(dir_len);
    dir.push(POI_CATEGORY_COUNT);
    dir.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
    for (cat_id, index_offset, node_count, chunk_count) in cat_entries {
        dir.push(cat_id);
        dir.extend_from_slice(&index_offset.to_le_bytes());
        dir.extend_from_slice(&node_count.to_le_bytes());
        dir.extend_from_slice(&chunk_count.to_le_bytes());
    }
    dir.extend_from_slice(&scaled(hours_pool_offset).to_le_bytes());
    dir.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    debug_assert_eq!(dir.len(), dir_len);

    let mut out = Vec::with_capacity(dir_len + payload.len());
    out.extend_from_slice(&dir);
    out.extend_from_slice(&payload);
    // The section ends on a unit boundary so the nav directory behind it can be named.
    out.resize(align_up(section_offset + out.len()) - section_offset, FILLER);
    out
}

/// Pack the hours-pool section (§7.5): `count u16` then `count × 29-byte` blobs,
/// back-to-back. `blob i` (as referenced by a record's `hours_ref`) lands at
/// `hours_pool_offset + 2 + i * 29`. An empty pool is just the `0` count (2 bytes).
fn pack_hours_pool(pool: &[[u8; POI_HOURS_BLOB_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + pool.len() * POI_HOURS_BLOB_LEN);
    out.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    for blob in pool {
        out.extend_from_slice(blob);
    }
    out
}

// --- Nav-graph section (v9, spec §8) --------------------------------------------

/// One adjacency entry of a junction record (§8.3), holding the neighbor's **absolute** µdeg coords
/// (the serializer turns them into `i16` deltas from the owning record at pack time), the resolved
/// wire `edge_id` (pool-relative byte offset), the edge's ground `cost_m` (written `u16`), its
/// `way_kind` class byte, and the v12 `ascent_m` — the integrated climb of riding the edge **toward
/// this neighbor**, which is the one field the two sides of an edge legitimately disagree on.
struct WireNeighbor {
    id: u32,
    lat: i32,
    lon: i32,
    edge_id: u32,
    cost_m: u32,
    way_kind: u8,
    ascent_m: u16,
}

/// A junction node ready to serialize: absolute µdeg coords, R1's dense id, and
/// its (capped) neighbor list. `record_len` is the §8.3 on-wire size.
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

/// One node of the nav quadtree, mirroring [`PoiNode`] so the shared
/// [`flatten_tree`] serializes it to the identical index layout. Leaves split on
/// **packed bytes** (records are variable-length), not record count.
enum NavTreeNode {
    Leaf(Vec<NavPoint>),
    Branch(Box<[NavTreeNode; 4]>),
}

/// Pack one v12 §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`, then one 17-byte
/// entry per neighbor (`id u32, dlat i16, dlon i16, edge_id u32, cost_m u16, way_kind u8,
/// ascent_m u16`).
/// Coordinates are absolute µdeg in the head, lat first (the §7.3/§8 record convention); each
/// neighbor's coord is stored as an `i16` **delta from this record's own lat/lon** (N1's edge
/// splits guarantee the delta fits), so relaxation reconstructs `neighbor = node + delta` exactly.
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

/// Bin-pack the tree's leaves into 512-byte node chunks (§8.3, the v9 optimization). `build_nav_tree`
/// already split every leaf to ≤ one chunk of records; v8 then gave each leaf its own chunk, wasting
/// the ~58% of every leaf that didn't fill 512 B. v9 assigns chunk ids **first-fit over the leaves in
/// BFS emission order**: each leaf's record block is placed in the **first already-open chunk with
/// room**, opening a new chunk only when none fits — a small leaf therefore back-fills the slack an
/// earlier large leaf left, which is what lifts the fill rate to ~90 %+ (plain next-fit, considering
/// only the newest chunk, stalls near ~75 % on real graphs). **Distinct leaves may share a chunk id**,
/// and because first-fit reaches back to earlier chunks those leaves can be spatially distant — so a
/// quadtree walk that visits several leaves sharing a chunk decodes that chunk's records once per
/// leaf, handing a consumer the same junction more than once. The reference consumers (A\* settle
/// match-by-id, snap best-tracking) are idempotent, so this is the documented §8.3 contract, not a
/// bug. A single leaf's records never straddle chunks; a pathological leaf larger than one chunk
/// (co-located junctions past the 10-µdeg split floor) keeps what fits and drops the rest, counted in
/// `dropped` (the same safety net v8's `pack_nav_chunk` had).
///
/// Returns `(index_bytes, node_count, chunk_bytes, chunk_count, dropped)` — the same shape as
/// [`flatten_tree`], so the directory-writing code is shared. BFS order and the branch/leaf/chunk-id
/// index encoding are identical to [`flatten_tree`]; only the leaf→chunk assignment differs (many
/// leaves per chunk instead of one).
fn flatten_nav_tree(root: &NavTreeNode) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
    flatten_binned_tree(root, NAV_CHUNK_SIZE)
}

/// A resident tree whose leaf records are first-fit packed without splitting a
/// leaf across chunks. Graph nodes and snap anchors have the same traversal and
/// binning invariants, but retain their own record sizing and wire encoding.
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
        // First-fit: the first open chunk whose remaining space holds the whole leaf; else a new one.
        // A leaf larger than a whole chunk can't fit anywhere, so it opens a fresh chunk and drops its
        // overflow (the tree builders make this effectively impossible).
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

    // Concatenate the bins, each 0xFF-padded to a full chunk (the padding's first byte lands on a
    // `degree` slot, giving the reader its end-of-chunk sentinel).
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

/// Build the node quadtree over the **global bbox** (§8.2), splitting a leaf once
/// its packed records exceed one chunk. Split geometry is identical to
/// [`build_poi_tree`] (floor-division midpoints, NW/NE/SW/SE, 10-µdeg floor), so
/// the reader's `walk_leaves` resolves it verbatim.
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

/// A working edge on its way into the pool: endpoints (dense node ids), the
/// **densified** polyline, the cost carried into both endpoints' records, and the
/// parent way's `kind` class byte (inherited by every split piece).
struct WorkEdge {
    a: u32,
    b: u32,
    polyline: Vec<(i32, i32)>,
    cost_m: u32,
    kind: u8,
}

/// One v13 §8.7 interior lookup anchor. It is deliberately not a graph node: the coordinate only
/// gets the router to a small candidate set, after which the full §8.4 geometry is projected
/// exactly and connected virtually to the edge's real endpoints.
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
/// records because they already live in the node quadtree. `ceil(length / 300)` intervals make
/// every gap at most 300 m even when the edge's rounded wire cost differs slightly from the sum of
/// its floating segment lengths.
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

/// Flatten the anchor quadtree with the graph node tree's first-fit leaf bin packing. Records keep
/// absolute coordinates because distinct spatial leaves may share one chunk.
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

/// Densify one nav polyline: insert midpoints on any segment whose lon **or** lat
/// delta exceeds [`MAX_SEGMENT`] so every §8.4 `(dlat, dlon)` fits an `i16` — the
/// exact [`densify`] the geometry rings use, over the same 30 000-µdeg threshold.
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

/// Pack one v9 §8.4 edge record: `length_m u32, pt_count u16, way_kind u8, anchor_lat i32,
/// anchor_lon i32`, then `pt_count - 1` × `(dlat i16, dlon i16)`. `length_m` **stays** in v9 (N3
/// sums it at emit for the displayed distance — weighted `g` is no longer a distance). The polyline
/// is already densified, so every delta fits.
fn pack_edge_record(e: &WorkEdge, out: &mut Vec<u8>) {
    out.extend_from_slice(&e.cost_m.to_le_bytes());
    out.extend_from_slice(&(e.polyline.len() as u16).to_le_bytes());
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

/// Pack the §8.6 profile table: `profiles.len()` consecutive 56-byte v12 records (`name [u8;12]`,
/// `highway_mult [u8;32]`, `surface_mult [u8;8]`, `climb_weight u8`, 3 reserved bytes written `0`).
/// The name is UTF-8 truncated to 12 bytes and `0xFF`-padded (the POI-name convention); the reserved
/// tail is **zero**, not `0xFF` — it is a reserved field, not a padded string. `profiles` is `1..=8`
/// (the packer never writes an empty table; the reader rejects `profile_count` outside that range).
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

/// Serialize the full nav-graph section (spec §8, v9) at absolute byte `section_offset`:
/// `[directory (40 B)][profile table (§8.6)][node quadtree index][node chunks][edge pool]
/// [snap-anchor quadtree index][snap-anchor chunks]`. The
/// **profile table is written immediately after the directory**, before the node index, so even an
/// empty graph (no routable ways) still carries its profiles; the section is **always present**.
/// `profiles` is `1..=8` entries (validated in [`crate::config`]).
///
/// Graph normalizations happen here, on working copies (the caller's [`NavGraph`] is untouched):
/// - **Densify + split.** Polylines are densified to the 30 000-µdeg segment bound; an edge whose
///   record would overflow one chunk ([`NAV_MAX_EDGE_PTS`]) **or** whose piece would span more than
///   [`NAV_MAX_NEIGHBOR_DELTA`] (so its neighbor delta wouldn't fit `i16`) is split at a vertex into
///   pieces joined by synthetic degree-2 nodes (each piece's cost re-measured over its sub-polyline
///   and `way_kind` inherited), so **no record straddles a chunk** and **every neighbor delta fits
///   `i16`**.
/// - **Degree cap.** A node keeps its first [`NAV_MAX_DEGREE`] adjacency entries (edge-pool order —
///   deterministic); the packer warns on stderr about the rest.
/// - **Bin-packed node chunks.** Leaves are first-fit-packed into shared 512-byte chunks
///   ([`flatten_nav_tree`]) — distinct leaves may reference one chunk.
///
/// Wire `edge_id` = the record's **pool-relative byte offset** (§8.4): the reader derives
/// `(chunk, offset)` as `id / 512`, `id % 512` with **zero resident index**. A self-loop edge
/// (`a == b`) contributes **one** adjacency entry, not two.
///
/// `terrain` is where the v12 `Ascent M` comes from. It is sampled **after** every split, over each
/// final piece's own polyline, because that is the only place the geometry an adjacency entry
/// describes actually exists — an edge's total climb cannot be divided among its pieces after the
/// fact (the dead-band is a fold over samples, not a length). Hand it
/// [`NullElevation`](obc_elevation::NullElevation) and every entry gets `0`: a decode-valid map that
/// routes exactly as v11 did, which is the degrade path *and* what keeps small test packs cheap.
/// Mints §8.4 `Edge Id`s as records are placed in the edge pool.
///
/// The writer got *simpler* at v14 — the id is a per-chunk record counter, so nothing has to know a
/// record's byte position any more — but the counter has one subtlety worth a type of its own.
/// **The ordinal restarts whenever the chunk does, which is not the same as "whenever a record was
/// pushed past a boundary".** A record whose length happens to divide the space left in its chunk
/// exactly ends flush with it: the next record opens the next chunk with no filler and no push, and
/// an ordinal tied to the push keeps counting straight across. Deriving both halves from the byte
/// the record lands on makes that case fall out instead of needing to be remembered — and it is not
/// hypothetical, since it is what a real Freiburg pack hit.
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
        // §8.4's producer cap, and the reason `0xFFFFFFFF` is an impossible id *unconditionally*.
        // The 19-byte minimum record puts the real maximum at 26, so this never binds today; it is
        // asserted so that it stays true if a future record shrinks.
        assert!(
            (self.ordinal as usize) < NAV_EDGE_MAX_RECORDS_PER_CHUNK,
            "an edge chunk may hold at most {NAV_EDGE_MAX_RECORDS_PER_CHUNK} records"
        );
        let id = nav_edge_id(chunk, self.ordinal).expect("chunk index and ordinal are both in field");
        self.ordinal += 1;
        id
    }
}

pub fn serialize_nav_section(
    graph: &NavGraph,
    profiles: &[NavProfile],
    global_bbox: (i64, i64, i64, i64),
    section_offset: usize,
    terrain: &mut dyn ElevationSource,
) -> Vec<u8> {
    let profile_table = pack_profile_table(profiles);
    // The profile table sits behind the 40-byte directory, at the first unit boundary past it — 40
    // is not a multiple of 16, so at the default scale the table starts at the directory's byte 48
    // and the eight bytes between them are §1.2 filler. That is the whole cost of scaling in this
    // section. A populated graph may then insert up to one sector of alignment run before its node
    // index; its exact size is known once the index has been built.
    let dir_gap = filler_len(section_offset + NAV_DIR_LEN);
    let profile_table_offset = section_offset + NAV_DIR_LEN + dir_gap;
    let unpadded_index_offset = profile_table_offset + profile_table.len();

    // Directory writer, shared by the empty and populated paths. `idx_off`/`edge_off` point at the
    // node index and edge pool; an empty graph passes `unpadded_index_offset` for both zero-length
    // regions.
    let write_dir = |out: &mut Vec<u8>,
                     idx_off: usize,
                     node_count: u32,
                     node_chunks: u32,
                     edge_off: usize,
                     edge_chunks: u32,
                     snap_idx_off: usize,
                     snap_node_count: u32,
                     snap_chunks: u32| {
        out.extend_from_slice(&scaled(idx_off).to_le_bytes()); // index_offset
        out.extend_from_slice(&node_count.to_le_bytes()); // index_node_count
        out.extend_from_slice(&node_chunks.to_le_bytes()); // node_chunk_count
        out.extend_from_slice(&scaled(edge_off).to_le_bytes()); // edge_pool_offset
        out.extend_from_slice(&edge_chunks.to_le_bytes()); // edge_chunk_count
        out.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes()); // chunk_size (pinned 512)
        out.extend_from_slice(&scaled(profile_table_offset).to_le_bytes()); // profile_table_offset
        out.push(profiles.len() as u8); // profile_count
        out.push(0u8); // reserved — a field, so `0`, unlike a gap
        out.extend_from_slice(&scaled(snap_idx_off).to_le_bytes()); // snap_index_offset
        out.extend_from_slice(&snap_node_count.to_le_bytes()); // snap_index_node_count
        out.extend_from_slice(&snap_chunks.to_le_bytes()); // snap_chunk_count
        debug_assert_eq!(out.len(), NAV_DIR_LEN);
        out.resize(NAV_DIR_LEN + dir_gap, FILLER);
    };

    if graph.nodes.is_empty() {
        // Empty graph: the directory (all regions zero-length, just past the profile table) + the
        // always-present profile table. The zero-length regions still have to be *nameable*, so
        // they point at the first unit boundary past the table rather than at its last byte.
        let empty_at = align_up(unpadded_index_offset);
        let mut out = Vec::with_capacity(empty_at - section_offset);
        write_dir(&mut out, empty_at, 0, 0, empty_at, 0, empty_at, 0, 0);
        out.extend_from_slice(&profile_table);
        out.resize(empty_at - section_offset, FILLER);
        return out;
    }

    // R1 assigns dense ids in push order — the serializer indexes `coords` by id.
    debug_assert!(graph.nodes.iter().enumerate().all(|(i, n)| n.id as usize == i), "node ids are dense");
    let mut coords: Vec<(i32, i32)> = graph.nodes.iter().map(|n| n.coord).collect();

    // Densify, then split anything over one chunk's worth of points OR whose piece would span more
    // than the i16-delta bound. Splitting appends synthetic nodes, so it runs before adjacency.
    let mut edges: Vec<WorkEdge> = Vec::with_capacity(graph.edges.len());
    for e in &graph.edges {
        let poly = densify_polyline(&e.polyline);
        // Fast path: short polyline whose endpoints (= junctions a,b) already fit the i16 neighbor
        // delta. N1 guarantees this for real packs, but a hand-built graph (tests) may not, so we
        // re-check the span here rather than trust it — a violating edge falls into the split loop.
        let (a, z) = (poly[0], *poly.last().unwrap());
        let span_ok = (a.0 as i64 - z.0 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA
            && (a.1 as i64 - z.1 as i64).abs() <= NAV_MAX_NEIGHBOR_DELTA;
        if poly.len() <= NAV_MAX_EDGE_PTS && span_ok {
            edges.push(WorkEdge { a: e.a, b: e.b, polyline: poly, cost_m: e.length_m, kind: e.kind });
            continue;
        }
        // Walk the long polyline in pieces bounded by BOTH the chunk point-cap and the i16 endpoint
        // span; each interior cut vertex becomes a synthetic junction. Densify caps each segment to
        // 30 000 µdeg (< the span bound), so a piece always advances ≥ 1 vertex (termination).
        // Pieces are re-measured so their costs sum to (within rounding of) the original.
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

    // Edge pool: records back-to-back in `edges` order, each pushed to the next chunk start if it
    // would straddle a boundary. Since v14 the wire `edge_id` is the packed `(chunk, ordinal)` pair
    // (§8.4), minted by [`EdgeIds`] from the byte the record lands on.
    let mut pool: Vec<u8> = Vec::new();
    let mut edge_ids: Vec<u32> = Vec::with_capacity(edges.len());
    let mut ids = EdgeIds::default();
    for e in &edges {
        let rec_len = NAV_EDGE_FIXED_LEN + (e.polyline.len() - 1) * 4;
        debug_assert!(rec_len <= NAV_CHUNK_SIZE, "split bounded every record to one chunk");
        let within = pool.len() % NAV_CHUNK_SIZE;
        if within + rec_len > NAV_CHUNK_SIZE {
            pool.resize(pool.len() + (NAV_CHUNK_SIZE - within), FILLER);
        }
        edge_ids.push(ids.mint(pool.len()));
        pack_edge_record(e, &mut pool);
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
    for (e, &edge_id) in edges.iter().zip(&edge_ids) {
        // §8.3 v12: the two entries of an edge differ in exactly one field. `a→b` books the climb of
        // riding the polyline forwards, `b→a` the climb of riding it backwards (= the forward
        // descent). A self-loop writes the forward one and nothing else, matching its single entry.
        let (ascent_ab, ascent_ba) = crate::nav::integrate_edge_ascent(&e.polyline, terrain);
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
        // Co-located junctions inside the 10-µdeg split floor — effectively
        // impossible in real OSM, but never silent.
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

    // Layout: [directory][filler][profile table][alignment run][node index][filler][node chunks]
    // [edge pool][alignment run][snap index][filler][snap chunks].
    //
    // `nav_index_padding` chooses each alignment run so that two things hold at once: the index
    // starts on a **unit** boundary (or no scaled offset could name it) and the fixed 512-byte
    // chunks behind it start on a **sector** boundary, so a full-chunk read is one card command.
    // The rounding step in `align_up(index_offset * U + node_count * 4, U)` is the slack that lets
    // both hold for every node count. The edge pool stays sector-aligned because the node region is
    // whole 512-byte chunks.
    //
    // Every gap here is `0xFF` since v14, where v13 wrote zeros for the alignment runs and `0xFF`
    // inside chunks. One fill byte, one rule (§1.2): a gap is `0xFF` and a reserved field is `0`.
    let index_pad = nav_index_padding(SCALE, unpadded_index_offset as u64, index.len() as u64)
        .expect("a nav index length never approaches u64::MAX");
    let index_offset = unpadded_index_offset + index_pad;
    let node_chunks_offset = align_up(index_offset + index.len());
    let index_gap = node_chunks_offset - (index_offset + index.len());
    let edge_pool_offset = node_chunks_offset + chunks.len();
    let unpadded_snap_index_offset = edge_pool_offset + pool.len();
    let snap_index_pad = if snap_node_count == 0 {
        filler_len(unpadded_snap_index_offset)
    } else {
        nav_index_padding(SCALE, unpadded_snap_index_offset as u64, snap_index.len() as u64)
            .expect("a snap index length never approaches u64::MAX")
    };
    let snap_index_offset = unpadded_snap_index_offset + snap_index_pad;
    let snap_gap = align_up(snap_index_offset + snap_index.len()) - (snap_index_offset + snap_index.len());
    let mut out = Vec::with_capacity(
        (snap_index_offset + snap_index.len() + snap_gap + snap_chunks.len()).saturating_sub(section_offset),
    );
    write_dir(
        &mut out,
        index_offset,
        node_count,
        chunk_count,
        edge_pool_offset,
        edge_chunk_count,
        snap_index_offset,
        snap_node_count,
        snap_chunk_count,
    );
    out.extend_from_slice(&profile_table);
    out.resize(out.len() + index_pad, FILLER);
    out.extend_from_slice(&index);
    out.resize(out.len() + index_gap, FILLER);
    out.extend_from_slice(&chunks);
    out.extend_from_slice(&pool);
    out.resize(out.len() + snap_index_pad, FILLER);
    out.extend_from_slice(&snap_index);
    out.resize(out.len() + snap_gap, FILLER);
    out.extend_from_slice(&snap_chunks);
    debug_assert_eq!(section_offset + out.len(), align_up(section_offset + out.len()), "the file tail stays aligned");
    out
}

/// The byte offset of the style table in every file this packer writes: the first unit boundary at
/// or after the 49-byte header (§1.2), which at the default `U = 16` is `64` — so `Style Offset` is
/// `4` and bytes `49..64` are [`FILLER`]. Reading the field rather than assuming the table follows
/// the header is what it was always for; v14 is simply the first version where the two differ.
const STYLE_OFFSET: usize = 64;
// Not just "past the header": §1.2 puts the style table on the *first unit boundary at or after*
// it, so 64 is a derivation with two halves and both are asserted. A scale change that moved the
// boundary used to leave this literal silently one gap behind.
const _: () = assert!(STYLE_OFFSET >= HEADER_LEN, "the style table cannot start inside the header");
const _: () = assert!(
    (STYLE_OFFSET as u64).is_multiple_of(SCALE.unit()),
    "and it must be a unit boundary a scaled offset can name"
);
const _: () =
    assert!(((STYLE_OFFSET - HEADER_LEN) as u64) < SCALE.unit(), "…the *first* such boundary, so the gap is one unit");

/// The 49-byte v14 OBCM header `<4sBiiiiIBIHIIBII>`: magic, version ([`OBCM_VERSION`]), bbox stored
/// as lat,lon,lat,lon, style offset, lod count, lod-table offset, marker color, the POI section
/// offset, the nav-graph section offset, and v14's `Offset Scale` plus the `Terrain Offset` /
/// `Terrain Length` pair. Every offset field here is **scaled** (§1.1). Shared by both serializers.
///
/// `obc-pack` writes a map with no embedded terrain, so the terrain pair is `(0, 0)` — §1.3's
/// unambiguous absence, since the header occupies byte `0` and no region can begin there. Splicing
/// a baked OBCT container into the tail is the assembler's step.
fn header_bytes(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
    poi_section_offset: usize,
    nav_section_offset: usize,
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
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

/// The header plus the §1.2 filler that carries it to the style table's unit boundary.
fn header_block(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
    poi_section_offset: usize,
    nav_section_offset: usize,
) -> Vec<u8> {
    let mut out =
        header_bytes(lod_count, marker_color, global_bbox, lod_table_offset, poi_section_offset, nav_section_offset);
    out.resize(STYLE_OFFSET, FILLER);
    out
}

/// Append one LOD-table entry `<fIIHI>`: max_mpp (`None` ⇒ `+inf`), index offset,
/// node count, chunk size, chunk count. The 18-byte layout is unchanged in v11; `cs` is now the
/// chunk **capacity bound** rather than a stride (§3).
fn push_lod_entry(table: &mut Vec<u8>, max_mpp: Option<f64>, index_offset: u32, nc: u32, cs: usize, cc: u32) {
    let mpp_f: f32 = max_mpp.map_or(f32::INFINITY, |v| v as f32);
    table.extend_from_slice(&mpp_f.to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&nc.to_le_bytes());
    table.extend_from_slice(&(cs as u16).to_le_bytes());
    table.extend_from_slice(&cc.to_le_bytes());
}

/// **Not an entry point — the in-memory parity oracle for [`serialize_lods_streaming`].**
///
/// It lays out the same complete `.obcm` byte stream (header field order, LOD table layout, the
/// bbox stored as lat,lon,lat,lon, the POI section §7, and the trailing nav-graph section §8) the
/// obvious way: build every LOD's bytes, then concatenate. Every production caller writes through
/// the streaming twin instead, which holds one tree at a time; this one exists so
/// `streaming_matches_in_memory` can assert the two are byte-identical, and because a corpus-building
/// test outside this crate wants a map in a `Vec<u8>` without a `Cursor`.
///
/// `pois` is the deduped classified POI list, `nav` the routable graph, and `profiles` the `1..=8`
/// routing profiles (§8.6) — all **always** get a section, empty or not. The second return value is
/// the total chunk-overflow feature drops (see [`pack_chunk`]). `terrain` is the OBCT source the
/// §8.3 `Ascent M` is integrated from; pass [`NullElevation`](obc_elevation::NullElevation) for a
/// map with no terrain.
// Eight positional arguments, one past clippy's default. Grouping them into a struct would only
// move the same eight names one indirection away and force every caller (and every test) to name a
// type to say "no styles, no POIs, an empty graph"; the streaming twin below carries the
// same list, and the two must stay in lockstep.
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
    let lod_table_offset = align_up(STYLE_OFFSET + style_data.len());
    let style_gap = lod_table_offset - (STYLE_OFFSET + style_data.len());

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

    // Each LOD's index is named by a scaled `Index Offset`, so it starts on a unit boundary; the
    // LOD table's own end is rounded up once and every region behind it ends aligned by
    // construction (`serialize_tree` pads its last chunk), so no further alignment is needed here.
    let payload_start = align_up(lod_table_offset + lod_count * LOD_ENTRY_LEN);
    let table_gap = payload_start - (lod_table_offset + lod_count * LOD_ENTRY_LEN);
    let mut cursor = payload_start;
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    let mut payload = Vec::new();
    for b in &blocks {
        push_lod_entry(&mut table, b.mpp, scaled(cursor), b.nc, b.cs, b.cc);
        payload.extend_from_slice(&b.ib);
        payload.extend_from_slice(&b.cb);
        cursor += b.ib.len() + b.cb.len();
    }

    // The POI section starts right after the last LOD's chunks (`cursor`); the
    // nav-graph section follows it at the file tail.
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset);
    let nav_section_offset = poi_section_offset + poi_section.len();
    let nav_section = serialize_nav_section(nav, profiles, global_bbox, nav_section_offset, terrain);

    let mut out = Vec::with_capacity(nav_section_offset + nav_section.len());
    out.extend_from_slice(&header_block(
        lod_count,
        marker_color,
        global_bbox,
        lod_table_offset,
        poi_section_offset,
        nav_section_offset,
    ));
    out.extend_from_slice(&style_data);
    out.resize(out.len() + style_gap, FILLER);
    out.extend_from_slice(&table);
    out.resize(out.len() + table_gap, FILLER);
    out.extend_from_slice(&payload);
    out.extend_from_slice(&poi_section);
    out.extend_from_slice(&nav_section);
    check_scale_covers(out.len() as u64);
    (out, dropped)
}

/// §1.1's one producer rule: **the scale MUST cover the file it writes** — `2^32 × U` at least the
/// file's total length. A file whose own bytes reach past what its scale can address is malformed,
/// and the producer that laid it out is the only party positioned to notice: a reader that never
/// resolves the last section never sees a thing wrong.
fn check_scale_covers(total: u64) {
    assert!(
        SCALE.covers(total),
        "a {total}-byte map does not fit the {}-byte-unit interior this packer writes (§1.1)",
        SCALE.unit()
    );
}

/// The production writer, and the streaming counterpart to [`serialize_lods`]: it writes the
/// **same** byte stream,
/// but builds, serializes, and drops one LOD tree at a time. Peak memory is ~one
/// tree + one LOD's chunk bytes rather than all trees plus the whole output buffer.
///
/// The header, style table, and LOD-table offset are known up front, but the POI
/// and nav section offsets are not (they depend on every LOD's serialized size). So
/// we write header + style table (with **placeholder** POI/nav offsets) + a
/// **zeroed** LOD table, stream each LOD's `index ++ chunks`, append the POI then
/// nav sections at the resulting cursors, then `seek` back and patch the LOD table
/// and both header offset fields. Byte-identical to `serialize_lods` for the same
/// trees, POIs and graph (asserted by `streaming_matches_in_memory`). Returns
/// `(bytes_written, dropped_features)` — the latter counts chunk-overflow drops
/// (see [`pack_chunk`]) so the CLI can warn.
///
/// `build(i)` yields LOD `i`'s `(root, chunk_size, max_mpp)`, called once per level
/// in order; each tree is dropped before the next call. The POI and nav sections
/// are built in memory (small — point/junction records, not geometry) after the
/// LODs stream out.
///
/// A `None` root writes an **empty region** for that level: `Index Node Count = 0`, `Chunk Count =
/// 0`, and the single-`0`-entry offset table `OBCM_Spec.md` §5.1 mandates for a chunkless LOD — 4
/// bytes of payload, and a reader walks it and finds nothing. That is not merely an optimisation of
/// the "leaf with no features" case (which still costs an index node): it is what a **cell artifact**
/// needs, because a cell writes the complete ladder with its out-of-band levels empty so that band
/// membership never appears in the bytes ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §3.1), and
/// `Index Node Count == 0` is the predicate a reader caches at mount to skip a level with no I/O at
/// all (§5.6).
#[allow(clippy::too_many_arguments)]
pub fn serialize_lods_streaming<W, F>(
    w: &mut W,
    lod_count: usize,
    styles: &[Style],
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    pois: &[Poi],
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
    let lod_table_offset = align_up(STYLE_OFFSET + style_data.len());
    let style_gap = lod_table_offset - (STYLE_OFFSET + style_data.len());
    let payload_start = align_up(lod_table_offset + lod_count * LOD_ENTRY_LEN);
    let table_gap = payload_start - (lod_table_offset + lod_count * LOD_ENTRY_LEN);

    // 1. Header + its filler to the style table's unit boundary (bbox stored lat,lon,lat,lon) —
    // needs no tree. The POI/nav section offsets aren't known until the LODs are sized, so write
    // `STYLE_OFFSET` placeholders (any unit-aligned byte will do; `0` is not one the writer may
    // name, since `scaled` refuses a non-boundary) and patch them in step 5.
    w.write_all(&header_block(lod_count, marker_color, global_bbox, lod_table_offset, STYLE_OFFSET, STYLE_OFFSET))?;

    // 2. Style table, then a zeroed LOD table we patch in step 5. Both runs of filler behind them
    // are written now, since their lengths are already known.
    w.write_all(&style_data)?;
    w.write_all(&vec![FILLER; style_gap])?;
    w.write_all(&vec![0u8; lod_count * LOD_ENTRY_LEN])?;
    w.write_all(&vec![FILLER; table_gap])?;

    // 3. Per-LOD: build → serialize → stream payload → drop the tree.
    let mut table = Vec::with_capacity(lod_count * LOD_ENTRY_LEN);
    let mut cursor = payload_start;
    let mut dropped = 0usize;
    for i in 0..lod_count {
        let (root, chunk_size, max_mpp) = build(i);
        let (ib, nc, cb, cc, lod_dropped) = match root {
            Some(root) => {
                let out = serialize_tree(&root, chunk_size);
                drop(root); // free the tree before writing this LOD / building the next
                out
            }
            // Empty region: no index, no chunk, and the mandatory single-`0` offset table — plus
            // the filler that carries the region to the next unit boundary, so the LOD behind it
            // still starts on one.
            None => {
                let mut cb = 0u32.to_le_bytes().to_vec();
                cb.resize(align_up(cb.len()), FILLER);
                (Vec::new(), 0u32, cb, 0u32, 0usize)
            }
        };
        dropped += lod_dropped;
        push_lod_entry(&mut table, max_mpp, scaled(cursor), nc, chunk_size, cc);
        w.write_all(&ib)?;
        w.write_all(&cb)?;
        cursor += ib.len() + cb.len();
    }

    // 4. The POI section begins at the current cursor (right after the last LOD);
    // the nav-graph section follows it at the file tail.
    let poi_section_offset = cursor;
    let poi_section = serialize_poi_section(pois, global_bbox, poi_section_offset);
    w.write_all(&poi_section)?;
    cursor += poi_section.len();
    let nav_section_offset = cursor;
    let nav_section = serialize_nav_section(nav, profiles, global_bbox, nav_section_offset, terrain);
    w.write_all(&nav_section)?;
    cursor += nav_section.len();

    // 5. Back-patch the LOD table and the header's two section-offset fields, then leave the cursor
    // at EOF. POI offset at header byte 32, nav at 36 (§1) — both **scaled**, like every offset the
    // header carries.
    check_scale_covers(cursor as u64);
    w.seek(SeekFrom::Start(lod_table_offset as u64))?;
    w.write_all(&table)?;
    w.seek(SeekFrom::Start(32))?;
    w.write_all(&scaled(poi_section_offset).to_le_bytes())?;
    w.write_all(&scaled(nav_section_offset).to_le_bytes())?;
    w.seek(SeekFrom::Start(cursor as u64))?;
    Ok((cursor as u64, dropped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_elevation::NullElevation;
    use obc_formats::obcm::{nav_edge_id_ordinal, FEATURE_HEADER_WIDE_LEN};

    /// The one case a push-driven ordinal gets wrong: a record that ends **flush** with its chunk.
    /// The record behind it opens the next chunk with no filler and no push, so an ordinal reset on
    /// the push would carry straight over the boundary and mint a duplicate id. A real Freiburg
    /// pack hits this, which is how the `31 records` assertion earned its keep.
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
        // Distinctness is the property that actually matters, and it is what a push-driven counter
        // would have broken here.
        let mut sorted = minted.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), minted.len(), "every record gets its own id");
    }

    /// …and the ordinary case, where the last record does *not* fit and is pushed.
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
        // Pins the mode `to_udeg` relies on: `round_ties_even` vs `f64::round`
        // (half-away-from-zero). Exact halves are representable in f64.
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

        // The bound runs the *other* way, and this is the honest statement of it: a chunk of
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
        // Streaming output must be byte-identical to `serialize_lods` for the same
        // 2-LOD pyramid *and* POI + nav sections — the two back-patched header
        // offsets and the trailing sections in the streaming path are easy to
        // drift, so the fixture carries POIs of a few categories (two sharing one
        // pooled schedule) *and* a small nav graph.
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
            dashed: false,
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
            subtype,
            lon_udeg: lon,
            lat_udeg: lat,
            name: name.map(String::from),
            from_node: true,
            hours: hours.and_then(crate::hours::parse),
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

        // Two profiles so §8.6 is non-trivial and the streaming/in-memory paths must agree on its
        // bytes as well as the graph's.
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
