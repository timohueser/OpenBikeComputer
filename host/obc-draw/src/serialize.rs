//! Drawing features and quadtree chunks.
use obc_formats::obcm::{CHUNK_END, FEATURE_FLAG_16BIT, FEATURE_FLAG_HOLES, FEATURE_FLAG_POLYGON, FEATURE_FLAG_WIDE};
use obc_map_core::serialize::{align_up, lay_out, scaled};
use obc_map_core::tree::{flatten_tree, FlattenTree, TreeWalk};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Line,
    Polygon,
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

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::obcm::{FEATURE_HEADER_COMPACT_LEN, FEATURE_HEADER_WIDE_LEN};
    use obc_map_core::serialize::{validate_chunk_size, MAX_SAFE_CHUNK_SIZE, MIN_CHUNK_SIZE};
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
}
