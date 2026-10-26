//! The road solver on synthetic terrain.

use super::*;
use crate::metres;

const RAILS: [&str; 5] = ["rail", "light_rail", "narrow_gauge", "rack", "funicular"];

/// The class table in its order: a row a line.
fn table() -> (Vec<String>, Classes) {
    let text = include_str!("../../../../data/trailview/roads.json");
    let rows: serde_json::Map<String, serde_json::Value> = serde_json::from_str(text).unwrap();
    let names: Vec<String> =
        text.lines().filter_map(|line| Some(line.trim().strip_prefix('"')?.split('"').next()?.to_string())).collect();
    let classes = names
        .iter()
        .map(|name| (name, &rows[name]))
        .map(|(name, row)| Class {
            grade: row["grade"].as_f64().unwrap_or(f64::INFINITY),
            smooth_m: row["smooth_m"].as_f64().unwrap(),
            rail: RAILS.contains(&name.as_str()),
        })
        .collect();
    let path = names.iter().position(|n| n == "path").unwrap();
    (names, Classes { rows: classes, path })
}

/// A place: world coordinates of (east, north) metres from it.
#[derive(Clone, Copy)]
struct Place {
    x0: f64,
    y0: f64,
    unit: f64,
}

fn engelberg() -> Place {
    let y0 = (1.0 - 46.82f64.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0;
    Place { x0: (8.40 + 180.0) / 360.0, y0, unit: 1.0 / metres(y0) }
}

impl Place {
    fn at(&self, east: f64, north: f64) -> [f64; 2] {
        [self.x0 + east * self.unit, self.y0 - north * self.unit]
    }

    fn east_north(&self, [x, y]: [f64; 2]) -> (f64, f64) {
        ((x - self.x0) / self.unit, (self.y0 - y) / self.unit)
    }

    /// Points through (east, north) corners, with a vertex every `step` metres between them.
    fn line(&self, corners: &[(f64, f64)], step: Option<f64>) -> Vec<[f64; 2]> {
        let mut out = Vec::new();
        for pair in corners.windows(2) {
            let ((e0, n0), (e1, n1)) = (pair[0], pair[1]);
            let count = step.map_or(1, |s| ((e1 - e0).hypot(n1 - n0) / s).round_ties_even().max(1.0) as usize);
            for i in 0..count {
                let f = i as f64 / count as f64;
                out.push(self.at(e0 + (e1 - e0) * f, n0 + (n1 - n0) * f));
            }
        }
        let (e, n) = corners[corners.len() - 1];
        out.push(self.at(e, n));
        out
    }

    /// The terrain as a function of metres east and north.
    fn terrain(&self, height: impl Fn(f64, f64) -> f64 + Sync) -> impl Fn(f64, f64) -> f64 + Sync {
        let place = *self;
        move |x, y| {
            let (e, n) = place.east_north([x, y]);
            height(e, n)
        }
    }

    /// Metres along a solved part and its heights.
    fn profile(&self, part: &Option<(Vec<[f64; 2]>, Vec<f64>)>) -> (Vec<f64>, Vec<f64>) {
        let (points, z) = part.as_ref().unwrap();
        let mut along = vec![0.0];
        for pair in points.windows(2) {
            let d = [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]];
            along.push(along[along.len() - 1] + d[0].hypot(d[1]) / self.unit);
        }
        (along, z.clone())
    }
}

fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let k = xp.partition_point(|&v| v < x).clamp(1, xp.len() - 1);
    let t = (x - xp[k - 1]) / (xp[k] - xp[k - 1]);
    (fp[k - 1] + t * (fp[k] - fp[k - 1])).clamp(fp[k - 1].min(fp[k]), fp[k - 1].max(fp[k]))
}

/// A way of a class with `half` of a road without a width; `edit` sets its other facts.
fn way(class: &str, points: Vec<[f64; 2]>, edit: impl FnOnce(&mut Feature)) -> Feature {
    let (names, _) = table();
    let rule = names.iter().position(|n| n == class).unwrap();
    let mut feature = Feature { rule, half: 3.0, parts: vec![points], ..Default::default() };
    edit(&mut feature);
    feature
}

