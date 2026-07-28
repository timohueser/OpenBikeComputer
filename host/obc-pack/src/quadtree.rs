//! Quadtree build — bucket a LOD's features into chunks.
//!
//! A node holds every feature reaching it. If their combined [`packed_size_budget`]
//! (an upper bound on the real packed bytes, including densify midpoints) fits the
//! chunk size it becomes a leaf; otherwise it splits four ways and hands each child
//! the features that reach it — contained ones whole, straddling ones clipped to the
//! child box — and recurses. The four children are built **in parallel**.
//!
//! This is a batch reformulation of the older one-feature-at-a-time insert+split:
//! since a leaf splits exactly when its features overflow the chunk and then
//! redistributes *all* of them, "does the running total ever exceed the chunk" and
//! "does the final total exceed the chunk" decide the same splits, and each child
//! still receives its features in input order — so the tree (and its serialized
//! bytes) are identical, only now the subtrees fan out across threads.
//!
//! Overlap/containment tests and clipping run in **degree space** (bbox / 1e6), so
//! leaf membership matches the node bounds the reader recomputes at render time.

use crate::geom::{clip_to_box, packed_size_budget, to_feature, Bounds, Geom};
use crate::progress::Progress;
use crate::serialize::Node;

/// Degree bounds `(min_lon, min_lat, max_lon, max_lat)` of an integer-µdeg box.
type DegBox = (f64, f64, f64, f64);

fn deg(bbox: (i64, i64, i64, i64)) -> DegBox {
    (bbox.0 as f64 / 1e6, bbox.1 as f64 / 1e6, bbox.2 as f64 / 1e6, bbox.3 as f64 / 1e6)
}

struct StoredFeature {
    style_id: u8,
    /// A simple geometry (Line/Polygon), post-flatten.
    geom: Geom,
    bounds: Bounds,
}

/// Below this many features a node builds its four children serially — the rayon
/// fork/join overhead isn't worth it for a small subtree (the big top-of-tree nodes
/// still fan out, which is where the time is).
const PARALLEL_MIN_FEATURES: usize = 2048;

/// Push a **simple** `geom` whose `bounds` are already known into the box `bbox`:
/// dropped if it misses, kept whole if fully inside, else clipped to the box and
/// flattened. `dbox` is `bbox` in degrees. The known `bounds` let the hot
/// contained-feature path skip a full coordinate scan — only a clip (which changes
/// the geometry) recomputes bounds, via [`flatten`].
fn place(
    bbox: (i64, i64, i64, i64),
    dbox: DegBox,
    style_id: u8,
    geom: Geom,
    bounds: Bounds,
    out: &mut Vec<StoredFeature>,
) {
    let (minxf, minyf, maxxf, maxyf) = dbox;
    // Reject: bbox misses the box entirely.
    if bounds.2 < minxf || bounds.0 > maxxf || bounds.3 < minyf || bounds.1 > maxyf {
        return;
    }
    // Contain: fully inside ⇒ keep whole, no clip, no bounds recompute.
    if bounds.0 >= minxf && bounds.2 <= maxxf && bounds.1 >= minyf && bounds.3 <= maxyf {
        out.push(StoredFeature { style_id, geom, bounds });
        return;
    }
    // Straddle: clip to the box and flatten whatever comes back.
    let clipped = clip_to_box(&geom, bbox);
    if !clipped.is_empty() {
        flatten(style_id, clipped, out);
    }
}

/// Append the simple parts of `geom` (recomputing each part's bounds) to `out`,
/// flattening `Multi`s. Used for geometry whose bounds aren't already known: a clip
/// result and each raw input feature at the root.
fn flatten(style_id: u8, geom: Geom, out: &mut Vec<StoredFeature>) {
    match geom {
        Geom::Line(_) | Geom::Polygon { .. } => {
            let bounds = geom.bounds();
            out.push(StoredFeature { style_id, geom, bounds });
        }
        Geom::Multi(parts) => {
            for p in parts {
                if !p.is_empty() {
                    flatten(style_id, p, out);
                }
            }
        }
        Geom::Empty => {}
    }
}

