//! Merging the navigation graph
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §4.6) — the assembler's one O(rewrite) component.
//!
//! Nothing in `OBCM_Spec.md` §8 survives concatenation: node ids are file-local and dense, and an
//! `Edge Id` is a **pool byte offset**. So the graph is read back out of every `network` cell,
//! unified at the seams, pruned, renumbered, and re-emitted. What it is *not* is re-derived: an edge
//! record's bytes are copied verbatim (§4.6.6 permits exactly this — the record is self-contained,
//! absolute anchor plus deltas), so no polyline is ever decoded and re-encoded, and the costs and
//! way kinds the packer measured travel through untouched.
//!
//! And because placement is the only thing that changes, a record's bytes are never *resident*
//! either. The merge holds a ten-byte [`EdgeRef`] — which cell, which pool offset, how long — the
//! new pool is laid out arithmetically over those lengths, and [`serialize`] reads each record out
//! of the cell that wrote it straight into the sink. The graph's working set is therefore the merge's
//! own bookkeeping, not a copy of the section it is about to write.
//!
//! That bookkeeping is in turn flat. Nothing here keeps a map or a heap allocation *per edge* or
//! *per node* over the whole graph: the §4.6.3 duplicate check is a sorted pass at the end of
//! collection rather than a hash set carried through it, and the rebuilt adjacency is one CSR buffer
//! — offsets into a single `Vec` — rather than a `Vec` per junction. Both are noted where they
//! happen, because both trade an obvious formulation for one whose *order* has to be argued.
//!
//! # The node side does not live in memory (#1116 D2)
//!
//! Flat is not the same as small. At DACH scale the per-node arrays — coordinates, digests, the
//! renumbering permutation — are gigabytes, and none of them is information that has to be resident:
//! **cross-cell coupling exists only at seam nodes**, coordinates on a boundary line of the
//! `network` band's grid (§4.6.2), measured at 19 337 of 3.0 M on a state-sized bake. Everything
//! else is settled inside one cell.
//!
//! So the collect pass keeps only the *seam table* — coordinate → seam slot, its node id, and its
//! accumulated digest — and spills every node's `(lat, lon, digest)` through the
//! [scratch seam](crate::scratch), in id order, so the record's **position in the file is the node's
//! id**. The §4.6.5 renumbering then reads that stream back once and sorts it
//! [externally](crate::extsort), bounded by [`crate::Options::merge_budget_bytes`], instead of
//! permuting an in-memory array.
//!
//! Two things had to be argued rather than assumed for that to be the same bytes:
//!
//! * **The digest may be accumulated during collection, before pruning.** §4.6.5's tie-break sums
//!   only the *kept* edges' contributions. But an edge is kept exactly when its component is
//!   (`prune` below), and an edge's two endpoints are in the same component by construction — so for
//!   any node that survives the prune, *every* incident edge survives with it, and the two sums are
//!   the same sum. Only nodes that are about to be dropped can differ, and they are dropped.
//! * **…but not before the §4.6.3 dedup**, which really does remove contributions. A duplicate's
//!   contribution is therefore *subtracted* when it dies, as a delta against the two node ids it
//!   touched. Accumulation is `wrapping_add`, so the subtraction is exact, and there are as many
//!   deltas as there are duplicate edges — none at all on either published region.
//!
//! The seam table's own digests are deltas of the same kind, because a seam node's record is written
//! by the first cell that saw it and later cells go on adding to it.
//!
//! # …and neither does the edge side (#1116 D3)
//!
//! The same is now true of the edges. A collected edge is a 35-byte record on the scratch seam —
//! endpoints, `Cost M`, `Way Kind`, the content hash, the ten-byte [`EdgeRef`] and both ascents —
//! appended in collection order, so that (exactly as with the node stream) a record's **position is
//! its collection index** and nothing has to store one. The only edges in memory at any moment are
//! the ones the cell currently being read collected, because the second sighting of an edge writes
//! the other direction's ascent back into the entry the first made and that write-back never crosses
//! a cell.
//!
//! Every step that used to walk `Vec<MergedEdge>` is a sorted or streamed pass over that file:
//!
//! * **§4.6.3 dedup** sorts a 25-byte key — `(hash, endpoints, cost, kind, collection index)` — and
//!   keeps the first member of each run of equal keys. The old formulation sorted by hash alone and
//!   searched each equal-hash run for an earlier copy; sorting by the *whole* key makes "the first
//!   copy" the first record of a run, so the pass is `O(1)` memory instead of `O(run²)` time, and
//!   the survivor is the same one for the same reason it always was: the collection index is the
//!   last component of the key, so the lowest one leads its group. What comes back is the list of
//!   dead collection indices and the digest subtractions they owe.
//! * **§4.6.4 island pruning** decomposes per cell — see [`crate::prune`], which owns the argument.
//! * **§4.6.5's renumbering** hands out dense ids as before, but instead of a whole-map
//!   collection → dense array it emits `(id, dense, lat, lon)` pairs, sorted by id, and the edges
//!   are resolved against them by **merge join**: sort the surviving edges by `a`, walk the two
//!   streams together, then again by `b`. The neighbour coordinates ride along, because that is what
//!   the §8.3 adjacency deltas are measured from and re-reading them would need the node array back.
//! * **§4.6.6's pool layout** sorts by the emission key over the *dense* ids and mints `Edge Id`s
//!   with a running [`place`] cursor. That key is a total order on what survives the dedup — two
//!   edges that tied on all five components would have been deduplicated — so the layout does not
//!   depend on the sort's stability, and the same walk fills the adjacency, because a record's
//!   `Edge Id` is known at exactly the moment it is placed.
//!
//! The order of the passes is normative and each one depends on the last:
//!
//! 1. **Read the serialized node set** — not a graph builder's, the *serializer's*. The §8.4 splits
//!    mint synthetic degree-2 junctions after `nav.rs` finishes (measured +4 489 nodes on a country
//!    bake) and they are in the bytes.
//!
//!    *Deliberate deviation from §4.6.1's letter.* The spec says "walk each `network` cell's §8 node
//!    quadtree through a real reader … the collection MUST be idempotent". This reads the cell's
//!    **chunk run** instead, straight through, and the requirement it exists to satisfy is met more
//!    strongly rather than less: §8.2's bin packing is what makes the *walk* hand a record back more
//!    than once, and the chunk run visits each stored record exactly once, so idempotence is not
//!    needed — a repeat is impossible, and a duplicate `Node Id` inside one cell is a
//!    [`crate::Error::Format`] rather than a silently absorbed re-read. It also drops the quadtree
//!    from the merge's cost entirely. The one thing it must not do is miss a record the walk would
//!    reach, which is why an unreferenced chunk would still be read: the run is `Chunk Count`
//!    chunks, not the leaves' union.
//! 2. **Unify seam nodes, and only seam nodes** — exact coordinate equality, and only where the
//!    coordinate lies on a boundary line of the `network` band's grid. There is no tolerance knob:
//!    at a cell seam genuinely distinct junctions sit 3.9 m apart (measured), while a whole-map
//!    coordinate key would fuse the interior collisions that exist inside a single file — stacked
//!    bridge/tunnel junctions, 9 in one regional bake and 28 in a country one.
//! 3. **Deduplicate adjacency**, 4. **prune islands** over the merged graph (the pass the bake
//!    deferred), 5. **renumber**, 6. **lay out the edge pool** — its offsets, not its bytes — 7.
//!    re-check the wire limits and rebuild the node quadtree.

use std::cmp::Ordering;
use std::collections::HashMap;

use obc_formats::obcm::{
    CHUNK_END, NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN, NAV_MAX_DEGREE, NAV_NEIGHBOR_ASCENT_OFF,
    NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN,
};

use crate::extsort::{ExternalSort, SpillReader, SpillWriter};
use crate::grid::{on_grid_boundary, UBox};
use crate::input::Cell;
use crate::qtree::{self, Point};
use crate::scratch::{ScratchId, ScratchStore};
use crate::{prune, Error, Result};

/// Largest neighbour delta the `int16` fields hold (§8.3). The packer's own split bound is 32 000;
/// unification never moves a coordinate, so an input that held it still holds it — but §4.8 says
/// re-check rather than assume.
const MAX_NEIGHBOR_DELTA: i64 = i16::MAX as i64;

/// The record the collect pass spills, one per collected node: `lat i32, lon i32, digest u64`.
///
/// There is no node id in it because there does not need to be one — records are appended in id
/// order, so a record's **offset is `id × NODE_REC`**. That is also what makes the read-back a plain
/// forward scan whose index is the id.
const NODE_REC: usize = 16;

/// …and the record §4.6.5 sorts: the same three fields plus the id they were spilled under.
const SORT_REC: usize = NODE_REC + 4;

/// A collected node, as the cell that read it names it: the id it was minted under, and which seam
/// slot it is — or [`NO_SEAM`].
///
/// This is the identity D3's edge stream joins against. It is dense over the whole collection and
/// assigned in read order, so it is the pair *(cell, index within that cell's minted nodes)*
/// flattened: cell `c`'s ids are the contiguous range that starts where cell `c − 1` stopped
/// minting, and a reference to a node an *earlier* cell minted is a seam unification by
/// construction (§4.6.2 is the only thing that unifies).
#[derive(Clone, Copy)]
struct NodeRef {
    /// The node's collection id — its position in the spilled node stream.
    id: u32,
    /// Its slot in the seam table, or [`NO_SEAM`] for a node no other cell can see.
    seam: u32,
}

/// [`NodeRef::seam`] for a node that is not on a grid boundary line.
const NO_SEAM: u32 = u32::MAX;

/// Where an edge's §8.4 record is, in the cell that wrote it — ten bytes instead of the record.
///
/// The record itself is never held: §4.6.6 copies it verbatim, so the merge only has to remember
/// *which* bytes, and [`serialize`] reads them back out of the cell at emission.
#[derive(Clone, Copy)]
struct EdgeRef {
    /// Index into the `network` cells [`merge`] read — the same slice [`serialize`] streams from.
    cell: u32,
    /// The record's offset inside that cell's edge pool: §8.4's `Edge Id`, verbatim.
    off: u32,
    /// The record's length. §8.4 forbids a record straddling a 512-byte chunk, so it is at most one
    /// chunk and a `u16` holds it by construction — which is also why one chunk-sized buffer is
    /// enough to read any record into.
    len: u16,
}

/// An edge on its way into the merged pool: unified endpoints plus the address of the source record.
struct MergedEdge {
    a: u32,
    b: u32,
    cost_m: u32,
    kind: u8,
    /// The §8.4 record its cell wrote — `length_m`, `pt_count`, `way_kind`, anchor and deltas —
    /// named rather than copied. Only its *placement* is new.
    rec: EdgeRef,
    /// FNV-1a over the record's bytes, computed **once** while the cell's pool is in hand. It is the
    /// content half of both the §4.6.3 duplicate key and the §4.6.6 emission order, and re-hashing a
    /// 511-byte record inside an `O(n log n)` comparator was measurable on a country-scale graph —
    /// the record is not resident to re-hash from anyway.
    hash: u64,
    /// The v12 §8.3 climb of riding `a → b`, and of riding `b → a`. Not part of the record and not
    /// derivable from it: the assembler carries both from the source adjacency entries, one per
    /// direction, and re-emits them on the sides they came from.
    ascent_ab: u16,
    ascent_ba: u16,
}

