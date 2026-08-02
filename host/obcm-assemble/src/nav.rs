//! Merging the navigation graph
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §4.6) — the assembler's one O(rewrite) component.
//!
//! Nothing in `OBCM_Spec.md` §8 survives concatenation: node ids are file-local and dense, and an
//! `Edge Id` is a **pool byte offset**. So the graph is read back out of every `network` cell,
//! unified at the seams, pruned, spatially bounded, renumbered, and re-emitted. Short edge records
//! are copied verbatim. A surviving edge longer than the device's bounded-snap invariant is decoded
//! and split only *after* island pruning, so synthetic routing detail cannot inflate a component's
//! source edge count.
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
//!    deferred), 5. **split surviving long edges**, 6. **renumber**, 7. **rebuild the edge pool**,
//!    8. re-check the wire limits and rebuild the node quadtree.

use std::collections::{BTreeMap, HashMap};

use obc_formats::obcm::{
    CHUNK_END, NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_EDGE_FIXED_LEN, NAV_MAX_DEGREE, NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN,
    NAV_SNAP_EDGE_MAX_M,
};
use obc_map_scene::ground_dist_m;

use crate::grid::{on_grid_boundary, UBox};
use crate::input::Cell;
use crate::qtree::{self, Point};
use crate::{Error, Result};

/// Largest neighbour delta the `int16` fields hold (§8.3). The packer's own split bound is 32 000;
/// unification never moves a coordinate, so an input that held it still holds it — but §4.8 says
/// re-check rather than assume.
const MAX_NEIGHBOR_DELTA: i64 = i16::MAX as i64;

/// An edge on its way into the merged pool: unified endpoints plus its self-contained record.
struct MergedEdge {
    a: u32,
    b: u32,
    cost_m: u32,
    kind: u8,
    /// The §8.4 record — `length_m`, `pt_count`, `way_kind`, anchor and deltas. Usually exactly as
    /// its cell wrote it; post-prune spatial splits create new records for their pieces.
    rec: Vec<u8>,
    /// FNV-1a over `rec`, computed **once** when the edge is interned. It is the content half of
    /// both the §4.6.3 duplicate key and the §4.6.6 emission order, and re-hashing a 511-byte record
    /// inside an `O(n log n)` comparator was measurable on a country-scale graph.
    hash: u64,
}

/// One junction ready to serialize.
struct NavPoint {
    lat: i32,
    lon: i32,
    id: u32,
    neighbors: Vec<WireNeighbor>,
}

struct WireNeighbor {
    id: u32,
    lat: i32,
    lon: i32,
    edge_id: u32,
    cost_m: u32,
    way_kind: u8,
}

