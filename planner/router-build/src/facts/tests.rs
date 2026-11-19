use osmpbfreader::{NodeId, OsmId, Tags, WayId};
use planner_router::geometry::METRES_PER_UDEG;
use serde_json::json;

use super::osm::{features, kind, Element, Kind, Lift, Parking, Place};
use super::shares::{forest, paved, Paved, Samples};
use super::*;

const LAT: i32 = 47_000_000;
/// Microdegrees of longitude per metre at `LAT`.
const EAST: f64 = 1.0 / (METRES_PER_UDEG * 0.681_998_360_062_498_5);

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs.iter().map(|&(k, v)| (k.into(), v.into())).collect()
}

/// The point `x` metres east and `y` metres north of 8° E, `LAT`.
fn at(x: f64, y: f64) -> P {
    [8_000_000 + (x * EAST).round() as i32, LAT + (y / METRES_PER_UDEG).round() as i32]
}

/// A route of 2 km along the latitude `LAT`, from 0 to 2 km east, in the cell `9-267-180`.
fn route(id: i64) -> Route {
    let line = vec![at(0.0, 0.0), at(2000.0, 0.0)];
    Route { id, cells: vec!["9-267-180".into()], samples: samples(&line, 2000.0), line, highest_m: Some(812) }
}

/// Two points that differ by the rounding of a microdegree.
fn assert_close(a: P, b: P) {
    assert!((a[0] - b[0]).abs() <= 1 && (a[1] - b[1]).abs() <= 1, "{a:?} {b:?}");
}

fn place(id: &str, kind: &'static str, at: P) -> Place {
    Place { id: id.into(), kind, name: None, ele_m: None, at, outline: Vec::new() }
}

#[test]
fn the_tags_decide_the_kind_of_an_element() {
    let kinds = [
        (&[("amenity", "parking")][..], Some(Kind::Parking)),
        (&[("amenity", "parking"), ("access", "private")], None),
        (&[("amenity", "parking"), ("access", "customers")], None),
        (&[("amenity", "parking"), ("access", "permissive")], Some(Kind::Parking)),
        (&[("aerialway", "gondola")], Some(Kind::Lift)),
        (&[("aerialway", "mixed_lift")], Some(Kind::Lift)),
        (&[("aerialway", "drag_lift")], None),
        (&[("aerialway", "station")], None),
        (&[("natural", "peak")], Some(Kind::Peak)),
        (&[("tourism", "alpine_hut"), ("amenity", "restaurant")], Some(Kind::Hut)),
        (&[("amenity", "biergarten")], Some(Kind::Food)),
        (&[("natural", "water"), ("water", "pond")], Some(Kind::Lake)),
        (&[("natural", "water"), ("water", "reservoir")], None),
        (&[("natural", "water"), ("water", "river")], Some(Kind::River)),
        (&[("waterway", "canal")], Some(Kind::River)),
        (&[("waterway", "stream")], None),
        (&[("waterway", "waterfall")], Some(Kind::Waterfall)),
        (&[("landuse", "retail")], Some(Kind::BuiltUp)),
        (&[("landuse", "farmland")], None),
    ];
    for (pairs, expected) in kinds {
        assert_eq!(kind(&tags(pairs)), expected, "{pairs:?}");
    }
}

#[test]
fn the_capacity_decides_the_size_class_and_else_the_area() {
    let parking = |capacity, area_m2| Parking { at: at(0.0, 0.0), capacity, area_m2 }.large();
    assert!(parking(Some(50), None));
    assert!(!parking(Some(49), Some(5000.0)), "a capacity under 50 is small, whatever the area");
    assert!(parking(None, Some(1250.0)));
    assert!(!parking(None, Some(1249.0)));
    assert!(!parking(None, None));
}