/// [`MergedEdge`] on the scratch seam: `a u32, b u32, cost u32, hash u64, cell u32, off u32,
/// len u16, ascent_ab u16, ascent_ba u16, kind u8`.
///
/// No collection index, for the same reason the node record carries no id — the stream is appended
/// in collection order, so a record's **position is its index** and the passes that need one count.
pub(crate) const EDGE_REC: usize = 35;

impl MergedEdge {
    fn encode(&self) -> [u8; EDGE_REC] {
        let mut r = [0u8; EDGE_REC];
        r[0..4].copy_from_slice(&self.a.to_le_bytes());
        r[4..8].copy_from_slice(&self.b.to_le_bytes());
        r[8..12].copy_from_slice(&self.cost_m.to_le_bytes());
        r[12..20].copy_from_slice(&self.hash.to_le_bytes());
        r[20..24].copy_from_slice(&self.rec.cell.to_le_bytes());
        r[24..28].copy_from_slice(&self.rec.off.to_le_bytes());
        r[28..30].copy_from_slice(&self.rec.len.to_le_bytes());
        r[30..32].copy_from_slice(&self.ascent_ab.to_le_bytes());
        r[32..34].copy_from_slice(&self.ascent_ba.to_le_bytes());
        r[34] = self.kind;
        r
    }
}

/// The fields the passes read straight out of an [`EDGE_REC`] record, without decoding the rest.
pub(crate) fn edge_a(r: &[u8; EDGE_REC]) -> u32 {
    u32::from_le_bytes(r[0..4].try_into().expect("4 bytes"))
}

pub(crate) fn edge_b(r: &[u8; EDGE_REC]) -> u32 {
    u32::from_le_bytes(r[4..8].try_into().expect("4 bytes"))
}

pub(crate) fn edge_cell(r: &[u8; EDGE_REC]) -> u32 {
    u32::from_le_bytes(r[20..24].try_into().expect("4 bytes"))
}

/// The §4.6.3 key, as a record the external sort can order: `hash u64, lo u32, hi u32, cost u32,
/// index u32, kind u8`. Twenty-five bytes rather than the edge, because the pass only ever compares
/// keys and reports indices.
const DUP_REC: usize = 25;

/// `(hash, lo, hi, cost, kind)` — the whole §4.6.3 duplicate key, without the collection index that
/// separates two copies of it.
fn dup_key(r: &[u8; DUP_REC]) -> (u64, u32, u32, u32, u8) {
    (
        u64::from_le_bytes(r[0..8].try_into().expect("8 bytes")),
        u32::from_le_bytes(r[8..12].try_into().expect("4 bytes")),
        u32::from_le_bytes(r[12..16].try_into().expect("4 bytes")),
        u32::from_le_bytes(r[16..20].try_into().expect("4 bytes")),
        r[24],
    )
}

/// The key one collected edge contributes, under the collection index it was appended at.
fn dup_record(e: &MergedEdge, index: u32) -> [u8; DUP_REC] {
    let mut r = [0u8; DUP_REC];
    r[0..8].copy_from_slice(&e.hash.to_le_bytes());
    r[8..12].copy_from_slice(&e.a.min(e.b).to_le_bytes());
    r[12..16].copy_from_slice(&e.a.max(e.b).to_le_bytes());
    r[16..20].copy_from_slice(&e.cost_m.to_le_bytes());
    r[20..24].copy_from_slice(&index.to_le_bytes());
    r[24] = e.kind;
    r
}

fn dup_index(r: &[u8; DUP_REC]) -> u32 {
    u32::from_le_bytes(r[20..24].try_into().expect("4 bytes"))
}

/// The dedup order: the full key, then the collection index — a **total** order, and one in which
/// the first record of every equal-key run is the first-collected copy.
fn by_dup_key(a: &[u8; DUP_REC], b: &[u8; DUP_REC]) -> Ordering {
    (dup_key(a), dup_index(a)).cmp(&(dup_key(b), dup_index(b)))
}

/// The renumbering's output as a joinable stream: `id u32, dense u32, lat i32, lon i32`, sorted by
/// the collection id the edges name their endpoints with.
const JOIN_REC: usize = 16;

fn join_id(r: &[u8; JOIN_REC]) -> u32 {
    u32::from_le_bytes(r[0..4].try_into().expect("4 bytes"))
}

fn by_join_id(a: &[u8; JOIN_REC], b: &[u8; JOIN_REC]) -> Ordering {
    join_id(a).cmp(&join_id(b))
}

/// A surviving edge once the joins have run: both endpoints as `dense u32, lat i32, lon i32`, then
/// the fields the emission needs — `cost u32, hash u64, cell u32, off u32, len u16, ascent_ab u16,
/// ascent_ba u16, kind u8`.
///
/// Between the two joins the `b` slot's first field still holds `b`'s **collection** id and its
/// coordinate is unset; that is what the second join fills in.
const DENSE_REC: usize = 51;

/// Where each endpoint's `(dense, lat, lon)` triple starts.
const A_AT: usize = 0;
const B_AT: usize = 12;

fn dense_id(r: &[u8; DENSE_REC], at: usize) -> u32 {
    u32::from_le_bytes(r[at..at + 4].try_into().expect("4 bytes"))
}

fn dense_coord(r: &[u8; DENSE_REC], at: usize) -> (i32, i32) {
    (
        i32::from_le_bytes(r[at + 4..at + 8].try_into().expect("4 bytes")),
        i32::from_le_bytes(r[at + 8..at + 12].try_into().expect("4 bytes")),
    )
}

/// Overwrite one endpoint slot with what the join found for it.
fn put_endpoint(r: &mut [u8; DENSE_REC], at: usize, j: &[u8; JOIN_REC]) {
    r[at..at + 12].copy_from_slice(&j[4..16]);
}

fn dense_cost(r: &[u8; DENSE_REC]) -> u32 {
    u32::from_le_bytes(r[24..28].try_into().expect("4 bytes"))
}

fn dense_hash(r: &[u8; DENSE_REC]) -> u64 {
    u64::from_le_bytes(r[28..36].try_into().expect("8 bytes"))
}

fn dense_ref(r: &[u8; DENSE_REC]) -> EdgeRef {
    EdgeRef {
        cell: u32::from_le_bytes(r[36..40].try_into().expect("4 bytes")),
        off: u32::from_le_bytes(r[40..44].try_into().expect("4 bytes")),
        len: u16::from_le_bytes(r[44..46].try_into().expect("2 bytes")),
    }
}

fn dense_ascents(r: &[u8; DENSE_REC]) -> (u16, u16) {
    (
        u16::from_le_bytes(r[46..48].try_into().expect("2 bytes")),
        u16::from_le_bytes(r[48..50].try_into().expect("2 bytes")),
    )
}

fn dense_kind(r: &[u8; DENSE_REC]) -> u8 {
    r[50]
}

/// The `a`-side sort for the first join. Ties are the edges of one junction, and their order among
/// themselves is invisible: the emission sort below is a total order over the same records, so it
/// lands on the same permutation whatever order it is handed.
fn by_edge_a(a: &[u8; EDGE_REC], b: &[u8; EDGE_REC]) -> Ordering {
    edge_a(a).cmp(&edge_a(b))
}

/// …and the `b`-side sort for the second, over the slot that still holds a collection id.
fn by_endpoint_b(a: &[u8; DENSE_REC], b: &[u8; DENSE_REC]) -> Ordering {
    dense_id(a, B_AT).cmp(&dense_id(b, B_AT))
}

/// §4.6.6's emission order: `(min dense id, max dense id, Cost M, Way Kind, content hash)`.
///
/// A **total** order on what reaches it — two edges equal on all five would have collapsed in
/// §4.6.3, because the dense ids are a bijection on the kept nodes and so agree with the collection
/// ids about which pairs are the same pair. So the pool's layout, and with it every `Edge Id` in the
/// file, is fixed by the records alone.
fn by_emission(x: &[u8; DENSE_REC], y: &[u8; DENSE_REC]) -> Ordering {
    let key = |r: &[u8; DENSE_REC]| {
        let (a, b) = (dense_id(r, A_AT), dense_id(r, B_AT));
        (a.min(b), a.max(b), dense_cost(r), dense_kind(r), dense_hash(r))
    };
    key(x).cmp(&key(y))
}

/// One junction ready to serialize: its coordinate, its dense id, and a *view* of the shared
/// adjacency buffer — `degree` entries starting at `at`. The neighbours are not owned, because a
/// `Vec` per junction was a heap allocation and a 24-byte header for a list that averages under
/// three entries (2.99 M of them on a state-sized bake).
struct NavPoint {
    lat: i32,
    lon: i32,
    id: u32,
    /// Index of this junction's first neighbour in the CSR buffer [`merge`] fills.
    at: u32,
    /// How many entries it kept — at most `OBCM_Spec.md` §8.3's cap of 24, so a byte.
    degree: u8,
}

#[derive(Clone, Copy, Default)]
struct WireNeighbor {
    id: u32,
    lat: i32,
    lon: i32,
    edge_id: u32,
    cost_m: u32,
    way_kind: u8,
    /// The v12 §8.3 climb of riding **toward** this neighbour. Carried through the merge per
    /// direction, because it is the one adjacency field the two sides of an edge do not share.
    ascent_m: u16,
}

impl Point for NavPoint {
    fn lat(&self) -> i32 {
        self.lat
    }
    fn lon(&self) -> i32 {
        self.lon
    }
    fn record_len(&self) -> usize {
        NAV_NODE_FIXED_LEN + self.degree as usize * NAV_NEIGHBOR_LEN
    }
}

/// What the merge is worth reporting about itself — the §4.8.5 reachability report plus the counters
/// that make a broken seam visible.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NavStats {
    pub cell_nodes: usize,
    pub unified: usize,
    pub duplicate_edges: usize,
    pub components_found: usize,
    pub components_kept: usize,
    pub pruned_nodes: usize,
    pub pruned_edges: usize,
    pub nodes: usize,
    pub edges: usize,
    /// Share of the merged graph in its largest component, in per-mille. An implausibly small value
    /// is what a broken seam looks like (§4.8.5); the assembler reports it and never repairs it.
    pub largest_component_permille: u32,
    /// Adjacency entries refused at `OBCM_Spec.md` §8.3's degree cap of 24. The arc survives one-way
    /// through the neighbour's own record, which §8.3 explicitly permits — but it is a lost turn, so
    /// it is reported exactly as the packer reports its own.
    pub degree_truncated: usize,
    /// Junction records the §8.3 chunk-capacity guard refused (co-located nodes past the quadtree's
    /// recursion floor). Never silent: truncating the chunk instead would drop its `0xFF` sentinel.
    pub dropped_nodes: usize,
}

