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
    let (records, report) = catalog(&selection, osm, 2, SECTION_M)?;
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
    assert_eq!((&trail["loop"], &trail["length_m"]), (&json!(false), &json!(8 * 445)));
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
    assert_eq!((&whole["stages"], &whole["length_m"]), (&json!([1, 2]), &json!(16 * 445)));
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
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, 2, section_m)?;
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
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, 2, SECTION_M)?;
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
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, 2, SECTION_M)?;
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
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, 2, SECTION_M)?;
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
