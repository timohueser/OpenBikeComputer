use super::*;
use crate::{
    source::{Node, Way},
    Graph,
};
use planner_router::{
    model::{Point, Profile, Road, Surface, BIKE, FOOT, PUSH},
    package::{digest, Package},
};
use std::sync::Arc;

#[derive(Clone)]
struct Memory(Arc<HashMap<String, Vec<u8>>>);
impl Source for Memory {
    fn read(&self, key: &str) -> planner_router::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| planner_router::Error::MissingRegion(key.into()))
    }
}

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

/// The way between grid nodes `a` and `b`.
fn way(a: i64, b: i64) -> i64 {
    1000 * a.min(b) + a.max(b)
}

/// Inside one zoom 9 cell.
const ORIGIN: i32 = 100_000;

/// A 7 by 7 grid of streets 445 m apart on rolling ground, with node `10 * row + column`, and its
/// source objects. The way from node 0 to node 1 is T3, and 10 to 11 is S2.
fn grid(relations: Vec<Relation>, advanced: bool) -> (Package<Memory>, Data) {
    grid_with(relations, advanced, |_| {})
}

fn grid_with(relations: Vec<Relation>, advanced: bool, change: impl FnOnce(&mut Graph)) -> (Package<Memory>, Data) {
    let mut graph = Graph::default();
    let mut index = HashMap::new();
    for row in 0..7 {
        for column in 0..7 {
            let id = 10 * row + column;
            let point = Point {
                lat: ORIGIN + row as i32 * 4_000,
                lon: ORIGIN + column as i32 * 4_000,
                elevation: ((row * 3 + column * 7) % 5) as f32 * 20.0,
            };
            index.insert(id, graph.points.len() as u32);
            graph.points.push(point);
            graph.node_ids.push(id);
            graph.node_access.push(BIKE | FOOT | PUSH);
            graph.osm.nodes.insert(id, Node { id, point, tags: Tags::new() });
        }
    }
    for row in 0..7 {
        for column in 0..7 {
            let a = 10 * row + column;
            for b in [a + 1, a + 10].into_iter().filter(|b| b % 10 < 7 && b / 10 < 7) {
                let id = way(a, b);
                let hiking_difficulty = (id == way(0, 1)).then_some(3);
                let difficulty = if id == way(10, 11) { 2 } else { 255 };
                let mut source_tags = tags(&[("highway", "residential")]);
                if hiking_difficulty.is_some() {
                    source_tags.insert("sac_scale".into(), "demanding_mountain_hiking".into());
                }
                if difficulty != 255 {
                    source_tags.insert("mtb:scale".into(), difficulty.to_string());
                }
                graph.osm.ways.insert(id, Way { id, nodes: vec![a, b], tags: source_tags });
                for (from, to) in [(index[&a], index[&b]), (index[&b], index[&a])] {
                    let shape = vec![graph.points[from as usize], graph.points[to as usize]];
                    graph.roads.push(Road {
                        from,
                        to,
                        way: id,
                        reversed: from > to,
                        length_m: shape[0].distance(shape[1]).round() as u32,
                        ascent_m: (shape[1].elevation - shape[0].elevation).max(0.0) as u32,
                        descent_m: (shape[0].elevation - shape[1].elevation).max(0.0) as u32,
                        surface: Surface::Paved,
                        class: 1,
                        access: BIKE | FOOT | PUSH,
                        difficulty,
                        hiking_difficulty,
                        structure: false,
                        shape,
                    });
                }
            }
        }
    }
    graph.osm.relations = relations.into_iter().map(|r| (r.id, r)).collect();
    change(&mut graph);
    let profiles: Vec<_> = Profile::presets()
        .into_iter()
        .filter(|p| {
            PROFILES.contains(&p.name.as_str())
                || advanced && matches!(p.name.as_str(), "mtb/technical" | "hiking/alpine")
        })
        .collect();
    let mut objects = HashMap::new();
    let manifest =
        crate::prepare(&graph, "test".into(), [-1.0, -1.0, 1.0, 1.0], &profiles, vec![], "test".into(), |bytes| {
            objects.insert(digest(bytes), bytes.to_vec());
            Ok(digest(bytes))
        })
        .unwrap();
    (Package::open(Memory(Arc::new(objects)), &serde_json::to_vec(&manifest).unwrap()).unwrap(), graph.osm)
}