fn plain(_: &mut Feature) {}

fn bridge(feature: &mut Feature) {
    feature.free = true;
}

fn tunnel(feature: &mut Feature) {
    feature.free = true;
    feature.tunnel = true;
}

fn run(features: &[Feature], terrain: &dyn Terrain, rivers: &[River]) -> Solved {
    let refs: Vec<&Feature> = features.iter().collect();
    let rivers: Vec<&River> = rivers.iter().collect();
    solve(&refs, terrain, &rivers, &table().1, None)
}

fn height_at(part: &Option<(Vec<[f64; 2]>, Vec<f64>)>, point: [f64; 2]) -> f64 {
    let (points, z) = part.as_ref().unwrap();
    z[points.iter().position(|&p| p == point).unwrap()]
}

#[test]
fn a_road_on_a_noisy_slope_follows_the_slope_and_not_the_noise() {
    let place = engelberg();
    let terrain = place.terrain(|e, _| {
        let spike = if (e - 250.0).abs() < 2.0 { 3.0 } else { 0.0 };
        1000.0 + 0.10 * e + 0.3 * (e * 1.7).sin() + spike
    });
    let road = way("residential", place.line(&[(0.0, 0.0), (500.0, 0.0)], Some(50.0)), plain);
    let (along, z) = place.profile(&run(&[road], &terrain, &[]).heights[0][0]);
    for i in 0..z.len() {
        assert!((z[i] - (1000.0 + 0.10 * along[i])).abs() < 0.15);
        if i > 0 {
            assert!(((z[i] - z[i - 1]) / (along[i] - along[i - 1])).abs() <= 0.20 + 1e-4);
        }
    }
}

#[test]
fn a_hairpin_climbs_within_the_grade_of_its_class() {
    // A hillside of 30 %, and a tertiary road (15 %) that turns back on it: the bend alone would climb at 20 %.
    let place = engelberg();
    let terrain = place.terrain(|_, n| 1000.0 + 0.30 * n);
    let corners = [(0.0, 0.0), (300.0, 36.0), (312.0, 41.0), (316.0, 46.0), (312.0, 51.0), (300.0, 56.0), (0.0, 92.0)];
    let road = way("tertiary", place.line(&corners, Some(20.0)), plain);
    let (along, z) = place.profile(&run(&[road], &terrain, &[]).heights[0][0]);
    let grades: Vec<f64> = (1..z.len()).map(|i| (z[i] - z[i - 1]) / (along[i] - along[i - 1])).collect();
    assert!(grades.iter().all(|&g| g <= 0.15 + 1e-4 && g > 0.0));
    assert!((z[0] - 1000.0).abs() < 0.5);
    assert!((z[z.len() - 1] - (1000.0 + 0.30 * 92.0)).abs() < 0.5);
}

#[test]
fn a_side_road_meets_the_through_road_flat_and_on_one_height() {
    // The through road climbs east at 2 %, and from the junction on at 10 %, on a hillside that rises
    // north at 15 %; OSM splits it at the junction.
    let place = engelberg();
    let terrain = place.terrain(|e, n| 1000.0 + 0.02 * e + 0.08 * e.max(0.0) + 0.15 * n);
    let west = way("secondary", place.line(&[(-100.0, 0.0), (0.0, 0.0)], Some(20.0)), plain);
    let east = way("secondary", place.line(&[(0.0, 0.0), (100.0, 0.0)], Some(20.0)), plain);
    let side = way("residential", place.line(&[(0.0, 0.0), (0.0, 100.0)], Some(25.0)), plain);
    let solved = run(&[west, east, side], &terrain, &[]);
    let junction = place.at(0.0, 0.0);
    let heights: Vec<f64> = (0..3).map(|f| height_at(&solved.heights[f][0], junction)).collect();
    assert!(heights.iter().all(|&h| (h - heights[0]).abs() < 1e-9));
    // The side road's first metres lie on the through road's surface, which is level across it.
    let (along, z) = place.profile(&solved.heights[2][0]);
    let approach: Vec<usize> = (0..along.len()).filter(|&i| (along[i] - APPROACH_M).abs() <= 0.01).collect();
    assert_eq!(approach.len(), 1);
    assert!((z[approach[0]] - z[0]).abs() < 0.01);
    assert!(solved.metrics.approaches > 0);
    // The through road runs on smoothly from one way into the other: the kink of the terrain spreads.
    let ((aw, zw), (ae, ze)) = (place.profile(&solved.heights[0][0]), place.profile(&solved.heights[1][0]));
    let n = aw.len();
    let before = (zw[n - 1] - zw[n - 2]) / (aw[n - 1] - aw[n - 2]);
    let after = (ze[1] - ze[0]) / ae[1];
    assert!((after - before).abs() < 0.025);
}

