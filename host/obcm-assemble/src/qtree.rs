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
//!
//! # …and the same tree without the points (#1116 D4)
//!
//! [`build`] takes its points **by value** and [`flatten`] packs every chunk into a `Vec<u8>`, so
//! together they are the whole node set plus the whole §8.2/§8.3 output resident at once — a
//! gigabyte and change at DACH scale, and the last thing in the merge that was.
//!
//! [`flatten_streaming`] is the same tree over a *stream*. The shape is a pure function of the
//! points' coordinates, their record sizes, the capacity and the recursion floor, so it can be
//! recovered from a stream sorted in **tree order** ([`tree_key`] — the quadrant digits of the
//! descent, concatenated) without the points ever being addressable: a leaf is a contiguous run of
//! that stream, and the two walks below turn the runs into the index, the bin packing and a
//! placement plan the caller emits chunks from.
//!
//! The equivalence is not argued, it is *tested*: `the_streaming_tree_is_the_tree_build_and_flatten_
//! make` runs both over the same random point sets and compares the index and the chunk bytes.

use obc_formats::obcm::{BRANCH_BIT, EMPTY_LEAF};

use crate::extsort::{ExternalSort, SpillWriter};
use crate::grid::UBox;
use crate::scratch::{ScratchId, ScratchStore};
use crate::{Error, Result};

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
///
/// The free-space counters are `u32` rather than `usize`: a chunk is 512 bytes, and at a country's
/// millions of bins the halved node width is the difference between the accelerator being a rounding
/// error and it being tens of megabytes (#1116 D4, where it is the one structure left that is sized
/// by the *output*).
struct FirstFit {
    /// `1`-based complete binary tree; `tree[1]` is the root, leaf `k` lives at `leaves + k`.
    tree: Vec<u32>,
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
            let mut tree = vec![0u32; leaves * 2];
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
        debug_assert!(free <= u32::MAX as usize, "a chunk is 512 bytes; free space is a u32");
        let mut i = self.leaves + index;
        self.tree[i] = free as u32;
        while i > 1 {
            i /= 2;
            self.tree[i] = self.tree[2 * i].max(self.tree[2 * i + 1]);
        }
    }

    /// How much room bin `index` has left.
    fn free(&self, index: usize) -> usize {
        self.tree[self.leaves + index] as usize
    }

    /// The first bin with at least `want` bytes free, or `None`.
    fn first_fit(&self, want: usize) -> Option<usize> {
        let Ok(want) = u32::try_from(want) else {
            return None; // a leaf larger than any chunk never fits an open bin
        };
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

// -------------------------------------------------------------------------------------------------
// The same tree, over a stream (#1116 D4)
// -------------------------------------------------------------------------------------------------

/// Levels the [`tree_key`] descent can encode: two bits each in a `u64`.
///
/// The assembly bbox is a square of `2^span` µdeg with `span ≤ 29` (`grid.rs`), and the [`SPLIT_FLOOR`]
/// stops the descent once a side is under 10 — so a real tree is at most 27 deep and this is a
/// bound, not a policy. A box that would need more is refused rather than silently truncated, because
/// two points whose keys agreed only because the key ran out of bits would swap places.
const MAX_LEVELS: u32 = 32;

/// The digit that selects a depth-`d + 1` node from its depth-`d` parent lives here.
const fn digit_shift(depth: u32) -> u32 {
    62 - 2 * depth
}

/// The mask of a depth-`d` node's own prefix bits.
const fn prefix_mask(depth: u32) -> u64 {
    if depth == 0 {
        0
    } else {
        !0u64 << (64 - 2 * depth)
    }
}

/// Whether `bbox` is past the recursion floor — [`build`]'s own test, stated once.
fn at_floor(bbox: UBox) -> bool {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    max_lon - min_lon < SPLIT_FLOOR || max_lat - min_lat < SPLIT_FLOOR
}

/// The four children of `bbox`, in [`build`]'s own NW/NE/SW/SE order.
fn children(bbox: UBox) -> [UBox; 4] {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let mid_lon = (min_lon + max_lon).div_euclid(2);
    let mid_lat = (min_lat + max_lat).div_euclid(2);
    [
        (min_lon, mid_lat, mid_lon, max_lat),
        (mid_lon, mid_lat, max_lon, max_lat),
        (min_lon, min_lat, mid_lon, mid_lat),
        (mid_lon, min_lat, max_lon, mid_lat),
    ]
}

/// **Tree order**: the quadrant digits of a point's descent through `bbox`, concatenated two bits at
/// a time from the most significant end (NW = 0, NE = 1, SW = 2, SE = 3 — [`build`]'s child order).
///
/// Sorting by this key puts every subtree's points in one contiguous run, in child order, which is
/// what lets [`flatten_streaming`] recover the tree from a stream. The descent is [`build`]'s own,
/// midline rule and floor included, so a point lands under exactly the prefix of the leaf that would
/// hold it — and two points inside one floor-bounded box get the *same* key, which is why the sort
/// key ends in the record's input order.
pub fn tree_key(lat: i32, lon: i32, bbox: UBox) -> u64 {
    let mut box_ = bbox;
    let mut key = 0u64;
    let mut depth = 0;
    while !at_floor(box_) && depth < MAX_LEVELS {
        let (min_lon, min_lat, max_lon, max_lat) = box_;
        let mid_lon = (min_lon + max_lon).div_euclid(2);
        let mid_lat = (min_lat + max_lat).div_euclid(2);
        let west = (lon as i64) < mid_lon;
        let south = (lat as i64) < mid_lat;
        let digit = ((south as u64) << 1) | (!west) as u64;
        key |= digit << digit_shift(depth);
        box_ = children(box_)[digit as usize];
        depth += 1;
    }
    key
}

/// The deepest a tree over `bbox` can go, or `None` when that is more than [`MAX_LEVELS`].
///
/// A child's side is at most `⌈parent / 2⌉` (the midline is a floor-division, so one side may keep
/// the odd byte), which makes this an upper bound over every path rather than a guess.
pub fn depth_bound(bbox: UBox) -> Option<u32> {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    let (mut w, mut h) = (max_lon - min_lon, max_lat - min_lat);
    let mut depth = 0;
    while w >= SPLIT_FLOOR && h >= SPLIT_FLOOR {
        if depth == MAX_LEVELS {
            return None;
        }
        w = (w + 1) / 2;
        h = (h + 1) / 2;
        depth += 1;
    }
    Some(depth)
}

/// One record the streaming tree indexes: `key u64, ord u32, at u32, len u16`.
///
/// `ord` is the record's position in the caller's **input** order, which is the order [`build`]'s
/// partition preserves inside a leaf and therefore the order [`flatten`] packs a leaf in; `at` and
/// `len` say where the packed bytes are, and the tree never looks at them.
pub const TREE_REC: usize = 18;

pub fn tree_record(key: u64, ord: u32, at: u32, len: u16) -> [u8; TREE_REC] {
    let mut r = [0u8; TREE_REC];
    r[0..8].copy_from_slice(&key.to_le_bytes());
    r[8..12].copy_from_slice(&ord.to_le_bytes());
    r[12..16].copy_from_slice(&at.to_le_bytes());
    r[16..18].copy_from_slice(&len.to_le_bytes());
    r
}

pub fn rec_key(r: &[u8; TREE_REC]) -> u64 {
    u64::from_le_bytes(r[0..8].try_into().expect("8 bytes"))
}

pub fn rec_ord(r: &[u8; TREE_REC]) -> u32 {
    u32::from_le_bytes(r[8..12].try_into().expect("4 bytes"))
}

pub fn rec_at(r: &[u8; TREE_REC]) -> u32 {
    u32::from_le_bytes(r[12..16].try_into().expect("4 bytes"))
}

pub fn rec_len(r: &[u8; TREE_REC]) -> u16 {
    u16::from_le_bytes(r[16..18].try_into().expect("2 bytes"))
}

/// Tree order, then input order — a **total** order, because `ord` is unique.
pub fn by_tree_order(a: &[u8; TREE_REC], b: &[u8; TREE_REC]) -> std::cmp::Ordering {
    (rec_key(a), rec_ord(a)).cmp(&(rec_key(b), rec_ord(b)))
}

/// Where one leaf's records go: `chunk u32, at u32, first u32, count u32` — the chunk it was binned
/// into, the offset inside it, and the leaf's run of the tree-ordered stream.
pub const PLACE_REC: usize = 16;

fn place_record(chunk: u32, at: u32, first: u32, count: u32) -> [u8; PLACE_REC] {
    let mut r = [0u8; PLACE_REC];
    r[0..4].copy_from_slice(&chunk.to_le_bytes());
    r[4..8].copy_from_slice(&at.to_le_bytes());
    r[8..12].copy_from_slice(&first.to_le_bytes());
    r[12..16].copy_from_slice(&count.to_le_bytes());
    r
}

pub fn place_chunk(r: &[u8; PLACE_REC]) -> u32 {
    u32::from_le_bytes(r[0..4].try_into().expect("4 bytes"))
}

pub fn place_at(r: &[u8; PLACE_REC]) -> u32 {
    u32::from_le_bytes(r[4..8].try_into().expect("4 bytes"))
}

pub fn place_first(r: &[u8; PLACE_REC]) -> u32 {
    u32::from_le_bytes(r[8..12].try_into().expect("4 bytes"))
}

pub fn place_count(r: &[u8; PLACE_REC]) -> u32 {
    u32::from_le_bytes(r[12..16].try_into().expect("4 bytes"))
}

/// Emission order for the chunk bytes: chunk, then offset inside it.
fn by_placement(a: &[u8; PLACE_REC], b: &[u8; PLACE_REC]) -> std::cmp::Ordering {
    (place_chunk(a), place_at(a)).cmp(&(place_chunk(b), place_at(b)))
}

/// One node of the tree as the shape pass hands it to the index pass: `depth u8, prefix u64,
/// kind u8, a u32, b u32, c u32`. For a branch `a` is its rank among the branches at its depth; for
/// a leaf `(a, b, c)` is `(first, count, packed len)` — `first` a **position in the tree-ordered
/// stream**, which is what a leaf's run is named by.
///
/// Sorted by `(depth, prefix)`, which **is** the BFS order [`flatten`] numbers the index in: BFS
/// visits a whole level before the next, and within a level left to right, which is prefix order by
/// induction. A total order — two distinct nodes of one depth have distinct prefixes.
const NODE_REC: usize = 22;

const KIND_BRANCH: u8 = 0;
const KIND_EMPTY: u8 = 1;
const KIND_LEAF: u8 = 2;

fn node_record(depth: u32, prefix: u64, kind: u8, a: u32, b: u32, c: u32) -> [u8; NODE_REC] {
    let mut r = [0u8; NODE_REC];
    r[0] = depth as u8;
    r[1..9].copy_from_slice(&prefix.to_le_bytes());
    r[9] = kind;
    r[10..14].copy_from_slice(&a.to_le_bytes());
    r[14..18].copy_from_slice(&b.to_le_bytes());
    r[18..22].copy_from_slice(&c.to_le_bytes());
    r
}

fn node_depth(r: &[u8; NODE_REC]) -> u32 {
    r[0] as u32
}

fn node_prefix(r: &[u8; NODE_REC]) -> u64 {
    u64::from_le_bytes(r[1..9].try_into().expect("8 bytes"))
}

fn node_a(r: &[u8; NODE_REC]) -> u32 {
    u32::from_le_bytes(r[10..14].try_into().expect("4 bytes"))
}

fn node_b(r: &[u8; NODE_REC]) -> u32 {
    u32::from_le_bytes(r[14..18].try_into().expect("4 bytes"))
}

fn node_c(r: &[u8; NODE_REC]) -> u32 {
    u32::from_le_bytes(r[18..22].try_into().expect("4 bytes"))
}

fn by_bfs(a: &[u8; NODE_REC], b: &[u8; NODE_REC]) -> std::cmp::Ordering {
    (node_depth(a), node_prefix(a)).cmp(&(node_depth(b), node_prefix(b)))
}

/// A node being built: the box it covers, its depth and its key prefix.
#[derive(Clone, Copy)]
struct Cur {
    bbox: UBox,
    depth: u32,
    prefix: u64,
}

/// A **committed branch** — a node already known to exceed the capacity — and which of its four
/// children is currently open.
struct Frame {
    bbox: UBox,
    depth: u32,
    prefix: u64,
    child: u8,
}

/// The shape pass: one forward walk of the tree-ordered stream that closes each node as it is
/// passed, holding only the path to the current node and the points of the node currently open.
///
/// The rule it reproduces is [`build`]'s, in the one direction a stream allows. [`build`] asks "is
/// this node's *total* over the capacity?" before descending; this accumulates and splits the moment
/// the running total passes it. That is the same verdict: a running total that passes the capacity
/// proves the total does, and a node whose accumulation never passes it has a total that does not.
/// The floor is checked on the node's own box, exactly as [`build`] checks it before splitting.
struct Shape<'s> {
    capacity: usize,
    cur: Cur,
    stack: Vec<Frame>,
    /// The points accumulated into `cur` — at most a capacity's worth, except inside a floor-bounded
    /// box, which is the one place [`build`] cannot split either. `(key, pos, len)`, where `pos` is
    /// the point's **position in the tree-ordered stream**, not its `ord`: a leaf is named by its run
    /// of that stream, and the stream is what [`read_run`] seeks into.
    pending: Vec<(u64, u32, u16)>,
    packed: usize,
    /// Nodes closed at each depth, and branches committed at each depth — the two counters the BFS
    /// numbering is arithmetic over.
    at_depth: Vec<u32>,
    branches: Vec<u32>,
    out: ExternalSort<'s, NODE_REC>,
}

