use std::collections::{HashMap, HashSet};

use obc_ground_plan::graph::OpenEnd;
use serde_json::Value;

use super::ladder::Kind;
use super::measure::{Ask, Site};
use super::*;
use crate::tiles::{decode, Geometry, Tiles, MAX_ZOOM};
use crate::{world, Plan};

/// The world point at metres east and north of a point at about 46.8° N.
fn at([e, n]: [f64; 2]) -> [f64; 2] {
    let o = world(8.4, 46.8);
    let m = metres(o[1]);
    [o[0] + e / m, o[1] - n / m]
}

/// Metres east and north of a world point.
fn back(p: [f64; 2]) -> [f64; 2] {
    let o = world(8.4, 46.8);
    let m = metres(o[1]);
    [(p[0] - o[0]) * m, (o[1] - p[1]) * m]
}

fn tags(pairs: &[(&str, &str)]) -> Tags {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn table(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn widths() -> WidthRules {
    let roads = crate::kinds::solver_rows(&table(include_str!("../../../../data/trailview/roads.json")));
    let finishes = crate::kinds::finish_rows(&table(include_str!("../../../../data/trailview/finishes.json")));
    crate::kinds::widths(&roads, &finishes).unwrap()
}

/// A way's part through points in metres at the heights z, with an alignment along its chords from 0 m.
fn part(points: &[[f64; 2]], z: &[f64]) -> Part {
    let pts: Vec<[f64; 2]> = points.iter().map(|&p| at(p)).collect();
    let mut along = 0.0;
    let plan = (0..pts.len())
        .map(|k| {
            let (a, b) = if k + 1 < pts.len() { (pts[k], pts[k + 1]) } else { (pts[k - 1], pts[k]) };
            if k > 0 {
                along += distance(pts[k - 1], pts[k]);
            }
            Plan { tangent: (b[1] - a[1]).atan2(b[0] - a[0]), along, ..Plan::default() }
        })
        .collect();
    Part { outer: false, points: pts, z: Some(z.to_vec()), plan: Some(plan) }
}

const ROAD: [(&str, &str); 2] = [("highway", "residential"), ("tunnel", "yes")];
const FOOT: [(&str, &str); 2] = [("highway", "footway"), ("tunnel", "yes")];

/// The ways of a test: (kind, tags, parts) by id; the tunnels are the ways of kind `tunnel`.
#[derive(Default)]
struct Case {
    ways: Vec<(&'static str, Tags, Vec<Part>)>,
    /// The deck depth of a bridge way, by its place in `ways`.
    decks: HashMap<usize, f64>,
    /// The bake's region (west, south, east, north), where not the default.
    bounds: Option<[f64; 4]>,
}

/// The bake's region of a test.
const BOUNDS: [f64; 4] = [8.3, 46.7, 8.5, 46.9];

impl Case {
    /// A tunnel east from e0 to e1 at n, its floor at 500 m; its id.
    fn tunnel(&mut self, t: &[(&str, &str)], (e0, e1): (f64, f64), n: f64) -> u32 {
        let pts = [[e0, n], [(e0 + e1) / 2.0, n], [e1, n]];
        self.ways.push(("tunnel", tags(t), vec![part(&pts, &[500.0; 3])]));
        self.ways.len() as u32 - 1
    }

    fn way(&mut self, pts: &[[f64; 2]]) {
        let z = vec![500.0; pts.len()];
        self.ways.push(("road", tags(&[("highway", "residential")]), vec![part(pts, &z)]));
    }

    /// The placement of the tunnels, each ask measured by `site`; the open ends that the measure gets go to `opened`.
    fn place(&self, site: &dyn Fn(&Ask) -> Site, opened: &mut Vec<OpenEnd>) -> Placed {
        let roads = crate::kinds::solver_rows(&table(include_str!("../../../../data/trailview/roads.json")));
        let (class_names, classes) = crate::kinds::classes(&roads).unwrap();
        let widths = widths();
        let rules =
            Ladder::parse(&crate::kinds::part_rows(&table(include_str!("../../../../data/trailview/parts.json"))))
                .unwrap();
        let tunnels: HashMap<u32, (Tags, Vec<Part>)> = (self.ways.iter().enumerate())
            .filter(|(_, w)| w.0 == "tunnel")
            .map(|(id, w)| (id as u32, (w.1.clone(), w.2.clone())))
            .collect();
        let (tiles, levels) = (Tiles::new(self.bounds.unwrap_or(BOUNDS)), HashMap::new());
        let tunnel_ways: HashSet<usize> = tunnels.keys().map(|&id| id as usize).collect();
        let input = Input {
            tunnels: &tunnels,
            ways: self.ways.iter().map(|(k, t, p)| (*k, t, p.as_slice())).collect(),
            decks: &self.decks,
            tunnel_ways: &tunnel_ways,
            tiles: &tiles,
            levels: &levels,
            terrain: &|_: f64, _: f64| 0.0,
            rules: &rules,
            widths: &widths,
            class_names: &class_names,
            classes: &classes,
        };
        let opened = std::cell::RefCell::new(opened);
        place_with(&input, &|asks, ends| {
            opened.borrow_mut().extend_from_slice(ends);
            (asks.iter().map(site).collect(), 0)
        })
    }
}

/// The sites of a ground at metres east and north, with the rooms of a way on top and a building over the line.
fn site<'a>(ground: &'a dyn Fn([f64; 2]) -> f64, rooms: &[f64], building: bool) -> impl Fn(&Ask) -> Site + 'a {
    let rooms = rooms.to_vec();
    move |a: &Ask| {
        let across = |j: i64| Some(ground(back(offset(a.p, left_of(a.dir), j as f64))));
        Site {
            stations: a.stations.iter().map(|st| st.map(|p| Some(ground(back(p))))).collect(),
            row: (-(a.right as i64)..=a.left as i64).map(across).collect(),
            decks: Vec::new(),
            rooms: rooms.clone(),
            building,
        }
    }
}

/// The kinds and the rung of each opening that way `id` enters.
fn of(placed: &Placed, id: u32) -> Vec<(Vec<Kind>, &'static str)> {
    placed.ends.iter().filter(|e| e.0.contains(&id)).map(|(_, e)| (e.kinds.clone(), e.rung)).collect()
}

fn records<'a>(placed: &'a Placed, kind: &str) -> Vec<&'a Tags> {
    placed.features.iter().filter(|f| f.0 == kind).map(|f| &f.1).collect()
}

/// A ridge over a tunnel `len` metres long east from 0 m: it rises `rise` per metre from each end.
fn ridge(len: f64, rise: f64) -> impl Fn([f64; 2]) -> f64 {
    move |p: [f64; 2]| 500.0 + rise * p[0].min(len - p[0]).max(0.0)
}