#[test]
fn a_short_link_of_a_lower_class_does_not_move_its_roads() {
    // Two primary roads on the two levels of a terrace step of 2.4 m, and a residential link between
    // them, steeper than its class: its grade, its smoothness and its two ends give way.
    let place = engelberg();
    for gap in [4.5, 8.0, 9.5] {
        let terrain = place.terrain(move |_, n| 1000.0 + if n > gap / 2.0 { 2.4 } else { 0.0 });
        let lower = way("primary", place.line(&[(-100.0, 0.0), (0.0, 0.0), (100.0, 0.0)], Some(20.0)), plain);
        let upper = way("primary", place.line(&[(-100.0, gap), (0.0, gap), (100.0, gap)], Some(20.0)), plain);
        let link = way("residential", place.line(&[(0.0, 0.0), (0.0, gap)], None), plain);
        let solved = run(&[lower, upper, link], &terrain, &[]);
        assert!((height_at(&solved.heights[0][0], place.at(0.0, 0.0)) - 1000.0).abs() < 0.2, "gap {gap}");
        assert!((height_at(&solved.heights[1][0], place.at(0.0, gap)) - 1002.4).abs() < 0.2, "gap {gap}");
    }
}

#[test]
fn a_ford_lies_under_the_water_and_a_low_short_bridge_is_a_culvert() {
    let place = engelberg();
    let terrain = place.terrain(|e, n| {
        let gully = if (e - 115.0).abs() < 10.0 { 8.0 } else { 0.0 };
        if e > 251.0 && n > 60.0 {
            f64::NAN
        } else {
            1000.0 - gully
        }
    });
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let crossing = place.at(50.0, 0.0);
    let features = [
        way("track", line(&[(0.0, 0.0), (50.0, 0.0), (100.0, 0.0)]), plain),
        way("track", line(&[(30.0, 50.0), (36.0, 50.0)]), bridge),
        way("track", line(&[(100.0, 0.0), (130.0, 0.0)]), bridge),
        // A way tagged as a ford that crosses a stream between two of its vertices, 10 m along it.
        way("track", line(&[(60.0, 30.0), (66.0, 30.0), (74.0, 30.0), (84.0, 30.0)]), |f| f.ford = true),
        // A bridge that only roads in unknown terrain join to the known terrain.
        way("track", line(&[(200.0, 80.0), (250.0, 80.0)]), plain),
        way("track", line(&[(250.0, 80.0), (300.0, 80.0)]), plain),
        way("track", line(&[(320.0, 80.0), (350.0, 80.0)]), plain),
        way("track", line(&[(300.0, 80.0), (320.0, 80.0)]), bridge),
    ];
    let streams = [
        River {
            points: vec![place.at(50.0, -10.0), crossing, place.at(50.0, 10.0)],
            levels: vec![999.1, 999.0, 998.9],
        },
        River { points: vec![place.at(70.0, 20.0), place.at(70.0, 40.0)], levels: vec![998.6, 998.4] },
    ];
    let solved = run(&features, &terrain, &streams);
    assert!((height_at(&solved.heights[0][0], crossing) - (999.0 - FORD_M)).abs() < 0.01);
    let (along, z) = place.profile(&solved.heights[3][0]);
    assert!((interp(10.0, &along, &z) - (998.5 - FORD_M)).abs() < 0.01);
    assert_eq!(solved.culverts, BTreeSet::from([1]));
    // The bridge runs straight over the gully between its ends.
    let (along, z) = place.profile(&solved.heights[2][0]);
    let last = along.len() - 1;
    for i in 0..z.len() {
        assert!((z[i] - interp(along[i], &[along[0], along[last]], &[z[0], z[last]])).abs() < 0.05);
    }
    assert!(solved.heights[7][0].as_ref().unwrap().1.iter().all(|z| z.is_nan()));
}

