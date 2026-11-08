//! The road solver on synthetic terrain.

use super::*;
use crate::geometry::interp;
use crate::metres;

/// The curve of the kept vertices leaves the heights of the solve by at most this.
pub(super) const KEEP_M: f64 = 0.5;
/// A held deck, sag, cover or pass gives way: it leaves its bound by at most this.
const GIVE_M: f64 = 0.1;

/// The class table in its order.
pub(super) fn table() -> (Vec<String>, Classes) {
    let rows = serde_json::from_str(include_str!("../../../../data/trailview/roads.json")).unwrap();
    crate::kinds::classes(&crate::kinds::solver_rows(&rows)).unwrap()
}

/// A place: world coordinates of (east, north) metres from it.
#[derive(Clone, Copy)]
pub(super) struct Place {
    pub x0: f64,
    pub y0: f64,
    pub unit: f64,
}

pub(super) fn engelberg() -> Place {
    let y0 = (1.0 - 46.82f64.to_radians().tan().asinh() / std::f64::consts::PI) / 2.0;
    Place { x0: (8.40 + 180.0) / 360.0, y0, unit: 1.0 / metres(y0) }
}

impl Place {
    pub(super) fn at(&self, east: f64, north: f64) -> [f64; 2] {
        [self.x0 + east * self.unit, self.y0 - north * self.unit]
    }

    pub(super) fn east_north(&self, [x, y]: [f64; 2]) -> (f64, f64) {
        ((x - self.x0) / self.unit, (self.y0 - y) / self.unit)
    }

    /// Points through (east, north) corners, with a vertex every `step` metres between them.
    pub(super) fn line(&self, corners: &[(f64, f64)], step: Option<f64>) -> Vec<[f64; 2]> {
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
    pub(super) fn terrain(&self, height: impl Fn(f64, f64) -> f64 + Sync) -> impl Fn(f64, f64) -> f64 + Sync {
        let place = *self;
        move |x, y| {
            let (e, n) = place.east_north([x, y]);
            height(e, n)
        }
    }

    /// The curve of a solved way.
    fn curve(&self, part: &Option<Way>) -> crate::Part {
        let way = part.as_ref().unwrap();
        let (points, z, plan) = (way.points.clone(), Some(way.z.clone()), Some(way.plan.clone()));
        polyline(&crate::Part { outer: false, points, z, plan })
    }

    /// Metres along the curve of a solved way and its heights.
    pub(super) fn profile(&self, part: &Option<Way>) -> (Vec<f64>, Vec<f64>) {
        let curve = self.curve(part);
        let mut along = vec![0.0];
        for pair in curve.points.windows(2) {
            let d = [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]];
            along.push(along[along.len() - 1] + d[0].hypot(d[1]) / self.unit);
        }
        (along, curve.z.unwrap())
    }
}

/// A way of a class with the half-widths of a road without a width; `edit` sets its other facts.
pub(super) fn way(class: &str, points: Vec<[f64; 2]>, edit: impl FnOnce(&mut Feature)) -> Feature {
    let (names, _) = table();
    let rule = names.iter().position(|n| n == class).unwrap();
    let mut feature = Feature { rule, half: 3.0, outer: 3.0, parts: vec![points], ..Default::default() };
    edit(&mut feature);
    feature
}

pub(super) fn plain(_: &mut Feature) {}

pub(super) fn bridge(feature: &mut Feature) {
    feature.free = true;
}

fn tunnel(feature: &mut Feature) {
    feature.free = true;
    feature.tunnel = true;
}

pub(super) fn run(features: &[Feature], terrain: &dyn Terrain, rivers: &[River]) -> Solved {
    let refs: Vec<&Feature> = features.iter().collect();
    let near = Near { rivers: rivers.iter().collect(), ..Default::default() };
    solve(&refs, terrain, &near, &table().1, None)
}

fn height_at(part: &Option<Way>, point: [f64; 2]) -> f64 {
    let way = part.as_ref().unwrap();
    way.z[way.points.iter().position(|&p| p == point).unwrap()]
}

#[test]
fn a_road_on_a_noisy_slope_follows_the_slope_and_not_the_noise() {
    let place = engelberg();
    let terrain = place.terrain(|e, _| {
        let spike = if (e - 250.0).abs() < 2.0 { 3.0 } else { 0.0 };
        1000.0 + 0.10 * e + 0.3 * (e * 1.7).sin() + spike
    });
    let road = way("residential", place.line(&[(0.0, 0.0), (500.0, 0.0)], Some(50.0)), plain);
    let (along, z) = place.profile(&run(&[road], &terrain, &[]).ways[0][0]);
    for i in 0..z.len() {
        assert!((z[i] - (1000.0 + 0.10 * along[i])).abs() < 0.15);
        if i > 0 {
            assert!(((z[i] - z[i - 1]) / (along[i] - along[i - 1])).abs() <= 0.20 + 1e-4);
        }
    }
}