#[test]
fn each_rung_gives_its_kinds_in_the_order_of_the_rungs() {
    use Kind::*;
    let road = || {
        let mut case = Case::default();
        case.tunnel(&ROAD, (0.0, 200.0), 0.0);
        case
    };
    // A road tunnel 200 m long whose ridge rises 1 m per metre: the cover of 6 m (bore 5 m, shell and cover 0.5 m
    // each) comes 6 m in, within the road's collar of 25 m: an arch, then the free tube end.
    let placed = road().place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Arch, Free], "hill"); 2]);
    let ends = records(&placed, "tunnel-end");
    assert_eq!(ends.len(), 2);
    assert_eq!((ends[0]["collar"].as_str(), ends[0]["part"].as_str()), ("6", "arch"));
    // Covered 30 m in, past the collar: the free tube end; never covered: the same.
    let placed = road().place(&site(&ridge(200.0, 0.2), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Free], "free: covered past the limit"); 2]);
    let placed = road().place(&site(&|_| 500.0, &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Free], "free: never covered"); 2]);
    // An end that meets no way, covered at its vertex: the free tube end.
    let placed = road().place(&site(&|_| 520.0, &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Free], "free: a covered end that meets no way"); 2]);
    // A footway covered 2 m in (its cover is 3.6 m) opens in the rock first.
    let mut case = Case::default();
    case.tunnel(&FOOT, (0.0, 200.0), 0.0);
    let placed = case.place(&site(&ridge(200.0, 2.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Rock, Arch, Free], "hill"); 2]);
    assert!(
        records(&placed, "tunnel-end").iter().all(|t| t.contains_key("face")),
        "a rock end holds its wall's numbers"
    );
    // A footway 6 m long under a road on top at 503.5 m: an underpass at both ends. Its openings keep the floor's
    // height, so the floor of the tube cannot go down there: their section is as high as the road's underside (0.5 m
    // under it) less the shell allows, 2.5 m of the bore's 2.6 m. Under a building only: no part.
    let mut case = Case::default();
    case.tunnel(&FOOT, (0.0, 6.0), 0.0);
    case.ways.push((
        "road",
        tags(&[("highway", "residential")]),
        vec![part(&[[3.0, -50.0], [3.0, 50.0]], &[503.5; 2])],
    ));
    let placed = case.place(&site(&|_| 500.0, &[3.2], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Box, Free], "underpass"); 2]);
    let ends = records(&placed, "tunnel-end");
    assert_eq!((ends.len(), ends[0]["height"].as_str(), ends[0]["section"].as_str()), (2, "2.5", "box"));
    let placed = case.place(&site(&|_| 500.0, &[], true), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![], "under a building"); 2]);
    assert!(placed.features.is_empty() && placed.tags.is_empty());
    // Two tunnel ways end to end are one chain. A third way that ends at their node makes it a fork in the hill: no
    // opening there.
    let mut case = Case::default();
    let (a, b) = (case.tunnel(&ROAD, (0.0, 200.0), 0.0), case.tunnel(&ROAD, (200.0, 400.0), 0.0));
    let placed = case.place(&site(&ridge(400.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!((of(&placed, a), of(&placed, b)), (vec![(vec![Arch, Free], "hill")], vec![(vec![Arch, Free], "hill")]));
    assert_eq!((placed.metrics["tubes"].as_u64(), placed.tags[&a]["parts"].as_str()), (Some(1), "arch arch"));
    case.ways.push(("tunnel", tags(&ROAD), vec![part(&[[200.0, 0.0], [250.0, 50.0], [300.0, 100.0]], &[500.0; 3])]));
    let placed = case.place(&site(&ridge(400.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!((of(&placed, a).len(), of(&placed, b).len(), placed.metrics["fork_ends"].as_u64()), (1, 1, Some(3)));
    assert!(!placed.tags.contains_key(&a), "a chain that does not draw has no parts");
    // A vertex of unknown height: no part.
    let mut case = road();
    case.ways[0].2[0].z.as_mut().unwrap()[1] = f64::NAN;
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![], "a vertex of unknown height"); 2]);
}

#[test]
fn a_pair_has_one_frame_line_and_a_way_beside_a_wider_one_is_a_lane_of_its_room() {
    use Kind::*;
    // Two road tunnels 20 m apart under one ridge, which rises half as fast over the second; the second starts 3 m
    // farther in. A footway 2.5 m beside the first comes nearer it than a wall: the two are one room of two lanes.
    let mut case = Case::default();
    let a = case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let b = case.tunnel(&ROAD, (3.0, 200.0), 20.0);
    let foot = case.tunnel(&FOOT, (0.0, 200.0), 2.5);
    let (steep, gentle) = (ridge(200.0, 1.0), ridge(200.0, 0.5));
    let ground = |p: [f64; 2]| if p[1] < 10.0 { steep(p) } else { gentle(p) };
    let placed = case.place(&site(&ground, &[], false), &mut Vec::new());
    for id in [a, b, foot] {
        assert_eq!(of(&placed, id), vec![(vec![Arch, Free], "hill"); 2], "way {id}");
    }
    assert_eq!(placed.metrics["rooms"]["rooms"].as_u64(), Some(1));
    // The room's lanes from the right wall to the left: the footway runs at the road's side, with its way's width.
    let tubes = records(&placed, "tunnel-tube");
    let room = tubes.iter().find(|t| t["class"] == "residential footway").expect("the room");
    let half = |tags: &[(&str, &str)]| {
        widths().way("tunnel", &tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), false).half_m
    };
    assert_eq!(room["half"], num(half(&ROAD) + 0.6 + half(&FOOT)));
    // One head per pair, a hole per room from the right: in the west, the room's two lanes move 3 m in onto the line of
    // the second road, which lies about 20 m to the left; in the east, heading west, the second is the right hole.
    let ends = records(&placed, "tunnel-end");
    assert_eq!(ends.len(), 2);
    let west = ends.iter().find(|t| t["shift"] == "3 3 0").expect("the west pair");
    assert_eq!((west["part"].as_str(), west["kind"].as_str()), ("arch", "arch arch"));
    let east = ends.iter().find(|t| t["shift"] == "0 0 0").expect("the east pair");
    assert_eq!(west["across"], east["across"]);
    // Each room has its own collar: the hill over the second comes later, and the first moved 3 m in.
    assert_eq!((west["collar"].as_str(), east["collar"].as_str()), ("4 9", "12 7"));
    // A pair is built: the device opens the moved stretches, so the measure reads the four openings near them again.
    assert_eq!(placed.metrics["measured_again"].as_u64(), Some(4));
    for id in [a, foot] {
        assert_eq!(placed.tags[&id]["parts"], "arch arch");
    }
    // Where the nearer opening's way ends before the line of the other (a short first way), the two are no pair.
    let mut case = Case::default();
    let (a, _) = (case.tunnel(&ROAD, (0.0, 2.0), 0.0), case.tunnel(&ROAD, (2.0, 200.0), 0.0));
    let b = case.tunnel(&ROAD, (5.0, 200.0), 20.0);
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, a), [(vec![Arch, Free], "hill")]);
    assert_eq!(of(&placed, b)[0], (vec![Arch, Free], "hill"));
    assert_eq!(placed.metrics["pairs_that_do_not_reach"].as_u64(), Some(1));
    // A pair is one part of two chains: where the second chain forks in the hill, so that its end there has no part,
    // the two are no pair.
    let mut case = Case::default();
    let a = case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let b = case.tunnel(&ROAD, (3.0, 200.0), 20.0);
    for n in [60.0, -20.0] {
        case.ways.push(("tunnel", tags(&ROAD), vec![part(&[[200.0, 20.0], [210.0, n]], &[500.0; 2])]));
    }
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, a)[0], (vec![Arch, Free], "hill"));
    assert_eq!(of(&placed, b)[0], (vec![Arch, Free], "hill"));
    assert!(!placed.tags.contains_key(&b), "a chain that does not draw has no parts");
    // Two footways 3 m apart beside a road tube that starts 3 m farther in: the first comes nearer the road than a wall
    // and is a lane of its room, which opens on the road's line; the second pairs with the room and moves onto it too.
    let mut case = Case::default();
    case.tunnel(&ROAD, (3.0, 200.0), 0.0);
    let f1 = case.tunnel(&FOOT, (0.0, 200.0), 4.5);
    case.tunnel(&FOOT, (0.0, 200.0), 7.5);
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, f1)[0].0[0], Arch);
    let west = records(&placed, "tunnel-end").into_iter().find(|t| t["heading"] == "0").expect("the west head");
    assert_eq!((west["shift"].as_str(), west["half"].split(' ').count()), ("0 3 3", 2));
}

