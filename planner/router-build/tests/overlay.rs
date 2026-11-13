//! The search with cost tables (`Region`) against a relaxation of the turns of the installed roads,
//! on random graphs with a full and a partial install; and what the unpacking announces against
//! what it reads.

use planner_router::cells::{Directory, Group, Topology};
use planner_router::data::RoutingData;
use planner_router::host::{Host, Memory, Part, Span};
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
use std::sync::Mutex;

/// The cell sizes of the graphs, in turn. The download level is the level below the top, so the
/// second package has a level between level 1 and its download level.
const SIZES: [&[u32]; 2] = [&[4, 16, 64], &[4, 16, 64, 128]];

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

/// 150 to 300 junctions at random points, each with roads to its 2 to 4 nearest junctions, three
/// of four in both directions, and an island of four junctions, in cells of `sizes`. `None` when
/// the partition has fewer than 3 cells of level 3, or lacks a level.
fn graph(state: &mut u64, sizes: &[u32]) -> Option<Graph> {
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
    planner_router_build::layout::order(&mut graph, sizes, None).unwrap();
    let cells = &graph.cells;
    (cells.levels() == sizes.len() && cells.counts.values()[2] >= 3).then_some(graph)
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

/// The parts in memory, and each span that the engine announced.
struct Recorded {
    memory: Memory,
    announced: Mutex<Vec<Span>>,
}

impl Host for Recorded {
    fn range(&self, span: Span) -> planner_router::Result<&[u8]> {
        self.memory.range(span)
    }

    fn announce(&self, spans: &[Span]) {
        self.memory.announce(spans);
        self.announced.lock().unwrap().extend_from_slice(spans);
    }
}

/// A profile of an install: its name, its group and its place in the group, and its arcs as the
/// views of the parts give them, with no table.
struct Costed {
    name: String,
    group: usize,
    place: u32,
    arcs: Arcs,
}

/// An install in memory: the engine on it, its roads, its profiles and its files.
struct Install {
    region: Region<Recorded>,
    roads: Vec<u32>,
    profiles: Vec<Costed>,
    directory: Vec<u8>,
    parts: HashMap<(u32, usize), Vec<u8>>,
}

impl Install {
    /// The spans, as the views give them, of the two lists and the table of the cell `id` of
    /// `level` for `profile`; with `blocks`, of the topology block and the group block of a
    /// level-1 cell.
    fn spans(&self, level: u8, id: u32, profile: &Costed, blocks: bool) -> Vec<Span> {
        let view = Directory::new(&self.directory).unwrap();
        let record = view.find(level.max(view.header.download_level), id).unwrap().unwrap() as u32;
        let group = 2 + profile.group;
        let span = |part: usize, (at, bytes): (u32, u32)| Span { part: Part { record, part: part as u8 }, at, bytes };
        let topology = Topology::new(&self.parts[&(record, 0)]).unwrap();
        let costs = Group::new(&self.parts[&(record, group)]).unwrap();
        if level == 1 {
            let index = (0..topology.cells).find(|&k| topology.level1(k).unwrap().id == id).unwrap();
            let level1 = topology.level1(index).unwrap();
            let block = costs.level1(index, &level1).unwrap().span().unwrap();
            return match blocks {
                true => vec![span(0, topology.block(&level1).unwrap()), span(group, block)],
                false => {
                    vec![
                        span(0, topology.leaving_list(&level1).unwrap()),
                        span(group, costs.slot(profile.place, index).unwrap()),
                    ]
                }
            };
        }
        let upper = |k: u32| topology.upper(k).unwrap();
        let k = (0..topology.upper).find(|&k| (upper(k).level, upper(k).id) == (level, id)).unwrap();
        let (upper, table) = (upper(k), costs.slot(profile.place, costs.cells + k).unwrap());
        vec![
            span(0, (upper.entering_at, 8 * upper.entering)),
            span(0, (upper.leaving_at, 8 * upper.leaving)),
            span(group, table),
        ]
    }
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
    let mut costed = Vec::new();
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
        costed.push(Costed { name: name.clone(), group: *group as usize, place, arcs: list });
    }
    let marker = fs::read(target.join(install::MARKER)).unwrap();
    let host = Recorded { memory, announced: Mutex::default() };
    let mut region = Region::open(host, directory.clone(), info, &marker).unwrap();
    region.set_memory_budget(usize::MAX);
    Install { region, roads, profiles: costed, directory, parts }
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

/// What the paths of one kind of install showed, over the graphs of one size list.
#[derive(Default)]
struct Seen {
    paths: usize,
    levels: [usize; 5],
    /// A step of the top level in the complete upper cell, and of the download level in the
    /// partial one.
    complete: usize,
    partial: usize,
    /// The spans that the unpacking read and had announced.
    announced: usize,
}

#[test]
fn the_search_gives_the_least_cost_and_candidates_on_random_graphs() {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let scratch = Scratch(std::env::temp_dir().join(format!("obc-overlay-{}", std::process::id())));
    let profiles: Vec<Profile> =
        Profile::presets().into_iter().filter(|profile| ["hiking", "touring"].contains(&&*profile.name)).collect();
    let mut seen: [[Seen; 2]; 2] = Default::default();
    let mut graphs = 0;
    while graphs < 24 {
        let config = graphs % 2;
        let sizes = SIZES[config];
        let (top, download) = (sizes.len(), sizes.len() - 1);
        let Some(graph) = graph(&mut state, sizes) else { continue };
        let cells = &graph.cells;
        // An upper cell, all of whose download cells the partial install holds, and the next one,
        // of which it holds about half.
        let Some(complete) =
            (0..cells.counts.values()[top - 1] as usize - 1).find(|&c| cells.children(top, c + 1).len() > 1)
        else {
            continue;
        };
        let half = cells.children(top, complete + 1);
        let half = half.start..half.start + half.len() / 2;
        let some: Vec<u32> = cells.children(top, complete).chain(half.clone()).map(|d| cells.id(download, d)).collect();
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
            sizes,
            download_level: download as u8,
            info,
            level: 1,
        };
        let objects = scratch.0.join("objects");
        write(&input, &objects).unwrap();

        // The level-1 cell of the junction where a road arrives or departs, and its cell of a level.
        let arrival = |road: u32| cells.cell(graph.roads.to(road as usize));
        let departure = |road: u32| cells.cell(graph.roads.from(road as usize));
        let cell = |level: usize, level1: usize| cells.ancestor(level, level1);
        let border = cells.border.values();
        for (kind, ids) in [None, Some(&some[..])].into_iter().enumerate() {
            let mut data = install(&objects, &scratch.0.join(format!("install-{kind}")), ids);
            let roads = data.roads.clone();
            let held = |level1: usize| ids.is_none() || some.contains(&cells.id(download, cell(download, level1)));
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
            let borders: Vec<u32> =
                (0..cells.counts.values()[top - 1] as usize).flat_map(|c| leaving(top, c)).collect();
            for profile in 0..data.profiles.len() {
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
                        (2 | 3, _) if !borders.is_empty() => {
                            let road = borders[(xorshift(state) % borders.len() as u64) as usize];
                            let others = apart(road, 1);
                            (starts, ends) = (seeds(state, &others, counts.0), seeds(state, &others, counts.1));
                            match call {
                                2 => starts[0].node = road,
                                _ => ends[0].node = road,
                            }
                        }
                        // The partial install: the seeds on two sides of the complete upper cell,
                        // then of a download cell of the partial upper cell.
                        (4..=9, 1) => {
                            let (level, border) = match call % 2 {
                                0 => (top, leaving(top, complete)),
                                _ => (download, half.clone().flat_map(|d| leaving(download, d)).collect()),
                            };
                            if !border.is_empty() {
                                let road = border[(xorshift(state) % border.len() as u64) as usize];
                                starts = seeds(state, &apart(road, level), counts.0);
                                ends = vec![Seed { node: road, cost: 3, choice: 9 }];
                            }
                        }
                        _ => {}
                    }
                    let Costed { name, arcs, .. } = &data.profiles[profile];
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
                    let found = data.region.search(name, query).unwrap();
                    let context = format!("graph {graphs}, install {kind}, {name}, call {call}");
                    let triple = found.as_ref().map(|f| (f.cost, ends[f.target].choice, starts[f.source].choice));
                    assert_eq!(triple, expected, "{context}");
                    let Some(found) = found else { continue };
                    data.region.host().announced.lock().unwrap().clear();
                    let unpacked = data.region.unpack(name, &[&found], &Control::default()).unwrap().remove(0);
                    assert_eq!(unpacked[0], starts[found.source].node, "{context}");
                    assert_eq!(unpacked.last(), Some(&ends[found.target].node), "{context}");
                    let mut sum = starts[found.source].cost + ends[found.target].cost;
                    for pair in unpacked.windows(2) {
                        let turn = arcs[pair[0] as usize].iter().filter(|arc| arc.0 == pair[1]).map(|arc| arc.1).min();
                        sum += turn.unwrap_or_else(|| panic!("{context}: no turn from {} to {}", pair[0], pair[1]));
                    }
                    assert_eq!(sum, found.cost, "{context}");
                    let seen = &mut seen[config][kind];
                    seen.paths += 1;
                    // The cells that the unpacking of each step crosses below the level of the
                    // step, and its level-1 cells: a part of the roads of the step up to the next
                    // road that leaves the cell of that level. The unpacking reads the lists and
                    // the table of each of these cells, and the blocks of each level-1 cell.
                    let announced = data.region.host().announced.lock().unwrap().clone();
                    let mut read = Vec::new();
                    let mut at = 0;
                    for pair in found.path.windows(2) {
                        let level = usize::from(pair[1].level);
                        seen.levels[level] += 1;
                        let entered = cell(level.max(1), arrival(pair[0].road));
                        seen.complete += usize::from(level == top && entered == complete);
                        seen.partial += usize::from(level == download && half.contains(&entered));
                        let end = at + 1 + unpacked[at + 1..].iter().position(|&road| road == pair[1].road).unwrap();
                        for below in 1..=level {
                            let mut from = at;
                            for k in at + 1..=end {
                                if usize::from(border[unpacked[k] as usize]) >= below {
                                    let crossed = cell(below, arrival(unpacked[from]));
                                    let id = cells.id(below, crossed);
                                    if below < level {
                                        read.extend(data.spans(below as u8, id, &data.profiles[profile], false));
                                    }
                                    if below == 1 {
                                        read.extend(data.spans(1, id, &data.profiles[profile], true));
                                    }
                                    from = k;
                                }
                            }
                            assert_eq!(from, end, "{context}: the step ends at a road that leaves its cell");
                        }
                        at = end;
                    }
                    for span in read.iter().filter(|span| span.bytes > 0) {
                        let within =
                            |a: &Span| a.part == span.part && a.at <= span.at && span.at + span.bytes <= a.at + a.bytes;
                        assert!(
                            announced.iter().any(within),
                            "{context}: the unpacking read {span:?} without an announce"
                        );
                        seen.announced += 1;
                    }
                }
            }
        }
    }
    for (sizes, seen) in SIZES.iter().zip(&seen) {
        for (kind, seen) in seen.iter().enumerate() {
            let steps = &seen.levels[..=sizes.len()];
            assert!(seen.paths > 300 && steps.iter().all(|&steps| steps > 0), "{sizes:?}, install {kind}: {steps:?}");
            assert!(seen.announced > 1000, "{sizes:?}, install {kind}: {} spans", seen.announced);
        }
        assert!(seen[1].complete > 0 && seen[1].partial > 0, "{sizes:?}: {} and {}", seen[1].complete, seen[1].partial);
    }
}
