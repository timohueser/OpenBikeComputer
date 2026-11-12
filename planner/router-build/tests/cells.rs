use planner_router::cells::{self, Directory, Geometry, Group, Topology};
use planner_router::install::{self, Plan};
use planner_router::model::{Point, Profile, Road, Surface, BIKE, FOOT, NO_ELEVATION, PUSH};
use planner_router_build::cells::{write, Info, Input, Summary};
use planner_router_build::source::{Builder, Tags, Ways};
use planner_router_build::{base, Costs, Graph};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::{Path, PathBuf};

/// A grid of 10 × 10 junctions 0.01° apart, each pair of neighbours joined in both directions, and
/// one more road of 25 km from junction 0 to junction 1 by a point far to the north. In cells of at
/// most 8, 25 and 50 junctions.
fn graph() -> Graph {
    let side = 10u32;
    let points: Vec<Point> = (0..side * side)
        .map(|i| Point {
            lat: 46_000_000 + (i / side) as i32 * 10_000,
            lon: 7_000_000 + (i % side) as i32 * 10_000,
            elevation: if i % 7 == 0 { NO_ELEVATION } else { 400.0 + i as f32 * 1.25 },
        })
        .collect();
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for i in 0..side * side {
        if i % side + 1 < side {
            pairs.push((i, i + 1));
        }
        if i + side < side * side {
            pairs.push((i, i + side));
        }
    }
    pairs.push((0, 1));
    let (mut roads, mut ways) = (Vec::new(), Builder::<Ways>::default());
    for (k, &(a, b)) in pairs.iter().enumerate() {
        let way = 1000 + k as i64;
        let long = k == pairs.len() - 1;
        let far = Point { lat: points[a as usize].lat + 100_000, ..points[a as usize] };
        let access = [("access", "permit"), ("bicycle:conditional", "no @ (Nov-Mar)"), ("highway", "track")];
        let tags = match k % 5 {
            0 => Tags::from_pairs([access[0], ("highway", "track")]),
            1 => Tags::from_pairs([access[1], ("highway", "path")]),
            _ => Tags::from_pairs([("highway", "residential"), ("surface", "gravel")]),
        };
        ways.push(way, &[i64::from(a), i64::from(b)], &tags);
        for (from, to) in [(a, b), (b, a)] {
            let mut shape = vec![points[from as usize], points[to as usize]];
            if long {
                shape.insert(1, far);
            }
            roads.push(Road {
                from,
                to,
                way,
                reversed: from > to,
                length_m: if long { 25_000 } else { 780 },
                surface: [Surface::Paved, Surface::Gravel, Surface::Rough][k % 3],
                class: [1, 3, 4][k % 3],
                access: BIKE | FOOT | PUSH,
                difficulty: [0, 3, 6, 255][k % 4],
                hiking_difficulty: [None, Some(255), Some(2), Some(6)][k % 4],
                shape,
                ..Road::default()
            });
        }
    }
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
    planner_router_build::layout::order(&mut graph, &[8, 25, 50], None).unwrap();
    graph.osm.ways = ways.finish().0;
    graph.link_ways();
    graph
}

struct Bake {
    graph: Graph,
    topology: base::Topology,
    costs: Costs,
}

fn bake() -> Bake {
    let graph = graph();
    let profiles = Profile::presets();
    planner_router_build::check(&graph, &profiles).unwrap();
    let topology = base::Topology::new(&graph.roads, graph.points.len()).unwrap();
    let costs = planner_router_build::costs(&graph, &topology, &profiles).unwrap();
    Bake { graph, topology, costs }
}

/// A folder of the test that `Drop` removes.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("obc-cells-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn package(bake: &Bake, download_level: u8, output: &Path) -> Summary {
    let info = Info {
        region: "test".into(),
        bounds: [7.0, 46.0, 7.1, 46.1],
        source_sha256: vec![],
        attribution: "test".into(),
    };
    let input = Input {
        graph: &bake.graph,
        topology: &bake.topology,
        costs: &bake.costs,
        sizes: &[8, 25, 50],
        download_level,
        info,
        level: 3,
    };
    write(&input, output).unwrap()
}

