//! Host-only adapter: Fast Paths prepares shortcuts; the query crate reads paged summaries.
use crate::model::{Graph, Point, Profile};
use crate::partition::WeightedGraph;
use crate::search::{Progress, Search};
use crate::storage::{self, Arc, Cache, EdgeRef, Node, Page, Seed, NODES_PER_PAGE};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::time::Instant;

pub fn state_graph(graph: &Graph, profile: &Profile) -> Result<WeightedGraph, String> {
    profile.validate()?;
    let costs: Vec<_> = graph.roads.iter().map(|road| profile.cost(road)).collect();
    state_graph_with_costs(graph, profile.walking, &costs)
}

/// Costs are frozen before shortcut preparation. None excludes a directed road.
pub fn state_graph_with_costs(graph: &Graph, walking: bool, costs: &[Option<u64>]) -> Result<WeightedGraph, String> {
    if costs.len() != graph.roads.len() || costs.iter().flatten().any(|&c| c == 0 || c == u64::MAX) {
        return Err("One positive finite cost or exclusion is required per directed road".into());
    }
    let departures = graph.departures();
    let mut adjacency = vec![Vec::new(); graph.roads.len()];
    for (id, road) in graph.roads.iter().enumerate() {
        if costs[id].is_none() {
            continue;
        }
        for &next in &departures[road.to as usize] {
            if graph.permits_turn(id as u32, next, walking) {
                if let Some(cost) = costs[next as usize] {
                    adjacency[id].push((next, cost));
                }
            }
        }
    }
    Ok(WeightedGraph { coords: graph.roads.iter().map(|r| graph.points[r.to as usize]).collect(), adjacency })
}

#[derive(Serialize)]
pub struct ChStats {
    pub nodes: usize,
    pub arcs: usize,
    pub compressed_summary_bytes: usize,
    pub preparation_ms: u128,
    pub page_count: usize,
}

pub struct Prepared {
    pub ranks: Vec<u32>,
    pub stats: ChStats,
}

