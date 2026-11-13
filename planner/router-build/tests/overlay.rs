//! The search with cost tables (`Region`) against a relaxation of the turns of the installed roads,
//! on random graphs with a full and a partial install.

use planner_router::cells::{Directory, Group, Topology};
use planner_router::data::RoutingData;
use planner_router::host::{Memory, Part};
use planner_router::install;
use planner_router::model::{Point, Profile, Road, BIKE, FOOT, PUSH};
use planner_router::overlay::{Query, Seed};
use planner_router::region::Region;
use planner_router::Control;
use planner_router_build::cells::{write, Info, Input};
use planner_router_build::{base, tables, Costs, Graph};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::fs::{self, File};
use std::path::{Path, PathBuf};

const LEVELS: usize = 3;

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// 150 to 300 junctions at random points, each with roads to its 2 to 4 nearest junctions, three
/// of four in both directions, and an island of four junctions. `None` when the partition has
/// fewer than 3 cells of level 3.
fn graph(state: &mut u64) -> Option<Graph> {
    let count = 150 + (xorshift(state) % 151) as usize;
    let point =
        |lat: u64, lon: u64| Point { lat: 46_000_000 + lat as i32, lon: 7_000_000 + lon as i32, elevation: 0.0 };
    let mut points: Vec<Point> =
        (0..count).map(|_| point(xorshift(state) % 100_000, xorshift(state) % 150_000)).collect();
    let mut ends = BTreeSet::new();
    for a in 0..count {
        let mut near: Vec<usize> = (0..count).filter(|&b| b != a).collect();
        near.sort_by_cached_key(|&b| {
            let (x, y) = (i64::from(points[a].lon - points[b].lon), i64::from(points[a].lat - points[b].lat));
            x * x + y * y
        });
        for &b in &near[..2 + (xorshift(state) % 3) as usize] {
            ends.insert((a as u32, b as u32));
            if !xorshift(state).is_multiple_of(4) {
                ends.insert((b as u32, a as u32));
            }
        }
    }
    points.extend((0..4).map(|k| point(1_000_000 + k / 2 * 10_000, 1_000_000 + k % 2 * 10_000)));
    for (a, b) in [(0, 1), (1, 3), (3, 2), (2, 0)] {
        ends.extend([(count as u32 + a, count as u32 + b), (count as u32 + b, count as u32 + a)]);
    }
    let roads: Vec<Road> = ends
        .iter()
        .map(|&(from, to)| {
            let shape = vec![points[from as usize], points[to as usize]];
            Road { from, to, way: 1, length_m: 100, access: BIKE | FOOT | PUSH, shape, ..Road::default() }
        })
        .collect();
    let junctions = points.len();
    let mut graph = Graph {
        points: points.into_iter().collect(),
        node_ids: (0..junctions as i64).collect(),
        node_access: std::iter::repeat_n(BIKE | FOOT | PUSH, junctions).collect(),
        signals: std::iter::repeat_n(0, junctions).collect(),
        node_doubts: std::iter::repeat_n(0, junctions).collect(),
        node_closures: std::iter::repeat_n(u32::MAX, junctions).collect(),
        roads: roads.into_iter().collect(),
        ..Graph::default()
    };
    planner_router_build::layout::order(&mut graph, &[4, 16, 64], None).unwrap();
    let cells = &graph.cells;
    (cells.levels() == LEVELS && cells.counts.values()[LEVELS - 1] >= 3).then_some(graph)
}

/// The columns of one profile: road costs 1 to 8, turn costs 0 to 3, and one road and one turn of
/// 16 with none.
fn columns(state: &mut u64, roads: usize, turns: usize) -> (Vec<u32>, Vec<u32>) {
    let mut column = |count: usize, values: u64, least: u32| -> Vec<u32> {
        (0..count)
            .map(|_| match xorshift(state) % 16 {
                0 => u32::MAX,
                _ => least + (xorshift(state) % values) as u32,
            })
            .collect()
    };
    (column(roads, 8, 1), column(turns, 4, 0))
}

/// A folder of the test that `Drop` removes.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The arcs out of each road: the target road and the weight.
type Arcs = Vec<Vec<(u32, u64)>>;

/// An install in memory: the engine on it, its roads, and for each profile the arcs as the views
/// of the parts give them, with no table.
struct Install {
    region: Region<Memory>,
    roads: Vec<u32>,
    arcs: Vec<(String, Arcs)>,
}