#[test]
fn a_hairpin_climbs_within_the_grade_of_its_class() {
    let place = engelberg();
    let terrain = place.terrain(|_, n| 1000.0 + 0.30 * n);
    let corners = [(0.0, 0.0), (300.0, 36.0), (312.0, 41.0), (316.0, 46.0), (312.0, 51.0), (300.0, 56.0), (0.0, 92.0)];
    let road = way("tertiary", place.line(&corners, Some(20.0)), plain);
    let solved = run(&[road], &terrain, &[]);
    let curve = place.curve(&solved.ways[0][0]);
    let z = curve.z.unwrap();
    for k in 1..z.len() {
        let (a, b) = (place.east_north(curve.points[k - 1]), place.east_north(curve.points[k]));
        let grade = (z[k] - z[k - 1]) / (b.0 - a.0).hypot(b.1 - a.1);
        assert!(grade > 0.0);
        // The solve holds the grade on the line of OSM. The fit shortens the tip, and the heights stay.
        let tip = ((a.0 + b.0) / 2.0 - 316.0).hypot((a.1 + b.1) / 2.0 - 46.0);
        assert!(tip < 25.0 || grade <= 0.15 + 1e-4, "{grade} at {tip} m from the tip");
    }
    assert!((z[0] - 1000.0).abs() < 0.5);
    assert!((z[z.len() - 1] - (1000.0 + 0.30 * 92.0)).abs() < 0.5);
}

#[test]
fn a_side_road_between_two_near_junctions_holds_its_grade() {
    // A secondary ends at J on flat ground at 1000 m, where a residential runs on north. 4.4 m on, a
    // path leaves the residential on flat ground at 1002 m. The residential is a side at J, but the
    // path's junction is too near for a pin: nothing but its grade ties J to the path's junction.
    let place = engelberg();
    let terrain = place.terrain(|_, n| 1000.0 + 2.0 * (n / 4.4).clamp(0.0, 1.0));
    let secondary = way("secondary", place.line(&[(-100.0, 0.0), (0.0, 0.0)], Some(20.0)), plain);
    let residential =
        way("residential", place.line(&[(0.0, -100.0), (0.0, 0.0), (0.0, 4.4), (0.0, 100.0)], Some(20.0)), plain);
    let path = way("path", place.line(&[(0.0, 4.4), (60.0, 4.4)], Some(20.0)), plain);
    let solved = run(&[secondary, residential, path], &terrain, &[]);
    let rise = height_at(&solved.ways[1][0], place.at(0.0, 4.4)) - height_at(&solved.ways[1][0], place.at(0.0, 0.0));
    assert!(rise.abs() <= 0.20 * 4.4 + 0.01, "{rise} m over 4.4 m");
}

#[test]
fn a_path_leaves_a_road_as_a_ramp_and_climbs_on_with_the_terrain() {
    // A road on a terrace 2 m over flat ground, with its edge 1 m beside it, and a path that leaves it
    // north: past the approach, the path runs down as a ramp. On a hillside of 50 % instead, the path
    // is back on the terrain at the ramp's end.
    let place = engelberg();
    let terrace = place.terrain(|_, n| if n < 1.0 { 1002.0 } else { 1000.0 });
    let hill = place.terrain(|_, n| 1000.0 + 0.5 * n);
    for (terrain, steep) in [(&terrace as &dyn Terrain, false), (&hill, true)] {
        let road = way("residential", place.line(&[(-100.0, 0.0), (100.0, 0.0)], Some(20.0)), plain);
        let path = way("path", place.line(&[(0.0, 0.0), (0.0, 60.0)], Some(2.0)), plain);
        let (along, z) = place.profile(&run(&[road, path], terrain, &[]).ways[1][0]);
        if steep {
            for (&a, &z) in along.iter().zip(&z).filter(|(&a, _)| a >= RAMP_M) {
                assert!((z - 1000.0 - 0.5 * a).abs() < 0.3, "{} m at {a} m", z - 1000.0 - 0.5 * a);
            }
        } else {
            let ramp =
                (interp(RAMP_M, &along, &z) - interp(APPROACH_M + 1.0, &along, &z)) / (RAMP_M - APPROACH_M - 1.0);
            assert!((-RAMP_GRADE - 0.01..-0.05).contains(&ramp), "{ramp}");
        }
    }
}

#[test]
fn a_road_split_on_a_slope_keeps_the_slope() {
    // OSM cuts a residential on an 8 % slope into two ways, with a vertex 3 m before the cut: the
    // way that continues it is not a way beside it.
    let place = engelberg();
    let terrain = place.terrain(|e, _| 1000.0 + 0.08 * e);
    let west = way("residential", place.line(&[(-100.0, 0.0), (-3.0, 0.0), (0.0, 0.0)], Some(20.0)), plain);
    let east = way("residential", place.line(&[(0.0, 0.0), (100.0, 0.0)], Some(20.0)), plain);
    let solved = run(&[west, east], &terrain, &[]);
    for (f, start) in [(0, -100.0), (1, 0.0)] {
        let (along, z) = place.profile(&solved.ways[f][0]);
        assert!(along.iter().zip(&z).all(|(&a, &z)| (z - 1000.0 - 0.08 * (start + a)).abs() < 0.05));
    }
}

