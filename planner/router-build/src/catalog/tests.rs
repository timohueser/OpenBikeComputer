use super::*;
use crate::{
    source::{Builder, NodeRef, Relations, Ways},
    Graph,
};
use planner_router::{
    model::{Profile, Road, Surface, BIKE, FOOT, PUSH},
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

fn tags(pairs: &[(&str, &str)]) -> Tags<'static> {
    Tags::from_pairs(pairs.iter().copied())
}

/// The way between grid nodes `a` and `b`.
fn way(a: i64, b: i64) -> i64 {
    1000 * a.min(b) + a.max(b)
}

/// Inside one zoom 9 cell.
const ORIGIN: i32 = 100_000;

/// A relation: ID, tags and members with their roles.
type Route<'a> = (i64, Vec<(&'a str, &'a str)>, Vec<(Id, &'a str)>);

/// A 7 by 7 grid of streets 445 m apart on rolling ground, with node `10 * row + column`, and its
/// source objects. The way from node 0 to node 1 is T3, the way from node 25 to node 26 is a T5 path;
/// there is no street from node 15 to node 25.
fn grid(routes: Vec<Route>) -> (Package<Memory>, Data) {
    grid_at([ORIGIN, ORIGIN], routes)
}

/// The grid with node 0 at `origin`, in microdegrees of longitude and latitude.
fn grid_at(origin: [i32; 2], routes: Vec<Route>) -> (Package<Memory>, Data) {
    let mut graph = Graph::default();
    let mut index = HashMap::new();
    let (mut ways, mut relations) = (Builder::<Ways>::default(), Builder::<Relations>::default());
    for row in 0..7 {
        for column in 0..7 {
            let id = 10 * row + column;
            let point = Point {
                lat: origin[1] + row as i32 * 4_000,
                lon: origin[0] + column as i32 * 4_000,
                elevation: ((row * 3 + column * 7) % 5) as f32 * 20.0,
            };
            index.insert(id, graph.points.len() as u32);
            graph.points.push(point);
            graph.node_ids.push(id);
            graph.node_access.push(BIKE | FOOT | PUSH);
        }
    }
    for row in 0..7 {
        for column in 0..7 {
            let a = 10 * row + column;
            for b in [a + 1, a + 10].into_iter().filter(|b| b % 10 < 7 && b / 10 < 7 && way(a, *b) != way(15, 25)) {
                let id = way(a, b);
                ways.push(id, &[a, b], &tags(&[("highway", "residential")]));
                for (from, to) in [(index[&a], index[&b]), (index[&b], index[&a])] {
                    let shape = vec![graph.points.values()[from as usize], graph.points.values()[to as usize]];
                    graph.roads.push(
                        Road {
                            from,
                            to,
                            way: id,
                            reversed: from > to,
                            length_m: shape[0].distance(shape[1]).round() as u32,
                            ascent_m: (shape[1].elevation - shape[0].elevation).max(0.0) as u32,
                            descent_m: (shape[0].elevation - shape[1].elevation).max(0.0) as u32,
                            surface: Surface::Paved,
                            class: if id == way(25, 26) { 4 } else { 1 },
                            access: BIKE | FOOT | PUSH,
                            difficulty: 255,
                            hiking_difficulty: match id {
                                _ if id == way(0, 1) => Some(3),
                                _ if id == way(25, 26) => Some(5),
                                _ => None,
                            },
                            structure: false,
                            shape,
                        },
                        Default::default(),
                    );
                }
            }
        }
    }
    for (id, pairs, members) in routes {
        relations.push(id, &tags(&pairs), members);
    }
    let mut ways = ways.finish().0;
    let point = |id: i64| graph.points.values()[index[&id] as usize];
    let node =
        |id| NodeRef { lat: point(id).lat, lon: point(id).lon, flags: NodeRef::PRESENT, ..NodeRef::missing(false) };
    ways.set_points(ways.references().iter().map(|&id| node(id)).collect());
    graph.osm = Data { ways, relations: relations.finish().0, ..Data::default() };
    graph.link_ways();
    let profiles: Vec<_> = Profile::presets().into_iter().filter(|p| PROFILES.contains(&p.name.as_str())).collect();
    let mut objects = HashMap::new();
    let [lon, lat] = origin.map(|udeg| udeg as f64 * 1e-6);
    let manifest = crate::prepare(
        &graph,
        "test".into(),
        [lon - 1.0, lat - 1.0, lon + 1.0, lat + 1.0],
        &profiles,
        vec![],
        "test".into(),
        |bytes| {
            objects.insert(digest(bytes), bytes.to_vec());
            Ok(digest(bytes))
        },
    )
    .unwrap();
    (Package::open(Memory(Arc::new(objects)), &serde_json::to_vec(&manifest).unwrap()).unwrap(), graph.osm)
}

/// A package of `ways` on flat ground: each a highway value, a road class and its points in metres
/// east and north of grid node 0. Ways meet where they share a point.
fn streets(ways: &[(&str, u8, &[[f64; 2]])]) -> Package<Memory> {
    let mut graph = Graph::default();
    let mut nodes = HashMap::new();
    let mut builder = Builder::<Ways>::default();
    for (id, &(highway, class, points)) in (1..).zip(ways) {
        let ids: Vec<i64> = points
            .iter()
            .map(|&[east, north]| {
                let [lon, lat] = off(0, east, north);
                *nodes.entry([lon, lat]).or_insert_with(|| {
                    graph.points.push(Point { lat, lon, elevation: 0.0 });
                    graph.node_ids.push(graph.node_ids.len() as i64);
                    graph.node_access.push(BIKE | FOOT | PUSH);
                    graph.node_ids.len() as i64 - 1
                })
            })
            .collect();
        builder.push(id, &ids, &tags(&[("highway", highway)]));
        for pair in ids.windows(2) {
            for (from, to) in [(pair[0], pair[1]), (pair[1], pair[0])] {
                let shape = vec![graph.points.values()[from as usize], graph.points.values()[to as usize]];
                let road = Road {
                    from: from as u32,
                    to: to as u32,
                    way: id,
                    reversed: from > to,
                    length_m: shape[0].distance(shape[1]).round() as u32,
                    surface: Surface::Paved,
                    class,
                    access: BIKE | FOOT | PUSH,
                    difficulty: 255,
                    shape,
                    ..Road::default()
                };
                graph.roads.push(road, Default::default());
            }
        }
    }
    let mut ways = builder.finish().0;
    let point = |id: i64| graph.points.values()[id as usize];
    let node =
        |id| NodeRef { lat: point(id).lat, lon: point(id).lon, flags: NodeRef::PRESENT, ..NodeRef::missing(false) };
    ways.set_points(ways.references().iter().map(|&id| node(id)).collect());
    graph.osm = Data { ways, ..Data::default() };
    graph.link_ways();
    let profiles: Vec<_> = Profile::presets().into_iter().filter(|p| PROFILES.contains(&p.name.as_str())).collect();
    let mut objects = HashMap::new();
    let [lon, lat] = [ORIGIN as f64 * 1e-6; 2];
    let manifest = crate::prepare(
        &graph,
        "test".into(),
        [lon - 1.0, lat - 1.0, lon + 1.0, lat + 1.0],
        &profiles,
        vec![],
        "test".into(),
        |bytes| {
            objects.insert(digest(bytes), bytes.to_vec());
            Ok(digest(bytes))
        },
    )
    .unwrap();
    Package::open(Memory(Arc::new(objects)), &serde_json::to_vec(&manifest).unwrap()).unwrap()
}

/// The hiking plan of `line` in `package`, and the deviation of its route.
fn hiking_plan(package: &Package<Memory>, line: &[P], closed: bool) -> Result<f64, Failure> {
    let mut router = Router::new(Selection::whole(package.fork()), 768 << 20);
    let mut limits = Limits { control: control(), max_calls: SEARCH_CALLS, calls: 0 };
    let plan = shape::shape(&mut router, &["hiking/alpine"], line, &[], shape::length(line), closed, &mut limits)?;
    Ok(shape::deviation(&shape::vertices(&plan.route), line))
}

fn route<'a>(id: i64, pairs: &[(&'a str, &'a str)], ways: impl IntoIterator<Item = i64>) -> Route<'a> {
    (id, [&[("type", "route")], pairs].concat(), ways.into_iter().map(|way| (Id::Way(way), "")).collect())
}