pub fn prepare(graph: &WeightedGraph, directory: &Path) -> Result<Prepared, String> {
    let started = Instant::now();
    fs::create_dir_all(directory).map_err(|e| e.to_string())?;
    let mut input = fast_paths::InputGraph::new();
    for (from, outgoing) in graph.adjacency.iter().enumerate() {
        for &(to, cost) in outgoing {
            if cost == 0 || cost >= u32::MAX as u64 {
                return Err("CH requires positive costs below u32::MAX".into());
            }
            input.add_edge(from, to as usize, cost as usize);
        }
    }
    input.freeze();
    let ch = fast_paths::prepare(&input);
    drop(input);
    // FastGraph32 is Fast Paths' public export type. No runtime dependency on its layout remains.
    let raw = std::panic::catch_unwind(|| fast_paths::FastGraph32::new(&ch))
        .map_err(|_| "Prepared CH exceeds the experimental 32-bit export limits")?;
    drop(ch);
    let mut ranks = raw.ranks;
    let n = graph.coords.len();
    ranks.extend((ranks.len()..n).map(|i| i as u32));
    let mut ordering = vec![0usize; n];
    for (id, &rank) in ranks.iter().enumerate() {
        ordering[rank as usize] = id;
    }
    let reference = |id: u32, backward: bool| {
        let edges = if backward { &raw.edges_bwd } else { &raw.edges_fwd };
        let offsets = if backward { &raw.first_edge_ids_bwd } else { &raw.first_edge_ids_fwd };
        let node = ranks[edges[id as usize].base_node as usize];
        EdgeRef { node, index: id - offsets[node as usize], backward }
    };
    let mut bytes = 0;
    let page_count = n.div_ceil(NODES_PER_PAGE as usize);
    for page_id in 0..page_count {
        let first = page_id * NODES_PER_PAGE as usize;
        let mut nodes = Vec::new();
        for &original in &ordering[first..(first + NODES_PER_PAGE as usize).min(n)] {
            let rank = ranks[original] as usize;
            let mut node = Node::default();
            for backward in [false, true] {
                let edges = if backward { &raw.edges_bwd } else { &raw.edges_fwd };
                let offsets = if backward { &raw.first_edge_ids_bwd } else { &raw.first_edge_ids_fwd };
                if rank + 1 >= offsets.len() {
                    continue;
                }
                let list = if backward { &mut node.backward } else { &mut node.forward };
                for edge in &edges[offsets[rank] as usize..offsets[rank + 1] as usize] {
                    let children = (edge.replaced_in_edge != u32::MAX)
                        .then(|| [reference(edge.replaced_in_edge, true), reference(edge.replaced_out_edge, false)]);
                    list.push(Arc {
                        to: ranks[edge.adj_node as usize],
                        cost: edge.weight as u64,
                        children,
                        road: if backward { edge.base_node } else { edge.adj_node },
                    });
                }
            }
            nodes.push(node);
        }
        let encoded = storage::encode(&Page { first: first as u32, nodes })?;
        bytes += encoded.len();
        fs::write(directory.join(format!("{page_id}.bin")), encoded).map_err(|e| e.to_string())?;
    }
    // Endpoint records are spatially paged. Benchmarks use exact graph-state endpoints.
    let mut cells = BTreeMap::new();
    for (id, &point) in graph.coords.iter().enumerate() {
        cells.entry(storage::snap_cell(point)).or_insert_with(Vec::new).push((point, id as u32, ranks[id]));
    }
    fs::create_dir_all(directory.join("lookup")).map_err(|e| e.to_string())?;
    for ((lat, lon), entries) in cells {
        fs::write(directory.join(format!("lookup/{lat}_{lon}.bin")), storage::encode(&entries)?)
            .map_err(|e| e.to_string())?;
    }
    let manifest = serde_json::json!({"format":1,"nodes":n,"nodes_per_page":NODES_PER_PAGE});
    fs::write(directory.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).map_err(|e| e.to_string())?;
    Ok(Prepared {
        ranks,
        stats: ChStats {
            nodes: n,
            arcs: raw.edges_fwd.len() + raw.edges_bwd.len(),
            compressed_summary_bytes: bytes,
            preparation_ms: started.elapsed().as_millis(),
            page_count,
        },
    })
}

#[derive(Serialize)]
pub struct QueryStats {
    pub elapsed_ms: f64,
    pub cost: u64,
    pub labels: usize,
    pub summary_bytes: usize,
    pub endpoint_bytes: usize,
    pub summary_pages: usize,
    pub fetch_rounds: usize,
    pub decoded_cache_bytes: usize,
    #[serde(skip)]
    pub path_nodes: Vec<u32>,
}

