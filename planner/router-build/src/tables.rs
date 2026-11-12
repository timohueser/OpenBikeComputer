//! Computes the cost table of each profile for each cell of each level: the least cost of a path in
//! the cell from each entering road to each leaving road (rules T1 to T7 of
//! `specs/route-package.md`). Level 1 searches the roads of the cell; each higher level searches
//! the tables of the cells of the level below.

use crate::column::{Column, Layout};
use crate::partition::Cells;
use planner_router::cells::{encode_table, Table, TABLE_HEADER};
use rayon::prelude::*;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

/// A table whose values by row take more bytes than this also holds them by column. Then a column
/// read with a stride touches more than 4 pages of 4 KiB or more than one page of 16 KiB.
pub const COLUMNS_FROM: usize = 16_384;
/// The values of one batch of tables, at 8 bytes each, take at most this, unless the batch needs
/// more to give each thread one table.
const BATCH_BYTES: u64 = 64 << 20;

/// The cost columns of one profile of the costs layer.
#[derive(Clone, Copy)]
pub struct Costs<'a> {
    /// The total cost of each road; `u32::MAX` where the profile excludes it.
    pub totals: &'a [u32],
    /// The cost of each turn of the topology; `u32::MAX` where the profile cannot use it.
    pub turns: &'a [u32],
}

/// The tables of each level. The table of cell `c` and profile `p` of a level is
/// `bytes[at[c × P + p]..at[c × P + p + 1]]`, with the cells in the order of the partition and the
/// profiles in the order of the costs layer.
pub struct Tables {
    profiles: usize,
    levels: Vec<Level>,
}

struct Level {
    bytes: Column<u8>,
    at: Column<u64>,
}

impl Tables {
    pub fn levels(&self) -> usize {
        self.levels.len()
    }

    pub fn table(&self, level: usize, cell: usize, profile: usize) -> &[u8] {
        let Level { bytes, at } = &self.levels[level - 1];
        let k = cell * self.profiles + profile;
        &bytes.values()[at.values()[k] as usize..at.values()[k + 1] as usize]
    }

    /// Saves level `l` as `tables.<l>.bytes` and `tables.<l>.at`.
    pub(crate) fn save(self, layout: &mut Layout) -> Result<(), String> {
        for (k, level) in self.levels.into_iter().enumerate() {
            layout.save(&format!("tables.{}.bytes", k + 1), level.bytes)?;
            layout.save(&format!("tables.{}.at", k + 1), level.at)?;
        }
        Ok(())
    }

    /// Opens the tables of `levels` levels and `profiles` profiles.
    pub(crate) fn open(layout: &Layout, levels: usize, profiles: usize) -> Result<Self, String> {
        let open = |level: usize| -> Result<Level, String> {
            let bytes: Column<u8> = layout.open(&format!("tables.{level}.bytes"))?;
            let at: Column<u64> = layout.open(&format!("tables.{level}.at"))?;
            let (count, end) = (at.len().saturating_sub(1), at.values().last().copied());
            if profiles == 0 || count % profiles != 0 || end != Some(bytes.len() as u64) {
                return Err(format!("the tables of level {level} do not match their offsets"));
            }
            Ok(Level { bytes, at })
        };
        Ok(Self { profiles, levels: (1..=levels).map(open).collect::<Result<_, _>>()? })
    }