fn read(dir: &Path) -> (Vec<u8>, Vec<u8>) {
    (fs::read(dir.join("directory.bin")).unwrap(), fs::read(dir.join("package.json")).unwrap())
}

fn fetch(dir: &Path) -> impl Fn(&str) -> std::io::Result<File> + Sync + '_ {
    move |name| File::open(dir.join(name))
}

/// The bytes of installed part `part` of record `record`.
fn file(target: &Path, directory: &Directory, record: usize, part: usize) -> Option<Vec<u8>> {
    let cell = directory.cell(record).unwrap();
    fs::read(target.join("cells").join(format!("{}-{:08x}-{part}", cell.level, cell.id))).ok()
}

/// The run-time ID of a road reference.
fn road_id(directory: &Directory, reference: cells::Reference) -> u32 {
    let level = directory.header.download_level;
    let record = directory.find(level, reference.cell).unwrap().unwrap();
    directory.cell(record).unwrap().first_road + reference.index
}

/// The level-`level` cell of each junction, numbered in its level.
fn cell_of(graph: &Graph, level: usize, junction: u32) -> usize {
    graph.cells.ancestor(level, graph.cells.cell(junction))
}

/// The entering and the leaving roads of each cell of each level, by (level, ID), from a direct
/// count over the roads.
fn borders(graph: &Graph) -> BTreeMap<(u8, u32), (Vec<u32>, Vec<u32>)> {
    let cells = &graph.cells;
    let roads = &graph.roads;
    let mut lists = BTreeMap::new();
    for level in 1..=cells.levels() {
        let below: u32 = cells.widths.values()[..level - 1].iter().map(|&w| u32::from(w)).sum();
        for road in 0..roads.len() {
            let (from, to) = (cell_of(graph, level, roads.from(road)), cell_of(graph, level, roads.to(road)));
            if from == to {
                continue;
            }
            let id = |junction: u32| cells.ids.values()[cells.cell(junction)] >> below << below;
            let entry: &mut (Vec<u32>, Vec<u32>) = lists.entry((level as u8, id(roads.to(road)))).or_default();
            entry.0.push(road as u32);
            let entry = lists.entry((level as u8, id(roads.from(road)))).or_default();
            entry.1.push(road as u32);
        }
    }
    lists
}