/// Relation members with the empty role.
fn members(ids: &[i64], member: fn(i64) -> Id) -> Vec<(Id, &'static str)> {
    ids.iter().map(|&id| (member(id), "")).collect()
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
    let long = |id, children: &[i64]| -> Route {
        let tags = vec![("type", "superroute"), ("route", "hiking"), ("name", "Long"), ("network", "rwn")];
        (id, tags, members(children, Id::Relation))
    };
    let mut corner = path(&[0, 1, 2, 3, 4, 14, 24, 34, 44]);
    corner.remove(5);
    let (package, osm) = grid(vec![
        // East, then north: the router needs a shaping point near the corner. The way from node
        // 14 to node 24 is missing, a gap of 445 m.
        route(1, &trail, corner),
        // A ring around four blocks.
        route(2, &[("route", "foot"), ("ref", "R"), ("network", "rwn")], path(&[44, 45, 46, 56, 66, 65, 64, 54, 44])),
        // Two parts 1.3 km apart.
        route(3, &trail, [path(&[0, 10, 20, 30]), path(&[5, 6, 16])].concat()),
        route(4, &[("route", "mtb"), ("name", "Planned"), ("state", "proposed")], path(&[0, 1, 2, 3, 4, 5, 6])),
        // A way that the import does not hold: at the end it can be the edge of the region; between
        // held ways the line joins across two of them.
        route(7, &trail, [path(&[0, 1, 2, 3, 4, 5]), vec![999]].concat()),
        route(19, &trail, [path(&[50, 51, 52]), vec![990, 991], path(&[53, 54, 55])].concat()),
        // The patch from node 15 to node 25 goes around a block: three times the gap. It leaves the
        // route for more than 500 m, unless the route itself comes back along the block.
        route(20, &trail, [path(&[12, 13, 14, 15]), path(&[25, 35, 45])].concat()),
        route(21, &trail, [path(&[12, 13, 14, 15]), path(&[25, 24, 34, 44])].concat()),
        // A lollipop: out on one street, around a block and back.
        route(8, &trail, path(&[60, 61, 62, 52, 51, 61, 60])),
        // The member order jumps, and the way from node 21 to node 22 is missing.
        route(9, &trail, [path(&[22, 23, 33, 43, 53]), path(&[20, 21])].concat()),
        // A long relation inside another one with a lower relation ID.
        long(22, &[1, 2]),
        long(6, &[22]),
        // A signed piece between two signposts, which is no part of a long route either, a named walk
        // of 1.3 km, and an unnamed stage with its own mark that starts where the walk ends.
        route(
            10,
            &[
                ("route", "hiking"),
                ("name", "Rotkreuz - Auleten"),
                ("network", "rwn"),
                ("osmc:symbol", "yellow::yellow_diamond"),
            ],
            path(&[40, 41, 42]),
        ),
        route(11, &[("route", "hiking"), ("name", "Rundweg")], path(&[30, 31, 32, 33])),
        long(12, &[10, 11, 35]),
        route(35, &[("route", "hiking"), ("osmc:symbol", "red")], path(&[33, 34, 35, 36])),
        // A trail of an unnamed piece, its own ways and a connector leg, and a long route with it as
        // its stage; a trail with an absent child; a connector made of a piece.
        (
            13,
            vec![("type", "route"), ("route", "hiking"), ("name", "Joined"), ("network", "rwn")],
            [members(&[14], Id::Relation), members(&path(&[16, 26, 36]), Id::Way), members(&[15], Id::Relation)]
                .concat(),
        ),
        route(14, &[("route", "hiking")], path(&[6, 16])),
        route(15, &[("route", "hiking"), ("name", "C - D")], path(&[36, 46])),
        long(18, &[13]),
        (16, vec![("type", "route"), ("route", "hiking"), ("name", "Half")], members(&[14, 998], Id::Relation)),
        (
            17,
            [&trail[..1], &[("type", "route"), ("name", "A - B"), ("osmc:symbol", "yellow::yellow_diamond")]].concat(),
            members(&[15], Id::Relation),
        ),
        // A trail: a first stage and a stage with the role `part`, both drawn against the trail, a
        // stage that fails with a main line (two pieces 890 m apart), an absent stage, a stage after
        // it, and a stage 1.4 km away from the end of that one.
        (
            30,
            vec![("type", "superroute"), ("route", "hiking"), ("name", "Höhenweg"), ("network", "nwn")],
            [members(&[31], Id::Relation), vec![(Id::Relation(32), "part")], members(&[33, 997, 34, 36], Id::Relation)]
                .concat(),
        ),
        route(31, &[("route", "hiking"), ("name", "Etappe 1")], path(&[3, 2, 1, 0])),
        route(32, &[("route", "hiking"), ("name", "Etappe 2")], path(&[6, 5, 4, 3])),
        route(33, &[("route", "hiking"), ("name", "Etappe 3")], [path(&[6, 16, 26]), path(&[46, 56])].concat()),
        route(34, &[("route", "hiking"), ("name", "Etappe 5")], path(&[66, 65, 64, 63])),
        route(36, &[("route", "hiking"), ("name", "Etappe 6")], path(&[20, 30, 40, 50])),
    ]);
    let selection = Selection::whole(package.fork());
    let (records, report) = catalog(&selection, osm, Sources::default(), 2, SECTION_M)?;
    let by_id: HashMap<i64, &Value> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
    let dropped: Vec<_> = report.dropped.iter().map(|(reason, ids)| (reason.as_str(), ids.clone())).collect();
    assert_eq!(
        dropped,
        [("Connector", vec![10, 15, 17]), ("ExtractEdge", vec![7]), ("Gap", vec![3, 20, 33]), ("NoStages", vec![16])]
    );
    assert_eq!(
        by_id.keys().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from([1, 2, 6, 8, 9, 11, 12, 13, 18, 19, 21, 22, 30, 31, 32, 34, 35, 36])
    );
    assert_eq!((&by_id[&19]["length_m"], &report.skipped_ways), (&json!(5 * 445), &BTreeMap::from([(19, 2)])));
    let joined = by_id[&13];
    assert_eq!(joined["length_m"], 4 * 445);
    assert_eq!(&joined["line_udeg"].as_array().unwrap()[..2], [json!(ORIGIN + 6 * 4_000), json!(ORIGIN)]);
    assert!(joined.get("stages").is_none());
    assert_eq!((&joined["parent"], &joined["stage"]), (&json!(18), &json!(1)));
    for record in &records {
        let Some(parts) = record.get("parts").and_then(Value::as_array) else {
            assert!(record["moving_s"].as_u64().is_some_and(|s| s > 0));
            continue;
        };
        let kept: Vec<&Value> = parts.iter().filter(|part| part.get("missing").is_none()).collect();
        let sum = kept.iter().map(|part| part["moving_s"].as_u64().unwrap()).sum();
        assert_eq!(record["moving_s"].as_u64(), Some(sum));
        for part in kept {
            let own = by_id[&part["id"].as_i64().unwrap()];
            assert_eq!((&part["moving_s"], part.get("hardest")), (&own["moving_s"], own.get("hardest")));
        }
    }

    let trail = by_id[&1];
    assert_eq!(trail["loop"], json!(false));
    // A shaping point can split a street, and each leg rounds its length.
    assert!(trail["length_m"].as_u64().is_some_and(|m| m.abs_diff(8 * 445) <= 1), "{trail}");
    assert_eq!((&trail["grades_m"][2], &trail["hardest"]), (&json!(445), &json!(2)));
    assert_eq!((&trail["parent"], &trail["stage"]), (&json!(22), &json!(1)), "the innermost complete trail");
    assert_eq!((&trail["symbol"], &trail["website"]), (&json!("red"), &json!("https://t.example")));
    assert_eq!(trail["cells"], json!(["9-256-255"]));
    let via = trail["via"].as_array().unwrap().len();
    assert!(via > 0);
    // The figures are the route engine's totals for the plan, as a client routes it. The T3 member
    // makes the client choose the alpine preset.
    assert_eq!(trail["hardest"], json!(2));
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
    let routed = router.route(&shape::request("hiking/alpine", &plan, vec![]), &control()).unwrap();
    let profile = trail["profile_m"].as_array().unwrap();
    let height = |p: &Point| json!(p.elevation.round() as i32);
    assert_eq!(
        [&profile[0], &profile[profile.len() - 1]],
        [&height(&routed.points[0]), &height(routed.points.last().unwrap())]
    );
    let routed = routed.totals();
    assert!(routed.ascent_m > 0);
    assert_eq!(
        [&trail["length_m"], &trail["ascent_m"], &trail["descent_m"], &trail["moving_s"]],
        [
            &json!(routed.distance_m),
            &json!(routed.ascent_m),
            &json!(routed.descent_m),
            &json!(routed.seconds.round() as u64)
        ]
    );

    for id in [1, 2, 8, 9] {
        let length = by_id[&id]["length_m"].as_u64().unwrap() as usize;
        assert_eq!(by_id[&id]["profile_m"].as_array().unwrap().len(), length / 100 + 2);
    }

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

    let whole = by_id[&22];
    assert_eq!(whole["stages"], json!([1, 2]));
    assert_eq!(whole["length_m"], by_id[&1]["length_m"].as_u64().unwrap() + 8 * 445);
    assert_eq!(whole["start_udeg"], json!([ORIGIN, ORIGIN]));
    assert!(whole.get("line_udeg").is_none() && whole.get("profile_m").is_none());
    // A long relation inside another opens in its place, and its stages keep the inner one.
    assert_eq!((&by_id[&6]["stages"], &by_id[&1]["trail"]), (&json!([1, 2]), &json!(22)));
    // A connector leg is no part, and its trail is not complete; an unnamed stage takes the name of
    // its trail.
    assert!(by_id[&12].get("stages").is_none() && by_id[&35].get("parent").is_none());
    assert_eq!(
        (&by_id[&35]["name"], &by_id[&35]["trail"], &by_id[&35]["trail_stage"]),
        (&json!("Long"), &json!(12), &json!(2))
    );

    let trail = by_id[&30];
    assert!(trail.get("stages").is_none() && trail.get("start_udeg").is_none());
    let parts = trail["parts"].as_array().unwrap();
    let flags: Vec<Vec<&str>> = parts
        .iter()
        .map(|p| ["missing", "backwards", "jump"].into_iter().filter(|f| p.get(*f).is_some()).collect())
        .collect();
    assert_eq!(
        flags,
        [vec!["backwards"], vec!["backwards"], vec!["missing"], vec!["missing"], vec!["jump"], vec!["jump"]]
    );
    assert_eq!((&parts[2]["start_udeg"], &parts[3].get("length_m")), (&json!([ORIGIN + 6 * 4_000, ORIGIN]), &None));
    for (id, number) in [(31, 1), (32, 2), (34, 5), (36, 6)] {
        assert_eq!((&by_id[&id]["trail"], &by_id[&id]["trail_stage"]), (&json!(30), &json!(number)));
        assert!(by_id[&id].get("parent").is_none());
    }
    let heights = trail["trail_profile_m"].as_array().unwrap();
    assert_eq!(heights.len(), 201);
    assert_eq!(heights[0], *by_id[&31]["profile_m"].as_array().unwrap().last().unwrap(), "the trail starts at its end");
    assert_eq!(heights[80], by_id[&32]["profile_m"][0], "the stage against the trail ends at its own start");
    assert!(heights[81..120].iter().all(Value::is_null), "the failed stage has no heights");
    assert!(!heights[121].is_null() && !heights[200].is_null());
    // The trail ends where the ring starts, so that point counts once.
    let points = via + 2 + ring["via"].as_array().unwrap().len() + 2 - 1;
    assert!(report.long_points.contains(&(22, points)));
    Ok(())
}

