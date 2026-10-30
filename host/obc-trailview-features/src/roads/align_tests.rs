//! The alignment of the road solver on synthetic ways.

use super::tests::{bridge, engelberg, plain, run, table, way, Place, KEEP_M};
use super::*;
use crate::tiles::{rounded_grade, rounded_tangent};
use crate::{SIDE_BACK, SIDE_ON};

const TAU: f64 = std::f64::consts::TAU;

fn flat(_: f64, _: f64) -> f64 {
    500.0
}

fn unwrapped(angle: f64) -> f64 {
    (angle + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0
}

/// A road of ways `length` metres long along a sine `metres` high and `wave` long, heading east, of
/// the `classes` in turn. `flip` tells which ways OSM draws to the west.
fn wavy(
    place: &Place,
    from: f64,
    (ways, length): (usize, f64),
    (metres, wave): (f64, f64),
    classes: &[&str],
    flip: impl Fn(usize) -> bool,
) -> Vec<Feature> {
    let point = |e: f64| (e, metres * (e / wave * TAU).sin());
    (0..ways)
        .map(|w| {
            let start = from + w as f64 * length;
            let mut corners: Vec<(f64, f64)> = (0..=8).map(|k| point(start + k as f64 / 8.0 * length)).collect();
            if flip(w) {
                corners.reverse();
            }
            way(classes[w % classes.len()], place.line(&corners, None), plain)
        })
        .collect()
}

#[test]
fn a_stroke_runs_on_with_one_tangent_where_osm_splits_it_and_stays_near_its_line() {
    let place = engelberg();
    let flip = |mut feature: Feature| {
        feature.parts[0].reverse();
        feature
    };
    // A bend of 20° at a vertex that two ways share, one of them drawn against the other.
    let (west, east) =
        (place.line(&[(-200.0, 0.0), (0.0, 0.0)], Some(20.0)), place.line(&[(0.0, 0.0), (188.0, 68.0)], Some(20.0)));
    let features = [way("tertiary", west, plain), flip(way("tertiary", east, plain))];
    let solved = run(&features, &flat, &[]);
    let (a, b) = (solved.ways[0][0].as_ref().unwrap(), solved.ways[1][0].as_ref().unwrap());
    let (end, start) = (a.points.len() - 1, b.points.len() - 1);
    assert_eq!(a.points[end], b.points[start], "the two ways still meet");
    assert_ne!(a.points[end], place.at(0.0, 0.0), "the corner is rounded");
    assert!(unwrapped(a.plan[end].tangent - b.plan[start].tangent + TAU / 2.0).abs() < 1e-12, "one tangent");
    let (e, n) = place.east_north(a.points[end]);
    assert!(e.hypot(n) <= 1.0 + 1e-6, "within the class's tolerance");
    assert_eq!(solved.joins, [[(0, 0, true), (1, 0, true)]]);
}

#[test]
fn a_corner_sharper_than_60_degrees_ends_the_stroke_and_a_change_of_class_does_not() {
    let place = engelberg();
    let run_of = |classes: [&str; 2], turn: (f64, f64)| {
        let features = [
            way(classes[0], place.line(&[(-200.0, 0.0), (0.0, 0.0)], Some(20.0)), plain),
            way(classes[1], place.line(&[(0.0, 0.0), turn], Some(20.0)), plain),
        ];
        run(&features, &flat, &[]).joins.len()
    };
    assert_eq!(run_of(["tertiary", "residential"], (190.0, 60.0)), 1);
    assert_eq!(run_of(["tertiary", "tertiary"], (60.0, 190.0)), 0);
}

#[test]
fn a_junction_a_deck_and_the_end_of_an_approach_stay_where_osm_has_them() {
    let place = engelberg();
    let line = |corners: &[(f64, f64)]| place.line(corners, Some(10.0));
    let features = [
        way("tertiary", line(&[(-300.0, 0.0), (0.0, 0.0), (300.0, 60.0)]), plain),
        way("residential", line(&[(0.0, 0.0), (5.0, 100.0)]), plain),
        way("tertiary", line(&[(300.0, 60.0), (340.0, 60.0), (380.0, 70.0)]), bridge),
    ];
    let solved = run(&features, &flat, &[]);
    let ways: Vec<&Way> = solved.ways.iter().map(|w| w[0].as_ref().unwrap()).collect();
    assert!(ways[0].points.contains(&place.at(0.0, 0.0)));
    assert!(ways[1].points.contains(&place.at(0.0, 0.0)));
    assert!(ways[2].points.contains(&place.at(340.0, 60.0)));
    // The first vertex of the side road after its junction is the end of its approach.
    let (e, n) = place.east_north(ways[1].points[1]);
    assert!((e.hypot(n) - 4.0).abs() < 0.1 && (e - n * 0.05).abs() < 1e-6, "{e} {n}");
}

/// The solve of ways of (class, corners), and per way its `side` at the vertex on `at`.
fn sides_at(ways: &[(&str, &[(f64, f64)])], at: (f64, f64)) -> Vec<u8> {
    let place = engelberg();
    let features: Vec<Feature> =
        ways.iter().map(|(class, corners)| way(class, place.line(corners, Some(10.0)), plain)).collect();
    let solved = run(&features, &flat, &[]);
    let side = |w: &Option<Way>| {
        let way = w.as_ref().unwrap();
        let of = |k: usize| way.plan[k].side;
        way.points.iter().enumerate().filter(|(_, p)| **p == place.at(at.0, at.1)).fold(0, |bits, (k, _)| bits | of(k))
    };
    solved.ways.iter().map(|parts| side(&parts[0])).collect()
}

#[test]
fn a_way_out_that_no_through_pair_holds_is_a_side_and_a_path_is_never_marked() {
    let (west, east, north) = ([(-200.0, 0.0), (0.0, 0.0)], [(0.0, 0.0), (200.0, 0.0)], [(0.0, 0.0), (0.0, 200.0)]);
    let through = [(-200.0, 0.0), (0.0, 0.0), (200.0, 0.0)];
    let across = [(-100.0, -100.0), (0.0, 0.0), (100.0, -100.0)];
    let path = [(0.0, 0.0), (100.0, 100.0)];
    let ways: [(&str, &[(f64, f64)]); 4] =
        [("tertiary", &through), ("residential", &north), ("residential", &across), ("path", &path)];
    assert_eq!(sides_at(&ways, (0.0, 0.0)), [0, SIDE_ON, SIDE_BACK | SIDE_ON, 0]);
    // The same side road, drawn toward the junction.
    let toward = [(0.0, 200.0), (0.0, 0.0)];
    assert_eq!(sides_at(&[("tertiary", &through), ("residential", &toward)], (0.0, 0.0)), [0, SIDE_BACK]);
    // Two ways run on, also across a change of class and past a path.
    let two: [(&str, &[(f64, f64)]); 3] = [("tertiary", &west), ("residential", &east), ("path", &north)];
    assert_eq!(sides_at(&two, (0.0, 0.0)), [0, 0, 0]);
}

#[test]
fn a_road_runs_on_into_a_lesser_class_and_a_way_at_a_corner_gives_way_to_it() {
    let (west, east, north) = ([(-200.0, 0.0), (0.0, 0.0)], [(0.0, 0.0), (200.0, 0.0)], [(0.0, 0.0), (0.0, 200.0)]);
    let ways: [(&str, &[(f64, f64)]); 3] = [("secondary", &west), ("tertiary", &east), ("service", &north)];
    assert_eq!(sides_at(&ways, (0.0, 0.0)), [0, 0, SIDE_ON]);
    // Two tertiaries turn a corner where a residential road runs straight on.
    let ways: [(&str, &[(f64, f64)]); 3] = [("tertiary", &west), ("tertiary", &north), ("residential", &east)];
    assert_eq!(sides_at(&ways, (0.0, 0.0)), [0, SIDE_ON, 0]);
}

#[test]
fn a_road_that_ends_on_a_straight_road_of_a_lesser_class_is_its_side() {
    let (west, east, stem) = ([(-200.0, 0.0), (0.0, 0.0)], [(0.0, 0.0), (200.0, 0.0)], [(0.0, 200.0), (0.0, 0.0)]);
    let bar = [(-200.0, 0.0), (0.0, 0.0), (200.0, 0.0)];
    assert_eq!(sides_at(&[("tertiary", &stem), ("residential", &bar)], (0.0, 0.0)), [SIDE_BACK, 0]);
    let split: [(&str, &[(f64, f64)]); 3] = [("tertiary", &stem), ("residential", &west), ("residential", &east)];
    assert_eq!(sides_at(&split, (0.0, 0.0)), [SIDE_BACK, 0, 0]);
}

#[test]
fn a_track_runs_through_a_level_crossing_and_a_junction_with_no_pair_has_no_side() {
    let through = [(-200.0, 0.0), (0.0, 0.0), (200.0, 0.0)];
    let rail = [(0.0, -200.0), (0.0, 0.0), (0.0, 200.0)];
    assert_eq!(sides_at(&[("tertiary", &through), ("rail", &rail)], (0.0, 0.0)), [SIDE_BACK | SIDE_ON, 0]);
    let fan = [[(0.0, 0.0), (200.0, 0.0)], [(0.0, 0.0), (173.0, 100.0)], [(0.0, 0.0), (100.0, 173.0)]];
    let ways: [(&str, &[(f64, f64)]); 3] = [("tertiary", &fan[0]), ("tertiary", &fan[1]), ("tertiary", &fan[2])];
    assert_eq!(sides_at(&ways, (0.0, 0.0)), [0, 0, 0]);
}

#[test]
fn a_way_out_runs_toward_its_point_8_m_along_past_a_stub_of_osm() {
    // The first segment of the service road leaves the secondary at a right angle, then it runs straight on.
    let west = [(-200.0, 0.0), (0.0, 0.0)];
    let on = [(0.0, 0.0), (0.0, 1.0), (200.0, 1.0)];
    let north = [(0.0, 0.0), (-60.0, 200.0)];
    let ways: [(&str, &[(f64, f64)]); 3] = [("secondary", &west), ("service", &on), ("service", &north)];
    assert_eq!(sides_at(&ways, (0.0, 0.0)), [0, 0, SIDE_ON]);
}

#[test]
fn a_way_keeps_few_vertices_and_its_curve_holds_the_terrain_it_follows() {
    let place = engelberg();
    let hill = place.terrain(|e, _| 500.0 + 6.0 * (e / 400.0 * TAU).sin());
    let road = wavy(&place, 0.0, (1, 800.0), (8.0, 400.0), &["secondary"], |_| false).remove(0);
    let road = Feature { parts: vec![place.line(&[(0.0, 0.0), (800.0, 0.0)], Some(10.0))], ..road };
    let solved = run(&[road], &hill, &[]);
    let way = solved.ways[0][0].as_ref().unwrap();
    assert!(way.points.len() < 12, "{} of 81 vertices", way.points.len());
    let (along, z) = place.profile(&solved.ways[0][0]);
    for (s, z) in along.iter().zip(&z) {
        assert!((z - hill.height(place.at(*s, 0.0))).abs() < KEEP_M, "at {s}");
    }
}

#[test]
fn a_tight_bend_banks_to_its_inside_and_widens_away_from_a_junction() {
    let place = engelberg();
    // A left turn of 150° of a radius of 15 m, far from any junction.
    let mut corners = vec![(-300.0, 0.0)];
    corners.extend((0..=15).map(|k| {
        let a = k as f64 * 10f64.to_radians();
        (15.0 * a.sin(), 15.0 * (1.0 - a.cos()))
    }));
    let last = corners[corners.len() - 1];
    let turned = 150f64.to_radians();
    corners.push((last.0 + 300.0 * turned.cos(), last.1 + 300.0 * turned.sin()));
    let solved = run(&[way("secondary", place.line(&corners, Some(10.0)), plain)], &flat, &[]);
    let way = solved.ways[0][0].as_ref().unwrap();
    let apex = (0..way.points.len()).max_by(|&a, &b| way.plan[a].widen.total_cmp(&way.plan[b].widen)).unwrap();
    let plan = way.plan[apex];
    // Its left is the inside, so a bank rises to the right. The class allows 6.5 %.
    assert!(plan.bank < -0.03 && plan.bank > -0.0651, "{}", plan.bank);
    // Half of the 2 m of a full widening moves the curve, the other half stays with the half-width.
    assert!(plan.widen > 0.4 && plan.widen <= 1.0, "{}", plan.widen);
    let (first, last) = (way.plan[0], way.plan[way.plan.len() - 1]);
    assert!([first.bank, first.widen, last.bank, last.widen].iter().all(|v| v.abs() < 1e-6));
}

/// The first corner of the areas east of `place`, in metres east of it.
fn seam(place: &Place) -> f64 {
    let n = (1u64 << area::AREA_ZOOM) as f64;
    ((place.x0 * n).floor() + 1.0) / n / place.unit - place.x0 / place.unit
}

#[test]
fn two_bake_areas_meet_within_2_cm_and_half_a_degree_across_classes_and_ways_against_the_stroke() {
    let place = engelberg();
    let (ways, length) = (24, 250.0);
    let from = seam(&place) - ways as f64 / 2.0 * length;
    // Vertices close enough that a curve between two of them is the fit.
    let (_, mut tight) = table();
    tight.rows.iter_mut().for_each(|row| row.keep_m = 0.005);
    // A job runs a stroke in the direction of the first way that it holds: these two give the two
    // jobs opposite directions, one way or the other.
    for odd in [0, 1] {
        let features =
            wavy(&place, from, (ways, length), (12.0, 330.0), &["secondary", "residential", "tertiary", "path"], |w| {
                w % 2 == odd
            });
        let refs: Vec<&Feature> = features.iter().collect();
        let apart = solve_by_area(&refs, &[], &flat, &[], &tight, usize::MAX);
        let mut whole = solve(&refs, &flat, &[], &tight, None);
        align::measure(&mut whole.ways, &whole.joins);
        for (f, way) in apart.ways.iter().enumerate() {
            let (way, one) = (way[0].as_ref().unwrap(), whole.ways[f][0].as_ref().unwrap());
            for k in 0..way.points.len() {
                let p = way.points[k];
                let (mut off, mut heading) = (f64::INFINITY, 0.0);
                for i in 0..one.points.len() - 1 {
                    let (a, b) = (one.points[i], one.points[i + 1]);
                    let (d, e) = ([b[0] - a[0], b[1] - a[1]], [p[0] - a[0], p[1] - a[1]]);
                    let t = ((e[0] * d[0] + e[1] * d[1]) / (d[0] * d[0] + d[1] * d[1])).clamp(0.0, 1.0);
                    let gap = (e[0] - t * d[0]).hypot(e[1] - t * d[1]) / place.unit;
                    if gap < off {
                        let turn = unwrapped(one.plan[i + 1].tangent - one.plan[i].tangent);
                        (off, heading) = (gap, one.plan[i].tangent + t * turn);
                    }
                }
                assert!(off < 0.01, "odd {odd}, way {f}, vertex {k}: {off} m");
                let turn = unwrapped(way.plan[k].tangent - heading).abs().to_degrees();
                assert!(turn < 0.5, "odd {odd}, way {f}, vertex {k}: {turn}°");
            }
        }
        // The metres along run on through the stroke of all the ways.
        let end = |f: usize, last: bool| {
            let plan = &apart.ways[f][0].as_ref().unwrap().plan;
            if last != (f % 2 == odd) {
                plan[plan.len() - 1].along
            } else {
                plan[0].along
            }
        };
        assert!((1..ways).all(|f| (end(f, false) - end(f - 1, true)).abs() < 1e-6));
        let total = end(ways - 1, true).max(end(0, false));
        assert!(total > ways as f64 * length * 1.01, "the length of the curve, not of its east: {total}");
    }
}

/// The distance from `point` to the polyline `line`, in metres.
fn off_line(place: &Place, point: [f64; 2], line: &[[f64; 2]]) -> f64 {
    let gap = |a: [f64; 2], b: [f64; 2]| {
        let (d, e) = ([b[0] - a[0], b[1] - a[1]], [point[0] - a[0], point[1] - a[1]]);
        let t = ((e[0] * d[0] + e[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-30)).clamp(0.0, 1.0);
        (e[0] - t * d[0]).hypot(e[1] - t * d[1]) / place.unit
    };
    line.windows(2).map(|s| gap(s[0], s[1])).fold(f64::INFINITY, f64::min)
}

#[test]
fn the_ways_over_a_seam_keep_their_tolerances_though_the_two_jobs_round_apart() {
    let place = engelberg();
    let (_, classes) = table();
    let (_, mut tight) = table();
    tight.rows.iter_mut().for_each(|row| row.keep_m = 0.005);
    for layout in 0..48 {
        // Long ways: a job holds a stroke that its neighbour cuts elsewhere, so a tangent can round apart.
        let (ways, length) = (7, 900.0);
        let from = seam(&place) - 3.5 * length + 13.0 * layout as f64;
        let shape = (5.0 + 0.25 * layout as f64, 400.0 + 11.0 * layout as f64);
        let features = wavy(&place, from, (ways, length), shape, &["secondary"], |_| false);
        let refs: Vec<&Feature> = features.iter().collect();
        let apart = solve_by_area(&refs, &[], &flat, &[], &classes, usize::MAX);
        let fit = solve(&refs, &flat, &[], &tight, None);
        for (f, way) in apart.ways.iter().enumerate() {
            let way = way[0].as_ref().unwrap();
            let fit = fit.ways[f][0].as_ref().unwrap();
            let fit = polyline(&crate::Part {
                outer: false,
                points: fit.points.clone(),
                z: Some(fit.z.clone()),
                plan: Some(fit.plan.clone()),
            })
            .points;
            let plan: Vec<Plan> = way.plan.iter().map(|p| Plan { tangent: rounded_tangent(p.tangent), ..*p }).collect();
            let curve = polyline(&crate::Part {
                outer: false,
                points: way.points.clone(),
                z: Some(way.z.clone()),
                plan: Some(plan),
            });
            for &p in &curve.points {
                let off = off_line(&place, p, &fit);
                assert!(off < 0.32, "layout {layout}, way {f}: the curve leaves the fit by {off} m");
            }
            for &p in fit.iter() {
                let sag = off_line(&place, p, &way.points);
                assert!(sag < 2.1, "layout {layout}, way {f}: a chord leaves the fit by {sag} m");
            }
        }
    }
}

#[test]
fn each_end_of_a_run_of_known_heights_stays_a_vertex() {
    let place = engelberg();
    let terrain = place.terrain(|e, _| if e < 300.0 { 500.0 } else { f64::NAN });
    let road = way("residential", place.line(&[(0.0, 0.0), (600.0, 0.0)], Some(10.0)), plain);
    let solved = run(&[road], &terrain, &[]);
    let way = solved.ways[0][0].as_ref().unwrap();
    let known: Vec<f64> =
        (0..way.z.len()).filter(|&k| !way.z[k].is_nan()).map(|k| place.east_north(way.points[k]).0).collect();
    assert!(known.len() >= 2 && known[0] < 1.0 && known[known.len() - 1] > 280.0, "{known:?}");
    // The polyline keeps the last known height: a segment to an unknown one ends where it starts.
    let (points, z, plan) = (way.points.clone(), Some(way.z.clone()), Some(way.plan.clone()));
    let curve = polyline(&crate::Part { outer: false, points, z, plan });
    assert!(curve.z.unwrap().iter().filter(|z| z.is_finite()).count() >= known.len());
}

#[test]
fn a_long_straight_holds_its_curve_as_a_tile_rounds_it() {
    let place = engelberg();
    let terrain = place.terrain(|e, n| 500.0 + 0.0137 * (e + n));
    let end = (2900.0, 2850.0);
    let road = way("secondary", place.line(&[(0.0, 0.0), end], Some(100.0)), plain);
    let solved = run(&[road], &terrain, &[]);
    let way = solved.ways[0][0].as_ref().unwrap();
    let plan: Vec<Plan> = way
        .plan
        .iter()
        .map(|p| Plan { tangent: rounded_tangent(p.tangent), grade: rounded_grade(p.grade), ..*p })
        .collect();
    let curve =
        polyline(&crate::Part { outer: false, points: way.points.clone(), z: Some(way.z.clone()), plan: Some(plan) });
    let (ex, ny) = (end.0 / end.0.hypot(end.1), end.1 / end.0.hypot(end.1));
    for (&p, z) in curve.points.iter().zip(curve.z.unwrap()) {
        let (e, n) = place.east_north(p);
        assert!((e * ny - n * ex).abs() < 0.31, "{e} {n}");
        assert!((z - 500.0 - 0.0137 * (e + n)).abs() < 0.31, "{e} {n} {z}");
    }
}

#[test]
fn a_way_out_and_back_keeps_its_far_end() {
    let place = engelberg();
    let path = way("path", place.line(&[(0.0, 0.0), (100.0, 0.0), (0.0, 0.0)], Some(10.0)), plain);
    let solved = run(&[path], &flat, &[]);
    let way = solved.ways[0][0].as_ref().unwrap();
    let far = way.points.iter().map(|&p| place.east_north(p).0).fold(0.0, f64::max);
    assert!(far > 98.0, "{far}");
}

#[test]
fn a_vertex_shared_with_a_river_stays_and_lies_on_a_whole_tile_unit() {
    let place = engelberg();
    let road = way("tertiary", place.line(&[(-200.0, 0.0), (0.0, 12.0), (200.0, 0.0)], Some(20.0)), plain);
    let shared = place.at(0.0, 12.0);
    let river = River { points: vec![place.at(0.0, -50.0), shared, place.at(0.0, 60.0)], levels: vec![f64::NAN; 3] };
    let solved = run(&[road], &flat, &[river]);
    let way = solved.ways[0][0].as_ref().unwrap();
    let k = way.points.iter().position(|&p| p == shared).expect("the vertex stays where the river has it");
    assert!(way.plan[k].whole);
    assert_eq!(way.plan.iter().filter(|p| p.whole).count(), 1);
}

#[test]
fn the_metres_along_fall_along_a_way_against_its_stroke() {
    let place = engelberg();
    let mut against = way("tertiary", place.line(&[(100.0, 0.0), (200.0, 0.0)], Some(20.0)), plain);
    against.parts[0].reverse();
    let features = [way("tertiary", place.line(&[(0.0, 0.0), (100.0, 0.0)], Some(20.0)), plain), against];
    let refs: Vec<&Feature> = features.iter().collect();
    let solved = solve_by_area(&refs, &[], &flat, &[], &table().1, usize::MAX);
    let along = |f: usize| solved.ways[f][0].as_ref().unwrap().plan.iter().map(|p| p.along).collect::<Vec<_>>();
    let (a, b) = (along(0), along(1));
    assert!((a[a.len() - 1] - 100.0).abs() < 1e-6);
    assert!((b[0] - 200.0).abs() < 1e-6 && (b[b.len() - 1] - 100.0).abs() < 1e-6);
}
