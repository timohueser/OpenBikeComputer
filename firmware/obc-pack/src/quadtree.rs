//! Quadtree build — bucket a LOD's features into chunks.
//!
//! Per LOD: insert every feature into a quadtree over the global bbox, splitting
//! a leaf once its accumulated size (`12 + pt_count*4` per feature) exceeds the
//! chunk size. Features fully inside a node are inserted as-is; straddling
//! features are clipped to the node box via GEOS `intersection` (see [`crate::geom`]).
//! The result converts to a [`serialize::Node`] tree, which the serializer walks
//! in BFS order.
//!
//! Overlap/containment tests and clipping run in **degree space** (bbox / 1e6), so
//! leaf membership stays consistent with the node bounds the reader recomputes at
//! render time.

use crate::geom::{clip_to_box, pt_count, to_feature, Bounds, Geom};
use crate::serialize::Node;

struct StoredFeature {
    style_id: u8,
    /// A simple geometry (Line/Polygon), post-flatten.
    geom: Geom,
    bounds: Bounds,
}

struct QuadtreeNode {
    bbox: (i64, i64, i64, i64),
    chunk_size: usize,
    features: Vec<StoredFeature>,
    children: Option<Box<[QuadtreeNode; 4]>>,
    current_size: usize,
    // Float boundaries (degrees), precomputed.
    minxf: f64,
    minyf: f64,
    maxxf: f64,
    maxyf: f64,
}

impl QuadtreeNode {
    fn new(bbox: (i64, i64, i64, i64), chunk_size: usize) -> Self {
        QuadtreeNode {
            bbox,
            chunk_size,
            features: Vec::new(),
            children: None,
            current_size: 0,
            minxf: bbox.0 as f64 / 1e6,
            minyf: bbox.1 as f64 / 1e6,
            maxxf: bbox.2 as f64 / 1e6,
            maxyf: bbox.3 as f64 / 1e6,
        }
    }

    fn insert(&mut self, style_id: u8, geom: Geom, bounds: Bounds) {
        // Fast bbox-overlap reject (degree space).
        if bounds.2 < self.minxf || bounds.0 > self.maxxf || bounds.3 < self.minyf || bounds.1 > self.maxyf {
            return;
        }
        // Fast containment: fully inside ⇒ no clip, reuse the geometry + bounds.
        if bounds.0 >= self.minxf && bounds.2 <= self.maxxf && bounds.1 >= self.minyf && bounds.3 <= self.maxyf {
            self.flatten_and_process(style_id, geom, bounds);
        } else {
            let clipped = clip_to_box(&geom, self.bbox);
            if clipped.is_empty() {
                return;
            }
            let cb = clipped.bounds();
            self.flatten_and_process(style_id, clipped, cb);
        }
    }

    fn flatten_and_process(&mut self, style_id: u8, geom: Geom, bounds: Bounds) {
        match geom {
            Geom::Line(_) | Geom::Polygon { .. } => {
                self.process_clipped(StoredFeature { style_id, geom, bounds });
            }
            Geom::Multi(parts) => {
                for part in parts {
                    if !part.is_empty() {
                        let b = part.bounds();
                        self.flatten_and_process(style_id, part, b);
                    }
                }
            }
            Geom::Empty => {}
        }
    }

    fn process_clipped(&mut self, f: StoredFeature) {
        if let Some(children) = &mut self.children {
            // Branch (split mid-insert): hand to every child; each rejects/clips.
            for child in children.iter_mut() {
                child.insert(f.style_id, f.geom.clone(), f.bounds);
            }
        } else {
            // Leaf: accumulate, then split if over capacity.
            let delta = 12 + pt_count(&f.geom) * 4;
            self.features.push(f);
            self.current_size += delta;
            if self.should_split() {
                self.split();
            }
        }
    }

    fn should_split(&self) -> bool {
        self.current_size > self.chunk_size
    }

    fn split(&mut self) {
        let (min_lon, min_lat, max_lon, max_lat) = self.bbox;
        // Recursion guard: don't split below 10 µdeg on either axis.
        if max_lon - min_lon < 10 || max_lat - min_lat < 10 {
            return;
        }
        // Floor-division midpoints (`div_euclid` floors toward −∞, matching the reader).
        let mid_lon = (min_lon + max_lon).div_euclid(2);
        let mid_lat = (min_lat + max_lat).div_euclid(2);
        let cs = self.chunk_size;
        self.children = Some(Box::new([
            QuadtreeNode::new((min_lon, mid_lat, mid_lon, max_lat), cs), // NW
            QuadtreeNode::new((mid_lon, mid_lat, max_lon, max_lat), cs), // NE
            QuadtreeNode::new((min_lon, min_lat, mid_lon, mid_lat), cs), // SW
            QuadtreeNode::new((mid_lon, min_lat, max_lon, mid_lat), cs), // SE
        ]));

        // Re-insert the accumulated features into the new children.
        let moved = std::mem::take(&mut self.features);
        let children = self.children.as_mut().unwrap();
        for f in moved {
            for child in children.iter_mut() {
                child.insert(f.style_id, f.geom.clone(), f.bounds);
            }
        }
    }

