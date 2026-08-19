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
//! # …and neither does the emission (#1116 D4)
//!
//! What was left after D3 was the *output* side: the rebuilt adjacency as one CSR buffer, the node
//! set the quadtree takes **by value**, and the §8.2 index and §8.3 chunks built whole before a
//! shard was planned — 176 + 57 + 175 MiB at a state-sized bake, and gigabytes at DACH. All three
//! are streams now, and [`serialize`] writes the whole §8 section source→sink.
//!
//! * **The adjacency is a stream, not a buffer.** The emission walk explodes each edge into its ≤ 2
//!   directed entries and pushes them into an external sort keyed by `(from, entry order)`, where
//!   the entry order is the position the emission walk wrote them at. §8.3's degree cap is
//!   *order*-sensitive — it refuses the entries that arrive after a junction is full, which is a
//!   property of the walk — so reproducing the walk position is the whole requirement, and it is why
//!   the key carries the counter explicitly instead of leaning on the sort's stability. A merge walk
//!   of the dense node stream against that sorted stream then yields each junction's §8.3 record,
//!   with the cap biting at exactly the entry it bit at before.
//! * **The quadtree is built in passes.** Its *shape* is a pure function of the coordinates, the
//!   record lengths, the 512-byte capacity and the recursion floor, so [`crate::qtree`]'s
//!   `flatten_streaming` recovers it from the same records sorted in **tree order** — a different
//!   key than the dense one, so one more external sort — and hands back the index, the bin packing
//!   and a placement plan. Nothing holds a point.
//! * **The section is written from the seam.** `MergedNav` is directories, counts and five scratch
//!   ids; its size projection is still arithmetic over them, so a shard is still planned before a
//!   byte is written.
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
    nav_edge_id, nav_edge_id_chunk, nav_edge_id_ordinal, nav_edge_record_range, nav_index_padding, CHUNK_END,
    NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN, NAV_EDGE_MAX_CHUNKS, NAV_EDGE_MAX_RECORDS_PER_CHUNK,
    NAV_MAX_DEGREE, NAV_NEIGHBOR_ASCENT_OFF, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_SNAP_ANCHOR_GAP_M,
    NAV_SNAP_EDGE_MIN_M, NAV_SNAP_RECORD_LEN,
};
use obc_map_scene::ground_dist_m;

use crate::emit::{scaled, MapWriter, SCALE};
use crate::extsort::{ExternalSort, SpillReader, SpillWriter};
use crate::grid::{on_grid_boundary, UBox};
use crate::input::Cell;
use crate::qtree;
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
    /// The record's **byte** offset inside that cell's edge pool.
    ///
    /// Since v14 that is no longer the cell's `Edge Id`: an id is a `(chunk, ordinal)` pair, and
    /// resolving it means walking the chunk it names. The walk happens once, while the cell's pool
    /// is the buffer in hand, and what survives is the address — so [`serialize`] still reads the
    /// record with one seek and the resolve never runs twice.
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

/// The renumbered junctions as a stream: `lat i32, lon i32`, in dense order, so a record's
/// **position is its dense id** — the same trick the collection stream plays with the collection id.
const DENSE_NODE: usize = 8;

/// The §4.6.6 pool plan on the seam: one [`EdgeRef`] per kept edge, in emission order —
/// `cell u32, off u32, len u16`.
const POOL_REC: usize = 10;

fn pool_record(r: &EdgeRef) -> [u8; POOL_REC] {
    let mut out = [0u8; POOL_REC];
    out[0..4].copy_from_slice(&r.cell.to_le_bytes());
    out[4..8].copy_from_slice(&r.off.to_le_bytes());
    out[8..10].copy_from_slice(&r.len.to_le_bytes());
    out
}

fn pool_ref(r: &[u8; POOL_REC]) -> EdgeRef {
    EdgeRef {
        cell: u32::from_le_bytes(r[0..4].try_into().expect("4 bytes")),
        off: u32::from_le_bytes(r[4..8].try_into().expect("4 bytes")),
        len: u16::from_le_bytes(r[8..10].try_into().expect("2 bytes")),
    }
}

/// One **directed** adjacency entry, as the emission walk wrote it: `from u32, seq u32, to u32,
/// lat i32, lon i32, edge id u32, cost u16, ascent u16, kind u8`.
///
/// `seq` is the entry's position in the emission walk, and it is in the record rather than implied
/// because §8.3's degree cap is a property of *that* order: the cap does not choose 24 arcs, it
/// refuses the ones that arrive after the junction is full. `(from, seq)` is therefore both the sort
/// key and a total order, and the merge walk it feeds refuses exactly what the CSR fill refused.
///
/// The neighbour's coordinate rides along because §8.3 stores it as an `int16` delta from the
/// junction's own, and looking it up would be the whole-map node array this pass exists to remove.
///
/// `seq` is a `u32` by construction rather than by hope: step 6 refuses a pool past 4 GiB, an §8.4
/// edge record is at least [`NAV_EDGE_FIXED_LEN`] bytes, and the walk writes at most two entries per
/// edge — so `seq` cannot pass `2 × 2^32 / 15 ≈ 573 M`, a seventh of what a `u32` holds.
const ADJ_REC: usize = 29;

#[allow(clippy::too_many_arguments)]
fn adj_record(
    from: u32,
    seq: u32,
    to: u32,
    (lat, lon): (i32, i32),
    edge_id: u32,
    cost: u16,
    ascent: u16,
    kind: u8,
) -> [u8; ADJ_REC] {
    let mut r = [0u8; ADJ_REC];
    r[0..4].copy_from_slice(&from.to_le_bytes());
    r[4..8].copy_from_slice(&seq.to_le_bytes());
    r[8..12].copy_from_slice(&to.to_le_bytes());
    r[12..16].copy_from_slice(&lat.to_le_bytes());
    r[16..20].copy_from_slice(&lon.to_le_bytes());
    r[20..24].copy_from_slice(&edge_id.to_le_bytes());
    r[24..26].copy_from_slice(&cost.to_le_bytes());
    r[26..28].copy_from_slice(&ascent.to_le_bytes());
    r[28] = kind;
    r
}

fn adj_from(r: &[u8; ADJ_REC]) -> u32 {
    u32::from_le_bytes(r[0..4].try_into().expect("4 bytes"))
}

fn adj_seq(r: &[u8; ADJ_REC]) -> u32 {
    u32::from_le_bytes(r[4..8].try_into().expect("4 bytes"))
}

fn adj_coord(r: &[u8; ADJ_REC]) -> (i32, i32) {
    (
        i32::from_le_bytes(r[12..16].try_into().expect("4 bytes")),
        i32::from_le_bytes(r[16..20].try_into().expect("4 bytes")),
    )
}

/// `(from, seq)` — the emission walk's own order, restored.
fn by_adjacency(a: &[u8; ADJ_REC], b: &[u8; ADJ_REC]) -> Ordering {
    (adj_from(a), adj_seq(a)).cmp(&(adj_from(b), adj_seq(b)))
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
    /// Interior lookup anchors written for exact start/end edge snapping.
    pub snap_anchors: usize,
    /// Lookup anchors refused by the quadtree chunk-capacity guard.
    pub dropped_snap_anchors: usize,
}

/// One laid-out quadtree whose fixed-width records live on the scratch seam.
struct SnapFiles {
    index: ScratchId,
    places: ScratchId,
    points: ScratchId,
    recs: ScratchId,
}

/// The graph's scratch streams: everything §8 needs, none of it resident.
struct NavFiles {
    /// The §8.2 index, already in wire form — `Node Count` little-endian `uint32`s.
    index: ScratchId,
    /// One `qtree::PLACE_REC` per non-empty leaf, in chunk-emission order.
    places: ScratchId,
    /// The junctions in tree order, which is what a leaf's run of the placement plan names.
    points: ScratchId,
    /// Every junction's packed §8.3 record, in dense order, addressed by the tree records.
    recs: ScratchId,
    /// Every kept edge's [`EdgeRef`], in emission order.
    pool: ScratchId,
    /// v13's sparse interior edge-lookup anchors; absent when every edge is short.
    snap: Option<SnapFiles>,
}

/// The merged graph, **already laid out** and living on the [scratch seam](crate::scratch): the node
/// quadtree's index and its bin-packed chunks as a plan plus the packed records, and the edge pool
/// as a plan rather than a buffer — every record's source address in emission order. What is in
/// memory is the directory's counts, which is what lets a shard's size be known before its header is
/// written.
///
/// The streams outlive the merge because the write does not happen until the set has been planned;
/// [`MergedNav::release`] is what gives them back.
pub struct MergedNav {
    /// `None` for the legal empty section every non-core shard carries (§5.1).
    files: Option<NavFiles>,
    /// Tree nodes in the §8.2 index — [`crate::qtree::Flattened::node_count`], not the junction
    /// count.
    node_count: u32,
    chunk_count: u32,
    index_len: u64,
    /// What the pool comes to once §8.4's no-straddle padding and the chunk-aligned tail are
    /// applied — the one number the directory and the shard projection need from it. `u64` for the
    /// same reason the projection chain is: on wasm32 a `usize` here wraps before the §5.7 ceiling
    /// ever sees the number.
    pool_len: u64,
    snap_node_count: u32,
    snap_chunk_count: u32,
    snap_index_len: u64,
    /// What a read buffer in [`serialize`] may hold; the merge's own share of its budget.
    read_budget: usize,
    pub stats: NavStats,
}

