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
/// packer's own literal in `build_poi_tree` / `build_nav_tree` (`host/obc-pack/src/serialize.rs`),
/// so a dense cluster stops recursing at the same place in both — and pinned by
/// `tests/pinning.rs::the_split_floor_matches_the_packers`, because a silent divergence here would
/// put records outside the leaf that indexes them.
pub const SPLIT_FLOOR: i64 = 10;

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

/// "First bin, in creation order, with at least `want` bytes free" in `O(log n)`.
///
/// The predicate is the *only* thing this accelerates: [`flatten`]'s placement is byte-for-byte the
/// linear scan `bins.iter().position(|b| b.len() + leaf_len <= chunk_size)` it replaces, and the
/// packer's own `flatten_nav_tree` still spells out. The scan is what made the nav rewrite
/// `O(leaves × chunks)` — at a country's ~250 k node chunks that is tens of billions of comparisons,
/// which measured as most of the assembler's nav phase.
///
/// A segment tree over per-bin free space, laid out as a complete binary tree with the bins at the
/// leaves: each internal node keeps the **maximum** free space below it, so the descent takes the
/// left child whenever it can hold the record and the right one otherwise — the leftmost, i.e.
/// first-created, bin that fits.
struct FirstFit {
    /// `1`-based complete binary tree; `tree[1]` is the root, leaf `k` lives at `leaves + k`.
    tree: Vec<usize>,
    leaves: usize,
    len: usize,
}

impl FirstFit {
    fn new() -> FirstFit {
        FirstFit { tree: vec![0; 2], leaves: 1, len: 0 }
    }

    /// Open a new bin with `free` bytes of room, returning its index.
    fn push(&mut self, free: usize) -> usize {
        if self.len == self.leaves {
            // Double the leaf array and rebuild: amortised O(1) per bin, and the rebuild is a
            // bottom-up max over a vector rather than a tree walk.
            let leaves = self.leaves * 2;
            let mut tree = vec![0usize; leaves * 2];
            for k in 0..self.len {
                tree[leaves + k] = self.tree[self.leaves + k];
            }
            for i in (1..leaves).rev() {
                tree[i] = tree[2 * i].max(tree[2 * i + 1]);
            }
            self.tree = tree;
            self.leaves = leaves;
        }
        let index = self.len;
        self.len += 1;
        self.set(index, free);
        index
    }

    /// Update bin `index`'s remaining free space.
    fn set(&mut self, index: usize, free: usize) {
        let mut i = self.leaves + index;
        self.tree[i] = free;
        while i > 1 {
            i /= 2;
            self.tree[i] = self.tree[2 * i].max(self.tree[2 * i + 1]);
        }
    }

    /// The first bin with at least `want` bytes free, or `None`.
    fn first_fit(&self, want: usize) -> Option<usize> {
        if self.tree[1] < want {
            return None;
        }
        let mut i = 1;
        while i < self.leaves {
            i = if self.tree[2 * i] >= want { 2 * i } else { 2 * i + 1 };
        }
        let index = i - self.leaves;
        debug_assert!(index < self.len, "the max is only ever non-zero on an open bin");
        Some(index)
    }
}

