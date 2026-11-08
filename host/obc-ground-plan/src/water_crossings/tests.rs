//! The water crossings on synthetic cells: a stream 1 m under flat terrain at 500 m, and ways over it.

use super::WaterCrossings;
use crate::faces::geom::winding;
use crate::faces::heights::check::Class;
use crate::faces::heights::tests::{count, stream, surface_at, ways, Built};
use crate::faces::heights::{height_at, Step};
use crate::faces::records::{self, find, LINE, STONE_MOUTH};
use crate::faces::tests::{line, mpu, road, Line};
use crate::faces::{Rules, Surface};
use crate::graph::{self, CrossingKind, Source, WayGraph};

const W: f64 = 499.0;

fn flat(_: f64, _: f64) -> f64 {
    500.0
}

fn graph_of(c: &Built) -> WayGraph {
    graph::build(c.key, &c.input, &Rules::embedded().sections)
}

fn crossings_of(c: &Built) -> WaterCrossings {
    crate::faces::water_crossings(&graph_of(c), &c.input, Rules::embedded(), c.key)
}

fn kinds(g: &WayGraph) -> Vec<(CrossingKind, Source)> {
    g.crossings.iter().map(|x| (x.kind, x.source)).collect()
}

/// A way of `kind` and `tags` at height z through the centre, at `deg` degrees to a stream running south.
fn across(kind: &'static str, tags: &[(&'static str, &'static str)], deg: f64, z: f64) -> Line {
    let (s, c) = (libm::sin(deg.to_radians()), libm::cos(deg.to_radians()));
    let mut l = line(kind, tags, &[[-200.0 * s, -200.0 * c], [200.0 * s, 200.0 * c]]);
    l.z = vec![z; 2];
    l
}

fn residential(deg: f64, z: f64) -> Line {
    across("road", &[("highway", "residential")], deg, z)
}

/// The metres of a point (frame units) from the line through the centre at `deg` degrees to the stream.
fn off_line(c: &Built, p: [i32; 2], deg: f64) -> f64 {
    let o = c.q([0.0, 0.0]);
    let (x, y) = ((f64::from(p[0]) - o[0]) * mpu(), (f64::from(p[1]) - o[1]) * mpu());
    let (s, k) = (libm::sin(deg.to_radians()), libm::cos(deg.to_radians()));
    (x * k - y * s).abs()
}

#[test]
fn a_road_fill_over_a_stream_takes_a_zone_to_its_fill_line_and_meets_the_water_in_headwalls() {
    // A residential road (edge 2.9 m from its curve) 1.5 m over the terrain: its fill (2/3) falls to the
    // pipe's need, the bed plus a 0.7 m pipe and 0.25 m of cover (499.65 m), at 1.85 / (2/3) + 0.25 m (its
    // round foot) = 3.03 m past the edge: the scan's step 3.1 m, so the zone ends 3.0 m past it on both sides.
    let road_only = ways(&[residential(90.0, 501.5)], &flat);
    for deg in [90.0, 45.0, 20.5] {
        let c = ways(&[stream(&[[0.0, -200.0], [0.0, 200.0]], W), residential(deg, 501.5)], &flat);
        assert_eq!(kinds(&graph_of(&c)), [(CrossingKind::Culvert, Source::Rule)]);
        let wc = crossings_of(&c);
        assert_eq!((wc.zones.len(), wc.zones[0].mouths.len()), (1, 2), "{deg}°");
        for m in &wc.zones[0].mouths {
            let off = off_line(&c, m.0, deg);
            assert!((off - 5.9).abs() < 0.1, "{deg}°: a mouth {off} m off the road");
        }
        // On the stream's axis: the zone up to the mouth line, the water past it, a headwall between.
        let along = 5.9 / libm::sin(deg.to_radians());
        for y in [-1.0, 1.0] {
            assert_eq!(surface_at(&c, [0.0, y * (along - 0.3)]), Some(Surface::Zone as u32), "{deg}°");
            assert_eq!(surface_at(&c, [0.0, y * (along + 0.3)]), Some(Surface::Water as u32), "{deg}°");
            assert!(c.h([0.0, y * (along - 0.05)]).h - W >= 0.65, "{deg}°: the headwall's top");
        }
        let s = c.scan();
        assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0), "{deg}°");
        assert!(count(&s, Class::Named(Step::Headwall)) > 0, "{deg}°");
        // The water gives no slope in the zone: there the ground is the road's alone.
        if deg == 90.0 {
            for y in [3.5, 4.5, 5.5] {
                let (a, b) = (c.h([0.3, y]).h, road_only.h([0.3, y]).h);
                assert!((a - b).abs() < 0.001, "{y} m: {a} with the stream, {b} without");
            }
        }
    }
}