#[test]
fn an_install_gives_back_each_road() {
    let bake = bake();
    let (graph, costs) = (&bake.graph, &bake.costs);
    let scratch = Scratch::new("roads");
    let (output, target) = (scratch.0.join("out"), scratch.0.join("target"));
    let summary = package(&bake, 2, &output);
    let (directory_bytes, info) = read(&output);
    let report = install::install(&target, (&directory_bytes, &info), (None, None), 2, fetch(&output)).unwrap();
    assert_eq!(report.files as usize, summary.objects.len());
    assert_eq!(install::check(&directory_bytes, &info, 2, fetch(&output)).unwrap().files, report.files);
    let directory = Directory::new(&directory_bytes).unwrap();
    let downloads = directory.downloads();
    assert!(downloads >= 3 && directory.len() > downloads, "{downloads} download cells of {}", directory.len());
    assert_eq!(summary.long_roads, 2, "the long road has two directions");

    let info: serde_json::Value = serde_json::from_slice(&info).unwrap();
    let levels = graph.cells.levels();
    assert_eq!(info["levels"], serde_json::json!([8, 25, 50][..levels]));
    assert_eq!(
        (info["download_level"].as_u64(), info["counts"]["cells"].clone()),
        (Some(2), serde_json::json!(graph.cells.counts.values()))
    );
    let counts = &info["counts"];
    assert_eq!(
        (counts["roads"].as_u64(), counts["turns"].as_u64(), counts["long_roads"].as_u64()),
        (Some(graph.roads.len() as u64), Some(bake.topology.head.len() as u64), Some(2))
    );
    // The place of each profile in its group, by its index in `costs`.
    let mut places = BTreeMap::new();
    for entry in info["profiles"].as_array().unwrap() {
        let (name, group) = (entry["profile"]["name"].as_str().unwrap(), entry["group"].as_u64().unwrap() as usize);
        let index = costs.profiles.iter().position(|p| p.name == name).unwrap();
        let place = places.values().filter(|&&(g, _)| g == group).count();
        places.insert(index, (group, place as u32));
    }
    let groups = 1 + places.values().map(|&(g, _)| g).max().unwrap();
    assert_eq!(usize::from(directory.header.groups), groups);

    let bits = |road: &Road| road.shape.iter().map(|p| (p.lat, p.lon, p.elevation.to_bits())).collect::<Vec<_>>();
    let (first, head) = (bake.topology.first.values(), bake.topology.head.values());
    let mut found = BTreeMap::new();
    let union = |boxes: &mut dyn Iterator<Item = [i32; 4]>| {
        boxes.reduce(|a, b| [a[0].min(b[0]), a[1].min(b[1]), a[2].max(b[2]), a[3].max(b[3])]).unwrap_or_default()
    };
    // The level-1 boxes of each download cell, by record.
    let mut level1_boxes = Vec::new();
    let (mut seen, mut closures) = (0, 0);
    let (mut road, mut expected) = (Road::default(), Road::default());
    for record in 0..downloads {
        let cell = directory.cell(record).unwrap();
        let topology_bytes = file(&target, &directory, record, cells::TOPOLOGY).unwrap();
        let geometry_bytes = file(&target, &directory, record, cells::GEOMETRY).unwrap();
        let group_bytes: Vec<_> = (0..groups).map(|g| file(&target, &directory, record, 2 + g).unwrap()).collect();
        let topology = Topology::new(&topology_bytes).unwrap();
        let geometry = Geometry::new(&geometry_bytes).unwrap();
        let group_views: Vec<_> = group_bytes.iter().map(|bytes| Group::new(bytes).unwrap()).collect();
        let boxes: Vec<[i32; 4]> = (0..geometry.cells).map(|i| geometry.level1(i).unwrap().bounds).collect();
        assert_eq!(cell.bounds, union(&mut boxes.iter().copied()), "the box of record {record}");
        let longs = &mut (0..geometry.long).map(|k| geometry.long(k).unwrap().1);
        assert_eq!(cell.long_bounds, union(longs), "the long box of record {record}");
        level1_boxes.push(boxes);
        for k in 0..geometry.long {
            let (index, shape) = geometry.long(k).unwrap();
            let id = (cell.first_road + index) as usize;
            assert!(graph.roads.attributes(id).length_m > cells::LONG_ROAD_M);
            for point in graph.roads.shape(id) {
                let inside =
                    |b: [i32; 4]| b[0] <= point.lon && point.lon <= b[2] && b[1] <= point.lat && point.lat <= b[3];
                assert!(inside(shape) && inside(cell.long_bounds));
            }
        }
        for i in 0..topology.cells {
            let level1 = topology.level1(i).unwrap();
            let shapes = geometry.level1(i).unwrap();
            let first_id = cell.first_road + level1.first_road;
            let entering = (0..u32::from(level1.entering)).map(|k| first_id + k).collect::<Vec<_>>();
            let leaving =
                (0..u32::from(level1.leaving)).map(|k| road_id(&directory, topology.leaving(&level1, k).unwrap()));
            found.insert((1, level1.id), (entering, leaving.collect::<Vec<_>>()));
            for k in 0..u32::from(level1.entering) {
                let from = graph.roads.from((first_id + k) as usize);
                assert_eq!(topology.departure(&level1, k).unwrap(), graph.cells.ids.values()[graph.cells.cell(from)]);
            }
            for j in 0..level1.roads {
                let id = (first_id + j) as usize;
                seen += 1;
                // The turns, with the turn costs of each group.
                let mut turns: Vec<_> = (first[id]..first[id + 1]).map(|arc| (head[arc as usize], arc)).collect();
                turns.sort_unstable();
                let range = topology.turns(&level1, j).unwrap();
                let mut targets = Vec::new();
                for t in range.clone() {
                    let target = topology.head(&level1, range.start + (t - range.start)).unwrap();
                    let target = match target.checked_sub(level1.roads) {
                        None => first_id + target,
                        Some(k) => road_id(&directory, topology.leaving(&level1, k).unwrap()),
                    };
                    targets.push((target, t));
                }
                targets.sort_unstable();
                assert_eq!(
                    targets.iter().map(|t| t.0).collect::<Vec<_>>(),
                    turns.iter().map(|t| t.0).collect::<Vec<_>>()
                );
                for (profile, &(g, place)) in &places {
                    let view = &group_views[g];
                    let cell = view.level1(i, &level1).unwrap();
                    for (&(_, t), &(_, arc)) in targets.iter().zip(&turns) {
                        let turn = costs.turns(*profile)[arc as usize];
                        assert_eq!(view.turn(&cell, t).unwrap(), (turn != u32::MAX).then_some(turn));
                    }
                    let total = costs.total(*profile, id);
                    assert_eq!(view.cost(&cell, place, j).unwrap(), (total != u32::MAX).then_some(total));
                    let class = view.class(&cell, j).unwrap();
                    let basis = |b: Option<planner_router::cost::CostBasis>| {
                        b.map(|b| (b.factor.to_bits(), b.turn.to_bits(), b.ferry))
                    };
                    assert_eq!(basis(view.basis(class, place).unwrap()), basis(costs.basis(*profile, id)));
                    assert_eq!(view.allowed(&cell, place, j).unwrap(), costs.snappable(*profile, id));
                    assert_eq!(view.slot(place, i).unwrap().1, 0);
                }
                // The record.
                let closure = geometry.road(&shapes, j, &mut road).unwrap();
                graph.roads.get(id, &mut expected);
                assert_eq!(bits(&road), bits(&expected), "road {id}");
                let fields = |r: &Road| (r.way, r.length_m, r.surface, r.access, r.difficulty, r.hiking_difficulty);
                assert_eq!(fields(&road), fields(&expected));
                let set = graph.roads.closures(id);
                match closure {
                    0 => assert_eq!(set, u32::MAX),
                    k => {
                        assert_eq!(geometry.set(k - 1).unwrap(), graph.sets.get(set));
                        closures += 1;
                    }
                }
                // A point of a road that is not long lies in the box of its level-1 cell.
                if expected.length_m <= cells::LONG_ROAD_M {
                    let b = shapes.bounds;
                    assert!(expected
                        .shape
                        .iter()
                        .all(|p| b[0] <= p.lon && p.lon <= b[2] && b[1] <= p.lat && p.lat <= b[3]));
                }
            }
        }
        for k in 0..topology.upper {
            let upper = topology.upper(k).unwrap();
            let list =
                |at, n| (0..n).map(|k| road_id(&directory, topology.reference(u64::from(at), k).unwrap())).collect();
            found.insert(
                (upper.level, upper.id),
                (list(upper.entering_at, upper.entering), list(upper.leaving_at, upper.leaving)),
            );
        }
    }
    assert_eq!(seen, graph.roads.len());
    assert!(closures > 0, "the fixture has closures");
    // The upper cells: each is complete, so the install has its topology.
    for record in downloads..directory.len() {
        let bytes = file(&target, &directory, record, cells::TOPOLOGY).unwrap();
        let topology = Topology::new(&bytes).unwrap();
        let upper = topology.upper(0).unwrap();
        assert_eq!(topology.cells, 0);
        let cell = directory.cell(record).unwrap();
        let below: Vec<usize> = (0..downloads)
            .filter(|&d| directory.cell(d).unwrap().id & !directory.below(cell.level) == cell.id)
            .collect();
        let first = directory.cell(below[0]).unwrap().first_road;
        let roads: u32 = below.iter().map(|&d| directory.cell(d).unwrap().roads).sum();
        assert_eq!((cell.first_road, cell.roads, cell.long_bounds), (first, roads, [0; 4]));
        assert_eq!(cell.bounds, union(&mut below.iter().flat_map(|&d| level1_boxes[d].iter().copied())));
        let list = |at, n| (0..n).map(|k| road_id(&directory, topology.reference(u64::from(at), k).unwrap())).collect();
        found.insert(
            (upper.level, upper.id),
            (list(upper.entering_at, upper.entering), list(upper.leaving_at, upper.leaving)),
        );
    }
    let expected = borders(graph);
    for (key, lists) in &expected {
        assert_eq!(found.get(key), Some(lists), "the border roads of cell {key:?}");
    }
    assert!(found.iter().all(|(key, lists)| expected.contains_key(key) || (lists.0.is_empty() && lists.1.is_empty())));

    // No level-1 box holds the far point of the long road.
    let far = graph.roads.iter().find(|road| road.length_m > cells::LONG_ROAD_M).unwrap().shape[1];
    for record in 0..downloads {
        let geometry_bytes = file(&target, &directory, record, cells::GEOMETRY).unwrap();
        let geometry = Geometry::new(&geometry_bytes).unwrap();
        for i in 0..geometry.cells {
            let b = geometry.level1(i).unwrap().bounds;
            assert!(!(b[0] <= far.lon && far.lon <= b[2] && b[1] <= far.lat && far.lat <= b[3]));
        }
    }
}