/// The start of a `line_udeg` and its last vertex.
fn line_ends(record: &Value) -> [[i64; 2]; 2] {
    let line: Vec<i64> = record["line_udeg"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    let sum = |axis: usize| line.iter().skip(axis).step_by(2).sum::<i64>();
    [[line[0], line[1]], [sum(0), sum(1)]]
}

#[test]
fn catalog_cuts_a_long_line_whose_plan_fails_into_sections() -> Result<(), String> {
    let bike = |name| vec![("route", "bicycle"), ("name", name), ("network", "rcn")];
    let (package, osm) = grid(vec![
        // 4 km whose end takes the T5 path, which no bike profile follows: the plan fails.
        route(40, &bike("Long line"), path(&[10, 11, 12, 13, 14, 15, 16, 26, 25, 24])),
        (41, [vec![("type", "superroute")], bike("Trail")].concat(), members(&[42, 40], Id::Relation)),
        route(42, &bike("Etappe 1"), path(&[50, 40, 30, 20, 10])),
        route(44, &bike("Plain"), path(&[60, 61, 62, 63, 64, 65, 66])),
        // 3.6 km with the T3 way in its first half and the T5 path in its second: no section passes.
        route(45, &bike("Two faults"), path(&[0, 1, 2, 3, 4, 14, 24, 25, 26])),
        (46, [vec![("type", "superroute")], bike("Faults")].concat(), members(&[44, 45], Id::Relation)),
    ]);
    // The test grid is 2.7 km wide.
    let section_m = 2_000.0;
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, Sources::default(), 2, section_m)?;
    let by_id: HashMap<i64, &Value> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
    let dropped: BTreeSet<i64> = report.dropped.values().flatten().copied().collect();
    assert_eq!(dropped, BTreeSet::from([-4003, 45]), "{:?}", report.dropped);
    // A line longer than a section whose plan passes stays one route.
    assert!(by_id[&44].get("section").is_none() && by_id[&44]["length_m"].as_u64() > Some(section_m as u64));
    // A line with no section that passes leaves as a whole, and its trail lists it as one missing part.
    let part = &by_id[&46]["parts"][1];
    assert_eq!((&part["id"], &part["missing"], part.get("cut")), (&json!(45), &json!(true), None));
    assert!(part["length_m"].as_u64() > Some(section_m as u64), "the length of the whole main line");

    // Three sections of 1.3 km, cut at the grid nodes nearest to each third.
    let (first, second) = (by_id[&-4001], by_id[&-4002]);
    assert_eq!((&first["section"], &first["trail"], &first["name"]), (&json!(1), &json!(40), &json!("Long line")));
    assert!(first.get("trail_stage").is_none() && first.get("parent").is_none());
    assert_eq!(line_ends(first)[0], [ORIGIN as i64, ORIGIN as i64 + 4_000]);
    assert_eq!(line_ends(first)[1], line_ends(second)[0], "the sections meet");
    assert_eq!([&first["length_m"], &second["length_m"]], [&json!(3 * 445), &json!(3 * 445)]);

    let line = by_id[&40];
    assert!(line.get("line_udeg").is_none() && line.get("stages").is_none());
    let parts = line["parts"].as_array().unwrap();
    let ids: Vec<_> = parts.iter().map(|p| (&p["id"], p.get("stage"), p.get("cut"), p.get("missing"))).collect();
    let yes = Some(&json!(true));
    assert_eq!(
        ids,
        [(&json!(-4001), None, yes, None), (&json!(-4002), None, yes, None), (&json!(-4003), None, None, yes)]
    );
    assert_eq!((&line["stage_count"], &line["trail"], &line["trail_stage"]), (&json!(0), &json!(41), &json!(2)));
    // In the trail, the sections of its cut stage take the place of the stage.
    let parts = by_id[&41]["parts"].as_array().unwrap();
    let stages: Vec<_> = parts.iter().map(|p| (p["id"].as_i64().unwrap(), &p["stage"])).collect();
    assert_eq!(stages, [(42, &json!(1)), (-4001, &json!(2)), (-4002, &json!(2)), (-4003, &json!(2))]);
    assert!(by_id[&41].get("stages").is_none() && by_id[&42].get("parent").is_none());
    assert_eq!((&by_id[&41]["stage_count"], report.cut_long_routes), (&json!(2), 2));
    Ok(())
}

#[test]
fn catalog_keeps_only_long_relations_whose_parts_make_one_line() -> Result<(), String> {
    let hiking = |name| vec![("route", "hiking"), ("name", name)];
    let long = |id, children: &[i64]| -> Route {
        (id, [vec![("type", "superroute")], hiking("Long")].concat(), members(children, Id::Relation))
    };
    let (package, osm) = grid(vec![
        // Two loops from one village: a set.
        route(50, &hiking("Dorf"), path(&[0, 1, 11, 10, 0])),
        route(51, &hiking("Badi"), path(&[0, 10, 20, 21, 11, 1, 0])),
        long(52, &[50, 51]),
        // Two stages that meet, and an absent stage.
        route(54, &hiking("Etappe 1"), path(&[60, 61, 62, 63])),
        route(55, &hiking("Etappe 2"), path(&[63, 64, 65, 66])),
        long(53, &[54, 55, 996]),
        // One jump is a hole in the data.
        route(56, &hiking("Elsewhere"), path(&[20, 30, 40, 50])),
        long(57, &[54, 56]),
        // A pair that touches, the two loops and lone lines are a set, and its unnamed member is no route.
        route(60, &hiking("Hin"), path(&[0, 1, 2, 3])),
        route(62, &[("route", "hiking")], path(&[6, 16, 26, 36])),
        route(65, &hiking("Aussicht"), path(&[40, 41, 42, 43])),
        long(58, &[54, 56, 50, 60, 55, 62, 65, 51]),
        // Four stages of one line, listed so that no two neighbours join.
        route(61, &hiking("Weiter"), path(&[3, 4, 5, 6])),
        route(63, &hiking("Hinauf"), path(&[6, 16, 26, 36])),
        route(64, &hiking("Hinunter"), path(&[36, 46, 56, 66])),
        long(59, &[61, 64, 60, 63]),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, Sources::default(), 2, SECTION_M)?;
    let by_id: HashMap<i64, &Value> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
    assert_eq!(report.dropped.get("Apart"), Some(&vec![52, 58]), "{:?}", report.dropped);
    assert!([50, 51].iter().all(|id| by_id[id].get("trail").is_none()));
    assert!(!by_id.contains_key(&62));
    assert_eq!(by_id[&53]["parts"][2]["missing"], true);
    assert!(by_id.contains_key(&57) && by_id.contains_key(&59));
    assert_eq!((&by_id[&56]["trail"], &by_id[&55]["trail"]), (&json!(57), &json!(53)));
    assert_eq!(by_id[&64]["trail"], json!(59));
    Ok(())
}

#[test]
fn catalog_leaves_out_the_mark_of_a_route_in_france() -> Result<(), String> {
    let marked = |name| vec![("route", "hiking"), ("name", name), ("osmc:symbol", "red:white:red_bar")];
    let long = |id, children: &[i64]| -> Route {
        (id, [vec![("type", "superroute")], marked("Long")].concat(), members(children, Id::Relation))
    };
    // Near Geneva: from node 2 on, the south row is in France; the two north rows are in Switzerland.
    let (package, osm) = grid_at(
        [6_180_000, 46_178_000],
        vec![
            route(1, &marked("Into France"), path(&[0, 1, 2, 3, 4, 5, 6])),
            route(2, &marked("Swiss"), path(&[60, 61, 62, 63, 64, 65, 66])),
            route(3, &[("route", "hiking"), ("name", "Plain")], path(&[50, 51, 52, 53, 54, 55, 56])),
            long(4, &[1, 2]),
            long(5, &[2, 3]),
        ],
    );
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, Sources::default(), 2, SECTION_M)?;
    assert!(report.dropped.is_empty(), "{:?}", report.dropped);
    let symbols: Vec<_> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r.get("symbol").cloned())).collect();
    let mark = Some(json!("red:white:red_bar"));
    assert_eq!(symbols, [(1, None), (2, mark.clone()), (3, None), (4, None), (5, mark)]);
    Ok(())
}

#[test]
fn catalog_keeps_a_hiking_route_over_a_t5_way() -> Result<(), String> {
    let (package, osm) =
        grid(vec![route(13, &[("route", "hiking"), ("name", "Ridge")], path(&[23, 24, 25, 26, 36, 46]))]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, Sources::default(), 2, SECTION_M)?;
    assert!(report.dropped.is_empty(), "{:?}", report.dropped);
    assert_eq!((&records[0]["id"], &records[0]["hardest"]), (&json!(13), &json!(4)));
    Ok(())
}

#[test]
fn profile_gives_the_height_at_each_100_m_and_at_the_end() {
    // Points at the given metres along a meridian, with their heights.
    let line = |points: &[(f64, Option<f32>)]| -> Vec<Point> {
        let at = |(m, h): &(f64, Option<f32>)| Point {
            lat: (m / planner_router::geometry::METRES_PER_UDEG).round() as i32,
            lon: 0,
            elevation: h.unwrap_or(NO_ELEVATION),
        };
        points.iter().map(at).collect()
    };
    let entries = Some(vec![Some(100), Some(200), Some(300), Some(350)]);
    assert_eq!(profile(&line(&[(0.0, Some(100.0)), (250.0, Some(350.0))]), 250.0), entries);
    let short = line(&[(0.0, Some(100.0)), (240.0, Some(350.0))]);
    assert_eq!(profile(&short, 250.0), entries, "the walked distance is scaled to the length");
    assert_eq!(profile(&line(&[(0.0, Some(0.0)), (300.0, Some(300.0))]), 300.0).unwrap().len(), 5);

    let gap = line(&[(0.0, Some(0.0)), (150.0, Some(15.0)), (250.0, None), (400.0, Some(40.0))]);
    assert_eq!(profile(&gap, 400.0), Some(vec![Some(0), Some(10), None, None, Some(40), Some(40)]));
    // The ends fall on the end points, so they keep their known heights beside an unknown point.
    let ends = line(&[(0.0, Some(5.0)), (100.0, None), (250.0, Some(20.0))]);
    assert_eq!(profile(&ends, 250.0), Some(vec![Some(5), None, None, Some(20)]));
    assert_eq!(profile(&line(&[(0.0, None), (400.0, None)]), 400.0), None);
}

#[test]
fn trails_are_long_records_of_two_stages_or_records_of_more_than_12_hours() {
    let at = |lat: i32| [0, lat];
    let route = |id: i64, trail: Option<i64>, line: &[P]| {
        let mut record = json!({"id": id, "kind": "hiking", "name": "R", "rank": 2, "loop": false, "length_m": 1});
        record["line_udeg"] = json!(encode(line));
        if let Some(trail) = trail {
            record["trail"] = json!(trail);
        }
        record
    };
    let long = |id: i64, stage_count: usize, parts: Value| json!({"id": id, "kind": "hiking", "stage_count": stage_count, "parts": parts});
    let mut records = vec![
        route(1, None, &[at(0), at(1_000)]),
        route(2, None, &[at(0), at(1_000)]),
        // Part 32 runs backwards from the end of part 31; part 35 starts 445 m from the end of part 34.
        long(
            3,
            3,
            json!([{"id": 31}, {"id": 32, "backwards": true}, {"id": 33, "missing": true}, {"id": 34}, {"id": 35}]),
        ),
        route(31, Some(3), &[at(0), at(1_000)]),
        route(32, Some(3), &[at(2_000), at(1_000)]),
        route(34, Some(3), &[at(5_000), at(6_000)]),
        route(35, Some(3), &[at(10_000), at(11_000)]),
        // A cut line of more than 12 hours, and a cut stage of more than 12 hours.
        long(5, 0, json!([{"id": -501, "cut": true}, {"id": -502}])),
        long(6, 0, json!([{"id": -601}])),
        // A long record inside trail 3.
        long(7, 2, json!([{"id": 31}, {"id": 32, "backwards": true}])),
    ];
    for (id, trail) in [(-501, 5), (-502, 5), (-601, 6)] {
        records.push(route(id, Some(trail), &[at(0), at(1_000)]));
    }
    records[8]["trail"] = json!(3);
    // The moving time in hours; a long record holds the sum over its parts.
    let hours = HashMap::from([(1, 13), (2, 11), (5, 13), (6, 13), (7, 2)]);
    for record in &mut records {
        if let Some(h) = hours.get(&record["id"].as_i64().unwrap()) {
            record["moving_s"] = json!(h * 3600);
        }
    }
    let trails = trails::trails(&records);
    let ids: Vec<i64> = trails.iter().map(|(entry, _)| entry["id"].as_i64().unwrap()).collect();
    assert_eq!(ids, [1, 3, 5, 7]);
    let (entry, record) = &trails[1];
    assert_eq!(record, &records[2], "the record file holds the whole record with its parts");
    let pieces: Vec<Vec<P>> = entry["far_udeg"].as_array().unwrap().iter().map(decode).collect();
    assert_eq!(pieces, [vec![at(0), at(1_000), at(2_000)], vec![at(5_000), at(6_000)], vec![at(10_000), at(11_000)]]);
    assert_eq!((&entry["stage_count"], &entry["rank"]), (&json!(3), &Value::Null));
    assert!(trails[0].0.get("stage_count").is_none() && trails[2].0.get("stage_count").is_none());
    assert_eq!(trails[0].0["far_udeg"], json!([records[0]["line_udeg"]]));
}

