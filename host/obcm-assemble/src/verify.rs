//! The verify pass ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §4.8): read the finished output
//! back through the **real reader** — the same crate the firmware runs — before anything is handed
//! anywhere.
//!
//! This is a *precondition of writing a set*, not an optional extra, and the reason is stated in the
//! spec's design principles: a catalog artifact was verified by the bakery, but an assembly was made
//! on the rider's own machine, outside the manifest. Nothing self-made reaches a device unverified.
//!
//! It is also the pass that catches the graft's characteristic failures. A mis-relocated index node
//! produces geometry in the wrong place *and* an anchor that no longer fits its leaf, and a wrong
//! chunk base produces a stream that never meets its `0xFF` sentinel — so "decode every feature of
//! every chunk" is not paranoia, it is the tripwire that fires on exactly the bug this crate can
//! have.
//!
//! It is also, at country scale, the assembler's **peak-memory phase** — it re-derives the whole
//! graph from bytes that were just written, while the merged graph they came from is still alive
//! (#1116). So nothing here is allowed to hold a hash entry or a heap allocation per graph element:
//! [`verify_nav`] walks the nav section over dense arrays instead of a resident arc list, and
//! re-reading a section that is already on disk is the cheaper half of that trade.
//!
//! #1116 phase D takes the next step and makes those dense arrays **bounded** rather than merely
//! compact, because "8 bytes per junction" is still 1.5–2.9 GB at DACH:
//!
//! * The junction table is **banded**. Only a contiguous range of dense ids is resident at a time,
//!   sized from [`crate::Options::merge_budget_bytes`]; a graph that does not fit is walked once
//!   more per band. See [`NodeTable`].
//! * The edge claims go through [`crate::extsort::ExternalSort`] instead of one `Vec` of `2 × E`
//!   records, and each claim carries its claimant's coordinate so that the §4.8.4 edge checks need
//!   no random access back into the junction table.
//! * The union-find keeps each component's size **in its root's slot**, which is what removes the
//!   second whole-map `Vec` the component histogram used to need.
//!
//! What is left resident and proportional to the graph is 4.25 bytes per junction: the union-find
//! (4 B) and two bitmaps (⅛ B each). Everything else is the band (at most the budget) plus the
//! sort's buffers (an eighth of it) — a ceiling of `1.125 × budget`, whatever the map.

use std::cmp::Ordering;

use obc_formats::io::ByteSource;
use obc_formats::obcm::{NAV_CHUNK_SIZE, NAV_EDGE_FIXED_LEN, NAV_MAX_DEGREE, NAV_NODE_FIXED_LEN};
use obc_map_scene::BBox;
use obc_reader::{MapCache, MapTables, NavNodeRef, Reader, MAX_FEAT_PTS, MAX_FEAT_RINGS, NAV_MAX_CHUNK_BYTES};

use crate::extsort::ExternalSort;
use crate::grid::AlignedBox;
use crate::scratch::ScratchStore;
use crate::{Error, Result};

/// Vertices the longest legal `OBCM_Spec.md` §8.4 edge record can hold. Derived, not chosen: a
/// record never straddles a chunk, so `15 + (Pt Count − 1) × 4 ≤ 512` bounds it at 125. Sizing the
/// verify buffer from the format means a record the format permits can never be reported as
/// undecodable because the *checker* ran out of room.
const MAX_EDGE_PTS: usize = 1 + (NAV_CHUNK_SIZE - NAV_EDGE_FIXED_LEN) / 4;

/// What a verified file reports about itself. Counts, not opinions — the caller decides what an
/// implausible number means (§4.8.5 forbids silently repairing one).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub chunks: u64,
    pub features: u64,
    pub nav_nodes: u64,
    pub nav_edges: u64,
    pub components: u64,
    /// Share of the graph in its largest component, per mille. A broken seam shows up here.
    pub largest_component_permille: u32,
}

/// Walk one finished shard: parse, decode every feature of every chunk, re-check the offset-table
/// invariants, and validate nav integrity end to end.
///
/// `scratch` is where the nav pass spills its claim stream and `budget` is
/// [`crate::Options::merge_budget_bytes`] — the same number the §4.6 merge divides, because verify
/// runs after the merge and the two never hold their buffers at the same time. Everything the pass
/// sizes from the graph rather than the budget is listed in the module header.
pub fn verify_shard(
    src: &dyn ByteSource,
    expected_box: AlignedBox,
    expect_sections: bool,
    scratch: &dyn ScratchStore,
    budget: usize,
) -> Result<VerifyReport> {
    let mut report = VerifyReport::default();
    // 1. Parse. Header, style table, LOD table, POI directory, nav directory and profile table all
    //    parse and validate — `MapTables::parse` is exactly that gate.
    let tables = MapTables::parse(src).map_err(|e| Error::Verify(format!("the output does not parse: {e:?}")))?;
    let (min_lon, min_lat, max_lon, max_lat) = expected_box.ubox();
    let b = tables.bbox;
    if (b.min_lon as i64, b.min_lat as i64, b.max_lon as i64, b.max_lat as i64) != (min_lon, min_lat, max_lon, max_lat)
    {
        return Err(Error::Verify(format!(
            "shard header bbox ({}, {}, {}, {}) is not its planned box ({min_lon}, {min_lat}, {max_lon}, {max_lat})",
            b.min_lon, b.min_lat, b.max_lon, b.max_lat
        )));
    }
    if src.len() as u64 > crate::shard::FILE_CEILING {
        return Err(Error::Verify(format!("the shard is {} bytes, past the 4 GiB − 1 ceiling", src.len())));
    }

    let cache = MapCache::new_boxed();
    let reader = Reader::new(src, &tables, &cache);
    let view =
        BBox { min_lon: min_lon as i32, min_lat: min_lat as i32, max_lon: max_lon as i32, max_lat: max_lat as i32 };

    // 2/3. Every chunk, every feature — plus the §5.1 offset-table invariants, re-derived from the
    //      bytes rather than trusted.
    let mut points: heapless::Vec<(i32, i32), MAX_FEAT_PTS> = heapless::Vec::new();
    let mut rings: heapless::Vec<usize, MAX_FEAT_RINGS> = heapless::Vec::new();
    for (i, lod) in reader.lods().iter().enumerate() {
        if lod.node_count == 0 {
            if lod.chunk_count != 0 {
                return Err(Error::Verify(format!("LOD {i} has no index but claims {} chunks", lod.chunk_count)));
            }
            continue;
        }
        check_offset_table(src, lod, i)?;
        // The leaf walk borrows the reader's index cache for its duration, so the chunk list is
        // collected first and decoded after — a nested streaming call would legally fail.
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        reader
            .for_each_chunk(i, &view, |id, node| chunks.push((id, node)))
            .map_err(|e| Error::Verify(format!("LOD {i}: the quadtree walk failed: {e:?}")))?;
        for (id, node) in &chunks {
            let status = reader
                .for_each_feature(i, *id, node, &mut points, &mut rings, |_| report.features += 1)
                .map_err(|e| Error::Verify(format!("LOD {i} chunk {id}: {e:?}")))?;
            if status.malformed > 0 || status.capacity_dropped > 0 {
                return Err(Error::Verify(format!(
                    "LOD {i} chunk {id}: {} malformed and {} over-capacity feature(s) — a mis-relocated index or a \
                     bad chunk base (OBCA §4.8.2)",
                    status.malformed, status.capacity_dropped
                )));
            }
            report.chunks += 1;
        }
    }

    // 4/5. Nav integrity and the reachability report.
    let dir = reader.nav_directory();
    if !expect_sections {
        if !dir.is_empty() {
            return Err(Error::Verify("a non-core shard carries a nav graph (OBCA §5.1)".into()));
        }
        if reader.poi_directory().entries.iter().any(|e| e.chunk_count > 0) {
            return Err(Error::Verify("a non-core shard carries POIs (OBCA §5.1)".into()));
        }
    }
    if reader.nav_profiles().is_empty() {
        return Err(Error::Verify("the shard carries no §8.6 profile table".into()));
    }
    if !dir.is_empty() {
        verify_nav(&reader, &view, &mut report, scratch, budget)?;
    }
    Ok(report)
}

