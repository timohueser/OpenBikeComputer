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

use std::collections::HashMap;

use obc_formats::io::ByteSource;
use obc_formats::obcm::{NAV_CHUNK_SIZE, NAV_EDGE_FIXED_LEN};
use obc_map_scene::BBox;
use obc_reader::{MapCache, MapTables, Reader, MAX_FEAT_PTS, MAX_FEAT_RINGS, NAV_MAX_CHUNK_BYTES};

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

/// §4.8.4/§4.8.5: every neighbour resolves, degrees are capped, every `Edge Id` decodes to a record
/// whose endpoints are the two junctions' coordinates, both directions agree — then the component
/// histogram, as a report.
fn verify_nav(reader: &Reader<'_>, view: &BBox, report: &mut VerifyReport) -> Result<()> {
    /// One decoded adjacency entry: `(node id, neighbour id, edge id, cost m, way kind, the
    /// neighbour's coordinate **as the record's int16 deltas reconstruct it**)`.
    type Arc = (u32, u32, u32, u32, u8, (i32, i32));
    let mut coords: HashMap<u32, (i32, i32)> = HashMap::new();
    let mut adjacency: Vec<Arc> = Vec::new();
    let mut over_cap = 0usize;
    let mut scratch = vec![0u8; NAV_MAX_CHUNK_BYTES];
    reader
        .for_each_nav_node(view, &mut scratch, |node| {
            // §8.2's bin packing can hand the same record back more than once; the collection is
            // idempotent, exactly as every consumer of these records must be.
            coords.insert(node.id, (node.lat, node.lon));
            if node.degree() > obc_formats::obcm::NAV_MAX_DEGREE {
                over_cap += 1;
            }
            for n in node.neighbors() {
                adjacency.push((node.id, n.id, n.edge_id, n.cost_m, n.way_kind, (n.lat, n.lon)));
            }
        })
        .map_err(|e| Error::Verify(format!("the nav walk failed: {e:?}")))?;
    if over_cap > 0 {
        return Err(Error::Verify(format!(
            "{over_cap} junction(s) exceed the §8.3 degree cap of {}",
            obc_formats::obcm::NAV_MAX_DEGREE
        )));
    }
    adjacency.sort_unstable();
    adjacency.dedup();
    report.nav_nodes = coords.len() as u64;

    // Both directions of an edge must agree on `Edge Id`, `Cost M` and `Way Kind`, and the id must
    // decode to a record whose first and last vertices are the two endpoints' coordinates.
    /// What one edge id must agree about across both of its directions: its cost, its way kind, and
    /// the endpoints (node id + coordinate) that claim it.
    type EdgeClaims = (u32, u8, Vec<(u32, (i32, i32))>);
    let mut by_edge: HashMap<u32, EdgeClaims> = HashMap::new();
    for &(from, to, edge_id, cost, kind, nbr_coord) in &adjacency {
        let coord = *coords
            .get(&to)
            .ok_or_else(|| Error::Verify(format!("neighbour id {to} of node {from} resolves to no record (§4.8.4)")))?;
        if coord != nbr_coord {
            return Err(Error::Verify(format!(
                "node {from}'s int16 delta reconstructs neighbour {to} at {nbr_coord:?}, but its record says {coord:?}"
            )));
        }
        let e = by_edge.entry(edge_id).or_insert((cost, kind, Vec::new()));
        if e.0 != cost || e.1 != kind {
            return Err(Error::Verify(format!(
                "edge {edge_id} is written with two different (cost, kind) pairs — the two directions disagree"
            )));
        }
        e.2.push((from, coords[&from]));
    }
    report.nav_edges = by_edge.len() as u64;
    let mut points: heapless::Vec<(i32, i32), MAX_EDGE_PTS> = heapless::Vec::new();
    for (edge_id, (cost, _, ends)) in &by_edge {
        let length = reader
            .nav_edge(*edge_id, &mut points)
            .ok_or_else(|| Error::Verify(format!("edge {edge_id} does not decode (§4.8.4)")))?;
        let first = *points.first().ok_or_else(|| Error::Verify(format!("edge {edge_id} decodes to nothing")))?;
        let last = *points.last().expect("a non-empty polyline has a last vertex");
        // The polyline runs from endpoint `a` to endpoint `b` inclusive, so each endpoint's stored
        // coordinate must be one of its ends.
        for (node, coord) in ends {
            // The record's coordinates are (lat, lon); the reader hands polyline vertices back as
            // (lon, lat), so the comparison is made in the reader's order.
            let want = (coord.1, coord.0);
            if first != want && last != want {
                return Err(Error::Verify(format!(
                    "edge {edge_id} does not end at node {node}'s coordinate {coord:?} (§4.8.4)"
                )));
            }
        }
        if length != *cost {
            return Err(Error::Verify(format!(
                "edge {edge_id} records {length} m but its adjacency entries say {cost} m"
            )));
        }
    }

    // §4.8.5: the component histogram, as a report. A selection whose largest component covers an
    // implausibly small share of the graph is what a broken seam looks like — surfaced, never
    // silently repaired.
    let ids: Vec<u32> = {
        let mut v: Vec<u32> = coords.keys().copied().collect();
        v.sort_unstable();
        v
    };
    let slot: HashMap<u32, usize> = ids.iter().enumerate().map(|(i, &id)| (id, i)).collect();
    let mut parent: Vec<usize> = (0..ids.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for &(from, to, ..) in &adjacency {
        let (a, b) = (find(&mut parent, slot[&from]), find(&mut parent, slot[&to]));
        if a != b {
            parent[a] = b;
        }
    }
    let mut sizes: HashMap<usize, u64> = HashMap::new();
    for i in 0..ids.len() {
        let r = find(&mut parent, i);
        *sizes.entry(r).or_insert(0) += 1;
    }
    report.components = sizes.len() as u64;
    let largest = sizes.values().copied().max().unwrap_or(0);
    report.largest_component_permille = (largest * 1000 / ids.len().max(1) as u64) as u32;
    Ok(())
}