/// A square of `side` metres with its south-west corner at `x`, `y`, as a closed ring of nodes
/// from `first`.
fn square(first: i64, x: f64, y: f64, side: f64, nodes: &mut Vec<(i64, P)>) -> Vec<i64> {
    let corners = [(0.0, 0.0), (side, 0.0), (side, side), (0.0, side)];
    let ids: Vec<i64> = (first..first + 4).collect();
    nodes.extend(ids.iter().zip(corners).map(|(&id, (dx, dy))| (id, at(x + dx, y + dy))));
    ids.iter().copied().chain([first]).collect()
}

fn element(id: OsmId, pairs: &[(&str, &str)], point: Option<P>, ways: Vec<(Vec<i64>, bool)>) -> Element {
    let tags = tags(pairs);
    Element { point, ways, ..Element::new(id, kind(&tags).unwrap(), &tags) }
}

#[test]
fn areas_give_the_size_the_position_and_the_food_rule() {
    let mut nodes = Vec::new();
    let town = square(1, 0.0, 0.0, 500.0, &mut nodes);
    let parking = square(11, 1000.0, 0.0, 40.0, &mut nodes);
    let pond = square(21, 2000.0, 0.0, 90.0, &mut nodes);
    let lake = square(31, 3000.0, 0.0, 120.0, &mut nodes);
    // A lake of 200 m by 200 m with an island of 150 m by 150 m: 17,500 m², in two ways.
    let outer = square(41, 4000.0, 0.0, 200.0, &mut nodes);
    let island = square(51, 4025.0, 25.0, 150.0, &mut nodes);
    let way = |n| OsmId::Way(WayId(n));
    let elements = vec![
        element(way(1), &[("landuse", "residential")], None, vec![(town, false)]),
        element(OsmId::Node(NodeId(100)), &[("amenity", "cafe")], Some(at(250.0, 250.0)), vec![]),
        element(OsmId::Node(NodeId(101)), &[("amenity", "cafe")], Some(at(750.0, 250.0)), vec![]),
        element(way(2), &[("amenity", "parking")], None, vec![(parking, false)]),
        element(way(3), &[("natural", "water"), ("water", "pond")], None, vec![(pond, false)]),
        element(way(4), &[("natural", "water"), ("water", "lake")], None, vec![(lake.clone(), false)]),
        element(
            way(5),
            &[("natural", "water"), ("water", "lake"), ("name", "Seeli")],
            None,
            vec![(vec![21, 22, 23, 21], false)],
        ),
        element(
            OsmId::Relation(osmpbfreader::RelationId(6)),
            &[("natural", "water"), ("water", "lake")],
            None,
            vec![(outer[..3].to_vec(), false), (outer[2..].to_vec(), false), (island, true)],
        ),
        element(way(7), &[("aerialway", "chair_lift")], None, vec![(vec![11, 13], false)]),
    ];
    let point = |id: i64| nodes.iter().find(|(node, _)| *node == id).map(|(_, p)| *p);
    let found = features(elements, &point, &|node| (node == 13).then_some(1650.0));
    let ids: Vec<&str> = found.places.iter().map(|place| place.id.as_str()).collect();
    assert_eq!(ids, ["n101", "w4", "w5", "r6"], "the café in the town and the unnamed pond under 1 ha are no places");
    let parking = &found.parkings[0];
    assert_close(parking.at, at(1020.0, 20.0));
    assert!((parking.area_m2.unwrap() - 1600.0).abs() < 10.0, "{:?}", parking.area_m2);
    let lake = found.places.iter().find(|place| place.id == "r6").unwrap();
    assert_eq!((lake.kind, lake.outline.len()), ("lake", 3 + 3 + 5));
    assert_eq!(found.water.len(), 1 + 1 + 3, "each way of each lake is a shore");
    assert_eq!(found.lifts, [Lift { ends: [at(1000.0, 0.0), at(1040.0, 40.0)], ele: [None, Some(1650.0)] }]);
}

