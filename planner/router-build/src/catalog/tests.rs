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
    let mut graph = Graph::default();
    let mut index = HashMap::new();
    let (mut ways, mut relations) = (Builder::<Ways>::default(), Builder::<Relations>::default());
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
    let manifest =
        crate::prepare(&graph, "test".into(), [-1.0, -1.0, 1.0, 1.0], &profiles, vec![], "test".into(), |bytes| {
            objects.insert(digest(bytes), bytes.to_vec());
            Ok(digest(bytes))
        })
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
    let (records, report) = catalog(&selection, osm, false, 2)?;
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
        [&trail["length_m"], &trail["ascent_m"], &trail["descent_m"]],
        [&json!(routed.distance_m), &json!(routed.ascent_m), &json!(routed.descent_m)]
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

#[test]
fn catalog_keeps_a_hiking_route_over_a_t5_way() -> Result<(), String> {
    let (package, osm) =
        grid(vec![route(13, &[("route", "hiking"), ("name", "Ridge")], path(&[23, 24, 25, 26, 36, 46]))]);
    let (records, report) = catalog(&Selection::whole(package.fork()), osm, false, 2)?;
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