/// `OBCM_Spec.md` §5.1's offset-table invariants for every chunk of one LOD: `offsets[0] == 0`,
/// monotone, ends in the region, and no pair spans more than `Chunk Size`.
fn check_offset_table(src: &dyn ByteSource, lod: &obc_reader::Lod, i: usize) -> Result<()> {
    let table_start = lod.index_offset + lod.node_count * 4;
    let raw = crate::input::read_at(src, table_start, (lod.chunk_count + 1) * 4)?;
    let mut prev = 0u32;
    for (k, w) in raw.chunks_exact(4).enumerate() {
        let v = u32::from_le_bytes([w[0], w[1], w[2], w[3]]);
        if k == 0 {
            if v != 0 {
                return Err(Error::Verify(format!("LOD {i}: offsets[0] is {v}, not 0")));
            }
            continue;
        }
        if v < prev {
            return Err(Error::Verify(format!("LOD {i}: offset table runs backwards at chunk {}", k - 1)));
        }
        if (v - prev) as usize > lod.chunk_size {
            return Err(Error::Verify(format!(
                "LOD {i}: chunk {} spans {} bytes, past the {} capacity bound",
                k - 1,
                v - prev,
                lod.chunk_size
            )));
        }
        prev = v;
    }
    if prev as usize != lod.chunk_bytes_total {
        return Err(Error::Verify(format!(
            "LOD {i}: the offset table ends at {prev} but the LOD table says {} chunk bytes",
            lod.chunk_bytes_total
        )));
    }
    let end = table_start + raw.len() + lod.chunk_bytes_total;
    if end > src.len() as usize {
        return Err(Error::Verify(format!("LOD {i}: the chunk region runs past the end of the file")));
    }
    Ok(())
}

/// One **band** of the junction table: a contiguous range of dense ids, `[lo, lo + span)`.
///
/// The table is dense because `OBCM_Spec.md` §8.3 says node ids are file-local and dense and
/// `OBCA_Spec.md` §4.6.5's renumbering is what writes them that way. Indexing a `Vec` by id is what
/// lets this pass hold sixteen bytes per junction instead of a hash entry, and it turns §8.3's
/// "every id below the highest has a record" into a popcount.
///
/// It is **banded** because dense is not the same as bounded: sixteen bytes times DACH's junctions
/// is a quarter of a gigabyte, and #1116's standing law is that no structure here may grow with the
/// map. A band holds `span` ids, `span` is set from [`crate::Options::merge_budget_bytes`], and a
/// graph with more ids than that is walked once more per band. Records that fall outside the band
/// are still *examined* — the id-range check, the degree cap and the running node count are global
/// — they are simply not stored.
///
/// The one thing banding costs is re-reads, and it is why the band gets the lion's share of the
/// budget while the claim sort (a stream, which only trades buffer for run count) gets an eighth:
/// re-walking §8.2 at country scale delivers each record ~2.3× and is far more expensive than a
/// deeper k-way merge.
///
/// The seen set is a bitmap and not a `Vec<bool>` because it survives alongside the coordinates, the
/// digests, the union-find and the sort's run buffer at the peak: at Baden-Württemberg's ~3 M
/// junctions it is 366 KiB rather than 2.9 MiB.
///
/// Faults are **recorded, not raised**: the walk that fills this is a `FnMut` callback with no error
/// channel, and bailing out of it early would under-count the degree-cap tally the caller reports
/// first. The first fault wins, which is the order a `?` would have produced anyway.
#[derive(Debug, Default)]
struct NodeTable {
    /// First id this band holds.
    lo: usize,
    /// How many ids it may hold — the budget, in junctions.
    span: usize,
    /// `(lat, lon)` per in-band id, offset by `lo` — absolute µdeg, as §8.3 stores them.
    coords: Vec<(i32, i32)>,
    /// [`record_digest`] of the record that claimed each in-band id, so a re-delivery can be told
    /// from a second, *different* record wearing the same id.
    digest: Vec<u64>,
    /// One bit per in-band id: "a §8.3 record for this id was seen".
    seen: Vec<u64>,
    /// Popcount of `seen`, maintained as it fills.
    count: usize,
    /// The first id this section cannot possibly dense-number (see [`verify_nav`]).
    ceiling: usize,
    /// The highest id seen *anywhere*, plus one — the graph's junction count, which the first
    /// band's walk is what learns (nothing in the directory states it: §8.1's `node_count` is the
    /// quadtree index's size, not the graph's).
    top: usize,
    fault: Option<String>,
}