    /// The figures of level `level`: the tables, their bytes, the bytes for each road and profile,
    /// the tables of each width, the tables and bytes of the copies by column, the largest finite
    /// value of each profile, the finite values and those below 2^16 − 1, and a SHA-256 of the store.
    pub fn report(&self, level: usize, roads: usize) -> Value {
        let Level { bytes, at } = &self.levels[level - 1];
        let (mut widths, mut copies, mut copy_bytes) = ([0u64; 3], 0u64, 0u64);
        let (mut largest, mut finite, mut narrow) = (vec![0u64; self.profiles], 0u64, 0u64);
        let tables = at.len() - 1;
        for k in 0..tables {
            let raw = &bytes.values()[at.values()[k] as usize..at.values()[k + 1] as usize];
            let size = |at: usize| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
            let table = Table::parse(raw, size(0), size(4)).expect("the stage wrote each table");
            widths[usize::from(table.width.ilog2() as u8)] += 1;
            if table.layout == 1 {
                copies += 1;
                copy_bytes += (raw.len() - TABLE_HEADER) as u64 / 2;
            }
            for row in 0..table.rows {
                for value in table.row(row).filter(|&value| value != u64::MAX) {
                    finite += 1;
                    narrow += u64::from(value < 0xFFFF);
                    largest[k % self.profiles] = largest[k % self.profiles].max(value);
                }
            }
        }
        let mut hash = Sha256::new();
        hash.update(bytes.values());
        at.values().iter().for_each(|value| hash.update(value.to_le_bytes()));
        let total = bytes.len() as u64;
        json!({
            "level": level, "tables": tables, "bytes": total,
            "bytes_per_road_and_profile": total as f64 / (roads * self.profiles).max(1) as f64,
            "width_1_2_4": widths, "column_copies": copies, "column_copy_bytes": copy_bytes,
            "largest": largest, "finite": finite, "below_16_bits": narrow,
            "sha256": format!("{:x}", hash.finalize()),
        })
    }
}

/// The search graph of one cell: the inner nodes, each with its arcs, then the leaving nodes in
/// the order of the leaving list.
#[derive(Default)]
struct Graph {
    /// The arcs of inner node `n` are `first[n]..first[n + 1]`.
    first: Vec<u32>,
    head: Vec<u32>,
    /// The weight of each arc for the profile that runs; `u64::MAX` for none.
    weight: Vec<u64>,
    /// The node of each row of the table.
    rows: Vec<u32>,
    leaving: usize,
}

impl Graph {
    fn clear(&mut self) {
        self.first.clear();
        self.head.clear();
        self.weight.clear();
        self.rows.clear();
        self.first.push(0);
    }
}

/// The memory of one worker.
#[derive(Default)]
struct Scratch {
    graph: Graph,
    /// Level 2 and up: the entering roads of the children, the first node of each child, and the
    /// target node of each column of each child.
    roads: Vec<u32>,
    starts: Vec<usize>,
    targets: Vec<u32>,
    cost: Vec<u64>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
    values: Vec<u64>,
    /// The level and the cell of `graph`.
    mapped: Option<(usize, usize)>,
}

struct Input<'a> {
    cells: &'a Cells,
    first: &'a [u32],
    head: &'a [u32],
    profiles: &'a [Costs<'a>],
    columns_from: usize,
}

/// Level 1: the nodes are the roads of the cell, then its leaving roads. The arcs are the turns of
/// the roads of the cell, in topology order.
fn level1(input: &Input, cell: usize, graph: &mut Graph) -> Result<(), String> {
    let (cells, first, head) = (input.cells, input.first, input.head);
    graph.clear();
    for road in cells.roads(cell) {
        for arc in first[road]..first[road + 1] {
            graph.head.push(cells.target(cell, head[arc as usize])?);
        }
        graph.first.push(graph.head.len() as u32);
    }
    graph.rows.extend(0..cells.entering(1, cell).count() as u32);
    graph.leaving = cells.leaving_list(1, cell).len();
    Ok(())
}

/// Level 2 and up: the nodes are the entering roads of the children, in child order (so
/// ascending), then the leaving roads of the cell. The arcs of a node are the columns of its row in
/// the table of its child.
fn upper(input: &Input, level: usize, cell: usize, scratch: &mut Scratch) -> Result<(), String> {
    let cells = input.cells;
    let Scratch { graph, roads, starts, targets, .. } = scratch;
    let leaving = cells.leaving_list(level, cell);
    let fail = |what: &str| format!("{what} of level-{level} cell ID {:08x}", cells.id(level, cell));
    roads.clear();
    starts.clear();
    targets.clear();
    graph.clear();
    for child in cells.children(level, cell) {
        starts.push(roads.len());
        roads.extend(cells.entering(level - 1, child));
    }
    starts.push(roads.len());
    if !roads.is_sorted_by(|a, b| a < b) {
        return Err(fail("entering roads out of order in the children"));
    }
    let inner = roads.len();
    for (k, child) in cells.children(level, cell).enumerate() {
        let start = targets.len();
        for &road in cells.leaving_list(level - 1, child) {
            let node = match roads.binary_search(&road) {
                Ok(node) => node,
                Err(_) => inner + leaving.binary_search(&road).map_err(|_| fail("a child road outside the lists"))?,
            };
            targets.push(node as u32);
        }
        for _ in starts[k]..starts[k + 1] {
            graph.head.extend_from_slice(&targets[start..]);
            graph.first.push(graph.head.len() as u32);
        }
    }
    for road in cells.entering(level, cell) {
        graph.rows.push(roads.binary_search(&road).map_err(|_| fail("an entering road outside the children"))? as u32);
    }
    graph.leaving = leaving.len();
    Ok(())
}

