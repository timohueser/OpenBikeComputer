use std::collections::{HashMap, HashSet};

use obc_ground_plan::graph::level_tags::Chain;
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
}

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

    /// The placement of the tunnels, each one chain with two tag ends, each ask measured by `site`; the open ends
    /// that the measure gets go to `opened`.
    fn place(&self, site: &dyn Fn(&Ask) -> Site, opened: &mut Vec<OpenEnd>) -> Placed {
        let roads = crate::kinds::solver_rows(&table(include_str!("../../../../data/trailview/roads.json")));
        let (class_names, _) = crate::kinds::classes(&roads).unwrap();
        let widths = widths();
        let rules =
            Rules::parse(&crate::kinds::part_rows(&table(include_str!("../../../../data/trailview/parts.json"))))
                .unwrap();
        let tunnels: HashMap<u32, (Tags, Vec<Part>)> = (self.ways.iter().enumerate())
            .filter(|(_, w)| w.0 == "tunnel")
            .map(|(id, w)| (id as u32, (w.1.clone(), w.2.clone())))
            .collect();
        let mut chains: Vec<Chain> = (tunnels.iter())
            .map(|(&id, (_, parts))| {
                let plan = parts[0].plan.as_ref().unwrap();
                Chain { ways: vec![(id, 0.0, plan[plan.len() - 1].along)], tag: [true, true] }
            })
            .collect();
        chains.sort_by_key(|c| c.ways[0].0);
        let (tiles, levels) = (Tiles::new([8.3, 46.7, 8.5, 46.9]), HashMap::new());
        let tunnel_ways: HashSet<usize> = tunnels.keys().map(|&id| id as usize).collect();
        let input = Input {
            chains: &chains,
            tunnels: &tunnels,
            ways: self.ways.iter().map(|(k, t, p)| (*k, t, p.as_slice())).collect(),
            tunnel_ways: &tunnel_ways,
            tiles: &tiles,
            levels: &levels,
            terrain: &|_: f64, _: f64| 0.0,
            rules: &rules,
            widths: &widths,
            class_names: &class_names,
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

/// The kinds and the rung of each opening of way `id`.
fn of(placed: &Placed, id: u32) -> Vec<(Vec<Kind>, &'static str)> {
    placed.ends.iter().filter(|e| e.0 == id).map(|(_, e)| (e.kinds.clone(), e.rung)).collect()
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
    // A footway 40 m long under a way: an underpass at both ends, as high as the room under the way allows, at least
    // the least height of a path. Under a building only: no part.
    let mut case = Case::default();
    case.tunnel(&FOOT, (0.0, 40.0), 0.0);
    let placed = case.place(&site(&|_| 500.0, &[3.2], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![Box, Free], "underpass"); 2]);
    let ends = records(&placed, "tunnel-end");
    assert_eq!((ends.len(), ends[0]["height"].as_str(), ends[0]["section"].as_str()), (2, "2.4", "box"));
    let placed = case.place(&site(&|_| 500.0, &[], true), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![], "under a building"); 2]);
    assert!(placed.features.is_empty() && placed.tags.is_empty());
    // Where another tunnel way meets an end, the tube goes on in the hill: that end has no opening.
    let mut case = Case::default();
    let (a, b) = (case.tunnel(&ROAD, (0.0, 200.0), 0.0), case.tunnel(&ROAD, (200.0, 400.0), 0.0));
    let placed = case.place(&site(&ridge(400.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!((of(&placed, a), of(&placed, b)), (vec![(vec![Arch, Free], "hill")], vec![(vec![Arch, Free], "hill")]));
    assert_eq!(placed.metrics["fork_ends"], 2);
    assert_eq!(placed.tags[&a]["parts"], "arch none");
    // A vertex of unknown height: no part.
    let mut case = road();
    case.ways[0].2[0].z.as_mut().unwrap()[1] = f64::NAN;
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, 0), vec![(vec![], "a vertex of unknown height"); 2]);
}

#[test]
fn a_pair_has_one_frame_line_and_a_way_in_a_wider_tube_has_no_part() {
    use Kind::*;
    // Two road tunnels 20 m apart under one ridge; the second starts 3 m farther in. A footway 2.5 m beside the first
    // lies in its tube.
    let mut case = Case::default();
    let a = case.tunnel(&ROAD, (0.0, 200.0), 0.0);
    let b = case.tunnel(&ROAD, (3.0, 200.0), 20.0);
    let foot = case.tunnel(&FOOT, (0.0, 200.0), 2.5);
    let placed = case.place(&site(&ridge(200.0, 1.0), &[], false), &mut Vec::new());
    assert_eq!(of(&placed, a), vec![(vec![Pair, Arch, Free], "hill"); 2]);
    assert_eq!(of(&placed, b), vec![(vec![Pair, Arch, Free], "hill"); 2]);
    assert_eq!(of(&placed, foot), vec![(vec![], "same tube"); 2]);
    // One record per pair, at the lower id. In the west the first opening moves 3 m in onto the line of the second,
    // and its mate lies 20 m to its left; in the east, heading west, to its right.
    let ends = records(&placed, "tunnel-end");
    assert_eq!(ends.len(), 2);
    let west = ends.iter().find(|t| t["mate"] == "20").expect("the west pair");
    assert_eq!((west["part"].as_str(), west["shift"].as_str(), west["mate_shift"].as_str()), ("pair", "3", "0"));
    assert!(ends.iter().any(|t| t["mate"] == "-20" && t["shift"] == "0"));
    // The first tube widens to the far edge of the footway and its margin.
    let foot_half = widths().way("tunnel", &tags(&FOOT), false).half_m;
    assert_eq!(west["half"], num(2.5 + foot_half + 0.6));
    assert!(!placed.tags.contains_key(&foot), "a chain with no part has no tag");
    assert_eq!(placed.tags[&a]["parts"], "pair pair");
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
    let rules = Rules::parse(&crate::kinds::part_rows(&table(include_str!("../../../../data/trailview/parts.json"))));
    let rules = rules.unwrap();
    let roads = table(include_str!("../../../../data/trailview/roads.json"));
    for class in roads.as_object().unwrap().keys() {
        assert!(rules.group(class).is_some(), "{class}");
    }
}