#[test]
fn a_moved_opening_names_the_end_vertex_of_its_way_and_the_metres_to_it() {
    // A road tunnel east from a junction of two residential ways (outer half-width 4 m), one from the west and one
    // from the north. Its frame line (3.6 m each side) clears the stroke of the northern way 5 m in, the first whole
    // metre past 4 m.
    let mut case = Case::default();
    let tunnel = case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    case.way(&[[-50.0, 0.0], [0.0, 0.0]]);
    case.way(&[[0.0, 50.0], [0.0, 0.0]]);
    let mut opened = Vec::new();
    let placed = case.place(&site(&|_| 500.0, &[], false), &mut opened);
    let ends = records(&placed, "tunnel-end");
    let moved = ends.iter().find(|t| t["shift"] != "0").expect("the moved opening");
    let vertex = frame_point(&case.ways[tunnel as usize].2, 0, 0);
    assert_eq!((moved["way"].clone(), moved["shift"].as_str()), (format!("{} {}", vertex[0], vertex[1]), "5"));
    // The measure builds the ground with that stretch open.
    let way = obc_ground_plan::frame::GlobalPoint { x: vertex[0], y: vertex[1] };
    assert_eq!(opened, [OpenEnd { way, shift_m: 5.0 }]);
    // The tube starts at the opening.
    let first = placed.features.iter().find(|f| f.0 == "tunnel-tube").unwrap();
    assert!((back(first.2.points[0])[0] - 5.0).abs() < 1e-6);
    // A through road with a vertex at the tunnel's end (a T) is two arms: the opening moves the same.
    let mut tee = Case::default();
    tee.tunnel(&ROAD, (0.0, 200.0), 0.0);
    tee.way(&[[0.0, -50.0], [0.0, 0.0], [0.0, 50.0]]);
    let placed = tee.place(&site(&|_| 500.0, &[], false), &mut Vec::new());
    assert!(records(&placed, "tunnel-end").iter().any(|t| t["shift"] == "5"));
    assert_eq!(placed.metrics["t_ends"].as_u64(), Some(1));
    // A tunnel 6 m long between two through roads: its openings would meet, so both stay at its ends.
    let mut short = Case::default();
    let id = short.tunnel(&ROAD, (0.0, 6.0), 0.0);
    short.way(&[[0.0, -50.0], [0.0, 0.0], [0.0, 50.0]]);
    short.way(&[[6.0, -50.0], [6.0, 0.0], [6.0, 50.0]]);
    let placed = short.place(&site(&|_| 500.0, &[], false), &mut Vec::new());
    assert_eq!(of(&placed, id), vec![(vec![Kind::Free], "free: never covered"); 2]);
    assert!(records(&placed, "tunnel-end").iter().all(|t| t["shift"] == "0"));
    assert_eq!(placed.metrics["chains_whose_openings_would_meet"].as_u64(), Some(1));
}

#[test]
fn a_tube_record_keeps_its_two_vertices_and_its_tags_after_a_tile_cut() {
    let mut case = Case::default();
    case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    let tubes: Vec<&(&str, Tags, Part)> = placed.features.iter().filter(|f| f.0 == "tunnel-tube").collect();
    // Each chord of the curve is a record of two vertices in the rising metres of its way.
    assert!(tubes.len() > 2 && tubes.iter().all(|t| t.2.points.len() == 2 && t.2.points[1][0] > t.2.points[0][0]));
    assert_eq!(tubes[0].1["ends"], "arch arch");
    // A record over the line between two tiles is whole in both, with its tags.
    let n = f64::from(1u32 << MAX_ZOOM);
    let (column, row) = (4286.0, 2889.0);
    let points = vec![[(column + 0.999) / n, (row + 0.5) / n], [(column + 1.001) / n, (row + 0.5) / n]];
    let record = Part { points, ..tubes[0].2.clone() };
    let mut tiles = Tiles::new([8.3, 46.7, 8.5, 46.9]);
    assert!(tiles.add(MAX_ZOOM, "tunnel-tube", Geometry::Line, &tubes[0].1, std::slice::from_ref(&record), None));
    for c in [column as u32, column as u32 + 1] {
        let read = decode(&tiles.tiles[&(MAX_ZOOM, c, row as u32)].bytes()).unwrap();
        assert_eq!((read.len(), read[0].parts.len(), read[0].parts[0].xy.len()), (1, 1, 2), "tile {c}");
        assert_eq!(&read[0].tags, &tubes[0].1, "tile {c}");
    }
}

#[test]
fn every_road_class_has_one_group() {
    let rules = Ladder::parse(&crate::kinds::part_rows(&table(include_str!("../../../../data/trailview/parts.json"))));
    let rules = rules.unwrap();
    let roads = table(include_str!("../../../../data/trailview/roads.json"));
    for class in roads.as_object().unwrap().keys() {
        assert!(rules.group(class).is_some(), "{class}");
    }
}

/// The tube records in metres east, west first: the east of both vertices, their floors and the record's height.
fn chords(placed: &Placed) -> Vec<([f64; 2], [f64; 2], f64)> {
    let mut out: Vec<([f64; 2], [f64; 2], f64)> = (placed.features.iter())
        .filter(|f| f.0 == "tunnel-tube")
        .map(|f| {
            let (z, east) = (f.2.z.as_ref().unwrap(), |k: usize| back(f.2.points[k])[0]);
            ([east(0), east(1)], [z[0], z[1]], f.1["height"].parse().unwrap())
        })
        .collect();
    out.sort_by(|x, y| x.0[0].total_cmp(&y.0[0]));
    out
}