/// Exact §8 section sizing before a shard is written: the section's **unpadded lengths**, from which
/// [`NavLayout`] derives everything else. Alignment depends on the section's absolute file offset,
/// so this answers once the shard layout has placed the section — which is what preserves the
/// assembler's "project exactly, then stream" invariant.
#[derive(Clone, Copy)]
pub struct NavProjection {
    profile_len: u64,
    index_len: u64,
    chunk_bytes: u64,
    pool_len: u64,
    snap_index_len: u64,
    snap_chunk_bytes: u64,
    populated: bool,
    snap_populated: bool,
}

/// Where §8's regions and gaps fall inside its section, **relative to the section's own start**.
///
/// **This is the section's layout, and there is one of it.** The projection that sizes a map before
/// the write and the write itself both read their numbers out of here, so §8.1's alignment
/// arithmetic — which of two boundaries a run reconciles, and how long each gap is — exists once
/// rather than once per direction. What [`serialize`] does with it is put bytes and pad by the named
/// amounts; it reasons about no boundary of its own.
///
/// Relative rather than absolute because [`NavProjection::bytes_at`] is asked about deliberately
/// absurd positions (a planner probing the ceiling), and only the section's position **inside a
/// 512-byte sector** changes any answer here — both `nav_index_padding` and `align_up` divide 512.
/// So the walk runs at the section's sector remainder and cannot overflow whatever it is asked.
#[derive(Clone, Copy, Default)]
struct NavLayout {
    /// §1.2 filler between the 40-byte directory and the profile table it cannot otherwise name.
    dir_gap: u64,
    profile_table: u64,
    /// §8.1's alignment run: the index on a unit boundary, its chunks on a sector.
    index_pad: u64,
    index: u64,
    /// …and the rounding step between the index and the chunks behind it.
    index_gap: u64,
    edge_pool: u64,
    snap_index_pad: u64,
    snap_index: u64,
    snap_gap: u64,
    /// The section's total length.
    end: u64,
}

impl NavProjection {
    pub fn bytes_at(self, section_offset: u64) -> u64 {
        self.layout_at(section_offset).end
    }

    /// Walk the section's shape with a cursor that writes nothing — the projection, computed by the
    /// arithmetic that emits rather than by a second copy of it.
    fn layout_at(self, section_offset: u64) -> NavLayout {
        let start = section_offset % NAV_CHUNK_SIZE as u64;
        crate::emit::place(start, |w| {
            w.pad(NAV_DIR_LEN as u64)?;
            let profile_table = w.begin_section()?;
            let dir_gap = profile_table - (start + NAV_DIR_LEN as u64);
            w.pad(self.profile_len)?;
            if !self.populated {
                // An empty graph is the directory, the always-present profile table, and the filler
                // that puts the first unit boundary past it — where all three zero-length regions
                // point, because a zero-length region still has to be nameable.
                let end = w.begin_section()?;
                let at = end - start;
                return Ok(NavLayout {
                    dir_gap,
                    profile_table: profile_table - start,
                    index: at,
                    edge_pool: at,
                    snap_index: at,
                    end: at,
                    ..NavLayout::default()
                });
            }
            let index_pad = index_run(w.at(), self.index_len);
            w.pad(index_pad)?;
            let index = w.at();
            w.pad(self.index_len)?;
            let after_index = w.at();
            let index_gap = w.begin_section()? - after_index;
            w.pad(self.chunk_bytes)?;
            let edge_pool = w.at();
            w.pad(self.pool_len)?;
            // A snap index of no nodes has no sector to reconcile: its region only has to be
            // nameable. In practice the pool leaves the cursor on a sector and this run is empty.
            let before_snap = w.at();
            let snap_index_pad = if self.snap_populated {
                let run = index_run(before_snap, self.snap_index_len);
                w.pad(run)?;
                run
            } else {
                w.begin_section()? - before_snap
            };
            let snap_index = w.at();
            w.pad(self.snap_index_len)?;
            let after_snap_index = w.at();
            let snap_gap = w.begin_section()? - after_snap_index;
            w.pad(self.snap_chunk_bytes)?;
            Ok(NavLayout {
                dir_gap,
                profile_table: profile_table - start,
                index_pad,
                index: index - start,
                index_gap,
                edge_pool: edge_pool - start,
                snap_index_pad,
                snap_index: snap_index - start,
                snap_gap,
                end: w.at() - start,
            })
        })
        .expect("a walk that writes nothing cannot fail")
    }
}

/// §8.1's alignment run before a quadtree index of `index_len` bytes starting, unpadded, at `at`.
///
/// It reconciles two alignments at once — the index on a **unit** boundary (or no scaled offset
/// could name it) and the fixed 512-byte chunks behind it on a **sector**, so a full-chunk read is
/// one card command. The rounding step in `align_up(index_offset × U + node_count × 4, U)` is the
/// slack that lets both hold for every node count.
#[inline]
fn index_run(at: u64, index_len: u64) -> u64 {
    nav_index_padding(SCALE, at, index_len).expect("a nav index length never approaches u64::MAX") as u64
}

impl MergedNav {
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// The section's unpadded lengths, from which [`NavProjection::bytes_at`] derives its exact
    /// size and [`serialize`] its every gap. Every data chunk is padded to `NAV_CHUNK_SIZE`; the
    /// §1.2 filler and §8.1's alignment runs are the layout's business, not this one's.
    pub fn projection(&self, profile_table: &[u8]) -> NavProjection {
        NavProjection {
            profile_len: profile_table.len() as u64,
            index_len: self.index_len,
            chunk_bytes: self.chunk_count as u64 * NAV_CHUNK_SIZE as u64,
            pool_len: self.pool_len,
            snap_index_len: self.snap_index_len,
            snap_chunk_bytes: self.snap_chunk_count as u64 * NAV_CHUNK_SIZE as u64,
            populated: self.files.is_some(),
            snap_populated: self.snap_node_count > 0,
        }
    }