impl NodeTable {
    /// A band over `[lo, lo + span)`, clamped to the ids the section could possibly hold. The clamp
    /// is what keeps a budget larger than the map from sizing anything: no id reaches `ceiling`, so
    /// a band that stretched past it would only ever allocate for ids that cannot exist.
    fn new(lo: usize, span: usize, ceiling: usize) -> Self {
        NodeTable { lo, span: span.min(ceiling.saturating_sub(lo)), ceiling, ..Default::default() }
    }

    /// Record one §8.3 junction — or, when it is not this band's, just count it.
    ///
    /// §8.2's bin packing can hand the same record back more than once, so a repeat carrying the
    /// same content is accepted — that is the idempotence every consumer of these records owes the
    /// format, and it is what the old pass's `HashMap::insert` accepted. A repeat that *disagrees*
    /// is two different junctions wearing one id, and is a verify failure: the coordinates get their
    /// own message because they are what the rest of the pass indexes by, and any other difference
    /// is caught by the digest.
    ///
    /// Proving a repeat is a repeat is what lets the adjacency walk process each junction exactly
    /// once (§8.2 delivered 17.5 M records for BW's 3.0 M junctions), and the strictness is why that
    /// is not a loss of checking: the old pass re-checked a re-delivered record's adjacency, this
    /// one refuses the only case where re-checking could have found anything.
    fn see(&mut self, id: u32, lat: i32, lon: i32, digest: u64) {
        let id_usize = id as usize;
        if id_usize >= self.ceiling {
            // Refused before it is used to size anything: `coords` is indexed by id, so a corrupt
            // id must not be allowed to name an allocation.
            self.fail(format!(
                "node id {id} is out of the section's range: its node chunks cannot hold more than {} dense §8.3 \
                 records (OBCM §8.3)",
                self.ceiling
            ));
            return;
        }
        self.top = self.top.max(id_usize + 1);
        let Some(i) = id_usize.checked_sub(self.lo).filter(|i| *i < self.span) else {
            return; // another band's junction; its own pass records it.
        };
        if i >= self.coords.len() {
            self.grow_past(i);
        }
        let (word, bit) = (i / 64, 1u64 << (i % 64));
        if self.seen[word] & bit != 0 {
            if self.coords[i] != (lat, lon) {
                let first = self.coords[i];
                self.fail(format!(
                    "node id {id} has two §8.3 records with different coordinates, {first:?} and {:?}",
                    (lat, lon)
                ));
            } else if self.digest[i] != digest {
                self.fail(format!("node id {id} has two §8.3 records with different adjacency"));
            }
            return;
        }
        self.seen[word] |= bit;
        self.coords[i] = (lat, lon);
        self.digest[i] = digest;
        self.count += 1;
    }

    /// Extend the band's arrays past in-band index `i`, in **fixed steps of a sixteenth of the
    /// band** and with `reserve_exact`.
    ///
    /// The obvious `resize(i + 1, …)` is what this replaces, and the reason is measured: `Vec`'s
    /// amortised growth doubles, so a table that ends at 2.99 M junctions (Baden-Württemberg) sits in
    /// capacity for 4.19 M — 46 MiB of slack across the coordinates and the digests, which is most of
    /// a band. Growing in fixed steps caps the slack at one step (4 MiB at the default budget) for
    /// at most sixteen reallocations over the whole walk.
    fn grow_past(&mut self, i: usize) {
        let step = (self.span / 16).max(1024);
        let want = (i + 1).next_multiple_of(step).min(self.span);
        self.coords.reserve_exact(want - self.coords.len());
        self.coords.resize(want, (0, 0));
        self.digest.reserve_exact(want - self.digest.len());
        self.digest.resize(want, 0);
        let words = want.div_ceil(64);
        self.seen.reserve_exact(words - self.seen.len());
        self.seen.resize(words, 0);
    }

    fn fail(&mut self, msg: String) {
        if self.fault.is_none() {
            self.fault = Some(msg);
        }
    }

    /// The recorded fault, then the density check over this band's slice of the numbering.
    ///
    /// §8.3's ids are dense, so every id below the highest one seen must have a record — and `total`
    /// is that highest one plus one, learned by the first band's walk. The old pass could not notice
    /// a gap at all (a hash map has no opinion about which keys are absent) and would have carried
    /// on as if the graph were simply smaller. A gap means the walk never saw a junction that some
    /// neighbour entry will point at, so §4.8.4's "every neighbour resolves" is only truly checkable
    /// once this holds — for **every** band, which is why a hole in band 3 refuses in band 3's pass
    /// rather than being lost in a table that stops at band 0.
    fn finish(mut self, total: usize) -> Result<Self> {
        if let Some(msg) = self.fault.take() {
            return Err(Error::Verify(msg));
        }
        let expected = total.saturating_sub(self.lo).min(self.span);
        if self.count != expected {
            return Err(Error::Verify(format!(
                "the graph's node ids are not dense: {} of the {total} ids below the highest one have no §8.3 record \
                 (OBCM §8.3)",
                expected.saturating_sub(self.count),
            )));
        }
        Ok(self)
    }

    /// The graph's junction count: the highest id seen anywhere, plus one.
    fn total(&self) -> usize {
        self.top
    }

    /// The junction's `(lat, lon)`, or `None` if `id` is outside this band or has no record in it.
    ///
    /// The seen bit, not the array's length, is what answers "has a record": [`NodeTable::grow_past`]
    /// extends the arrays in steps, so an index inside them is not proof of a record. Answering from
    /// the bitmap makes an unwritten slot unreachable rather than readable as `(0, 0)` — after
    /// [`NodeTable::finish`] no in-band id below `total` can be missing anyway, so this costs one
    /// bit test to make the failure mode impossible instead of merely unreachable.
    fn get(&self, id: u32) -> Option<(i32, i32)> {
        let i = (id as usize).checked_sub(self.lo).filter(|i| *i < self.span)?;
        let word = self.seen.get(i / 64)?;
        (word & (1u64 << (i % 64)) != 0).then(|| self.coords[i])
    }
}