/// The least cost from each row node to each leaving node, by row, into `values`: a Dijkstra search
/// that settles a leaving node and does not expand it.
fn search(graph: &Graph, cost: &mut Vec<u64>, heap: &mut BinaryHeap<Reverse<(u64, u32)>>, values: &mut Vec<u64>) {
    let inner = graph.first.len() - 1;
    values.clear();
    if graph.leaving == 0 {
        return;
    }
    for &start in &graph.rows {
        cost.clear();
        heap.clear();
        cost.resize(inner + graph.leaving, u64::MAX);
        cost[start as usize] = 0;
        heap.push(Reverse((0, start)));
        let mut open = graph.leaving;
        while let Some(Reverse((here, node))) = heap.pop() {
            let node = node as usize;
            if here > cost[node] {
                continue;
            }
            if node >= inner {
                open -= 1;
                if open == 0 {
                    break;
                }
                continue;
            }
            for arc in graph.first[node] as usize..graph.first[node + 1] as usize {
                let weight = graph.weight[arc];
                if weight == u64::MAX {
                    continue;
                }
                // A path has fewer than 2^32 arcs of at most 2^33 each: the sum stays below 2^64.
                let next = here + weight;
                let target = graph.head[arc];
                if next < cost[target as usize] {
                    cost[target as usize] = next;
                    heap.push(Reverse((next, target)));
                }
            }
        }
        values.extend_from_slice(&cost[inner..]);
    }
}

/// The table of cell `cell` of `level` for profile `p`, from the store of the level below.
fn task(
    input: &Input,
    below: &Tables,
    level: usize,
    cell: usize,
    p: usize,
    scratch: &mut Scratch,
) -> Result<Vec<u8>, String> {
    if scratch.mapped != Some((level, cell)) {
        scratch.mapped = None;
        match level {
            1 => level1(input, cell, &mut scratch.graph)?,
            _ => upper(input, level, cell, scratch)?,
        }
        scratch.mapped = Some((level, cell));
    }
    let (cells, costs) = (input.cells, &input.profiles[p]);
    let Scratch { graph, starts, cost, heap, values, .. } = scratch;
    graph.weight.clear();
    if level == 1 {
        let arcs = input.first[cells.roads(cell).start]..input.first[cells.roads(cell).end];
        graph.weight.extend(arcs.map(|arc| {
            let (turn, total) = (costs.turns[arc as usize], costs.totals[input.head[arc as usize] as usize]);
            match turn == u32::MAX || total == u32::MAX {
                true => u64::MAX,
                false => u64::from(turn) + u64::from(total),
            }
        }));
    } else {
        for (k, child) in cells.children(level, cell).enumerate() {
            let columns = cells.leaving_list(level - 1, child).len() as u32;
            let table = Table::parse(below.table(level - 1, child, p), (starts[k + 1] - starts[k]) as u32, columns)
                .map_err(|e| e.to_string())?;
            (0..table.rows).for_each(|row| graph.weight.extend(table.row(row)));
        }
    }
    search(graph, cost, heap, values);
    let (rows, columns) = (graph.rows.len() as u32, graph.leaving as u32);
    let mut table = Vec::new();
    encode_table(rows, columns, values, input.columns_from, &mut table).map_err(|value| {
        let id = cells.id(level, cell);
        format!("the cost table of profile {p}, level {level}, cell ID {id:08x} holds the value {value}, above 32 bits")
    })?;
    Ok(table)
}

