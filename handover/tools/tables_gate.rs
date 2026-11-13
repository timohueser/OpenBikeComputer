//! Not for merge. `tables_gate ROADS COSTS` compares the cost tables of a costs layer with a plain
//! road search: for sampled cells of each level and each profile, from each sampled entering road,
//! a Dijkstra search over all roads of the region from that road at cost 0, which expands only the
//! roads whose index is in the road range of the cell, with the weight of `Costs::arc` of the
//! engine (turn cost plus the total of the target road, none when either is u32::MAX) in checked
//! u64. The entering list comes from the departure junction of each road of the cell.

use planner_router::cells::Table;
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::path::Path;

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

struct Search {
    cost: Vec<u64>,
    touched: Vec<u32>,
    heap: BinaryHeap<Reverse<(u64, u32)>>,
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (graph, topology, costs, tables) = planner_router_build::step::layers(Path::new(&args[0]), Path::new(&args[1]))?;
    let cells = &graph.cells;
    let (first, head) = (topology.first.values(), topology.head.values());
    let columns = costs.columns();
    let roads = graph.roads.len();
    // For each level: the cells to sample, and the rows to sample in each (all when 0).
    let plan: [(usize, usize); 8] = [(60, 0), (25, 0), (8, 12), (4, 8), (3, 6), (2, 4), (2, 4), (2, 4)];
    let mut state = 0x2545_F491_4F6C_DD1D_u64;
    let mut tasks = Vec::new();
    for level in 1..=cells.levels() {
        let count = cells.counts.values()[level - 1] as usize;
        let (sample, rows) = plan[level - 1];
        let mut chosen: Vec<usize> = match std::env::var("ALL") {
            Ok(_) => (0..count).collect(),
            Err(_) => (0..sample).map(|_| xorshift(&mut state) as usize % count).collect(),
        };
        // The cells with a table of width 4 or a copy by column, where they exist.
        let wide: fn(&Table) -> bool = |t| t.width == 4;
        let copied: fn(&Table) -> bool = |t| t.layout == 1;
        for wanted in [wide, copied] {
            let found = (0..count).find(|&c| {
                (0..columns.len()).any(|p| {
                    let raw = tables.table(level, c, p);
                    let size = |at: usize| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
                    wanted(&Table::parse(raw, size(0), size(4)).unwrap())
                })
            });
            chosen.extend(found);
        }
        chosen.sort_unstable();
        chosen.dedup();
        for cell in chosen {
            let l1 = cells.level1_cells(level, cell);
            let range = cells.roads(l1.start).start..cells.roads(l1.end - 1).end;
            let inside = |junction: u32| l1.contains(&cells.cell(junction));
            let entering: Vec<u32> =
                range.clone().filter(|&r| !inside(graph.roads.from(r))).map(|r| r as u32).collect();
            let listed: Vec<u32> = cells.entering(level, cell).collect();
            if entering != listed {
                return Err(format!("the entering list of level {level} cell {cell} differs"));
            }
            let picked: Vec<usize> = match rows {
                _ if std::env::var("ALL").is_ok() => (0..entering.len()).collect(),
                0 => (0..entering.len()).collect(),
                n => (0..n.min(entering.len())).map(|_| xorshift(&mut state) as usize % entering.len()).collect(),
            };
            for row in picked {
                for p in 0..columns.len() {
                    tasks.push((level, cell, range.clone(), entering[row], row as u32, p));
                }
            }
        }
    }
    eprintln!("{} searches", tasks.len());
    let started = std::time::Instant::now();
    let results: Vec<Result<[u64; 4], String>> = tasks
        .par_iter()
        .map_init(
            || Search { cost: vec![u64::MAX; roads], touched: Vec::new(), heap: BinaryHeap::new() },
            |s, (level, cell, range, start, row, p)| {
                let (level, cell, p, start) = (*level, *cell, *p, *start as usize);
                let c = &columns[p];
                for &road in &s.touched {
                    s.cost[road as usize] = u64::MAX;
                }
                s.touched.clear();
                s.heap.clear();
                s.cost[start] = 0;
                s.touched.push(start as u32);
                s.heap.push(Reverse((0, start as u32)));
                while let Some(Reverse((here, road))) = s.heap.pop() {
                    let road = road as usize;
                    if here > s.cost[road] || (road != start && !range.contains(&road)) {
                        continue;
                    }
                    for arc in first[road] as usize..first[road + 1] as usize {
                        let target = head[arc] as usize;
                        let (turn, total) = (c.turns[arc], c.totals[target]);
                        if turn == u32::MAX || total == u32::MAX {
                            continue;
                        }
                        let next = here.checked_add(u64::from(turn) + u64::from(total)).ok_or("overflow")?;
                        if next < s.cost[target] {
                            if s.cost[target] == u64::MAX {
                                s.touched.push(target as u32);
                            }
                            s.cost[target] = next;
                            s.heap.push(Reverse((next, target as u32)));
                        }
                    }
                }
                let leaving = cells.leaving_list(level, cell);
                // Each road outside the cell that the search reached is in the leaving list.
                for &road in &s.touched {
                    if !range.contains(&(road as usize)) && leaving.binary_search(&road).is_err() {
                        return Err(format!("level {level} cell {cell}: road {road} reached outside the list"));
                    }
                }
                let raw = tables.table(level, cell, p);
                let table = Table::parse(raw, cells.entering(level, cell).count() as u32, leaving.len() as u32)
                    .map_err(|e| e.to_string())?;
                let (mut checked, mut differ, mut finite) = (0, 0, 0);
                for (q, &l) in leaving.iter().enumerate() {
                    let value = table.get(*row, q as u32);
                    checked += 1;
                    finite += u64::from(value != u64::MAX);
                    if value != s.cost[l as usize] {
                        differ += 1;
                        eprintln!("DIFF level {level} cell {cell} row {row} column {q} profile {p}: {value} != {}", s.cost[l as usize]);
                    }
                }
                Ok([level as u64, checked, differ, finite])
            },
        )
        .collect();
    let mut by_level = vec![[0u64; 4]; cells.levels()];
    for result in results {
        let [level, checked, differ, finite] = result?;
        let entry = &mut by_level[level as usize - 1];
        (entry[0], entry[1], entry[2], entry[3]) = (entry[0] + 1, entry[1] + checked, entry[2] + differ, entry[3] + finite);
    }
    for (level, [rows, checked, differ, finite]) in by_level.iter().enumerate() {
        println!("level {}: {rows} rows (row × profile), {checked} entries checked, {finite} finite, {differ} differ", level + 1);
    }
    let total: u64 = by_level.iter().map(|e| e[1]).sum();
    let differ: u64 = by_level.iter().map(|e| e[2]).sum();
    println!("all: {total} entries checked, {differ} differ, {:.0} s", started.elapsed().as_secs_f64());
    // The tables of each level compressed as #3301 compresses an object: zstd level 12, window log
    // 23, in pieces of 1 MiB (about the median object).
    for level in 1..=cells.levels() {
        let mut store = Vec::new();
        for cell in 0..cells.counts.values()[level - 1] as usize {
            (0..columns.len()).for_each(|p| store.extend(tables.table(level, cell, p)));
        }
        let mut compressor = zstd::bulk::Compressor::new(12).map_err(|e| e.to_string())?;
        compressor.set_parameter(zstd::zstd_safe::CParameter::WindowLog(23)).map_err(|e| e.to_string())?;
        let packed: usize = store.chunks(1 << 20).map(|piece| compressor.compress(piece).unwrap().len()).sum();
        println!("level {level}: {} table bytes, {packed} compressed", store.len());
    }
    Ok(())
}