#[test]
fn a_tunnel_that_ends_in_the_mountain_is_level_from_the_road_that_it_leaves() {
    let place = engelberg();
    let terrain = place.terrain(|e, _| 1000.0 + 0.3 * e.max(0.0));
    let features = [
        way("residential", place.line(&[(0.0, -300.0), (0.0, 300.0)], Some(20.0)), plain),
        way("footway", place.line(&[(0.0, 0.0), (400.0, 0.0)], Some(5.0)), tunnel),
    ];
    let (_, z) = place.profile(&run(&features, &terrain, &[]).heights[1][0]);
    assert!(z.iter().all(|h| (h - z[0]).abs() < 1e-3));
}

#[test]
fn a_way_on_the_ground_of_a_higher_layer_passes_over_and_one_indoors_stays_on_the_terrain() {
    let place = engelberg();
    let terrain = place.terrain(|_, _| 1000.0);
    let street = way("residential", place.line(&[(-100.0, 0.0), (100.0, 0.0)], Some(20.0)), plain);
    let over = |f: &mut Feature| (f.layer, f.layered) = (1, true);
    let indoor = |f: &mut Feature| (f.layer, f.layered, f.indoor) = (-1, true, true);
    for (edit, rise) in [(&over as &dyn Fn(&mut Feature), CLEAR_ROAD_M + DECK_PATH_M), (&indoor, 0.0)] {
        let corridor = way("footway", place.line(&[(5.0, -50.0), (5.0, 50.0)], None), edit);
        let solved = run(&[street.clone(), corridor], &terrain, &[]);
        let (ca, cz) = place.profile(&solved.heights[1][0]);
        let (sa, sz) = place.profile(&solved.heights[0][0]);
        let above = interp(50.0, &ca, &cz) - interp(105.0, &sa, &sz);
        assert_eq!(solved.metrics.passes, usize::from(rise > 0.0));
        assert!(above >= rise - KEEP_M && above < rise + 0.5);
    }
}