fn route(id: i64, pairs: &[(&str, &str)], ways: impl IntoIterator<Item = i64>) -> Relation {
    let members = ways.into_iter().map(|w| (Id::Way(w), String::new())).collect();
    Relation { id, tags: tags(&[[("type", "route")].as_slice(), pairs].concat()), members }
}

fn path(nodes: &[i64]) -> Vec<i64> {
    nodes.windows(2).map(|w| way(w[0], w[1])).collect()
}

#[test]
fn catalog_shapes_routes_patches_short_gaps_and_joins_stages() -> Result<(), String> {
    let trail = [
        ("route", "hiking"),
        ("name", "Trail"),
        ("network", "rwn"),
        ("osmc:symbol", "red"),
        ("url", "https://t.example"),
    ];
    let long = |id, children: &[i64]| Relation {
        id,
        tags: tags(&[("type", "superroute"), ("route", "hiking"), ("name", "Long"), ("network", "rwn")]),
        members: children.iter().map(|&c| (Id::Relation(c), String::new())).collect(),
    };
    let mut corner = path(&[0, 1, 2, 3, 4, 14, 24, 34, 44]);
    corner.remove(5);
    let (package, osm) = grid(
        vec![
            // East, then north: the router needs a shaping point near the corner. The way from node
            // 14 to node 24 is missing, a gap of 445 m.
            route(1, &trail, corner),
            // A ring around four blocks.
            route(
                2,
                &[("route", "foot"), ("ref", "R"), ("network", "rwn")],
                path(&[44, 45, 46, 56, 66, 65, 64, 54, 44]),
            ),
            // Two parts 1.3 km apart.
            route(3, &trail, [path(&[0, 10, 20, 30]), path(&[5, 6, 16])].concat()),
            route(4, &[("route", "mtb"), ("name", "Planned"), ("state", "proposed")], path(&[0, 1, 2, 3, 4, 5, 6])),
            route(7, &trail, [path(&[0, 1, 2, 3, 4, 5]), vec![999]].concat()),
            // A lollipop: out on one street, around a block and back.
            route(8, &trail, path(&[60, 61, 62, 52, 51, 61, 60])),
            // The member order jumps, and the way from node 21 to node 22 is missing.
            route(9, &trail, [path(&[22, 23, 33, 43, 53]), path(&[20, 21])].concat()),
            long(5, &[1, 2]),
            long(6, &[5]),
        ],
        true,
    );
    let selection = Selection::whole(package.fork());
    let (records, report) = catalog(&selection, osm, false, 2)?;
    let by_id: HashMap<i64, &Value> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
    let dropped: Vec<_> = report.dropped.iter().map(|(reason, ids)| (reason.as_str(), ids.clone())).collect();
    assert_eq!(dropped, [("ExtractEdge", vec![7]), ("Gap", vec![3]), ("NestedLongRoute", vec![6])]);
    assert_eq!(by_id.keys().copied().collect::<BTreeSet<_>>(), BTreeSet::from([1, 2, 5, 8, 9]));

    let trail = by_id[&1];
    assert_eq!(trail["profile"], "hiking/alpine");
    assert_eq!((&trail["loop"], &trail["length_m"]), (&json!(false), &json!(8 * 445)));
    assert_eq!((&trail["grades_m"][2], &trail["hardest"]), (&json!(445), &json!(2)));
    assert_eq!((&trail["parent"], &trail["stage"]), (&json!(5), &json!(1)));
    assert_eq!((&trail["symbol"], &trail["website"]), (&json!("red"), &json!("https://t.example")));
    assert_eq!(trail["cells"], json!(["9-256-255"]));
    let via = trail["via"].as_array().unwrap().len();
    assert!(via > 0);
    // The figures are the route engine's totals for the plan, as a client routes it.
    let line: Vec<i64> = trail["line_udeg"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    let mut vertices = vec![[line[0], line[1]]];
    for pair in line[2..].chunks(2) {
        let last = vertices[vertices.len() - 1];
        vertices.push([last[0] + pair[0], last[1] + pair[1]]);
    }
    let plan: Vec<P> = std::iter::once(0)
        .chain(trail["via"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as usize))
        .chain([vertices.len() - 1])
        .map(|k| [vertices[k][0] as i32, vertices[k][1] as i32])
        .collect();
    let mut router = Router::new(Selection::whole(package.fork()), 768 << 20);
    let routed =
        router.route(&shape::request(trail["profile"].as_str().unwrap(), &plan, vec![]), &control()).unwrap().totals();
    assert!(routed.ascent_m > 0);
    assert_eq!(
        [&trail["length_m"], &trail["ascent_m"], &trail["descent_m"]],
        [&json!(routed.distance_m), &json!(routed.ascent_m), &json!(routed.descent_m)]
    );

    let ring = by_id[&2];
    assert_eq!((&ring["loop"], &ring["length_m"]), (&json!(true), &json!(8 * 445)));
    let line = ring["line_udeg"].as_array().unwrap();
    let end = |axis: usize| line.iter().skip(axis).step_by(2).map(|v| v.as_i64().unwrap()).sum::<i64>();
    assert_eq!([end(0), end(1)], [line[0].as_i64().unwrap(), line[1].as_i64().unwrap()]);

    let lollipop = by_id[&8];
    assert_eq!(lollipop["loop"], true);
    assert_eq!(lollipop["turnarounds"][0], 0, "the plan route turns back at its start");
    assert!(ring.get("turnarounds").is_none());
    let jumping = by_id[&9];
    assert_eq!(jumping["length_m"], 6 * 445);
    assert_eq!(&jumping["line_udeg"].as_array().unwrap()[..2], [json!(ORIGIN), json!(ORIGIN + 2 * 4_000)]);

    let whole = by_id[&5];
    assert_eq!(whole["profile"], "hiking/alpine");
    assert_eq!(ring["profile"], "hiking");
    assert_eq!((&whole["stages"], &whole["length_m"]), (&json!([1, 2]), &json!(16 * 445)));
    assert_eq!(whole["start_udeg"], json!([ORIGIN, ORIGIN]));
    assert!(whole.get("line_udeg").is_none());
    // The trail ends where the ring starts, so that point counts once.
    let points = via + 2 + ring["via"].as_array().unwrap().len() + 2 - 1;
    assert_eq!(report.long_points, [(5, points)]);
    Ok(())
}

#[test]
fn known_grades_require_their_explicit_profile_without_a_balanced_fallback() -> Result<(), String> {
    for available in [false, true] {
        let (package, osm) = grid(
            vec![
                route(1, &[("route", "hiking"), ("name", "Alpine")], path(&[0, 1, 2, 3, 4, 5, 6])),
                route(2, &[("route", "mtb"), ("name", "Technical")], path(&[10, 11, 12, 13, 14, 15, 16])),
                route(3, &[("route", "hiking"), ("name", "Ordinary")], path(&[20, 21, 22, 23, 24, 25, 26])),
            ],
            available,
        );
        let (records, report) = catalog(&Selection::whole(package), osm, false, 1)?;
        let by_id: HashMap<_, _> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
        assert_eq!(by_id[&3]["profile"], "hiking");
        if available {
            assert!(report.dropped.is_empty());
            assert_eq!(by_id[&1]["profile"], "hiking/alpine");
            assert_eq!(by_id[&2]["profile"], "mtb/technical");
            for id in [1, 2] {
                assert_eq!(by_id[&id]["hardest"], 2);
                assert_eq!(by_id[&id]["grades_m"][2], 445);
            }
        } else {
            assert_eq!(records.len(), 1);
            assert_eq!(report.dropped["MissingProfile"], [1, 2]);
        }
    }
    for (kind, key, grade) in [
        ("hiking", "sac_scale", "demanding_alpine_hiking"),
        ("foot", "sac_scale", "difficult_alpine_hiking"),
        ("mtb", "mtb:scale", "4+"),
        ("mtb", "sac_scale", "demanding_mountain_hiking"),
    ] {
        assert!(matches!(advanced(kind, &tags(&[(key, grade)])), Err(Reason::TooDifficult)));
    }
    Ok(())
}

#[test]
fn advanced_parent_shapes_ordinary_stages_against_its_shortcuts() -> Result<(), String> {
    let ordinary = [0, 10, 20, 21, 22, 23, 24, 25, 26, 16, 6];
    let alpine = [6, 5, 4, 3, 2, 1, 0];
    let trail = [("route", "hiking"), ("name", "Trail"), ("network", "rwn")];
    let parent = Relation {
        id: 3,
        tags: tags(&[("type", "superroute"), ("route", "hiking"), ("name", "Long"), ("network", "rwn")]),
        members: vec![(Id::Relation(1), String::new()), (Id::Relation(2), String::new())],
    };
    let mut wrong_family = parent.clone();
    wrong_family.id = 4;
    wrong_family.tags.insert("route".into(), "mtb".into());
    let (package, osm) = grid_with(
        vec![route(1, &trail, path(&ordinary)), route(2, &trail, path(&alpine)), parent, wrong_family],
        true,
        |graph| {
            let ways: HashSet<_> = path(&ordinary).into_iter().chain(path(&alpine)).collect();
            graph.roads.retain(|road| ways.contains(&road.way));
            graph.osm.ways.retain(|way, _| ways.contains(way));
            for road in &mut graph.roads {
                road.ascent_m = 0;
                road.descent_m = 0;
                for point in &mut road.shape {
                    point.elevation = 0.0;
                }
            }
        },
    );
    let selection = Selection::whole(package);
    let mut standalone = osm.clone();
    standalone.relations.remove(&3);
    standalone.relations.remove(&4);
    let (alone, _) = catalog(&selection, standalone, false, 1)?;
    assert!(alone.iter().find(|r| r["id"] == 1).unwrap()["via"].as_array().unwrap().is_empty());

    let (records, report) = catalog(&selection, osm, false, 1)?;
    assert_eq!(report.dropped, BTreeMap::from([("MissingStage".into(), vec![4])]));
    let by_id: HashMap<_, _> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
    assert_eq!(by_id[&1]["profile"], "hiking");
    assert!(!by_id[&1]["via"].as_array().unwrap().is_empty());
    assert_eq!(by_id[&3]["profile"], "hiking/alpine");
    let mut points = Vec::new();
    for id in [1, 2] {
        let record = by_id[&id];
        let mut previous = [0; 2];
        let line: Vec<P> = record["line_udeg"]
            .as_array()
            .unwrap()
            .chunks(2)
            .map(|delta| {
                previous[0] += delta[0].as_i64().unwrap() as i32;
                previous[1] += delta[1].as_i64().unwrap() as i32;
                previous
            })
            .collect();
        points.extend(
            std::iter::once(0)
                .chain(record["via"].as_array().unwrap().iter().map(|i| i.as_u64().unwrap() as usize))
                .chain([line.len() - 1])
                .map(|i| line[i]),
        );
    }
    points.dedup();
    let line: Vec<P> = ordinary
        .into_iter()
        .chain(alpine.into_iter().skip(1))
        .map(|id| [ORIGIN + (id % 10) as i32 * 4_000, ORIGIN + (id / 10) as i32 * 4_000])
        .collect();
    let mut router = Router::new(selection, 768 << 20);
    let routed = router.route(&shape::request("hiking/alpine", &points, vec![]), &control()).unwrap();
    assert!(shape::deviation(&shape::vertices(&routed), &line) <= shape::SHARE * shape::length(&line));
    Ok(())
}