/// The merged graph, **already laid out**: the node quadtree and its bin-packed chunks in the file's
/// own encoding, and the edge pool as a plan rather than a buffer — every record's source address in
/// emission order, plus the padded length the layout comes to. Only the directory's absolute offsets
/// are left, which is what lets a shard's size be known before its header is written.
pub struct MergedNav {
    index: Vec<u8>,
    node_count: u32,
    chunks: Vec<u8>,
    chunk_count: u32,
    /// Every kept edge's record, in emission order, named by where its cell wrote it. The pool's
    /// bytes are streamed from those cells by [`serialize`] and are never held here.
    pool: Vec<EdgeRef>,
    /// What the pool comes to once §8.4's no-straddle padding and the chunk-aligned tail are
    /// applied — the one number the directory and the shard projection need from it.
    pool_len: usize,
    pub stats: NavStats,
}

impl MergedNav {
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Bytes this section occupies once written after `profile_table`.
    pub fn section_len(&self, profile_table: &[u8]) -> usize {
        NAV_DIR_LEN + profile_table.len() + self.index.len() + self.chunks.len() + self.pool_len
    }
}

/// Read, unify, prune, renumber and rebuild — §4.6 end to end. `cells` are the `network`-band cells;
/// `band_log2` is that band's cell size, which defines which coordinates are eligible for
/// unification.
///
/// `scratch` is where the node side is spilled and sorted (see the module header) and `budget` is
/// the most memory those passes may hold at once.
pub fn merge(
    cells: &[&Cell<'_>],
    band_log2: u32,
    min_component_edges: usize,
    global_bbox: UBox,
    scratch: &dyn ScratchStore,
    budget: usize,
) -> Result<MergedNav> {
    let mut stats = NavStats::default();
    // What one streaming buffer may hold. Several of them are alive at once in every pass below —
    // a reader, a writer, and a sort that takes half — so the share is deliberately small.
    let share = budget / 8;

    // --- 1/2. The serialized node set, unified at boundary coordinates only.
    //
    // The only whole-map structure here is the seam table, which is the 0.6 % of nodes another cell
    // can name. Everything else is per cell and is spilled as the cell ends. ---
    let mut seam: HashMap<(i32, i32), u32> = HashMap::new(); // coordinate → seam slot
    let mut seam_id: Vec<u32> = Vec::new(); // slot → the id its first cell minted
    let mut seam_digest: Vec<u64> = Vec::new(); // slot → §4.6.5 digest, still accumulating
    let mut node_out = SpillWriter::<NODE_REC>::create(scratch, share)?;
    let mut edge_out = SpillWriter::<EDGE_REC>::create(scratch, share)?;
    // §4.6.3's keys are generated here rather than in a pass of their own: the key is a projection
    // of the record, and the record is in hand exactly once.
    let mut dups = ExternalSort::<DUP_REC>::new(scratch, budget / 2, by_dup_key);
    let mut id_count: u32 = 0;
    let mut edge_count: u32 = 0;
    // Where each cell's minted ids start, with the total appended — the map from a collection id
    // back to the cell that named it, which is what lets §4.6.4 decompose per cell. One entry per
    // cell, so it is the cell list's size and not the graph's.
    let mut cell_base: Vec<u32> = Vec::with_capacity(cells.len() + 1);

    for (ci, cell) in cells.iter().enumerate() {
        cell_base.push(id_count);
        let dir = &cell.nav;
        if dir.is_empty() {
            continue;
        }
        let ci = u32::try_from(ci).expect("a cell list indexes in u32");
        let data_start =
            dir.data_start().ok_or_else(|| Error::Format(format!("cell {}: nav directory overflows", cell.id)))?;
        let pool = cell.read(dir.edge_pool_offset, dir.edge_chunk_count * dir.chunk_size)?;

        // Pass A: every junction record in the cell, straight off its chunks. Reading the chunk run
        // rather than walking the quadtree visits each record exactly once — §8.2's bin packing
        // makes the *walk* non-idempotent, not the storage.
        let base = id_count;
        let mut local: HashMap<u32, NodeRef> = HashMap::new();
        // (id, lat, lon, chunk, offset)
        let mut records: Vec<(u32, i32, i32, usize, usize)> = Vec::new();
        // The coordinates of the ids *this* cell minted, indexed by `id - base`. It is what the
        // cell's own slice of the node stream is written from, and it is the only per-node array
        // alive at any point — one cell's worth, not the map's.
        let mut minted: Vec<(i32, i32)> = Vec::new();
        let mut chunks: Vec<Vec<u8>> = Vec::with_capacity(dir.chunk_count);
        for k in 0..dir.chunk_count {
            let chunk = cell.read(data_start + k * dir.chunk_size, dir.chunk_size)?;
            let mut at = 0usize;
            while at + NAV_NODE_FIXED_LEN <= chunk.len() {
                let degree = chunk[at + 12];
                if degree == CHUNK_END {
                    break; // the §8.3 padding sentinel
                }
                let rec_len = NAV_NODE_FIXED_LEN + degree as usize * NAV_NEIGHBOR_LEN;
                if at + rec_len > chunk.len() {
                    return Err(Error::Format(format!("cell {}: nav record straddles chunk {k}", cell.id)));
                }
                let lat = i32::from_le_bytes(chunk[at..at + 4].try_into().expect("4 bytes"));
                let lon = i32::from_le_bytes(chunk[at + 4..at + 8].try_into().expect("4 bytes"));
                let id = u32::from_le_bytes(chunk[at + 8..at + 12].try_into().expect("4 bytes"));
                stats.cell_nodes += 1;
                let mut mint = || {
                    let id = id_count;
                    id_count += 1;
                    minted.push((lat, lon));
                    id
                };
                let node = if on_grid_boundary(lat as i64, lon as i64, band_log2) {
                    match seam.get(&(lat, lon)) {
                        Some(&slot) => {
                            stats.unified += 1;
                            NodeRef { id: seam_id[slot as usize], seam: slot }
                        }
                        None => {
                            let slot = seam_id.len() as u32;
                            let node = NodeRef { id: mint(), seam: slot };
                            seam.insert((lat, lon), slot);
                            seam_id.push(node.id);
                            seam_digest.push(0);
                            node
                        }
                    }
                } else {
                    NodeRef { id: mint(), seam: NO_SEAM }
                };
                if local.insert(id, node).is_some() {
                    return Err(Error::Format(format!("cell {}: node id {id} appears twice", cell.id)));
                }
                records.push((id, lat, lon, k, at));
                at += rec_len;
            }
            chunks.push(chunk);
        }

        // Pass B: adjacency → edges. Every edge shows up in both endpoints' records with the same
        // `Edge Id`, so the first sighting interns it and the second only has to agree — and, in
        // v12, to hand over the other direction's ascent. `edge_id` → its index in `pending`; the
        // value matters because the second direction writes back into the entry the first created.
        // Nothing here looks past the cell: cross-cell duplicates are settled once, after the whole
        // collection, by the sorted pass below — which is also why `pending` is a *cell's* edges and
        // not the map's. The write-back is the only reason an edge is held at all, and it never
        // crosses a cell, so the buffer dies with the cell that filled it.
        //
        // It is also where the §4.6.5 digest is accumulated (module header): a non-seam node's
        // incident edges are all in its own cell, so its total is final when this loop ends, and a
        // seam node's goes to the seam table where later cells can still add to it.
        let mut cell_digest = vec![0u64; minted.len()];
        let mut cell_edges: HashMap<u32, usize> = HashMap::new();
        let mut pending: Vec<MergedEdge> = Vec::new();
        for &(own_id, lat, lon, k, at) in &records {
            let chunk = &chunks[k];
            let degree = chunk[at + 12] as usize;
            for n in 0..degree {
                let e = &chunk[at + NAV_NODE_FIXED_LEN + n * NAV_NEIGHBOR_LEN..][..NAV_NEIGHBOR_LEN];
                let nbr_id = u32::from_le_bytes(e[0..4].try_into().expect("4 bytes"));
                let edge_id = u32::from_le_bytes(e[8..12].try_into().expect("4 bytes"));
                let cost_m = u16::from_le_bytes(e[12..14].try_into().expect("2 bytes")) as u32;
                let way_kind = e[14];
                let ascent_m = u16::from_le_bytes(
                    e[NAV_NEIGHBOR_ASCENT_OFF..NAV_NEIGHBOR_ASCENT_OFF + 2].try_into().expect("2 bytes"),
                );
                // v12: the second sighting of an edge is no longer redundant — it carries the *other*
                // direction's ascent, which nothing else in the file states. So it is read rather
                // than skipped, and only the edge itself is de-duplicated.
                if let Some(&index) = cell_edges.get(&edge_id) {
                    let edge: &mut MergedEdge = &mut pending[index];
                    // Orientation again: this entry runs from *this* record's node. It is the
                    // a→b direction exactly when this node is the edge's `a`.
                    let own = local.get(&own_id).expect("own id interned above").id;
                    if own == edge.a {
                        edge.ascent_ab = ascent_m;
                    } else {
                        edge.ascent_ba = ascent_m;
                    }
                    continue;
                }
                let own_node = *local.get(&own_id).expect("own id interned above");
                let nbr_node = *local.get(&nbr_id).ok_or_else(|| {
                    Error::Format(format!("cell {}: neighbour id {nbr_id} resolves to no record", cell.id))
                })?;
                let (a, b) = (own_node.id, nbr_node.id);
                // Everything this pass needs from the record's bytes is taken here, while the cell's
                // pool is still the buffer in hand: the orientation, the content hash, and the
                // length. What survives the loop is the ten-byte address, not the record.
                let rec = edge_record(&pool, edge_id, cell)?;
                // The record's anchor is endpoint `a`'s coordinate, so a record whose anchor is not
                // this node's coordinate belongs to the other direction — keep the orientation the
                // record itself states.
                let anchor_lat = i32::from_le_bytes(rec[7..11].try_into().expect("4 bytes"));
                let anchor_lon = i32::from_le_bytes(rec[11..15].try_into().expect("4 bytes"));
                let own_is_anchor = (anchor_lat, anchor_lon) == (lat, lon);
                let (a, b) = if own_is_anchor { (a, b) } else { (b, a) };
                let hash = fnv(rec);
                let rec = EdgeRef { cell: ci, off: edge_id, len: rec.len() as u16 };
                // This entry rides from the record's own node, so it books a→b when that node is
                // `a`. The opposite direction arrives with the neighbour's own entry above; if it
                // never does (a degree-capped arc, or a self-loop, which §8.3 writes once) the
                // other direction stays `0` — the same value a map packed without terrain carries.
                let (ascent_ab, ascent_ba) = if own_is_anchor { (ascent_m, 0) } else { (0, ascent_m) };
                // The tie-break's contribution, booked to both endpoints while they are in hand. A
                // self-loop books twice, exactly as summing over the edge list would.
                let h = digest_of(hash, cost_m, way_kind);
                for node in [own_node, nbr_node] {
                    add_digest(&mut seam_digest, &mut cell_digest, base, node, h);
                }
                cell_edges.insert(edge_id, pending.len());
                pending.push(MergedEdge { a, b, cost_m, kind: way_kind, rec, hash, ascent_ab, ascent_ba });
            }
        }

        // The cell is done: its nodes' coordinates are known and their digests are final (a seam
        // node's stays 0 here and arrives as a delta below). Ids were minted in this order, so the
        // appends land at exactly `id × NODE_REC`.
        for (k, &(lat, lon)) in minted.iter().enumerate() {
            node_out.push(node_record(lat, lon, cell_digest[k]))?;
        }
        // …and its edges go out in the order they were collected, so the stream stays *grouped by
        // cell* — which is what §4.6.4's per-cell decomposition reads it as — and each one drops its
        // §4.6.3 key into that pass's sort on the way past, while the record is in hand.
        for e in &pending {
            dups.push(dup_record(e, edge_count))?;
            edge_count += 1;
            edge_out.push(e.encode())?;
        }
    }
    cell_base.push(id_count);

    let (node_file, spilled) = node_out.seal()?;
    let (edge_file, edge_total) = edge_out.seal()?;
    debug_assert_eq!(spilled, id_count as u64, "one spilled record per minted node id");
    if id_count == 0 {
        scratch.remove(node_file)?;
        scratch.remove(edge_file)?;
        return Ok(MergedNav::empty(stats));
    }

    // The §4.6.5 digest corrections (module header): every seam node's accumulated total, plus a
    // subtraction for each duplicate the pass below drops. Both are added to what the stream holds.
    let mut deltas: Vec<(u32, u64)> =
        seam_id.iter().copied().zip(seam_digest.iter().copied()).filter(|&(_, d)| d != 0).collect();
    drop(seam);
    drop(seam_digest);

    // --- 3. Deduplicate (§4.6.3) from the keys the collection already handed over, then 4. prune
    // islands over the spilled stream itself. ---
    let dead = dedup(dups, &mut deltas, &mut stats)?;
    let pruned = prune::prune(
        scratch,
        share,
        (edge_file, edge_total),
        &dead,
        &cell_base,
        &seam_id,
        id_count,
        min_component_edges,
        &mut stats,
    )?;
    drop(cell_base);
    drop(seam_id);

    // --- 5. Renumber densely by (lat, lon) ascending — deterministic and content-derived (§4.6.5).
    //
    // Two *distinct* surviving nodes can share a coordinate: unification is restricted to boundary
    // lines (§4.6.2), so the interior collisions a single file legitimately contains — stacked
    // bridge/tunnel junctions — arrive here as separate nodes at one `(lat, lon)`. Their order must
    // still come from content, not from which cell happened to be read first, or two assemblies of
    // the same cells in a different order would produce different bytes. The tie-break is therefore
    // an order-independent digest of the node's own incident edges.
    //
    // Done as a sorted pass over the spilled node stream rather than a permutation of an in-memory
    // array (module header) — the same key, and `nodes` comes out already in the emission order the
    // quadtree wants. What the edges are resolved through is no longer an array either: it is the
    // `(id, dense, lat, lon)` stream this returns, sorted by id and joined against below. ---
    deltas.sort_unstable();
    let (mut nodes, dense_by_id) =
        renumber(scratch, budget, share, node_file, pruned.node_comp, &pruned.keep, &deltas)?;
    drop(deltas);

    // --- 5 (cont.). Attach the dense ids, by merge join rather than by lookup: sort what survived
    // by `a`, walk it beside the id-ordered dense stream, then do the same for `b`. The endpoint
    // coordinates come across with the ids, because §8.3's neighbour deltas are measured from them
    // and re-reading them later would be exactly the whole-map array this pass is removing.
    //
    // The second pass also counts each junction's **uncapped** degree while both dense ids are in
    // hand, which is what lets the emission walk below be a single pass.
    //
    // Each join hands its result straight into the **next** pass's sort rather than through a file:
    // a sort that is generating runs is holding at most half its budget, and one that is merging is
    // holding all of it, so two of them at half a budget each are one budget — and the stream
    // between them is never written down at all. ---
    let by_b = join_first(scratch, budget, share, edge_file, &pruned, &dead, dense_by_id)?;
    scratch.remove(edge_file)?;
    scratch.remove(pruned.edge_comp)?;
    drop(pruned);
    drop(dead);
    let mut fill: Vec<u32> = vec![0; nodes.len()]; // uncapped degree, then entries written
    let by_emit = join_second(scratch, budget, share, by_b, dense_by_id, &mut fill, &mut stats)?;
    scratch.remove(dense_by_id)?;

    // --- 6/7. Lay the edge pool out and rebuild the adjacency, in **one** walk of the emission
    // order (§4.6.6).
    //
    // `Edge Id` is a pool byte offset, so every record lands at a new place and the no-straddle rule
    // is re-applied at the 512-byte granularity — but that is arithmetic over the records' *lengths*
    // (see [`serialize`], which walks the same rule to emit the padding), so the cursor below mints
    // the id of the record it is about to place, and the two adjacency entries that quote that id
    // are written on the spot.
    //
    // One CSR buffer, not a `Vec` per junction: at state scale that was three million heap
    // allocations and three million 24-byte headers for lists averaging under three entries. §8.3's
    // degree cap is order-sensitive — it does not "keep 24 of the arcs", it refuses the ones that
    // arrive after a node is already full — so the offsets reserve `min(degree, 24)` from the
    // uncapped degrees the join counted, and the walk refuses at exactly the point a growing `Vec`
    // used to. (Reserving the capped count is exact: a node is pushed to `degree` times and stops
    // accepting at 24, so it ends with `min(degree, 24)` entries and the buffer has no slack.) ---
    // An offset counts at most two entries per kept edge, and step 6 refuses a pool past 4 GiB of
    // records that are 15 bytes at their smallest — so it is a `u32` by construction.
    let mut offsets: Vec<u32> = Vec::with_capacity(nodes.len() + 1);
    let mut total = 0usize;
    for d in fill.iter_mut() {
        offsets.push(total as u32);
        total += (*d as usize).min(NAV_MAX_DEGREE);
        *d = 0;
    }
    offsets.push(total as u32);
    let mut csr: Vec<WireNeighbor> = vec![WireNeighbor::default(); total];
    let mut pool: Vec<EdgeRef> = Vec::with_capacity(stats.edges);
    let mut at = 0usize;
    {
        for rec in by_emit.finish()? {
            let rec = rec?;
            let r = dense_ref(&rec);
            let len = r.len as usize;
            at = place(at, len);
            if at > u32::MAX as usize {
                return Err(Error::Capacity(
                    "the merged edge pool passes 4 GiB: `Edge Id` is a uint32 pool byte offset (OBCA §5.7)".into(),
                ));
            }
            let edge_id = at as u32;
            pool.push(r);
            at += len;

            let (a, b) = (dense_id(&rec, A_AT), dense_id(&rec, B_AT));
            let (a_at, b_at) = (dense_coord(&rec, A_AT), dense_coord(&rec, B_AT));
            let (cost_m, way_kind) = (dense_cost(&rec), dense_kind(&rec));
            let (ascent_ab, ascent_ba) = dense_ascents(&rec);
            let mut push = |from: u32, (flat, flon): (i32, i32), to: u32, (lat, lon): (i32, i32), ascent_m: u16| {
                let used = &mut fill[from as usize];
                if *used as usize >= NAV_MAX_DEGREE {
                    stats.degree_truncated += 1;
                    return Ok(());
                }
                let (dlat, dlon) = (lat as i64 - flat as i64, lon as i64 - flon as i64);
                if dlat.abs() > MAX_NEIGHBOR_DELTA || dlon.abs() > MAX_NEIGHBOR_DELTA {
                    return Err(Error::Format(format!(
                        "a merged adjacency spans ({dlat}, {dlon}) µdeg, past the §8.3 int16 neighbour delta"
                    )));
                }
                csr[offsets[from as usize] as usize + *used as usize] =
                    WireNeighbor { id: to, lat, lon, edge_id, cost_m: cost_m.min(u16::MAX as u32), way_kind, ascent_m };
                *used += 1;
                Ok(())
            };
            push(a, a_at, b, b_at, ascent_ab)?;
            if a != b {
                push(b, b_at, a, a_at, ascent_ba)?; // a self-loop appears once (§8.3)
            }
        }
    }
    // The tail pads to a whole chunk, because §8.1's `Edge Chunk Count` measures the pool in chunks.
    let pool_len = at.div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE;
    debug_assert_eq!(pool.len(), stats.edges, "one pool entry per kept edge");

    // The CSR views, into the points that were renumbered into it. `degree` is what the walk above
    // actually wrote, which is `min(uncapped, 24)`.
    for (dense, p) in nodes.iter_mut().enumerate() {
        p.at = offsets[dense];
        p.degree = fill[dense] as u8;
    }
    // The views are copied into the points; the two build vectors have no further reader.
    drop(offsets);
    drop(fill);
    stats.nodes = nodes.len();

    // --- 7 (cont.). The node quadtree over the **assembly** bbox, with §8.2's bin-packed 512-byte
    // chunks. Laid out here so a shard's size is known before its header is written. The tree owns
    // the points and borrows nothing, but the records they name live in `csr`, which therefore has
    // to outlive the flatten below — it is a local of this function and does. ---
    let tree = qtree::build(nodes, global_bbox, NAV_CHUNK_SIZE);
    let (index, node_count, chunks, chunk_count, dropped) =
        qtree::flatten(&tree, NAV_CHUNK_SIZE, true, &|p, out| pack_record(p, &csr, out));
    stats.dropped_nodes = dropped;
    Ok(MergedNav { index, node_count, chunks, chunk_count, pool, pool_len, stats })
}

/// §4.6.3's duplicate check, as one walk of the sorted key stream the collection filled.
///
/// An edge two cells both wrote *in full*, keyed on the unified endpoint pair, `Cost M`, `Way Kind`
/// and the record's content. The half-open ownership of §3.3/§3.4(3) should already prevent it, so
/// this is a net, not a mechanism — and a net does not have to be a hash set carried through the
/// entire collection. It runs once, here, over a sorted stream of 25-byte keys, and the
/// **first-collected** copy of each key survives.
///
/// *Why that is the same outcome as refusing duplicates at the door.* `cell_edges` is per-cell and
/// unchanged, so a cell still interns each of its own `Edge Id`s once and still writes the second
/// direction's ascent back into its **own** entry — no write-back ever crossed cells, because
/// nothing outside a cell's own `cell_edges` could name another cell's entry. What changes is only
/// the fate of a later copy: it used to be refused before it was pushed (its id mapped to `None`, so
/// that cell's second direction wrote nowhere), and now it is collected, takes that cell's own
/// write-backs, and is dropped here. Either way the survivor is the first-collected entry carrying
/// the first cell's ascents, which is what the bytes are. The counts agree for the same reason:
/// exactly one entry per (cell, `Edge Id`) reaches this pass, and the ones that die here are exactly
/// the ones the door used to refuse — one per refusal, with three copies of an edge counting two, as
/// before. The argument does not depend on the duplicate being *cross-cell*: two distinct `Edge Id`s
/// inside one cell with identical content and endpoints collapse the same way, the lower index
/// surviving, because collection order inside a cell is the order the old first-sighting rule used
/// too.
///
/// *And why sorting by the whole key is the same set of deaths.* The previous formulation sorted by
/// the content hash and, inside each run of equal hashes, killed an entry that any **earlier** entry
/// of the run matched on the full key — which is exactly "every member of a full-key group except
/// its first". Equal keys have equal hashes, so a group never spans two runs, and adding the rest of
/// the key to the sort only orders the groups within a run. With the collection index last, the
/// first record of each group is the lowest-indexed copy: the survivor, in one forward pass instead
/// of a quadratic search.
///
/// What comes back is the dead copies' collection indices, ascending, and `deltas` has gained the
/// §4.6.5 contributions each of them owes back to its two endpoints. Both are sized by the number of
/// duplicates — a defect count, not a map size, and zero on both published regions.
fn dedup(sort: ExternalSort<'_, DUP_REC>, deltas: &mut Vec<(u32, u64)>, stats: &mut NavStats) -> Result<Vec<u32>> {
    let mut dead: Vec<u32> = Vec::new();
    let mut previous: Option<(u64, u32, u32, u32, u8)> = None;
    for rec in sort.finish()? {
        let rec = rec?;
        let key = dup_key(&rec);
        if previous == Some(key) {
            // A dropped copy's digest contribution was booked during collection and has to come back
            // off, at both of the endpoints it was booked to. `wrapping_neg` of what was added:
            // exact, because the accumulation is a wrapping sum and nothing else about it is
            // order-dependent. The endpoints are the key's own `lo`/`hi`, in whichever order — the
            // same two ids either way.
            let undo = digest_of(key.0, key.3, key.4).wrapping_neg();
            deltas.push((key.1, undo));
            deltas.push((key.2, undo));
            dead.push(dup_index(&rec));
        }
        previous = Some(key);
    }
    stats.duplicate_edges += dead.len();
    // The later passes walk the stream in collection order with this as a cursor.
    dead.sort_unstable();
    Ok(dead)
}

/// The first join: the surviving, kept edges, with endpoint `a` resolved to its dense id and
/// coordinate.
///
/// This is also where §4.6.4's verdict is applied to the edges. The prune's label stream runs beside
/// the edge stream — one label per **surviving** edge, in collection order — so the two are read in
/// lockstep with the dead list as the third cursor, and no edge is ever looked up by index.
fn join_first<'s>(
    scratch: &'s dyn ScratchStore,
    budget: usize,
    share: usize,
    edge_file: ScratchId,
    pruned: &prune::Pruned,
    dead: &[u32],
    dense_by_id: ScratchId,
) -> Result<ExternalSort<'s, DENSE_REC>> {
    let mut sort = ExternalSort::<EDGE_REC>::new(scratch, budget / 2, by_edge_a);
    {
        let mut labels = SpillReader::<4>::open(scratch, pruned.edge_comp, share)?;
        let mut next_dead = 0usize;
        for (index, rec) in SpillReader::<EDGE_REC>::open(scratch, edge_file, share)?.enumerate() {
            let rec = rec?;
            if next_dead < dead.len() && dead[next_dead] as usize == index {
                next_dead += 1;
                continue;
            }
            let label = labels.next().ok_or_else(|| {
                Error::Scratch("the §4.6.4 label stream is shorter than the surviving edge stream".into())
            })??;
            if pruned.keep[u32::from_le_bytes(label) as usize] {
                sort.push(rec)?;
            }
        }
    }

    let mut out = ExternalSort::<DENSE_REC>::new(scratch, budget / 2, by_endpoint_b);
    let mut dense = SpillReader::<JOIN_REC>::open(scratch, dense_by_id, share)?;
    let mut head = dense.next().transpose()?;
    for rec in sort.finish()? {
        let rec = rec?;
        let a = edge_a(&rec);
        // Both streams ascend, so the cursor only moves forward — this is a merge, not a lookup.
        while head.map(|h| join_id(&h) < a).unwrap_or(false) {
            head = dense.next().transpose()?;
        }
        let h = head.filter(|h| join_id(h) == a).ok_or_else(|| {
            Error::Format(format!("a kept edge names node {a}, which §4.6.4 did not keep — the prune split an edge"))
        })?;
        let mut wide = [0u8; DENSE_REC];
        put_endpoint(&mut wide, A_AT, &h);
        wide[B_AT..B_AT + 4].copy_from_slice(&rec[4..8]); // `b`, still a collection id
        wide[24..28].copy_from_slice(&rec[8..12]); // cost
        wide[28..36].copy_from_slice(&rec[12..20]); // hash
        wide[36..46].copy_from_slice(&rec[20..30]); // cell, off, len
        wide[46..50].copy_from_slice(&rec[30..34]); // both ascents
        wide[50] = rec[34]; // kind
        out.push(wide)?;
    }
    Ok(out)
}