/// FNV-1a over one §8.3 record's decoded content: its coordinates, its degree and every field of
/// every adjacency entry — `Ascent M` included, because here the question is "are these the same
/// record?", not "do the two directions of an edge agree?".
///
/// Its only job is to tell a §8.2 re-delivery (identical bytes, so identical digest) from a second
/// record that merely shares an id. A 64-bit fold makes a false "same" a 2⁻⁶⁴ event on a comparison
/// that is only ever made between two records already known to share an id and a coordinate.
fn record_digest(node: &NavNodeRef<'_>) -> u64 {
    #[inline]
    fn fold(h: u64, v: u64) -> u64 {
        (h ^ v).wrapping_mul(0x0000_0100_0000_01b3)
    }
    let mut h = fold(0xcbf2_9ce4_8422_2325, ((node.lat as u32 as u64) << 32) | node.lon as u32 as u64);
    h = fold(h, node.degree() as u64);
    for n in node.neighbors() {
        h = fold(h, ((n.id as u64) << 32) | n.edge_id as u64);
        h = fold(h, ((n.lat as u32 as u64) << 32) | n.lon as u32 as u64);
        h = fold(h, ((n.cost_m as u64) << 32) | ((n.way_kind as u64) << 16) | n.ascent_m as u64);
    }
    h
}

/// One adjacency entry's claim on its edge, as a spillable record:
/// `(edge id, the claiming junction, cost m, way kind, the claimant's coordinate)` — everything
/// §4.8.4's edge checks still need once the walk has gone by.
///
/// The claimant's **coordinate travels with the claim**, which is the change that makes the banded
/// table work: the edge-decode pass reads the claim stream in edge order, and edge ids have nothing
/// to do with node ids, so looking the coordinate up would have been a random access into a table
/// that is only partly resident. It is the same value — a claim is written while its claimant's own
/// record is in hand, and `(node.lat, node.lon)` is by definition what the junction table stored for
/// that id (the duplicate check above is what proves there is only one such pair).
///
/// The fields are laid out **big-endian, in key order**, so the byte-lexicographic comparator
/// [`by_claim`] *is* the `(edge_id, from, cost, kind)` ordering the in-memory tuple sort had. The
/// coordinate sits last, where it can never change an ordering: it is a function of `from`.
///
/// 21 bytes, packed — a spilled record has no alignment to serve, where the old in-memory tuple paid
/// 3 bytes of padding on every one of `2 × E`.
///
/// `Ascent M` is deliberately absent: §8.3 makes it the one adjacency field the two directions of an
/// edge legitimately disagree about, so it is not a claim about the edge.
const CLAIM_LEN: usize = 21;

fn claim(edge_id: u32, from: u32, cost: u32, kind: u8, lat: i32, lon: i32) -> [u8; CLAIM_LEN] {
    let mut r = [0u8; CLAIM_LEN];
    r[0..4].copy_from_slice(&edge_id.to_be_bytes());
    r[4..8].copy_from_slice(&from.to_be_bytes());
    r[8..12].copy_from_slice(&cost.to_be_bytes());
    r[12] = kind;
    // Biased so the byte order is the signed order — the coordinate never decides a comparison, but
    // an encoding that only sorts correctly for positive values is a trap for whoever keys on it
    // next.
    r[13..17].copy_from_slice(&((lat as u32) ^ 0x8000_0000).to_be_bytes());
    r[17..21].copy_from_slice(&((lon as u32) ^ 0x8000_0000).to_be_bytes());
    r
}

fn be32(r: &[u8; CLAIM_LEN], at: usize) -> u32 {
    u32::from_be_bytes(r[at..at + 4].try_into().expect("4 bytes"))
}

fn claim_edge(r: &[u8; CLAIM_LEN]) -> u32 {
    be32(r, 0)
}

fn claim_from(r: &[u8; CLAIM_LEN]) -> u32 {
    be32(r, 4)
}

/// The `(cost m, way kind)` pair both directions of an edge must agree on.
fn claim_agreement(r: &[u8; CLAIM_LEN]) -> (u32, u8) {
    (be32(r, 8), r[12])
}

/// The claimant's `(lat, lon)` in absolute µdeg.
fn claim_coord(r: &[u8; CLAIM_LEN]) -> (i32, i32) {
    ((be32(r, 13) ^ 0x8000_0000) as i32, (be32(r, 17) ^ 0x8000_0000) as i32)
}

/// Claims order by their bytes, which is `(edge_id, from, cost, kind)` — see [`claim`].
fn by_claim(a: &[u8; CLAIM_LEN], b: &[u8; CLAIM_LEN]) -> Ordering {
    a.cmp(b)
}

/// A bitmap over dense node ids: `words(n)` `u64`s cover ids `0..n`.
fn words(n: usize) -> usize {
    n.div_ceil(64)
}

/// Set the bit for `id` and report whether it was already set.
fn mark(bits: &mut [u64], id: u32) -> bool {
    let (word, bit) = (id as usize / 64, 1u64 << (id % 64));
    let was = bits[word] & bit != 0;
    bits[word] |= bit;
    was
}

/// A root's marker bit in the union-find: `parent[i] & ROOT != 0` means *i is a root* and the low
/// 31 bits are its component's size.
///
/// Carrying the size in the root's own slot is what removes the second whole-map `Vec` — the old
/// pass allocated a `counts: Vec<u32>` of one entry per junction and filled it with a `find` per id
/// purely to answer "how big is the largest component". Sizes and component counts are structural,
/// so which junction ends up as a representative (this is union by size; the old one was
/// last-wins) cannot move `largest_component_permille` by a single per mille.
///
/// The bit is free: §8.3 ids are `uint32` and the id ceiling derived below is at most
/// `FILE_CEILING / NAV_NODE_FIXED_LEN` ≈ 330 M, so no legal graph reaches 2³¹ junctions —
/// [`verify_nav`] refuses one that claims to rather than aliasing the flag.
const ROOT: u32 = 1 << 31;

/// Union-find root of `x`, with path halving. `parent` is indexed by `Node Id` **directly** — the
/// dense-id invariant is what removes the id → slot map the old pass had to build.
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] & ROOT == 0 {
        let p = parent[x as usize];
        let grandparent = parent[p as usize];
        if grandparent & ROOT != 0 {
            return p;
        }
        parent[x as usize] = grandparent;
        x = grandparent;
    }
    x
}

/// Union by size, so the size the root carries stays the component's.
fn union(parent: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a == b {
        return;
    }
    let (sa, sb) = (parent[a as usize] & !ROOT, parent[b as usize] & !ROOT);
    let (keep, folded) = if sa >= sb { (a, b) } else { (b, a) };
    parent[folded as usize] = keep;
    parent[keep as usize] = ROOT | (sa + sb);
}