#[test]
fn steps_that_meet_a_footway_below_a_deck_do_not_move_the_deck() {
    // A residential bridge 6 m over a valley floor, and steps from its middle that meet a footway on
    // the floor some metres on. The smoothing of the steps alone draws the deck down by 0.61 m; their
    // ramp bends the steps, and moves neither the deck nor the footway.
    let place = engelberg();
    let terrain = place.terrain(|e, _| if e.abs() > 30.0 { 1006.0 } else { 1000.0 });
    for long in [12.0, 16.0, 20.0, 30.0] {
        let solve = |steps: bool| {
            let mut features = vec![
                way("residential", place.line(&[(-100.0, 0.0), (-30.0, 0.0)], Some(10.0)), plain),
                way("residential", place.line(&[(-30.0, 0.0), (0.0, 0.0), (30.0, 0.0)], Some(10.0)), bridge),
                way("residential", place.line(&[(30.0, 0.0), (100.0, 0.0)], Some(10.0)), plain),
                way("footway", place.line(&[(-20.0, long), (0.0, long), (20.0, long)], Some(2.0)), plain),
            ];
            if steps {
                features.push(way("steps", place.line(&[(0.0, 0.0), (0.0, long)], Some(2.0)), plain));
            }
            let solved = run(&features, &terrain, &[]);
            let [(da, dz), (fa, fz)] = [1, 3].map(|f| place.profile(&solved.ways[f][0]));
            (interp(30.0, &da, &dz), interp(20.0, &fa, &fz))
        };
        let ((alone, floor), (with, foot)) = (solve(false), solve(true));
        assert!(with - alone > -0.65, "at {long} m the deck moves from {alone} to {with}");
        assert!((foot - floor).abs() < 0.3, "at {long} m the footway moves from {floor} to {foot}");
    }
}

#[test]
fn a_short_link_with_a_footway_off_its_middle_does_not_move_its_roads() {
    // A residential link of 9 m between two secondaries 3 m apart in height, with a footway that
    // leaves the link at its middle: the link is a side at both of its ends.
    let place = engelberg();
    let terrain = place.terrain(|_, n| 1000.0 + if n > 4.5 { 3.0 } else { 0.0 });
    let features = [
        way("secondary", place.line(&[(-100.0, 0.0), (0.0, 0.0), (100.0, 0.0)], Some(20.0)), plain),
        way("secondary", place.line(&[(-100.0, 9.0), (0.0, 9.0), (100.0, 9.0)], Some(20.0)), plain),
        way("residential", place.line(&[(0.0, 0.0), (0.0, 4.5), (0.0, 9.0)], None), plain),
        way("footway", place.line(&[(0.0, 4.5), (40.0, 4.5)], Some(10.0)), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    assert!((height_at(&solved.ways[0][0], place.at(0.0, 0.0)) - 1000.0).abs() < 0.05);
    assert!((height_at(&solved.ways[1][0], place.at(0.0, 9.0)) - 1003.0).abs() < 0.05);
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
    let heights: Vec<f64> = (0..3).map(|f| height_at(&solved.ways[f][0], junction)).collect();
    assert!(heights.iter().all(|&h| (h - heights[0]).abs() < 1e-9));
    // The side road's first metres lie on the through road's surface, which is level across it.
    let (along, z) = place.profile(&solved.ways[2][0]);
    let approach: Vec<usize> = (0..along.len()).filter(|&i| (along[i] - APPROACH_M).abs() <= 0.01).collect();
    assert_eq!(approach.len(), 1);
    assert!((z[approach[0]] - z[0]).abs() < 0.01);
    assert!(solved.metrics.approaches > 0);
    // The through road runs on smoothly from one way into the other: the kink of the terrain spreads.
    let ((aw, zw), (ae, ze)) = (place.profile(&solved.ways[0][0]), place.profile(&solved.ways[1][0]));
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
        assert!((height_at(&solved.ways[0][0], place.at(0.0, 0.0)) - 1000.0).abs() < 0.2, "gap {gap}");
        assert!((height_at(&solved.ways[1][0], place.at(0.0, gap)) - 1002.4).abs() < 0.2, "gap {gap}");
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
            half: 1.0,
            cover: 1.0,
            lift: 1.5,
            untagged_culvert: true,
        },
        River {
            points: vec![place.at(70.0, 20.0), place.at(70.0, 40.0)],
            levels: vec![998.6, 998.4],
            half: 1.0,
            cover: 1.0,
            lift: 1.5,
            untagged_culvert: true,
        },
    ];
    let solved = run(&features, &terrain, &streams);
    assert!((height_at(&solved.ways[0][0], crossing) - (999.0 - FORD_M)).abs() < 0.01);
    let (along, z) = place.profile(&solved.ways[3][0]);
    assert!((interp(10.0, &along, &z) - (998.5 - FORD_M)).abs() < 0.01);
    assert_eq!(solved.culverts, BTreeSet::from([1]));
    // The bridge runs straight over the gully between its ends.
    let (along, z) = place.profile(&solved.ways[2][0]);
    let last = along.len() - 1;
    for i in 0..z.len() {
        assert!((z[i] - interp(along[i], &[along[0], along[last]], &[z[0], z[last]])).abs() < 0.05);
    }
    assert!(solved.ways[7][0].as_ref().unwrap().z.iter().all(|z| z.is_nan()));
}