#[test]
fn the_samples_are_where_the_profile_has_its_entries() {
    let line = [at(0.0, 0.0), at(300.0, 0.0), at(300.0, 50.0)];
    let samples = samples(&line, 700.0);
    // 700 m of plan route on 350 m of line: entries at 0, 100, …, 700 and the end again.
    assert_eq!(samples.len(), 9);
    assert_eq!(samples[0], at(0.0, 0.0));
    assert_close(samples[2], at(100.0, 0.0));
    assert_close(samples[6], at(300.0, 0.0));
    assert_eq!((samples[7], samples[8]), (line[2], line[2]));
}

#[test]
fn the_nearest_highway_decides_the_paved_share_and_any_big_road_within_200_m_counts() {
    assert_eq!(paved("track", Some("asphalt")), Paved::Surface);
    assert_eq!(paved("residential", Some("gravel")), Paved::No);
    assert_eq!(paved("tertiary_link", None), Paved::Class);
    assert_eq!(paved("service", None), Paved::No);
    let routes = [route(1)];
    let mut samples = Samples::new(&routes, 47.0);
    // The first km lies 5 m from a track with gravel and 12 m from an asphalt road. Then a path
    // without a surface to 1,500 m, a residential road without a surface 3 m away, and a primary
    // road 150 m away.
    samples.road("track", Some("gravel"), &[at(0.0, 5.0), at(1050.0, 5.0)]);
    samples.road("residential", Some("asphalt"), &[at(0.0, -12.0), at(1050.0, -12.0)]);
    samples.road("path", None, &[at(1050.0, 0.0), at(1500.0, 0.0)]);
    samples.road("residential", None, &[at(1550.0, -3.0), at(2000.0, -3.0)]);
    samples.road("primary", None, &[at(1450.0, 150.0), at(2000.0, 150.0)]);
    samples.water(&[vec![at(1500.0, -90.0), at(1500.0, -500.0)]]);
    let shares = samples.shares()[0];
    // 22 samples: each 100 m from 0 to 2,000 m, and the end again.
    let percent = |count: f64| (100.0 * count / 22.0).round() as u8;
    assert_eq!(shares.paved, percent(6.0), "1,600 to 2,000 m and the end, by the class of the road");
    assert_eq!(samples.paved(), (22, 6, 6), "the nearer track is not paved, so the asphalt road does not count");
    assert_eq!(shares.big_road, percent(8.0), "1,400 to 2,000 m and the end are within 200 m");
    assert_eq!(shares.water, percent(1.0), "only the sample at 1,500 m is within 100 m");
    assert_eq!(shares.canopy, None, "without the archive the canopy share is absent");
}

#[test]
fn a_canopy_cell_is_forest_from_a_cover_of_50_percent() {
    let code = |height: u16, cover: u16| height | cover << 7;
    assert!(forest(code(40, 5)) && forest(code(40, 10)));
    assert!(!forest(code(40, 4)) && !forest(0));
}

fn features_with(parkings: Vec<Parking>, lifts: Vec<Lift>, places: Vec<Place>) -> Features {
    Features { parkings, lifts, places, water: Vec::new() }
}