/// BFS-flatten a tree into `(index bytes, node count, chunk bytes, chunk count, dropped)`.
///
/// `pack_one` writes **one** record into the chunk buffer it is handed; this function owns the
/// capacity bound, exactly as the packer's `pack_poi_chunk` / `flatten_nav_tree` do. A record that
/// would not fit its chunk is **dropped and counted**, never written past `chunk_size` — the tree
/// already split every leaf to at most one chunk, so `dropped` is the safety net for the one case
/// the tree cannot split away (co-located records inside the [`SPLIT_FLOOR`] recursion floor).
/// Truncating a chunk silently would be worse than losing a record: a nav chunk with no `0xFF`
/// sentinel violates `OBCM_Spec.md` §8.3's no-straddle rule and decodes as garbage.
///
/// `bin_pack` selects the chunk policy:
///
/// - `false` — one chunk per non-empty leaf, padded to `chunk_size` (the §7.3 POI stride);
/// - `true` — **first-fit** over already-open chunks (the §8.2 v9 bin packing), so distinct leaves
///   may share a chunk id and a walk may hand a consumer the same record twice. That is the
///   documented contract, and the reason every consumer of nav records must be idempotent.
pub fn flatten<T: Point>(
    root: &Tree<T>,
    chunk_size: usize,
    bin_pack: bool,
    pack_one: &dyn Fn(&T, &mut Vec<u8>),
) -> (Vec<u8>, u32, Vec<u8>, u32, usize) {
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
    let mut free = FirstFit::new();
    let mut dropped = 0usize;
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
        let fits = if bin_pack { free.first_fit(leaf_len) } else { None };
        let bin = match fits {
            Some(c) => c,
            None => {
                bins.push(Vec::with_capacity(chunk_size));
                free.push(chunk_size)
            }
        };
        index.push(bin as u32 & !BRANCH_BIT);
        for p in points {
            if bins[bin].len() + p.record_len() > chunk_size {
                dropped += 1;
                continue; // co-located overflow inside one leaf — the packer's own safety net
            }
            pack_one(p, &mut bins[bin]);
        }
        free.set(bin, chunk_size - bins[bin].len());
    }

    let chunk_count = bins.len() as u32;
    let mut chunks = Vec::with_capacity(bins.len() * chunk_size);
    for mut b in bins {
        debug_assert!(b.len() <= chunk_size, "the per-record guard keeps every bin inside its chunk");
        b.resize(chunk_size, obc_formats::obcm::CHUNK_END);
        chunks.extend_from_slice(&b);
    }
    let mut index_bytes = Vec::with_capacity(index.len() * 4);
    for v in &index {
        index_bytes.extend_from_slice(&v.to_le_bytes());
    }
    (index_bytes, index.len() as u32, chunks, chunk_count, dropped)
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

    /// A record writer that honours its own declared `record_len`, so the capacity guard sees the
    /// same widths the tree budgeted with.
    fn pad(p: &P, out: &mut Vec<u8>) {
        out.extend_from_slice(&p.0.to_le_bytes());
        out.resize(out.len() + p.2 - 4, 0);
    }

    #[test]
    fn a_full_leaf_splits_and_children_stay_contiguous() {
        let bbox = (0, 0, 1_000_000, 1_000_000);
        let pts: Vec<P> = (0..8).map(|k| P(100_000 + k * 100_000, 100_000, 100)).collect();
        let tree = build(pts, bbox, 256);
        let (index, count, chunks, chunk_count, dropped) = flatten(&tree, 512, false, &pad);
        assert_eq!(dropped, 0);
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
        let packed = flatten(&tree, 512, true, &pad);
        let unpacked = flatten(&tree, 512, false, &pad);
        assert_eq!(packed.3, 1, "first-fit puts every leaf in one chunk");
        assert_eq!(unpacked.3, 4);
        assert_eq!((packed.4, unpacked.4), (0, 0), "nothing is dropped when every leaf fits");
    }

    /// The accelerated first-fit must place **exactly** where the linear scan did: the first bin in
    /// creation order with room, back-filling the slack an earlier large leaf left. This is the
    /// property `perf` must not have changed, so it is asserted against the naive scan itself over a
    /// deliberately awkward size sequence (large, small, large, small…, which is where next-fit and
    /// best-fit both diverge from first-fit).
    #[test]
    fn first_fit_placement_matches_the_naive_scan() {
        let sizes: Vec<usize> = (0..400).map(|k| [500usize, 40, 300, 60, 120, 200][k % 6]).collect();
        // Each leaf is one record, so leaf order is placement order and the comparison is direct.
        let mut naive: Vec<Vec<u8>> = Vec::new();
        let mut want: Vec<usize> = Vec::new();
        for &s in &sizes {
            let bin = match naive.iter().position(|b: &Vec<u8>| b.len() + s <= 512) {
                Some(c) => c,
                None => {
                    naive.push(Vec::new());
                    naive.len() - 1
                }
            };
            let was = naive[bin].len();
            naive[bin].resize(was + s, 0);
            want.push(bin);
        }
        let mut fit = FirstFit::new();
        let mut used: Vec<usize> = Vec::new();
        let mut got: Vec<usize> = Vec::new();
        for &s in &sizes {
            let bin = match fit.first_fit(s) {
                Some(c) => c,
                None => {
                    used.push(0);
                    fit.push(512)
                }
            };
            used[bin] += s;
            fit.set(bin, 512 - used[bin]);
            got.push(bin);
        }
        assert_eq!(got, want, "the segment tree must reproduce the linear scan bin for bin");
    }

    /// The guard the review restored: a leaf the tree could not split (co-located records past the
    /// recursion floor) keeps what fits and **counts** the rest, rather than writing past the chunk
    /// and having `resize` truncate it into a chunk with no sentinel.
    #[test]
    fn an_unsplittable_leaf_drops_loudly_instead_of_overflowing() {
        // Twelve records of 60 bytes at one coordinate: 720 > 512, and the 10-µdeg floor stops the
        // tree from splitting them apart.
        let pts: Vec<P> = (0..12).map(|_| P(500_000, 500_000, 60)).collect();
        let tree = build(pts, (499_999, 499_999, 500_001, 500_001), 100);
        let (_, _, chunks, chunk_count, dropped) = flatten(&tree, 512, true, &pad);
        assert_eq!(chunk_count, 1, "one leaf, one chunk");
        assert_eq!(chunks.len(), 512, "the chunk is exactly one chunk wide — nothing ran past it");
        assert_eq!(dropped, 4, "8 × 60 = 480 fit; the other four are counted, not written");
        assert_eq!(chunks[480], obc_formats::obcm::CHUNK_END, "the padding sentinel survives");
    }
}