/// The second join: endpoint `b`, and the uncapped degrees the emission walk reserves from. What it
/// returns is the emission sort, already loaded — see [`merge`] for why the streams between the
/// joins are never written down.
fn join_second<'s>(
    scratch: &'s dyn ScratchStore,
    budget: usize,
    share: usize,
    sort: ExternalSort<'s, DENSE_REC>,
    dense_by_id: ScratchId,
    fill: &mut [u32],
    stats: &mut NavStats,
) -> Result<ExternalSort<'s, DENSE_REC>> {
    let mut out = ExternalSort::<DENSE_REC>::new(scratch, budget / 2, by_emission);
    let mut dense = SpillReader::<JOIN_REC>::open(scratch, dense_by_id, share)?;
    let mut head = dense.next().transpose()?;
    let mut kept = 0usize;
    for rec in sort.finish()? {
        let mut rec = rec?;
        let b = dense_id(&rec, B_AT);
        while head.map(|h| join_id(&h) < b).unwrap_or(false) {
            head = dense.next().transpose()?;
        }
        let h = head.filter(|h| join_id(h) == b).ok_or_else(|| {
            Error::Format(format!("a kept edge names node {b}, which §4.6.4 did not keep — the prune split an edge"))
        })?;
        put_endpoint(&mut rec, B_AT, &h);
        let (a, b) = (dense_id(&rec, A_AT), dense_id(&rec, B_AT));
        fill[a as usize] += 1;
        if a != b {
            fill[b as usize] += 1;
        }
        kept += 1;
        out.push(rec)?;
    }
    stats.edges = kept;
    Ok(out)
}