impl<'s> Shape<'s> {
    fn new(bbox: UBox, capacity: usize, out: ExternalSort<'s, NODE_REC>) -> Shape<'s> {
        Shape {
            capacity,
            cur: Cur { bbox, depth: 0, prefix: 0 },
            stack: Vec::new(),
            pending: Vec::new(),
            packed: 0,
            at_depth: vec![0; MAX_LEVELS as usize + 2],
            branches: vec![0; MAX_LEVELS as usize + 2],
            out,
        }
    }

    fn contains(&self, key: u64) -> bool {
        key & prefix_mask(self.cur.depth) == self.cur.prefix
    }

    /// Close the open node as a leaf — empty or not — and count it at its depth.
    fn close_leaf(&mut self) -> Result<()> {
        let (kind, first, count, len) = if self.pending.is_empty() {
            (KIND_EMPTY, 0, 0, 0)
        } else {
            (KIND_LEAF, self.pending[0].1, self.pending.len() as u32, self.packed as u32)
        };
        self.out.push(node_record(self.cur.depth, self.cur.prefix, kind, first, count, len))?;
        self.at_depth[self.cur.depth as usize] += 1;
        self.pending.clear();
        self.packed = 0;
        Ok(())
    }

    /// The open node is over capacity and can still split: record it as a branch and open its first
    /// child. Its `pending` points are the caller's to redistribute.
    fn commit_branch(&mut self) -> Result<()> {
        let depth = self.cur.depth;
        let rank = self.branches[depth as usize];
        self.branches[depth as usize] += 1;
        self.at_depth[depth as usize] += 1;
        self.out.push(node_record(depth, self.cur.prefix, KIND_BRANCH, rank, 0, 0))?;
        self.stack.push(Frame { bbox: self.cur.bbox, depth, prefix: self.cur.prefix, child: 0 });
        self.open_child();
        Ok(())
    }

    /// Point `cur` at the open child of the deepest frame.
    fn open_child(&mut self) {
        let f = self.stack.last().expect("open_child is only called under a frame");
        let child = f.child as usize;
        self.cur = Cur {
            bbox: children(f.bbox)[child],
            depth: f.depth + 1,
            prefix: f.prefix | ((child as u64) << digit_shift(f.depth)),
        };
    }

    /// Move past the node that was just closed: the parent's next child, popping frames whose four
    /// children are all closed. `false` once the root itself is closed.
    fn advance(&mut self) -> bool {
        loop {
            match self.stack.last_mut() {
                None => return false,
                Some(top) => {
                    top.child += 1;
                    if top.child < 4 {
                        self.open_child();
                        return true;
                    }
                    self.stack.pop();
                }
            }
        }
    }

    /// Take the point at position `pos` of the tree-ordered stream.
    fn insert(&mut self, key: u64, pos: u32, len: u16) -> Result<()> {
        let mut batch: Vec<(u64, u32, u16)> = vec![(key, pos, len)];
        loop {
            let mut split_at = None;
            for (k, (key, pos, len)) in batch.iter().copied().enumerate() {
                while !self.contains(key) {
                    self.close_leaf()?;
                    if !self.advance() {
                        return Err(Error::Scratch(
                            "the node stream left the assembly bbox — it is not in tree order".into(),
                        ));
                    }
                }
                self.pending.push((key, pos, len));
                self.packed += len as usize;
                if self.packed > self.capacity && !at_floor(self.cur.bbox) && self.cur.depth < MAX_LEVELS {
                    self.commit_branch()?;
                    split_at = Some(k + 1);
                    break;
                }
            }
            let Some(k) = split_at else { return Ok(()) };
            // The points that had accumulated in the node just split move down into it, followed by
            // the ones this batch had not reached yet — the stream's own order, preserved.
            let mut next = std::mem::take(&mut self.pending);
            next.extend_from_slice(&batch[k..]);
            self.packed = 0;
            batch = next;
        }
    }

    /// Close the open node and every node still open above it.
    fn finish(mut self) -> Result<(ExternalSort<'s, NODE_REC>, Vec<u32>)> {
        loop {
            self.close_leaf()?;
            if !self.advance() {
                break;
            }
        }
        debug_assert!(self.stack.is_empty());
        Ok((self.out, self.at_depth))
    }
}

/// What [`flatten_streaming`] produces: the index and the placement plan, both on the scratch seam,
/// plus the counters the §8.1 directory needs.
#[derive(Debug)]
pub struct Flattened {
    /// The §8.2 index, already in its wire form — `Node Count` little-endian `uint32`s.
    pub index: ScratchId,
    pub node_count: u32,
    pub chunk_count: u32,
    /// One [`PLACE_REC`] per non-empty leaf, in chunk-emission order.
    pub places: ScratchId,
    /// The tree-ordered point stream this was built from, handed back because the placement plan
    /// names each leaf as a *run* of it — [`read_run`] is how the caller reads one back.
    pub points: ScratchId,
    pub leaf_count: u64,
    /// Records the §8.3 chunk-capacity guard refused — [`flatten`]'s own counter.
    pub dropped: usize,
}

/// [`build`] + [`flatten`] with `bin_pack`, over a stream: the tree's shape from a tree-ordered
/// record stream, the §8.2 bin packing over its leaves, and a plan the caller emits chunk bytes from.
///
/// `points` is a [`TREE_REC`] stream sorted by [`by_tree_order`]; nothing else about it is assumed,
/// and it is read forward once here plus once per leaf (by range) later. The two walks are:
///
/// 1. **Shape** — one forward pass ([`Shape`]) that closes every node as the stream passes it,
///    holding the path and the open node's points and nothing else, and files each closed node under
///    `(depth, prefix)`.
/// 2. **Index and bins** — that file read back in BFS order, which is the order [`flatten`] numbers
///    the index in *and* the order it opens and fills chunks in. A branch's `First Child` is
///    arithmetic (`start[depth + 1] + 4 × rank`), because children are laid out in groups of four in
///    their parents' order; a leaf is first-fit into the open chunks exactly as before.
pub fn flatten_streaming(
    scratch: &dyn ScratchStore,
    budget: usize,
    points: ScratchId,
    bbox: UBox,
    capacity: usize,
    chunk_size: usize,
) -> Result<Flattened> {
    if depth_bound(bbox).is_none() {
        return Err(Error::Capacity(format!(
            "a quadtree over {bbox:?} would recurse past {MAX_LEVELS} levels before reaching the {SPLIT_FLOOR} µdeg \
             floor"
        )));
    }
    let share = (budget / 8).max(TREE_REC);

    // 1. The shape.
    let mut shape = Shape::new(bbox, capacity, ExternalSort::<NODE_REC>::new(scratch, budget / 2, by_bfs));
    // The stream's **position** is what a leaf's run is named by, so that — not `ord` — is what goes
    // into the shape pass. The two differ the moment the tree order is not the input order, which is
    // the normal case.
    for (pos, rec) in crate::extsort::SpillReader::<TREE_REC>::open(scratch, points, share)?.enumerate() {
        let rec = rec?;
        let pos = u32::try_from(pos).map_err(|_| {
            Error::Capacity("a quadtree over more than 4 G records: the leaf runs are named by a uint32".into())
        })?;
        shape.insert(rec_key(&rec), pos, rec_len(&rec))?;
    }
    let (nodes, at_depth) = shape.finish()?;

    // Where each depth's run of the BFS numbering starts.
    let mut start = vec![0u32; at_depth.len() + 1];
    for (d, &n) in at_depth.iter().enumerate() {
        start[d + 1] = start[d] + n;
    }
    let node_count = *start.last().expect("at least the root");

    // 2. The index and the bin packing, in BFS order.
    let mut index = SpillWriter::<4>::create(scratch, share)?;
    let mut places = ExternalSort::<PLACE_REC>::new(scratch, budget / 2, by_placement);
    let mut free = FirstFit::new();
    let mut bins = 0usize;
    let mut dropped = 0usize;
    let mut leaves = 0u64;
    let mut written = 0u32;
    for rec in nodes.finish()? {
        let rec = rec?;
        let depth = node_depth(&rec);
        let value = match rec[9] {
            KIND_BRANCH => BRANCH_BIT | (start[depth as usize + 1] + 4 * node_a(&rec)),
            KIND_EMPTY => EMPTY_LEAF,
            _ => {
                let (first, count, leaf_len) = (node_a(&rec), node_b(&rec), node_c(&rec) as usize);
                let bin = match free.first_fit(leaf_len) {
                    Some(c) => c,
                    None => {
                        bins += 1;
                        free.push(chunk_size)
                    }
                };
                let at = chunk_size - free.free(bin);
                // The common leaf fits whole — the tree split it to at most a chunk, so the only
                // leaf that can overflow is one the floor stopped it from splitting, and only that
                // one has to be walked record by record to find out how much of it lands.
                let used = if leaf_len <= chunk_size - at {
                    at + leaf_len
                } else {
                    let mut used = at;
                    for r in read_run(scratch, points, first, count)? {
                        if used + rec_len(&r) as usize > chunk_size {
                            dropped += 1;
                            continue;
                        }
                        used += rec_len(&r) as usize;
                    }
                    used
                };
                free.set(bin, chunk_size - used);
                places.push(place_record(bin as u32, at as u32, first, count))?;
                leaves += 1;
                bin as u32 & !BRANCH_BIT
            }
        };
        index.push(value.to_le_bytes())?;
        written += 1;
    }
    debug_assert_eq!(written, node_count, "the BFS walk must visit every node the shape pass closed");
    let (index, _) = index.seal()?;

    let mut out = SpillWriter::<PLACE_REC>::create(scratch, share)?;
    for rec in places.finish()? {
        out.push(rec?)?;
    }
    Ok(Flattened {
        index,
        node_count,
        chunk_count: bins as u32,
        places: out.seal()?.0,
        points,
        leaf_count: leaves,
        dropped,
    })
}

/// One leaf's records, in the caller's **input** order — which is the order [`flatten`] packs a leaf
/// in, and the order the tree-ordered stream does *not* hold them in when a leaf spans several keys.
///
/// A leaf is a chunk's worth of records, so this is a bounded read; the one exception is a leaf the
/// floor could not split, which is also the only leaf that can overflow its chunk.
pub fn read_run(scratch: &dyn ScratchStore, points: ScratchId, first: u32, count: u32) -> Result<Vec<[u8; TREE_REC]>> {
    let mut buf = vec![0u8; count as usize * TREE_REC];
    scratch.read_at(points, first as u64 * TREE_REC as u64, &mut buf)?;
    let mut out: Vec<[u8; TREE_REC]> = buf.chunks_exact(TREE_REC).map(|c| c.try_into().expect("TREE_REC")).collect();
    out.sort_unstable_by_key(rec_ord);
    Ok(out)
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

    // ---------------------------------------------------------------------------------------------
    // The streaming tree (#1116 D4)
    // ---------------------------------------------------------------------------------------------

    /// A point that knows its own input position, so the packed bytes say **which** record they are
    /// and a chunk that holds the right records in the wrong order fails the comparison.
    struct Q {
        lat: i32,
        lon: i32,
        len: usize,
        ord: u32,
    }

    impl Point for Q {
        fn lat(&self) -> i32 {
            self.lat
        }
        fn lon(&self) -> i32 {
            self.lon
        }
        fn record_len(&self) -> usize {
            self.len
        }
    }

    fn pack_q(q: &Q, out: &mut Vec<u8>) {
        out.extend_from_slice(&q.ord.to_le_bytes());
        out.resize(out.len() + q.len - 4, (q.ord & 0xFF) as u8);
    }

    /// xorshift64* — a deterministic stream, so a failure is reproducible from the seed alone.
    fn rng(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    fn read_all(scratch: &dyn ScratchStore, id: ScratchId) -> Vec<u8> {
        let len = scratch.len(id).expect("a scratch length") as usize;
        let mut buf = vec![0u8; len];
        scratch.read_at(id, 0, &mut buf).expect("a scratch read");
        buf
    }

    /// **The equivalence.** [`flatten_streaming`] is worth nothing unless it is the *same* tree, so
    /// this runs both formulations over the same pseudo-random point sets and compares the §8.2
    /// index and the chunk bytes byte for byte — not the shape, the output.
    ///
    /// The sets are chosen to hit the three things that could diverge: ordinary points (the tree's
    /// shape and the BFS numbering), leaves small enough that first-fit back-fills an earlier chunk
    /// (the bin packing), and a co-located cluster inside the [`SPLIT_FLOOR`] (the one leaf that can
    /// overflow its chunk, which is the only place `dropped` is non-zero and the only place the
    /// streaming path has to walk a leaf record by record).
    #[test]
    fn the_streaming_tree_is_the_tree_build_and_flatten_make() {
        const CHUNK: usize = 512;
        let bbox: UBox = (0, 0, 1 << 20, 1 << 20);
        let mut seed = 0x2545_F491_4F6C_DD1D_u64;

        for trial in 0..24 {
            let n = 1 + (rng(&mut seed) % 400) as usize;
            let mut points: Vec<Q> = (0..n)
                .map(|k| Q {
                    lat: (rng(&mut seed) % (1 << 20)) as i32,
                    lon: (rng(&mut seed) % (1 << 20)) as i32,
                    len: 13 + (rng(&mut seed) % 180) as usize,
                    ord: k as u32,
                })
                .collect();
            // A cluster inside the recursion floor, which no tree can split apart.
            if trial % 3 == 0 {
                let (lat, lon) = (500_000, 500_000);
                for k in 0..20 {
                    points.push(Q { lat, lon, len: 60, ord: (n + k) as u32 });
                }
            }

            // The old formulation.
            let tree = build(points.iter().collect::<Vec<&Q>>(), bbox, CHUNK);
            let (want_index, want_nodes, want_chunks, want_bins, want_dropped) =
                flatten(&tree, CHUNK, true, &|p: &&Q, out: &mut Vec<u8>| pack_q(p, out));

            // The new one, over the same points as a tree-ordered stream.
            let scratch = crate::scratch::MemoryScratch::new();
            let mut recs: Vec<[u8; TREE_REC]> =
                points.iter().map(|q| tree_record(tree_key(q.lat, q.lon, bbox), q.ord, 0, q.len as u16)).collect();
            recs.sort_by(by_tree_order);
            let mut sorted = SpillWriter::<TREE_REC>::create(&scratch, 1 << 16).expect("a scratch write");
            for r in recs {
                sorted.push(r).expect("a scratch write");
            }
            let (sorted, _) = sorted.seal().expect("a scratch seal");
            let flat = flatten_streaming(&scratch, 1 << 16, sorted, bbox, CHUNK, CHUNK).expect("the streaming tree");

            assert_eq!(read_all(&scratch, flat.index), want_index, "trial {trial}: the §8.2 index differs");
            assert_eq!(flat.node_count, want_nodes, "trial {trial}: node count");
            assert_eq!(flat.chunk_count, want_bins, "trial {trial}: chunk count");
            assert_eq!(flat.dropped, want_dropped, "trial {trial}: dropped records");

            // The chunk bytes, emitted from the plan exactly as `nav::emit_chunks` does.
            let bodies: Vec<Vec<u8>> = points
                .iter()
                .map(|q| {
                    let mut b = Vec::new();
                    pack_q(q, &mut b);
                    b
                })
                .collect();
            let mut got: Vec<u8> = Vec::new();
            let mut chunk: Vec<u8> = Vec::new();
            let mut current = 0u32;
            let plan = read_all(&scratch, flat.places);
            for p in plan.chunks_exact(PLACE_REC) {
                let p: &[u8; PLACE_REC] = p.try_into().expect("PLACE_REC");
                while current < place_chunk(p) {
                    chunk.resize(CHUNK, obc_formats::obcm::CHUNK_END);
                    got.extend_from_slice(&chunk);
                    chunk.clear();
                    current += 1;
                }
                assert_eq!(chunk.len(), place_at(p) as usize, "trial {trial}: the plan and the write disagree");
                for r in read_run(&scratch, flat.points, place_first(p), place_count(p)).expect("a leaf run") {
                    let body = &bodies[rec_ord(&r) as usize];
                    if chunk.len() + body.len() > CHUNK {
                        continue;
                    }
                    chunk.extend_from_slice(body);
                }
            }
            if flat.chunk_count > 0 {
                chunk.resize(CHUNK, obc_formats::obcm::CHUNK_END);
                got.extend_from_slice(&chunk);
                assert_eq!(current + 1, flat.chunk_count, "trial {trial}: every chunk is opened by a leaf");
            }
            assert_eq!(got, want_chunks, "trial {trial}: the streamed chunk bytes are not the packed ones");
        }
    }

    /// The tree-order key is only a total order over a box the descent can actually encode, so a box
    /// that would recurse past [`MAX_LEVELS`] is a refusal rather than two points swapping places.
    #[test]
    fn a_box_too_deep_for_the_key_is_refused() {
        assert_eq!(depth_bound((0, 0, 1 << 20, 1 << 20)), Some(17), "10 µdeg floor over a 2^20 box");
        assert!(depth_bound((0, 0, 1 << 29, 1 << 29)).is_some(), "the largest box `grid.rs` can produce");
        let scratch = crate::scratch::MemoryScratch::new();
        let points = scratch.create().expect("a scratch file");
        let huge: UBox = (0, 0, 1 << 50, 1 << 50);
        assert!(depth_bound(huge).is_none(), "the fixture has to be past the bound to test the refusal");
        let err = flatten_streaming(&scratch, 1 << 16, points, huge, 512, 512).expect_err("past the key's depth");
        assert!(format!("{err}").contains("levels"), "got: {err}");
    }
}