impl Point for NavPoint {
    fn lat(&self) -> i32 {
        self.lat
    }
    fn lon(&self) -> i32 {
        self.lon
    }
    fn record_len(&self) -> usize {
        NAV_NODE_FIXED_LEN + self.neighbors.len() * NAV_NEIGHBOR_LEN
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
    /// Synthetic degree-2 nodes added after pruning to enforce [`NAV_SNAP_EDGE_MAX_M`]. Kept
    /// separate from the source/prune counters so component-size accounting remains auditable.
    pub spatial_split_nodes: usize,
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

/// The merged graph, **already laid out**: the node quadtree, its bin-packed chunks and the edge
/// pool, all in the file's own encoding. Only the directory's absolute offsets are left, which is
/// what lets a shard's size be known before its header is written.
pub struct MergedNav {
    index: Vec<u8>,
    node_count: u32,
    chunks: Vec<u8>,
    chunk_count: u32,
    /// Edge pool bytes, chunk-aligned (§8.4).
    pool: Vec<u8>,
    pub stats: NavStats,
}

impl MergedNav {
    pub fn is_empty(&self) -> bool {
        self.node_count == 0
    }

    /// Bytes this section occupies once written after `profile_table`.
    pub fn section_len(&self, profile_table: &[u8]) -> usize {
        NAV_DIR_LEN + profile_table.len() + self.index.len() + self.chunks.len() + self.pool.len()
    }
}

/// Read, unify, prune, renumber and rebuild — §4.6 end to end. `cells` are the `network`-band cells;
/// `band_log2` is that band's cell size, which defines which coordinates are eligible for
/// unification.
pub fn merge(cells: &[&Cell<'_>], band_log2: u32, min_component_edges: usize, global_bbox: UBox) -> Result<MergedNav> {
    let mut stats = NavStats::default();

    // --- 1/2. The serialized node set, unified at boundary coordinates only. ---
    let mut coords: Vec<(i32, i32)> = Vec::new();
    let mut seam: HashMap<(i32, i32), u32> = HashMap::new();
    let mut edges: Vec<MergedEdge> = Vec::new();
    // Duplicate detection (§4.6.3): an edge two cells both wrote in full. The half-open ownership of
    // §3.3/§3.4(3) should already prevent it, so this is a net, not a mechanism.
    let mut seen_edges: HashMap<(u32, u32, u32, u8, u64), ()> = HashMap::new();

    for cell in cells {
        let dir = &cell.nav;
        if dir.is_empty() {
            continue;
        }
        let data_start =
            dir.data_start().ok_or_else(|| Error::Format(format!("cell {}: nav directory overflows", cell.id)))?;
        let pool = cell.read(dir.edge_pool_offset, dir.edge_chunk_count * dir.chunk_size)?;

        // Pass A: every junction record in the cell, straight off its chunks. Reading the chunk run
        // rather than walking the quadtree visits each record exactly once — §8.2's bin packing
        // makes the *walk* non-idempotent, not the storage.
        let mut local: HashMap<u32, u32> = HashMap::new();
        let mut records: Vec<(u32, i32, i32, usize, usize)> = Vec::new(); // (id, lat, lon, chunk, offset)
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
                let global = if on_grid_boundary(lat as i64, lon as i64, band_log2) {
                    match seam.get(&(lat, lon)) {
                        Some(&g) => {
                            stats.unified += 1;
                            g
                        }
                        None => {
                            let g = coords.len() as u32;
                            coords.push((lat, lon));
                            seam.insert((lat, lon), g);
                            g
                        }
                    }
                } else {
                    let g = coords.len() as u32;
                    coords.push((lat, lon));
                    g
                };
                if local.insert(id, global).is_some() {
                    return Err(Error::Format(format!("cell {}: node id {id} appears twice", cell.id)));
                }
                records.push((id, lat, lon, k, at));
                at += rec_len;
            }
            chunks.push(chunk);
        }

        // Pass B: adjacency → edges. Every edge shows up in both endpoints' records with the same
        // `Edge Id`, so the first sighting wins and the second only has to agree.
        let mut cell_edges: HashMap<u32, ()> = HashMap::new();
        for &(own_id, lat, lon, k, at) in &records {
            let chunk = &chunks[k];
            let degree = chunk[at + 12] as usize;
            for n in 0..degree {
                let e = &chunk[at + NAV_NODE_FIXED_LEN + n * NAV_NEIGHBOR_LEN..][..NAV_NEIGHBOR_LEN];
                let nbr_id = u32::from_le_bytes(e[0..4].try_into().expect("4 bytes"));
                let edge_id = u32::from_le_bytes(e[8..12].try_into().expect("4 bytes"));
                let cost_m = u16::from_le_bytes(e[12..14].try_into().expect("2 bytes")) as u32;
                let way_kind = e[14];
                if cell_edges.insert(edge_id, ()).is_some() {
                    continue; // the other direction of an edge already taken
                }
                let a = *local.get(&own_id).expect("own id interned above");
                let b = *local.get(&nbr_id).ok_or_else(|| {
                    Error::Format(format!("cell {}: neighbour id {nbr_id} resolves to no record", cell.id))
                })?;
                let rec = edge_record(&pool, edge_id, cell)?;
                // The record's anchor is endpoint `a`'s coordinate, so a record whose anchor is not
                // this node's coordinate belongs to the other direction — keep the orientation the
                // record itself states.
                let anchor_lat = i32::from_le_bytes(rec[7..11].try_into().expect("4 bytes"));
                let anchor_lon = i32::from_le_bytes(rec[11..15].try_into().expect("4 bytes"));
                let (a, b) = if (anchor_lat, anchor_lon) == (lat, lon) { (a, b) } else { (b, a) };
                let hash = fnv(&rec);
                let key = (a.min(b), a.max(b), cost_m, way_kind, hash);
                if seen_edges.insert(key, ()).is_some() {
                    stats.duplicate_edges += 1;
                    continue;
                }
                edges.push(MergedEdge { a, b, cost_m, kind: way_kind, rec, hash });
            }
        }
    }

    if coords.is_empty() {
        return Ok(MergedNav::empty(stats));
    }

    // --- 4. Island pruning over the *merged* graph: the only place the schema's threshold means
    // what it says — an island in the map, not in a cell (§3.5/§4.6.4). ---
    let (mut keep_node, keep_edge) = prune(coords.len(), &edges, min_component_edges, &mut stats);

    // --- 5. Bound every surviving edge for the device's nearest-edge query. This must happen
    // *after* pruning: `min_component_edges` describes source topology, and counting synthetic
    // degree-2 pieces would let a single long road masquerade as a large component. Whole-map
    // packs do the same split after their own prune; cell packs defer it until this point. ---
    let source_node_count = coords.len();
    let mut bounded = Vec::with_capacity(edges.len());
    for (edge, keep) in edges.into_iter().zip(keep_edge) {
        if keep {
            split_spatial_edge(edge, &mut coords, &mut bounded)?;
        }
    }
    edges = bounded;
    stats.spatial_split_nodes = coords.len() - source_node_count;
    keep_node.resize(coords.len(), true); // only surviving edges mint synthetic nodes
    let keep_edge = vec![true; edges.len()];

    // --- 6. Renumber densely by (lat, lon) ascending — deterministic and content-derived (§4.6.5).
    //
    // Two *distinct* surviving nodes can share a coordinate: unification is restricted to boundary
    // lines (§4.6.2), so the interior collisions a single file legitimately contains — stacked
    // bridge/tunnel junctions — arrive here as separate nodes at one `(lat, lon)`. Their order must
    // still come from content, not from which cell happened to be read first, or two assemblies of
    // the same cells in a different order would produce different bytes. The tie-break is therefore
    // an order-independent digest of the node's own incident edges. ---
    let mut digest = vec![0u64; coords.len()];
    for (e, &keep) in edges.iter().zip(&keep_edge) {
        if !keep {
            continue;
        }
        // Commutative accumulation: the sum does not depend on the order the edges were read in.
        let h = e.hash ^ ((e.cost_m as u64) << 8) ^ e.kind as u64;
        digest[e.a as usize] = digest[e.a as usize].wrapping_add(h);
        digest[e.b as usize] = digest[e.b as usize].wrapping_add(h);
    }
    let mut order: Vec<u32> = (0..coords.len() as u32).filter(|&i| keep_node[i as usize]).collect();
    order.sort_by_key(|&i| (coords[i as usize].0, coords[i as usize].1, digest[i as usize]));
    let mut new_id = vec![u32::MAX; coords.len()];
    for (dense, &old) in order.iter().enumerate() {
        new_id[old as usize] = dense as u32;
    }

    // --- 7. Rebuild the edge pool. `Edge Id` is a pool byte offset, so every record is re-emitted
    // at a new place and the no-straddle rule re-applied at the 512-byte granularity. ---
    let mut kept: Vec<&MergedEdge> = edges.iter().zip(&keep_edge).filter(|(_, &k)| k).map(|(e, _)| e).collect();
    kept.sort_by_key(|e| {
        let (a, b) = (new_id[e.a as usize], new_id[e.b as usize]);
        (a.min(b), a.max(b), e.cost_m, e.kind, e.hash)
    });
    let mut pool: Vec<u8> = Vec::new();
    let mut edge_ids: Vec<u32> = Vec::with_capacity(kept.len());
    for e in &kept {
        let within = pool.len() % NAV_CHUNK_SIZE;
        if within + e.rec.len() > NAV_CHUNK_SIZE {
            pool.resize(pool.len() + (NAV_CHUNK_SIZE - within), CHUNK_END);
        }
        if pool.len() > u32::MAX as usize {
            return Err(Error::Capacity(
                "the merged edge pool passes 4 GiB: `Edge Id` is a uint32 pool byte offset (OBCA §5.7)".into(),
            ));
        }
        edge_ids.push(pool.len() as u32);
        pool.extend_from_slice(&e.rec);
    }
    pool.resize(pool.len().div_ceil(NAV_CHUNK_SIZE) * NAV_CHUNK_SIZE, CHUNK_END);

    // --- 8. Adjacency with inline neighbour coords, degree-capped, wire limits re-checked. ---
    let mut adj: Vec<Vec<WireNeighbor>> = (0..order.len()).map(|_| Vec::new()).collect();
    for (e, &edge_id) in kept.iter().zip(&edge_ids) {
        let (a, b) = (new_id[e.a as usize], new_id[e.b as usize]);
        let mut push = |from: u32, to: u32| -> Result<()> {
            let list = &mut adj[from as usize];
            if list.len() >= NAV_MAX_DEGREE {
                stats.degree_truncated += 1;
                return Ok(());
            }
            let (lat, lon) = coords[order[to as usize] as usize];
            let (flat, flon) = coords[order[from as usize] as usize];
            let (dlat, dlon) = (lat as i64 - flat as i64, lon as i64 - flon as i64);
            if dlat.abs() > MAX_NEIGHBOR_DELTA || dlon.abs() > MAX_NEIGHBOR_DELTA {
                return Err(Error::Format(format!(
                    "a merged adjacency spans ({dlat}, {dlon}) µdeg, past the §8.3 int16 neighbour delta"
                )));
            }
            list.push(WireNeighbor {
                id: to,
                lat,
                lon,
                edge_id,
                cost_m: e.cost_m.min(u16::MAX as u32),
                way_kind: e.kind,
            });
            Ok(())
        };
        push(a, b)?;
        if a != b {
            push(b, a)?; // a self-loop appears once (§8.3)
        }
    }

    let nodes: Vec<NavPoint> = order
        .iter()
        .zip(adj)
        .enumerate()
        .map(|(dense, (&old, neighbors))| {
            let (lat, lon) = coords[old as usize];
            NavPoint { lat, lon, id: dense as u32, neighbors }
        })
        .collect();
    stats.nodes = nodes.len();
    stats.edges = kept.len();

    // --- 8 (cont.). The node quadtree over the **assembly** bbox, with §8.2's bin-packed 512-byte
    // chunks. Laid out here so a shard's size is known before its header is written. ---
    let tree = qtree::build(nodes, global_bbox, NAV_CHUNK_SIZE);
    let (index, node_count, chunks, chunk_count, dropped) =
        qtree::flatten(&tree, NAV_CHUNK_SIZE, true, &|p, out| pack_record(p, out));
    stats.dropped_nodes = dropped;
    Ok(MergedNav { index, node_count, chunks, chunk_count, pool, stats })
}

/// Split a surviving cell edge until every piece satisfies the spatial contract used by the
/// firmware's bounded nearest-edge search. Cell serialization has already enforced the wire
/// delta/record-size limits, so a split can reuse the decoded vertices and every new record still
/// fits one chunk.
fn split_spatial_edge(edge: MergedEdge, coords: &mut Vec<(i32, i32)>, out: &mut Vec<MergedEdge>) -> Result<()> {
    if edge.cost_m <= NAV_SNAP_EDGE_MAX_M {
        out.push(edge);
        return Ok(());
    }

    let mut polyline = decode_edge_polyline(&edge.rec)?;
    let expected_a = coords[edge.a as usize];
    let expected_b = coords[edge.b as usize];
    if polyline.first().copied() != Some((expected_a.1, expected_a.0))
        || polyline.last().copied() != Some((expected_b.1, expected_b.0))
    {
        return Err(Error::Format("a long nav edge record does not terminate at its adjacency endpoints".into()));
    }

    // A two-point edge has no existing shape vertex at which to split. Insert an exact integer
    // midpoint, matching the packer's graph-level split and guaranteeing progress even for a
    // single unusually long OSM segment.
    if polyline.len() == 2 {
        let (a, b) = (polyline[0], polyline[1]);
        let midpoint = (((a.0 as i64 + b.0 as i64) / 2) as i32, ((a.1 as i64 + b.1 as i64) / 2) as i32);
        polyline.insert(1, midpoint);
    }
    let cut = midpoint_index(&polyline);
    let left_polyline = &polyline[..=cut];
    let right_polyline = &polyline[cut..];
    let (lon, lat) = polyline[cut];
    let synthetic = coords.len() as u32;
    coords.push((lat, lon));

    let left = make_edge(edge.a, synthetic, left_polyline, edge.kind)?;
    let right = make_edge(synthetic, edge.b, right_polyline, edge.kind)?;
    split_spatial_edge(left, coords, out)?;
    split_spatial_edge(right, coords, out)
}

/// Decode one self-contained §8.4 edge record into µdegree `(lon, lat)` vertices.
fn decode_edge_polyline(rec: &[u8]) -> Result<Vec<(i32, i32)>> {
    if rec.len() < NAV_EDGE_FIXED_LEN {
        return Err(Error::Format("a nav edge record is shorter than its fixed header".into()));
    }
    let count = u16::from_le_bytes(rec[4..6].try_into().expect("2 bytes")) as usize;
    if count < 2 || rec.len() != NAV_EDGE_FIXED_LEN + (count - 1) * 4 {
        return Err(Error::Format("a nav edge record has an inconsistent point count".into()));
    }
    let mut lat = i32::from_le_bytes(rec[7..11].try_into().expect("4 bytes")) as i64;
    let mut lon = i32::from_le_bytes(rec[11..15].try_into().expect("4 bytes")) as i64;
    let mut points = Vec::with_capacity(count);
    points.push((lon as i32, lat as i32));
    for delta in rec[NAV_EDGE_FIXED_LEN..].chunks_exact(4) {
        lat += i16::from_le_bytes(delta[..2].try_into().expect("2 bytes")) as i64;
        lon += i16::from_le_bytes(delta[2..].try_into().expect("2 bytes")) as i64;
        if i32::try_from(lat).is_err() || i32::try_from(lon).is_err() {
            return Err(Error::Format("a nav edge delta walks outside the int32 coordinate domain".into()));
        }
        points.push((lon as i32, lat as i32));
    }
    Ok(points)
}

/// Re-encode a split edge and derive its rounded physical cost from its geometry.
fn make_edge(a: u32, b: u32, polyline: &[(i32, i32)], kind: u8) -> Result<MergedEdge> {
    let cost_m = polyline_len_m(polyline);
    let mut rec = Vec::with_capacity(NAV_EDGE_FIXED_LEN + (polyline.len() - 1) * 4);
    rec.extend_from_slice(&cost_m.to_le_bytes());
    rec.extend_from_slice(&(polyline.len() as u16).to_le_bytes());
    rec.push(kind);
    rec.extend_from_slice(&polyline[0].1.to_le_bytes());
    rec.extend_from_slice(&polyline[0].0.to_le_bytes());
    for segment in polyline.windows(2) {
        let dlat = segment[1].1 as i64 - segment[0].1 as i64;
        let dlon = segment[1].0 as i64 - segment[0].0 as i64;
        let (dlat, dlon) = (i16::try_from(dlat), i16::try_from(dlon));
        let (Ok(dlat), Ok(dlon)) = (dlat, dlon) else {
            return Err(Error::Format("a spatially split nav edge exceeds the int16 delta limit".into()));
        };
        rec.extend_from_slice(&dlat.to_le_bytes());
        rec.extend_from_slice(&dlon.to_le_bytes());
    }
    if rec.len() > NAV_CHUNK_SIZE {
        return Err(Error::Format("a spatially split nav edge exceeds one edge-pool chunk".into()));
    }
    let hash = fnv(&rec);
    Ok(MergedEdge { a, b, cost_m, kind, rec, hash })
}

fn polyline_len_m(polyline: &[(i32, i32)]) -> u32 {
    polyline
        .windows(2)
        .map(|segment| ground_dist_m(segment[0], segment[1]) as f64)
        .sum::<f64>()
        .round()
        .clamp(0.0, u32::MAX as f64) as u32
}

/// Interior vertex nearest half the polyline's physical length.
fn midpoint_index(polyline: &[(i32, i32)]) -> usize {
    let total = polyline.windows(2).map(|segment| ground_dist_m(segment[0], segment[1]) as f64).sum::<f64>();
    let mut cumulative = 0.0f64;
    let mut best = 1usize;
    let mut best_distance = f64::MAX;
    for index in 1..polyline.len() - 1 {
        cumulative += ground_dist_m(polyline[index - 1], polyline[index]) as f64;
        let distance = (cumulative - total / 2.0).abs();
        if distance < best_distance {
            best = index;
            best_distance = distance;
        }
    }
    best
}

/// One §8.4 edge record, sliced out of its cell's pool at `edge_id` (a pool-relative byte offset).
fn edge_record(pool: &[u8], edge_id: u32, cell: &Cell<'_>) -> Result<Vec<u8>> {
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
    Ok(pool[at..at + rec_len].to_vec())
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

/// Union-find island pruning over the merged graph: keep the largest component plus every component
/// with at least `min_component_edges` edges (§4.6.4). Ties are broken deterministically (edge
/// count, then smallest root) so the result never depends on iteration order.
fn prune(
    node_count: usize,
    edges: &[MergedEdge],
    min_component_edges: usize,
    stats: &mut NavStats,
) -> (Vec<bool>, Vec<bool>) {
    let mut parent: Vec<u32> = (0..node_count as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    for e in edges {
        let (ra, rb) = (find(&mut parent, e.a), find(&mut parent, e.b));
        if ra != rb {
            parent[ra as usize] = rb;
        }
    }
    let roots: Vec<u32> = (0..node_count as u32).map(|i| find(&mut parent, i)).collect();
    let mut nodes_per: BTreeMap<u32, usize> = BTreeMap::new();
    for &r in &roots {
        *nodes_per.entry(r).or_insert(0) += 1;
    }
    let mut edges_per: BTreeMap<u32, usize> = BTreeMap::new();
    for e in edges {
        *edges_per.entry(roots[e.a as usize]).or_insert(0) += 1;
    }
    stats.components_found = nodes_per.len();
    let largest = *nodes_per
        .iter()
        .max_by_key(|(r, n)| (**n, edges_per.get(r).copied().unwrap_or(0), std::cmp::Reverse(**r)))
        .expect("a non-empty graph has a component")
        .0;
    let keep_root = |r: u32| r == largest || edges_per.get(&r).copied().unwrap_or(0) >= min_component_edges;
    stats.components_kept = nodes_per.keys().filter(|r| keep_root(**r)).count();
    stats.largest_component_permille = (nodes_per[&largest] as u64 * 1000 / node_count.max(1) as u64) as u32;

    let keep_node: Vec<bool> = roots.iter().map(|&r| keep_root(r)).collect();
    let keep_edge: Vec<bool> = edges.iter().map(|e| keep_root(roots[e.a as usize])).collect();
    stats.pruned_nodes = keep_node.iter().filter(|k| !**k).count();
    stats.pruned_edges = keep_edge.iter().filter(|k| !**k).count();
    (keep_node, keep_edge)
}

/// Serialize the whole §8 section at absolute byte `section_offset`:
/// `[directory][profile table][node index][node chunks][edge pool]`.
///
/// `profile_table` is the cells' own, copied after every cell was checked to agree (§4.3) — it is
/// schema data and the assembler has no business re-deriving it. An empty graph still writes the
/// directory and the profile table, both regions zero-length just past it.
pub fn serialize(nav: &MergedNav, profile_table: &[u8], section_offset: usize) -> Vec<u8> {
    let profile_count = profile_table.len() / obc_formats::obcm::NAV_PROFILE_LEN;
    let profile_table_offset = section_offset + NAV_DIR_LEN;
    let index_offset = profile_table_offset + profile_table.len();
    let edge_pool_offset = index_offset + nav.index.len() + nav.chunks.len();
    let edge_chunk_count = (nav.pool.len() / NAV_CHUNK_SIZE) as u32;

    let mut out = Vec::with_capacity(nav.section_len(profile_table));
    out.extend_from_slice(&(index_offset as u32).to_le_bytes());
    out.extend_from_slice(&nav.node_count.to_le_bytes());
    out.extend_from_slice(&nav.chunk_count.to_le_bytes());
    out.extend_from_slice(&(edge_pool_offset as u32).to_le_bytes());
    out.extend_from_slice(&edge_chunk_count.to_le_bytes());
    out.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes());
    out.extend_from_slice(&(profile_table_offset as u32).to_le_bytes());
    out.push(profile_count as u8);
    out.push(0); // reserved
    debug_assert_eq!(out.len(), NAV_DIR_LEN);
    out.extend_from_slice(profile_table);
    out.extend_from_slice(&nav.index);
    out.extend_from_slice(&nav.chunks);
    out.extend_from_slice(&nav.pool);
    debug_assert_eq!(out.len(), nav.section_len(profile_table));
    out
}

/// The §8.3 junction record: `lat i32, lon i32, node_id u32, degree u8`, then 15 bytes per
/// neighbour with the coord stored as an `int16` delta from this record's own.
fn pack_record(p: &NavPoint, out: &mut Vec<u8>) {
    out.extend_from_slice(&p.lat.to_le_bytes());
    out.extend_from_slice(&p.lon.to_le_bytes());
    out.extend_from_slice(&p.id.to_le_bytes());
    out.push(p.neighbors.len() as u8);
    for n in &p.neighbors {
        out.extend_from_slice(&n.id.to_le_bytes());
        out.extend_from_slice(&((n.lat as i64 - p.lat as i64) as i16).to_le_bytes());
        out.extend_from_slice(&((n.lon as i64 - p.lon as i64) as i16).to_le_bytes());
        out.extend_from_slice(&n.edge_id.to_le_bytes());
        out.extend_from_slice(&(n.cost_m.min(u16::MAX as u32) as u16).to_le_bytes());
        out.push(n.way_kind);
    }
}

impl MergedNav {
    /// The graph a shard with no nav carries: the directory plus the always-present profile table,
    /// both data regions zero-length (§5.1/§8.1).
    pub fn empty(stats: NavStats) -> MergedNav {
        MergedNav { index: Vec::new(), node_count: 0, chunks: Vec::new(), chunk_count: 0, pool: Vec::new(), stats }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_section_still_carries_its_profiles() {
        let profiles = vec![0u8; obc_formats::obcm::NAV_PROFILE_LEN];
        let bytes = serialize(&MergedNav::empty(NavStats::default()), &profiles, 500);
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 0, "empty graph ⇒ no index nodes");
        assert_eq!(u16::from_le_bytes(bytes[20..22].try_into().unwrap()) as usize, NAV_CHUNK_SIZE, "pinned 512");
        assert_eq!(u32::from_le_bytes(bytes[22..26].try_into().unwrap()) as usize, 500 + NAV_DIR_LEN);
        assert_eq!(bytes[26], 1, "one profile");
        assert_eq!(bytes.len(), NAV_DIR_LEN + profiles.len());
    }
}