#[test]
fn a_tunnel_that_ends_in_the_mountain_is_level_from_the_road_that_it_leaves() {
    let place = engelberg();
    let terrain = place.terrain(|e, _| 1000.0 + 0.3 * e.max(0.0));
    let features = [
        way("residential", place.line(&[(0.0, -300.0), (0.0, 300.0)], Some(20.0)), plain),
        way("footway", place.line(&[(0.0, 0.0), (400.0, 0.0)], Some(5.0)), tunnel),
    ];
    let (_, z) = place.profile(&run(&features, &terrain, &[]).ways[1][0]);
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
        let (ca, cz) = place.profile(&solved.ways[1][0]);
        let (sa, sz) = place.profile(&solved.ways[0][0]);
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
    let ((na, nz), (sa, sz)) = (place.profile(&solved.ways[0][0]), place.profile(&solved.ways[1][0]));
    assert!(sa.iter().zip(&sz).all(|(&a, &z)| (interp(a, &na, &nz) - z).abs() < 0.01));
}

#[test]
fn a_road_whose_carriageway_overlaps_another_lies_at_its_height() {
    // A service road and a track 4.5 m apart on a slope across them: their carriageways overlap, and
    // the terrain under them differs by 0.45 m. A narrower footway as far from the track does not
    // reach it, and keeps its own height.
    let place = engelberg();
    let terrain = place.terrain(|e, n| 500.0 + 0.02 * e + 0.1 * n);
    let narrow = |f: &mut Feature| f.half = 1.0;
    let features = [
        way("service", place.line(&[(0.0, 2.25), (300.0, 2.25)], Some(10.0)), plain),
        way("track", place.line(&[(0.0, -2.25), (300.0, -2.25)], Some(10.0)), plain),
        way("footway", place.line(&[(0.0, -6.75), (300.0, -6.75)], Some(10.0)), narrow),
    ];
    let solved = run(&features, &terrain, &[]);
    let [(na, nz), (sa, sz), (fa, fz)] = [0, 1, 2].map(|f| place.profile(&solved.ways[f][0]));
    // Without the rule they lie 0.45 m apart; the kept curves leave the solve by a few centimetres.
    assert!(sa.iter().zip(&sz).all(|(&a, &z)| (interp(a, &na, &nz) - z).abs() < 0.1));
    assert!(fa.iter().zip(&fz).all(|(&a, &z)| (interp(a, &sa, &sz) - z).abs() > 0.3));
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
    let (along, z) = place.profile(&run(&features, &terrain, &[]).ways[0][0]);
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
    assert_eq!(solved.metrics.clearances_low_dm, 0);
    let (along, z) = place.profile(&solved.ways[2][0]);
    let (ra, rz) = place.profile(&solved.ways[0][0]);
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
fn a_road_over_a_culvert_holds_its_cover_a_path_keeps_its_height_and_no_cover_lifts_too_far() {
    let place = engelberg();
    let features = [
        way("residential", place.line(&[(-100.0, 0.0), (100.0, 0.0)], Some(20.0)), plain),
        way("footway", place.line(&[(-100.0, 200.0), (100.0, 200.0)], Some(20.0)), plain),
        way("residential", place.line(&[(-100.0, 400.0), (100.0, 400.0)], Some(20.0)), plain),
        way("residential", place.line(&[(-100.0, 600.0), (100.0, 600.0)], Some(20.0)), plain),
    ];
    // Each culvert part falls by 0.2 m: 499.7 m under the way, but 503.7 m, over the terrain, under the
    // third. The last asks a cover of 2 m: a lift of 1.7 m, past the most of 1.5 m.
    let culvert = |north: f64| River {
        points: vec![place.at(0.0, north - 5.0), place.at(0.0, north + 5.0)],
        levels: if north > 300.0 { vec![503.8, 503.6] } else { vec![499.8, 499.6] },
        half: 0.5,
        cover: 1.0,
        lift: 1.5,
        untagged_culvert: true,
    };
    let culverts = [culvert(0.0), culvert(200.0), culvert(400.0), River { cover: 2.0, ..culvert(600.0) }];
    let refs: Vec<&Feature> = features.iter().collect();
    let near = Near { culverts: culverts.iter().collect(), ..Default::default() };
    let solved = solve(&refs, &flat, &near, &table().1, None);
    let at_culvert = |f: usize| {
        let (along, z) = place.profile(&solved.ways[f][0]);
        interp(100.0, &along, &z)
    };
    assert!(at_culvert(0) > 499.7 + 1.0 - GIVE_M, "{}", at_culvert(0));
    assert!((at_culvert(1) - 500.0).abs() < 0.01, "{}", at_culvert(1));
    assert!((at_culvert(2) - 500.0).abs() < 0.01, "{}", at_culvert(2));
    assert!((at_culvert(3) - 500.0).abs() < 0.01, "{}", at_culvert(3));
    assert_eq!((solved.metrics.covers, solved.metrics.covers_lifted), (1, 1));
}

#[test]
fn a_culvert_bridge_of_a_road_holds_the_cover_over_its_water() {
    let place = engelberg();
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("residential", line(&[(-100.0, 0.0), (0.0, 0.0)]), plain),
        way("residential", line(&[(0.0, 0.0), (6.0, 0.0)]), bridge),
        way("residential", line(&[(6.0, 0.0), (100.0, 0.0)]), plain),
    ];
    let stream = River {
        points: vec![place.at(3.0, -50.0), place.at(3.0, 50.0)],
        levels: vec![499.7, 499.7],
        half: 0.5,
        cover: 1.0,
        lift: 1.5,
        untagged_culvert: true,
    };
    let solved = run(&features, &flat, &[stream]);
    assert_eq!(solved.culverts, BTreeSet::from([1]));
    let (along, z) = place.profile(&solved.ways[1][0]);
    assert!(interp(3.0, &along, &z) > 499.7 + 1.0 - GIVE_M, "{z:?}");
    assert_eq!((solved.metrics.covers, solved.metrics.covers_lifted), (1, 1));
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
    let river = River {
        points: vec![place.at(20.0, -100.0), place.at(20.0, 100.0)],
        levels: vec![499.0, 499.0],
        half: 1.0,
        cover: 1.0,
        lift: 1.5,
        untagged_culvert: true,
    };
    let solved = run(&features, &flat, &[river]);
    assert_eq!((solved.metrics.clearances, solved.metrics.clearances_low_dm), (2, 0));
    let (along, z) = place.profile(&solved.ways[1][0]);
    assert!(interp(20.0, &along, &z) >= 499.0 + CLEAR_WATER_M + DECK_M - KEEP_M);
    let (below_along, below_z) = place.profile(&solved.ways[3][0]);
    // The row holds the deck and the road at the crossing; the tiles keep their curves within KEEP_M of it.
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
    let river = River {
        points: vec![place.at(1.0, -50.0), place.at(1.0, 50.0)],
        levels: vec![500.0, 500.0],
        half: 1.0,
        cover: 1.0,
        lift: 1.5,
        untagged_culvert: true,
    };
    let features = [path(line(&[(-50.0, 0.0), (0.0, 0.0)]), plain), path(line(&[(0.0, 0.0), (9.0, 0.0)]), bridge)];
    let solved = run(&features, &flat, &[river]);
    // The deck clears the stream, and its free end stands on the terrain.
    let (along, z) = place.profile(&solved.ways[1][0]);
    assert!(interp(1.0, &along, &z) >= least - 0.01);
    assert!(z.iter().copied().fold(f64::MIN, f64::max) < least + 0.5);
    assert!(interp(9.0, &along, &z) < 501.0);
    // As a side of a path, the bridge lands on it: 1 m on it cannot climb the clearance.
    let river = River {
        points: vec![place.at(-50.0, 1.0), place.at(50.0, 1.0)],
        levels: vec![500.0, 500.0],
        half: 1.0,
        cover: 1.0,
        lift: 1.5,
        untagged_culvert: true,
    };
    let features = [path(line(&[(-50.0, 0.0), (50.0, 0.0)]), plain), path(line(&[(0.0, 0.0), (0.0, 9.0)]), bridge)];
    let solved = run(&features, &flat, &[river]);
    assert!(solved.ways[1][0].as_ref().unwrap().z.iter().all(|&z| z < 500.05));
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
            half: 1.0,
            cover: 1.0,
            lift: 1.5,
            untagged_culvert: true,
        })
        .collect();
    let solved = run(&features, &terrain, &rivers);
    assert_eq!(solved.culverts, BTreeSet::from([4]));
    let (along, z) = place.profile(&solved.ways[1][0]);
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
    let (oa, oz) = place.profile(&solved.ways[1][0]);
    let (ua, uz) = place.profile(&solved.ways[4][0]);
    let (over, under) = (interp(50.0, &oa, &oz), interp(30.0, &ua, &uz));
    assert!(over - under >= CLEAR_ROAD_M + DECK_M - KEEP_M);
    assert!((over - 500.0).abs() < (under - 500.0).abs());
    assert_eq!((solved.metrics.passes, solved.metrics.passes_near_dm), (1, 0));
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
            let way = solved.ways[f][0].as_ref().unwrap();
            way.points.iter().zip(&way.z).map(|(&p, &z)| (place.east_north(p).0, z)).collect::<Vec<_>>()
        })
        .collect();
    let (first, last) = (vertices[0].1, vertices[vertices.len() - 1].1);
    for &(e, z) in &vertices {
        assert!((z - interp(e, &[-3000.0, 3000.0], &[first, last])).abs() < 0.01);
    }
    assert!(vertices.iter().all(|&(_, z)| z < 1001.0));
}