    /// Give the scratch streams back, once the last shard that could name them has been written.
    ///
    /// Best-effort like every other scratch delete (see [`crate::scratch`]): the bytes are already
    /// unreachable, and a host that fails to reclaim them is about to drop its whole scratch area.
    pub fn release(&self, scratch: &dyn ScratchStore) {
        if let Some(f) = &self.files {
            for id in [f.index, f.places, f.points, f.recs, f.pool] {
                let _ = scratch.remove(id);
            }
            if let Some(snap) = &f.snap {
                for id in [snap.index, snap.places, snap.points, snap.recs] {
                    let _ = scratch.remove(id);
                }
            }
        }
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
            let chunk = cell.read(data_start + (k * dir.chunk_size) as u64, dir.chunk_size)?;
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
                let (rec_at, rec) = edge_record(&pool, edge_id, cell)?;
                // The record's anchor is endpoint `a`'s coordinate, so a record whose anchor is not
                // this node's coordinate belongs to the other direction — keep the orientation the
                // record itself states.
                let anchor_lat = i32::from_le_bytes(rec[7..11].try_into().expect("4 bytes"));
                let anchor_lon = i32::from_le_bytes(rec[11..15].try_into().expect("4 bytes"));
                let own_is_anchor = (anchor_lat, anchor_lon) == (lat, lon);
                let (a, b) = if own_is_anchor { (a, b) } else { (b, a) };
                let hash = fnv(rec);
                let rec = EdgeRef { cell: ci, off: rec_at, len: rec.len() as u16 };
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
    let (nodes_file, node_count, dense_by_id) =
        renumber(scratch, budget, share, node_file, pruned.node_comp, &pruned.keep, &deltas)?;
    drop(deltas);
    stats.nodes = node_count as usize;

    // --- 5 (cont.). Attach the dense ids, by merge join rather than by lookup: sort what survived
    // by `a`, walk it beside the id-ordered dense stream, then do the same for `b`. The endpoint
    // coordinates come across with the ids, because §8.3's neighbour deltas are measured from them
    // and re-reading them later would be exactly the whole-map array this pass is removing.
    //
    // Each join hands its result straight into the **next** pass's sort rather than through a file:
    // a sort that is generating runs is holding at most half its budget, and one that is merging is
    // holding all of it, so two of them at half a budget each are one budget — and the stream
    // between them is never written down at all. ---
    let by_b = join_first(scratch, budget, share, edge_file, &pruned, &dead, dense_by_id)?;
    drop(pruned);
    drop(dead);
    let by_emit = join_second(scratch, budget, share, by_b, dense_by_id, &mut stats)?;
    scratch.remove(dense_by_id)?;

    // --- 6/7. Lay the edge pool out and explode the adjacency, in **one** walk of the emission
    // order (§4.6.6).
    //
    // `Edge Id` is a pool byte offset, so every record lands at a new place and the no-straddle rule
    // is re-applied at the 512-byte granularity — but that is arithmetic over the records' *lengths*
    // (see [`serialize`], which walks the same rule to emit the padding), so the cursor below mints
    // the id of the record it is about to place, and the two adjacency entries that quote that id
    // are written on the spot.
    //
    // What they are written *into* is a sort, not a CSR buffer. The buffer needed the uncapped
    // degrees reserved in advance and 24 bytes per entry resident until the quadtree had read the
    // last one — 176 MiB at a state bake, and the second-largest thing left in the merge. The
    // entries carry the walk position instead, and `emit_nodes` puts them back in it. §8.3's degree
    // cap is order-sensitive — it does not "keep 24 of the arcs", it refuses the ones that arrive
    // after a node is already full — so reproducing that order is the whole requirement. ---
    let mut pool_out = SpillWriter::<POOL_REC>::create(scratch, share)?;
    let mut adj = ExternalSort::<ADJ_REC>::new(scratch, budget / 2, by_adjacency);
    let mut snap_recs = ByteSpill::create(scratch, share)?;
    let mut snap_points = SpillWriter::<{ qtree::TREE_REC }>::create(scratch, share)?;
    let mut snap_ord = 0u32;
    let mut source_edge = [0u8; NAV_CHUNK_SIZE];
    let mut at = 0u64;
    let mut chunk_index = 0u64;
    let mut ordinal = 0u32;
    let mut seq: u32 = 0;
    for rec in by_emit.finish()? {
        let rec = rec?;
        let r = dense_ref(&rec);
        let len = r.len as u64;
        let start = place(at, len);
        // §8.4's `(chunk, ordinal)` id, minted at the moment the record is placed. The no-straddle
        // rule the layout already obeyed carries a second weight since v14: it is what makes "the
        // *n*th record of a chunk" a well-defined thing to name, so the ordinal is simply the
        // per-chunk record counter and nothing has to know a byte position any more.
        let here = start / NAV_CHUNK_SIZE as u64;
        if here != chunk_index {
            chunk_index = here;
            ordinal = 0;
        }
        at = start;
        // The two halves of §8.4's field, each refused rather than wrapped. The 19-byte minimum
        // record puts the real per-chunk maximum at 26, so the ordinal cap never binds today; it is
        // checked so that it stays true if a future record shrinks, and because it is what keeps
        // `0xFFFFFFFF` an impossible id *unconditionally*.
        if chunk_index >= NAV_EDGE_MAX_CHUNKS {
            return Err(Error::Capacity(format!(
                "the merged edge pool needs more than the {NAV_EDGE_MAX_CHUNKS} chunks an `Edge Id`'s 27-bit chunk \
                 field can name (OBCM §8.4) — reduce the coverage"
            )));
        }
        if ordinal as usize >= NAV_EDGE_MAX_RECORDS_PER_CHUNK {
            return Err(Error::Format(format!(
                "an edge chunk would hold more than the {NAV_EDGE_MAX_RECORDS_PER_CHUNK} records §8.4 permits"
            )));
        }
        let edge_id = nav_edge_id(chunk_index as u32, ordinal).expect("both halves checked against their fields");
        ordinal += 1;
        pool_out.push(pool_record(&r))?;
        at += len;

        // The final pool id is now known. Decode this one bounded source record and add lookup-only
        // anchors before moving on; no edge geometry survives the iteration.
        let cell = cells
            .get(r.cell as usize)
            .ok_or_else(|| Error::Format(format!("a merged edge names missing source cell {}", r.cell)))?;
        let source = &mut source_edge[..r.len as usize];
        cell.read_into(cell.nav.edge_pool_offset + u64::from(r.off), source)?;
        stats.snap_anchors +=
            append_snap_anchors(source, edge_id, global_bbox, &mut snap_recs, &mut snap_points, &mut snap_ord)?;

        let (a, b) = (dense_id(&rec, A_AT), dense_id(&rec, B_AT));
        let (a_at, b_at) = (dense_coord(&rec, A_AT), dense_coord(&rec, B_AT));
        // §8.3 stores the cost as a `uint16`, so the entry is the width of the field it becomes.
        let cost = dense_cost(&rec).min(u16::MAX as u32) as u16;
        let kind = dense_kind(&rec);
        let (ascent_ab, ascent_ba) = dense_ascents(&rec);
        adj.push(adj_record(a, seq, b, b_at, edge_id, cost, ascent_ab, kind))?;
        seq += 1;
        if a != b {
            // A self-loop appears once (§8.3), exactly as the CSR fill wrote it once.
            adj.push(adj_record(b, seq, a, a_at, edge_id, cost, ascent_ba, kind))?;
            seq += 1;
        }
    }
    // The tail pads to a whole chunk, because §8.1's `Edge Chunk Count` measures the pool in chunks.
    let pool_len = at.div_ceil(NAV_CHUNK_SIZE as u64) * NAV_CHUNK_SIZE as u64;
    let (pool, pool_count) = pool_out.seal()?;
    debug_assert_eq!(pool_count as usize, stats.edges, "one pool entry per kept edge");

    // Anchor records are already packed; sort only their tiny tree references, then run the same
    // streaming quadtree/bin-packing implementation as the junction index.
    let snap_recs = snap_recs.seal()?;
    let (snap_points, unsorted_snap_count) = snap_points.seal()?;
    let mut snap_sort = ExternalSort::<{ qtree::TREE_REC }>::new(scratch, budget / 2, qtree::by_tree_order);
    for rec in SpillReader::<{ qtree::TREE_REC }>::open(scratch, snap_points, share)? {
        snap_sort.push(rec?)?;
    }
    scratch.remove(snap_points)?;
    let mut sorted_snap = SpillWriter::<{ qtree::TREE_REC }>::create(scratch, share)?;
    for rec in snap_sort.finish()? {
        sorted_snap.push(rec?)?;
    }
    let (sorted_snap, snap_count) = sorted_snap.seal()?;
    debug_assert_eq!(snap_count, unsorted_snap_count);
    debug_assert_eq!(snap_count as usize, stats.snap_anchors);
    let snap_flat = if snap_count == 0 {
        scratch.remove(snap_recs)?;
        scratch.remove(sorted_snap)?;
        None
    } else {
        let flat = qtree::flatten_streaming(scratch, budget, sorted_snap, global_bbox, NAV_CHUNK_SIZE, NAV_CHUNK_SIZE)?;
        stats.dropped_snap_anchors = flat.dropped;
        Some((snap_recs, flat))
    };

    // --- 7 (cont.). The junction records, and the node quadtree over the **assembly** bbox with
    // §8.2's bin-packed 512-byte chunks — both in passes, and both laid out here so a shard's size
    // is known before its header is written. ---
    let (recs, flat) = emit_nodes(scratch, budget, share, nodes_file, node_count, adj, global_bbox, &mut stats)?;
    let (snap, snap_node_count, snap_chunk_count, snap_index_len) = match snap_flat {
        Some((recs, flat)) => {
            let node_count = flat.node_count;
            let chunk_count = flat.chunk_count;
            let index_len = node_count as u64 * 4;
            (
                Some(SnapFiles { index: flat.index, places: flat.places, points: flat.points, recs }),
                node_count,
                chunk_count,
                index_len,
            )
        }
        None => (None, 0, 0, 0),
    };
    Ok(MergedNav {
        files: Some(NavFiles { index: flat.index, places: flat.places, points: flat.points, recs, pool, snap }),
        node_count: flat.node_count,
        chunk_count: flat.chunk_count,
        index_len: flat.node_count as u64 * 4,
        pool_len,
        snap_node_count,
        snap_chunk_count,
        snap_index_len,
        read_budget: share,
        stats,
    })
}

/// The §8.3 junction records and the tree over them, as a merge walk and two sorted passes.
///
/// One forward walk of the dense node stream against the adjacency sorted by `(from, seq)` produces
/// every junction's record — the cap applied at the entry the emission walk applied it at, the
/// `int16` neighbour deltas re-checked on the entries that survive it — and appends it to a byte
/// stream in dense order. What goes into the tree's sort is eighteen bytes per junction: where the
/// record is, how long it is, its tree key and its dense id.
///
/// Nothing here is sized by the graph. The record buffer is one junction's — 421 bytes at §8.3's
/// degree cap — and the two sorts are bounded by the caller's budget.
#[allow(clippy::too_many_arguments)]
fn emit_nodes(
    scratch: &dyn ScratchStore,
    budget: usize,
    share: usize,
    nodes_file: ScratchId,
    node_count: u32,
    adj: ExternalSort<'_, ADJ_REC>,
    global_bbox: UBox,
    stats: &mut NavStats,
) -> Result<(ScratchId, qtree::Flattened)> {
    let mut recs = ByteSpill::create(scratch, share)?;
    let mut points = ExternalSort::<{ qtree::TREE_REC }>::new(scratch, budget / 2, qtree::by_tree_order);
    {
        let mut entries = adj.finish()?;
        let mut head = entries.next().transpose()?;
        let mut buf: Vec<u8> = Vec::with_capacity(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN);
        for (dense, rec) in SpillReader::<DENSE_NODE>::open(scratch, nodes_file, share)?.enumerate() {
            let rec = rec?;
            let dense = dense as u32;
            let lat = i32::from_le_bytes(rec[0..4].try_into().expect("4 bytes"));
            let lon = i32::from_le_bytes(rec[4..8].try_into().expect("4 bytes"));
            buf.clear();
            buf.extend_from_slice(&lat.to_le_bytes());
            buf.extend_from_slice(&lon.to_le_bytes());
            buf.extend_from_slice(&dense.to_le_bytes());
            buf.push(0); // degree, once the cap has had its say
            let mut degree = 0usize;
            while let Some(e) = head.filter(|e| adj_from(e) == dense) {
                head = entries.next().transpose()?;
                // The entries are in the emission walk's own order, so the ones past the cap are the
                // same ones the CSR fill refused.
                if degree >= NAV_MAX_DEGREE {
                    stats.degree_truncated += 1;
                    continue;
                }
                let (nlat, nlon) = adj_coord(&e);
                let (dlat, dlon) = (nlat as i64 - lat as i64, nlon as i64 - lon as i64);
                if dlat.abs() > MAX_NEIGHBOR_DELTA || dlon.abs() > MAX_NEIGHBOR_DELTA {
                    return Err(Error::Format(format!(
                        "a merged adjacency spans ({dlat}, {dlon}) µdeg, past the §8.3 int16 neighbour delta"
                    )));
                }
                buf.extend_from_slice(&e[8..12]); // neighbour id
                buf.extend_from_slice(&(dlat as i16).to_le_bytes());
                buf.extend_from_slice(&(dlon as i16).to_le_bytes());
                buf.extend_from_slice(&e[20..26]); // edge id, cost
                buf.push(e[28]); // way kind
                buf.extend_from_slice(&e[26..28]); // ascent
                degree += 1;
            }
            buf[12] = degree as u8;
            debug_assert_eq!(buf.len(), NAV_NODE_FIXED_LEN + degree * NAV_NEIGHBOR_LEN);
            let at = recs.push(&buf)?;
            points.push(qtree::tree_record(qtree::tree_key(lat, lon, global_bbox), dense, at, buf.len() as u16))?;
        }
        if let Some(e) = head {
            return Err(Error::Format(format!(
                "an adjacency entry names junction {}, past the {node_count} the renumbering handed out",
                adj_from(&e)
            )));
        }
    }
    scratch.remove(nodes_file)?;
    let recs = recs.seal()?;

    // The tree wants the same records in tree order, and every leaf is a *range* of that order — so
    // it is written down rather than consumed: `flatten_streaming` reads it forward once and then
    // one leaf at a time.
    let mut sorted = SpillWriter::<{ qtree::TREE_REC }>::create(scratch, share)?;
    for rec in points.finish()? {
        sorted.push(rec?)?;
    }
    let (sorted, count) = sorted.seal()?;
    debug_assert_eq!(count, node_count as u64, "one tree record per renumbered junction");
    let flat = qtree::flatten_streaming(scratch, budget, sorted, global_bbox, NAV_CHUNK_SIZE, NAV_CHUNK_SIZE)?;
    stats.dropped_nodes = flat.dropped;
    Ok((recs, flat))
}

/// A stream of **variable-length** records on the scratch seam: append bytes, get back the offset
/// they landed at.
///
/// [`crate::extsort`] is deliberately fixed-width — every sort key in the merge is one — and this is
/// the one producer that is not: a §8.3 junction record is 13 bytes plus 17 per neighbour, and
/// padding three million of them to the 421-byte maximum would cost seven times what they are. So it
/// lives here rather than there, and adds nothing to that module's contract.
struct ByteSpill<'s> {
    scratch: &'s dyn ScratchStore,
    id: ScratchId,
    buf: Vec<u8>,
    cap: usize,
    at: u64,
}

