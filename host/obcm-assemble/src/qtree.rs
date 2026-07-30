//! The point quadtree the POI section (`OBCM_Spec.md` §7.2) and the nav node index (§8.2) share.
//!
//! Both are the §4 encoding over the file's global bbox, built with floor-division midpoints in
//! NW/NE/SW/SE order and a 10 µdeg recursion floor — the same split geometry `obc-pack`'s
//! `build_poi_tree` / `build_nav_tree` use, because the reader's `walk_leaves` resolves exactly one
//! subdivision rule and a second one would put records outside the leaf that indexes them.
//!
//! The two differ only in what "a leaf is full" means (POI: 14 fixed records; nav: 512 packed bytes)
//! and in how leaves map to chunks (POI: one each; nav: first-fit bin packing), so both are
//! parameters here rather than two copies of a tree.

use obc_formats::obcm::{BRANCH_BIT, EMPTY_LEAF};

use crate::grid::UBox;

/// Recursion floor, µdeg (~1 m): below this a leaf keeps whatever it holds. Identical to the
/// packer's, so a dense cluster stops recursing at the same place in both.
const SPLIT_FLOOR: i64 = 10;

/// A record the tree can bin: it has an absolute coordinate and an on-wire size.
pub trait Point {
    fn lat(&self) -> i32;
    fn lon(&self) -> i32;
    /// Packed length of this record, for the byte-budgeted split the nav tree needs.
    fn record_len(&self) -> usize;
}

/// A borrowed record bins exactly like an owned one, so a tree can hold references and never copy a
/// record on its way into a chunk.
impl<T: Point> Point for &T {
    fn lat(&self) -> i32 {
        (*self).lat()
    }
    fn lon(&self) -> i32 {
        (*self).lon()
    }
    fn record_len(&self) -> usize {
        (*self).record_len()
    }
}

/// A built tree: a leaf holding its records, or a branch over NW/NE/SW/SE.
pub enum Tree<T> {
    Leaf(Vec<T>),
    Branch(Box<[Tree<T>; 4]>),
}

/// Build the tree over `bbox`, splitting a leaf once its records exceed `capacity` **bytes**
/// (`Point::record_len` summed). A point exactly on a midline lands in the East / North child, which
/// keeps it inside that child's bbox for the query.
pub fn build<T: Point>(points: Vec<T>, bbox: UBox, capacity: usize) -> Tree<T> {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let packed: usize = points.iter().map(Point::record_len).sum();
    if packed <= capacity || max_lon - min_lon < SPLIT_FLOOR || max_lat - min_lat < SPLIT_FLOOR {
        return Tree::Leaf(points);
    }
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    let (mut nw, mut ne, mut sw, mut se) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for p in points {
        let west = (p.lon() as i64) < mid_lon;
        let south = (p.lat() as i64) < mid_lat;
        match (west, south) {
            (true, false) => nw.push(p),
            (false, false) => ne.push(p),
            (true, true) => sw.push(p),
            (false, true) => se.push(p),
        }
    }
    Tree::Branch(Box::new([
        build(nw, (min_lon, mid_lat, mid_lon, max_lat), capacity),
        build(ne, (mid_lon, mid_lat, max_lon, max_lat), capacity),
        build(sw, (min_lon, min_lat, mid_lon, mid_lat), capacity),
        build(se, (mid_lon, min_lat, max_lon, mid_lat), capacity),
    ]))
}