#[test]
fn a_tunnel_under_a_road_by_its_portals_sinks_as_far_as_its_portals_on_the_ground_let_it() {
    // On flat terrain the north portal lies just past the motorway's edge. The portals stay on the
    // ground, so the tunnel cannot lie its bore and cover under the road: it lies as deep as they let it.
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
    let (ra, rz) = place.profile(&solved.ways[0][0]);
    assert!((interp(205.0, &ra, &rz) - 500.0).abs() < 0.05);
    let (ta, tz) = place.profile(&solved.ways[2][0]);
    for north in [-20.0, -main_half, 0.0, main_half, 7.0] {
        let z = interp(north + 20.0, &ta, &tz);
        assert!((z - (500.0 - PORTAL_M)).abs() < 0.5, "{north}: {z}");
    }
}

/// Whether each vertex of the curve of a solved way lies within GROUND_M of the terrain, and the
/// curve's KEEP_M off the heights of the solve.
fn on_the_ground(place: &Place, part: &Option<Way>, terrain: &dyn Terrain) -> bool {
    let curve = place.curve(part);
    let ground = terrain.heights(&curve.points);
    curve.z.unwrap().iter().zip(ground).all(|(z, h)| (z - h).abs() <= GROUND_M + KEEP_M)
}

#[test]
fn a_road_over_a_tunnel_by_its_portal_does_not_lever_its_far_portal_off_the_terrain() {
    // A slope with a mountain over the tunnel; the road crosses 20 m from the lower portal, where the
    // tunnel's straight line lies too near it.
    let place = bridges();
    let height = |e: f64| {
        let inside = (e.clamp(-1000.0, 1000.0) + 1000.0) / 2000.0;
        500.0 + 400.0 * inside + 100.0 * (std::f64::consts::PI * inside).sin()
    };
    let terrain = place.terrain(move |e, _| height(e));
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("primary", line(&[(-1200.0, 0.0), (-1000.0, 0.0)]), plain),
        way("primary", line(&[(-1000.0, 0.0), (1000.0, 0.0)]), tunnel),
        way("primary", line(&[(1000.0, 0.0), (1200.0, 0.0)]), plain),
        way("unclassified", line(&[(-980.0, -100.0), (-980.0, 100.0)]), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    for f in [0, 2, 3] {
        assert!(on_the_ground(&place, &solved.ways[f][0], &terrain), "way {f}");
    }
}

#[test]
fn a_path_that_osm_joins_to_a_tunnel_inside_takes_its_height_there_and_does_not_move_it() {
    // A tunnel under 4 m of a hill; OSM joins a path to it at its middle.
    let place = bridges();
    let terrain = place.terrain(|e, _| if e.abs() < 995.0 { 504.0 } else { 500.0 });
    let line = |corners: &[(f64, f64)]| place.line(corners, None);
    let features = [
        way("primary", line(&[(-1200.0, 0.0), (-1000.0, 0.0)]), plain),
        way("primary", line(&[(-1000.0, 0.0), (0.0, 0.0), (1000.0, 0.0)]), tunnel),
        way("primary", line(&[(1000.0, 0.0), (1200.0, 0.0)]), plain),
        way("path", line(&[(0.0, 0.0), (0.0, 200.0)]), plain),
    ];
    let solved = run(&features, &terrain, &[]);
    let (along, z) = place.profile(&solved.ways[1][0]);
    let middle = interp(1000.0, &along, &z);
    assert!((middle - 500.0).abs() < 0.5, "{middle}");
    // Past its first segment the path lies on the ground.
    let (path_along, path_z) = place.profile(&solved.ways[3][0]);
    let past = path_along.iter().zip(&path_z).filter(|(&s, _)| s >= VERTEX_M);
    let off: Vec<f64> = past.map(|(_, &z)| z - 504.0).collect();
    assert!(off.len() > 5 && off.iter().all(|d| d.abs() <= GROUND_M + KEEP_M), "{off:?}");
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
    let z = &solved.ways[1][0].as_ref().unwrap().z;
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
    (
        features,
        vec![River {
            points: stream,
            levels: vec![990.0, 989.5],
            half: 1.0,
            cover: 1.0,
            lift: 1.5,
            untagged_culvert: true,
        }],
        terrain,
    )
}

#[test]
fn the_jobs_of_four_areas_that_each_hold_the_whole_network_stitch_into_its_one_solve_and_its_sidewalks() {
    let place = engelberg();
    let (features, rivers, terrain) = corner_network(&place);
    let refs: Vec<&Feature> = features.iter().collect();
    let areas: BTreeSet<(i64, i64)> = features.iter().flat_map(|f| &f.parts[0]).map(|&p| area::area_of(p)).collect();
    assert_eq!(areas.len(), 4);
    // Houses east of the residential way through the corner give it a sidewalk in each area.
    let houses: Vec<[f64; 2]> = features[1].parts[0]
        .iter()
        .flat_map(|&[x, y]| [-10.0, 0.0, 10.0].map(|north| [x + 15.0 * place.unit, y - north * place.unit]))
        .collect();
    let buildings = rstar::RTree::bulk_load(houses);
    let near = Near { rivers: rivers.iter().collect(), buildings: Some(&buildings), ..Default::default() };
    let mut whole = solve(&refs, &terrain, &near, &table().1, None);
    align::measure(&mut whole.ways, &whole.joins);
    assert!(whole.ways[1][0].as_ref().unwrap().plan.iter().any(|p| p.walk != 0));
    let around = Surroundings { rivers: rivers.clone(), buildings: buildings.clone(), ..Default::default() };
    let apart = solve_by_area(&refs, &[], &terrain, &around, &table().1, usize::MAX);
    assert_eq!(whole.ways, apart.ways);
    assert_eq!((whole.culverts, whole.underpasses), (apart.culverts, apart.underpasses));
    // The jobs add the metres of sidewalk in another order: they agree to the micrometre.
    let mm = |m: f64| (m * 1e6).round();
    let sums = |m: &Metrics| {
        let w = m.sidewalks;
        Metrics {
            residual_p99_m: 0.0,
            sidewalks: Walks {
                tagged_m: mm(w.tagged_m),
                built_m: mm(w.built_m),
                steep_m: mm(w.steep_m),
                bend_m: mm(w.bend_m),
                bed_m: mm(w.bed_m),
                short_m: mm(w.short_m),
            },
            ..m.clone()
        }
    };
    assert_eq!(sums(&whole.metrics), sums(&apart.metrics));
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
    let solved = solve_by_area(&refs, &context_refs, &terrain, &Surroundings::default(), &table().1, usize::MAX);
    let (along, z) = place.profile(&solved.ways[1][0]);
    for i in 0..z.len() {
        assert!((z[i] - interp(along[i], &[0.0, 6000.0], &[1000.0, 880.0])).abs() < 0.5);
    }
    let (_, cut) = context(&features, &beyond[..1], &|p| place.east_north(p).0 < 5000.0);
    assert_eq!(cut, BTreeSet::from([1]));
}

/// A stream from (east, north) to (east, north) at 499.7 m, 1 m wide.
fn stream(place: &Place, from: (f64, f64), to: (f64, f64)) -> River {
    let points = place.line(&[from, to], None);
    River { levels: vec![499.7; points.len()], points, half: 0.5, cover: 1.0, lift: 1.5, untagged_culvert: true }
}

#[test]
fn a_road_over_an_untagged_stream_holds_its_cover_and_a_ford_a_path_a_wide_river_and_a_grazing_line_do_not() {
    let place = bridges();
    let road = |class: &str, north: f64, edit: fn(&mut Feature)| {
        way(class, place.line(&[(-100.0, north), (100.0, north)], Some(20.0)), edit)
    };
    let shared = |class: &str, north: f64, edit: fn(&mut Feature)| {
        way(class, place.line(&[(-100.0, north), (0.0, north), (100.0, north)], None), edit)
    };
    let features = [
        road("residential", 0.0, plain),
        road("footway", 200.0, plain),
        way("residential", place.line(&[(-95.0, 400.0), (100.0, 400.0)], None), |f| f.ford = true),
        road("residential", 600.0, plain),
        road("residential", 800.0, plain),
        // Shares a vertex with a stream, crosses another 4 m on and a third 300 m on.
        way("residential", place.line(&[(-100.0, 1000.0), (0.0, 1000.0), (400.0, 1000.0)], None), plain),
        shared("rail", 1200.0, plain),
        shared("rail", 1400.0, plain),
        shared("residential", 1600.0, plain),
        shared("footway", 1800.0, |f| f.stones = true),
        shared("rail", 2000.0, plain),
        road("residential", 2200.0, plain),
    ];
    let across = |north: f64| stream(&place, (0.0, north - 10.0), (0.0, north + 10.0));
    let through = |north: f64| River {
        points: vec![place.at(0.0, north - 10.0), place.at(0.0, north), place.at(0.0, north + 10.0)],
        levels: vec![499.7; 3],
        ..across(north)
    };
    let rivers = [
        across(0.0),
        across(200.0),
        across(400.0),
        // 10° to the road.
        stream(&place, (-100.0, 600.0 - 17.6), (100.0, 600.0 + 17.6)),
        River { half: 2.5, untagged_culvert: false, ..across(800.0) },
        through(1000.0),
        stream(&place, (4.0, 990.0), (4.0, 1010.0)),
        stream(&place, (300.0, 990.0), (300.0, 1010.0)),
        through(1200.0),
        // Ends on the railway and on the road.
        stream(&place, (0.0, 1390.0), (0.0, 1400.0)),
        stream(&place, (0.0, 1590.0), (0.0, 1600.0)),
        through(1800.0),
        stream(&place, (20.0, 1790.0), (20.0, 1810.0)),
        // Two parts that meet on the railway.
        stream(&place, (0.0, 1990.0), (0.0, 2000.0)),
        stream(&place, (0.0, 2000.0), (0.0, 2010.0)),
        River { cover: 2.0, ..across(2200.0) },
    ];
    let solved = run(&features, &flat, &rivers);
    let at = |f: usize, east: f64| {
        let (along, z) = place.profile(&solved.ways[f][0]);
        let start = place.east_north(solved.ways[f][0].as_ref().unwrap().points[0]).0;
        interp(east - start, &along, &z)
    };
    let lifted = |z: f64| z > 499.7 + 1.0 - GIVE_M;
    let level = |z: f64, want: f64| (z - want).abs() < 0.01;
    assert!(lifted(at(0, 0.0)), "{}", at(0, 0.0));
    assert!(level(at(1, 0.0), 500.0) && level(at(3, 0.0), 500.0) && level(at(4, 0.0), 500.0));
    assert!(level(at(2, 0.0), 499.7 - FORD_M), "{}", at(2, 0.0));
    assert!(level(at(5, 0.0), 499.7 - FORD_M) && lifted(at(5, 300.0)), "{} {}", at(5, 0.0), at(5, 300.0));
    assert!(lifted(at(6, 0.0)) && level(at(7, 0.0), 500.0) && level(at(8, 0.0), 500.0));
    assert!(level(at(9, 0.0), 500.0) && level(at(9, 20.0), 500.0));
    assert!(lifted(at(10, 0.0)) && level(at(11, 0.0), 500.0));
    let m = &solved.metrics;
    assert_eq!((m.covers_untagged, m.covers_untagged_lifted, m.covers_untagged_capped, m.covers_capped), (4, 4, 1, 0));
    assert_eq!((m.covers, m.covers_lifted, m.fords), (4, 4, 2));
    assert!(m.cover_lift_top_m > 0.69 && m.cover_lift_top_m < 0.71, "{}", m.cover_lift_top_m);
}

#[test]
fn a_sidewalk_runs_on_over_an_untagged_culvert_and_not_along_a_stream_beside_it() {
    let place = bridges();
    let road = way("residential", place.line(&[(-100.0, 0.0), (300.0, 0.0)], Some(20.0)), |f| {
        (f.half, f.outer, f.sidewalks) = (2.5, 4.0, [Some(true); 2]);
    });
    let rivers = [stream(&place, (0.0, -20.0), (0.0, 20.0)), stream(&place, (100.0, 3.5), (160.0, 3.5))];
    let solved = run(&[road], &flat, &rivers);
    let way = solved.ways[0][0].as_ref().unwrap();
    // The sides of the kept segment at `east`.
    let walk = |east: f64| {
        let at = way.points.iter().rposition(|&p| place.east_north(p).0 <= east).unwrap();
        way.plan[at].walk
    };
    for east in [-5.0, -1.0, 0.0, 1.0, 5.0] {
        assert_eq!(walk(east), crate::WALK_LEFT | crate::WALK_RIGHT, "{east} m");
    }
    for east in [110.0, 130.0, 150.0] {
        assert_eq!(walk(east), crate::WALK_RIGHT, "{east} m");
    }
}

#[test]
fn a_slanted_stream_that_ends_on_a_road_vertex_is_no_crossing_and_one_through_a_railway_vertex_is_one() {
    let place = bridges();
    let features = [
        way("residential", place.line(&[(-7.0, -18.0 * 0.037), (0.0, 0.0), (7.0, 0.9)], None), plain),
        way("rail", place.line(&[(-100.0, 263.1), (0.0, 300.0), (100.0, 338.7)], None), plain),
    ];
    let rivers = [
        // The parameters of this pair miss both ends by a rounding.
        stream(&place, (-19.0 * 1.3, 61.3), (0.0, 0.0)),
        River {
            points: place.line(&[(-27.1, 247.9), (0.0, 300.0), (23.3, 352.9)], None),
            levels: vec![499.7; 3],
            ..stream(&place, (0.0, 0.0), (1.0, 1.0))
        },
    ];
    let solved = run(&features, &flat, &rivers);
    let m = &solved.metrics;
    assert_eq!((m.covers_untagged, m.covers_untagged_lifted, m.fords), (1, 1, 0));
    assert!((height_at(&solved.ways[0][0], place.at(0.0, 0.0)) - 500.0).abs() < 0.01);
    assert!(height_at(&solved.ways[1][0], place.at(0.0, 300.0)) > 499.7 + 1.0 - GIVE_M);
}