/// The share of the budget the **claim sort** gets, as a divisor — on top of the band's, not out of
/// it, so the pass's ceiling is `budget + budget / CLAIM_SHARE`.
///
/// An eighth, and additive, because the two structures degrade in different currencies. The sort is
/// a stream: a smaller buffer buys more runs and a deeper k-way merge, which is a fraction of a
/// second either way. The band is random access: a smaller band buys another **whole walk of the nav
/// section**, and §8.2 delivers each record ~2.3× at country scale, so a band one junction short of
/// the graph costs seconds. Carving the sort's share out of the band's would put that cliff at 7/8
/// of the budget instead of at the budget, to save an eighth of the peak — the wrong trade in both
/// directions.
const CLAIM_SHARE: usize = 8;

/// Bytes one band spends per junction: `(lat, lon)` plus the record digest.
const BAND_BYTES_PER_NODE: usize = 16;

/// §4.8.4/§4.8.5: every neighbour resolves, degrees are capped, every `Edge Id` decodes to a record
/// whose endpoints are the two junctions' coordinates, both directions agree — then the component
/// histogram, as a report.
///
/// **Two walks per band, no resident graph.** The old shape held one hash entry per node, a 28-byte
/// arc tuple per adjacency entry, a hash entry *and* a heap `Vec` per edge, and a second id map for
/// the union-find — which made this the assembler's peak phase at country scale (#1116). C5 replaced
/// that with dense arrays; D5 bounds the arrays. The nav section is on disk by the time verify runs,
/// so re-reading it is the cheap side of every trade here:
///
/// 1. **Walk 1** fills one band of the [`NodeTable`] — coordinates, a [`record_digest`] and a seen
///    bit per in-band id — while counting *every* delivered record's degree against the cap and
///    every id against the section's capacity. Band 0's walk is also what learns the graph's
///    junction count, since §8.1's `node_count` is the quadtree index's size and says nothing about
///    the graph.
/// 2. **Walk 2** re-reads the same records and checks each adjacency entry *streaming*: the
///    neighbour resolves (against the junction count — density makes that a bounds test), and, for
///    the neighbours this band holds, the entry's `int16` deltas reconstruct the coordinate that
///    neighbour's own record states. §8.2's bin packing delivers a record once per leaf that shares
///    its chunk (17.5 M deliveries for BW's 3.0 M junctions), and walk 1 has already proved every
///    repeat is the *same* record, so a `done` bitmap lets this walk process each junction once.
///
/// Band 0's walk 2 additionally feeds the union-find and emits one [`claim`] per adjacency entry
/// into an [`ExternalSort`]; later bands re-check only the coordinates they now hold, so the claim
/// stream stays the graph's size however many bands there are. The sorted claims are grouped by edge
/// id, which replaces the old per-edge `Vec` with a run of one stream and makes the edge decodes
/// happen in id order (so a failure reports the same edge every run, where the old `HashMap`
/// iteration did not).
fn verify_nav(
    reader: &Reader<'_>,
    view: &BBox,
    report: &mut VerifyReport,
    scratch: &dyn ScratchStore,
    budget: usize,
) -> Result<()> {
    let dir = *reader.nav_directory();
    let mut chunk = vec![0u8; NAV_MAX_CHUNK_BYTES];

    // A §8.3 record is at least `NAV_NODE_FIXED_LEN` bytes (a degree-0 junction), so the node
    // chunks' capacity bounds how many junctions the section can hold — and the ids being dense
    // makes that a bound on the ids too. Deriving the ceiling from the directory rather than
    // trusting the record is what keeps a corrupt id from naming a multi-gigabyte allocation.
    let ceiling = dir.chunk_count.saturating_mul(dir.chunk_size) / NAV_NODE_FIXED_LEN;

    // The band gets the **whole** budget and the claim sort an eighth on top, rather than the two
    // dividing it. That is deliberate: what the sort's share buys is a shallower k-way merge (a
    // fraction of a second either way at country scale), and what the band's buys is *not walking
    // the section again*. Trading a ninth of the peak for a hard edge at exactly the budget would be
    // paying seconds to save megabytes.
    let claim_budget = (budget / CLAIM_SHARE).max(4 * CLAIM_LEN);
    let span = (budget / BAND_BYTES_PER_NODE).max(1);

    let mut total = 0usize;
    let mut parent: Vec<u32> = Vec::new();
    let mut lo = 0usize;
    while lo == 0 || lo < total {
        let first = lo == 0;

        // --- Walk 1: this band of the junction table; on band 0, the global §8.3 invariants. -----
        let mut nodes = NodeTable::new(lo, span, ceiling);
        let mut over_cap = 0usize;
        reader
            .for_each_nav_node(view, &mut chunk, |node| {
                if first && node.degree() > NAV_MAX_DEGREE {
                    over_cap += 1;
                }
                nodes.see(node.id, node.lat, node.lon, record_digest(&node));
            })
            .map_err(|e| Error::Verify(format!("the nav walk failed: {e:?}")))?;
        if over_cap > 0 {
            return Err(Error::Verify(format!(
                "{over_cap} junction(s) exceed the §8.3 degree cap of {NAV_MAX_DEGREE}"
            )));
        }
        if first {
            total = nodes.total();
            if total >= ROOT as usize {
                return Err(Error::Verify(format!(
                    "the graph claims {total} junctions, past the {ROOT} the §4.8.5 component pass can represent"
                )));
            }
            report.nav_nodes = total as u64;
            // Every junction starts as its own component of size one — the encoding [`ROOT`]
            // describes.
            parent = vec![ROOT | 1; total];
        }
        let nodes = nodes.finish(total)?;

        // --- Walk 2: the adjacency checks, streaming; on band 0, the union-find and the claims. --
        let mut done: Vec<u64> = vec![0; words(total)];
        let mut claims = first.then(|| ExternalSort::<CLAIM_LEN>::new(scratch, claim_budget, by_claim));
        let mut fault: Option<String> = None;
        let mut spill: Option<Error> = None;
        reader
            .for_each_nav_node(view, &mut chunk, |node| {
                if fault.is_some() || spill.is_some() || mark(&mut done, node.id) {
                    return;
                }
                for n in node.neighbors() {
                    // Density (checked band by band) makes "every neighbour resolves" a bounds test
                    // against the junction count, so it is answered on band 0 for *every* neighbour
                    // rather than deferred to whichever band happens to hold it.
                    if first && n.id as usize >= total {
                        fault =
                            Some(format!("neighbour id {} of node {} resolves to no record (§4.8.4)", n.id, node.id));
                        return;
                    }
                    if let Some(coord) = nodes.get(n.id) {
                        if coord != (n.lat, n.lon) {
                            fault = Some(format!(
                                "node {}'s int16 delta reconstructs neighbour {} at {:?}, but its record says \
                                 {coord:?}",
                                node.id,
                                n.id,
                                (n.lat, n.lon)
                            ));
                            return;
                        }
                    }
                    if let Some(sort) = claims.as_mut() {
                        union(&mut parent, node.id, n.id);
                        if let Err(e) = sort.push(claim(n.edge_id, node.id, n.cost_m, n.way_kind, node.lat, node.lon)) {
                            spill = Some(e);
                            return;
                        }
                    }
                }
            })
            .map_err(|e| Error::Verify(format!("the nav walk failed: {e:?}")))?;
        if fault.is_some() || spill.is_some() {
            // Hand the sort its runs back before refusing. `ExternalSort::finish` is what owns them
            // — the stream it returns deletes them as it drops — so abandoning the sort where it
            // stands would leave a spill behind on a host that is about to be told the map is
            // broken. A refusal must cost the host nothing but the message.
            if let Some(sort) = claims.take() {
                drop(sort.finish());
            }
            return Err(spill.unwrap_or_else(|| Error::Verify(fault.expect("a fault or a spill failure"))));
        }
        // The band and the delivery bitmap are dead the moment the walk ends, and what comes next
        // wants their bytes.
        drop(nodes);
        drop(done);

        if let Some(sort) = claims {
            // §4.8.5's histogram, while the union-find is still hot and before the claim stream's
            // buffers are allocated. A selection whose largest component covers an implausibly small
            // share of the graph is what a broken seam looks like — surfaced, never silently
            // repaired. The size in each root's slot is the whole answer: no second array, and no
            // pass over the ids to find which roots exist.
            report.components = parent.iter().filter(|p| *p & ROOT != 0).count() as u64;
            let largest = parent.iter().filter(|p| *p & ROOT != 0).map(|p| p & !ROOT).max().unwrap_or(0) as u64;
            report.largest_component_permille = (largest * 1000 / total.max(1) as u64) as u32;
            parent = Vec::new();

            check_edges(reader, sort, report)?;
        }
        lo += span;
    }
    Ok(())
}

