use super::*;
use route_engine::{
    model::{Graph, Point, Profile, Road, Surface, BIKE, FOOT, PUSH},
    package::digest,
};
use std::sync::Arc;

#[derive(Clone)]
struct Memory(Arc<HashMap<String, Vec<u8>>>);
impl Source for Memory {
    fn read(&self, key: &str) -> route_engine::Result<Vec<u8>> {
        self.0.get(key).cloned().ok_or_else(|| route_engine::Error::MissingRegion(key.into()))
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

/// A 7 by 7 grid of streets 445 m apart on rolling ground, with node `10 * row + column`. The way
/// from node 0 to node 1 is T3.
fn grid(relations: Vec<Relation>) -> Package<Memory> {
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
                graph.osm.ways.insert(id, Way { id, nodes: vec![a, b], tags: tags(&[("highway", "residential")]) });
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
                        difficulty: 255,
                        hiking_difficulty: (id == way(0, 1)).then_some(3),
                        uncertain_access: false,
                        structure: false,
                        shape,
                    });
                }
            }
        }
    }
    graph.osm.relations = relations.into_iter().map(|r| (r.id, r)).collect();
    let profiles: Vec<_> = Profile::presets()
        .into_iter()
        .filter(|p| ["touring", "road", "gravel", "mtb", "hiking"].contains(&p.name.as_str()))
        .collect();
    let mut objects = HashMap::new();
    let manifest = crate::prepare(&graph, "test".into(), [-1.0, -1.0, 1.0, 1.0], &profiles, vec![], |bytes| {
        objects.insert(digest(bytes), bytes.to_vec());
        Ok(digest(bytes))
    })
    .unwrap();
    Package::open(Memory(Arc::new(objects)), &serde_json::to_vec(&manifest).unwrap()).unwrap()
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
    let package = grid(vec![
        // East, then north: the router needs a shaping point near the corner. The way from node
        // 14 to node 24 is missing, a gap of 445 m.
        route(1, &trail, corner),
        // A ring around four blocks.
        route(2, &[("route", "foot"), ("ref", "R"), ("network", "rwn")], path(&[44, 45, 46, 56, 66, 65, 64, 54, 44])),
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
    ]);
    let relations = read!(package, relations, Relation, |_| true);
    let (records, report) = catalog(&package, relations, false, 2)?;
    let by_id: HashMap<i64, &Value> = records.iter().map(|r| (r["id"].as_i64().unwrap(), r)).collect();
    let dropped: Vec<_> = report.dropped.iter().map(|(reason, ids)| (reason.as_str(), ids.clone())).collect();
    assert_eq!(dropped, [("ExtractEdge", vec![7]), ("Gap", vec![3]), ("NestedLongRoute", vec![6])]);
    assert_eq!(by_id.keys().copied().collect::<BTreeSet<_>>(), BTreeSet::from([1, 2, 5, 8, 9]));

    let trail = by_id[&1];
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
    let mut router = Router::new(package.fork(), 768 << 20);
    let routed = router.route(&shape::request("hiking", &plan, vec![]), &shape::control()).unwrap().totals;
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
    assert_eq!((&whole["stages"], &whole["length_m"]), (&json!([1, 2]), &json!(16 * 445)));
    assert_eq!(whole["start_udeg"], json!([ORIGIN, ORIGIN]));
    assert!(whole.get("line_udeg").is_none());
    // The trail ends where the ring starts, so that point counts once.
    let points = via + 2 + ring["via"].as_array().unwrap().len() + 2 - 1;
    assert_eq!(report.long_points, [(5, points)]);
    Ok(())
}