/// The point `east_m` east and `north_m` north of grid node `node`.
fn off(node: i64, east_m: f64, north_m: f64) -> P {
    let udeg = |metres: f64| (metres * 4_000.0 / 445.0).round() as i32;
    [ORIGIN + (node % 10) as i32 * 4_000 + udeg(east_m), ORIGIN + (node / 10) as i32 * 4_000 + udeg(north_m)]
}

fn at(nodes: &[i64]) -> Vec<P> {
    nodes.iter().map(|&node| off(node, 0.0, 0.0)).collect()
}

/// A route of a second source with its members, as `route-sources.json` holds it.
fn official(key: u64, kind: &str, rank: u8, reference: &str, name: &str, members: &[Vec<P>]) -> Value {
    let flat = |line: &Vec<P>| line.iter().flat_map(|p| [p[0], p[1]]).collect::<Vec<_>>();
    json!({"key": key, "kind": kind, "rank": rank, "ref": reference, "name": name, "members": members.iter().map(flat).collect::<Vec<_>>()})
}

/// A route of a second source with stages, each a stage number and its members.
fn staged(route: Value, stages: &[(u64, &[Vec<P>])]) -> Value {
    let mut route = route;
    let members = route.as_object_mut().unwrap().remove("members");
    assert!(members.is_some());
    let stage = |&(number, members): &(u64, &[Vec<P>])| {
        let part = official(0, "", 0, "", &format!("Stage {number}"), members);
        json!({"stage": number, "name": part["name"], "members": part["members"]})
    };
    route["stages"] = json!(stages.iter().map(stage).collect::<Vec<_>>());
    route
}

/// The second sources with origins 1, 2, …, each with its routes.
fn sources_of(routes: Vec<Vec<Value>>) -> Sources {
    let sources = routes.into_iter().enumerate().map(|(i, routes)| {
        json!({"id": format!("source-{}", i + 1), "credit": format!("Credit {}", i + 1), "origin": i + 1, "routes": routes})
    });
    sources::jobs(&json!({"format": 1, "sources": sources.collect::<Vec<_>>()})).unwrap()
}

/// The `id` of a record of a second source.
fn source_id(origin: i64, key: i64, stage: i64) -> i64 {
    -(origin * sources::ORIGIN_ID + key * 100 + stage)
}

fn by_id(records: &[Value]) -> HashMap<i64, &Value> {
    records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect()
}

#[test]
fn route_sources_give_a_job_for_each_route_and_each_stage_with_members() {
    let spec = include_str!("../../../../specs/obc-data.md");
    let example = spec.split("##### Route sources").nth(1).unwrap().split("```json").nth(1).unwrap();
    let mut file: Value = serde_json::from_str(example.split("```").next().unwrap()).unwrap();
    let Sources { ids, jobs, trails } = sources::jobs(&file).unwrap();
    assert_eq!(ids, ["schweizmobil-hiking"]);
    // Stage 1 has no members.
    let fields: Vec<_> =
        jobs.iter().map(|j| (j.id(), j.kind.as_str(), j.rank, j.ref_.as_str(), j.name.as_str())).collect();
    assert_eq!(
        fields,
        [
            (-1_000_000_000_311, "hiking", 3, "3", "Alpenpanorama-Weg, Luzern – Malters"),
            (-1_000_000_056_100, "hiking", 1, "561", "Rigi Panoramaweg"),
        ]
    );
    assert_eq!(
        (jobs[0].route.as_str(), jobs[0].credit.as_str()),
        ("Alpenpanorama-Weg", "Bundesamt für Strassen, Kanton, Stiftung SchweizMobil")
    );
    assert_eq!(jobs[1].members, [vec![[8461200, 47052310], [8461350, 47052400]]]);
    // A route with stages keeps each stage row, also one with no members.
    let rows: Vec<_> =
        trails.iter().map(|t| (t.route.id(), t.stages.iter().map(|s| s.0).collect::<Vec<_>>())).collect();
    assert_eq!(rows, [(-1_000_000_000_300, vec![1, 11])]);
    file["format"] = json!(2);
    assert!(sources::jobs(&file).is_err());
}

#[test]
fn the_cut_takes_out_each_stretch_away_from_the_roads() {
    use planner_router::geometry::METRES_PER_UDEG;
    let m = |x: f64, y: f64| [(x / METRES_PER_UDEG).round() as i32, (y / METRES_PER_UDEG).round() as i32];
    let line = |points: &[(f64, f64)]| -> Vec<P> { points.iter().map(|&(x, y)| m(x, y)).collect() };
    // Roads along y = 0, y = 330 and y = −330, and a square from (0, 0) to (1000, 1000).
    let roads = [
        line(&[(-3000.0, 0.0), (3000.0, 0.0)]),
        line(&[(-3000.0, 330.0), (3000.0, 330.0)]),
        line(&[(-3000.0, -330.0), (3000.0, -330.0)]),
        line(&[(1000.0, 0.0), (1000.0, 1000.0), (0.0, 1000.0), (0.0, 0.0)]),
    ];
    let nears: Vec<shape::Near> = roads.iter().map(|road| shape::Near::new(road)).collect();
    let near = |p: P| nears.iter().any(|road| road.near(p));
    let cut = |points: &[(f64, f64)], closed: bool| sources::cut(&[line(points)], closed, &near);
    // A bump of 60 m below the road: a cut of 200 m along the road.
    let bump = |depth: f64| {
        cut(&[(-500.0, 0.0), (0.0, 0.0), (0.0, -depth), (200.0, -depth), (200.0, 0.0), (900.0, 0.0)], false)
    };
    let (runs, cuts) = bump(60.0).unwrap();
    assert_eq!(runs.len(), 2);
    assert!(cuts.len() == 1 && (250..300).contains(&cuts[0]), "{cuts:?}");
    assert!((distance(runs[0][runs[0].len() - 1], runs[1][0]) - 200.0).abs() < 1.0);
    assert!(matches!(bump(230.0), Err(Reason::Gap)), "a cut of 600 m");
    // A start 300 m below the road shortens the line; 600 m below, it drops it.
    let (runs, _) = cut(&[(0.0, -300.0), (0.0, 0.0), (900.0, 0.0)], false).unwrap();
    assert_eq!(runs.len(), 1);
    assert!(distance(runs[0][0], m(0.0, 0.0)) <= 30.0, "the line starts at the first point near the road");
    assert!(matches!(cut(&[(0.0, -600.0), (0.0, 0.0), (900.0, 0.0)], false), Err(Reason::Gap)));
    // From y = 330 to y = −330: two stretches of 270 m and a road crossing of 60 m are one cut; along
    // the road for 100 m more, they are two.
    let crossing = |along: f64| {
        let x = -1000.0 + along;
        cut(&[(-2000.0, 330.0), (-1000.0, 330.0), (-1000.0, 0.0), (x, 0.0), (x, -330.0), (-200.0, -330.0)], false)
    };
    assert!(matches!(crossing(0.0), Err(Reason::Gap)));
    assert_eq!(crossing(100.0).unwrap().0.len(), 3);
    // A loop that starts and ends below the square: the two cuts at its ends count together.
    let ring = |depth: f64| {
        let points = [
            (500.0, -depth),
            (500.0, 0.0),
            (1000.0, 0.0),
            (1000.0, 1000.0),
            (0.0, 1000.0),
            (0.0, 0.0),
            (500.0, 0.0),
            (500.0, -depth),
        ];
        cut(&points, true)
    };
    let (runs, _) = ring(150.0).unwrap();
    assert_eq!((runs.len(), runs[0][0]), (1, runs[0][runs[0].len() - 1]));
    assert!(distance(runs[0][0], m(500.0, 0.0)) <= 30.0);
    assert!(matches!(ring(300.0), Err(Reason::Gap)), "two cuts of 270 m at the ends of a loop");
}

#[test]
fn a_source_line_joins_its_members_and_patches_a_cut_off_the_roads_of_its_kind() -> Result<(), String> {
    let hiking = |key, name, members: &[Vec<P>]| official(key, "hiking", 1, "", name, members);
    let sources = sources_of(vec![vec![
        // Two members that meet at node 2.
        hiking(1, "Joined", &[at(&[0, 1, 2]), at(&[4, 3, 2])]),
        // A point outside the bounds of the release.
        hiking(2, "Abroad", &[[at(&[10, 11, 12]), vec![[ORIGIN - 2_000_000, ORIGIN]]].concat()]),
        // A bump 100 m north of the grid, which the router goes around along the street.
        hiking(
            3,
            "Bump",
            &[[at(&[60, 61]), vec![off(61, 50.0, 100.0), off(61, 150.0, 100.0)], at(&[62, 63, 64])].concat()],
        ),
        // A loop that starts and ends 150 m west of the grid.
        hiking(
            4,
            "Ring",
            &[[
                vec![off(20, -150.0, 222.0), off(20, 0.0, 222.0)],
                at(&[30, 31, 21, 20]),
                vec![off(20, 0.0, 222.0), off(20, -150.0, 222.0)],
            ]
            .concat()],
        ),
        // The T5 path from node 25 to node 26, which no bike profile may use.
        official(5, "bicycle", 1, "", "Closed", &[at(&[45, 35, 25, 26, 36, 46])]),
    ]]);
    let (package, osm) = grid(vec![]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("ExtractEdge"), Some(&vec![source_id(1, 2, 0)]), "{:?}", report.dropped);
    assert_eq!(
        (&by_id[&source_id(1, 1, 0)]["length_m"], &by_id[&source_id(1, 3, 0)]["length_m"]),
        (&json!(4 * 445), &json!(4 * 445))
    );
    assert_eq!(by_id[&source_id(1, 4, 0)]["loop"], true);
    // The plan goes around the path, and back.
    let closed = by_id[&source_id(1, 5, 0)];
    assert!(closed["length_m"].as_u64() > Some(4 * 445), "{closed}");
    Ok(())
}