/// A road tunnel 200 m long east from 0 m, its floor at 500 m, and a residential way over it through `points` (metres
/// east and north) at the height `top`; with a deck of `deck` metres where it is a bridge.
fn under(points: &[[f64; 2]], top: f64, deck: Option<f64>) -> Placed {
    let mut case = Case::default();
    case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let road = tags(&[("highway", "residential")]);
    case.ways.push(("road", road, vec![part(points, &vec![top; points.len()])]));
    if let Some(deck) = deck {
        case.decks.insert(1, deck);
    }
    case.place(&site(&|_| 500.0, &[], false), &mut Vec::new())
}

/// The floor of the tube records at metres east.
fn floor_at(chords: &[([f64; 2], [f64; 2], f64)], e: f64) -> f64 {
    let c = chords.iter().find(|c| c.0[0] <= e && e <= c.0[1]).unwrap();
    c.1[0] + (e - c.0[0]) / (c.0[1] - c.0[0]) * (c.1[1] - c.1[0])
}

#[test]
fn the_floor_keeps_the_crown_a_slab_under_a_way_on_top_within_the_grade_and_the_openings_stay() {
    // A road across the tube at 504 m: its underside lies a slab (0.5 m) lower, so the floor under it is 2 m lower, at
    // most 498 m (bore 5 m, shell 0.5 m). Far from the openings the grade of the class (0.2) allows that.
    let placed = under(&[[100.0, -50.0], [100.0, 50.0]], 504.0, None);
    let chords = chords(&placed);
    assert!((floor_at(&chords, 100.0) - 498.0).abs() < 1e-9, "{}", floor_at(&chords, 100.0));
    // The stretch under it is the tube within the road's outer half-width, the tube's half and its shell.
    let reach = widths().way("road", &tags(&[("highway", "residential")]), false).outer_m()
        + widths().way("tunnel", &tags(&ROAD), false).half_m
        + 0.6
        + 0.5;
    for e in [100.0 - reach, 100.0 + reach] {
        assert!((floor_at(&chords, e) - 498.0).abs() < 1e-6, "the floor at {e} m: {}", floor_at(&chords, e));
    }
    assert!(floor_at(&chords, 100.0 - reach - 1.0) > 498.0 && floor_at(&chords, 100.0 + reach + 1.0) > 498.0);
    assert!(chords.iter().all(|c| (c.1[1] - c.1[0]).abs() <= 0.2 * (c.0[1] - c.0[0]) + 1e-9), "{chords:?}");
    assert!(chords.iter().all(|c| c.2 == 5.0));
    let (first, last) = (chords[0], chords[chords.len() - 1]);
    assert_eq!((first.0[0], first.1[0], last.1[1]), (0.0, 500.0, 500.0));
    assert!(placed.features.iter().filter(|f| f.0 == "tunnel-end").all(|f| f.2.z.as_ref().unwrap()[0] == 500.0));
    assert_eq!(placed.metrics["tubes_lowered"].as_u64(), Some(1));
    assert_eq!(placed.metrics["floor_clashes"].as_u64(), Some(0));
    // A road along the top, 2 m beside the axis from 60 m to 140 m, holds the floor down along it.
    let chords = self::chords(&under(&[[60.0, 2.0], [100.0, 2.0], [140.0, 2.0]], 504.0, None));
    assert!([60.0, 100.0, 140.0].iter().all(|&e| (floor_at(&chords, e) - 498.0).abs() < 1e-6));
    // A road within the at-grade band (0.5 m) over the floor crosses at grade: the floor does not change.
    let placed = under(&[[100.0, -50.0], [100.0, 50.0]], 500.5, None);
    assert!(self::chords(&placed).iter().all(|c| c.1 == [500.0, 500.0]));
    assert_eq!(placed.metrics["tops"].as_u64(), Some(0));
    // A tube that falls into the hill from a junction of two roads: its opening moves 5 m in. The road that goes on
    // from the junction climbs; near the opening it lies 0.75 m over the tube there, but its nearest point is the
    // chain's end vertex, at the chain's height: it lies at grade.
    let mut case = Case::default();
    let falls = part(&[[0.0, 0.0], [5.0, 0.0], [100.0, 0.0], [200.0, 0.0]], &[515.0, 514.25, 500.0, 485.0]);
    case.ways.push(("tunnel", tags(&ROAD), vec![falls]));
    let residential = tags(&[("highway", "residential")]);
    case.ways.push(("road", residential.clone(), vec![part(&[[-50.0, 0.0], [0.0, 0.0]], &[520.0, 515.0])]));
    case.ways.push(("road", residential, vec![part(&[[0.0, 50.0], [0.0, 0.0]], &[515.0, 515.0])]));
    let placed = case.place(&site(&|_| 400.0, &[], false), &mut Vec::new());
    assert!(records(&placed, "tunnel-end").iter().any(|t| t["shift"] == "5"));
    assert_eq!((placed.metrics["tops"].as_u64(), placed.metrics["tubes_lowered"].as_u64()), (Some(0), Some(0)));
    // A bridge at 507 m with a deck 2 m deep: its underside is 505 m, so the floor goes to 499.5 m. On the ground, its
    // underside would be a slab under it, and the floor would stay.
    assert!(
        (floor_at(&self::chords(&under(&[[100.0, -50.0], [100.0, 50.0]], 507.0, Some(2.0))), 100.0) - 499.5).abs()
            < 1e-9
    );
    assert_eq!(floor_at(&self::chords(&under(&[[100.0, -50.0], [100.0, 50.0]], 507.0, None)), 100.0), 500.0);
}

#[test]
fn a_way_whose_short_reach_lies_next_to_a_curve_point_holds_the_floor_down() {
    // A road ends beside the tube: the tube is within its reach only from 100.65 m to 100.95 m east. Both ends of that
    // reach lie within `vertex_gap_m` of the curve point at 100 m, so neither is a vertex; that point takes the cap.
    let reach = widths().way("road", &tags(&[("highway", "residential")]), false).outer_m()
        + widths().way("tunnel", &tags(&ROAD), false).half_m
        + 0.6
        + 0.5;
    let beside = (reach * reach - 0.15 * 0.15).sqrt();
    let placed = under(&[[100.8, 50.0], [100.8, beside]], 504.0, None);
    let chords = chords(&placed);
    assert!(chords.iter().all(|c| c.0[0] != 100.65 && c.0[1] != 100.95), "{chords:?}");
    assert_eq!(placed.metrics["tops"].as_u64(), Some(1));
    assert!((floor_at(&chords, 100.0) - 498.0).abs() < 1e-6, "{}", floor_at(&chords, 100.0));
}