/// §4.8.4's edge half, in **one pass over the sorted claim stream**: both directions agree, every
/// `Edge Id` decodes, and the record's polyline ends at the junctions that claim it.
///
/// The dedup is the old pass's `adjacency.sort_unstable(); adjacency.dedup()` in its new clothes:
/// the stream is sorted, so equal records are adjacent. §8.2's re-deliveries are already gone (the
/// `done` bitmap), so what it removes is the one case that survives — a junction with two neighbours
/// over a single edge id at the same cost and kind. The old pass pushed the very same claim twice
/// into that edge's list and checked it twice with the same answer, so collapsing the two changes no
/// verdict.
///
/// **The one ordering the in-memory pass had and this one does not**: it ran the whole `(Cost M, Way
/// Kind)` agreement check before any decode, so a map with *both* a disagreeing edge and an
/// undecodable one always named the disagreement. Grouped streaming names whichever comes first by
/// edge id. Both are refusals, neither check is weakened, and every claim is still checked — the
/// price of that ordering was writing the deduped `2 × E` records out and reading them back, which
/// measured at ~1 s of Baden-Württemberg's verify and would be several at DACH. It is given up in
/// the *message* a broken map gets, never in whether it is refused.
fn check_edges(reader: &Reader<'_>, sort: ExternalSort<'_, CLAIM_LEN>, report: &mut VerifyReport) -> Result<()> {
    let mut points: heapless::Vec<(i32, i32), MAX_EDGE_PTS> = heapless::Vec::new();
    let mut previous: Option<[u8; CLAIM_LEN]> = None;
    let mut open: Option<Group> = None;
    let mut edges = 0u64;
    for record in sort.finish()? {
        let record = record?;
        if previous == Some(record) {
            continue;
        }
        previous = Some(record);
        let edge_id = claim_edge(&record);
        match open {
            // Both directions of an edge must agree on `Cost M` and `Way Kind` (§8.3).
            Some(g) if g.edge_id == edge_id => g.check_agreement(&record)?,
            _ => {
                // A group's `Cost M` is checked against the §8.4 record's `length_m` once the last
                // of its claims has gone by — the order the in-memory pass used, where every
                // endpoint of an edge was checked before that edge's length.
                if let Some(g) = open.take() {
                    g.check_length()?;
                }
                let length = reader
                    .nav_edge(edge_id, &mut points)
                    .ok_or_else(|| Error::Verify(format!("edge {edge_id} does not decode (§4.8.4)")))?;
                let first =
                    *points.first().ok_or_else(|| Error::Verify(format!("edge {edge_id} decodes to nothing")))?;
                let last = *points.last().expect("a non-empty polyline has a last vertex");
                open = Some(Group { edge_id, length, agreement: claim_agreement(&record), first, last });
                edges += 1;
            }
        }
        open.expect("the group is open").check_endpoint(&record)?;
    }
    if let Some(g) = open {
        g.check_length()?;
    }
    report.nav_edges = edges;
    Ok(())
}

/// One edge's decoded record, held open across the run of claims that name it.
#[derive(Clone, Copy)]
struct Group {
    edge_id: u32,
    /// The `length_m` the §8.4 record states.
    length: u32,
    /// The `(Cost M, Way Kind)` the group's first claim states; every later one must match.
    agreement: (u32, u8),
    /// The polyline's ends, in the reader's `(lon, lat)` order.
    first: (i32, i32),
    last: (i32, i32),
}

impl Group {
    fn check_agreement(&self, record: &[u8; CLAIM_LEN]) -> Result<()> {
        if claim_agreement(record) != self.agreement {
            return Err(Error::Verify(format!(
                "edge {} is written with two different (cost, kind) pairs — the two directions disagree",
                self.edge_id
            )));
        }
        Ok(())
    }

    /// The polyline runs from endpoint `a` to endpoint `b` inclusive, so each claimant's stored
    /// coordinate must be one of its ends. The claim carries `(lat, lon)` and the reader hands
    /// polyline vertices back as `(lon, lat)`, so the comparison is made in the reader's order.
    fn check_endpoint(&self, record: &[u8; CLAIM_LEN]) -> Result<()> {
        let coord = claim_coord(record);
        let want = (coord.1, coord.0);
        if self.first != want && self.last != want {
            return Err(Error::Verify(format!(
                "edge {} does not end at node {}'s coordinate {coord:?} (§4.8.4)",
                self.edge_id,
                claim_from(record)
            )));
        }
        Ok(())
    }