#[test]
fn the_tracks_of_a_double_line_on_noisy_ground_lie_at_one_height() {
    let place = engelberg();
    let terrain = place.terrain(|e, n| 500.0 + 0.02 * e + 0.2 * (e / 7.0).sin() * n.signum());
    let features = [
        way("rail", place.line(&[(0.0, 2.0), (400.0, 2.0)], Some(10.0)), plain),
        way("rail", place.line(&[(0.0, -2.0), (400.0, -2.0)], Some(10.0)), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    let ((na, nz), (sa, sz)) = (place.profile(&solved.heights[0][0]), place.profile(&solved.heights[1][0]));
    assert!(sa.iter().zip(&sz).all(|(&a, &z)| (interp(a, &na, &nz) - z).abs() < 0.01));
}

#[test]
fn a_track_on_a_ramp_beside_a_level_track_keeps_its_own_height() {
    // The embankment north of the level track rises 8 m.
    let place = engelberg();
    let terrain = place.terrain(|e, n| 500.0 + if n > 2.0 { 8.0 * ((e - 50.0) / 300.0).clamp(0.0, 1.0) } else { 0.0 });
    let features = [
        way("rail", place.line(&[(0.0, 4.0), (400.0, 4.0)], Some(10.0)), plain),
        way("rail", place.line(&[(0.0, 0.0), (400.0, 0.0)], Some(10.0)), plain),
    ];
    let (along, z) = place.profile(&run(&features, &terrain, &[]).heights[0][0]);
    assert!(interp(380.0, &along, &z) > 507.5);
}

#[test]
fn a_road_bridge_clears_the_track_under_it() {
    let place = engelberg();
    let terrain = place.terrain(|_, _| 500.0);
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("rail", place.line(&[(-300.0, 0.0), (300.0, 0.0)], Some(10.0)), plain),
        way("tertiary", line(&[(0.0, -150.0), (0.0, -30.0)]), plain),
        way("tertiary", line(&[(0.0, -30.0), (0.0, 30.0)]), bridge),
        way("tertiary", line(&[(0.0, 30.0), (0.0, 150.0)]), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    assert_eq!(solved.metrics.clearances_low_cm, 0);
    let (along, z) = place.profile(&solved.heights[2][0]);
    let (ra, rz) = place.profile(&solved.heights[0][0]);
    assert!(interp(30.0, &along, &z) - interp(300.0, &ra, &rz) >= CLEAR_RAIL_M + DECK_M - 0.01);
}

/// Metres east and north of a point at about 46.7° N.
fn bridges() -> Place {
    let y0 = 0.3218;
    Place { x0: 0.53, y0, unit: 1.0 / metres(y0) }
}

fn flat(_: f64, _: f64) -> f64 {
    500.0
}

#[test]
fn a_deck_rises_over_the_water_and_the_road_that_it_crosses() {
    let place = bridges();
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("residential", line(&[(-100.0, 0.0), (0.0, 0.0)]), plain),
        way("residential", line(&[(0.0, 0.0), (60.0, 0.0)]), bridge),
        way("residential", line(&[(60.0, 0.0), (160.0, 0.0)]), plain),
        way("tertiary", line(&[(45.0, -80.0), (45.0, 80.0)]), plain),
    ];
    let river = River { points: vec![place.at(20.0, -100.0), place.at(20.0, 100.0)], levels: vec![499.0, 499.0] };
    let solved = run(&features, &flat, &[river]);
    assert_eq!((solved.metrics.clearances, solved.metrics.clearances_low_cm), (2, 0));
    let (along, z) = place.profile(&solved.heights[1][0]);
    assert!(interp(20.0, &along, &z) >= 499.0 + CLEAR_WATER_M + DECK_M - KEEP_M);
    let (below_along, below_z) = place.profile(&solved.heights[3][0]);
    // The row holds the deck and the road at the crossing; the tiles drop vertices within KEEP_M of the profile.
    let below = interp(80.0, &below_along, &below_z);
    assert!(interp(45.0, &along, &z) - below >= CLEAR_ROAD_M + DECK_M - 2.0 * KEEP_M);
}

#[test]
fn a_stream_by_the_end_of_a_footbridge_neither_levers_it_up_nor_lifts_it_off_the_path_that_it_leaves() {
    let place = bridges();
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let path = |points, edit: fn(&mut Feature)| {
        way("footway", points, |f| {
            f.half = 1.0;
            edit(f);
        })
    };
    let least = 500.0 + CLEAR_WATER_M + DECK_PATH_M;
    let river = River { points: vec![place.at(1.0, -50.0), place.at(1.0, 50.0)], levels: vec![500.0, 500.0] };
    let features = [path(line(&[(-50.0, 0.0), (0.0, 0.0)]), plain), path(line(&[(0.0, 0.0), (9.0, 0.0)]), bridge)];
    let solved = run(&features, &flat, &[river]);
    // The deck clears the stream, and its free end stands on the terrain.
    let (along, z) = place.profile(&solved.heights[1][0]);
    assert!(interp(1.0, &along, &z) >= least - 0.01);
    assert!(z.iter().copied().fold(f64::MIN, f64::max) < least + 0.5);
    assert!(interp(9.0, &along, &z) < 501.0);
    // As a side of a path, the bridge lands on it: 1 m on it cannot climb the clearance.
    let river = River { points: vec![place.at(-50.0, 1.0), place.at(50.0, 1.0)], levels: vec![500.0, 500.0] };
    let features = [path(line(&[(-50.0, 0.0), (50.0, 0.0)]), plain), path(line(&[(0.0, 0.0), (0.0, 9.0)]), bridge)];
    let solved = run(&features, &flat, &[river]);
    assert!(solved.heights[1][0].as_ref().unwrap().1.iter().all(|&z| z < 500.05));
    // A bridge whose end lies on a road, within its half-width, meets it and does not pass over it.
    let road = way("residential", line(&[(0.5, -50.0), (0.5, 50.0)]), plain);
    let features =
        [path(line(&[(-50.0, 0.0), (0.0, 0.0)]), plain), path(line(&[(0.0, 0.0), (9.0, 0.0)]), bridge), road];
    assert_eq!(run(&features, &flat, &[]).metrics.clearances, 0);
}

#[test]
fn a_short_bridge_over_a_ditch_clears_its_water_and_one_on_the_ground_is_a_culvert() {
    let place = bridges();
    let terrain = place.terrain(|e, n| if e.abs() < 2.0 && n < 25.0 { 498.0 } else { 500.0 });
    let mut features = Vec::new();
    for north in [0.0, 50.0] {
        for (ends, edit) in [((-100.0, -3.0), plain as fn(&mut Feature)), ((-3.0, 3.0), bridge), ((3.0, 100.0), plain)]
        {
            features.push(way("track", place.line(&[(ends.0, north), (ends.1, north)], None), edit));
        }
    }
    let rivers: Vec<River> = [(0.0, 498.2), (50.0, 499.9)]
        .iter()
        .map(|&(north, level)| River {
            points: vec![place.at(0.0, north - 20.0), place.at(0.0, north + 20.0)],
            levels: vec![level, level],
        })
        .collect();
    let solved = run(&features, &terrain, &rivers);
    assert_eq!(solved.culverts, BTreeSet::from([4]));
    let (along, z) = place.profile(&solved.heights[1][0]);
    assert!(interp(3.0, &along, &z) >= 498.2 + CLEAR_WATER_M + DECK_M - KEEP_M);
}

#[test]
fn of_two_bridges_at_one_layer_the_better_class_passes_over_and_moves_less() {
    let place = bridges();
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let layered = |f: &mut Feature| (f.free, f.layer, f.layered) = (true, 1, true);
    let features = [
        way("motorway", line(&[(-200.0, 0.0), (-50.0, 0.0)]), plain),
        way("motorway", line(&[(-50.0, 0.0), (50.0, 0.0)]), layered),
        way("motorway", line(&[(50.0, 0.0), (200.0, 0.0)]), plain),
        way("service", line(&[(0.0, -200.0), (0.0, -30.0)]), plain),
        way("service", line(&[(0.0, -30.0), (0.0, 30.0)]), layered),
        way("service", line(&[(0.0, 30.0), (0.0, 200.0)]), plain),
    ];
    let solved = run(&features, &flat, &[]);
    let (oa, oz) = place.profile(&solved.heights[1][0]);
    let (ua, uz) = place.profile(&solved.heights[4][0]);
    let (over, under) = (interp(50.0, &oa, &oz), interp(30.0, &ua, &uz));
    assert!(over - under >= CLEAR_ROAD_M + DECK_M - KEEP_M);
    assert!((over - 500.0).abs() < (under - 500.0).abs());
    assert_eq!((solved.metrics.passes, solved.metrics.passes_near_cm), (1, 0));
}

#[test]
fn a_long_tunnel_runs_straight_between_its_portals_and_a_shaft_does_not_lift_it() {
    let place = bridges();
    let terrain = place.terrain(|e, _| if e.abs() < 2995.0 { 2500.0 } else { 1000.0 });
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("primary", line(&[(-3200.0, 0.0), (-3000.0, 0.0)]), plain),
        way("primary", line(&[(-3000.0, 0.0), (-1000.0, 0.0), (0.0, 0.0)]), tunnel),
        way("primary", line(&[(0.0, 0.0), (3000.0, 0.0)]), tunnel),
        way("primary", line(&[(3000.0, 0.0), (3200.0, 0.0)]), plain),
        way("service", line(&[(-1000.0, 0.0), (-1000.0, 300.0)]), tunnel),
        way("service", line(&[(-1000.0, 300.0), (-1000.0, 500.0)]), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    let vertices: Vec<(f64, f64)> = [1, 2]
        .iter()
        .flat_map(|&f| {
            let (points, z) = solved.heights[f][0].as_ref().unwrap();
            points.iter().zip(z).map(|(&p, &z)| (place.east_north(p).0, z)).collect::<Vec<_>>()
        })
        .collect();
    let (first, last) = (vertices[0].1, vertices[vertices.len() - 1].1);
    for &(e, z) in &vertices {
        assert!((z - interp(e, &[-3000.0, 3000.0], &[first, last])).abs() < 0.01);
    }
    assert!(vertices.iter().all(|&(_, z)| z < 1001.0));
}

#[test]
fn a_tunnel_sinks_under_the_whole_width_of_the_road_over_it_and_the_road_stays() {
    // The north portal lies just past the motorway's edge, its half-width from its centreline.
    let place = bridges();
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let main_half = 6.0;
    let features = [
        way("motorway", line(&[(-200.0, 0.0), (200.0, 0.0)]), |f| f.half = main_half),
        way("unclassified", line(&[(5.0, -200.0), (5.0, -20.0)]), plain),
        way("unclassified", line(&[(5.0, -20.0), (5.0, 7.0)]), tunnel),
        way("unclassified", line(&[(5.0, 7.0), (5.0, 200.0)]), plain),
    ];
    let solved = run(&features, &flat, &[]);
    assert_eq!(solved.underpasses, BTreeSet::from([2]));
    let (ra, rz) = place.profile(&solved.heights[0][0]);
    assert!((interp(205.0, &ra, &rz) - 500.0).abs() < 0.05);
    let (ta, tz) = place.profile(&solved.heights[2][0]);
    for north in [-main_half, 0.0, main_half] {
        assert!(interp(north + 20.0, &ta, &tz) <= 500.0 - BORE_M - COVER_M + 0.01);
    }
}

#[test]
fn a_footbridge_over_a_gorge_does_not_sag_under_its_ends() {
    let place = bridges();
    let terrain = place.terrain(|e, _| {
        if e < 0.0 {
            500.0 - 0.3 * e
        } else if e > 30.0 {
            500.0 + 0.05 * (e - 30.0)
        } else {
            470.0
        }
    });
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("path", line(&[(-100.0, 0.0), (0.0, 0.0)]), plain),
        way("path", line(&[(0.0, 0.0), (15.0, 8.0), (30.0, 0.0)]), bridge),
        way("path", line(&[(30.0, 0.0), (130.0, 0.0)]), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    let z = &solved.heights[1][0].as_ref().unwrap().1;
    assert!(z.iter().copied().fold(f64::MAX, f64::min) >= z[0].min(z[z.len() - 1]) - 0.05);
}

/// The networks of a region one zoom-10 tile wide around a corner of four areas.
fn corner_network(place: &Place) -> (Vec<Feature>, Vec<River>, impl Fn(f64, f64) -> f64 + Sync) {
    let n = (1u64 << area::AREA_ZOOM) as f64;
    let corner = (((place.x0 * n).floor() + 1.0) / n - place.x0) / place.unit;
    let corner = (corner, (place.y0 - (place.y0 * n).floor() / n) / place.unit);
    let near = |points: &[(f64, f64)]| points.iter().map(|&(e, n)| (corner.0 + e, corner.1 + n)).collect::<Vec<_>>();
    // A fixed pseudo-random ground roughness.
    let noise: Vec<f64> =
        (0..20000u64).map(|i| ((i.wrapping_mul(2654435761) % 1000) as f64 / 1000.0 - 0.5) * 0.8).collect();
    let terrain =
        place.terrain(move |e, n| 1000.0 + 0.15 * e - 0.03 * n + noise[((e + 3.0 * n).abs() / 0.5) as usize % 20000]);
    let features = vec![
        way("secondary", place.line(&near(&[(-600.0, -400.0), (0.0, 0.0), (600.0, 300.0)]), Some(40.0)), plain),
        way("residential", place.line(&near(&[(0.0, 0.0), (-200.0, 300.0), (-300.0, 500.0)]), Some(30.0)), plain),
        way("track", place.line(&near(&[(200.0, -300.0), (220.0, -200.0)]), Some(10.0)), bridge),
        way("track", place.line(&near(&[(-500.0, 200.0), (-100.0, 200.0)]), Some(50.0)), plain),
        way("residential", place.line(&near(&[(-300.0, -100.0), (300.0, -100.0)]), Some(60.0)), tunnel),
        way("track", place.line(&near(&[(40.0, 100.0), (46.0, 100.0)]), None), bridge),
    ];
    let stream = near(&[(210.0, -260.0), (180.0, -240.0)]).iter().map(|&(e, n)| place.at(e, n)).collect();
    (features, vec![River { points: stream, levels: vec![990.0, 989.5] }], terrain)
}

#[test]
fn the_jobs_of_four_areas_that_each_hold_the_whole_network_stitch_into_its_one_solve() {
    let place = engelberg();
    let (features, rivers, terrain) = corner_network(&place);
    let refs: Vec<&Feature> = features.iter().collect();
    let areas: BTreeSet<(i64, i64)> = features.iter().flat_map(|f| &f.parts[0]).map(|&p| area::area_of(p)).collect();
    assert_eq!(areas.len(), 4);
    let whole = run(&features, &terrain, &rivers);
    let apart = solve_by_area(&refs, &[], &terrain, &rivers, &table().1, usize::MAX);
    assert_eq!(whole.heights, apart.heights);
    assert_eq!((whole.culverts, whole.underpasses), (apart.culverts, apart.underpasses));
    let without_residual = |m: &Metrics| Metrics { residual_p99_m: 0.0, ..m.clone() };
    assert_eq!(without_residual(&whole.metrics), without_residual(&apart.metrics));
}

#[test]
fn a_tunnel_runs_straight_to_its_portal_on_a_road_past_the_region_and_one_that_the_input_cuts_has_none() {
    let place = engelberg();
    let terrain = place.terrain(|e, _| 1000.0 - 0.02 * e + if (e - 3000.0).abs() < 2900.0 { 800.0 } else { 0.0 });
    let features = [
        way("residential", place.line(&[(-300.0, 0.0), (0.0, 0.0)], Some(20.0)), plain),
        way("residential", place.line(&[(0.0, 0.0), (2000.0, 0.0)], Some(50.0)), tunnel),
    ];
    let beyond = [
        way("residential", place.line(&[(2000.0, 0.0), (6000.0, 0.0)], Some(50.0)), tunnel),
        way("residential", place.line(&[(6000.0, -300.0), (6000.0, 0.0), (6000.0, 300.0)], Some(20.0)), plain),
        way("residential", place.line(&[(0.0, 5000.0), (300.0, 5000.0)], Some(20.0)), plain),
    ];
    let (context_of, cut) = context(&features, &beyond, &|_| true);
    assert_eq!((context_of.clone(), cut), (vec![0, 1], BTreeSet::new()));
    let refs: Vec<&Feature> = features.iter().collect();
    let context_refs: Vec<&Feature> = context_of.iter().map(|&i| &beyond[i]).collect();
    let solved = solve_by_area(&refs, &context_refs, &terrain, &[], &table().1, usize::MAX);
    let (along, z) = place.profile(&solved.heights[1][0]);
    for i in 0..z.len() {
        assert!((z[i] - interp(along[i], &[0.0, 6000.0], &[1000.0, 880.0])).abs() < 0.5);
    }
    let (_, cut) = context(&features, &beyond[..1], &|p| place.east_north(p).0 < 5000.0);
    assert_eq!(cut, BTreeSet::from([1]));
}