#[test]
fn a_way_half_a_metre_beside_the_reach_cuts_nothing() {
    // A road runs beside the tube 0.5 m outside its reach from 60 m to 140 m east, then crosses it near 141 m. Only the
    // crossing holds the floor down, from about 134 m, with the grade on to 118 m; beside it, the floor stays.
    let reach = widths().way("road", &tags(&[("highway", "residential")]), false).outer_m()
        + widths().way("tunnel", &tags(&ROAD), false).half_m
        + 0.6
        + 0.5;
    let placed = under(&[[60.0, reach + 0.5], [140.0, reach + 0.5], [150.0, -50.0]], 504.0, None);
    let chords = chords(&placed);
    assert!((floor_at(&chords, 141.4) - 498.0).abs() < 0.5, "{}", floor_at(&chords, 141.4));
    assert!([60.0, 80.0, 100.0, 110.0].iter().all(|&e| floor_at(&chords, e) == 500.0), "{chords:?}");
    assert!(chords.iter().filter(|c| c.0[1] <= 118.0).all(|c| c.2 == 5.0));
}

#[test]
fn a_way_on_top_near_an_opening_makes_the_section_lower_down_to_the_least_height() {
    // 3 m from the opening, the floor cannot go down in time: the section there is lower. A road on top at 505.5 m
    // leaves 4.5 m over the floor of the opening (its underside, less the shell); the opening takes that height.
    let near = [[3.0, -50.0], [3.0, 50.0]];
    let placed = under(&near, 505.5, None);
    let low = chords(&placed);
    assert_eq!((low[0].1[0], low[0].2), (500.0, 4.5));
    assert!(low.iter().all(|c| c.2 >= 4.5));
    let mut heights: Vec<&str> = records(&placed, "tunnel-end").iter().map(|t| t["height"].as_str()).collect();
    heights.sort_unstable();
    assert_eq!(heights, ["4.5", "5"], "the opening near the road takes the height of its first chord");
    assert_eq!(placed.metrics["low_stretches"].as_u64(), Some(1));
    assert_eq!(placed.metrics["floor_clashes"].as_u64(), Some(0));
    // At 504 m the section reaches the least height of a road (4 m), and the crown's shell still cuts the road on
    // top: a clash.
    let placed = under(&near, 504.0, None);
    assert_eq!(chords(&placed)[0].2, 4.0);
    assert_eq!(placed.metrics["low_stretches_at_least_height"].as_u64(), Some(1));
    assert_eq!(placed.metrics["floor_clashes"].as_u64(), Some(1));
}