/// Builds the tables of each level on the `rayon` pool, level after level. The tables of a level,
/// by cell and then profile, go in batches whose values take at most `BATCH_BYTES`, or more when
/// the batch needs it to hold one table for each thread. So the own memory of the stage is at most
/// about twice the larger of `BATCH_BYTES` and the values of the largest table times the threads,
/// with the values and their encoding; the memory guard stops the stage `tables` above its
/// budget. Each table is a function of its cell and profile alone, so the bytes do not depend on
/// the thread count, and an error is the first in table order. A table holds its values by column
/// too when they take more than `columns_from` bytes by row. Prints the figures of each level.
pub fn build(
    cells: &Cells,
    first: &[u32],
    head: &[u32],
    profiles: &[Costs],
    columns_from: usize,
) -> Result<Tables, String> {
    let input = Input { cells, first, head, profiles, columns_from };
    let roads = cells.level1.values().last().map_or(0, |last| last[1] as usize);
    let mut tables = Tables { profiles: profiles.len(), levels: Vec::new() };
    for level in 1..=cells.levels() {
        let started = Instant::now();
        let count = cells.counts.values()[level - 1] as usize;
        let sizes: Vec<u64> = (0..count)
            .map(|cell| cells.entering(level, cell).count() as u64 * cells.leaving_list(level, cell).len() as u64 * 8)
            .collect();
        let (units, threads, size) =
            (count * profiles.len(), rayon::current_num_threads(), |unit: usize| sizes[unit / profiles.len()]);
        let (mut bytes, mut at) = (Column::default(), Column::default());
        at.push(0u64);
        let mut start = 0;
        while start < units {
            let (mut end, mut batch) = (start + 1, size(start));
            while end < units && (batch + size(end) <= BATCH_BYTES || end - start < threads) {
                batch += size(end);
                end += 1;
            }
            let built: Vec<_> = (start..end)
                .into_par_iter()
                .with_max_len(1)
                .map_init(Scratch::default, |scratch, unit| {
                    task(&input, &tables, level, unit / profiles.len(), unit % profiles.len(), scratch)
                })
                .collect();
            for table in crate::cost::in_order(built)? {
                bytes.extend(&table);
                at.push(bytes.len() as u64);
            }
            start = end;
        }
        tables.levels.push(Level { bytes, at });
        let mut report = tables.report(level, roads);
        report["seconds"] = json!(started.elapsed().as_secs_f64());
        eprintln!("Tables: {report}");
    }
    Ok(tables)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::base::Topology;
    use crate::Graph as RoadGraph;
    use planner_router::model::{Point, Road, BIKE, FOOT, PUSH};

    /// The roads `(from, to)` on `junctions` points of a grid, ordered into cells of `sizes`.
    fn graph(points: Vec<Point>, ends: &[(u32, u32)], sizes: &[u32]) -> (RoadGraph, Topology) {
        let junctions = points.len();
        let roads: Vec<Road> = ends
            .iter()
            .map(|&(from, to)| {
                let shape = vec![points[from as usize], points[to as usize]];
                Road { from, to, way: 1, length_m: 100, access: BIKE | FOOT | PUSH, shape, ..Road::default() }
            })
            .collect();
        let mut graph = RoadGraph {
            points: points.into_iter().collect(),
            node_ids: (0..junctions as i64).collect(),
            node_access: std::iter::repeat_n(BIKE | FOOT | PUSH, junctions).collect(),
            signals: std::iter::repeat_n(0, junctions).collect(),
            node_doubts: std::iter::repeat_n(0, junctions).collect(),
            node_closures: std::iter::repeat_n(u32::MAX, junctions).collect(),
            roads: roads.into_iter().collect(),
            ..RoadGraph::default()
        };
        crate::layout::order(&mut graph, sizes, None).unwrap();
        let topology = Topology::new(&graph.roads, graph.points.len()).unwrap();
        (graph, topology)
    }

    /// A grid of 12 × 12 junctions: two-way roads, a one-way road on each fifth pair, a spur of
    /// three junctions that ends at a dead end, and an island.
    fn grid() -> (RoadGraph, Topology) {
        let side = 12u32;
        let point = |lat: i32, lon: i32| Point { lat, lon, elevation: 0.0 };
        let mut points: Vec<Point> = (0..side * side)
            .map(|i| point(46_000_000 + (i / side) as i32 * 10_000, 7_000_000 + (i % side) as i32 * 10_000))
            .collect();
        let mut ends = Vec::new();
        let pair = |a: u32, b: u32, k: usize, ends: &mut Vec<(u32, u32)>| {
            ends.push((a, b));
            if !k.is_multiple_of(5) {
                ends.push((b, a));
            }
        };
        let mut k = 0;
        for i in 0..side * side {
            for j in [i + 1, i + side] {
                if (j == i + 1 && i % side + 1 < side) || (j == i + side && j < side * side) {
                    pair(i, j, k, &mut ends);
                    k += 1;
                }
            }
        }
        for s in 0..3u32 {
            points.push(point(45_990_000 - s as i32 * 10_000, 7_000_000));
            let from = if s == 0 { 0 } else { side * side + s - 1 };
            pair(from, side * side + s, 1, &mut ends);
        }
        // An island of four junctions far away, the size of a level-1 cell: its cells have no
        // border road.
        let island = side * side + 3;
        for k in 0..4u32 {
            points.push(point(47_000_000 + (k / 2) as i32 * 10_000, 8_000_000 + (k % 2) as i32 * 10_000));
        }
        for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0)] {
            pair(island + a, island + b, 1, &mut ends);
        }
        graph(points, &ends, &[4, 16, 64])
    }

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// The test columns of three profiles: costs from a fixed xorshift; the same with a band of
    /// excluded roads and a forbidden turn; costs that need tables of width 4.
    fn profiles(graph: &RoadGraph, topology: &Topology) -> Vec<(Vec<u32>, Vec<u32>)> {
        let (roads, head, first) = (graph.roads.len(), topology.head.values(), topology.first.values());
        let mut state = 0x9E37_79B9_7F4A_7C15;
        let totals: Vec<u32> = (0..roads).map(|_| 1 + (xorshift(&mut state) % 400) as u32).collect();
        let turns: Vec<u32> = (0..head.len()).map(|_| (xorshift(&mut state) % 50) as u32).collect();
        let mut band = (totals.clone(), turns.clone());
        let lat = |road: usize| graph.points.values()[graph.roads.to(road) as usize].lat;
        for road in (0..roads).filter(|&road| (46_040_000..46_050_000).contains(&lat(road))) {
            band.0[road] = u32::MAX;
        }
        for road in 0..roads {
            let arcs = first[road] as usize..first[road + 1] as usize;
            for (arc, &target) in arcs.clone().zip(&head[arcs]) {
                if band.0[road] == u32::MAX || band.0[target as usize] == u32::MAX {
                    band.1[arc] = u32::MAX;
                }
            }
        }
        let forbidden = (0..head.len()).find(|&arc| band.1[arc] != u32::MAX).unwrap();
        band.1[forbidden] = u32::MAX;
        let large = (totals.iter().map(|&t| t * 1_000).collect(), turns.iter().map(|&t| t * 1_000).collect());
        vec![(totals, turns), band, large]
    }

    /// The least cost from `start` at cost 0 to each road, over all roads, expanding only the
    /// roads that `inside` accepts, with the weight of `Costs::arc` of the engine.
    fn reference(topology: &Topology, costs: &Costs, start: usize, inside: impl Fn(usize) -> bool) -> Vec<u64> {
        let (first, head) = (topology.first.values(), topology.head.values());
        let mut cost = vec![u64::MAX; first.len() - 1];
        let mut heap = BinaryHeap::from([Reverse((0u64, start))]);
        cost[start] = 0;
        while let Some(Reverse((here, road))) = heap.pop() {
            if here > cost[road] || (road != start && !inside(road)) {
                continue;
            }
            let arcs = first[road] as usize..first[road + 1] as usize;
            for (arc, &target) in arcs.clone().zip(&head[arcs]) {
                let target = target as usize;
                let (turn, total) = (costs.turns[arc], costs.totals[target]);
                if turn == u32::MAX || total == u32::MAX {
                    continue;
                }
                let next = here.checked_add(u64::from(turn) + u64::from(total)).unwrap();
                if next < cost[target] {
                    cost[target] = next;
                    heap.push(Reverse((next, target)));
                }
            }
        }
        cost
    }

    #[test]
    fn each_value_is_the_cost_of_a_road_search_in_its_cell() {
        let (graph, topology) = grid();
        let cells = &graph.cells;
        assert!(cells.levels() >= 3, "{} levels", cells.levels());
        let columns = profiles(&graph, &topology);
        let costs: Vec<Costs> = columns.iter().map(|(totals, turns)| Costs { totals, turns }).collect();
        let (first, head) = (topology.first.values(), topology.head.values());
        let tables = build(cells, first, head, &costs, 16).unwrap();
        let (mut finite, mut copies, mut widths, mut empty) = (0, 0, [false; 3], 0);
        for level in 1..=cells.levels() {
            for cell in 0..cells.counts.values()[level - 1] as usize {
                let entering: Vec<u32> = cells.entering(level, cell).collect();
                let leaving = cells.leaving_list(level, cell);
                let roads = cells.roads(cells.level1_cells(level, cell).start).start
                    ..cells.roads(cells.level1_cells(level, cell).end - 1).end;
                for (p, costs) in costs.iter().enumerate() {
                    let bytes = tables.table(level, cell, p);
                    let table = Table::parse(bytes, entering.len() as u32, leaving.len() as u32).unwrap();
                    if leaving.is_empty() {
                        assert_eq!(bytes.len(), TABLE_HEADER);
                        empty += 1;
                    }
                    let mut largest = None;
                    for (row, &e) in entering.iter().enumerate() {
                        let expected = reference(&topology, costs, e as usize, |road| roads.contains(&road));
                        for (column, &l) in leaving.iter().enumerate() {
                            let value = table.get(row as u32, column as u32);
                            assert_eq!(value, expected[l as usize], "level {level}, cell {cell}, profile {p}");
                            if value != u64::MAX {
                                finite += 1;
                                largest = largest.max(Some(value));
                            }
                        }
                    }
                    let width = planner_router::cells::width(largest.map(|value| value as u32));
                    assert_eq!(table.width, width, "the narrowest width");
                    widths[width.ilog2() as usize] = true;
                    let (rows, columns) = (table.rows as usize, table.columns as usize);
                    let copy = rows * columns * usize::from(width) > 16 && rows > 1 && columns > 1;
                    assert_eq!(table.layout, u8::from(copy), "the copy by column by its rule");
                    copies += usize::from(copy);
                    for column in 0..table.columns {
                        let by_row: Vec<u64> = (0..table.rows).map(|row| table.get(row, column)).collect();
                        assert_eq!(table.column(column).collect::<Vec<_>>(), by_row);
                    }
                }
            }
        }
        assert!(
            finite > 1000 && copies > 0 && empty > 0 && widths == [true; 3],
            "{finite} {copies} {empty} {widths:?}"
        );
    }

    #[test]
    fn a_value_above_32_bits_stops_the_bake() {
        // A ring of four junctions on a line, in cells of two: each cell has an entering road, an
        // inner road and a leaving road, the last two with costs of 2^31.
        let point = |lon: i32| Point { lat: 46_000_000, lon, elevation: 0.0 };
        let points = (0..4).map(|k| point(7_000_000 + k * 10_000)).collect();
        let (graph, topology) = graph(points, &[(0, 1), (1, 2), (2, 3), (3, 0)], &[2]);
        let totals = vec![1u32 << 31; 4];
        let turns = vec![0; topology.head.len()];
        let costs = [Costs { totals: &totals, turns: &turns }];
        let error = build(&graph.cells, topology.first.values(), topology.head.values(), &costs, 16).err().unwrap();
        assert!(error.contains("profile 0, level 1, cell ID") && error.contains("above 32 bits"), "{error}");
    }
}
