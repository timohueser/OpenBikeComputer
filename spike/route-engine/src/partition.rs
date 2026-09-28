//! Exact one-level overlay experiment. Coordinate cuts are a baseline, not a graph separator.
use crate::model::Point;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap};

#[derive(Clone, Debug)]
pub struct WeightedGraph {
    pub coords: Vec<Point>,
    pub adjacency: Vec<Vec<(u32, u64)>>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Stats {
    pub cells: usize,
    pub boundary_nodes: usize,
    pub max_boundary_nodes: usize,
    pub summary_arcs: usize,
    pub directory_bytes: usize,
    pub summary_bytes: usize,
    pub detail_bytes: usize,
    pub raw_directory_bytes: usize,
    pub raw_summary_bytes: usize,
    pub raw_detail_bytes: usize,
    /// Owned vector storage, excluding allocator overhead and the caller's source graph.
    pub directory_resident_bytes: usize,
    pub summary_resident_bytes: usize,
    pub detail_resident_bytes: usize,
}

/// Zlib-compressed postcard payload sizes; excludes HTTP, snapping, geometry and elevation.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Transfer {
    pub directory_bytes: usize,
    pub summary_bytes: usize,
    pub detail_bytes: usize,
    pub summary_cells: Vec<u32>,
    pub detail_cells: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub cost: u64,
    pub path_nodes: Vec<u32>,
    pub settled_nodes: usize,
    pub transfer: Transfer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Detail {
    nodes: Vec<u32>,
    adjacency: Vec<Vec<(u32, u64)>>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
struct Arc {
    to: u32,
    cost: u64,
    /// Internal shortcuts require the detailed cell to reconstruct their path.
    internal: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Summary {
    nodes: Vec<u32>,
    adjacency: Vec<Vec<Arc>>,
}

pub struct Partition {
    cell_of: Vec<u32>,
    local_of: Vec<usize>,
    summary_of: Vec<Option<usize>>,
    details: Vec<Detail>,
    summaries: Vec<Summary>,
    summary_sizes: Vec<usize>,
    detail_sizes: Vec<usize>,
    stats: Stats,
}

fn raw_size<T: Serialize>(value: &T) -> Result<usize, String> {
    postcard::to_allocvec(value).map(|bytes| bytes.len()).map_err(|e| e.to_string())
}

impl Partition {
    pub fn build(graph: &WeightedGraph, max_cell_nodes: usize, max_boundary_nodes: usize) -> Result<Self, String> {
        let n = graph.coords.len();
        if max_cell_nodes == 0 || graph.adjacency.len() != n || n > u32::MAX as usize {
            return Err("Invalid graph size or cell size".into());
        }
        for edges in &graph.adjacency {
            if edges.iter().any(|&(v, w)| v as usize >= n || w == u64::MAX) {
                return Err("Invalid edge target or reserved infinite cost".into());
            }
        }
        let mut groups = Vec::new();
        partition_nodes((0..n as u32).collect(), &graph.coords, max_cell_nodes, &mut groups);
        let mut cell_of = vec![0; n];
        let mut local_of = vec![0; n];
        for (cell, nodes) in groups.iter().enumerate() {
            for (local, &node) in nodes.iter().enumerate() {
                cell_of[node as usize] = cell as u32;
                local_of[node as usize] = local;
            }
        }
        let mut boundary = vec![false; n];
        for (u, edges) in graph.adjacency.iter().enumerate() {
            for &(v, _) in edges {
                if cell_of[u] != cell_of[v as usize] {
                    boundary[u] = true;
                    boundary[v as usize] = true;
                }
            }
        }
        let mut result = Self {
            cell_of,
            local_of,
            summary_of: vec![None; n],
            details: Vec::new(),
            summaries: Vec::new(),
            summary_sizes: Vec::new(),
            detail_sizes: Vec::new(),
            stats: Stats::default(),
        };
        for (cell, nodes) in groups.into_iter().enumerate() {
            let terminals: Vec<_> = nodes.iter().copied().filter(|&u| boundary[u as usize]).collect();
            if terminals.len() > max_boundary_nodes {
                return Err(format!(
                    "Cell {cell} has {} boundary nodes among {} nodes; cap is {max_boundary_nodes}",
                    terminals.len(),
                    nodes.len()
                ));
            }
            result.stats.max_boundary_nodes = result.stats.max_boundary_nodes.max(terminals.len());
            let adjacency = nodes.iter().map(|&u| graph.adjacency[u as usize].clone()).collect();
            result.details.push(Detail { nodes, adjacency });
            let mut summary = Summary { nodes: terminals.clone(), adjacency: Vec::new() };
            for (index, &source) in terminals.iter().enumerate() {
                result.summary_of[source as usize] = Some(index);
                let (distances, _) = result.local_search(cell, source, None)?;
                let mut arcs: Vec<_> = terminals
                    .iter()
                    .filter_map(|&target| {
                        let cost = distances[result.local_of[target as usize]];
                        (source != target && cost != u64::MAX).then_some(Arc { to: target, cost, internal: true })
                    })
                    .collect();
                arcs.extend(graph.adjacency[source as usize].iter().filter_map(|&(to, cost)| {
                    (result.cell_of[to as usize] != cell as u32).then_some(Arc { to, cost, internal: false })
                }));
                result.stats.summary_arcs += arcs.len();
                summary.adjacency.push(arcs);
            }
            result.stats.boundary_nodes += terminals.len();
            result.summaries.push(summary);
        }
        result.stats.cells = result.details.len();
        // Node-to-cell mapping and boundary row positions are required to locate each page.
        let directory = (&result.cell_of, &result.summary_of);
        result.stats.directory_bytes = crate::storage::encode(&directory)?.len();
        result.stats.raw_directory_bytes = raw_size(&directory)?;
        result.stats.raw_detail_bytes =
            result.details.iter().map(raw_size).collect::<Result<Vec<_>, _>>()?.iter().sum();
        result.stats.raw_summary_bytes =
            result.summaries.iter().map(raw_size).collect::<Result<Vec<_>, _>>()?.iter().sum();
        use std::mem::size_of;
        result.stats.directory_resident_bytes = result.cell_of.capacity() * size_of::<u32>()
            + result.local_of.capacity() * size_of::<usize>()
            + result.summary_of.capacity() * size_of::<Option<usize>>();
        result.stats.summary_resident_bytes = result.summaries.capacity() * size_of::<Summary>()
            + result
                .summaries
                .iter()
                .map(|s| {
                    s.nodes.capacity() * size_of::<u32>()
                        + s.adjacency.capacity() * size_of::<Vec<Arc>>()
                        + s.adjacency.iter().map(|a| a.capacity() * size_of::<Arc>()).sum::<usize>()
                })
                .sum::<usize>();
        result.stats.detail_resident_bytes = result.details.capacity() * size_of::<Detail>()
            + result
                .details
                .iter()
                .map(|d| {
                    d.nodes.capacity() * size_of::<u32>()
                        + d.adjacency.capacity() * size_of::<Vec<(u32, u64)>>()
                        + d.adjacency.iter().map(|a| a.capacity() * size_of::<(u32, u64)>()).sum::<usize>()
                })
                .sum::<usize>();
        result.detail_sizes = result
            .details
            .iter()
            .map(|page| crate::storage::encode(page).map(|bytes| bytes.len()))
            .collect::<Result<_, _>>()?;
        result.summary_sizes = result
            .summaries
            .iter()
            .map(|page| crate::storage::encode(page).map(|bytes| bytes.len()))
            .collect::<Result<_, _>>()?;
        result.stats.detail_bytes = result.detail_sizes.iter().sum();
        result.stats.summary_bytes = result.summary_sizes.iter().sum();
        Ok(result)
    }

    pub fn stats(&self) -> &Stats {
        &self.stats
    }

    /// Searches detailed endpoint cells and summary cells elsewhere, then refines shortcuts.
    pub fn query(&self, start: u32, end: u32) -> Result<Option<Route>, String> {
        let n = self.cell_of.len();
        if start as usize >= n || end as usize >= n {
            return Err("Endpoint outside graph".into());
        }
        let start_cell = self.cell_of[start as usize];
        let end_cell = self.cell_of[end as usize];
        let mut distances = vec![u64::MAX; n];
        let mut parents = vec![None::<(u32, bool)>; n];
        let mut heap = BinaryHeap::from([Reverse((0u64, start))]);
        let mut summary_cells = BTreeSet::new();
        let mut detail_cells = BTreeSet::from([start_cell, end_cell]);
        distances[start as usize] = 0;
        let mut settled_nodes = 0;
        while let Some(Reverse((distance, u))) = heap.pop() {
            if distance != distances[u as usize] {
                continue;
            }
            settled_nodes += 1;
            if u == end {
                break;
            }
            let cell = self.cell_of[u as usize];
            let mut relax = |arc: Arc| -> Result<(), String> {
                let cost = distance.checked_add(arc.cost).filter(|&x| x != u64::MAX).ok_or("Path cost overflow")?;
                if cost < distances[arc.to as usize] {
                    distances[arc.to as usize] = cost;
                    parents[arc.to as usize] = Some((u, arc.internal));
                    heap.push(Reverse((cost, arc.to)));
                }
                Ok(())
            };
            if cell == start_cell || cell == end_cell {
                for &(to, cost) in &self.details[cell as usize].adjacency[self.local_of[u as usize]] {
                    relax(Arc { to, cost, internal: false })?;
                }
            } else {
                summary_cells.insert(cell);
                let row = self.summary_of[u as usize].ok_or("Non-boundary node reached outside endpoint cells")?;
                for &arc in &self.summaries[cell as usize].adjacency[row] {
                    relax(arc)?;
                }
            }
        }
        let cost = distances[end as usize];
        if cost == u64::MAX {
            return Ok(None);
        }
        let mut steps = Vec::new();
        let mut current = end;
        while current != start {
            let (previous, internal) = parents[current as usize].ok_or("Broken search witness")?;
            steps.push((previous, current, internal));
            current = previous;
        }
        let mut path_nodes = vec![start];
        for (from, to, internal) in steps.into_iter().rev() {
            if internal {
                let cell = self.cell_of[from as usize] as usize;
                detail_cells.insert(cell as u32);
                let (_, parent) = self.local_search(cell, from, Some(to))?;
                let mut path = vec![to];
                let mut node = to;
                while node != from {
                    node = parent[self.local_of[node as usize]].ok_or("Broken cell witness")?;
                    path.push(node);
                }
                path_nodes.extend(path.into_iter().rev().skip(1));
            } else {
                // Original edges are stored in their source cell's detail page.
                detail_cells.insert(self.cell_of[from as usize]);
                path_nodes.push(to);
            }
        }
        let transfer = Transfer {
            directory_bytes: self.stats.directory_bytes,
            summary_bytes: summary_cells.iter().map(|&c| self.summary_sizes[c as usize]).sum(),
            detail_bytes: detail_cells.iter().map(|&c| self.detail_sizes[c as usize]).sum(),
            summary_cells: summary_cells.into_iter().collect(),
            detail_cells: detail_cells.into_iter().collect(),
        };
        Ok(Some(Route { cost, path_nodes, settled_nodes, transfer }))
    }

    fn local_search(
        &self,
        cell: usize,
        source: u32,
        target: Option<u32>,
    ) -> Result<(Vec<u64>, Vec<Option<u32>>), String> {
        let detail = &self.details[cell];
        let mut distances = vec![u64::MAX; detail.nodes.len()];
        let mut parents = vec![None; detail.nodes.len()];
        let mut heap = BinaryHeap::from([Reverse((0u64, source))]);
        distances[self.local_of[source as usize]] = 0;
        while let Some(Reverse((distance, u))) = heap.pop() {
            if distance != distances[self.local_of[u as usize]] {
                continue;
            }
            if target == Some(u) {
                break;
            }
            for &(v, weight) in &detail.adjacency[self.local_of[u as usize]] {
                if self.cell_of[v as usize] as usize != cell {
                    continue;
                }
                let cost = distance.checked_add(weight).filter(|&x| x != u64::MAX).ok_or("Cell cost overflow")?;
                let local = self.local_of[v as usize];
                if cost < distances[local] {
                    distances[local] = cost;
                    parents[local] = Some(u);
                    heap.push(Reverse((cost, v)));
                }
            }
        }
        Ok((distances, parents))
    }
}

fn partition_nodes(mut nodes: Vec<u32>, points: &[Point], limit: usize, groups: &mut Vec<Vec<u32>>) {
    if nodes.is_empty() {
        return;
    }
    if nodes.len() <= limit {
        groups.push(nodes);
        return;
    }
    let (mut min_lat, mut max_lat, mut min_lon, mut max_lon) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &node in &nodes {
        let p = points[node as usize];
        min_lat = min_lat.min(p.lat);
        max_lat = max_lat.max(p.lat);
        min_lon = min_lon.min(p.lon);
        max_lon = max_lon.max(p.lon);
    }
    let lat = i64::from(max_lat) - i64::from(min_lat) >= i64::from(max_lon) - i64::from(min_lon);
    let middle = nodes.len() / 2;
    nodes.select_nth_unstable_by_key(middle, |&node| {
        let p = points[node as usize];
        (if lat { p.lat } else { p.lon }, node)
    });
    let right = nodes.split_off(middle);
    partition_nodes(nodes, points, limit, groups);
    partition_nodes(right, points, limit, groups);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(graph: &WeightedGraph, start: u32, end: u32) -> Option<u64> {
        let mut distances = vec![u64::MAX; graph.coords.len()];
        let mut heap = BinaryHeap::from([Reverse((0u64, start))]);
        distances[start as usize] = 0;
        while let Some(Reverse((cost, u))) = heap.pop() {
            if cost != distances[u as usize] {
                continue;
            }
            if u == end {
                return Some(cost);
            }
            for &(v, weight) in &graph.adjacency[u as usize] {
                if cost + weight < distances[v as usize] {
                    distances[v as usize] = cost + weight;
                    heap.push(Reverse((cost + weight, v)));
                }
            }
        }
        None
    }

    #[test]
    fn directed_overlay_and_witness_match_reference() {
        let mut graph = WeightedGraph {
            coords: (0..36).map(|i| Point { lat: i / 6, lon: i % 6, elevation: 0 }).collect(),
            adjacency: vec![vec![]; 36],
        };
        for u in 0..35 {
            for v in 0..35 {
                if u != v && (u * 41 + v * 13) % 23 < 3 {
                    graph.adjacency[u].push((v as u32, ((u * 11 + v * 7) % 19) as u64));
                }
            }
        }
        // Parallel arcs, zero costs, and an isolated node exercise directed edge cases.
        graph.adjacency[0].extend([(1, 1), (1, 19)]);
        let partition = Partition::build(&graph, 5, 5).unwrap();
        for start in 0..36 {
            for end in 0..36 {
                let result = partition.query(start, end).unwrap();
                assert_eq!(result.as_ref().map(|r| r.cost), reference(&graph, start, end));
                if let Some(route) = result {
                    let cost: u64 = route
                        .path_nodes
                        .windows(2)
                        .map(|pair| {
                            graph.adjacency[pair[0] as usize]
                                .iter()
                                .filter(|(v, _)| *v == pair[1])
                                .map(|(_, cost)| *cost)
                                .min()
                                .unwrap()
                        })
                        .sum();
                    assert_eq!(cost, route.cost);
                    assert_eq!(route.path_nodes.first(), Some(&start));
                    assert_eq!(route.path_nodes.last(), Some(&end));
                    assert!(route.transfer.detail_bytes <= partition.stats.detail_bytes);
                    assert!(route.transfer.summary_bytes <= partition.stats.summary_bytes);
                }
            }
        }
    }

    #[test]
    fn caps_and_invalid_inputs_are_errors() {
        let graph = WeightedGraph { coords: vec![Point::default(); 2], adjacency: vec![vec![(1, 1)], vec![]] };
        assert!(Partition::build(&graph, 1, 0).is_err());
        assert!(Partition::build(&graph, 0, 2).is_err());
        assert!(Partition::build(&graph, 2, 2).unwrap().query(0, 2).is_err());
    }
}