/// The pairs of tube records of two rooms that come nearer than a wall at one level, as a tile holds them: each point
/// rounded to the tile's grid (8 world units of 2^30), which with the decimals of the heights is the only allowance. A
/// point reads another chord where its foot lies on it; two chords that end on one seam's line meet there.
fn records_that_touch(placed: &Placed) -> Vec<String> {
    const UNITS: f64 = (1u64 << 27) as f64;
    let unit = |p: [f64; 2]| [(p[0] * UNITS).round() / UNITS, (p[1] * UNITS).round() / UNITS];
    // Per record its two points, their floors, its half-width and its height.
    type Record = ([f64; 2], [f64; 2], [f64; 2], f64, f64);
    let tubes: Vec<Record> = (placed.features.iter())
        .filter(|f| f.0 == "tunnel-tube")
        .map(|f| {
            let z = f.2.z.as_ref().unwrap();
            // The higher of a chord's heights at its two vertices.
            let num = |k: &str| f.1[k].split(' ').map(|v| v.parse::<f64>().unwrap()).fold(0.0, f64::max);
            (unit(f.2.points[0]), unit(f.2.points[1]), [z[0], z[1]], num("half"), num("height"))
        })
        .collect();
    // The rooms: the chords that share a vertex.
    let mut room: Vec<usize> = (0..tubes.len()).collect();
    for i in 0..tubes.len() {
        for k in 0..i {
            let shared = [tubes[i].0, tubes[i].1].iter().any(|p| *p == tubes[k].0 || *p == tubes[k].1);
            if shared {
                let (ri, rk) = (room[i], room[k]);
                room.iter_mut().filter(|r| **r == ri.max(rk)).for_each(|r| *r = ri.min(rk));
            }
        }
    }
    let allowance = distance(unit([0.5, 0.5]), unit([0.5 + 1.0 / UNITS, 0.5 + 1.0 / UNITS]));
    // The seams' lines: a point and the unit heading.
    let seams: Vec<([f64; 2], [f64; 2])> = (placed.features.iter())
        .filter(|f| f.0 == "tunnel-end" && f.1["part"] == "seam")
        .map(|f| {
            let h: f64 = f.1["heading"].parse().unwrap();
            (unit(f.2.points[0]), [h.cos(), h.sin()])
        })
        .collect();
    let on_seam = |t: &Record| {
        (seams.iter().enumerate())
            .filter(|(_, (p, d))| [t.0, t.1].iter().any(|q| super::along_dir(*p, *q, *d).abs() < 2.0 * allowance))
            .map(|(s, _)| s)
            .collect::<Vec<usize>>()
    };
    let mut out = Vec::new();
    for (i, a) in tubes.iter().enumerate() {
        let len = distance(a.0, a.1);
        let mine = on_seam(a);
        for k in
            (0..tubes.len()).filter(|&k| room[k] != room[i] && !on_seam(&tubes[k]).iter().any(|s| mine.contains(s)))
        {
            let b = &tubes[k];
            for m in 0..=len.ceil() as usize {
                let u = (m as f64 / len.max(1.0)).min(1.0);
                let p = [a.0[0] + u * (a.1[0] - a.0[0]), a.0[1] + u * (a.1[1] - a.0[1])];
                let za = a.2[0] + u * (a.2[1] - a.2[0]);
                let d = [b.1[0] - b.0[0], b.1[1] - b.0[1]];
                let v = ((p[0] - b.0[0]) * d[0] + (p[1] - b.0[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]);
                if !(0.0..=1.0).contains(&v) {
                    continue;
                }
                let foot = [b.0[0] + v * d[0], b.0[1] + v * d[1]];
                let (gap, rise) = (distance(p, foot) - a.3 - b.3, b.2[0] + v * (b.2[1] - b.2[0]) - za);
                let clear = gap >= 1.0 - 0.01 - allowance || rise >= a.4 + 1.0 - 0.01 || -rise >= b.4 + 1.0 - 0.01;
                if !clear {
                    out.push(format!("{:?} gap {gap:.2} rise {rise:.2}", back(p)));
                    break;
                }
            }
        }
    }
    out
}

/// The layouts of the rooms' tests: road tunnels 200 m east, a first on the line 0 m north at the floor 500 m, and a
/// second as the layout says; the count of rooms of lanes and of seams.
fn layouts() -> Vec<(&'static str, Case, u64, u64)> {
    let pair = |north: f64, start: f64, z: f64| {
        let mut case = Case::default();
        case.tunnel(&ROAD, (0.0, 200.0), 0.0);
        let line = [[start, north], [(start + 200.0) / 2.0, north], [200.0, north]];
        case.ways.push(("tunnel", tags(&ROAD), vec![part(&line, &[z; 3])]));
        case
    };
    let mut footway = Case::default();
    footway.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let rise = 200.0 * 3f64.to_radians().tan();
    footway.ways.push(("tunnel", tags(&FOOT), vec![part(&[[0.0, 4.3], [200.0, 4.3 + rise]], &[500.0; 2])]));
    // A road tunnel 3 m over the first, crossing it at 40 degrees from 30 m east to 170 m east.
    let mut cross = Case::default();
    cross.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let t = 40f64.to_radians().tan();
    cross.ways.push((
        "tunnel",
        tags(&ROAD),
        vec![part(&[[30.0, -70.0 * t], [100.0, 0.0], [170.0, 70.0 * t]], &[503.0; 3])],
    ));
    let mut away = Case::default();
    away.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let line = [[0.0, 7.0], [60.0, 7.0], [120.0, 40.0], [200.0, 40.0]];
    away.ways.push(("tunnel", tags(&ROAD), vec![part(&line, &[500.0; 4])]));
    // Twins 7 m apart for 60 m that part at 5 degrees.
    let mut part5 = Case::default();
    part5.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let t = 5f64.to_radians().tan();
    let line = [[0.0, 7.0], [60.0, 7.0], [130.0, 7.0 + 70.0 * t], [200.0, 7.0 + 140.0 * t]];
    part5.ways.push(("tunnel", tags(&ROAD), vec![part(&line, &[500.0; 4])]));
    let (rail, _) = rail_over_rail();
    vec![
        ("twins that overlap", pair(5.0, 0.0, 500.0), 1, 0),
        ("floors 3 m apart", pair(5.0, 0.0, 503.0), 1, 0),
        ("floors 6 m apart", pair(5.0, 0.0, 506.0), 0, 0),
        ("one 7 m under the other", pair(5.0, 0.0, 493.0), 0, 0),
        ("a staggered start of 3 m", pair(5.0, 3.0, 500.0), 1, 0),
        ("a staggered start of 60 m", pair(5.0, 60.0, 500.0), 1, 0),
        ("a footway at 3 degrees beside a road", footway, 1, 1),
        ("one that turns away", away, 1, 2),
        ("twins that part at 5 degrees", part5, 1, 1),
        ("a footway that turns away", path_turns().0, 1, 2),
        ("one track 12 m over the other", rail, 2, 2),
        ("two that cross", cross, 0, 0),
    ]
}

const RAIL: [(&str, &str); 2] = [("railway", "rail"), ("tunnel", "yes")];

/// A road tunnel 200 m east and a footway at the numbers of stagger-path (46.92647, 8.58401): 4.7 m beside the road's
/// axis at the openings, 3.8 m 29 m in, then turned 40 and 60 degrees away to its own opening; and the footway's id.
fn path_turns() -> (Case, u32) {
    let mut case = Case::default();
    case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let (c40, s40, c60, s60) = (40f64.to_radians().cos(), 40f64.to_radians().sin(), 0.5, 60f64.to_radians().sin());
    let turn = [32.0 + 5.0 * c40, 3.8 + 5.0 * s40];
    let line = [
        [0.0, 4.7],
        [29.0, 3.8],
        turn,
        [turn[0] + 20.0 * c60, turn[1] + 20.0 * s60],
        [turn[0] + 40.0 * c60, turn[1] + 40.0 * s60],
    ];
    let line = [line[0], [32.0, 3.8], line[2], line[3], line[4]];
    case.ways.push(("tunnel", tags(&FOOT), vec![part(&line, &[500.0; 5])]));
    (case, 1)
}

/// Two rail tunnels 200 m east, their axes 4 m apart, the second level with the first at its openings and 12 m over it
/// in the middle; and the second's id.
fn rail_over_rail() -> (Case, u32) {
    let mut case = Case::default();
    case.tunnel(&RAIL, (0.0, 200.0), 0.0);
    let line = [[0.0, 4.0], [50.0, 4.0], [100.0, 4.0], [150.0, 4.0], [200.0, 4.0]];
    case.ways.push(("tunnel", tags(&RAIL), vec![part(&line, &[500.0, 506.0, 512.0, 506.0, 500.0])]));
    (case, 1)
}

#[test]
fn a_lane_that_turns_away_ends_at_a_door() {
    let hill = ridge(200.0, 1.0);
    let placed = path_turns().0.place(&site(&hill, &[], false), &mut Vec::new());
    let m = &placed.metrics;
    let rooms = &m["rooms"];
    assert_eq!(
        (rooms["rooms"].as_u64(), rooms["caps"].as_u64(), rooms["doors"].as_u64()),
        (Some(1), Some(1), Some(1)),
        "{m}"
    );
    assert!(rooms["lanes_in_a_wall_m"].as_f64().unwrap() < 5.0, "{m}");
    assert_eq!(m["rooms_that_touch"].as_u64(), Some(0), "{}", m["rooms_that_touch_places"]);
    let half = |tags: &[(&str, &str)]| {
        widths().way("tunnel", &tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), false).half_m
    };
    // The room is as wide as its two lanes: the road and the footway at its side with its way's width, on a line within
    // `pair_max_deg` of it.
    let most = half(&ROAD) + 0.6 + half(&FOOT) / 25f64.to_radians().cos() + 0.01;
    for t in records(&placed, "tunnel-tube").into_iter().chain(records(&placed, "tunnel-end")) {
        let widest = t["half"].split(' ').map(|v| v.parse::<f64>().unwrap()).fold(0.0, f64::max);
        assert!(widest <= most, "a record {widest} m wide: {t:?}");
    }
    // The seam at the turn has the road's hole and the footway's door; the footway's own room starts with a cap.
    let seams: Vec<&Tags> = records(&placed, "tunnel-end").into_iter().filter(|t| t["part"] == "seam").collect();
    let turn = seams.iter().find(|t| t.contains_key("door")).expect("the seam with the door");
    assert_eq!((turn["across"].split(' ').count(), turn["door"].split(' ').count()), (2, 4));
    assert!(seams.iter().any(|t| !t["across"].contains(' ')), "the cap: {seams:?}");
}