impl<'s> ByteSpill<'s> {
    fn create(scratch: &'s dyn ScratchStore, budget: usize) -> Result<ByteSpill<'s>> {
        Ok(ByteSpill { scratch, id: scratch.create()?, buf: Vec::new(), cap: budget.max(NAV_CHUNK_SIZE), at: 0 })
    }

    /// Append one record; the offset it starts at is how it is found again.
    fn push(&mut self, rec: &[u8]) -> Result<u32> {
        let at = u32::try_from(self.at).map_err(|_| {
            Error::Capacity(
                "the merged quadtree record bytes pass 4 GiB, which no OBCM section can hold (OBCA §5.7)".into(),
            )
        })?;
        if self.buf.len() + rec.len() > self.cap {
            self.flush()?;
        }
        self.buf.extend_from_slice(rec);
        self.at += rec.len() as u64;
        Ok(at)
    }

    fn flush(&mut self) -> Result<()> {
        if !self.buf.is_empty() {
            self.scratch.append(self.id, &self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    fn seal(mut self) -> Result<ScratchId> {
        self.flush()?;
        self.buf = Vec::new();
        Ok(self.id)
    }
}

/// Decode one final §8.4 edge record and spill evenly spaced lookup-only anchors for it. The edge
/// record is at most one 512-byte chunk, so the temporary polyline is strictly bounded even though
/// the whole graph remains streaming.
fn append_snap_anchors(
    record: &[u8],
    edge_id: u32,
    global_bbox: UBox,
    recs: &mut ByteSpill<'_>,
    points: &mut SpillWriter<'_, { qtree::TREE_REC }>,
    ord: &mut u32,
) -> Result<usize> {
    if record.len() < NAV_EDGE_FIXED_LEN {
        return Err(Error::Format("a merged edge record is shorter than the §8.4 fixed header".into()));
    }
    let point_count = u16::from_le_bytes(record[4..6].try_into().expect("2 bytes")) as usize;
    let expected = NAV_EDGE_FIXED_LEN + point_count.saturating_sub(1) * 4;
    if point_count < 2 || expected != record.len() {
        return Err(Error::Format(format!(
            "a merged edge record declares {point_count} points but occupies {} byte(s)",
            record.len()
        )));
    }

    let mut polyline: Vec<(i32, i32)> = Vec::with_capacity(point_count);
    let mut lat = i32::from_le_bytes(record[7..11].try_into().expect("4 bytes"));
    let mut lon = i32::from_le_bytes(record[11..15].try_into().expect("4 bytes"));
    polyline.push((lon, lat));
    let mut at = NAV_EDGE_FIXED_LEN;
    for _ in 1..point_count {
        lat += i16::from_le_bytes(record[at..at + 2].try_into().expect("2 bytes")) as i32;
        lon += i16::from_le_bytes(record[at + 2..at + 4].try_into().expect("2 bytes")) as i32;
        polyline.push((lon, lat));
        at += 4;
    }
    let lengths: Vec<f32> = polyline.windows(2).map(|w| ground_dist_m(w[0], w[1])).collect();
    let length: f32 = lengths.iter().sum();
    if length <= NAV_SNAP_EDGE_MIN_M as f32 {
        return Ok(0);
    }

    let intervals = (length / NAV_SNAP_ANCHOR_GAP_M as f32).ceil() as usize;
    let mut segment = 0usize;
    let mut before = 0.0f32;
    for i in 1..intervals {
        let target = length * i as f32 / intervals as f32;
        while segment + 1 < lengths.len() && before + lengths[segment] < target {
            before += lengths[segment];
            segment += 1;
        }
        let a = polyline[segment];
        let b = polyline[segment + 1];
        let t = ((target - before) / lengths[segment].max(f32::EPSILON)).clamp(0.0, 1.0);
        // Preserve microdegree precision by interpolating only the small segment delta. Casting
        // the absolute coordinate to f32 first would quantize latitude by several microdegrees.
        let anchor_lon = a.0.saturating_add(((b.0 - a.0) as f32 * t).round() as i32);
        let anchor_lat = a.1.saturating_add(((b.1 - a.1) as f32 * t).round() as i32);
        let mut packed = [0u8; NAV_SNAP_RECORD_LEN];
        packed[0..4].copy_from_slice(&anchor_lat.to_le_bytes());
        packed[4..8].copy_from_slice(&anchor_lon.to_le_bytes());
        packed[8..12].copy_from_slice(&edge_id.to_le_bytes());
        let rec_at = recs.push(&packed)?;
        points.push(qtree::tree_record(
            qtree::tree_key(anchor_lat, anchor_lon, global_bbox),
            *ord,
            rec_at,
            NAV_SNAP_RECORD_LEN as u16,
        ))?;
        *ord = ord.checked_add(1).ok_or_else(|| {
            Error::Capacity("more than 4 G snap anchors: the quadtree input order is a uint32".into())
        })?;
    }
    Ok(intervals - 1)
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
    // Both streams are spent, and the runs the sort is about to merge are the same bytes again —
    // on a host whose scratch is its own linear memory that difference is the whole peak, so they
    // go before the merge starts rather than when the merge returns.
    scratch.remove(edge_file)?;
    scratch.remove(pruned.edge_comp)?;

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

/// The second join: endpoint `b`. What it returns is the emission sort, already loaded — see
/// [`merge`] for why the streams between the joins are never written down.
fn join_second<'s>(
    scratch: &'s dyn ScratchStore,
    budget: usize,
    share: usize,
    sort: ExternalSort<'s, DENSE_REC>,
    dense_by_id: ScratchId,
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
///
/// The junctions themselves are no longer a `Vec` either (#1116 D4): they go out as a
/// [`DENSE_NODE`] stream whose *position is the dense id*, which is all the emission walk and the
/// quadtree ever asked of the array.
fn renumber(
    scratch: &dyn ScratchStore,
    budget: usize,
    share: usize,
    node_file: ScratchId,
    node_comp: ScratchId,
    keep: &[bool],
    deltas: &[(u32, u64)],
) -> Result<(ScratchId, u32, ScratchId)> {
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

    let mut nodes = SpillWriter::<DENSE_NODE>::create(scratch, share)?;
    let mut count = 0u32;
    let mut by_id = ExternalSort::<JOIN_REC>::new(scratch, (budget / 2).max(JOIN_REC), by_join_id);
    for rec in sort.finish()? {
        let (lat, lon, _, id) = node_key(&rec?);
        let dense = count;
        count = count.checked_add(1).ok_or_else(|| {
            Error::Capacity("the merged graph passes 4 G junctions, which §8.2's uint32 `Node Id` cannot name".into())
        })?;
        let mut join = [0u8; JOIN_REC];
        join[0..4].copy_from_slice(&id.to_le_bytes());
        join[4..8].copy_from_slice(&dense.to_le_bytes());
        join[8..12].copy_from_slice(&lat.to_le_bytes());
        join[12..16].copy_from_slice(&lon.to_le_bytes());
        by_id.push(join)?;
        let mut node = [0u8; DENSE_NODE];
        node[0..4].copy_from_slice(&lat.to_le_bytes());
        node[4..8].copy_from_slice(&lon.to_le_bytes());
        nodes.push(node)?;
    }
    let (nodes, _) = nodes.seal()?;

    // Materialized rather than handed back as a stream, because the two joins each read it whole.
    let mut out = SpillWriter::<JOIN_REC>::create(scratch, share)?;
    for rec in by_id.finish()? {
        out.push(rec?)?;
    }
    Ok((nodes, count, out.seal()?.0))
}

/// §8.4's placement rule: a record of `len` bytes goes at the cursor `at`, unless it would straddle
/// a chunk boundary — then it goes at the next one, and the bytes it skips are `0xFF` padding.
///
/// One function because there are two callers and they must not drift: [`merge`] lays the pool out
/// with it to mint the `Edge Id`s, and [`serialize`] walks the same rule to emit the padding those
/// ids assume. A disagreement between the two would be a file whose adjacency points into the gaps.
fn place(at: u64, len: u64) -> u64 {
    let within = at % NAV_CHUNK_SIZE as u64;
    if within + len > NAV_CHUNK_SIZE as u64 {
        at + (NAV_CHUNK_SIZE as u64 - within)
    } else {
        at
    }
}

/// One §8.4 edge record, resolved inside its cell's pool from `edge_id` — since v14 a packed
/// `(chunk, ordinal)` pair rather than a byte offset — and returned with the **byte offset** it was
/// found at, which is the address [`serialize`] later reads it back from.
///
/// The resolve is `obc_formats`' own walk, applied to the one 512-byte chunk the id names: it takes
/// each record's length from its own `Pt Count` and bounds-checks every intermediate record exactly
/// as it does the target. An id a cell's adjacency states is an input, not this engine's output, so
/// a refused id is a [`Error::Format`] and not an absent edge.
///
/// The slice is borrowed from the pool buffer the caller has open — the merge reads what it needs
/// from it and keeps only the address.
fn edge_record<'p>(pool: &'p [u8], edge_id: u32, cell: &Cell<'_>) -> Result<(u32, &'p [u8])> {
    let bad = |what: &str| Error::Format(format!("cell {}: edge id {edge_id} {what}", cell.id));
    let chunk_index = nav_edge_id_chunk(edge_id) as usize;
    let chunk_at = chunk_index.checked_mul(NAV_CHUNK_SIZE).ok_or_else(|| bad("names a chunk past any pool"))?;
    let chunk = pool
        .get(chunk_at..chunk_at + NAV_CHUNK_SIZE)
        .ok_or_else(|| bad("names a chunk outside the cell's edge pool"))?;
    let (start, end) = nav_edge_record_range(chunk, nav_edge_id_ordinal(edge_id))
        .ok_or_else(|| bad("does not resolve to a record in its chunk (OBCM §8.4)"))?;
    // Additively, and checked: `chunk_at` is already bounded by the pool, but a `usize` add is a
    // `usize` add and this crate builds for wasm32, where one wraps at 4 GiB.
    let at = chunk_at
        .checked_add(start)
        .and_then(|a| u32::try_from(a).ok())
        .ok_or_else(|| bad("resolves past the 4 GiB a pool address fits"))?;
    Ok((at, &chunk[start..end]))
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
/// `[directory][filler][profile table][alignment run][node index][filler][node chunks][edge pool]
/// [alignment run][snap index][filler][snap chunks]`.
///
/// **Every one of those gaps is `0xFF` since v14**, where v13 wrote zeros for the 512-byte alignment
/// runs and `0xFF` only inside a chunk. One fill byte, one rule (`OBCM_Spec.md` §1.2): a gap is
/// `0xFF` and a reserved field is `0`. An alignment run is a gap no offset reaches, so it takes the
/// gap's byte.
///
/// Every offset in the directory is known before the first byte goes out — that is what
/// [`MergedNav::projection`] already proves — so nothing is back-patched and nothing is staged.
/// **Nothing section-sized exists at all** (#1116 D4): the index is copied off the scratch seam a
/// block at a time, the chunks are assembled one 512-byte chunk at a time from the placement plan,
/// and the pool's records come from the cells that wrote them, one read into one reusable buffer.
/// `cells` must therefore be the same `network` cells [`merge`] read, in the same order.
///
/// `scratch` must likewise be the store the merge spilled into; the section is read out of it and
/// the streams stay valid until [`MergedNav::release`].
///
/// `profile_table` is the cells' own, copied after every cell was checked to agree (§4.3) — it is
/// schema data and the assembler has no business re-deriving it. An empty graph still writes the
/// directory and the profile table, both regions zero-length just past it.
pub fn serialize(
    nav: &MergedNav,
    profile_table: &[u8],
    section_offset: usize,
    cells: &[&Cell<'_>],
    scratch: &dyn ScratchStore,
    w: &mut MapWriter<'_>,
) -> Result<()> {
    let start = w.at();
    debug_assert_eq!(start, section_offset as u64, "the cursor is where the layout placed the section");
    let projection = nav.projection(profile_table);
    // The one place §8's boundaries are decided; everything below puts bytes and pads by its
    // numbers. `l`'s offsets are section-relative, so the absolute ones are `start + …`.
    let l = projection.layout_at(start);
    let profile_count = profile_table.len() / obc_formats::obcm::NAV_PROFILE_LEN;
    let edge_chunk_count = (nav.pool_len / NAV_CHUNK_SIZE as u64) as u32;

    // A zero-length region still has to be **nameable**, so an empty graph points all three of its
    // offsets at the first unit boundary past the profile table rather than at its last byte — which
    // is what the layout put in `index`/`edge_pool`/`snap_index` for it.
    let mut dir = Vec::with_capacity(NAV_DIR_LEN);
    dir.extend_from_slice(&scaled(start + l.index)?.to_le_bytes());
    dir.extend_from_slice(&nav.node_count.to_le_bytes());
    dir.extend_from_slice(&nav.chunk_count.to_le_bytes());
    dir.extend_from_slice(&scaled(start + l.edge_pool)?.to_le_bytes());
    dir.extend_from_slice(&edge_chunk_count.to_le_bytes());
    dir.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes());
    dir.extend_from_slice(&scaled(start + l.profile_table)?.to_le_bytes());
    dir.push(profile_count as u8);
    dir.push(0); // reserved — a field, so `0`, unlike a gap
    dir.extend_from_slice(&scaled(start + l.snap_index)?.to_le_bytes());
    dir.extend_from_slice(&nav.snap_node_count.to_le_bytes());
    dir.extend_from_slice(&nav.snap_chunk_count.to_le_bytes());
    debug_assert_eq!(dir.len(), NAV_DIR_LEN);
    w.put(&dir)?;
    w.pad(l.dir_gap)?;
    w.put(profile_table)?;
    let Some(files) = &nav.files else {
        // The empty pair a non-core shard carries (§5.1): the directory and the profile table are
        // the whole section, plus the filler that carries it to the boundary the three zero-length
        // regions are named at.
        w.begin_section()?;
        debug_assert_eq!(w.at() - start, l.end);
        return Ok(());
    };
    w.pad(l.index_pad)?;
    // The §8.2 index is already in wire form on the seam, so it is a copy through one block buffer.
    let mut block = vec![0u8; nav.read_budget.clamp(NAV_CHUNK_SIZE, 1 << 20)];
    let mut at = 0u64;
    let end = nav.index_len;
    while at < end {
        let want = block.len().min((end - at) as usize);
        scratch.read_at(files.index, at, &mut block[..want])?;
        w.put(&block[..want])?;
        at += want as u64;
    }
    w.pad(l.index_gap)?;
    emit_tree_chunks(nav.chunk_count, nav.read_budget, files.places, files.points, files.recs, scratch, w)?;

    // The pool, record by record. `pad` is §8.3's `0xFF` sentinel run — the bytes a record's chunk
    // gets instead of a record that would straddle it — and `rec` is the one buffer every record is
    // read into. Both are a chunk long, which is the largest either can ever be.
    let pad = [CHUNK_END; NAV_CHUNK_SIZE];
    let mut rec = [0u8; NAV_CHUNK_SIZE];
    let mut at = 0u64;
    for r in SpillReader::<POOL_REC>::open(scratch, files.pool, nav.read_budget)? {
        let r = pool_ref(&r?);
        let len = r.len as usize;
        // The same rule the layout used, so the padding lands exactly where the `Edge Id`s say.
        let placed = place(at, len as u64);
        if placed > at {
            w.put(&pad[..(placed - at) as usize])?;
            at = placed;
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
        cell.read_into(cell.nav.edge_pool_offset + u64::from(r.off), buf)?;
        w.put(buf)?;
        at += len as u64;
    }
    // §8.1 measures the pool in whole chunks, so the tail pads out to one.
    if at < nav.pool_len {
        // `pad` is `[0xFF; 512]`; the remainder is at most one chunk by construction.
        w.put(&pad[..(nav.pool_len - at) as usize])?;
    }
    w.pad(l.snap_index_pad)?;
    if let Some(snap) = &files.snap {
        let mut at = 0u64;
        while at < nav.snap_index_len {
            let want = block.len().min((nav.snap_index_len - at) as usize);
            scratch.read_at(snap.index, at, &mut block[..want])?;
            w.put(&block[..want])?;
            at += want as u64;
        }
        w.pad(l.snap_gap)?;
        emit_tree_chunks(nav.snap_chunk_count, nav.read_budget, snap.places, snap.points, snap.recs, scratch, w)?;
    }
    debug_assert_eq!(w.at() - start, l.end, "the projection is the write");
    Ok(())
}

/// The §8.2 chunk region, one 512-byte chunk at a time.
///
/// The placement plan is in emission order — chunk, then offset inside it — and every chunk is
/// opened by a leaf, so the walk fills one chunk, pads it with §8.3's `0xFF` sentinel and moves on.
/// A leaf's records are read back in **dense** order, which is the order `qtree::build`'s partition
/// left them in and therefore the order `qtree::flatten` packed them in, and the capacity guard is
/// re-applied per record — the same guard, over the same running fill, that the plan counted
/// `dropped_nodes` with.
fn emit_tree_chunks(
    chunk_count: u32,
    read_budget: usize,
    places: ScratchId,
    points: ScratchId,
    recs: ScratchId,
    scratch: &dyn ScratchStore,
    w: &mut MapWriter<'_>,
) -> Result<()> {
    let mut chunk: Vec<u8> = Vec::with_capacity(NAV_CHUNK_SIZE);
    let mut rec = [0u8; NAV_CHUNK_SIZE];
    let mut current = 0u32;
    let flush = |chunk: &mut Vec<u8>, w: &mut MapWriter<'_>| -> Result<()> {
        chunk.resize(NAV_CHUNK_SIZE, CHUNK_END);
        w.put(chunk)?;
        chunk.clear();
        Ok(())
    };
    for p in SpillReader::<{ qtree::PLACE_REC }>::open(scratch, places, read_budget)? {
        let p = p?;
        while current < qtree::place_chunk(&p) {
            flush(&mut chunk, w)?;
            current += 1;
        }
        debug_assert_eq!(chunk.len(), qtree::place_at(&p) as usize, "the plan and the write disagree about a leaf");
        for r in qtree::read_run(scratch, points, qtree::place_first(&p), qtree::place_count(&p))? {
            let len = qtree::rec_len(&r) as usize;
            if chunk.len() + len > NAV_CHUNK_SIZE {
                continue; // co-located overflow inside one leaf — counted as `dropped_nodes`
            }
            let buf = &mut rec[..len];
            scratch.read_at(recs, qtree::rec_at(&r) as u64, buf)?;
            chunk.extend_from_slice(buf);
        }
    }
    if chunk_count > 0 {
        flush(&mut chunk, w)?;
        debug_assert_eq!(current + 1, chunk_count, "every chunk is opened by a leaf");
    }
    Ok(())
}

impl MergedNav {
    /// The graph a shard with no nav carries: the directory plus the always-present profile table,
    /// both data regions zero-length (§5.1/§8.1).
    pub fn empty(stats: NavStats) -> MergedNav {
        MergedNav {
            files: None,
            node_count: 0,
            chunk_count: 0,
            index_len: 0,
            pool_len: 0,
            snap_node_count: 0,
            snap_chunk_count: 0,
            snap_index_len: 0,
            read_budget: NAV_CHUNK_SIZE,
            stats,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::MemoryScratch;

    /// §8.1's alignment run is the section's one offset-dependent term, and it depends on the
    /// offset only through its position inside a sector.
    ///
    /// The v14 arithmetic reserves `align_up(index_len, U)` rather than `index_len` — that rounding
    /// slack is what lets the index start on a **unit** boundary while the chunks behind it start on
    /// a **sector** one — so a four-byte index at prefix 84 takes 412 bytes of run where v13 took
    /// 424, and lands the index twelve bytes below the sector instead of exactly on it.
    ///
    /// `u64::MAX - 511` is not a position any map reaches; it is there because the layout walk runs
    /// at the section's **sector remainder**, and a walk that ran at the absolute offset would
    /// overflow before it answered.
    #[test]
    fn projection_alignment_depends_only_on_the_sector_remainder() {
        // A 36-byte profile table puts the prefix at 40 + 8 + 36 = 84, the offset the run below is
        // stated for; one sector of node chunks and one of edge pool behind it.
        let projection = NavProjection {
            profile_len: 36,
            index_len: 4,
            chunk_bytes: NAV_CHUNK_SIZE as u64,
            pool_len: NAV_CHUNK_SIZE as u64,
            snap_index_len: 0,
            snap_chunk_bytes: 0,
            populated: true,
            snap_populated: false,
        };
        // 84 prefix + 412 run + 4 index + 12 gap + 512 chunks + 512 pool.
        assert_eq!(projection.bytes_at(0), 1_536);
        assert_eq!(projection.bytes_at(512), 1_536);
        assert_eq!(projection.bytes_at(u64::MAX - 511), 1_536);
        // An empty graph is the directory, the profile table, and the run to the boundary its three
        // zero-length regions are named at.
        assert_eq!(NavProjection { populated: false, ..projection }.bytes_at(u64::MAX - 511), 96);

        // The two properties the run buys, at the offset the numbers above are stated for.
        let l = projection.layout_at(0);
        assert_eq!((l.index_pad, l.index), (412, 84 + 412));
        assert_eq!(l.index % SCALE.unit(), 0, "the index starts on a unit boundary");
        assert_eq!(crate::emit::align_up(l.index + 4) % NAV_CHUNK_SIZE as u64, 0, "…and its chunks on a sector");
        assert_eq!(l.index_gap, 12, "twelve bytes below the sector, not on it");
    }

    /// **The budget sweep.** Every whole-merge fixture goes through here rather than calling
    /// [`merge`] directly, because §4.6.5's renumbering is now an external sort and a fixture is far
    /// too small to reach its spill path on its own — at any sane budget a dozen nodes sort in one
    /// run, and the k-way merge would never be exercised by this file at all.
    ///
    /// So each fixture is merged at four budgets, from *one record per run* upwards, and the results
    /// are compared as bytes. The map may not depend on how much memory the merge was given; that is
    /// the whole claim the pass rests on. It also asserts the scratch area is **empty** afterwards:
    /// a merge that leaked a run would still produce the right map, and would fill a card.
    ///
    /// Since #1116 D4 a `MergedNav` is five scratch streams and no bytes, so "the results are
    /// compared as bytes" means the **section** — what [`serialize`] writes out of those streams,
    /// which is the only thing a shard ever sees. Each budget's merge is written, released, and the
    /// scratch is then asserted empty.
    fn merged(cells: &[&Cell<'_>], band_log2: u32, min_component_edges: usize, bbox: UBox) -> Merged {
        let mut out: Option<Merged> = None;
        for budget in [1usize, 24, 200, 1 << 20] {
            let scratch = MemoryScratch::new();
            let nav = merge(cells, band_log2, min_component_edges, bbox, &scratch, budget).expect("the merge succeeds");
            let got = Merged { section: serialized(&nav, &[], 0, cells, &scratch), stats: nav.stats.clone() };
            nav.release(&scratch);
            assert_eq!(scratch.resident_bytes(), 0, "budget {budget}: the merge left a scratch file behind");
            match &out {
                None => out = Some(got),
                Some(first) => {
                    assert_eq!(got.section, first.section, "budget {budget} merged to different bytes");
                    assert_eq!(got.stats, first.stats, "budget {budget} reported different counters");
                }
            }
        }
        out.expect("the sweep runs at least once")
    }

    /// One merge as a shard sees it: the §8 section bytes at offset 0, and what the merge reported.
    struct Merged {
        section: Vec<u8>,
        stats: NavStats,
    }

    impl Merged {
        /// The §8.2 chunk region, located through the directory the write just emitted — so the
        /// tests below read the junction records back the way a reader would, including §8.1's one
        /// rounding step between the index and the chunks.
        fn chunks(&self) -> &[u8] {
            let word = |at: usize| u32::from_le_bytes(self.section[at..at + 4].try_into().unwrap()) as u64;
            let data_start = crate::emit::align_up(word(0) * SCALE.unit() + word(4) * 4) as usize;
            &self.section[data_start..][..word(8) as usize * NAV_CHUNK_SIZE]
        }
    }

    /// The section through the sink, for a test that wants it as one buffer.
    fn serialized(
        nav: &MergedNav,
        profile_table: &[u8],
        section_offset: usize,
        cells: &[&Cell<'_>],
        scratch: &dyn ScratchStore,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut sink = |b: &[u8]| -> Result<()> {
                out.extend_from_slice(b);
                Ok(())
            };
            let mut w = MapWriter::new(SCALE, section_offset as u64, &mut sink);
            serialize(nav, profile_table, section_offset, cells, scratch, &mut w).expect("the section serializes");
        }
        out
    }

    /// A `MergedNav` that is **nothing but an edge pool**, on the caller's scratch: what the two
    /// streaming pins below need, and the one place a test builds [`NavFiles`] by hand.
    fn pool_nav(scratch: &dyn ScratchStore, pool: &[EdgeRef], pool_len: usize, node_count: u32) -> MergedNav {
        let mut out = SpillWriter::<POOL_REC>::create(scratch, 1 << 16).expect("a scratch write");
        for r in pool {
            out.push(pool_record(r)).expect("a scratch write");
        }
        let empty = || scratch.create().expect("a scratch file");
        MergedNav {
            files: Some(NavFiles {
                index: empty(),
                places: empty(),
                points: empty(),
                recs: empty(),
                pool: out.seal().expect("a scratch seal").0,
                snap: None,
            }),
            node_count,
            chunk_count: 0,
            index_len: 0,
            pool_len: pool_len as u64,
            snap_node_count: 0,
            snap_chunk_count: 0,
            snap_index_len: 0,
            read_budget: 1 << 16,
            stats: NavStats::default(),
        }
    }

    /// The empty section a non-core shard carries, and the three §1.2 gaps v14 puts in it: the
    /// eight bytes behind the 40-byte directory, and the run that carries the profile table's tail
    /// to the boundary the three zero-length regions are *named* at.
    ///
    /// Both gap assertions are load-bearing. Every directory field below reads correctly whatever
    /// those bytes are, and the section would still parse if the tail run were missing entirely —
    /// it would simply leave the next section starting mid-unit, which is unnameable rather than
    /// wrong-looking. So the fill byte and the exact length are pinned, not just the offsets.
    #[test]
    fn an_empty_section_still_carries_its_profiles_and_its_filler() {
        const AT: usize = 512; // a section offset is always a unit boundary
        let profiles = vec![0u8; obc_formats::obcm::NAV_PROFILE_LEN];
        let scratch = MemoryScratch::new();
        let bytes = serialized(&MergedNav::empty(NavStats::default()), &profiles, AT, &[], &scratch);
        let unit = SCALE.unit() as usize;
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0, "empty graph ⇒ no index nodes");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()) as usize, NAV_CHUNK_SIZE, "pinned 512");
        // §8.1: 40 is not a multiple of 16, so the table starts at the directory's byte 48.
        assert_eq!(u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize * unit, AT + NAV_DIR_LEN + 8);
        assert_eq!(bytes[26], 1, "one profile");
        // The three zero-length regions point at the first boundary past the profile table, not at
        // its last byte — a scaled offset cannot name byte 616.
        let past = AT + 48 + profiles.len();
        for (field, what) in [(0usize, "node index"), (12, "edge pool"), (28, "snap index")] {
            let at = u32::from_le_bytes(bytes[field..field + 4].try_into().unwrap()) as usize * unit;
            assert_eq!(
                at,
                crate::emit::align_up(past as u64) as usize,
                "the {what}'s zero-length region is still nameable"
            );
        }

        // --- the gaps, as bytes ---
        assert_eq!(&bytes[NAV_DIR_LEN..NAV_DIR_LEN + 8], &[CHUNK_END; 8], "§1.2's fill byte behind the directory");
        assert_eq!(past - AT, 104, "48 + one 56-byte profile record");
        assert_eq!(&bytes[104..], &[CHUNK_END; 8], "…and the run that carries 104 to 112");
        assert_eq!(bytes.len(), 112);
        assert_eq!(bytes.len() as u64, MergedNav::empty(NavStats::default()).projection(&profiles).bytes_at(AT as u64));
    }

    /// The country-scale path derives anchors from one bounded final edge record at a time. Pin the
    /// decode/interpolation/spill seam directly: a ~334 m two-point edge gets one midpoint anchor,
    /// naming the caller's final pool id, without retaining a graph-sized geometry collection.
    #[test]
    fn a_long_streamed_edge_spills_its_lookup_anchor() {
        let mut edge = Vec::new();
        edge.extend_from_slice(&334u32.to_le_bytes());
        edge.extend_from_slice(&2u16.to_le_bytes());
        edge.push(0);
        edge.extend_from_slice(&500_000i32.to_le_bytes());
        edge.extend_from_slice(&500_000i32.to_le_bytes());
        edge.extend_from_slice(&0i16.to_le_bytes());
        edge.extend_from_slice(&3_000i16.to_le_bytes());

        let scratch = MemoryScratch::new();
        let mut recs = ByteSpill::create(&scratch, 1 << 10).unwrap();
        let mut points = SpillWriter::<{ qtree::TREE_REC }>::create(&scratch, 1 << 10).unwrap();
        let mut ord = 0;
        let count =
            append_snap_anchors(&edge, 123, (0, 0, 1_000_000, 1_000_000), &mut recs, &mut points, &mut ord).unwrap();
        assert_eq!((count, ord), (1, 1));

        let recs = recs.seal().unwrap();
        let (points, point_count) = points.seal().unwrap();
        assert_eq!(point_count, 1);
        let mut anchor = [0u8; NAV_SNAP_RECORD_LEN];
        scratch.read_at(recs, 0, &mut anchor).unwrap();
        assert_eq!(i32::from_le_bytes(anchor[0..4].try_into().unwrap()), 500_000);
        assert_eq!(i32::from_le_bytes(anchor[4..8].try_into().unwrap()), 501_500);
        assert_eq!(u32::from_le_bytes(anchor[8..12].try_into().unwrap()), 123);
        let mut point = [0u8; qtree::TREE_REC];
        scratch.read_at(points, 0, &mut point).unwrap();
        assert_eq!(qtree::rec_at(&point), 0);
        assert_eq!(qtree::rec_len(&point) as usize, NAV_SNAP_RECORD_LEN);
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
                edge_pool_offset: SRC_POOL_AT as u64,
                edge_chunk_count: chunks,
                chunk_size: NAV_CHUNK_SIZE,
                ..obc_reader::NavDirectory::EMPTY
            },
            profile_table: Vec::new(),
            style_ids: Vec::new(),
            bytes: src.len(),
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

        let scratch = MemoryScratch::new();
        let nav = pool_nav(&scratch, &pool, pool_len, 3);
        // A unit boundary, because that is the only thing a scaled `Nav Graph Offset` can name.
        const SECTION_OFFSET: usize = 128;
        let bytes = serialized(&nav, &[], SECTION_OFFSET, &[&cell], &scratch);
        let pool_offset = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize * SCALE.unit() as usize;
        assert_eq!(&bytes[pool_offset - SECTION_OFFSET..], &want[..], "the streamed pool is not the laid-out pool");
        assert_eq!(pool_offset % NAV_CHUNK_SIZE, 0, "edge pool is sector-aligned");
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 2, "edge chunk count");
        // **The alignment run is `0xFF` since v14**, where v13 filled it with zeros — and no offset
        // in the directory reaches a byte of it, so nothing else in this file would notice. The run
        // here is everything between the 40-byte directory and the edge pool: the eight bytes behind
        // the directory, and the §8.1 run that lands the (empty) index and its chunks on a sector.
        let run = &bytes[NAV_DIR_LEN..pool_offset - SECTION_OFFSET];
        assert_eq!(run.len(), 512 - 128 - NAV_DIR_LEN, "the fixture really has a run to fill");
        assert!(run.iter().all(|&b| b == CHUNK_END), "a §1.2 gap is 0xFF, never 0");
        assert_eq!(
            bytes.len() as u64,
            nav.projection(&[]).bytes_at(SECTION_OFFSET as u64),
            "the section is the length it was projected at"
        );

        // The rule both walks share, stated once: a record moves to the next chunk only when it
        // would cross the boundary, and a record that ends exactly on one does not move.
        assert_eq!(place(300, 300), NAV_CHUNK_SIZE as u64, "a straddling record starts the next chunk");
        assert_eq!(place(300, 212), 300, "…and one that ends exactly on the boundary stays");
        assert_eq!(place(NAV_CHUNK_SIZE as u64, 1), NAV_CHUNK_SIZE as u64, "an aligned cursor never pads");
    }

    /// The layout names its source cells by index, so writing it against a different cell list is a
    /// refusal rather than a plausible-looking file full of the wrong polylines.
    #[test]
    fn writing_the_pool_against_the_wrong_cells_is_refused() {
        let scratch = MemoryScratch::new();
        let nav = pool_nav(&scratch, &[EdgeRef { cell: 1, off: 0, len: 16 }], NAV_CHUNK_SIZE, 1);
        let src = vec![0u8; SRC_POOL_AT + NAV_CHUNK_SIZE];
        let slice = obc_formats::io::SliceSource(&src);
        let cell = pool_cell(&slice, 1);
        let mut discard = |_: &[u8]| -> Result<()> { Ok(()) };
        let err = serialize(&nav, &[], 0, &[&cell], &scratch, &mut MapWriter::new(SCALE, 0, &mut discard))
            .expect_err("cell 1 was not handed over");
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

    /// Where a packed cell would put a run of §8.4 records — [`place`] from a zero cursor — as
    /// `(byte offset, Edge Id)` pairs.
    ///
    /// The two are no longer the same number. Since v14 an `Edge Id` is the packed `(chunk,
    /// ordinal)` pair, so laying the fixture's pool out still needs the byte offset while the
    /// adjacency entries that name those records need the id — and a fixture that confused the two
    /// would be exactly the map this slice exists to stop being written.
    fn pool_layout(recs: &[Vec<u8>]) -> Vec<(u32, u32)> {
        let mut at = 0u64;
        let mut chunk = 0u64;
        let mut ordinal = 0u32;
        recs.iter()
            .map(|r| {
                at = place(at, r.len() as u64);
                let here = at / NAV_CHUNK_SIZE as u64;
                if here != chunk {
                    chunk = here;
                    ordinal = 0;
                }
                let placed = (at as u32, nav_edge_id(chunk as u32, ordinal).expect("a fixture stays in field"));
                ordinal += 1;
                at += r.len() as u64;
                placed
            })
            .collect()
    }

    /// Just the ids, for a fixture's adjacency entries.
    fn pool_ids(recs: &[Vec<u8>]) -> Vec<u32> {
        pool_layout(recs).into_iter().map(|(_, id)| id).collect()
    }

    /// A synthetic `network` cell's §8 section: a node index (never read — [`merge`] walks the chunk
    /// run), the §8.3 node chunks, then the §8.4 edge pool at its own offset.
    ///
    /// The records themselves come out of `obcm-testkit`, which is the tree's one hand-written OBCM
    /// byte builder: an independent transcription of §8 here would be a *second* oracle for the same
    /// bytes, free to drift from the one `obc-reader`'s and `obc-render`'s tests are checked against.
    fn nav_bytes(nodes: &[SrcNode], recs: &[Vec<u8>]) -> (Vec<u8>, obc_reader::NavDirectory) {
        const INDEX_LEN: usize = 16;
        let coord = |id: u32| {
            let n = nodes.iter().find(|n| n.id == id).expect("a neighbour is a node of the same cell");
            (n.lat, n.lon)
        };
        // Chunk packing is greedy, exactly as a packed cell's is: a record that does not fit opens
        // the next chunk rather than being split.
        let mut chunks: Vec<Vec<Vec<u8>>> = vec![Vec::new()];
        let mut open_len = 0usize;
        for n in nodes {
            let nbrs: Vec<obcm_testkit::NavNeighborSpec> = n
                .nbrs
                .iter()
                .map(|nb| {
                    let (lat, lon) = coord(nb.id);
                    (nb.id, lat, lon, nb.edge_id, u32::from(nb.cost), nb.kind, nb.ascent)
                })
                .collect();
            let rec = obcm_testkit::pack_nav_record(n.lat, n.lon, n.id, &nbrs);
            if open_len + rec.len() > NAV_CHUNK_SIZE {
                chunks.push(Vec::new());
                open_len = 0;
            }
            open_len += rec.len();
            chunks.last_mut().expect("a chunk is always open").push(rec);
        }
        let mut bytes = vec![0u8; INDEX_LEN];
        for c in &chunks {
            bytes.extend_from_slice(&obcm_testkit::pack_nav_chunk(c, NAV_CHUNK_SIZE));
        }
        let pool_at = bytes.len();
        let mut pool: Vec<u8> = Vec::new();
        for (r, &(off, _)) in recs.iter().zip(&pool_layout(recs)) {
            pool.resize(off as usize, CHUNK_END);
            pool.extend_from_slice(r);
        }
        pool.resize(pool.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE, CHUNK_END);
        let dir = obc_reader::NavDirectory {
            index_offset: 0,
            node_count: INDEX_LEN / 4,
            chunk_count: chunks.len(),
            edge_pool_offset: pool_at as u64,
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
            bytes: src.len(),
        }
    }

    /// The merged junction records, read back out of the chunks the merge packed, by dense id:
    /// `(lat, lon, id, [(neighbour id, edge id, cost, way kind, ascent)])`.
    #[allow(clippy::type_complexity)]
    fn merged_nodes(nav: &Merged) -> Vec<(i32, i32, u32, Vec<(u32, u32, u16, u8, u16)>)> {
        let mut out = Vec::new();
        for chunk in nav.chunks().chunks_exact(NAV_CHUNK_SIZE) {
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
        let rec = obcm_testkit::pack_nav_edge_record(1000, 3, &[(lat_p, SEAM_LON), (lat_q, SEAM_LON)]);
        let e = pool_ids(std::slice::from_ref(&rec))[0];
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
            .map(|k| {
                let far = (hub_lat + 100 * (k as i32 + 1), SEAM_LON + 1);
                obcm_testkit::pack_nav_edge_record(100 + k as u32, 3, &[(hub_lat, SEAM_LON), far])
            })
            .collect();
        let half = |from: usize, to: usize| {
            let own: Vec<Vec<u8>> = recs[from..to].to_vec();
            let ids = pool_ids(&own);
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

    /// **The ordinal reset, at merge level.** Every other fixture in this file lays its edges out in
    /// a single 512-byte chunk, so `Edge Chunk Count == 1` throughout and the `if here !=
    /// chunk_index { ordinal = 0 }` branch in [`merge`]'s minting loop has never executed under
    /// test. The branch that mints duplicate ids when it is wrong is the branch nothing runs.
    ///
    /// This drives the merged pool past 512 bytes twice, once through each way a chunk can end —
    /// and they are genuinely different code paths, which is what the bug in this PR's own history
    /// was about:
    ///
    /// - **pushed**: a record that does not fit the space left is moved to the next boundary, so
    ///   the chunk ends with filler behind its last record;
    /// - **flush**: a record whose length divides the space left *exactly* ends on the boundary, so
    ///   the next chunk opens with no filler and no push. This is the case the original writer got
    ///   wrong — it reset the ordinal when a record was pushed, and a flush run carried the counter
    ///   straight over into the next chunk and minted a duplicate id.
    ///
    /// Both assert the ids as `(chunk, ordinal)` pairs rather than as raw `u32`s, because the whole
    /// point of the field is that those two halves are separable.
    #[test]
    fn a_merged_pool_that_spans_two_chunks_restarts_the_ordinal_in_each() {
        const MIN_REC: usize = NAV_EDGE_FIXED_LEN + 4; // a 2-point record: 19 bytes

        /// `edges[k] = point count of edge k`, laid out as disjoint two-node edges whose latitude
        /// increases with `k` — dense renumbering is by latitude, so emission order is `k` order
        /// and the fixture controls exactly where each record lands.
        fn ids_of(points: &[usize]) -> Vec<(u32, u32)> {
            let lat_of = |k: usize| 6_000_000i32 + 1_000 * k as i32;
            let recs: Vec<Vec<u8>> = points
                .iter()
                .enumerate()
                .map(|(k, &n)| {
                    let pts: Vec<(i32, i32)> = (0..n).map(|i| (lat_of(k), SEAM_LON + i as i32)).collect();
                    obcm_testkit::pack_nav_edge_record(100 + k as u32, 3, &pts)
                })
                .collect();
            let ids = pool_ids(&recs);
            let mut nodes = Vec::new();
            for (k, _) in points.iter().enumerate() {
                let (a, b) = (2 * k as u32, 2 * k as u32 + 1);
                let nbr = |id: u32| SrcNbr { id, edge_id: ids[k], cost: 100 + k as u16, kind: 3, ascent: 7 };
                nodes.push(SrcNode { id: a, lat: lat_of(k), lon: SEAM_LON, nbrs: vec![nbr(b)] });
                nodes.push(SrcNode { id: b, lat: lat_of(k), lon: SEAM_LON + points[k] as i32 - 1, nbrs: vec![nbr(a)] });
            }
            let (bytes, dir) = nav_bytes(&nodes, &recs);
            let src = obc_formats::io::SliceSource(&bytes);
            let cell = nav_cell(&src, dir);
            let bbox =
                (SEAM_LON as i64 - 10, lat_of(0) as i64 - 10, SEAM_LON as i64 + 100, lat_of(points.len()) as i64 + 10);
            let nav = merged(&[&cell], 20, 0, bbox);
            assert_eq!(nav.stats.edges, points.len(), "every disjoint edge survives the merge");
            let mut seen: Vec<(u32, u32)> = merged_nodes(&nav)
                .iter()
                .flat_map(|n| n.3.iter().map(|e| (nav_edge_id_chunk(e.1), nav_edge_id_ordinal(e.1))))
                .collect();
            seen.sort_unstable();
            seen.dedup();
            seen
        }

        // --- pushed ------------------------------------------------------------------------
        // 26 x 19 = 494 bytes; the 27th needs 19 more and 494 + 19 = 513 > 512, so it is pushed
        // to the boundary and opens chunk 1.
        assert_eq!(26 * MIN_REC, 494);
        const { assert!(494 + MIN_REC > NAV_CHUNK_SIZE, "the 27th record cannot fit — it is pushed") };
        let pushed = ids_of(&[2; 27]);
        let mut want: Vec<(u32, u32)> = (0..26).map(|o| (0, o)).collect();
        want.push((1, 0));
        assert_eq!(pushed, want, "26 ordinals in chunk 0, then the ordinal restarts at 0 in chunk 1");

        // --- flush -------------------------------------------------------------------------
        // Record lengths are `15 + 4*(n-1)`, so every one is 3 mod 4 and a run can only land on
        // 512 with a multiple of four records. 23 x 19 + 75 = 512 exactly: a 16-point record
        // closes chunk 0 on its last byte, with no filler and nothing pushed.
        let big = NAV_EDGE_FIXED_LEN + 4 * (16 - 1);
        assert_eq!(big, 75);
        assert_eq!(23 * MIN_REC + big, NAV_CHUNK_SIZE, "chunk 0 ends flush on its boundary");
        let mut shape = vec![2usize; 23];
        shape.push(16); // the record that ends flush at 512
        shape.push(2); // and the one that must therefore be (1, 0)
        let flush = ids_of(&shape);
        let mut want: Vec<(u32, u32)> = (0..24).map(|o| (0, o)).collect();
        want.push((1, 0));
        assert_eq!(flush, want, "a flush chunk end restarts the ordinal just as a pushed one does");
    }
}