    fn into_node(self) -> Node {
        match self.children {
            Some(children) => {
                let [a, b, c, d] = *children;
                Node::Branch(Box::new([a.into_node(), b.into_node(), c.into_node(), d.into_node()]))
            }
            None => {
                let features = self.features.iter().filter_map(|f| to_feature(f.style_id, &f.geom)).collect();
                Node::Leaf { bbox: self.bbox, features }
            }
        }
    }
}

/// Build one LOD's quadtree from its (already simplified) features and convert it
/// to a serializable [`Node`] tree. `features` yields `(style_id, geom)` with
/// geometry in degrees; empties are skipped (simplify can empty a geometry).
pub fn build_lod(
    features: impl IntoIterator<Item = (u8, Geom)>,
    global_bbox: (i64, i64, i64, i64),
    chunk_size: usize,
) -> Node {
    let mut root = QuadtreeNode::new(global_bbox, chunk_size);
    for (style_id, geom) in features {
        if geom.is_empty() {
            continue;
        }
        let bounds = geom.bounds();
        root.insert(style_id, geom, bounds);
    }
    root.into_node()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(coords: &[(f64, f64)]) -> Geom {
        Geom::Line(coords.to_vec())
    }

    // Helpers to inspect the produced Node tree.
    fn leaf_feature_count(n: &Node) -> usize {
        match n {
            Node::Leaf { features, .. } => features.len(),
            Node::Branch(c) => c.iter().map(leaf_feature_count).sum(),
        }
    }
    fn is_branch(n: &Node) -> bool {
        matches!(n, Node::Branch(_))
    }

    // These cases are all containment/flatten/split/guard, so they pin the
    // algorithm with no GEOS clip involved.

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
        let mut root = QuadtreeNode::new((0, 0, 1000, 1000), 4096);
        let b = mls.bounds();
        root.insert(1, mls, b);
        let n = root.into_node();
        assert_eq!(leaf_feature_count(&n), 2);
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
        let mut root = QuadtreeNode::new((0, 0, 1000, 1000), 4096);
        let b = poly.bounds();
        root.insert(1, poly, b);
        let n = root.into_node();
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
        // 2-point line in a big bbox: 12 + 2*4 = 20 < 4096 → no split.
        let mut root = QuadtreeNode::new((0, 0, 40000, 40000), 4096);
        let g = line(&[(0.01, 0.01), (0.02, 0.02)]);
        let b = g.bounds();
        root.insert(1, g, b);
        assert!(!root.should_split());
    }

    // --- Split + straddle ⇒ real GEOS clip (issue #95, item 1) ----------------
    // The six tests above all use fully-contained geometry, so the `else { clip }`
    // arm of `insert` (quadtree.rs ~61) was never reached. These force a split and
    // feed geometry that crosses the new child boundaries, so every clipped piece
    // goes through `clip_to_box`. The reassembled pieces must cover the original.

    /// A leaf as `(features, bbox)`, the unit `leaves` collects.
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

    /// A long horizontal line across the top half of the bbox, dense enough to force
    /// a split. After the split it straddles the NW/NE vertical midline, so the two
    /// top children each clip it via GEOS. The surviving clipped segments must
    /// reassemble to exactly the original x-span `[min_x, max_x]`, and every clipped
    /// vertex must stay within its leaf's bbox (the clip is the only thing keeping a
    /// feature inside its node — a bad clip-box ring order or `/1e6` scale would push
    /// vertices out of bounds or drop the join point).
    #[test]
    fn split_then_clip_straddling_line_reassembles() {
        // bbox 0..1.0° in both axes (0..1_000_000 µdeg). Horizontal line at y=0.75°
        // (top half) spanning x = 0.05° .. 0.95°, 40 vertices → 12 + 40*4 = 172 > 100
        // ⇒ split.
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
                    // Each clipped vertex sits inside (or on) its leaf box — the clip
                    // is what enforces this.
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

    /// A polygon straddling the bbox's vertical midline splits across NW/NE after a
    /// split: each half is clipped to its child box. The two clipped polygons'
    /// combined x-extent must still cover the original `[min_x, max_x]`, and each
    /// clipped polygon stays a valid closed ring inside its leaf. Exercises the
    /// polygon branch of `clip_to_box`/`from_geos` under a real split.
    #[test]
    fn split_then_clip_straddling_polygon_covers_original() {
        // A wide, short rectangle centered on x=0.5° so it straddles the midline,
        // tall enough only in the top half. Make it dense (many edge points) so the
        // accumulated size forces a split.
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

    /// An `Empty` geometry (what simplify/clip can return) is dropped by `build_lod`
    /// (quadtree.rs ~159), not panicked on or stored. Pairs with the geom-level
    /// simplify tests: confirms the consumer honors the drop contract. Mixed with a
    /// real feature so we can prove only the Empty one is gone.
    #[test]
    fn build_lod_drops_empty_geometry() {
        let real = line(&[(0.1, 0.1), (0.2, 0.2)]);
        let tree = build_lod([(1u8, Geom::Empty), (2u8, real)], (0, 0, 1_000_000, 1_000_000), 4096);
        assert_eq!(leaf_feature_count(&tree), 1, "the Empty geom is dropped; only the real line remains");
        let only = leaves(&tree).into_iter().flat_map(|(f, _)| f.iter()).next().expect("one feature");
        assert_eq!(only.style_id, 2, "the surviving feature is the real line, not the dropped Empty");
    }
}
