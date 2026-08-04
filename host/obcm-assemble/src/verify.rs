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
//! [`verify_nav`] walks the nav section twice over dense arrays instead of once over a resident arc
//! list, and re-reading a section that is already on disk is the cheaper half of that trade.

use obc_formats::io::ByteSource;
use obc_formats::obcm::{NAV_CHUNK_SIZE, NAV_EDGE_FIXED_LEN, NAV_MAX_DEGREE, NAV_NODE_FIXED_LEN};
use obc_map_scene::BBox;
use obc_reader::{MapCache, MapTables, NavNodeRef, Reader, MAX_FEAT_PTS, MAX_FEAT_RINGS, NAV_MAX_CHUNK_BYTES};

use crate::grid::AlignedBox;
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
pub fn verify_shard(src: &dyn ByteSource, expected_box: AlignedBox, expect_sections: bool) -> Result<VerifyReport> {
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
        verify_nav(&reader, &view, &mut report)?;
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

/// The junction table: **dense**, because `OBCM_Spec.md` §8.3 says node ids are file-local and
/// dense and `OBCA_Spec.md` §4.6.5's renumbering is what writes them that way. Indexing a `Vec` by
/// id is what lets this pass hold eight bytes per junction instead of a hash entry, and it turns
/// §4.8.4's "every neighbour resolves" into a bounds test.
///
/// The seen set is a bitmap and not a `Vec<bool>` because it is one of the structures that has to
/// survive alongside coords, the digests, the union-find parents and the claim list at the peak: at
/// Baden-Württemberg's ~3 M junctions it is 366 KiB rather than 2.9 MiB, and "how many ids have a
/// record" is then a popcount rather than a scan.
///
/// Faults are **recorded, not raised**: the walk that fills this is a `FnMut` callback with no error
/// channel, and bailing out of it early would under-count the degree-cap tally the caller reports
/// first. The first fault wins, which is the order a `?` would have produced anyway.
#[derive(Debug, Default)]
struct NodeTable {
    /// `(lat, lon)` per id, in the records' own order — absolute µdeg, as §8.3 stores them.
    coords: Vec<(i32, i32)>,
    /// [`record_digest`] of the record that claimed each id, so a re-delivery can be told from a
    /// second, *different* record wearing the same id.
    digest: Vec<u64>,
    /// One bit per id: "a §8.3 record for this id was seen".
    seen: Vec<u64>,
    /// Popcount of `seen`, maintained as it fills.
    count: usize,
    /// The first id this section cannot possibly dense-number (see [`verify_nav`]).
    ceiling: usize,
    fault: Option<String>,
}

impl NodeTable {
    fn new(ceiling: usize) -> Self {
        NodeTable { ceiling, ..Default::default() }
    }

    /// Record one §8.3 junction.
    ///
    /// §8.2's bin packing can hand the same record back more than once, so a repeat carrying the
    /// same content is accepted — that is the idempotence every consumer of these records owes the
    /// format, and it is what the old pass's `HashMap::insert` accepted. A repeat that *disagrees*
    /// is two different junctions wearing one id, and is a verify failure: the coordinates get their
    /// own message because they are what the rest of the pass indexes by, and any other difference
    /// is caught by the digest.
    ///
    /// Proving a repeat is a repeat is what lets the second walk process each junction exactly once
    /// (§8.2 delivered 17.5 M records for BW's 3.0 M junctions), and the strictness is why that is
    /// not a loss of checking: the old pass re-checked a re-delivered record's adjacency, this one
    /// refuses the only case where re-checking could have found anything.
    fn see(&mut self, id: u32, lat: i32, lon: i32, digest: u64) {
        let i = id as usize;
        if i >= self.ceiling {
            // Refused before it is used to size anything: `coords` is indexed by id, so a corrupt
            // id must not be allowed to name an allocation.
            self.fail(format!(
                "node id {id} is out of the section's range: its node chunks cannot hold more than {} dense §8.3 \
                 records (OBCM §8.3)",
                self.ceiling
            ));
            return;
        }
        if i >= self.coords.len() {
            self.coords.resize(i + 1, (0, 0));
            self.digest.resize(self.coords.len(), 0);
            self.seen.resize(self.coords.len().div_ceil(64), 0);
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

    fn fail(&mut self, msg: String) {
        if self.fault.is_none() {
            self.fault = Some(msg);
        }
    }

    /// The recorded fault, then the density check.
    ///
    /// §8.3's ids are dense, so every id below the highest one seen must have a record. The old pass
    /// could not notice a gap — a hash map has no opinion about which keys are absent — and would
    /// have carried on as if the graph were simply smaller. A gap means the walk never saw a
    /// junction that some neighbour entry will point at, so §4.8.4's "every neighbour resolves" is
    /// only truly checkable once this holds.
    fn finish(mut self) -> Result<Self> {
        if let Some(msg) = self.fault.take() {
            return Err(Error::Verify(msg));
        }
        if self.count != self.coords.len() {
            return Err(Error::Verify(format!(
                "the graph's node ids are not dense: {} of the {} ids below the highest one have no §8.3 record \
                 (OBCM §8.3)",
                self.coords.len() - self.count,
                self.coords.len()
            )));
        }
        Ok(self)
    }

    fn len(&self) -> usize {
        self.coords.len()
    }

    /// The junction's `(lat, lon)`, or `None` if `id` names no record. After [`NodeTable::finish`]
    /// every id below `len` has one, so this is a pure bounds test.
    fn get(&self, id: u32) -> Option<(i32, i32)> {
        self.coords.get(id as usize).copied()
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

/// One adjacency entry's claim on its edge: `(edge id, the claiming junction, cost m, way kind)` —
/// everything §4.8.4's edge checks still need once the walk has gone by. The endpoint *coordinate*
/// is not carried, because [`NodeTable`] already answers that from the node id; 13 bytes of payload,
/// 16 with padding, against the old pass's 28-byte arc tuple plus its per-edge `Vec`.
///
/// `Ascent M` is deliberately absent: §8.3 makes it the one adjacency field the two directions of an
/// edge legitimately disagree about, so it is not a claim about the edge.
type Claim = (u32, u32, u32, u8);

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

/// Union-find root of `x`, with path halving. `parent` is indexed by `Node Id` **directly** — the
/// dense-id invariant is what removes the id → slot map the old pass had to build.
fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

fn union(parent: &mut [u32], a: u32, b: u32) {
    let (a, b) = (find(parent, a), find(parent, b));
    if a != b {
        parent[a as usize] = b;
    }
}

/// §4.8.4/§4.8.5: every neighbour resolves, degrees are capped, every `Edge Id` decodes to a record
/// whose endpoints are the two junctions' coordinates, both directions agree — then the component
/// histogram, as a report.
///
/// **Two walks, no resident graph.** The old shape held one hash entry per node, a 28-byte arc tuple
/// per adjacency entry, a hash entry *and* a heap `Vec` per edge, and a second id map for the
/// union-find — which made this the assembler's peak phase at country scale (#1116). The nav section
/// is on disk by the time verify runs, so re-reading it is nearly free, and reading it twice buys
/// the whole arc list away:
///
/// 1. **Walk 1** fills the dense [`NodeTable`] — coordinates, a [`record_digest`] and a seen bit per
///    id — and counts degree-cap violations.
/// 2. **Walk 2** re-reads the same records and checks each adjacency entry *streaming* — the
///    neighbour resolves, and the entry's `int16` deltas reconstruct the coordinate that
///    neighbour's own record states — then feeds the union-find and emits one 16-byte claim. §8.2's
///    bin packing delivers a record once per leaf that shares its chunk (17.5 M deliveries for BW's
///    3.0 M junctions), and walk 1 has already proved every repeat is the *same* record, so a
///    `done` bitmap lets this walk process each junction once. That is what keeps the claim list at
///    the graph's size rather than the delivery count.
///
/// The claims are then sorted and grouped by edge id, which replaces the per-edge `Vec` with a slice
/// of one array and makes the edge decodes happen in id order (so a failure reports the same edge
/// every run, where the old `HashMap` iteration did not).
fn verify_nav(reader: &Reader<'_>, view: &BBox, report: &mut VerifyReport) -> Result<()> {
    let dir = *reader.nav_directory();
    let mut scratch = vec![0u8; NAV_MAX_CHUNK_BYTES];

    // --- Walk 1: the junction table, the degree cap, the §8.3 id invariants. ---------------------
    // A §8.3 record is at least `NAV_NODE_FIXED_LEN` bytes (a degree-0 junction), so the node
    // chunks' capacity bounds how many junctions the section can hold — and the ids being dense
    // makes that a bound on the ids too. Deriving the ceiling from the directory rather than
    // trusting the record is what keeps a corrupt id from naming a multi-gigabyte allocation.
    let ceiling = dir.chunk_count.saturating_mul(dir.chunk_size) / NAV_NODE_FIXED_LEN;
    let mut nodes = NodeTable::new(ceiling);
    let mut over_cap = 0usize;
    reader
        .for_each_nav_node(view, &mut scratch, |node| {
            if node.degree() > NAV_MAX_DEGREE {
                over_cap += 1;
            }
            nodes.see(node.id, node.lat, node.lon, record_digest(&node));
        })
        .map_err(|e| Error::Verify(format!("the nav walk failed: {e:?}")))?;
    if over_cap > 0 {
        return Err(Error::Verify(format!("{over_cap} junction(s) exceed the §8.3 degree cap of {NAV_MAX_DEGREE}")));
    }
    let nodes = nodes.finish()?;
    report.nav_nodes = nodes.len() as u64;

    // --- Walk 2: the adjacency checks, streaming; the union-find; the edge claims. ---------------
    let mut claims: Vec<Claim> = Vec::new();
    let mut parent: Vec<u32> = (0..nodes.len() as u32).collect();
    let mut done: Vec<u64> = vec![0; words(nodes.len())];
    let mut fault: Option<String> = None;
    reader
        .for_each_nav_node(view, &mut scratch, |node| {
            if fault.is_some() || mark(&mut done, node.id) {
                return;
            }
            for n in node.neighbors() {
                let Some(coord) = nodes.get(n.id) else {
                    fault = Some(format!("neighbour id {} of node {} resolves to no record (§4.8.4)", n.id, node.id));
                    return;
                };
                if coord != (n.lat, n.lon) {
                    fault = Some(format!(
                        "node {}'s int16 delta reconstructs neighbour {} at {:?}, but its record says {coord:?}",
                        node.id,
                        n.id,
                        (n.lat, n.lon)
                    ));
                    return;
                }
                union(&mut parent, node.id, n.id);
                claims.push((n.edge_id, node.id, n.cost_m, n.way_kind));
            }
        })
        .map_err(|e| Error::Verify(format!("the nav walk failed: {e:?}")))?;
    if let Some(msg) = fault {
        return Err(Error::Verify(msg));
    }

    // Sorting by the whole tuple groups the claims by edge id and, within an edge, by claimant. The
    // `dedup` is the old pass's `adjacency.sort_unstable(); adjacency.dedup()` in its new clothes.
    // §8.2's re-deliveries are already gone (the `done` bitmap above), so what it removes is the one
    // case that survives: a junction with two neighbours over a single edge id at the same cost and
    // kind. The old pass pushed the very same `(from, coord)` pair twice into that edge's claim list
    // and checked it twice with the same answer, so collapsing the two changes no verdict.
    claims.sort_unstable();
    claims.dedup();

    // Both directions of an edge must agree on `Cost M` and `Way Kind` (§8.3). This is a pass of its
    // own, ahead of any decode, because that is the order the old pass raised these two failures in.
    let mut edges = 0u64;
    let mut i = 0usize;
    while i < claims.len() {
        let (edge_id, _, cost, kind) = claims[i];
        let mut j = i + 1;
        while j < claims.len() && claims[j].0 == edge_id {
            if (claims[j].2, claims[j].3) != (cost, kind) {
                return Err(Error::Verify(format!(
                    "edge {edge_id} is written with two different (cost, kind) pairs — the two directions disagree"
                )));
            }
            j += 1;
        }
        edges += 1;
        i = j;
    }
    report.nav_edges = edges;

    // Every `Edge Id` decodes, and to a record whose polyline ends at the junctions that claim it.
    let mut points: heapless::Vec<(i32, i32), MAX_EDGE_PTS> = heapless::Vec::new();
    let mut i = 0usize;
    while i < claims.len() {
        let (edge_id, _, cost, _) = claims[i];
        let mut j = i + 1;
        while j < claims.len() && claims[j].0 == edge_id {
            j += 1;
        }
        let length = reader
            .nav_edge(edge_id, &mut points)
            .ok_or_else(|| Error::Verify(format!("edge {edge_id} does not decode (§4.8.4)")))?;
        let first = *points.first().ok_or_else(|| Error::Verify(format!("edge {edge_id} decodes to nothing")))?;
        let last = *points.last().expect("a non-empty polyline has a last vertex");
        // The polyline runs from endpoint `a` to endpoint `b` inclusive, so each endpoint's stored
        // coordinate must be one of its ends.
        for &(_, node, ..) in &claims[i..j] {
            let coord = nodes.get(node).expect("walk 2 only claims junctions walk 1 saw");
            // The record's coordinates are (lat, lon); the reader hands polyline vertices back as
            // (lon, lat), so the comparison is made in the reader's order.
            let want = (coord.1, coord.0);
            if first != want && last != want {
                return Err(Error::Verify(format!(
                    "edge {edge_id} does not end at node {node}'s coordinate {coord:?} (§4.8.4)"
                )));
            }
        }
        if length != cost {
            return Err(Error::Verify(format!(
                "edge {edge_id} records {length} m but its adjacency entries say {cost} m"
            )));
        }
        i = j;
    }

    // §4.8.5: the component histogram, as a report. A selection whose largest component covers an
    // implausibly small share of the graph is what a broken seam looks like — surfaced, never
    // silently repaired. Dense ids make this a `Vec` of counts indexed by root: no map, and no
    // second pass to find which roots exist (a root with no members simply counts zero).
    let n = nodes.len();
    let mut counts = vec![0u32; n];
    for id in 0..n as u32 {
        counts[find(&mut parent, id) as usize] += 1;
    }
    report.components = counts.iter().filter(|&&c| c > 0).count() as u64;
    let largest = counts.iter().copied().max().unwrap_or(0) as u64;
    report.largest_component_permille = (largest * 1000 / n.max(1) as u64) as u32;
    Ok(())
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
    /// would, i.e. equal content ⇒ equal digest.
    fn table(ceiling: usize, records: &[(u32, i32, i32, u64)]) -> Result<NodeTable> {
        let mut t = NodeTable::new(ceiling);
        for &(id, lat, lon, digest) in records {
            t.see(id, lat, lon, digest);
        }
        t.finish()
    }

    /// `(id, lat, lon)` records whose digest follows their coordinates, as a real record's would.
    fn digested(records: &[(u32, i32, i32)]) -> Vec<(u32, i32, i32, u64)> {
        records.iter().map(|&(id, lat, lon)| (id, lat, lon, ((lat as u32 as u64) << 32) | lon as u32 as u64)).collect()
    }

    #[test]
    fn dense_records_are_accepted_in_any_order() {
        let t = table(8, &digested(&[(2, 20, 21), (0, 0, 1), (1, 10, 11)])).expect("dense ids");
        assert_eq!(t.len(), 3);
        assert_eq!(t.get(1), Some((10, 11)));
        assert_eq!(t.get(3), None);
    }

    /// §8.2's bin packing re-delivers a record when two leaves share a chunk. Same bytes, same
    /// answer — the pass must not care.
    #[test]
    fn a_re_delivered_record_is_not_a_duplicate() {
        let t = table(8, &digested(&[(0, 0, 1), (1, 10, 11), (0, 0, 1)])).expect("an idempotent re-delivery");
        assert_eq!(t.len(), 2);
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
        assert_eq!(t.len(), 0);
    }

    /// The claim list is grouped by scanning runs of equal `edge_id`, so the sort must put an edge's
    /// claims together and the dedup must leave the distinct set.
    #[test]
    fn sorting_groups_the_claims_by_edge_and_dedup_leaves_the_distinct_set() {
        let mut claims: Vec<Claim> = vec![(7, 1, 30, 2), (3, 9, 10, 1), (7, 1, 30, 2), (3, 2, 10, 1)];
        claims.sort_unstable();
        claims.dedup();
        assert_eq!(claims, vec![(3, 2, 10, 1), (3, 9, 10, 1), (7, 1, 30, 2)]);
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

    #[test]
    fn union_find_over_dense_ids() {
        let mut parent: Vec<u32> = (0..5).collect();
        union(&mut parent, 0, 1);
        union(&mut parent, 1, 2);
        union(&mut parent, 3, 4);
        let roots: Vec<u32> = (0..5).map(|i| find(&mut parent, i)).collect();
        assert_eq!(roots[0], roots[1]);
        assert_eq!(roots[1], roots[2]);
        assert_eq!(roots[3], roots[4]);
        assert_ne!(roots[0], roots[3]);
    }
}
