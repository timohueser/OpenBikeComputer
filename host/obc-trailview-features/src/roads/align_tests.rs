//! The alignment of the road solver on synthetic ways.

use super::tests::{bridge, engelberg, plain, run, table, way, Place, KEEP_M};
use super::*;

const TAU: f64 = std::f64::consts::TAU;

fn flat(_: f64, _: f64) -> f64 {
    500.0
}

fn unwrapped(angle: f64) -> f64 {
    (angle + TAU / 2.0).rem_euclid(TAU) - TAU / 2.0
}

/// A road of ways `length` metres long along a sine `metres` high and `wave` long, heading east.
fn wavy(place: &Place, from: f64, ways: usize, length: f64, metres: f64, wave: f64) -> Vec<Feature> {
    let point = |e: f64| (e, metres * (e / wave * TAU).sin());
    (0..ways)
        .map(|w| {
            let start = from + w as f64 * length;
            let corners: Vec<(f64, f64)> = (0..=8).map(|k| point(start + k as f64 / 8.0 * length)).collect();
            way("secondary", place.line(&corners, None), plain)
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

#[test]
fn a_way_keeps_few_vertices_and_its_curve_holds_the_terrain_it_follows() {
    let place = engelberg();
    let hill = place.terrain(|e, _| 500.0 + 6.0 * (e / 400.0 * TAU).sin());
    let road = wavy(&place, 0.0, 1, 800.0, 8.0, 400.0).remove(0);
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
fn two_bake_areas_meet_within_2_cm_and_half_a_degree_and_their_ways_run_on() {
    let place = engelberg();
    let (ways, length) = (24, 250.0);
    let from = seam(&place) - ways as f64 / 2.0 * length;
    let features = wavy(&place, from, ways, length, 12.0, 330.0);
    let refs: Vec<&Feature> = features.iter().collect();
    let apart = solve_by_area(&refs, &[], &flat, &[], &table().1, usize::MAX);
    let mut whole = run(&features, &flat, &[]);
    align::measure(&mut whole.ways, &whole.joins);
    for (f, way) in apart.ways.iter().enumerate() {
        let (way, one) = (way[0].as_ref().unwrap(), whole.ways[f][0].as_ref().unwrap());
        assert_eq!(way.points.len(), one.points.len());
        for k in 0..way.points.len() {
            let d = [way.points[k][0] - one.points[k][0], way.points[k][1] - one.points[k][1]];
            assert!(d[0].hypot(d[1]) / place.unit < 0.01, "way {f} vertex {k}");
            assert!(unwrapped(way.plan[k].tangent - one.plan[k].tangent).abs() < 0.25f64.to_radians());
            assert!((way.plan[k].along - one.plan[k].along).abs() < 0.01);
        }
    }
    // The metres along run on through the stroke of all the ways.
    let ends: Vec<f64> = apart.ways.iter().map(|w| w[0].as_ref().unwrap().plan.last().unwrap().along).collect();
    let starts: Vec<f64> = apart.ways.iter().map(|w| w[0].as_ref().unwrap().plan[0].along).collect();
    assert_eq!(starts[0], 0.0);
    assert!((1..ways).all(|f| (starts[f] - ends[f - 1]).abs() < 1e-6));
    assert!(ends[ways - 1] > ways as f64 * length * 1.01, "the length of the curve, not of its east");
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