#[test]
fn an_install_of_some_cells_holds_those_cells() {
    let bake = bake();
    let scratch = Scratch::new("some");
    let (output, target) = (scratch.0.join("out"), scratch.0.join("target"));
    package(&bake, 2, &output);
    let (directory_bytes, info) = read(&output);
    let directory = Directory::new(&directory_bytes).unwrap();
    // The first and the last download cell, in two upper cells that each hold more cells.
    let last = directory.downloads() - 1;
    let ids: Vec<u32> = [0, last].map(|record| directory.cell(record).unwrap().id).into();
    let upper = |id: u32| id & !directory.below(directory.header.download_level + 1);
    let count = |id: u32| (0..=last).filter(|&r| upper(directory.cell(r).unwrap().id) == upper(id)).count();
    assert!(upper(ids[0]) != upper(ids[1]) && count(ids[0]) > 1 && count(ids[1]) > 1);
    let names = |target: &Path| {
        let mut names: Vec<String> = fs::read_dir(target.join("cells"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    };
    let level = directory.header.download_level;
    // The profile `hiking` and its group only.
    let hiking = ["hiking".to_string()];
    let selection = (Some(&ids[..2]), Some(&hiking[..]));
    let two = install::install(&target, (&directory_bytes, &info), selection, 2, fetch(&output)).unwrap();
    let plan = install::plan(&directory_bytes, &info, Some(&ids[..2]), Some(&hiking)).unwrap();
    let package: serde_json::Value = serde_json::from_slice(&info).unwrap();
    let entry = package["profiles"].as_array().unwrap().iter().find(|p| p["profile"]["name"] == "hiking").unwrap();
    let group = entry["group"].as_u64().unwrap() as u8;
    assert_eq!(plan.groups, 1 << group);
    assert!(plan.files.iter().all(|file| file.part < 2 || file.part == 2 + group));
    assert_eq!(plan.files.len(), 2 * 3, "topology, geometry and one group of two cells");
    assert_eq!(plan.records, [0, last as u32], "no upper cell is complete");
    assert_eq!(install::installed_bytes(&plan.missing(&target)), 0);
    assert_eq!(two.files as usize, plan.files.len());
    let prefixes = |ids: &[u32]| ids.iter().map(|id| format!("{level}-{id:08x}-")).collect::<Vec<_>>();
    assert!(names(&target).iter().all(|name| prefixes(&ids[..2]).iter().any(|prefix| name.starts_with(prefix))));
    assert_eq!(names(&target).len(), plan.files.len());

    fs::write(target.join("notes"), b"kept").unwrap();
    let one =
        install::install(&target, (&directory_bytes, &info), (Some(&ids[..1]), Some(&hiking[..])), 1, fetch(&output))
            .unwrap();
    assert_eq!(
        (one.files, one.removed as usize),
        (0, plan.files.len() - plan.files.iter().filter(|f| f.record == 0).count())
    );
    assert!(names(&target).iter().all(|name| name.starts_with(&prefixes(&ids[..1])[0])));
    assert_eq!(fs::read(target.join("notes")).unwrap(), b"kept");
    let (_, installed) = install::installed(&target).unwrap();
    assert_eq!(installed.records, [0]);

    // A turn into a cell that is not installed names a road that no installed cell holds.
    let topology_bytes = file(&target, &directory, 0, cells::TOPOLOGY).unwrap();
    let topology = Topology::new(&topology_bytes).unwrap();
    let outside = (0..topology.cells)
        .flat_map(|i| {
            let level1 = topology.level1(i).unwrap();
            (0..u32::from(level1.leaving)).map(move |k| (level1, k)).collect::<Vec<_>>()
        })
        .map(|(level1, k)| topology.leaving(&level1, k).unwrap())
        .find(|reference| reference.cell != ids[0])
        .expect("a road leaves the installed cell");
    let road = road_id(&directory, outside);
    let holder = directory.download_of_road(road).unwrap().unwrap();
    assert!(!installed.records.contains(&(holder as u32)));
}

#[test]
fn an_install_refuses_what_it_cannot_trust() {
    let bake = bake();
    let scratch = Scratch::new("trust");
    let (output, target) = (scratch.0.join("out"), scratch.0.join("target"));
    package(&bake, 2, &output);
    let (directory_bytes, info) = read(&output);
    let all = install::plan(&directory_bytes, &info, None, None).unwrap();

    // One changed byte in an object.
    let damaged = scratch.0.join("damaged");
    fs::create_dir(&damaged).unwrap();
    for entry in fs::read_dir(&output).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), damaged.join(entry.file_name())).unwrap();
    }
    let name: String = all.files[1].object.iter().map(|b| format!("{b:02x}")).collect();
    let mut bytes = fs::read(damaged.join(&name)).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    fs::write(damaged.join(&name), bytes).unwrap();
    let error = install::install(&target, (&directory_bytes, &info), (None, None), 2, fetch(&damaged)).unwrap_err();
    assert!(matches!(&error, install::Error::Object { object, .. } if *object == name), "{error}");
    assert!(!target.join(install::MARKER).exists());
    assert!(install::check(&directory_bytes, &info, 2, fetch(&damaged)).is_err());

    // A folder of another package.
    let other = scratch.0.join("other");
    package(&bake, 1, &other);
    let (other_directory, other_info) = read(&other);
    assert_ne!(other_directory, directory_bytes);
    let error = install::install(&target, (&other_directory, &other_info), (None, None), 2, fetch(&other)).unwrap_err();
    assert!(matches!(&error, install::Error::Invalid(text) if text.contains("another package")), "{error}");

    // A second call while the lock is held.
    let lock = File::create(target.join(install::LOCK)).unwrap();
    lock.try_lock().unwrap();
    let error = install::install(&target, (&directory_bytes, &info), (None, None), 2, fetch(&output)).unwrap_err();
    assert!(matches!(error, install::Error::Locked(_)), "{error}");
    drop(lock);

    // A frame that decodes to more than its part, with the content size of the part, writes no
    // more than the part; a later install deletes the leftover `.part` file.
    let item = &all.files[0];
    let part = fs::read(output.join(item.object.iter().map(|b| format!("{b:02x}")).collect::<String>())).unwrap();
    let part = zstd::stream::decode_all(&part[..]).unwrap();
    let mut longer = part.clone();
    longer.extend(vec![7u8; 1 << 20]);
    let mut frame = zstd::bulk::Compressor::new(3).unwrap();
    frame.include_contentsize(false).unwrap();
    let mut frame = frame.compress(&longer).unwrap();
    // A frame header without a size: magic, a descriptor with the single-segment bit clear, a
    // window byte. Insert a size field of 4 bytes with the size of the part.
    assert_eq!(frame[4] & 0xC0, 0);
    frame[4] |= 0x80;
    let size = (item.bytes as u32).to_le_bytes();
    frame.splice(6..6, size);
    let lying = scratch.0.join("lying");
    fs::create_dir(&lying).unwrap();
    for entry in fs::read_dir(&output).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), lying.join(entry.file_name())).unwrap();
    }
    let object: String = item.object.iter().map(|b| format!("{b:02x}")).collect();
    fs::write(lying.join(&object), &frame).unwrap();
    let fresh = scratch.0.join("fresh");
    let error = install::install(&fresh, (&directory_bytes, &info), (None, None), 1, fetch(&lying)).unwrap_err();
    assert!(matches!(&error, install::Error::Object { object: o, .. } if *o == object), "{error}");
    let leftover = fresh.join("cells").join(format!("{}.part", item.name));
    assert!(fs::metadata(&leftover).unwrap().len() <= u64::from(item.bytes));
    install::install(&fresh, (&directory_bytes, &info), (None, None), 1, fetch(&output)).unwrap();
    assert!(!leftover.exists());

    // One changed byte in an installed file.
    install::install(&target, (&directory_bytes, &info), (None, None), 2, fetch(&output)).unwrap();
    assert_eq!(install::scrub(&target).unwrap(), Vec::<String>::new());
    let Plan { files, .. } = all;
    let path = target.join("cells").join(&files[0].name);
    let mut bytes = fs::read(&path).unwrap();
    bytes[0] ^= 1;
    fs::write(&path, bytes).unwrap();
    assert_eq!(install::scrub(&target).unwrap(), [files[0].name.clone()]);
}

#[test]
fn the_bytes_do_not_depend_on_the_threads() {
    let bake = bake();
    let scratch = Scratch::new("threads");
    let packages: Vec<_> = [1, 4]
        .into_iter()
        .map(|threads| {
            let output = scratch.0.join(threads.to_string());
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| package(&bake, 2, &output));
            read(&output)
        })
        .collect();
    assert_eq!(packages[0], packages[1]);
}

#[test]
fn a_download_level_above_the_levels_takes_the_top_level() {
    let bake = bake();
    let scratch = Scratch::new("clamp");
    let output = scratch.0.join("out");
    package(&bake, 8, &output);
    let (directory_bytes, info) = read(&output);
    let directory = Directory::new(&directory_bytes).unwrap();
    let levels = bake.graph.cells.levels();
    assert_eq!(usize::from(directory.header.download_level), levels);
    assert_eq!(directory.downloads(), directory.len(), "no upper cell");
    assert_eq!(directory.len(), bake.graph.cells.counts.values()[levels - 1] as usize);
    let info: serde_json::Value = serde_json::from_slice(&info).unwrap();
    assert_eq!(info["download_level"].as_u64(), Some(levels as u64));
}