/// One edge's contribution to its endpoints' §4.6.5 tie-break digest.
///
/// Stated once because it is now computed in two places — when the edge is collected, and (negated)
/// when a §4.6.3 duplicate is dropped — and a disagreement between them would move node ids.
fn digest_of(hash: u64, cost_m: u32, kind: u8) -> u64 {
    hash ^ ((cost_m as u64) << 8) ^ kind as u64
}

/// Book `h` to `node`: to the seam table when another cell can still add to it, and to the cell's
/// own array otherwise. `base` is the first id this cell minted.
fn add_digest(seam_digest: &mut [u64], cell_digest: &mut [u64], base: u32, node: NodeRef, h: u64) {
    let slot = if node.seam == NO_SEAM {
        &mut cell_digest[(node.id - base) as usize]
    } else {
        &mut seam_digest[node.seam as usize]
    };
    *slot = slot.wrapping_add(h);
}

/// The spilled node record — see [`NODE_REC`] for why it carries no id.
fn node_record(lat: i32, lon: i32, digest: u64) -> [u8; NODE_REC] {
    let mut r = [0u8; NODE_REC];
    r[0..4].copy_from_slice(&lat.to_le_bytes());
    r[4..8].copy_from_slice(&lon.to_le_bytes());
    r[8..16].copy_from_slice(&digest.to_le_bytes());
    r
}

/// …and the one §4.6.5 sorts, which is that record plus the id it was spilled under.
fn sort_record(rec: &[u8; NODE_REC], id: u32) -> [u8; SORT_REC] {
    let mut r = [0u8; SORT_REC];
    r[0..NODE_REC].copy_from_slice(rec);
    r[NODE_REC..SORT_REC].copy_from_slice(&id.to_le_bytes());
    r
}

/// `(lat, lon, digest, id)` out of a [`SORT_REC`] record.
fn node_key(r: &[u8; SORT_REC]) -> (i32, i32, u64, u32) {
    (
        i32::from_le_bytes(r[0..4].try_into().expect("4 bytes")),
        i32::from_le_bytes(r[4..8].try_into().expect("4 bytes")),
        u64::from_le_bytes(r[8..16].try_into().expect("8 bytes")),
        u32::from_le_bytes(r[16..20].try_into().expect("4 bytes")),
    )
}

/// §4.6.5's order, as a comparator.
///
/// It is a **total** order: the collection id is unique, so no two records compare equal and the
/// result does not depend on the sort's stability. That matters because the spec's own key —
/// `(lat, lon, digest)` — is *not* total (two stacked junctions with identical incident edges would
/// tie), and the id is exactly the tie-break the previous formulation got from a stable sort over
/// the collection order.
fn by_node_key(a: &[u8; SORT_REC], b: &[u8; SORT_REC]) -> std::cmp::Ordering {
    node_key(a).cmp(&node_key(b))
}