#[test]
fn a_road_at_the_water_is_a_ford_and_a_confluence_in_a_zone_keeps_no_water_face() {
    let low = ways(&[stream(&[[0.0, -200.0], [0.0, 200.0]], W), residential(90.0, W - 0.1)], &flat);
    let g = graph_of(&low);
    assert_eq!((kinds(&g), g.stats.culvert_to_ford), (vec![(CrossingKind::Ford, Source::Fallback)], 1));
    let wc = crossings_of(&low);
    assert_eq!((wc.zones.len(), wc.wets.len()), (0, 1));
    // A side stream joins 4 m south of the road's curve, inside the zone (to 5.9 m).
    let lines = [
        stream(&[[0.0, -200.0], [0.0, 4.0], [0.0, 200.0]], W),
        stream(&[[80.0, 4.0], [0.0, 4.0]], W),
        residential(90.0, 501.5),
    ];
    let c = ways(&lines, &flat);
    let wc = crossings_of(&c);
    assert_eq!(wc.zones.len(), 1);
    let zone = &wc.zones[0];
    assert_eq!(zone.mouths.len(), 3, "a mouth for each water way that leaves the zone");
    let (mut inside, mut water) = (0, 0);
    let o = c.q([0.0, 0.0]);
    for i in -40..=40 {
        for j in -40..=40 {
            let q = [o[0] + f64::from(i) * 0.25 / mpu(), o[1] + f64::from(j) * 0.25 / mpu()];
            if winding(&zone.rings, [libm::round(2.0 * q[0]) as i64, libm::round(2.0 * q[1]) as i64]) != 0 {
                inside += 1;
                let surface = find(&c.words, q).map(|id| records::label(&c.words, id)[0] & 0xff);
                water += usize::from(surface == Some(Surface::Water as u32));
            }
        }
    }
    assert!(inside > 100 && water == 0, "{water} of {inside} points in the zone are water");
}

#[test]
fn a_fords_wet_face_runs_to_where_the_road_rises_out_of_the_water_and_a_dry_ford_has_none() {
    // A road at a shared vertex 0.2 m under the water, rising 5 % each way: under the water for 4 m.
    let mut r = road(&[[-200.0, 0.0], [-10.0, 0.0], [0.0, 0.0], [10.0, 0.0], [200.0, 0.0]]);
    r.z = vec![W + 9.3, W + 0.3, W - 0.2, W + 0.3, W + 9.3];
    let c = ways(&[r, stream(&[[0.0, -200.0], [0.0, 0.0], [0.0, 200.0]], W)], &|_, _| W + 0.4);
    let wc = crossings_of(&c);
    assert_eq!((wc.wets.len(), wc.stats.ford_long), (1, 0));
    let road_label = find(&c.words, c.q([-30.0, 0.0]));
    let (mut most, mut least) = (f64::NEG_INFINITY, f64::INFINITY);
    for p in wc.wets[0].rings.iter().flatten() {
        let q = [f64::from(p[0]), f64::from(p[1])];
        let t = c.t(q);
        let gap = height_at(&c.words, q, road_label, t).h - height_at(&c.words, q, find(&c.words, q), t).h;
        (most, least) = (most.max(gap), least.min(gap.abs()));
    }
    assert!(most <= 0.05, "the road stands {most} m over the water at a vertex of the wet face");
    assert!(least < 0.005, "the wet face's tips lie {least} m off the water");
    assert_eq!(surface_at(&c, [0.0, 1.0]), Some(Surface::Water as u32));
    assert_eq!(surface_at(&c, [6.0, 1.0]), Some(Surface::Carriageway as u32));
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed), s.water_off.len()), (0, 0, 0));
    // A road tagged a ford that lies over the water: no part of it is wet.
    let mut dry = line("road", &[("highway", "residential"), ("ford", "yes")], &[[-200.0, 0.0], [200.0, 0.0]]);
    dry.z = vec![W + 0.3; 2];
    let c = ways(&[dry, stream(&[[0.0, -200.0], [0.0, 200.0]], W)], &|_, _| W + 0.4);
    let wc = crossings_of(&c);
    assert_eq!((wc.wets.len(), wc.stats.ford_dry), (0, 1));
}

#[test]
fn an_untagged_path_over_a_stream_has_a_pipe_in_stones_and_a_short_culvert_under_no_way_two_headwalls() {
    let c =
        ways(&[stream(&[[0.0, -200.0], [0.0, 200.0]], W), across("path", &[("highway", "path")], 80.0, 501.0)], &flat);
    assert_eq!(kinds(&graph_of(&c)), [(CrossingKind::PipeInStones, Source::Rule)]);
    // The path's edge lies 0.6 m from its curve (its finish's cap), the fill line 1.7 m past it.
    let l = records::label(&c.words, find(&c.words, c.q([0.0, 1.5])).unwrap());
    assert_eq!(l[0] & 0xff, Surface::Zone as u32);
    assert_ne!(c.words[c.words[6] as usize + LINE * l[2] as usize + 1] & STONE_MOUTH, 0, "a stone mouth");
    // A 2 m culvert part under no way: its open ends' zones cover it, and the water lies on both sides.
    let mut tunnel = line("stream", &[("waterway", "stream"), ("tunnel", "culvert")], &[[0.0, -1.0], [0.0, 1.0]]);
    tunnel.z = vec![W; 2];
    let c = ways(&[stream(&[[0.0, -200.0], [0.0, -1.0]], W), tunnel, stream(&[[0.0, 1.0], [0.0, 200.0]], W)], &flat);
    let wc = crossings_of(&c);
    assert_eq!((wc.stats.chain_ends, wc.zones.iter().filter(|z| z.way.is_none()).count()), (2, 2));
    assert_eq!(surface_at(&c, [0.0, 0.0]), Some(Surface::Zone as u32));
    for y in [-1.5, 1.5] {
        assert_eq!(surface_at(&c, [0.0, y]), Some(Surface::Water as u32));
    }
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    assert!(count(&s, Class::Named(Step::Headwall)) >= 2);
}