pub fn query(
    _prepared: &Prepared,
    graph: &WeightedGraph,
    directory: &Path,
    start: u32,
    end: u32,
) -> Result<Option<QueryStats>, String> {
    let now = Instant::now();
    let mut cache = Cache::default();
    let mut bytes = 0;
    let mut rounds = 0;
    let cells: BTreeSet<_> = [start, end].iter().map(|id| storage::snap_cell(graph.coords[*id as usize])).collect();
    let mut endpoint_bytes = fs::metadata(directory.join("manifest.json")).map_err(|e| e.to_string())?.len() as usize;
    let mut endpoint_ranks = [None, None];
    for (lat, lon) in cells {
        let data = fs::read(directory.join(format!("lookup/{lat}_{lon}.bin"))).map_err(|e| e.to_string())?;
        let entries: Vec<(Point, u32, u32)> = storage::decode(&data)?;
        for (_, id, rank) in entries {
            for (slot, requested) in [start, end].iter().enumerate() {
                if id == *requested {
                    endpoint_ranks[slot] = Some(rank);
                }
            }
        }
        endpoint_bytes += data.len();
    }
    let mut search = Search::new(
        &[Seed { node: endpoint_ranks[0].ok_or("Missing source lookup")?, cost: 0, road: start }],
        &[Seed { node: endpoint_ranks[1].ok_or("Missing target lookup")?, cost: 0, road: end }],
        1_000_000,
    );
    loop {
        match search.poll(&cache, 4096) {
            Progress::Working { .. } => (),
            Progress::NeedPages { pages } => {
                rounds += 1;
                for page in pages {
                    if cache.pages.contains_key(&page) {
                        return Err("Missing node in loaded page".into());
                    }
                    let data = fs::read(directory.join(format!("{page}.bin"))).map_err(|e| e.to_string())?;
                    bytes += data.len();
                    cache.insert_page(page, &data, 256 * 1024 * 1024)?;
                }
            }
            Progress::Done { cost, roads, labels } => {
                return Ok(Some(QueryStats {
                    elapsed_ms: now.elapsed().as_secs_f64() * 1000.0,
                    cost,
                    labels,
                    summary_bytes: bytes,
                    endpoint_bytes,
                    summary_pages: cache.pages.len(),
                    fetch_rounds: rounds,
                    decoded_cache_bytes: cache.decoded_bytes,
                    path_nodes: roads,
                }))
            }
            Progress::NoPath => return Ok(None),
            other => return Err(format!("CH query: {other:?}")),
        }
    }
}

pub fn path_cost(graph: &WeightedGraph, nodes: &[u32]) -> Result<u64, String> {
    nodes.windows(2).try_fold(0u64, |sum, pair| {
        let edge = graph.adjacency[pair[0] as usize]
            .iter()
            .filter(|(to, _)| *to == pair[1])
            .map(|(_, c)| *c)
            .min()
            .ok_or("Path has an absent transition")?;
        sum.checked_add(edge).ok_or_else(|| "Path overflow".into())
    })
}

pub fn dijkstra(graph: &WeightedGraph, start: u32, end: u32) -> Option<u64> {
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;
    let mut distances = vec![u64::MAX; graph.coords.len()];
    let mut heap = BinaryHeap::from([Reverse((0u64, start))]);
    distances[start as usize] = 0;
    while let Some(Reverse((cost, node))) = heap.pop() {
        if distances[node as usize] != cost {
            continue;
        }
        if node == end {
            return Some(cost);
        }
        for &(next, weight) in &graph.adjacency[node as usize] {
            if let Some(value) = cost.checked_add(weight) {
                if value < distances[next as usize] {
                    distances[next as usize] = value;
                    heap.push(Reverse((value, next)));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paged_ch_matches_exact_oracle_on_directed_graphs() {
        let dir = std::env::temp_dir().join(format!("trip-ch-test-{}", std::process::id()));
        let n = 26;
        let mut graph = WeightedGraph { coords: vec![Point::default(); n], adjacency: vec![vec![]; n] };
        // Two components, an isolated final node, parallel arcs, and asymmetric weights.
        for a in 0..n - 1 {
            for b in 0..n - 1 {
                if a != b && a / 12 == b / 12 && (a * 17 + b * 13) % 7 < 2 {
                    graph.adjacency[a].push((b as u32, (a * 7 + b * 11) as u64 % 29 + 1));
                }
            }
        }
        graph.adjacency[0].extend([(1, 3), (1, 8)]);
        let prepared = prepare(&graph, &dir).unwrap();
        for a in 0..n as u32 {
            for b in 0..n as u32 {
                let result = query(&prepared, &graph, &dir, a, b).unwrap();
                assert_eq!(result.as_ref().map(|r| r.cost), dijkstra(&graph, a, b), "{a}->{b}");
                if let Some(result) = result {
                    assert_eq!(result.path_nodes.first(), Some(&a));
                    assert_eq!(result.path_nodes.last(), Some(&b));
                    assert_eq!(path_cost(&graph, &result.path_nodes).unwrap(), result.cost);
                }
            }
        }
        fs::remove_dir_all(dir).unwrap();
    }
}