/// §4.6.5's renumbering, as a sorted pass over the spilled node stream.
///
/// Reads the stream once — its index *is* the collection id — applies the digest deltas
/// (`(id, addend)`, sorted, possibly several per id), drops the pruned nodes, and sorts what is left
/// externally. The walk over the sorted stream then hands out dense ids in order, which produces
/// both halves the rest of the merge needs: the junctions **in emission order**, and the
/// collection-id → dense-id map the edges are still resolved through.
///
/// The second half used to be the one whole-map array left on the node side — a
/// collection-id-indexed `Vec<u32>`, because the caller still held its edges in memory and looked
/// them up in it. It does not any more (#1116 D3): the walk emits `(id, dense, lat, lon)` records,
/// a second external sort puts them in **id** order, and the edges — which are a sorted stream now
/// — are resolved against that by merge join. The coordinate rides along because the §8.3 adjacency
/// deltas need the neighbour's, and the alternative is keeping the node array addressable.
///
/// `node_comp` is §4.6.4's label stream, one `u32` per collection id in id order, so it is read in
/// lockstep with the node stream and `keep` is indexed by the label rather than by the node.
fn renumber(
    scratch: &dyn ScratchStore,
    budget: usize,
    share: usize,
    node_file: ScratchId,
    node_comp: ScratchId,
    keep: &[bool],
    deltas: &[(u32, u64)],
) -> Result<(Vec<NavPoint>, ScratchId)> {
    // Three things are alive at once here — the two readers, the §4.6.5 sort, and (in the walk
    // below) the id-order sort it feeds. So the two sorts take half the budget each and the readers
    // take a share; the streams are read strictly forward, which is why their share is small.
    let read_budget = share.max(NODE_REC);
    let mut sort = ExternalSort::<SORT_REC>::new(scratch, (budget / 2).max(SORT_REC), by_node_key);
    {
        let mut labels = SpillReader::<4>::open(scratch, node_comp, read_budget)?;
        let mut next_delta = 0usize;
        for (id, rec) in SpillReader::<NODE_REC>::open(scratch, node_file, read_budget)?.enumerate() {
            let mut rec = rec?;
            let id = id as u32;
            // Deltas are sorted by id and the stream is walked in id order, so this cursor only ever
            // moves forward — even when several deltas name the same node.
            while next_delta < deltas.len() && deltas[next_delta].0 == id {
                let digest = u64::from_le_bytes(rec[8..16].try_into().expect("8 bytes"));
                rec[8..16].copy_from_slice(&digest.wrapping_add(deltas[next_delta].1).to_le_bytes());
                next_delta += 1;
            }
            let label = labels
                .next()
                .ok_or_else(|| Error::Scratch("the §4.6.4 label stream is shorter than the node stream".into()))??;
            if keep[u32::from_le_bytes(label) as usize] {
                sort.push(sort_record(&rec, id))?;
            }
        }
    }
    scratch.remove(node_file)?;
    scratch.remove(node_comp)?;

    let mut nodes: Vec<NavPoint> = Vec::new();
    let mut by_id = ExternalSort::<JOIN_REC>::new(scratch, (budget / 2).max(JOIN_REC), by_join_id);
    for rec in sort.finish()? {
        let (lat, lon, _, id) = node_key(&rec?);
        let dense = u32::try_from(nodes.len()).expect("a kept node count fits a u32");
        let mut join = [0u8; JOIN_REC];
        join[0..4].copy_from_slice(&id.to_le_bytes());
        join[4..8].copy_from_slice(&dense.to_le_bytes());
        join[8..12].copy_from_slice(&lat.to_le_bytes());
        join[12..16].copy_from_slice(&lon.to_le_bytes());
        by_id.push(join)?;
        // `at` and `degree` are the CSR view, which does not exist yet — the emission walk fills
        // them in.
        nodes.push(NavPoint { lat, lon, id: dense, at: 0, degree: 0 });
    }

    // Materialized rather than handed back as a stream, because the two joins each read it whole.
    let mut out = SpillWriter::<JOIN_REC>::create(scratch, share)?;
    for rec in by_id.finish()? {
        out.push(rec?)?;
    }
    Ok((nodes, out.seal()?.0))
}

/// §8.4's placement rule: a record of `len` bytes goes at the cursor `at`, unless it would straddle
/// a chunk boundary — then it goes at the next one, and the bytes it skips are `0xFF` padding.
///
/// One function because there are two callers and they must not drift: [`merge`] lays the pool out
/// with it to mint the `Edge Id`s, and [`serialize`] walks the same rule to emit the padding those
/// ids assume. A disagreement between the two would be a file whose adjacency points into the gaps.
fn place(at: usize, len: usize) -> usize {
    let within = at % NAV_CHUNK_SIZE;
    if within + len > NAV_CHUNK_SIZE {
        at + (NAV_CHUNK_SIZE - within)
    } else {
        at
    }
}

/// One §8.4 edge record, located inside its cell's pool at `edge_id` (a pool-relative byte offset)
/// and checked there: in the pool, a polyline of at least two vertices, and not straddling its
/// chunk. The slice is borrowed from the pool buffer the caller has open — the merge reads what it
/// needs from it and keeps only the address.
fn edge_record<'p>(pool: &'p [u8], edge_id: u32, cell: &Cell<'_>) -> Result<&'p [u8]> {
    let at = edge_id as usize;
    let bad = |what: &str| Error::Format(format!("cell {}: edge id {edge_id} {what}", cell.id));
    if at % NAV_CHUNK_SIZE + NAV_EDGE_FIXED_LEN > NAV_CHUNK_SIZE || at + NAV_EDGE_FIXED_LEN > pool.len() {
        return Err(bad("is out of the edge pool"));
    }
    let pt_count = u16::from_le_bytes(pool[at + 4..at + 6].try_into().expect("2 bytes")) as usize;
    if pt_count < 2 {
        return Err(bad("decodes to a polyline with fewer than two vertices"));
    }
    let rec_len = NAV_EDGE_FIXED_LEN + (pt_count - 1) * 4;
    if at % NAV_CHUNK_SIZE + rec_len > NAV_CHUNK_SIZE || at + rec_len > pool.len() {
        return Err(bad("straddles its chunk"));
    }
    Ok(&pool[at..at + rec_len])
}