#[test]
fn a_track_that_climbs_over_the_other_is_its_own_tunnel() {
    let hill = ridge(200.0, 1.0);
    let placed = rail_over_rail().0.place(&site(&hill, &[], false), &mut Vec::new());
    let m = &placed.metrics;
    assert_eq!((m["rooms"]["rooms"].as_u64(), m["rooms"]["seams"].as_u64()), (Some(2), Some(2)), "{m}");
    assert_eq!(m["rooms_that_touch"].as_u64(), Some(0), "{}", m["rooms_that_touch_places"]);
    // No room has a step past the rail's least height and a roof, and the rise of 10 m of floor (0.12 m per metre).
    let steps = m["largest_step_places"].as_str().unwrap();
    let most = steps.split("; ").filter_map(|p| p.split(' ').nth(1)?.parse::<f64>().ok()).fold(0.0, f64::max);
    assert!(most > 3.0 && most < 4.8 + 1.0 + 1.2, "steps {steps}");
}

#[test]
fn rooms_keep_apart() {
    let hill = ridge(200.0, 1.0);
    let ground = site(&hill, &[], false);
    for (name, case, rooms, seams) in layouts() {
        let placed = case.place(&ground, &mut Vec::new());
        let m = &placed.metrics;
        assert_eq!(
            (m["rooms"]["rooms"].as_u64(), m["rooms"]["seams"].as_u64()),
            (Some(rooms), Some(seams)),
            "{name}: {m}"
        );
        assert_eq!(m["rooms_that_touch"].as_u64(), Some(0), "{name}: {}", m["rooms_that_touch_places"]);
        let touch = records_that_touch(&placed);
        assert!(touch.is_empty(), "{name}: the records come nearer than a wall at {touch:?}");
    }
}

#[test]
fn a_seams_record_is_whole() {
    let hill = ridge(200.0, 1.0);
    let ground = site(&hill, &[], false);
    let number = |t: &Tags, k: &str| t[k].split(' ').map(|v| v.parse::<f64>().unwrap()).collect::<Vec<f64>>();
    let tile = distance(world(8.4, 46.8), [world(8.4, 46.8)[0] + 8.0 / (1u64 << 30) as f64, world(8.4, 46.8)[1]]);
    for (name, case, _, seams) in layouts() {
        let placed = case.place(&ground, &mut Vec::new());
        let ends = records(&placed, "tunnel-end");
        assert_eq!(ends.iter().filter(|t| t["part"] == "seam").count() as u64, seams, "{name}: {}", placed.metrics);
        let tubes: Vec<&(&str, Tags, Part)> = placed.features.iter().filter(|f| f.0 == "tunnel-tube").collect();
        // Per lane of a chord at a vertex: its axis and its floor.
        let lanes_at = |f: &(&str, Tags, Part), k: usize| -> Vec<([f64; 2], f64)> {
            let (a, b) = (f.2.points[0], f.2.points[1]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l = d[0].hypot(d[1]);
            let left = left_of([d[0] / l, d[1] / l]);
            let pair = |tag: &str| {
                f.1.contains_key(tag).then(|| number(&f.1, tag).into_iter().skip(k).step_by(2).collect::<Vec<f64>>())
            };
            let n = f.1["class"].split(' ').count();
            let (off, up) = (pair("off").unwrap_or(vec![0.0; n]), pair("up").unwrap_or(vec![0.0; n]));
            (0..n).map(|i| (offset(f.2.points[k], left, off[i]), f.2.z.as_ref().unwrap()[k] + up[i])).collect()
        };
        for seam in ends.iter().filter(|t| t["part"] == "seam") {
            let (across, half) = (number(seam, "across"), number(seam, "half"));
            // Each hole lies in the room before the line, a wall from the next.
            for k in 1..across.len() {
                assert!(
                    across[k].abs() + half[k] <= half[0] + 1e-9,
                    "{name}: hole {k} reaches past the room before: {seam:?}"
                );
            }
            for k in 1..across.len() - 1 {
                assert!(
                    across[k + 1] - half[k + 1] - across[k] - half[k] >= 1.0 - 1e-9,
                    "{name}: holes {k} and {} less than a wall apart: {seam:?}",
                    k + 1
                );
            }
            // Each hole's point is the first vertex of its room, within the tile's rounding; the lanes at the line have
            // one floor on both sides, to the centimetre.
            let f = placed.features.iter().find(|f| f.0 == "tunnel-end" && std::ptr::eq(&f.1, *seam)).unwrap();
            let h: f64 = seam["heading"].parse().unwrap();
            let lanes: Vec<([f64; 2], f64)> = tubes
                .iter()
                .flat_map(|t| [0, 1].map(|k| (t, k)))
                .filter(|(t, k)| distance(t.2.points[*k], f.2.points[0]) < 60.0)
                .flat_map(|(t, k)| lanes_at(t, k))
                .collect();
            for (k, &a) in across.iter().enumerate() {
                let p = offset(f.2.points[0], left_of([h.cos(), h.sin()]), a);
                let vertex =
                    tubes.iter().flat_map(|t| t.2.points.iter()).map(|&q| distance(p, q)).fold(f64::INFINITY, f64::min);
                assert!(vertex <= tile, "{name}: hole {k} lies {vertex:.3} m from the first vertex of its room");
            }
            for (i, x) in lanes.iter().enumerate() {
                for y in lanes.iter().skip(i + 1).filter(|y| distance(x.0, y.0) < 0.05) {
                    assert!((x.1 - y.1).abs() <= 0.02, "{name}: a lane's floor {} and {} at the seam", x.1, y.1);
                }
            }
        }
        // The two chords of a room at a vertex write the same strings there; no chord is shorter than the vertex gap,
        // but for the rounding of a tile.
        let at = |f: &(&str, Tags, Part), k: usize| {
            let off =
                f.1.get("off").map(|o| o.split(' ').skip(k).step_by(2).map(String::from).collect::<Vec<String>>());
            (f.2.points[k], off)
        };
        for (i, a) in tubes.iter().enumerate() {
            assert!(
                distance(a.2.points[0], a.2.points[1]) >= 1.0 - 0.3,
                "{name}: a chord of {} m",
                distance(a.2.points[0], a.2.points[1])
            );
            for c in tubes.iter().skip(i + 1) {
                for (ka, kc) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                    let ((pa, oa), (pc, oc)) = (at(a, ka), at(c, kc));
                    if pa == pc && oa.is_some() && oc.is_some() && a.1["class"] == c.1["class"] {
                        // Chords that meet against each other's way see the lanes the other way round.
                        let flip: Vec<String> = oc
                            .clone()
                            .unwrap()
                            .iter()
                            .rev()
                            .map(|v| crate::parts::num(-v.parse::<f64>().unwrap()))
                            .collect();
                        let same = oa.clone().unwrap().iter().map(|v| v.to_string()).collect::<Vec<_>>();
                        assert!(oa == oc || same == flip, "{name}: two chords at a vertex: {oa:?} and {oc:?}");
                    }
                }
            }
        }
    }
}