fn install(objects: &Path, target: &Path, cells: Option<&[u32]>) -> Install {
    let read = |file: &str| fs::read(objects.join(file)).unwrap();
    let (directory, info) = (read("directory.bin"), read("package.json"));
    let fetch = |name: &str| File::open(objects.join(name));
    install::install(target, (&directory, &info), (cells, None), 2, fetch).unwrap();
    let (_, plan) = install::installed(target).unwrap();
    let mut parts = HashMap::new();
    let mut memory = Memory::default();
    for item in &plan.files {
        let bytes = fs::read(target.join(install::CELLS).join(&item.name)).unwrap();
        memory.insert(Part { record: item.record, part: item.part }, bytes.clone());
        parts.insert((item.record, usize::from(item.part)), bytes);
    }
    let view = Directory::new(&directory).unwrap();
    let downloads: Vec<u32> = plan.records.iter().copied().filter(|&r| (r as usize) < view.downloads()).collect();
    let first: HashMap<u32, u32> = downloads
        .iter()
        .map(|&record| {
            let cell = view.cell(record as usize).unwrap();
            (cell.id, cell.first_road)
        })
        .collect();
    // Calls `visit` with the topology, the group part, each level-1 cell and the ID of its first road.
    let each = |group: usize, visit: &mut dyn FnMut(&Topology, &Group, u32, u32)| {
        for &record in &downloads {
            let cell = view.cell(record as usize).unwrap();
            let topology = Topology::new(&parts[&(record, 0)]).unwrap();
            let costs = Group::new(&parts[&(record, 2 + group)]).unwrap();
            for index in 0..topology.cells {
                let base = cell.first_road + topology.level1(index).unwrap().first_road;
                visit(&topology, &costs, index, base);
            }
        }
    };
    let package: serde_json::Value = serde_json::from_slice(&info).unwrap();
    let profiles: Vec<(u64, String)> = package["profiles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["group"].as_u64().unwrap(), entry["profile"]["name"].as_str().unwrap().to_string()))
        .collect();
    let total = view.header.roads as usize;
    let mut arcs = Vec::new();
    let mut roads = Vec::new();
    for (k, (group, name)) in profiles.iter().enumerate() {
        let place = profiles[..k].iter().filter(|other| other.0 == *group).count() as u32;
        let mut cost = vec![None; total];
        each(*group as usize, &mut |topology, costs, index, base| {
            let level1 = topology.level1(index).unwrap();
            let cell = costs.level1(index, &level1).unwrap();
            for road in 0..level1.roads {
                cost[(base + road) as usize] = costs.cost(&cell, place, road).unwrap();
                if k == 0 {
                    roads.push(base + road);
                }
            }
        });
        let mut list = vec![Vec::new(); total];
        each(*group as usize, &mut |topology, costs, index, base| {
            let level1 = topology.level1(index).unwrap();
            let cell = costs.level1(index, &level1).unwrap();
            for road in 0..level1.roads {
                for turn in topology.turns(&level1, road).unwrap() {
                    let head = topology.head(&level1, turn).unwrap();
                    let to = if head < level1.roads {
                        Some(base + head)
                    } else {
                        let leaving = topology.leaving(&level1, head - level1.roads).unwrap();
                        first.get(&leaving.cell).map(|first| first + leaving.index)
                    };
                    if let (Some(to), Some(turn)) = (to, costs.turn(&cell, turn).unwrap()) {
                        if let Some(cost) = cost[to as usize] {
                            list[(base + road) as usize].push((to, u64::from(cost) + u64::from(turn)));
                        }
                    }
                }
            }
        });
        arcs.push((name.clone(), list));
    }
    let marker = fs::read(target.join(install::MARKER)).unwrap();
    let mut region = Region::open(memory, directory, info, &marker).unwrap();
    region.set_memory_budget(usize::MAX);
    Install { region, roads, arcs }
}

/// The least (cost, end choice, start choice) of a path from a start seed to an end seed that
/// costs at most `ceiling`: the labels (cost, start choice) relax over the arcs to a fixed point.
fn oracle(arcs: &Arcs, starts: &[Seed], ends: &[Seed], ceiling: u64) -> Option<(u64, u8, u8)> {
    let mut labels = vec![(u64::MAX, u8::MAX); arcs.len()];
    let mut queue = VecDeque::new();
    for seed in starts {
        if (seed.cost, seed.choice) < labels[seed.node as usize] {
            labels[seed.node as usize] = (seed.cost, seed.choice);
            queue.push_back(seed.node);
        }
    }
    while let Some(road) = queue.pop_front() {
        let (cost, choice) = labels[road as usize];
        for &(to, weight) in &arcs[road as usize] {
            if (cost + weight, choice) < labels[to as usize] {
                labels[to as usize] = (cost + weight, choice);
                queue.push_back(to);
            }
        }
    }
    let joined = ends.iter().filter(|seed| labels[seed.node as usize].0 != u64::MAX).map(|seed| {
        let (cost, choice) = labels[seed.node as usize];
        (cost + seed.cost, seed.choice, choice)
    });
    joined.filter(|path| path.0 <= ceiling).min()
}

/// `count` seeds on roads of `from`, with costs 0 to 20, each with its own choice.
fn seeds(state: &mut u64, from: &[u32], count: usize) -> Vec<Seed> {
    let base = xorshift(state) % 200;
    (0..count as u64)
        .map(|k| {
            let node = from[(xorshift(state) % from.len() as u64) as usize];
            Seed { node, cost: xorshift(state) % 21, choice: (base + k) as u8 }
        })
        .collect()
}

/// What the paths of one kind of install showed, over all graphs.
#[derive(Default)]
struct Seen {
    paths: usize,
    levels: [usize; LEVELS + 1],
    /// A step of level 3 in the complete upper cell, and of level 2 in the partial one.
    complete: usize,
    partial: usize,
}

#[test]
fn the_search_gives_the_least_cost_and_candidates_on_random_graphs() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let scratch = Scratch(std::env::temp_dir().join(format!("obc-overlay-{}", std::process::id())));
    let profiles: Vec<Profile> =
        Profile::presets().into_iter().filter(|profile| ["hiking", "touring"].contains(&&*profile.name)).collect();
    let mut seen = [Seen::default(), Seen::default()];
    let mut graphs = 0;
    while graphs < 24 {
        let Some(graph) = graph(&mut state) else { continue };
        let cells = &graph.cells;
        // An upper cell, all of whose download cells the partial install holds, and the next one,
        // of which it holds about half.
        let Some(complete) =
            (0..cells.counts.values()[LEVELS - 1] as usize - 1).find(|&c| cells.children(LEVELS, c + 1).len() > 1)
        else {
            continue;
        };
        let half = cells.children(LEVELS, complete + 1);
        let half = half.start..half.start + half.len() / 2;
        let some: Vec<u32> = cells.children(LEVELS, complete).chain(half.clone()).map(|d| cells.id(2, d)).collect();
        graphs += 1;

        let topology = base::Topology::new(&graph.roads, graph.points.len()).unwrap();
        let (first, head) = (topology.first.values(), topology.head.values());
        let columns = profiles.iter().map(|_| columns(&mut state, graph.roads.len(), head.len())).collect();
        let costs = Costs::of_columns(profiles.clone(), columns);
        let tables = tables::build(cells, first, head, &costs.columns(), 16).unwrap();
        let _ = fs::remove_dir_all(&scratch.0);
        let info = Info {
            region: "random".into(),
            bounds: [7.0, 46.0, 9.0, 48.0],
            source_sha256: vec![],
            attribution: "test".into(),
        };
        let input = Input {
            graph: &graph,
            topology: &topology,
            costs: &costs,
            tables: &tables,
            sizes: &[4, 16, 64],
            download_level: 2,
            info,
            level: 1,
        };
        let objects = scratch.0.join("objects");
        write(&input, &objects).unwrap();

        // The level-1 cell of the junction where a road arrives or departs, and its cell of a level.
        let arrival = |road: u32| cells.cell(graph.roads.to(road as usize));
        let departure = |road: u32| cells.cell(graph.roads.from(road as usize));
        let cell = |level: usize, level1: usize| cells.ancestor(level, level1);
        for (kind, ids) in [None, Some(&some[..])].into_iter().enumerate() {
            let Install { mut region, roads, arcs } =
                install(&objects, &scratch.0.join(format!("install-{kind}")), ids);
            let held = |level1: usize| ids.is_none() || some.contains(&cells.id(2, cell(2, level1)));
            // The roads that leave the cell `c` of `level`, with both junctions in installed cells.
            let leaving = |level: usize, c: usize| -> Vec<u32> {
                let border = cells.leaving_list(level, c).iter().copied();
                border.filter(|&road| held(arrival(road)) && held(departure(road))).collect()
            };
            // The roads that arrive outside the cell of `level` where `road` departs.
            let apart = |road: u32, level: usize| -> Vec<u32> {
                let from = cell(level, departure(road));
                roads.iter().copied().filter(|&r| cell(level, arrival(r)) != from).collect()
            };
            let top: Vec<u32> =
                (0..cells.counts.values()[LEVELS - 1] as usize).flat_map(|c| leaving(LEVELS, c)).collect();
            for (name, arcs) in &arcs {
                for call in 0..40 {
                    let state = &mut state;
                    let counts = (1 + (xorshift(state) % 4) as usize, 1 + (xorshift(state) % 4) as usize);
                    let (mut starts, mut ends) = (seeds(state, &roads, counts.0), seeds(state, &roads, counts.1));
                    match (call, kind) {
                        // A start seed and an end seed on one road.
                        (0, _) => ends[0].node = starts[0].node,
                        // All seeds in one level-1 cell.
                        (1, _) => {
                            let home = arrival(starts[0].node);
                            let inside: Vec<u32> = roads.iter().copied().filter(|&r| arrival(r) == home).collect();
                            (starts, ends) = (seeds(state, &inside, counts.0), seeds(state, &inside, counts.1));
                        }
                        // A start seed, then an end seed, on a border road of the top level whose
                        // departure cell is no seed cell.
                        (2 | 3, _) if !top.is_empty() => {
                            let road = top[(xorshift(state) % top.len() as u64) as usize];
                            let others = apart(road, 1);
                            (starts, ends) = (seeds(state, &others, counts.0), seeds(state, &others, counts.1));
                            match call {
                                2 => starts[0].node = road,
                                _ => ends[0].node = road,
                            }
                        }
                        // The partial install: the seeds on two sides of the complete upper cell,
                        // then of a download cell of the partial upper cell.
                        (4 | 5, 1) => {
                            let (level, border) = match call {
                                4 => (LEVELS, leaving(LEVELS, complete)),
                                _ => (2, half.clone().flat_map(|d| leaving(2, d)).collect()),
                            };
                            if !border.is_empty() {
                                let road = border[(xorshift(state) % border.len() as u64) as usize];
                                starts = seeds(state, &apart(road, level), counts.0);
                                ends = vec![Seed { node: road, cost: 3, choice: 9 }];
                            }
                        }
                        _ => {}
                    }
                    // No ceiling, the true cost, and the true cost less 1.
                    let least = oracle(arcs, &starts, &ends, u64::MAX);
                    let ceiling = match (call % 3, least) {
                        (1, Some((cost, ..))) => cost,
                        (2, Some((cost, ..))) if cost > 0 => cost - 1,
                        _ => u64::MAX,
                    };
                    let expected = oracle(arcs, &starts, &ends, ceiling);
                    let query = Query {
                        starts: &starts,
                        ends: &ends,
                        ceiling,
                        max_steps: usize::MAX,
                        heap_bytes: usize::MAX,
                        heap_limit: usize::MAX,
                        cancelled: &|| false,
                    };
                    let found = region.search(name, query).unwrap();
                    let context = format!("graph {graphs}, install {kind}, {name}, call {call}");
                    let triple = found.as_ref().map(|f| (f.cost, ends[f.target].choice, starts[f.source].choice));
                    assert_eq!(triple, expected, "{context}");
                    let Some(found) = found else { continue };
                    let unpacked = region.unpack(name, &[&found], &Control::default()).unwrap().remove(0);
                    assert_eq!(unpacked[0], starts[found.source].node, "{context}");
                    assert_eq!(unpacked.last(), Some(&ends[found.target].node), "{context}");
                    let mut sum = starts[found.source].cost + ends[found.target].cost;
                    for pair in unpacked.windows(2) {
                        let turn = arcs[pair[0] as usize].iter().filter(|arc| arc.0 == pair[1]).map(|arc| arc.1).min();
                        sum += turn.unwrap_or_else(|| panic!("{context}: no turn from {} to {}", pair[0], pair[1]));
                    }
                    assert_eq!(sum, found.cost, "{context}");
                    let seen = &mut seen[kind];
                    seen.paths += 1;
                    for pair in found.path.windows(2) {
                        let level = usize::from(pair[1].level);
                        seen.levels[level] += 1;
                        let entered = cell(level.max(1), arrival(pair[0].road));
                        seen.complete += usize::from(level == LEVELS && entered == complete);
                        seen.partial += usize::from(level == 2 && half.contains(&entered));
                    }
                }
            }
        }
    }
    for (kind, seen) in seen.iter().enumerate() {
        println!("install {kind}: {} paths, steps by level {:?}", seen.paths, seen.levels);
        assert!(seen.paths > 600 && seen.levels.iter().all(|&steps| steps > 0), "install {kind}: {:?}", seen.levels);
    }
    assert!(seen[1].complete > 0 && seen[1].partial > 0, "{} and {}", seen[1].complete, seen[1].partial);
}