    fn check_length(&self) -> Result<()> {
        if self.length != self.agreement.0 {
            return Err(Error::Verify(format!(
                "edge {} records {} m but its adjacency entries say {} m",
                self.edge_id, self.length, self.agreement.0
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dense table is what makes the whole pass cheap, and it is also where the *new* refusals
    /// live. Constructing a shard that trips them would mean hand-forging §8.3 bytes inside an
    /// otherwise valid OBCM — the end-to-end path is covered by `tests/oracle.rs`, so the refusals
    /// are pinned here, at the table.
    ///
    /// Records are `(id, lat, lon, digest)`; [`digested`] fills in the digest the way a real record
    /// would, i.e. equal content ⇒ equal digest. One band wide enough for everything, which is the
    /// shape every graph that fits the budget takes.
    fn table(ceiling: usize, records: &[(u32, i32, i32, u64)]) -> Result<NodeTable> {
        band(0, usize::MAX / 2, ceiling, records)
    }

    /// …and one explicit band of it, for the multi-band shape.
    fn band(lo: usize, span: usize, ceiling: usize, records: &[(u32, i32, i32, u64)]) -> Result<NodeTable> {
        let mut t = NodeTable::new(lo, span, ceiling);
        for &(id, lat, lon, digest) in records {
            t.see(id, lat, lon, digest);
        }
        let total = t.total();
        t.finish(total)
    }

    /// `(id, lat, lon)` records whose digest follows their coordinates, as a real record's would.
    fn digested(records: &[(u32, i32, i32)]) -> Vec<(u32, i32, i32, u64)> {
        records.iter().map(|&(id, lat, lon)| (id, lat, lon, ((lat as u32 as u64) << 32) | lon as u32 as u64)).collect()
    }

    #[test]
    fn dense_records_are_accepted_in_any_order() {
        let t = table(8, &digested(&[(2, 20, 21), (0, 0, 1), (1, 10, 11)])).expect("dense ids");
        assert_eq!(t.total(), 3);
        assert_eq!(t.get(1), Some((10, 11)));
        assert_eq!(t.get(3), None);
    }

    /// A band holds its own slice of the numbering and nothing else, but it still *counts* every
    /// record — the junction total is a global fact and only band 0's walk is asked for it.
    #[test]
    fn a_band_holds_its_own_ids_and_counts_the_rest() {
        let records = digested(&[(0, 0, 1), (1, 10, 11), (2, 20, 21), (3, 30, 31), (4, 40, 41)]);
        let low = band(0, 2, 8, &records).expect("ids 0..2");
        assert_eq!(low.total(), 5, "the walk sees the whole numbering whichever band it is filling");
        assert_eq!(low.get(1), Some((10, 11)));
        assert_eq!(low.get(2), None, "id 2 is the next band's");

        let high = band(4, 2, 8, &records).expect("ids 4..6");
        assert_eq!(high.get(4), Some((40, 41)));
        assert_eq!(high.get(3), None);
        assert_eq!(high.get(5), None, "past the highest id, inside the band");
    }

    /// The density check is per band, so a hole above band 0 is refused by the band that holds it —
    /// the failure a single table truncated at the budget would have lost.
    #[test]
    fn a_hole_above_the_first_band_is_still_refused() {
        let records = digested(&[(0, 0, 1), (1, 10, 11), (3, 30, 31)]);
        band(0, 2, 8, &records).expect("band 0 is dense");
        let err = band(2, 2, 8, &records).expect_err("id 2 has no record");
        assert!(format!("{err}").contains("not dense"), "{err}");
        assert!(format!("{err}").contains("1 of the 4 ids"), "{err}");
    }

    /// §8.2's bin packing re-delivers a record when two leaves share a chunk. Same bytes, same
    /// answer — the pass must not care.
    #[test]
    fn a_re_delivered_record_is_not_a_duplicate() {
        let t = table(8, &digested(&[(0, 0, 1), (1, 10, 11), (0, 0, 1)])).expect("an idempotent re-delivery");
        assert_eq!(t.total(), 2);
    }

    #[test]
    fn one_id_with_two_coordinates_is_refused() {
        let err = table(8, &digested(&[(0, 0, 1), (0, 5, 5)])).expect_err("two junctions under one id");
        assert!(format!("{err}").contains("two §8.3 records with different coordinates"), "{err}");
    }

    /// Walk 2 skips a re-delivered record, which is only sound because a record that differs in
    /// anything *but* its coordinates is refused here.
    #[test]
    fn one_id_with_two_adjacency_lists_is_refused() {
        let err = table(8, &[(0, 0, 1, 0xaaaa), (0, 0, 1, 0xbbbb)]).expect_err("two records under one id");
        assert!(format!("{err}").contains("different adjacency"), "{err}");
    }

    /// The band's arrays may never reach past one growth step above the highest id they have been
    /// asked to hold, whatever order the ids arrive in. This is the property the mem-profile numbers
    /// rest on: `Vec`'s own doubling left the table in capacity for 4.19 M junctions where 2.99 M
    /// had records, which was 46 MiB of air inside a 64 MiB budget.
    #[test]
    fn a_band_never_reaches_more_than_one_growth_step_past_its_highest_id() {
        let span = 1 << 16;
        let step = span / 16;
        for (name, ids) in [
            ("ascending", (0..span as u32).collect::<Vec<u32>>()),
            // The shape a real §8.2 walk produces: chunk-ordered, so the ids arrive scattered.
            ("scattered", (0..span as u32).map(|i| (i * 40_507) % span as u32).collect()),
        ] {
            let mut t = NodeTable::new(0, span, span);
            let mut highest = 0usize;
            for id in ids {
                t.see(id, id as i32, -(id as i32), id as u64);
                highest = highest.max(id as usize + 1);
                let bound = highest.next_multiple_of(step).min(span);
                assert!(
                    t.coords.capacity() <= bound,
                    "{name}: capacity {} past the {bound} one step above id {}",
                    t.coords.capacity(),
                    highest - 1
                );
                assert_eq!(t.digest.capacity(), t.coords.capacity(), "{name}: the two arrays grow together");
            }
            t.finish(span).expect("a full band is dense");
        }
        // …and the worst order there is — highest id first — still allocates the band once and not
        // twice, because the step rounds up rather than doubling.
        let mut t = NodeTable::new(0, span, span);
        t.see(span as u32 - 1, 0, 0, 0);
        assert_eq!(t.coords.capacity(), span, "one allocation of exactly the band");
    }

    /// New in #1116: the old pass hashed such an id and carried on.
    #[test]
    fn an_id_past_the_sections_capacity_is_refused() {
        let err = table(4, &digested(&[(0, 0, 1), (9, 9, 9)])).expect_err("an id no dense numbering could reach");
        assert!(format!("{err}").contains("out of the section's range"), "{err}");
    }

    /// New in #1116: a hole in the numbering means a junction the walk never saw.
    #[test]
    fn a_hole_in_the_numbering_is_refused() {
        let err = table(8, &digested(&[(0, 0, 1), (2, 20, 21)])).expect_err("id 1 has no record");
        assert!(format!("{err}").contains("not dense"), "{err}");
        assert!(format!("{err}").contains("1 of the 3 ids"), "{err}");
    }

    /// The first fault is the one raised, and the range check runs before the coordinate check —
    /// an out-of-range id must never reach the table's index arithmetic.
    #[test]
    fn the_first_fault_is_the_one_reported() {
        let err = table(4, &digested(&[(0, 0, 1), (99, 0, 0), (0, 7, 7)])).expect_err("both faults");
        assert!(format!("{err}").contains("out of the section's range"), "{err}");
    }

    #[test]
    fn an_empty_graph_is_dense() {
        let t = table(0, &[]).expect("no records");
        assert_eq!(t.total(), 0);
    }

    /// The claim stream is grouped by scanning runs of equal `edge_id`, so the external sort must
    /// put an edge's claims together and the dedup must leave the distinct set — which is only true
    /// if the byte layout's order *is* the `(edge_id, from, cost, kind)` order the tuple sort had.
    #[test]
    fn sorting_groups_the_claims_by_edge_and_dedup_leaves_the_distinct_set() {
        let rows = [(7, 1, 30, 2), (3, 9, 10, 1), (7, 1, 30, 2), (3, 2, 10, 1)];
        let mut claims: Vec<[u8; CLAIM_LEN]> =
            rows.iter().map(|&(e, f, c, k)| claim(e, f, c, k, -1_000 - f as i32, 2_000)).collect();
        claims.sort_by(by_claim);
        claims.dedup();
        let got: Vec<(u32, u32, u32, u8)> =
            claims.iter().map(|r| (claim_edge(r), claim_from(r), claim_agreement(r).0, claim_agreement(r).1)).collect();
        assert_eq!(got, vec![(3, 2, 10, 1), (3, 9, 10, 1), (7, 1, 30, 2)]);
        // …and the coordinate rides along untouched, negative µdeg included.
        assert_eq!(claim_coord(&claims[0]), (-1_002, 2_000));
    }

    /// The whole claim record round-trips, at the extremes of every field — a mis-sized slice in
    /// [`claim`] would otherwise show up as a mystery verify failure on one map in a thousand.
    #[test]
    fn a_claim_round_trips_at_the_extremes() {
        let r = claim(u32::MAX, u32::MAX - 1, u32::MAX, 0xff, i32::MIN, i32::MAX);
        assert_eq!(claim_edge(&r), u32::MAX);
        assert_eq!(claim_from(&r), u32::MAX - 1);
        assert_eq!(claim_agreement(&r), (u32::MAX, 0xff));
        assert_eq!(claim_coord(&r), (i32::MIN, i32::MAX));
        assert_eq!(by_claim(&claim(1, 0, 0, 0, 0, 0), &claim(2, 0, 0, 0, 0, 0)), Ordering::Less);
        assert_eq!(by_claim(&claim(1, 0, 0, 0, i32::MAX, 0), &claim(1, 0, 0, 0, i32::MIN, 0)), Ordering::Greater);
    }

    /// The `done` bitmap is what makes walk 2 one pass per junction rather than one per delivery.
    #[test]
    fn the_done_bitmap_reports_a_repeat() {
        let mut bits = vec![0u64; words(130)];
        assert_eq!(words(130), 3);
        assert!(!mark(&mut bits, 0));
        assert!(!mark(&mut bits, 129));
        assert!(mark(&mut bits, 0));
        assert!(mark(&mut bits, 129));
        assert!(!mark(&mut bits, 64));
    }

    /// Component count and largest size, straight out of the parent array — what
    /// [`verify_nav`] reports.
    fn histogram(parent: &[u32]) -> (usize, u32) {
        let roots = parent.iter().filter(|p| *p & ROOT != 0);
        (roots.clone().count(), roots.map(|p| p & !ROOT).max().unwrap_or(0))
    }

    #[test]
    fn union_find_over_dense_ids() {
        let mut parent: Vec<u32> = vec![ROOT | 1; 5];
        union(&mut parent, 0, 1);
        union(&mut parent, 1, 2);
        union(&mut parent, 3, 4);
        let roots: Vec<u32> = (0..5).map(|i| find(&mut parent, i)).collect();
        assert_eq!(roots[0], roots[1]);
        assert_eq!(roots[1], roots[2]);
        assert_eq!(roots[3], roots[4]);
        assert_ne!(roots[0], roots[3]);
        assert_eq!(histogram(&parent), (2, 3), "two components, the larger of three");
    }

    /// The size in the root's slot is the component histogram, and it must answer the same as the
    /// `counts`-array pass it replaces — over a chain long enough that path halving actually runs,
    /// and with the unions handed over in an order that makes both by-size branches fire.
    #[test]
    fn the_size_in_the_root_is_the_histogram_the_counts_array_gave() {
        for order in [true, false] {
            let n = 200usize;
            let mut parent: Vec<u32> = vec![ROOT | 1; n];
            // Ids 0..150 form a chain; 150..170 form a second one; the rest stay isolated.
            let mut links: Vec<(u32, u32)> =
                (1..150u32).map(|i| (i - 1, i)).chain((151..170u32).map(|i| (i - 1, i))).collect();
            if !order {
                links.reverse();
            }
            for (a, b) in links {
                union(&mut parent, a, b);
            }
            // The old formulation, verbatim.
            let mut counts = vec![0u32; n];
            for id in 0..n as u32 {
                counts[find(&mut parent, id) as usize] += 1;
            }
            let want = (counts.iter().filter(|&&c| c > 0).count(), counts.iter().copied().max().unwrap_or(0));
            assert_eq!(histogram(&parent), want, "the two formulations disagree");
            assert_eq!(want, (1 + 1 + 30, 150), "the fixture is the graph it says it is");
        }
    }
}