/// BFS-flatten a tree into `(index bytes, node count, chunk bytes, chunk count)`.
///
/// `pack_leaf` writes one leaf's records into the chunk buffer it is handed. `bin_pack` selects the
/// chunk policy:
///
/// - `false` — one chunk per non-empty leaf, padded to `chunk_size` (the §7.3 POI stride);
/// - `true` — **first-fit** over already-open chunks (the §8.2 v9 bin packing), so distinct leaves
///   may share a chunk id and a walk may hand a consumer the same record twice. That is the
///   documented contract, and the reason every consumer of nav records must be idempotent.
pub fn flatten<T: Point>(
    root: &Tree<T>,
    chunk_size: usize,
    bin_pack: bool,
    pack_leaf: &dyn Fn(&[T], &mut Vec<u8>),
) -> (Vec<u8>, u32, Vec<u8>, u32) {
    let mut nodes: Vec<&Tree<T>> = vec![root];
    let mut first_child: Vec<usize> = vec![0];
    let mut i = 0;
    while i < nodes.len() {
        if let Tree::Branch(children) = nodes[i] {
            first_child[i] = nodes.len();
            for c in children.iter() {
                nodes.push(c);
                first_child.push(0);
            }
        }
        i += 1;
    }

    let mut index: Vec<u32> = Vec::with_capacity(nodes.len());
    let mut bins: Vec<Vec<u8>> = Vec::new();
    for (idx, node) in nodes.iter().enumerate() {
        let points = match node {
            Tree::Branch(_) => {
                index.push(BRANCH_BIT | first_child[idx] as u32);
                continue;
            }
            Tree::Leaf(p) if !p.is_empty() => p,
            Tree::Leaf(_) => {
                index.push(EMPTY_LEAF);
                continue;
            }
        };
        let leaf_len: usize = points.iter().map(Point::record_len).sum();
        let bin = if bin_pack {
            match bins.iter().position(|b| b.len() + leaf_len <= chunk_size) {
                Some(c) => c,
                None => {
                    bins.push(Vec::with_capacity(chunk_size));
                    bins.len() - 1
                }
            }
        } else {
            bins.push(Vec::with_capacity(chunk_size));
            bins.len() - 1
        };
        index.push(bin as u32 & !BRANCH_BIT);
        pack_leaf(points, &mut bins[bin]);
    }

    let chunk_count = bins.len() as u32;
    let mut chunks = Vec::with_capacity(bins.len() * chunk_size);
    for mut b in bins {
        b.resize(chunk_size, obc_formats::obcm::CHUNK_END);
        chunks.extend_from_slice(&b);
    }
    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct P(i32, i32, usize);
    impl Point for P {
        fn lat(&self) -> i32 {
            self.0
        }
        fn lon(&self) -> i32 {
            self.1
        }
        fn record_len(&self) -> usize {
            self.2
        }
    }

    #[test]
    fn a_full_leaf_splits_and_children_stay_contiguous() {
        let bbox = (0, 0, 1_000_000, 1_000_000);
        let pts: Vec<P> = (0..8).map(|k| P(100_000 + k * 100_000, 100_000, 100)).collect();
        let tree = build(pts, bbox, 256);
        let (index, count, chunks, chunk_count) = flatten(&tree, 512, false, &|pts, out| {
            for p in pts {
                out.extend_from_slice(&p.0.to_le_bytes());
            }
        });
        assert_eq!(index.len(), count as usize * 4);
        assert!(chunk_count > 1, "the tree split");
        assert_eq!(chunks.len(), chunk_count as usize * 512, "one padded chunk per non-empty leaf");
        // Every branch points forward at a contiguous quadruple — the reader's walk invariant.
        let vals: Vec<u32> = index.chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect();
        for (i, v) in vals.iter().enumerate() {
            if v & BRANCH_BIT != 0 {
                let c = (v & !BRANCH_BIT) as usize;
                assert!(c > i && c + 3 < vals.len());
            }
        }
    }

    #[test]
    fn bin_packing_shares_chunks_between_leaves() {
        let bbox = (0, 0, 1_000_000, 1_000_000);
        // Four far-apart small leaves: one chunk holds them all under first-fit, four without it.
        let pts =
            vec![P(100_000, 100_000, 60), P(900_000, 100_000, 60), P(100_000, 900_000, 60), P(900_000, 900_000, 60)];
        let tree = build(pts, bbox, 100);
        let packed = flatten(&tree, 512, true, &|pts, out| out.resize(out.len() + pts.len() * 60, 0));
        let unpacked = flatten(&tree, 512, false, &|pts, out| out.resize(out.len() + pts.len() * 60, 0));
        assert_eq!(packed.3, 1, "first-fit puts every leaf in one chunk");
        assert_eq!(unpacked.3, 4);
    }
}