/// The records that the builder does not find whole as the tiles hold them: per tile of the zoom-13 tiles of `bounds`,
/// each record whose point it owns that the builder does not find whole, and each chord whose `a` it owns that does
/// not close.
fn not_whole_in_tiles(placed: &Placed, bounds: [f64; 4]) -> Vec<String> {
    use obc_trailview_parts::{read_back, Chord, End, Rules};
    let mut tiles = Tiles::new(bounds);
    for (kind, tags, part) in &placed.features {
        let geometry = if part.points.len() == 1 { Geometry::Point } else { Geometry::Line };
        tiles.add(MAX_ZOOM, kind, geometry, tags, std::slice::from_ref(part), None);
    }
    let (rules, side, unit) = (Rules::embedded(), f64::from(crate::tiles::EXTENT), 8.0);
    let mut out = Vec::new();
    for (&(_, col, row), tile) in &tiles.tiles {
        let (x0, y0) = (f64::from(col) * side, f64::from(row) * side);
        let world = |xy: [i64; 2]| [(x0 + xy[0] as f64) * unit, (y0 + xy[1] as f64) * unit];
        let owns = |p: [f64; 2]| {
            let q = [p[0] / unit - x0, p[1] / unit - y0];
            (0.0..side).contains(&q[0]) && (0.0..side).contains(&q[1])
        };
        let (mut ends, mut chords) = (Vec::new(), Vec::new());
        for f in decode(&tile.bytes()).unwrap() {
            let (xy, z) = (&f.parts[0].xy, f.parts[0].z.clone().unwrap());
            if f.kind == "tunnel-end" {
                ends.push(End::from_tags(world(xy[0]), z[0], &f.tags, rules).unwrap());
            } else if f.kind == "tunnel-tube" {
                let (a, b) = (world(xy[0]), world(xy[1]));
                chords.push(Chord::from_tags([a[0], a[1], z[0]], [b[0], b[1], z[1]], &f.tags, rules).unwrap());
            }
        }
        let (whole, closed) = read_back(&ends, &chords, rules);
        let place = |p: [f64; 2]| back([p[0] / (1u64 << 30) as f64, p[1] / (1u64 << 30) as f64]);
        for (e, ok) in ends.iter().zip(&whole).filter(|(e, ok)| owns(e.at) && !**ok) {
            out.push(format!("tile {col} {row}: record {} at {:?} ({ok})", e.part.name(), place(e.at)));
        }
        for (c, _) in chords.iter().zip(&closed).filter(|(c, ok)| owns([c.a[0], c.a[1]]) && !**ok) {
            out.push(format!("tile {col} {row}: chord at {:?} open", place([c.a[0], c.a[1]])));
        }
    }
    out
}

/// The case moved `east` metres east.
fn moved(mut case: Case, east: f64) -> Case {
    for (_, _, parts) in &mut case.ways {
        for p in parts.iter_mut().flat_map(|part| part.points.iter_mut()) {
            p[0] += east / metres(p[1]);
        }
    }
    case
}

/// The metres east of the origin of `at` to the next edge between two zoom-13 tiles.
fn tile_edge_east() -> f64 {
    let o = world(8.4, 46.8);
    let n = f64::from(1u32 << MAX_ZOOM);
    ((o[0] * n).ceil() / n - o[0]) * metres(o[1])
}

#[test]
fn what_cannot_draw_stays_out() {
    let hill = ridge(200.0, 1.0);
    let ground = site(&hill, &[], false);
    // A footway east from an opening to a fork 60 m in, and beside its opening a V of a footway: from an opening 1.22 m
    // to its left 5 m in to a point on its axis and back to an opening 2.51 m to its left.
    let mut v = Case::default();
    let through = v.tunnel(&FOOT, (0.0, 60.0), 0.0);
    for n in [20.0, -20.0] {
        v.ways.push(("tunnel", tags(&FOOT), vec![part(&[[60.0, 0.0], [80.0, n]], &[500.0; 2])]));
    }
    v.ways.push(("tunnel", tags(&FOOT), vec![part(&[[0.0, 1.22], [5.0, 0.0], [0.0, 2.51]], &[500.0; 3])]));
    // Two road tunnels 20 m apart, the second with a fork at its east end.
    let mut pair = Case::default();
    let single = pair.tunnel(&ROAD, (0.0, 200.0), 0.0);
    pair.tunnel(&ROAD, (0.0, 200.0), 20.0);
    for n in [60.0, -20.0] {
        pair.ways.push(("tunnel", tags(&ROAD), vec![part(&[[200.0, 20.0], [210.0, n]], &[500.0; 2])]));
    }
    // A V of a footway whose two openings lead 28 degrees apart: its legs come within a wall of each other.
    let mut fold = Case::default();
    fold.ways.push(("tunnel", tags(&FOOT), vec![part(&[[0.0, 1.5], [5.0, 0.0], [0.0, 5.0]], &[500.0; 3])]));
    // A road tunnel whose east end lies past the bake's region.
    let mut cut = Case::default();
    cut.tunnel(&ROAD, (0.0, 200.0), 0.0);
    cut.bounds = Some([BOUNDS[0], BOUNDS[1], 8.4 + 100.0 / metres(world(8.4, 46.8)[1]) * 360.0, BOUNDS[3]]);
    for (name, case) in [("the V", v), ("the pair", pair), ("the folded V", fold), ("the bounds", cut)] {
        let placed = case.place(&ground, &mut Vec::new());
        let bounds = case.bounds.unwrap_or(BOUNDS);
        let faults = not_whole_in_tiles(&placed, bounds);
        assert!(faults.is_empty(), "{name}: {faults:?}");
        for t in records(&placed, "tunnel-tube") {
            assert!(!t["ends"].contains("none"), "{name}: a chord of a room that does not draw: {t:?}");
        }
        match name {
            "the V" => assert!(placed.features.is_empty() && !placed.tags.contains_key(&through), "{name}"),
            "the pair" => {
                let heads = records(&placed, "tunnel-end");
                assert!(heads.len() == 2 && heads.iter().all(|t| !t.contains_key("across")), "{name}: {heads:?}");
                assert_eq!(placed.tags[&single]["parts"], "arch arch");
            }
            _ => assert!(placed.features.is_empty(), "{name}: {}", placed.metrics),
        }
    }
}

#[test]
fn the_builder_reads_each_joint_back_from_the_tiles() {
    let edge = tile_edge_east();
    for (name, _, _, _) in layouts() {
        // A tile edge through each 20 m of the layout, its heads and its seams.
        for at in (0..=10).map(|k| -1.0 + 20.0 * f64::from(k)) {
            let shift = edge - at;
            let hill = |p: [f64; 2]| ridge(200.0, 1.0)([p[0] - shift, p[1]]);
            let case = layouts().into_iter().find(|l| l.0 == name).unwrap().1;
            let placed = moved(case, shift).place(&site(&hill, &[], false), &mut Vec::new());
            assert!(!placed.features.is_empty(), "{name}: no parts: {}", placed.metrics);
            let faults = not_whole_in_tiles(&placed, BOUNDS);
            assert!(faults.is_empty(), "{name}, a tile edge {at} m east: {faults:?}");
        }
    }
}