/// Build the subtree for `bbox` from `feats` (already clipped to `bbox`, in input
/// order). Leaf iff the features fit the chunk or the box hit the 10 µdeg split
/// floor; otherwise split four ways and build the children in parallel.
fn build_node(bbox: (i64, i64, i64, i64), chunk_size: usize, feats: Vec<StoredFeature>) -> Node {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    // Recursion guard: don't split below 10 µdeg on either axis.
    let splittable = max_lon - min_lon >= 10 && max_lat - min_lat >= 10;
    let total: usize = feats.iter().map(|f| packed_size_budget(&f.geom)).sum();
    if !splittable || total <= chunk_size {
        let features = feats.iter().filter_map(|f| to_feature(f.style_id, &f.geom)).collect();
        return Node::Leaf { bbox, features };
    }
    let n_feats = feats.len();

    // Floor-division midpoints (`div_euclid` floors toward −∞, matching the reader).
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    let boxes = [
        (min_lon, mid_lat, mid_lon, max_lat), // NW
        (mid_lon, mid_lat, max_lon, max_lat), // NE
        (min_lon, min_lat, mid_lon, mid_lat), // SW
        (mid_lon, min_lat, max_lon, mid_lat), // SE
    ];
    let dboxes = [deg(boxes[0]), deg(boxes[1]), deg(boxes[2]), deg(boxes[3])];

    // Hand each feature to the children it reaches, in NW,NE,SW,SE order, cloning
    // into every reached child but the last (which takes it by move). A feature
    // contained in one quadrant thus never allocates a throwaway copy.
    let mut buckets: [Vec<StoredFeature>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for f in feats {
        let StoredFeature { style_id, geom, bounds } = f;
        let mut reached = [false; 4];
        let mut last = None;
        for (i, &(minxf, minyf, maxxf, maxyf)) in dboxes.iter().enumerate() {
            let miss = bounds.2 < minxf || bounds.0 > maxxf || bounds.3 < minyf || bounds.1 > maxyf;
            if !miss {
                reached[i] = true;
                last = Some(i);
            }
        }
        let Some(last) = last else { continue };
        let mut geom = Some(geom);
        for i in 0..4 {
            if !reached[i] {
                continue;
            }
            let g = if i == last { geom.take().unwrap() } else { geom.as_ref().unwrap().clone() };
            place(boxes[i], dboxes[i], style_id, g, bounds, &mut buckets[i]);
        }
    }

    let [nw, ne, sw, se] = buckets;
    // Build the four subtrees; big nodes fan out across threads (only plain `Geom`,
    // which is Send, crosses threads, and each `clip_to_box` builds/consumes its GEOS
    // geometry on its own thread), small ones stay serial to dodge the join overhead.
    let children = if n_feats >= PARALLEL_MIN_FEATURES {
        let ((nw, ne), (sw, se)) = rayon::join(
            || (build_node(boxes[0], chunk_size, nw), build_node(boxes[1], chunk_size, ne)),
            || (build_node(boxes[2], chunk_size, sw), build_node(boxes[3], chunk_size, se)),
        );
        [nw, ne, sw, se]
    } else {
        [
            build_node(boxes[0], chunk_size, nw),
            build_node(boxes[1], chunk_size, ne),
            build_node(boxes[2], chunk_size, sw),
            build_node(boxes[3], chunk_size, se),
        ]
    };
    Node::Branch(Box::new(children))
}

/// Build one LOD's quadtree from its (already simplified) features and convert it
/// to a serializable [`Node`] tree. `features` yields `(style_id, geom)` with
/// geometry in degrees; empties are skipped (simplify can empty a geometry).
pub fn build_lod(
    features: impl IntoIterator<Item = (u8, Geom)>,
    global_bbox: (i64, i64, i64, i64),
    chunk_size: usize,
) -> Node {
    build_lod_with(features, global_bbox, chunk_size, &Progress::silent())
}

/// [`build_lod`], abandonable.
///
/// The root placement loop is the only interruptible part — `build_node`'s
/// recursion is a single divide-and-conquer over whatever it is given — so a
/// cancelled build stops feeding it and hands it nothing. That is deliberate
/// rather than lazy: the result is discarded either way, and returning an empty
/// tree in microseconds is what keeps the tail after a cancel short instead of
/// paying for a split of a country's worth of features nobody will read.
pub fn build_lod_with(
    features: impl IntoIterator<Item = (u8, Geom)>,
    global_bbox: (i64, i64, i64, i64),
    chunk_size: usize,
    progress: &Progress,
) -> Node {
    // Root: clip every feature to the global bbox (features at the truncated edge
    // can poke just outside it) and flatten Multis to simple parts, in input order.
    let dbox = deg(global_bbox);
    let mut root = Vec::new();
    for (style_id, geom) in features {
        if progress.is_cancelled() {
            root.clear();
            break;
        }
        if !geom.is_empty() {
            place_any(global_bbox, dbox, style_id, geom, &mut root);
        }
    }
    build_node(global_bbox, chunk_size, root)
}

/// Root entry: flatten a raw input `geom` (possibly a `Multi`) to its simple parts
/// and [`place`] each against the box, computing per-part bounds. Split-level
/// distribution uses [`place`] directly since it already knows each feature's
/// bounds; only the root's raw inputs need this bounds-computing wrapper.
fn place_any(bbox: (i64, i64, i64, i64), dbox: DegBox, style_id: u8, geom: Geom, out: &mut Vec<StoredFeature>) {
    match geom {
        Geom::Multi(parts) => {
            for p in parts {
                if !p.is_empty() {
                    place_any(bbox, dbox, style_id, p, out);
                }
            }
        }
        Geom::Empty => {}
        simple => {
            let bounds = simple.bounds();
            place(bbox, dbox, style_id, simple, bounds, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(coords: &[(f64, f64)]) -> Geom {
        Geom::Line(coords.to_vec())
    }

    fn leaf_feature_count(n: &Node) -> usize {
        match n {
            Node::Leaf { features, .. } => features.len(),
            Node::Branch(c) => c.iter().map(leaf_feature_count).sum(),
        }
    }
    fn is_branch(n: &Node) -> bool {
        matches!(n, Node::Branch(_))
    }

    // Containment/flatten/split/guard, no GEOS clip involved.

    #[test]
    fn insertion_keeps_contained_line() {
        let n = build_lod([(1u8, line(&[(0.0005, 0.0005), (0.0006, 0.0006)]))], (0, 0, 1000, 1000), 4096);
        assert_eq!(leaf_feature_count(&n), 1);
        assert!(!is_branch(&n));
    }

    #[test]
    fn multilinestring_flattens_into_parts() {
        let mls =
            Geom::Multi(vec![line(&[(0.0001, 0.0001), (0.0002, 0.0002)]), line(&[(0.0003, 0.0003), (0.0004, 0.0004)])]);
        let n = build_lod([(1u8, mls)], (0, 0, 1000, 1000), 4096);
        assert_eq!(leaf_feature_count(&n), 2, "the multilinestring flattens into its two line parts");
        assert!(!is_branch(&n));
    }

    #[test]
    fn split_on_size() {
        // ~15-point line, chunk_size 50: 12 + 15*4 = 72 > 50 → split into 4.
        let coords: Vec<(f64, f64)> = (0..15).map(|i| (0.0001 * i as f64, 0.0001 * i as f64)).collect();
        let n = build_lod([(1u8, line(&coords))], (0, 0, 1000, 1000), 50);
        assert!(is_branch(&n));
        if let Node::Branch(c) = &n {
            assert_eq!(c.len(), 4);
        }
    }

    #[test]
    fn polygon_preserved() {
        let poly = Geom::Polygon {
            exterior: vec![(0.0001, 0.0001), (0.0005, 0.0001), (0.0005, 0.0005), (0.0001, 0.0005), (0.0001, 0.0001)],
            interiors: vec![],
        };
        let n = build_lod([(1u8, poly)], (0, 0, 1000, 1000), 4096);
        match &n {
            Node::Leaf { features, .. } => {
                assert_eq!(features.len(), 1);
                assert_eq!(features[0].kind, crate::serialize::Kind::Polygon);
            }
            _ => panic!("expected leaf"),
        }
    }

    #[test]
    fn recursion_guard_blocks_tiny_box() {
        // Box 8 µdeg wide, chunk_size 1 → would split, but guard (<10) blocks it.
        let n = build_lod([(1u8, line(&[(0.0, 0.0), (0.000005, 0.000005)]))], (0, 0, 8, 8), 1);
        assert!(!is_branch(&n), "must not split below 10 µdeg");
    }

    #[test]
    fn no_split_below_chunk_size() {
        // 2-point line in a big bbox: 12 + 2*4 = 20 < 4096 → no split (stays a leaf).
        let n = build_lod([(1u8, line(&[(0.01, 0.01), (0.02, 0.02)]))], (0, 0, 40000, 40000), 4096);
        assert!(!is_branch(&n), "a feature under the chunk budget must not split the tree");
        assert_eq!(leaf_feature_count(&n), 1);
    }

    // --- Split + straddle ⇒ real GEOS clip -----------------------------------
    // Force a split and feed geometry crossing the new child boundaries, so every
    // piece goes through `clip_to_box`. The reassembled pieces must cover the original.

    type LeafRef<'a> = (&'a Vec<crate::serialize::Feature>, (i64, i64, i64, i64));

    /// Collect each leaf's `(features, bbox)` from the Node tree.
    fn leaves(n: &Node) -> Vec<LeafRef<'_>> {
        fn walk<'a>(n: &'a Node, out: &mut Vec<LeafRef<'a>>) {
            match n {
                Node::Leaf { features, bbox } => out.push((features, *bbox)),
                Node::Branch(c) => c.iter().for_each(|n| walk(n, out)),
            }
        }
        let mut out = Vec::new();
        walk(n, &mut out);
        out
    }

    /// A dense horizontal line straddling the NW/NE midline: the two top children
    /// each clip it via GEOS. Surviving segments must reassemble to the original
    /// x-span, and every clipped vertex must stay within its leaf's bbox.
    #[test]
    fn split_then_clip_straddling_line_reassembles() {
        // bbox 0..1.0°. Line at y=0.75° spanning x=0.05°..0.95°, 40 vertices →
        // 12 + 40*4 = 172 > 100 ⇒ split.
        let n = 40;
        let (x0, x1, y) = (0.05_f64, 0.95_f64, 0.75_f64);
        let coords: Vec<(f64, f64)> = (0..n).map(|i| (x0 + (x1 - x0) * i as f64 / (n - 1) as f64, y)).collect();
        let tree = build_lod([(1u8, line(&coords))], (0, 0, 1_000_000, 1_000_000), 100);
        assert!(is_branch(&tree), "the dense line must force a split");

        // Gather every clipped line vertex, grouped by leaf. The midline is at
        // x=0.5°; the line lies entirely in the top half so only NW + NE hold it.
        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut total_pts = 0usize;
        for (features, bbox) in leaves(&tree) {
            let (bminx, bminy, bmaxx, bmaxy) =
                (bbox.0 as f64 / 1e6, bbox.1 as f64 / 1e6, bbox.2 as f64 / 1e6, bbox.3 as f64 / 1e6);
            for f in features {
                assert_eq!(f.kind, crate::serialize::Kind::Line);
                for &(px, py) in &f.rings[0] {
                    assert!(px >= bminx - 1e-9 && px <= bmaxx + 1e-9, "vertex x {px} inside leaf [{bminx},{bmaxx}]");
                    assert!(py >= bminy - 1e-9 && py <= bmaxy + 1e-9, "vertex y {py} inside leaf [{bminy},{bmaxy}]");
                    min_x = min_x.min(px);
                    max_x = max_x.max(px);
                    total_pts += 1;
                }
            }
        }
        // Reassembly: the union of clipped pieces spans the original line exactly.
        assert!((min_x - x0).abs() < 1e-6, "left end preserved: {min_x} vs {x0}");
        assert!((max_x - x1).abs() < 1e-6, "right end preserved: {max_x} vs {x1}");
        // A clip at the midline ADDS a vertex on each side (the cut point), so the
        // total is at least the original count — never fewer (no vertices lost).
        assert!(total_pts >= n, "clip must not drop interior vertices (got {total_pts}, had {n})");
    }

    /// A polygon straddling the vertical midline: each half clips to its child box.
    /// The combined x-extent must still cover the original, and each clipped polygon
    /// stays a valid closed ring inside its leaf.
    #[test]
    fn split_then_clip_straddling_polygon_covers_original() {
        // Wide short rectangle centered on x=0.5° (straddles the midline), dense
        // enough to force a split.
        let (x0, x1, y0, y1) = (0.1_f64, 0.9_f64, 0.55_f64, 0.95_f64);
        let edge = 12; // points per long edge
        let mut ext = Vec::new();
        for i in 0..=edge {
            ext.push((x0 + (x1 - x0) * i as f64 / edge as f64, y0)); // bottom edge L→R
        }
        for i in 0..=edge {
            ext.push((x1 - (x1 - x0) * i as f64 / edge as f64, y1)); // top edge R→L
        }
        ext.push((x0, y0)); // close
        let poly = Geom::Polygon { exterior: ext, interiors: vec![] };
        let tree = build_lod([(1u8, poly)], (0, 0, 1_000_000, 1_000_000), 100);
        assert!(is_branch(&tree), "the dense polygon must force a split");

        let mut min_x = f64::INFINITY;
        let mut max_x = f64::NEG_INFINITY;
        let mut poly_pieces = 0;
        for (features, bbox) in leaves(&tree) {
            let bmaxx = bbox.2 as f64 / 1e6;
            let bminx = bbox.0 as f64 / 1e6;
            for f in features {
                assert_eq!(f.kind, crate::serialize::Kind::Polygon);
                poly_pieces += 1;
                let ring = &f.rings[0];
                assert!(ring.len() >= 4, "each clipped piece is a closed ring");
                assert_eq!(ring.first(), ring.last(), "clipped exterior stays closed");
                for &(px, _) in ring {
                    assert!(px >= bminx - 1e-9 && px <= bmaxx + 1e-9, "vertex x {px} inside leaf [{bminx},{bmaxx}]");
                    min_x = min_x.min(px);
                    max_x = max_x.max(px);
                }
            }
        }
        assert!(poly_pieces >= 2, "a straddling polygon clips into ≥2 leaf pieces, got {poly_pieces}");
        assert!((min_x - x0).abs() < 1e-6, "left extent preserved: {min_x} vs {x0}");
        assert!((max_x - x1).abs() < 1e-6, "right extent preserved: {max_x} vs {x1}");
    }

    /// A 2-point line spanning 3° densifies to ~100 extra vertices at pack time,
    /// far beyond what its raw vertex count suggests (12 + 2*4 = 20 bytes). The
    /// budget must count the densified size so the tree keeps splitting until
    /// every leaf's REAL packed bytes fit its chunk — under raw accounting the
    /// single leaf would overflow and `pack_chunk` would silently drop the line.
    #[test]
    fn budget_counts_densified_midpoints_so_nothing_drops() {
        let g = line(&[(0.1, 3.9), (3.1, 3.9)]);
        let tree = build_lod([(1u8, g)], (0, 0, 4_000_000, 4_000_000), 100);
        assert!(is_branch(&tree), "densified budget must force a split (raw accounting would not)");
        let mut populated = 0;
        for (features, bbox) in leaves(&tree) {
            if features.is_empty() {
                continue;
            }
            let (_, dropped) = crate::serialize::pack_chunk(features, bbox, 100);
            assert_eq!(dropped, 0, "every leaf's features must genuinely fit its chunk");
            populated += 1;
        }
        assert!(populated >= 2, "the long line lands in several leaves, got {populated}");
    }

    /// An `Empty` geometry (what simplify/clip can return) is dropped by `build_lod`,
    /// not panicked on or stored. Mixed with a real feature to prove only the Empty
    /// one is gone.
    #[test]
    fn build_lod_drops_empty_geometry() {
        let real = line(&[(0.1, 0.1), (0.2, 0.2)]);
        let tree = build_lod([(1u8, Geom::Empty), (2u8, real)], (0, 0, 1_000_000, 1_000_000), 4096);
        assert_eq!(leaf_feature_count(&tree), 1, "the Empty geom is dropped; only the real line remains");
        let only = leaves(&tree).into_iter().flat_map(|(f, _)| f.iter()).next().expect("one feature");
        assert_eq!(only.style_id, 2, "the surviving feature is the real line, not the dropped Empty");
    }
}