#[test]
fn the_merge_keeps_the_official_record_of_a_route() -> Result<(), String> {
    let sources = sources_of(vec![
        vec![
            official(561, "hiking", 1, "561", "Rigi Panoramaweg", &[at(&[0, 1, 2, 3])]),
            official(7, "hiking", 1, "7", "Ring", &[at(&[40, 41, 42, 52, 62, 61, 60, 50, 40])]),
            official(9, "hiking", 1, "9", "Nine", &[at(&[6, 16, 26, 36])]),
            official(11, "hiking", 2, "11", "Eleven", &[at(&[20, 21, 22, 23, 24, 25, 26])]),
            official(13, "mtb", 1, "13", "Thirteen", &[at(&[30, 31, 32, 33, 34, 35, 36])]),
            staged(official(3, "bicycle", 3, "3", "Three", &[]), &[(1, &[]), (2, &[at(&[60, 61, 62, 63])])]),
            official(21, "bicycle", 0, "21", "Twenty-one", &[at(&[0, 10, 20, 30])]),
            official(31, "bicycle", 2, "31", "Thirty-one", &[at(&[4, 14, 24, 34, 44])]),
            // Into the block from node 22 to node 33: a cut of more than 500 m.
            official(41, "bicycle", 2, "41", "Forty-one", &[at(&[2, 12, 22, 33])]),
        ],
        vec![official(5, "hiking", 1, "5", "Five", &[at(&[0, 1, 2, 3])])],
    ]);
    let hiking = |reference| vec![("route", "hiking"), ("ref", reference), ("network", "lwn")];
    let bike = |reference| vec![("route", "bicycle"), ("ref", reference), ("network", "rcn")];
    let marked = [("description", "Ein Weg mit Aussicht"), ("osmc:symbol", "yellow:white:yellow_bar")];
    let (package, osm) = grid(vec![
        route(101, &[hiking("SM 561"), marked.to_vec()].concat(), path(&[0, 1, 2, 3])),
        // Inside the loop of route 7.
        route(107, &hiking("7"), path(&[40, 41, 42, 52])),
        // Three times the line of route 9: it stays beside it.
        route(109, &hiking("9"), path(&[6, 16, 26, 36, 46, 56, 66, 65, 64, 63])),
        // Another ref along half of route 11.
        route(111, &[("route", "hiking"), ("ref", "12"), ("network", "rwn")], path(&[20, 21, 22, 23])),
        // The ref of route 13 along 17 % of it.
        route(113, &[("route", "mtb"), ("ref", "13"), ("network", "lcn")], path(&[36, 35, 45, 44, 54, 53, 52])),
        // The ref of route 3 at another level.
        route(103, &[("route", "bicycle"), ("ref", "3"), ("network", "lcn")], path(&[60, 61, 62, 63, 64, 65, 66])),
        // On the line of route 21: another ref and name, and another kind family.
        route(121, &[("route", "bicycle"), ("ref", "R"), ("name", "Other")], path(&[0, 10, 20, 30])),
        route(122, &[("route", "mtb"), ("name", "Bike")], path(&[0, 10, 20, 30])),
        // A gap of 890 m, and a way that the import does not hold.
        route(131, &bike("31"), [path(&[4, 14]), path(&[34, 44])].concat()),
        route(132, &bike("31"), [path(&[4, 14, 24]), vec![999]].concat()),
        route(141, &bike("41"), path(&[2, 12, 22, 32])),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    let dropped = |reason: &str| report.dropped.get(reason).cloned().unwrap_or_default();
    assert_eq!(dropped("Replaced"), [source_id(2, 5, 0), 101, 107, 121, 131, 132], "{:?}", report.dropped);
    assert_eq!(dropped("Gap"), [source_id(1, 41, 0)]);
    for id in [109, 111, 113, 103, 122, 141]
        .into_iter()
        .chain([(9, 0), (11, 0), (13, 0), (3, 2), (21, 0), (31, 0)].map(|(key, stage)| source_id(1, key, stage)))
    {
        assert!(by_id.contains_key(&id), "{id}");
    }
    assert!(
        !report.searches.iter().any(|search| [101, 107, 121, 131].contains(&search.0)),
        "a replaced relation needs no search"
    );
    // The record takes the mark of OSM, not its description.
    let rigi = by_id[&source_id(1, 561, 0)];
    assert_eq!((&rigi["symbol"], &rigi["credit"]), (&json!("yellow:white:yellow_bar"), &json!("Credit 1")));
    assert!(rigi.get("description").is_none());
    let new = by_id[&source_id(1, 11, 0)];
    assert_eq!((&new["name"], &new["rank"], &new["hardest"]), (&json!("Eleven"), &json!(2), &json!(4)));
    assert!(new["moving_s"].as_u64() > Some(0) && new.get("description").is_none());
    assert_eq!(report.sources["source-1"]["dropped"], json!({"Gap": 1}));
    Ok(())
}

#[test]
fn official_stages_take_the_places_of_the_stages_of_an_osm_trail() -> Result<(), String> {
    let trail = |id, name, reference, children: &[i64]| -> Route {
        let tags =
            vec![("type", "superroute"), ("route", "hiking"), ("name", name), ("ref", reference), ("network", "rwn")];
        (id, tags, members(children, Id::Relation))
    };
    let stage = |id, name| move |ways| route(id, &[("route", "hiking"), ("name", name)], ways);
    let run = |against: bool| {
        let line = if against { at(&[6, 5, 4, 3]) } else { at(&[3, 4, 5, 6]) };
        let sources = sources_of(vec![vec![
            staged(official(50, "hiking", 2, "HW", "Höhenweg", &[]), &[(2, &[line])]),
            staged(
                official(60, "hiking", 2, "T3", "Dreier", &[]),
                &[(1, &[at(&[63, 64, 65, 66])]), (2, &[at(&[66, 56, 46, 36])])],
            ),
        ]]);
        let (package, osm) = grid(vec![
            // Stage 2 leaves the extract.
            trail(200, "Höhenweg", "HW", &[201, 202, 203]),
            stage(201, "Etappe 1")(path(&[0, 1, 2, 3])),
            stage(202, "Etappe 2")([path(&[3, 4, 5, 6]), vec![999]].concat()),
            stage(203, "Etappe 3")(path(&[6, 16, 26, 36])),
            // Stage 2 over two official stages. The official trail of its name covers two thirds of it: a long
            // record keeps four fifths, so both trails stay.
            trail(300, "Dreier", "T3", &[301, 302]),
            stage(301, "Dreier 1")(path(&[60, 61, 62, 63])),
            stage(302, "Dreier 2")(path(&[63, 64, 65, 66, 56, 46, 36])),
        ]);
        catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)
    };
    let (records, report) = run(false)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("Replaced"), Some(&vec![202, 302]), "{:?}", report.dropped);
    let (official, first, second) = (source_id(1, 50, 2), source_id(1, 60, 1), source_id(1, 60, 2));
    let parts = |id: i64| -> Vec<(i64, u64)> {
        by_id[&id]["parts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["id"].as_i64().unwrap(), p["stage"].as_u64().unwrap()))
            .collect()
    };
    assert_eq!(parts(200), [(201, 1), (official, 2), (203, 3)]);
    assert_eq!((&by_id[&200]["stages"], &by_id[&200]["credit"]), (&json!([201, official, 203]), &json!("Credit 1")));
    // The official trail of one stage lists it at the level of trail 200, and it has the lower `id`.
    let (stage, trail) = (by_id[&official], source_id(1, 50, 0));
    assert_eq!(
        [&stage["trail"], &stage["trail_stage"], &stage["parent"], &stage["stage"]],
        [&json!(trail), &json!(2), &json!(trail), &json!(2)]
    );
    assert!(by_id.contains_key(&source_id(1, 60, 0)), "both trails of one name stay");
    assert_eq!(parts(300), [(301, 1), (first, 2), (second, 2)]);
    assert_eq!((&by_id[&300]["stage_count"], &by_id[&300]["stages"]), (&json!(2), &json!([301, first, second])));

    let (records, _) = run(true)?;
    let trail = by_id_of(&records, 200);
    assert!(trail.get("stages").is_none() && trail["parts"][1]["backwards"] == true);
    Ok(())
}

fn by_id_of(records: &[Value], id: i64) -> Value {
    records.iter().find(|r| r["id"] == id).unwrap().clone()
}

#[test]
fn one_official_route_stands_for_osm_stages_and_a_regional_stage_takes_the_regional_one() -> Result<(), String> {
    let long = |id, name, reference, children: &[i64]| -> Route {
        let tags =
            vec![("type", "superroute"), ("route", "hiking"), ("name", name), ("ref", reference), ("network", "rwn")];
        (id, tags, members(children, Id::Relation))
    };
    let stage = |id, name, ways| route(id, &[("route", "hiking"), ("network", "rwn"), ("name", name)], ways);
    let sources = sources_of(vec![vec![
        official(70, "hiking", 2, "S6", "Sechs", &[at(&[6, 16, 26, 36])]),
        official(80, "hiking", 3, "N8", "National", &[at(&[21, 22, 23, 24])]),
        official(85, "hiking", 2, "R5", "Regio", &[at(&[21, 22, 23, 24])]),
        // Stages 3 and 4 of the trail 600, and both stages of the trail 700.
        official(90, "hiking", 2, "V4", "Vier", &[at(&[63, 64, 65, 66])]),
        official(95, "hiking", 2, "Z2", "Zwei", &[at(&[10, 11, 12, 13])]),
    ]]);
    let (package, osm) = grid(vec![
        long(400, "Sechs", "S6", &[401, 402, 403, 404, 405, 406]),
        stage(401, "Etappe 1", path(&[0, 1, 2, 3])),
        stage(402, "Etappe 2", path(&[3, 4, 5, 6])),
        stage(403, "Etappe 3", path(&[6, 16])),
        stage(404, "Etappe 4", path(&[16, 26])),
        stage(405, "Etappe 5", path(&[26, 36])),
        stage(406, "Etappe 6", path(&[36, 46, 56, 66])),
        long(500, "Regio", "R5", &[501, 502]),
        route(
            501,
            &[
                ("route", "hiking"),
                ("name", "Regio 1"),
                ("network", "rwn"),
                ("description", "Words of the regional stage"),
                ("website", "https://regio.example"),
            ],
            path(&[21, 22, 23, 24]),
        ),
        stage(502, "Regio 2", path(&[24, 34, 44, 54])),
        long(600, "Vier", "V4", &[601, 602, 603, 604]),
        stage(601, "Vier 1", path(&[30, 40, 50, 60])),
        stage(602, "Vier 2", path(&[60, 61, 62, 63])),
        stage(603, "Vier 3", path(&[63, 64])),
        stage(604, "Vier 4", path(&[64, 65, 66])),
        long(700, "Zwei", "Z2", &[701, 702]),
        stage(701, "Zwei 1", path(&[10, 11])),
        stage(702, "Zwei 2", path(&[11, 12, 13])),
    ]);
    let (records, _) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    let (six, national, regional) = (source_id(1, 70, 0), source_id(1, 80, 0), source_id(1, 85, 0));
    let parts: Vec<(i64, u64)> = by_id[&400]["parts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| (p["id"].as_i64().unwrap(), p["stage"].as_u64().unwrap()))
        .collect();
    assert_eq!(parts, [(401, 1), (402, 2), (six, 3), (406, 6)]);
    assert_eq!((&by_id[&400]["stage_count"], &by_id[&400]["stages"]), (&json!(6), &json!([401, 402, six, 406])));
    assert_eq!((&by_id[&six]["trail_stage"], &by_id[&six]["stage"]), (&json!(3), &json!(3)));
    // The national route on the same line is a pair of stage 501 too, by its line alone.
    assert_eq!(
        (&by_id[&regional]["trail"], &by_id[&regional]["website"]),
        (&json!(500), &json!("https://regio.example"))
    );
    assert!(by_id[&regional].get("description").is_none());
    assert!(by_id[&national].get("trail").is_none() && by_id[&national].get("website").is_none());
    // N of "Stage n of N" is the number of stages of the trail, also when one route stands for its last
    // stages or for all of them; so a trail of two stages stays a trail.
    let parts = |id: i64| -> Vec<(i64, u64)> {
        let parts = by_id[&id]["parts"].as_array().unwrap().iter();
        parts.map(|p| (p["id"].as_i64().unwrap(), p["stage"].as_u64().unwrap())).collect()
    };
    let (four, two) = (source_id(1, 90, 0), source_id(1, 95, 0));
    assert_eq!((parts(600), &by_id[&600]["stage_count"]), (vec![(601, 1), (602, 2), (four, 3)], &json!(4)));
    assert_eq!((parts(700), &by_id[&700]["stage_count"]), (vec![(two, 1)], &json!(2)));
    assert!(trails::trails(&records).iter().any(|(entry, _)| entry["id"] == 700));
    Ok(())
}

#[test]
fn a_job_that_leaves_still_shows_and_no_route_takes_the_place_of_another() -> Result<(), String> {
    for (a, b, agree) in [("SM 561", "561", true), ("1;EV17", "1", true), ("007", "7", true), ("4.1", "4.1", true)]
        .into_iter()
        .chain([("4.1", "4", false), ("4.1", "1", false), ("12", "1", false)])
    {
        assert_eq!(merge::refs_agree(a, b), agree, "{a} {b}");
    }
    for (a, b, agree) in [
        ("Cloefpfad", "Traumschleife Cloefpfad", true),
        ("Auto-Pédestre Wahlhausen", "Wahlhausen", true),
        ("Bliessteig Bliessteig - Etappe 4", "Bliessteig - Etappe 4 (Blieskastel - Kirkel)", true),
        ("Eigertrail", "Eiger Trail", true),
        ("Bliessteig", "Bliessteig - Etappe 4", false),
        ("Bech", "B6 - Bech", false),
        ("Rundwanderweg 11", "Rundwanderweg 12", false),
        ("X Etappe 4", "Y - Etappe 4 (A - B)", false),
        ("", "Boevange", false),
        ("–", "–", false),
        ("Etappe 04", "Etappe 4", false),
        ("Weg II", "Weg 2", false),
    ] {
        assert_eq!(merge::names_agree(a, b), agree, "{a} | {b}");
    }
    let national = |reference| vec![("route", "hiking"), ("ref", reference), ("network", "nwn")];
    let sources = sources_of(vec![vec![
        official(3, "hiking", 3, "3", "Drei", &[at(&[0, 1, 2, 3, 4])]),
        // The national stage 2 leaves the bounds, so it fails; the regional route lies on its line.
        staged(
            official(7, "hiking", 3, "N7", "Sieben", &[]),
            &[(2, &[[at(&[63, 64, 65, 66]), vec![[ORIGIN - 2_000_000, ORIGIN]]].concat()])],
        ),
        official(20, "hiking", 2, "R2", "Regio", &[at(&[63, 64, 65, 66])]),
    ]]);
    let marked = [("website", "https://n7.example"), ("osmc:symbol", "red:white:red_bar")];
    let (package, osm) = grid(vec![
        // Inside route 3: it leaves for it.
        route(30, &national("3"), path(&[1, 2, 3])),
        // Half of route 3 lies along it: a pair by its ref, which stays beside route 3.
        route(31, &[("route", "hiking"), ("ref", "3")], path(&[2, 3, 4, 14, 24, 34, 44])),
        (
            80,
            [vec![("type", "superroute"), ("name", "Sieben")], national("N7")].concat(),
            members(&[81, 82], Id::Relation),
        ),
        route(81, &[("route", "hiking"), ("name", "Sieben 1")], path(&[60, 61, 62, 63])),
        route(
            82,
            &[vec![("route", "hiking"), ("name", "Sieben 2")], marked.to_vec()].concat(),
            path(&[63, 64, 65, 66]),
        ),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("Replaced"), Some(&vec![30]), "{:?}", report.dropped);
    assert!(by_id.contains_key(&source_id(1, 3, 0)) && by_id.contains_key(&31), "the line of route 30 still shows");
    // Stage 82 lies on the line of the regional route. Only a pair by its ref or name takes the place
    // of a stage, so both stay, and the trail stays complete.
    assert!(by_id.contains_key(&source_id(1, 20, 0)));
    assert_eq!(by_id[&80]["stages"], json!([81, 82]));
    Ok(())
}

#[test]
fn a_stage_compares_the_name_of_its_trail_with_its_own_name_and_never_its_own_name_alone() -> Result<(), String> {
    let walk = |key, name, nodes: &[i64]| official(key, "hiking", 0, "", name, &[at(nodes)]);
    let sources = sources_of(vec![vec![
        walk(10, "T - Etappe 2 (A - B)", &[43, 44, 45, 46]),
        // On the same line with another stage number: a pair by (b) only.
        walk(11, "T - Etappe 3 (B - C)", &[43, 44, 45, 46]),
    ]]);
    let trail = |id, name, children: &[i64]| -> Route {
        (id, vec![("type", "superroute"), ("route", "hiking"), ("name", name)], members(children, Id::Relation))
    };
    let stage = |id, name, nodes: &[i64]| route(id, &[("route", "hiking"), ("name", name)], path(nodes));
    // The stages of T have a ref of their own and the walks have none, so the names decide.
    let numbered =
        |id, name, nodes: &[i64]| route(id, &[("route", "hiking"), ("name", name), ("ref", "T")], path(nodes));
    let marked = [("route", "hiking"), ("name", "T - Etappe 2"), ("ref", "T"), ("osmc:symbol", "red:white:red_bar")];
    let (package, osm) = grid(vec![
        trail(900, "T", &[901, 902, 903]),
        numbered(901, "T - Etappe 1", &[40, 41, 42, 43]),
        route(902, &marked, path(&[43, 44, 45, 46])),
        numbered(903, "T - Etappe 3", &[46, 56, 66]),
        // Walk 10 lies along all of stage 912, and it along three quarters of walk 10: "Etappe 2" alone
        // would make them a pair.
        trail(910, "U", &[911, 912]),
        stage(911, "Etappe 1", &[30, 31, 32, 33]),
        stage(912, "Etappe 2", &[33, 43, 44, 45, 46]),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("Replaced"), Some(&vec![902]), "{:?}", report.dropped);
    let (day, other) = (source_id(1, 10, 0), source_id(1, 11, 0));
    let numbers = |id: i64| parts_of(by_id[&id]).into_iter().map(|(id, stage, _)| (id, stage)).collect::<Vec<_>>();
    assert_eq!(numbers(900), [(901, 1), (day, 2), (903, 3)]);
    assert_eq!(
        (&by_id[&day]["name"], &by_id[&day]["symbol"]),
        (&json!("T - Etappe 2 (A - B)"), &json!("red:white:red_bar"))
    );
    assert!(by_id[&other].get("trail").is_none() && by_id[&other].get("symbol").is_none());
    assert_eq!(numbers(910), [(911, 1), (912, 2)]);
    Ok(())
}

#[test]
fn a_pair_by_the_words_of_the_names_needs_one_line_four_fifths_along_the_other() -> Result<(), String> {
    let sources = sources_of(vec![vec![official(1, "hiking", 0, "", "Kehlen", &[at(&[20, 21, 22, 23, 24, 25])])]]);
    let hiking = |name, symbol| vec![("route", "hiking"), ("name", name), ("osmc:symbol", symbol)];
    let (package, osm) = grid(vec![
        // Another route of the village: each line lies 0.6 along the other, so its name alone would make a
        // pair that covers more than a half of it.
        route(1, &hiking("Kehlen Promenade", "blue:white:blue_bar"), path(&[22, 23, 24, 25, 35, 45])),
        route(2, &hiking("Auto-Pédestre Kehlen", "red:white:red_dot"), path(&[20, 21, 22, 23, 24, 25])),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("Replaced"), Some(&vec![2]), "{:?}", report.dropped);
    assert!(by_id.contains_key(&1));
    assert_eq!(by_id[&source_id(1, 1, 0)]["symbol"], "red:white:red_dot");
    Ok(())
}

#[test]
fn a_job_whose_pairs_agree_with_it_by_name_leaves_when_they_cover_a_half_of_it() -> Result<(), String> {
    // Each official route lies along all of its OSM route, from its start to `end`.
    let walk = |key, reference, name, nodes: &[i64], end: P| {
        official(key, "hiking", 0, reference, name, &[[at(nodes), vec![end]].concat()])
    };
    let sources = sources_of(vec![vec![
        walk(1, "", "Weg", &[10, 11, 12, 13], off(13, 140.0, 0.0)),
        walk(2, "", "Weg", &[30, 31, 32], off(32, 295.0, 0.0)),
        walk(3, "7", "Beta", &[50, 51, 52, 53], off(53, 140.0, 0.0)),
    ]]);
    let hiking = |id, pairs: &[(&'static str, &'static str)], row: i64| {
        route(id, &[&[("route", "hiking")], pairs].concat(), path(&(row..row + 7).collect::<Vec<_>>()))
    };
    let (package, osm) = grid(vec![
        // Covered 0.56 and 0.45.
        hiking(1, &[("name", "Weg")], 10),
        hiking(2, &[("name", "Weg")], 30),
        // Covered 0.56 by a pair whose refs agree and whose names do not.
        hiking(3, &[("name", "Alpha"), ("ref", "7")], 50),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("Replaced"), Some(&vec![1]), "{:?}", report.dropped);
    assert!(by_id.contains_key(&2) && by_id.contains_key(&3));
    Ok(())
}

/// A step of the made-up generator of `every_job_that_left_shows_in_the_records_of_earlier_origins`.
fn next(seed: &mut u64, n: u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 33) % n
}

/// A walk of up to `ways` streets that visits no node twice, from `start`, or from a node of the
/// generator when it is `None`, of 3 to 7 streets then.
fn walk(seed: &mut u64, start: Option<i64>, ways: usize) -> Vec<i64> {
    let start = start.unwrap_or_else(|| next(seed, 7) as i64 * 10 + next(seed, 7) as i64);
    let ways = if ways == 0 { 3 + next(seed, 5) as usize } else { ways };
    let mut nodes = vec![start];
    for _ in 0..ways {
        let at = nodes[nodes.len() - 1];
        let mut steps: Vec<i64> = [at - 10, at + 10, at - 1, at + 1]
            .into_iter()
            .filter(|&b| (0..70).contains(&b) && b % 10 < 7 && (b / 10 == at / 10 || b % 10 == at % 10))
            .filter(|&b| way(at, b) != way(15, 25) && !nodes.contains(&b))
            .collect();
        if steps.is_empty() {
            break;
        }
        nodes.push(steps.swap_remove(next(seed, steps.len() as u64) as usize));
    }
    nodes
}

#[test]
fn every_job_that_left_shows_in_the_records_of_earlier_origins() -> Result<(), String> {
    // Walks along the streets as official routes of two sources (some with stages), and as OSM
    // relations: copies, tails and heads of them, longer versions, routes that share only a part,
    // walks of their own, stages of trails, and a route at each edge of the extract. Few refs and
    // levels, so many of them are pairs.
    let mut seed = 7;
    let refs = ["1", "2", "SM 2", "", "3"];
    let mut lines = BTreeMap::new();
    let (mut sources, mut walks) = (vec![Vec::new(), Vec::new()], Vec::new());
    for key in 1..=16u64 {
        let origin = 1 + next(&mut seed, 2) as usize;
        let nodes = walk(&mut seed, None, 0);
        let reference = refs[next(&mut seed, 5) as usize];
        let route =
            official(key, "hiking", next(&mut seed, 3) as u8, reference, &format!("Weg {reference}"), &[at(&nodes)]);
        if key % 4 == 0 && nodes.len() >= 5 {
            let (first, second) = (nodes[..=nodes.len() / 2].to_vec(), nodes[nodes.len() / 2..].to_vec());
            sources[origin - 1].push(staged(route, &[(1, &[at(&first)]), (2, &[at(&second)])]));
            lines.insert(source_id(origin as i64, key as i64, 1), at(&first));
            lines.insert(source_id(origin as i64, key as i64, 2), at(&second));
        } else {
            sources[origin - 1].push(route);
            lines.insert(source_id(origin as i64, key as i64, 0), at(&nodes));
        }
        walks.push(nodes);
    }
    sources[0].push(official(
        90,
        "hiking",
        1,
        "9",
        "Rand",
        &[[at(&[60, 61, 62, 63]), vec![[ORIGIN - 2_000_000, ORIGIN]]].concat()],
    ));
    // One route in both sources: the second one leaves.
    for origin in [1, 2] {
        sources[origin - 1].push(official(91, "hiking", 1, "8", "Doppel", &[at(&[3, 13, 23, 33])]));
    }
    lines.insert(source_id(2, 91, 0), at(&[3, 13, 23, 33]));
    let networks = ["lwn", "rwn", ""];
    let mut relations: Vec<(i64, &str, &str, Vec<i64>)> = Vec::new();
    for id in 101..=140i64 {
        let base = walks[next(&mut seed, walks.len() as u64) as usize].clone();
        let n = base.len();
        let nodes = match next(&mut seed, 6) {
            0 => base.clone(),
            1 if n > 3 => base[1 + next(&mut seed, n as u64 - 3) as usize..].to_vec(),
            2 if n > 3 => base[..n - 1 - next(&mut seed, n as u64 - 3) as usize].to_vec(),
            k @ (3 | 4) => {
                // Longer, or on its first half only and then away from it.
                let keep = if k == 3 { n } else { n / 2 + 1 };
                let away = walk(&mut seed, Some(base[keep - 1]), 3);
                let away = away[1..].iter().take_while(|node| !base.contains(node));
                base[..keep].iter().chain(away).copied().collect()
            }
            _ => walk(&mut seed, None, 0),
        };
        relations.push((id, refs[next(&mut seed, 5) as usize], networks[next(&mut seed, 3) as usize], nodes));
    }
    // Trails of two stages along official lines.
    let mut trails = Vec::new();
    for t in 0..4i64 {
        let base = walks[next(&mut seed, walks.len() as u64) as usize].clone();
        if base.len() < 5 {
            continue;
        }
        let mid = base.len() / 2;
        relations.push((201 + 2 * t, "", "", base[..=mid].to_vec()));
        relations.push((202 + 2 * t, "", "", base[mid..].to_vec()));
        trails.push((300 + t, refs[next(&mut seed, 5) as usize], [201 + 2 * t, 202 + 2 * t]));
    }
    let tags: Vec<Vec<(&str, &str)>> = relations
        .iter()
        .map(|&(id, reference, network, _)| match id > 200 {
            true => [("route", "hiking"), ("name", if id % 2 == 1 { "Etappe 1" } else { "Etappe 2" })].to_vec(),
            false => [("route", "hiking"), ("ref", reference), ("network", network), ("name", "Weg")].to_vec(),
        })
        .collect();
    let mut osm: Vec<Route> =
        relations.iter().zip(&tags).map(|((id, _, _, nodes), tags)| route(*id, tags, path(nodes))).collect();
    for (id, reference, stages) in &trails {
        let tags = vec![
            ("type", "superroute"),
            ("route", "hiking"),
            ("name", "Trail"),
            ("ref", *reference),
            ("network", "rwn"),
        ];
        osm.push((*id, tags, members(stages, Id::Relation)));
    }
    // A relation that leaves the extract, on an official line.
    osm.push(route(150, &[("route", "hiking"), ("ref", "1"), ("name", "Weg")], [path(&walks[0]), vec![999]].concat()));
    for (id, _, _, nodes) in &relations {
        lines.insert(*id, at(nodes));
    }
    lines.insert(150, at(&walks[0]));
    let (package, osm) = grid(osm);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources_of(sources), 2, SECTION_M)?;
    // The invariant: only OSM jobs and jobs of the second source leave, and each job that left shows by
    // at least a half in the records of earlier origins in the catalog, and by four fifths unless its
    // names agree with theirs. No official name agrees with a name of a trail stage here.
    let origin = |id: i64| if id <= -sources::ORIGIN_ID { -id / sources::ORIGIN_ID } else { 3 };
    // A long record that leaves takes no record out of the catalog: the invariant holds for the jobs.
    let replaced: Vec<i64> =
        report.dropped.get("Replaced").into_iter().flatten().copied().filter(|id| lines.contains_key(id)).collect();
    assert!(
        replaced.len() >= 20 && replaced.iter().any(|&id| origin(id) == 2),
        "the made-up region tests the merge: {:?}",
        report.dropped
    );
    assert!(replaced.iter().all(|&id| origin(id) > 1), "a job of the first source left: {replaced:?}");
    for id in replaced {
        let earlier: Vec<Vec<P>> = records
            .iter()
            .filter(|r| r.get("line_udeg").is_some() && origin(r["id"].as_i64().unwrap()) < origin(id))
            .map(|r| decode(&r["line_udeg"]))
            .collect();
        let nears: Vec<shape::Near> = earlier.iter().map(|line| shape::Near::new(line)).collect();
        let probe = merge::Probe::new(0, "hiking", 0, "", &[], vec![lines[&id].clone()]);
        let shown = merge::share(&probe, &nears.iter().collect::<Vec<_>>());
        let limit = if (201..=208).contains(&id) { 0.8 } else { 0.5 };
        assert!(shown >= limit, "only {shown:.2} of the line of {id} shows in records of earlier origins");
    }
    Ok(())
}

#[test]
fn a_shaping_point_holds_a_straight_line_on_a_road_that_hikers_avoid() {
    // A main road 1.2 km east, and a lane 100 m north of it between the same ends. The line is one
    // straight segment along the main road; alone, the hiking route takes the lane.
    let package = streets(&[
        ("secondary", 2, &[[0.0, 0.0], [1200.0, 0.0]]),
        ("residential", 1, &[[0.0, 0.0], [0.0, 100.0], [1200.0, 100.0], [1200.0, 0.0]]),
    ]);
    let line = [off(0, 0.0, 0.0), off(0, 1200.0, 0.0)];
    assert_eq!(hiking_plan(&package, &line, false), Ok(0.0));
}

#[test]
fn shaping_points_that_do_not_help_leave_before_the_whole_plan_decides() {
    // A block of streets, and a dead end from its south-east corner that loops out east and ends
    // 1 m from the line, which runs 5 m from the street there. The vertex at that end is the first
    // plan point of the loop and attaches to the dead end, so every leg to or from it runs out east.
    let package = streets(&[
        ("residential", 1, &[[0.0, 0.0], [300.0, 0.0], [300.0, 290.0], [0.0, 290.0], [0.0, 0.0]]),
        ("service", 1, &[[300.0, 0.0], [500.0, -100.0], [500.0, 105.0], [306.0, 105.0], [306.0, 95.0]]),
    ]);
    let corners = [[0.0, 0.0], [300.0, 0.0], [305.0, 20.0], [305.0, 100.0], [305.0, 290.0], [0.0, 290.0], [0.0, 0.0]];
    let line: Vec<P> = corners.iter().map(|&[east, north]| off(0, east, north)).collect();
    assert_eq!(hiking_plan(&package, &line, true), Ok(0.0));
}

/// The `id`, the stage number and the name of each part of a long record.
fn parts_of(record: &Value) -> Vec<(i64, u64, &str)> {
    let parts = record["parts"].as_array().unwrap().iter();
    parts.map(|p| (p["id"].as_i64().unwrap(), p["stage"].as_u64().unwrap(), p["name"].as_str().unwrap_or(""))).collect()
}

#[test]
fn a_source_route_with_stages_is_a_trail_with_the_official_numbers() -> Result<(), String> {
    let run = |stages: &[(u64, &[Vec<P>])]| {
        let route = staged(official(70, "hiking", 2, "70", "Drei Etappen", &[]), stages);
        let (package, osm) = grid(vec![]);
        catalog(&Selection::whole(package.fork()), osm, sources_of(vec![vec![route]]), 2, SECTION_M)
    };
    let (first, second, last) = ([at(&[0, 1, 2, 3])], [at(&[3, 4, 5, 6])], [at(&[6, 16, 26, 36])]);
    let (trail, stage) = (source_id(1, 70, 0), |number| source_id(1, 70, number));
    // A route that OSM lacks.
    let (records, _) = run(&[(1, &first), (2, &second), (3, &last)])?;
    let by_id = by_id(&records);
    let long = by_id[&trail];
    assert_eq!((&long["stages"], &long["stage_count"]), (&json!([stage(1), stage(2), stage(3)]), &json!(3)));
    assert_eq!(long["credit"], "Credit 1");
    for number in 1..=3 {
        let record = by_id[&stage(number)];
        let fields = [&record["trail"], &record["trail_stage"], &record["parent"], &record["stage"]];
        assert_eq!(fields, [&json!(trail), &json!(number), &json!(trail), &json!(number)]);
    }
    let far = trails::trails(&records);
    assert_eq!(far.iter().find(|(entry, _)| entry["id"] == trail).unwrap().0["credit"], "Credit 1");

    // A number with no stage row has no part, and N of "Stage n of N" is the highest number.
    let (records, _) = run(&[(1, &first), (2, &second), (4, &last)])?;
    let long = by_id_of(&records, trail);
    let numbers = [(stage(1), 1, "Stage 1"), (stage(2), 2, "Stage 2"), (stage(4), 4, "Stage 4")];
    assert_eq!((parts_of(&long), &long["stage_count"]), (numbers.to_vec(), &json!(4)));
    assert!(long.get("stages").is_some());
    // A stage with no members is a part not in the catalog, without ends.
    let (records, _) = run(&[(1, &first), (2, &[]), (4, &last)])?;
    let long = by_id_of(&records, trail);
    assert_eq!(parts_of(&long), numbers);
    let missing = &long["parts"][1];
    assert!(missing["missing"] == true && missing.get("start_udeg").is_none() && long.get("stages").is_none());
    Ok(())
}

#[test]
fn an_official_trail_takes_the_place_of_the_osm_trail_of_its_route_and_not_of_another_rank() -> Result<(), String> {
    let sources = sources_of(vec![vec![
        staged(
            official(50, "hiking", 2, "HW", "Höhenweg", &[]),
            &[
                (1, &[at(&[0, 1, 2, 3, 4, 5, 6])]),
                (2, &[at(&[6, 16, 26, 36, 46, 56, 66])]),
                (3, &[at(&[66, 65, 64, 63, 62, 61, 60])]),
            ],
        ),
        staged(
            official(80, "bicycle", 3, "8", "Acht", &[]),
            &[(1, &[at(&[11, 12, 13, 14])]), (2, &[at(&[14, 24, 34, 44])])],
        ),
    ]]);
    let hiking = |id, name, nodes: &[i64]| route(id, &[("route", "hiking"), ("name", name)], path(nodes));
    let bike = |id, name, nodes: &[i64]| route(id, &[("route", "bicycle"), ("name", name)], path(nodes));
    let marked = [("osmc:symbol", "red:white:red_bar"), ("description", "Ein Weg")];
    let (package, osm) = grid(vec![
        (
            200,
            [
                &[("type", "superroute"), ("route", "hiking"), ("name", "Höhenweg"), ("ref", "HW"), ("network", "rwn")]
                    [..],
                &marked,
            ]
            .concat(),
            members(&[201, 202, 203, 204], Id::Relation),
        ),
        hiking(201, "Etappe 1", &[0, 1, 2, 3, 4, 5, 6]),
        hiking(202, "Etappe 2", &[6, 16, 26, 36, 46, 56, 66]),
        hiking(203, "Etappe 3", &[66, 65, 64, 63, 62, 61, 60]),
        // A variant that no official stage replaces: a seventh of the trail.
        hiking(204, "Etappe 4", &[60, 50, 40, 30]),
        // The same trail at the international level.
        (
            800,
            vec![("type", "superroute"), ("route", "bicycle"), ("name", "Acht"), ("ref", "8"), ("network", "icn")],
            members(&[801, 802], Id::Relation),
        ),
        bike(801, "Acht 1", &[11, 12, 13, 14]),
        bike(802, "Acht 2", &[14, 24, 34, 44]),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    let (trail, acht) = (source_id(1, 50, 0), source_id(1, 80, 0));
    assert_eq!(report.dropped.get("Replaced"), Some(&vec![200, 201, 202, 203, 801, 802]), "{:?}", report.dropped);
    // The official trail takes the mark of the OSM trail that left for it, not its description.
    assert_eq!(by_id[&trail]["symbol"], "red:white:red_bar");
    assert!(by_id[&trail].get("description").is_none());
    let variant = by_id[&204];
    assert!(["trail", "trail_stage", "parent", "stage"].iter().all(|field| variant.get(*field).is_none()));
    // A trail of another rank stays, with the official stages as its parts, and they belong to the official trail.
    assert_eq!(parts_of(by_id[&800]), [(source_id(1, 80, 1), 1, "Stage 1"), (source_id(1, 80, 2), 2, "Stage 2")]);
    let first = by_id[&source_id(1, 80, 1)];
    assert_eq!((&first["trail"], &first["parent"]), (&json!(acht), &json!(acht)));
    let far: Vec<i64> = trails::trails(&records).iter().map(|(entry, _)| entry["id"].as_i64().unwrap()).collect();
    assert_eq!(far, [acht, trail, 800]);
    Ok(())
}

#[test]
fn a_failed_official_stage_takes_the_osm_stage_that_lies_along_it_both_ways() -> Result<(), String> {
    // It ends 560 m west of the grid, away from each road: a cut of more than 500 m. Stage 3 fails on the same path.
    let failing = [[at(&[6, 16, 26, 36, 46, 56, 66, 65, 64, 63, 62, 61, 60]), vec![off(60, -560.0, 0.0)]].concat()];
    let sources = sources_of(vec![vec![staged(
        official(60, "hiking", 2, "PR", "Panorama", &[]),
        &[(1, &[at(&[0, 1, 2, 3, 4, 5, 6])]), (2, &failing), (3, &failing)],
    )]]);
    let hiking = |id, name, nodes: &[i64]| route(id, &[("route", "hiking"), ("name", name)], path(nodes));
    let long = |id, name, children: &[i64]| -> Route {
        let tags = vec![("type", "superroute"), ("route", "hiking"), ("name", name), ("ref", "PR"), ("network", "rwn")];
        (id, tags, members(children, Id::Relation))
    };
    let (package, osm) = grid(vec![
        long(300, "Panorama", &[301, 302]),
        hiking(301, "Etappe 1", &[0, 1, 2, 3, 4, 5, 6]),
        hiking(302, "Etappe 2", &[6, 16, 26, 36, 46, 56, 66, 65, 64, 63, 62, 61, 60]),
        // A loop over the same path and back to its start: the stage lies along it, but it lies only half along the
        // stage.
        long(400, "Rundwege", &[299]),
        hiking(
            299,
            "Rundweg",
            &[6, 16, 26, 36, 46, 56, 66, 65, 64, 63, 62, 61, 60, 50, 40, 30, 20, 10, 0, 1, 2, 3, 4, 5, 6],
        ),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    let trail = source_id(1, 60, 0);
    let gaps = vec![source_id(1, 60, 3), source_id(1, 60, 2)];
    assert_eq!(report.dropped.get("Gap"), Some(&gaps), "{:?}", report.dropped);
    // A record stands once in a trail: stage 3 does not take record 302 again.
    let numbers = [(source_id(1, 60, 1), 1, "Stage 1"), (302, 2, "Stage 2"), (source_id(1, 60, 3), 3, "Stage 3")];
    assert_eq!(parts_of(by_id[&trail]), numbers);
    assert!(by_id[&trail]["parts"][2]["missing"] == true && by_id[&trail].get("stages").is_none());
    let fill = by_id[&302];
    let fields = [&fill["trail"], &fill["trail_stage"], &fill["name"]];
    assert_eq!(fields, [&json!(trail), &json!(2), &json!("Etappe 2")]);
    assert_eq!(by_id[&299]["trail"], 400);
    Ok(())
}

#[test]
fn an_unnamed_stage_keeps_the_name_of_a_trail_that_leaves() -> Result<(), String> {
    let sources = sources_of(vec![vec![staged(
        official(50, "hiking", 2, "HW", "Höhenweg", &[]),
        &[(1, &[at(&[0, 1, 2, 3])]), (2, &[at(&[3, 4, 5, 6])]), (3, &[at(&[6, 16, 26, 36])])],
    )]]);
    let hiking = |id, name, nodes: &[i64]| route(id, &[("route", "hiking"), ("name", name)], path(nodes));
    // Two pieces 1.3 km apart: the route fails, with the ends of its main line.
    let broken =
        |id, name, a: &[i64], b: &[i64]| route(id, &[("route", "hiking"), ("name", name)], [path(a), path(b)].concat());
    let (package, osm) = grid(vec![
        (
            200,
            vec![("type", "superroute"), ("route", "hiking"), ("name", "Höhenweg"), ("ref", "HW"), ("network", "rwn")],
            members(&[201, 210, 203], Id::Relation),
        ),
        hiking(201, "Etappe 1", &[0, 1, 2, 3]),
        // The unnamed second stage, with a mark of its own (no connector leg), whose only parent is a set of
        // separate routes.
        route(202, &[("route", "hiking"), ("osmc:symbol", "red")], path(&[3, 4, 5, 6])),
        hiking(203, "Etappe 3", &[6, 16, 26, 36]),
        (
            210,
            vec![("type", "route"), ("route", "hiking"), ("name", "Rundwege"), ("ref", "R9")],
            members(&[202, 211, 212, 213], Id::Relation),
        ),
        broken(211, "Rund 1", &[60, 61], &[64, 65]),
        broken(212, "Rund 2", &[40, 41], &[44, 45]),
        broken(213, "Rund 3", &[20, 21], &[24, 25]),
    ]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, sources, 2, SECTION_M)?;
    let by_id = by_id(&records);
    assert_eq!(report.dropped.get("Apart"), Some(&vec![210]), "{:?}", report.dropped);
    assert!(report.dropped["Replaced"].contains(&200), "{:?}", report.dropped);
    // Trail 200 left for the official trail; its stage still shows, with its name.
    let stage = by_id[&202];
    assert_eq!((&stage["name"], &stage["ref"], stage.get("trail")), (&json!("Höhenweg"), &json!("HW"), None));
    Ok(())
}

#[test]
fn a_long_record_joins_the_distinct_credits_of_its_origin_and_its_parts() {
    let record =
        json!({"length_m": 1_000, "ascent_m": 0, "descent_m": 0, "moving_s": 900, "loop": false, "credit": "A; B"});
    let kept = Kept {
        record: record.as_object().unwrap().clone(),
        start: [0, 0],
        finish: [0, 9_000],
        points: 2,
        cells: BTreeSet::new(),
        grades: None,
        french: false,
    };
    let part = long::Part { id: 1, name: String::new(), stage: Some(1), cut: false, kept: Some(&kept), line: None };
    let header = json!({"id": -100, "kind": "bicycle", "rank": 3, "credit": "A"}).as_object().unwrap().clone();
    let (record, _) = long::record(header, true, "bicycle", &[part], false, 1).unwrap();
    assert_eq!(record["credit"], "A; B");
}
