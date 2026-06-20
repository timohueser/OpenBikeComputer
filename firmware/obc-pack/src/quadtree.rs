//! Quadtree build — a faithful port of `packer/obcm/quadtree.py`.
//!
//! Per LOD: insert every feature into a quadtree over the global bbox, splitting
//! a leaf once its accumulated size (`12 + pt_count*4` per feature) exceeds the
//! chunk size. Features fully inside a node are inserted as-is; straddling
//! features are clipped to the node box via GEOS `intersection` (see [`crate::geom`]).
//! The result converts to a [`serialize::Node`] tree, which the serializer walks
//! in BFS order.
//!
//! Overlap/containment tests and clipping run in **degree space** (bbox / 1e6),
//! exactly like the oracle, so leaf membership matches. The only expected
//! divergence from the Python output is last-digit clip differences from the GEOS
//! version (3.14 vs shapely's 3.13) — hence the render-diff + multiset gate.

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
    // Float boundaries (degrees), precomputed like quadtree.py.
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
        if bounds.2 < self.minxf
            || bounds.0 > self.maxxf
            || bounds.3 < self.minyf
            || bounds.1 > self.maxyf
        {
            return;
        }
        // Fast containment: fully inside ⇒ no clip, reuse the geometry + bounds.
        if bounds.0 >= self.minxf
            && bounds.2 <= self.maxxf
            && bounds.1 >= self.minyf
            && bounds.3 <= self.maxyf
        {
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
        if self.children.is_none() {
            // Leaf: accumulate, then split if over capacity.
            let delta = 12 + pt_count(&f.geom) * 4;
            self.features.push(f);
            self.current_size += delta;
            if self.should_split() {
                self.split();
            }
        } else {
            // Branch (split mid-insert): hand to every child; each rejects/clips.
            let children = self.children.as_mut().unwrap();
            for child in children.iter_mut() {
                child.insert(f.style_id, f.geom.clone(), f.bounds);
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
        // Floor-division midpoints (`div_euclid` matches Python `//` for negatives).
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
                let features =
                    self.features.iter().filter_map(|f| to_feature(f.style_id, &f.geom)).collect();
                Node::Leaf { bbox: self.bbox, features }
            }
        }
    }
}

/// Build one LOD's quadtree from its (already simplified) features and convert it
/// to a serializable [`Node`] tree. `features` yields `(style_id, geom)` with
/// geometry in degrees; empties are skipped (the oracle drops simplify-emptied
/// geoms before insert).
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

    // test_quadtree.py — these cases are all containment/flatten/split/guard, so
    // they pin the algorithm with no GEOS clip involved.

    #[test]
    fn insertion_keeps_contained_line() {
        let n = build_lod(
            [(1u8, line(&[(0.0005, 0.0005), (0.0006, 0.0006)]))],
            (0, 0, 1000, 1000),
            4096,
        );
        assert_eq!(leaf_feature_count(&n), 1);
        assert!(!is_branch(&n));
    }

    #[test]
    fn multilinestring_flattens_into_parts() {
        let mls = Geom::Multi(vec![
            line(&[(0.0001, 0.0001), (0.0002, 0.0002)]),
            line(&[(0.0003, 0.0003), (0.0004, 0.0004)]),
        ]);
        let mut root = QuadtreeNode::new((0, 0, 1000, 1000), 4096);
        let b = mls.bounds();
        root.insert(1, mls, b);
        let n = root.into_node();
        assert_eq!(leaf_feature_count(&n), 2);
    }

    #[test]
    fn split_on_size() {
        // ~15-point line, chunk_size 50: 12 + 15*4 = 72 > 50 → split into 4.
        let coords: Vec<(f64, f64)> =
            (0..15).map(|i| (0.0001 * i as f64, 0.0001 * i as f64)).collect();
        let n = build_lod([(1u8, line(&coords))], (0, 0, 1000, 1000), 50);
        assert!(is_branch(&n));
        if let Node::Branch(c) = &n {
            assert_eq!(c.len(), 4);
        }
    }

    #[test]
    fn polygon_preserved() {
        let poly = Geom::Polygon {
            exterior: vec![
                (0.0001, 0.0001),
                (0.0005, 0.0001),
                (0.0005, 0.0005),
                (0.0001, 0.0005),
                (0.0001, 0.0001),
            ],
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
}