#[test]
fn an_entry_holds_the_car_parks_and_lifts_within_3_km_and_the_places_within_150_m() {
    let parking = |x: f64, y: f64, capacity| Parking { at: at(x, y), capacity: Some(capacity), area_m2: None };
    let features = features_with(
        vec![
            parking(500.0, 2900.0, 80),
            parking(1000.0, -400.0, 10),
            parking(500.0, 3100.0, 80),
            parking(0.0, 5250.0, 5),
        ],
        vec![
            // Its top by the `ele` tag; a car park 250 m from its valley station.
            Lift { ends: [at(0.0, 5000.0), at(100.0, 1000.0)], ele: [Some(600.0), Some(1500.0)] },
            // Its top by the terrain, 3.5 km from the route.
            Lift { ends: [at(0.0, -1000.0), at(0.0, -3500.0)], ele: [None, None] },
        ],
        vec![place("n1", "peak", at(1000.0, 140.0)), place("n2", "hut", at(1000.0, 160.0))],
    );
    let routes = [route(1)];
    let shares = Samples::new(&routes, 47.0).shares();
    let mut height = |p: P| Ok(Some(f64::from(LAT - p[1])));
    let Facts { entries, places, report } = entries(&routes, &features, &shares, &mut height).unwrap();
    let entry = Value::Object(entries[0].clone());
    assert_eq!(
        entry,
        json!({
            "car_parks": [{"at_udeg": at(1000.0, -400.0), "walk_m": 400, "size": "small"},
                          {"at_udeg": at(500.0, 2900.0), "walk_m": 2900, "size": "large"}],
            "lifts": [{"top_udeg": at(100.0, 1000.0), "walk_m": 1000, "valley_udeg": at(0.0, 5000.0),
                       "valley_parking": true}],
            "places": ["n1"], "highest_m": 812, "paved_pct": 0, "big_road_pct": 0, "water_pct": 0,
        })
    );
    assert_eq!(places.keys().collect::<Vec<_>>(), ["n1"]);
    assert_eq!(places["n1"], json!({"kind": "peak", "at_udeg": at(1000.0, 140.0)}));
    assert_eq!((report.with_car_park_1km, report.with_place, report.with_canopy), (1, 1, 0));
}

#[test]
fn a_lift_with_ends_of_one_height_or_without_a_height_is_left_out() {
    let lift = |ele| Lift { ends: [at(0.0, 100.0), at(500.0, 100.0)], ele };
    let features = features_with(vec![], vec![lift([Some(900.0), Some(900.0)]), lift([None, Some(900.0)])], vec![]);
    let routes = [route(1)];
    let shares = Samples::new(&routes, 47.0).shares();
    let facts = entries(&routes, &features, &shares, &mut |_| Ok(None)).unwrap();
    assert!(!facts.entries[0].contains_key("lifts"));
}

#[test]
fn a_lake_passes_when_a_point_of_its_shore_is_within_150_m() {
    let lake = |id: &str, y: f64| Place {
        outline: vec![at(500.0, y), at(900.0, y), at(900.0, y + 400.0)],
        ..place(id, "lake", at(700.0, y + 200.0))
    };
    let features = features_with(vec![], vec![], vec![lake("w1", 140.0), lake("w2", 160.0)]);
    let routes = [route(1)];
    let shares = Samples::new(&routes, 47.0).shares();
    let Facts { entries, places, .. } = entries(&routes, &features, &shares, &mut |_| Ok(None)).unwrap();
    assert_eq!(entries[0]["places"], json!(["w1"]));
    assert_eq!(places["w1"], json!({"kind": "lake", "at_udeg": at(700.0, 340.0)}));
}

#[test]
fn each_file_holds_the_routes_of_its_cell_and_the_places_that_they_name() {
    let mut routes = vec![route(1), route(-2), route(3)];
    routes[1].cells = vec!["9-267-180".into(), "9-268-180".into()];
    routes[2].cells = vec!["9-268-180".into(), "9-999-999".into()];
    let entry = |places: &[&str]| json!({"places": places}).as_object().unwrap().clone();
    let entries = [entry(&["n1"]), entry(&["n2"]), Map::new()];
    let places: BTreeMap<String, Value> = [("n1", 1), ("n2", 2)].map(|(id, n)| (id.into(), json!(n))).into();
    let facts = Facts { entries: entries.into(), places, report: Report::default() };
    let files = files(&["9-267-180", "9-268-180", "9-268-181"], &routes, &facts);
    assert_eq!(
        files["9-267-180"],
        json!({"format": 1, "places": {"n1": 1, "n2": 2},
        "routes": {"1": {"places": ["n1"]}, "-2": {"places": ["n2"]}}})
    );
    assert_eq!(
        files["9-268-180"],
        json!({"format": 1, "places": {"n2": 2}, "routes": {"-2": {"places": ["n2"]}, "3": {}}})
    );
    assert_eq!(files["9-268-181"], json!({"format": 1, "places": {}, "routes": {}}));
}