/// FNV-1a 64 over a record's bytes — the identity half of the §4.6.3 duplicate key. Content, not
/// address, so two cells that wrote the same edge agree.
fn fnv(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Write the whole §8 section at absolute byte `section_offset` through `sink`:
/// `[directory][profile table][node index][node chunks][edge pool]`.
///
/// Every offset in the directory is known before the first byte goes out — that is what
/// [`MergedNav::section_len`] already proves — so nothing is back-patched and nothing is staged. The
/// pool is written last and written *through*: the layout says where each record goes, and the
/// record's bytes come from the cell that wrote them, one read into one reusable buffer at a time.
/// `cells` must therefore be the same `network` cells [`merge`] read, in the same order.
///
/// `profile_table` is the cells' own, copied after every cell was checked to agree (§4.3) — it is
/// schema data and the assembler has no business re-deriving it. An empty graph still writes the
/// directory and the profile table, both regions zero-length just past it.
pub fn serialize(
    nav: &MergedNav,
    profile_table: &[u8],
    section_offset: usize,
    cells: &[&Cell<'_>],
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<()> {
    let profile_count = profile_table.len() / obc_formats::obcm::NAV_PROFILE_LEN;
    let profile_table_offset = section_offset + NAV_DIR_LEN;
    let index_offset = profile_table_offset + profile_table.len();
    let edge_pool_offset = index_offset + nav.index.len() + nav.chunks.len();
    let edge_chunk_count = (nav.pool_len / NAV_CHUNK_SIZE) as u32;

    let mut written = 0usize;
    let mut out = |buf: &[u8]| -> Result<()> {
        written += buf.len();
        sink(buf)
    };

    let mut dir = Vec::with_capacity(NAV_DIR_LEN);
    dir.extend_from_slice(&(index_offset as u32).to_le_bytes());
    dir.extend_from_slice(&nav.node_count.to_le_bytes());
    dir.extend_from_slice(&nav.chunk_count.to_le_bytes());
    dir.extend_from_slice(&(edge_pool_offset as u32).to_le_bytes());
    dir.extend_from_slice(&edge_chunk_count.to_le_bytes());
    dir.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes());
    dir.extend_from_slice(&(profile_table_offset as u32).to_le_bytes());
    dir.push(profile_count as u8);
    dir.push(0); // reserved
    debug_assert_eq!(dir.len(), NAV_DIR_LEN);
    out(&dir)?;
    out(profile_table)?;
    out(&nav.index)?;
    out(&nav.chunks)?;

    // The pool, record by record. `pad` is §8.3's `0xFF` sentinel run — the bytes a record's chunk
    // gets instead of a record that would straddle it — and `rec` is the one buffer every record is
    // read into. Both are a chunk long, which is the largest either can ever be.
    let pad = [CHUNK_END; NAV_CHUNK_SIZE];
    let mut rec = [0u8; NAV_CHUNK_SIZE];
    let mut at = 0usize;
    for r in &nav.pool {
        let len = r.len as usize;
        // The same rule the layout used, so the padding lands exactly where the `Edge Id`s say.
        let start = place(at, len);
        if start > at {
            out(&pad[..start - at])?;
            at = start;
        }
        // The layout named a cell by its index in the list the merge read. Handing a different list
        // to the write would produce a file full of plausible-looking wrong polylines, so it is a
        // refusal rather than an index.
        let cell = cells.get(r.cell as usize).ok_or_else(|| {
            Error::Verify(format!(
                "the nav section names source cell {} but the write was handed {} cell(s) — this is not the cell list \
                 the §4.6 merge read",
                r.cell,
                cells.len()
            ))
        })?;
        let buf = &mut rec[..len];
        cell.read_into(cell.nav.edge_pool_offset + r.off as usize, buf)?;
        out(buf)?;
        at += len;
    }
    // §8.1 measures the pool in whole chunks, so the tail pads out to one.
    if at < nav.pool_len {
        // `pad` is `[0xFF; 512]`; the remainder is at most one chunk by construction.
        out(&pad[..nav.pool_len - at])?;
    }
    debug_assert_eq!(written, nav.section_len(profile_table));
    Ok(())
}

/// The §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`, then 15 bytes per
/// neighbour with the coord stored as an `int16` delta from this record's own. `csr` is the shared
/// adjacency buffer the point's `(at, degree)` view names.
fn pack_record(p: &NavPoint, csr: &[WireNeighbor], out: &mut Vec<u8>) {
    let neighbors = &csr[p.at as usize..][..p.degree as usize];
    out.extend_from_slice(&p.lat.to_le_bytes());
    out.extend_from_slice(&p.lon.to_le_bytes());
    out.extend_from_slice(&p.id.to_le_bytes());
    out.push(neighbors.len() as u8);
    for n in neighbors {
        out.extend_from_slice(&n.id.to_le_bytes());
        out.extend_from_slice(&((n.lat as i64 - p.lat as i64) as i16).to_le_bytes());
        out.extend_from_slice(&((n.lon as i64 - p.lon as i64) as i16).to_le_bytes());
        out.extend_from_slice(&n.edge_id.to_le_bytes());
        out.extend_from_slice(&(n.cost_m.min(u16::MAX as u32) as u16).to_le_bytes());
        out.push(n.way_kind);
        out.extend_from_slice(&n.ascent_m.to_le_bytes());
    }
}

impl MergedNav {
    /// The graph a shard with no nav carries: the directory plus the always-present profile table,
    /// both data regions zero-length (§5.1/§8.1).
    pub fn empty(stats: NavStats) -> MergedNav {
        MergedNav {
            index: Vec::new(),
            node_count: 0,
            chunks: Vec::new(),
            chunk_count: 0,
            pool: Vec::new(),
            pool_len: 0,
            stats,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::MemoryScratch;

    /// **The budget sweep.** Every whole-merge fixture goes through here rather than calling
    /// [`merge`] directly, because §4.6.5's renumbering is now an external sort and a fixture is far
    /// too small to reach its spill path on its own — at any sane budget a dozen nodes sort in one
    /// run, and the k-way merge would never be exercised by this file at all.
    ///
    /// So each fixture is merged at four budgets, from *one record per run* upwards, and the results
    /// are compared as bytes. The map may not depend on how much memory the merge was given; that is
    /// the whole claim the pass rests on. It also asserts the scratch area is **empty** afterwards:
    /// a merge that leaked a run would still produce the right map, and would fill a card.
    fn merged(cells: &[&Cell<'_>], band_log2: u32, min_component_edges: usize, bbox: UBox) -> MergedNav {
        // Everything a shard is written from — `pool` by address, since `EdgeRef` is not `PartialEq`.
        #[allow(clippy::type_complexity)]
        let shape = |n: &MergedNav| -> (Vec<u8>, Vec<u8>, u32, u32, usize, Vec<(u32, u32, u16)>, NavStats) {
            (
                n.index.clone(),
                n.chunks.clone(),
                n.node_count,
                n.chunk_count,
                n.pool_len,
                n.pool.iter().map(|r| (r.cell, r.off, r.len)).collect(),
                n.stats.clone(),
            )
        };
        let mut out: Option<MergedNav> = None;
        for budget in [1usize, 24, 200, 1 << 20] {
            let scratch = MemoryScratch::new();
            let nav = merge(cells, band_log2, min_component_edges, bbox, &scratch, budget).expect("the merge succeeds");
            assert_eq!(scratch.resident_bytes(), 0, "budget {budget}: the merge left a scratch file behind");
            match &out {
                None => out = Some(nav),
                Some(first) => assert_eq!(shape(&nav), shape(first), "budget {budget} merged to different bytes"),
            }
        }
        out.expect("the sweep runs at least once")
    }

    /// The section through the sink, for a test that wants it as one buffer.
    fn serialized(nav: &MergedNav, profile_table: &[u8], section_offset: usize, cells: &[&Cell<'_>]) -> Vec<u8> {
        let mut out = Vec::new();
        serialize(nav, profile_table, section_offset, cells, &mut |b: &[u8]| {
            out.extend_from_slice(b);
            Ok(())
        })
        .expect("the section serializes");
        out
    }

    #[test]
    fn an_empty_section_still_carries_its_profiles() {
        let profiles = vec![0u8; obc_formats::obcm::NAV_PROFILE_LEN];
        let bytes = serialized(&MergedNav::empty(NavStats::default()), &profiles, 500, &[]);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0, "empty graph ⇒ no index nodes");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()) as usize, NAV_CHUNK_SIZE, "pinned 512");
        assert_eq!(u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize, 500 + NAV_DIR_LEN);
        assert_eq!(bytes[26], 1, "one profile");
        assert_eq!(bytes.len(), NAV_DIR_LEN + profiles.len());
    }

    /// Where a synthetic cell's edge pool starts — deliberately not 0, so an emission that forgot to
    /// add the cell's `edge_pool_offset` to an `Edge Id` reads the wrong bytes.
    const SRC_POOL_AT: usize = 64;

    /// A cell that is nothing but an edge pool: all [`serialize`] ever asks a cell for is
    /// `nav.edge_pool_offset` and a byte range under it.
    fn pool_cell<'a>(src: &'a dyn obc_formats::io::ByteSource, chunks: usize) -> Cell<'a> {
        Cell {
            id: crate::grid::CellId::parse("18/1052/1204").expect("a canonical id"),
            band: "network".into(),
            src,
            partial: false,
            lods: Vec::new(),
            pois: obc_reader::PoiDirectory::EMPTY,
            nav: obc_reader::NavDirectory {
                edge_pool_offset: SRC_POOL_AT,
                edge_chunk_count: chunks,
                chunk_size: NAV_CHUNK_SIZE,
                ..obc_reader::NavDirectory::EMPTY
            },
            profile_table: Vec::new(),
            style_ids: Vec::new(),
            bytes: src.len() as u64,
        }
    }

    /// **The streaming pin.** The pool is laid out as offsets in [`merge`] and emitted as bytes in
    /// [`serialize`], from two walks of [`place`] that never meet — so this builds the pool the old
    /// way (a `Vec<u8>` records were appended to, padded with `0xFF` wherever the next one would
    /// straddle) and asserts the streamed section is that buffer, byte for byte.
    ///
    /// The records are laid out in the source in a *different* order than they are emitted, which is
    /// the whole point of §4.6.6: placement is the only thing the merge changes.
    #[test]
    fn the_streamed_pool_is_the_pool_the_merge_laid_out() {
        // Source: two chunks holding A (200 B), B (300 B) and C (300 B), each a distinct fill.
        let mut src = vec![0u8; SRC_POOL_AT + 2 * NAV_CHUNK_SIZE];
        let source: [(usize, usize, u8); 3] = [(0, 200, 0xA1), (200, 300, 0xB2), (512, 300, 0xC3)];
        for &(at, len, fill) in &source {
            src[SRC_POOL_AT + at..SRC_POOL_AT + at + len].fill(fill);
        }
        let slice = obc_formats::io::SliceSource(&src);
        let cell = pool_cell(&slice, 2);

        // Emission order B, C, A: B fills 0..300, C would straddle and starts the next chunk, A fits
        // behind it, and the tail pads out to a whole chunk.
        let order = [source[1], source[2], source[0]];
        let pool: Vec<EdgeRef> =
            order.iter().map(|&(at, len, _)| EdgeRef { cell: 0, off: at as u32, len: len as u16 }).collect();

        // The old formulation, verbatim, as the oracle.
        let mut want: Vec<u8> = Vec::new();
        for &(_, len, fill) in &order {
            let within = want.len() % NAV_CHUNK_SIZE;
            if within + len > NAV_CHUNK_SIZE {
                want.resize(want.len() + (NAV_CHUNK_SIZE - within), CHUNK_END);
            }
            want.resize(want.len() + len, fill);
        }
        let pool_len = want.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE;
        want.resize(pool_len, CHUNK_END);
        assert_eq!(want.len(), 2 * NAV_CHUNK_SIZE, "the fixture is meant to straddle and to pad its tail");

        let nav = MergedNav {
            index: Vec::new(),
            node_count: 3,
            chunks: Vec::new(),
            chunk_count: 0,
            pool,
            pool_len,
            stats: NavStats::default(),
        };
        let bytes = serialized(&nav, &[], 0, &[&cell]);
        assert_eq!(&bytes[NAV_DIR_LEN..], &want[..], "the streamed pool is not the laid-out pool");
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize, NAV_DIR_LEN, "edge pool offset");
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 2, "edge chunk count");
        assert_eq!(bytes.len(), nav.section_len(&[]), "the section is the length it was projected at");

        // The rule both walks share, stated once: a record moves to the next chunk only when it
        // would cross the boundary, and a record that ends exactly on one does not move.
        assert_eq!(place(300, 300), NAV_CHUNK_SIZE, "a straddling record starts the next chunk");
        assert_eq!(place(300, 212), 300, "…and one that ends exactly on the boundary stays");
        assert_eq!(place(NAV_CHUNK_SIZE, 1), NAV_CHUNK_SIZE, "an aligned cursor never pads");
    }

    /// The layout names its source cells by index, so writing it against a different cell list is a
    /// refusal rather than a plausible-looking file full of the wrong polylines.
    #[test]
    fn writing_the_pool_against_the_wrong_cells_is_refused() {
        let nav = MergedNav {
            index: Vec::new(),
            node_count: 1,
            chunks: Vec::new(),
            chunk_count: 0,
            pool: vec![EdgeRef { cell: 1, off: 0, len: 16 }],
            pool_len: NAV_CHUNK_SIZE,
            stats: NavStats::default(),
        };
        let src = vec![0u8; SRC_POOL_AT + NAV_CHUNK_SIZE];
        let slice = obc_formats::io::SliceSource(&src);
        let cell = pool_cell(&slice, 1);
        let err = serialize(&nav, &[], 0, &[&cell], &mut |_: &[u8]| Ok(())).expect_err("cell 1 was not handed over");
        assert!(format!("{err}").contains("not the cell list"), "got: {err}");
    }

    // ---------------------------------------------------------------------------------------------
    // Whole-merge fixtures. Two of the merge's outcomes are *order*-sensitive — which copy of a
    // duplicated edge survives (§4.6.3) and which adjacency entries the §8.3 degree cap refuses —
    // and neither is reachable from the assembler's oracle fixtures, which are real packed cells
    // with no duplicates and no junction anywhere near degree 24. So they are built here, out of
    // synthetic `network` cells laid out exactly as a packed one is.
    // ---------------------------------------------------------------------------------------------

    /// A longitude on the `2^20` grid: `GRID_ORIGIN + 280 · 2^20`. A node here is on a boundary line
    /// and is therefore eligible for §4.6.2 unification; one a micro-degree away never is.
    const SEAM_LON: i32 = crate::grid::GRID_ORIGIN as i32 + 280 * (1 << 20);

    struct SrcNbr {
        id: u32,
        edge_id: u32,
        cost: u16,
        kind: u8,
        ascent: u16,
    }

    struct SrcNode {
        id: u32,
        lat: i32,
        lon: i32,
        nbrs: Vec<SrcNbr>,
    }

    /// The `Edge Id`s a packed cell would mint for a run of records — §8.4's placement rule, which
    /// is [`place`], applied from a zero cursor.
    fn pool_layout(recs: &[Vec<u8>]) -> Vec<u32> {
        let mut at = 0usize;
        recs.iter()
            .map(|r| {
                at = place(at, r.len());
                let id = at as u32;
                at += r.len();
                id
            })
            .collect()
    }

    /// One §8.4 edge record: `Length M`, `Pt Count`, `Way Kind`, the absolute anchor, then the
    /// chained `int16` deltas. The anchor is endpoint `a`'s coordinate, which is what fixes the
    /// merged edge's orientation.
    fn edge_rec(length_m: u32, kind: u8, anchor: (i32, i32), deltas: &[(i16, i16)]) -> Vec<u8> {
        let mut r = Vec::with_capacity(NAV_EDGE_FIXED_LEN + deltas.len() * 4);
        r.extend_from_slice(&length_m.to_le_bytes());
        r.extend_from_slice(&((deltas.len() + 1) as u16).to_le_bytes());
        r.push(kind);
        r.extend_from_slice(&anchor.0.to_le_bytes());
        r.extend_from_slice(&anchor.1.to_le_bytes());
        for &(dlat, dlon) in deltas {
            r.extend_from_slice(&dlat.to_le_bytes());
            r.extend_from_slice(&dlon.to_le_bytes());
        }
        r
    }

    /// A synthetic `network` cell's §8 section: a node index (never read — [`merge`] walks the chunk
    /// run), the §8.3 node chunks, then the §8.4 edge pool at its own offset.
    fn nav_bytes(nodes: &[SrcNode], recs: &[Vec<u8>]) -> (Vec<u8>, obc_reader::NavDirectory) {
        const INDEX_LEN: usize = 16;
        let coord = |id: u32| {
            let n = nodes.iter().find(|n| n.id == id).expect("a neighbour is a node of the same cell");
            (n.lat, n.lon)
        };
        let mut chunks: Vec<Vec<u8>> = vec![Vec::new()];
        for n in nodes {
            let mut rec = Vec::new();
            rec.extend_from_slice(&n.lat.to_le_bytes());
            rec.extend_from_slice(&n.lon.to_le_bytes());
            rec.extend_from_slice(&n.id.to_le_bytes());
            rec.push(n.nbrs.len() as u8);
            for nb in &n.nbrs {
                let (lat, lon) = coord(nb.id);
                rec.extend_from_slice(&nb.id.to_le_bytes());
                rec.extend_from_slice(&((lat as i64 - n.lat as i64) as i16).to_le_bytes());
                rec.extend_from_slice(&((lon as i64 - n.lon as i64) as i16).to_le_bytes());
                rec.extend_from_slice(&nb.edge_id.to_le_bytes());
                rec.extend_from_slice(&nb.cost.to_le_bytes());
                rec.push(nb.kind);
                rec.extend_from_slice(&nb.ascent.to_le_bytes());
            }
            let open = chunks.last_mut().expect("a chunk is always open");
            if open.len() + rec.len() > NAV_CHUNK_SIZE {
                chunks.push(rec);
            } else {
                open.extend_from_slice(&rec);
            }
        }
        let mut bytes = vec![0u8; INDEX_LEN];
        for c in &chunks {
            let mut c = c.clone();
            c.resize(NAV_CHUNK_SIZE, CHUNK_END);
            bytes.extend_from_slice(&c);
        }
        let pool_at = bytes.len();
        let mut pool: Vec<u8> = Vec::new();
        for (r, &off) in recs.iter().zip(&pool_layout(recs)) {
            pool.resize(off as usize, CHUNK_END);
            pool.extend_from_slice(r);
        }
        pool.resize(pool.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE, CHUNK_END);
        let dir = obc_reader::NavDirectory {
            index_offset: 0,
            node_count: INDEX_LEN / 4,
            chunk_count: chunks.len(),
            edge_pool_offset: pool_at,
            edge_chunk_count: pool.len() / NAV_CHUNK_SIZE,
            chunk_size: NAV_CHUNK_SIZE,
            ..obc_reader::NavDirectory::EMPTY
        };
        bytes.extend_from_slice(&pool);
        (bytes, dir)
    }

    fn nav_cell<'a>(src: &'a dyn obc_formats::io::ByteSource, nav: obc_reader::NavDirectory) -> Cell<'a> {
        Cell {
            id: crate::grid::CellId::parse("20/280/280").expect("a canonical id"),
            band: "network".into(),
            src,
            partial: false,
            lods: Vec::new(),
            pois: obc_reader::PoiDirectory::EMPTY,
            nav,
            profile_table: Vec::new(),
            style_ids: Vec::new(),
            bytes: src.len() as u64,
        }
    }

    /// The merged junction records, read back out of the chunks the merge packed, by dense id:
    /// `(lat, lon, id, [(neighbour id, edge id, cost, way kind, ascent)])`.
    #[allow(clippy::type_complexity)]
    fn merged_nodes(nav: &MergedNav) -> Vec<(i32, i32, u32, Vec<(u32, u32, u16, u8, u16)>)> {
        let mut out = Vec::new();
        for chunk in nav.chunks.chunks_exact(NAV_CHUNK_SIZE) {
            let mut at = 0usize;
            while at + NAV_NODE_FIXED_LEN <= chunk.len() && chunk[at + 12] != CHUNK_END {
                let lat = i32::from_le_bytes(chunk[at..at + 4].try_into().unwrap());
                let lon = i32::from_le_bytes(chunk[at + 4..at + 8].try_into().unwrap());
                let id = u32::from_le_bytes(chunk[at + 8..at + 12].try_into().unwrap());
                let degree = chunk[at + 12] as usize;
                let nbrs = (0..degree)
                    .map(|k| {
                        let e = &chunk[at + NAV_NODE_FIXED_LEN + k * NAV_NEIGHBOR_LEN..][..NAV_NEIGHBOR_LEN];
                        (
                            u32::from_le_bytes(e[0..4].try_into().unwrap()),
                            u32::from_le_bytes(e[8..12].try_into().unwrap()),
                            u16::from_le_bytes(e[12..14].try_into().unwrap()),
                            e[14],
                            u16::from_le_bytes(
                                e[NAV_NEIGHBOR_ASCENT_OFF..NAV_NEIGHBOR_ASCENT_OFF + 2].try_into().unwrap(),
                            ),
                        )
                    })
                    .collect();
                out.push((lat, lon, id, nbrs));
                at += NAV_NODE_FIXED_LEN + degree * NAV_NEIGHBOR_LEN;
            }
        }
        out.sort_by_key(|n| n.2);
        out
    }

    /// **The dedup pin.** One edge running *along* a seam line, written in full by both neighbours —
    /// §4.6.3's case, and the one §3.4(3)'s collinear rule is meant to prevent. The record bytes are
    /// identical in both cells (that is what makes them one edge), but the *ascents* are not, because
    /// those live in the adjacency entry — so which copy survived is visible in the output.
    ///
    /// The survivor must be the **first-collected** one. The dedup now runs after the whole
    /// collection rather than at the door, and the second cell's copy is pushed and does collect its
    /// own cell's write-backs before it is discarded; if the pass ever kept the wrong end of a run,
    /// this reads 33 / 44 instead of 11 / 22 and the bytes are silently different.
    #[test]
    fn a_duplicated_edge_keeps_the_first_cells_copy_and_its_ascents() {
        let (lat_p, lat_q) = (6_000_000i32, 6_001_000i32);
        let rec = edge_rec(1000, 3, (lat_p, SEAM_LON), &[(1000, 0)]);
        let e = pool_layout(std::slice::from_ref(&rec))[0];
        let cell = |ids: (u32, u32), asc: (u16, u16)| {
            let nbr = |id: u32, ascent: u16| SrcNbr { id, edge_id: e, cost: 1000, kind: 3, ascent };
            vec![
                SrcNode { id: ids.0, lat: lat_p, lon: SEAM_LON, nbrs: vec![nbr(ids.1, asc.0)] },
                SrcNode { id: ids.1, lat: lat_q, lon: SEAM_LON, nbrs: vec![nbr(ids.0, asc.1)] },
            ]
        };
        // Different local node ids in the two cells, because §8.2 ids are file-local.
        let (b0, d0) = nav_bytes(&cell((1, 2), (11, 22)), std::slice::from_ref(&rec));
        let (b1, d1) = nav_bytes(&cell((7, 8), (33, 44)), std::slice::from_ref(&rec));
        let (s0, s1) = (obc_formats::io::SliceSource(&b0), obc_formats::io::SliceSource(&b1));
        let (c0, c1) = (nav_cell(&s0, d0), nav_cell(&s1, d1));
        let bbox = (SEAM_LON as i64 - 1000, lat_p as i64 - 1000, SEAM_LON as i64 + 1000, lat_q as i64 + 1000);
        let nav = merged(&[&c0, &c1], 20, 0, bbox);

        assert_eq!(nav.stats.unified, 2, "both endpoints sit on the seam line");
        assert_eq!((nav.stats.nodes, nav.stats.edges), (2, 1), "one edge survives, not two");
        assert_eq!(nav.stats.duplicate_edges, 1, "and the other is counted, not silently absorbed");
        let nodes = merged_nodes(&nav);
        assert_eq!(nodes.len(), 2, "the two unified junctions");
        assert_eq!(nodes[0].3.iter().map(|n| n.4).collect::<Vec<_>>(), vec![11], "a→b is the first cell's climb");
        assert_eq!(nodes[1].3.iter().map(|n| n.4).collect::<Vec<_>>(), vec![22], "and so is b→a");
    }

    /// **The degree-cap pin.** Unification unions adjacency (§4.6.2), so a junction two cells share
    /// can pass §8.3's cap of 24 even though neither cell's own record does: a hub on a seam line
    /// with 13 spokes in each of two cells merges to degree 26.
    ///
    /// The cap does not *choose* 24 entries — it refuses the ones that arrive after the node is
    /// full, which makes it a property of the walk order. Emission order is by renumbered endpoint
    /// pair, so the hub keeps its 24 lowest-numbered spokes and the last two lose their hub-side arc
    /// while keeping their own, which §8.3 explicitly permits. A CSR that reserved slots by counting
    /// degrees and then clamped could keep a different 24.
    #[test]
    fn the_degree_cap_refuses_the_entries_that_arrive_after_the_junction_is_full() {
        const SPOKES: usize = 26;
        let hub_lat = 6_000_000i32;
        // One record per spoke, all anchored at the hub and all distinct — so no two are duplicates
        // of each other and the only thing under test is the cap.
        let recs: Vec<Vec<u8>> = (0..SPOKES)
            .map(|k| edge_rec(100 + k as u32, 3, (hub_lat, SEAM_LON), &[(100 * (k as i16 + 1), 1)]))
            .collect();
        let half = |from: usize, to: usize| {
            let own: Vec<Vec<u8>> = recs[from..to].to_vec();
            let ids = pool_layout(&own);
            let mut nodes = vec![SrcNode { id: 0, lat: hub_lat, lon: SEAM_LON, nbrs: Vec::new() }];
            for (n, k) in (from..to).enumerate() {
                let nbr = |id: u32, ascent: u16| SrcNbr { id, edge_id: ids[n], cost: 100 + k as u16, kind: 3, ascent };
                let spoke = k as u32 + 1;
                nodes[0].nbrs.push(nbr(spoke, 7));
                nodes.push(SrcNode {
                    id: spoke,
                    lat: hub_lat + 100 * (k as i32 + 1),
                    lon: SEAM_LON + 1,
                    nbrs: vec![nbr(0, 9)],
                });
            }
            nav_bytes(&nodes, &own)
        };
        let (b0, d0) = half(0, SPOKES / 2);
        let (b1, d1) = half(SPOKES / 2, SPOKES);
        let (s0, s1) = (obc_formats::io::SliceSource(&b0), obc_formats::io::SliceSource(&b1));
        let (c0, c1) = (nav_cell(&s0, d0), nav_cell(&s1, d1));
        let bbox = (SEAM_LON as i64 - 10, hub_lat as i64 - 10, SEAM_LON as i64 + 10, hub_lat as i64 + 100 * 27);
        let nav = merged(&[&c0, &c1], 20, 0, bbox);

        assert_eq!(nav.stats.unified, 1, "the hub is the only coordinate both cells wrote");
        assert_eq!((nav.stats.nodes, nav.stats.edges), (SPOKES + 1, SPOKES));
        assert_eq!(nav.stats.degree_truncated, SPOKES - NAV_MAX_DEGREE, "two entries past the §8.3 cap");
        let nodes = merged_nodes(&nav);
        // The hub sorts first (lowest latitude), so its spokes are dense ids 1..=26 in walk order.
        let hub = &nodes[0].3;
        assert_eq!(hub.len(), NAV_MAX_DEGREE, "the junction is capped, not overfull");
        assert_eq!(hub.iter().map(|n| n.0).collect::<Vec<_>>(), (1..=NAV_MAX_DEGREE as u32).collect::<Vec<_>>());
        assert!(hub.windows(2).all(|w| w[0].1 < w[1].1), "and its entries are in emission order");
        for (dense, n) in nodes.iter().enumerate().skip(1) {
            assert_eq!(n.3.len(), 1, "every spoke keeps its own arc back to the hub");
            assert_eq!(n.3[0].0, 0, "node {dense}'s neighbour is the hub");
        }
    }
}
