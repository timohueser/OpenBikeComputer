//! The water crossings on synthetic cells: a stream 1 m under flat terrain at 500 m, and ways over it.

use super::WaterCrossings;
use crate::faces::geom::winding;
use crate::faces::heights::check::Class;
use crate::faces::heights::tests::{count, stream, surface_at, ways, Built};
use crate::faces::records::{self, find};
use crate::faces::tests::{line, mpu, Line};
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

/// The points of a 0.25 m grid within 10 m of the centre that lie in a zone, and of them those that are water.
fn water_in(c: &Built, rings: &[Vec<[i32; 2]>]) -> (usize, usize) {
    let (mut inside, mut water) = (0, 0);
    let o = c.q([0.0, 0.0]);
    for i in -40..=40 {
        for j in -40..=40 {
            let q = [o[0] + f64::from(i) * 0.25 / mpu(), o[1] + f64::from(j) * 0.25 / mpu()];
            if winding(rings, [libm::round(2.0 * q[0]) as i64, libm::round(2.0 * q[1]) as i64]) != 0 {
                inside += 1;
                let surface = find(&c.words, q).map(|id| records::label(&c.words, id)[0] & 0xff);
                water += usize::from(surface == Some(Surface::Water as u32));
            }
        }
    }
    (inside, water)
}

#[test]
fn the_water_ends_at_a_ways_slope_band_and_meets_the_ground_there_at_its_level() {
    // A residential road (edge 2.9 m from its curve, slope band 10 m) 1.5 m over the terrain, and the same
    // road tagged a ford 0.2 m under the water: on the stream's axis the zone reaches to the slope band's
    // outer edge, 12.9 m from the curve, and the water lies past it. The ground meets the water's end at the
    // water's level, as a bank meets its sides: no step anywhere.
    let ford = across("road", &[("highway", "residential"), ("ford", "yes")], 90.0, W - 0.2);
    for (deg, way, kind) in [
        (90.0f64, residential(90.0, 501.5), (CrossingKind::Culvert, Source::Rule)),
        (45.0, residential(45.0, 501.5), (CrossingKind::Culvert, Source::Rule)),
        (90.0, ford, (CrossingKind::Ford, Source::Tag)),
    ] {
        let c = ways(&[stream(&[[0.0, -200.0], [0.0, 200.0]], W), way], &flat);
        assert_eq!(kinds(&graph_of(&c)), [kind], "{deg}°");
        assert_eq!(crossings_of(&c).zones.len(), 1, "{deg}°");
        assert_eq!(surface_at(&c, [0.0, 0.0]), Some(Surface::Carriageway as u32), "{deg}°");
        let along = 12.9 / libm::sin(deg.to_radians());
        for y in [-1.0, 1.0] {
            assert_eq!(surface_at(&c, [0.0, y * (along - 0.3)]), Some(Surface::Zone as u32), "{deg}°");
            assert_eq!(surface_at(&c, [0.0, y * (along + 0.3)]), Some(Surface::Water as u32), "{deg}°");
            let end = c.h([0.0, y * (along - 0.05)]).h - W;
            assert!(end.abs() < 0.1, "{deg}°: the ground {end} m over the water at its end");
        }
        let s = c.scan();
        assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0), "{deg}°");
    }
}

#[test]
fn a_confluence_in_a_zone_keeps_no_water_face_and_a_stream_that_ends_at_a_way_ends_at_its_band() {
    // A side stream joins 4 m south of the road's curve, inside the zone.
    let lines = [
        stream(&[[0.0, -200.0], [0.0, 4.0], [0.0, 200.0]], W),
        stream(&[[80.0, 4.0], [0.0, 4.0]], W),
        residential(90.0, 501.5),
    ];
    let c = ways(&lines, &flat);
    let wc = crossings_of(&c);
    assert_eq!(wc.zones.len(), 1);
    let (inside, water) = water_in(&c, &wc.zones[0].rings);
    assert!(inside > 100 && water == 0, "{water} of {inside} points in the zone are water");
    // A stream ends 4 m south of the road, in its slope band: a zone of the road's band on the stream's side.
    let c = ways(&[stream(&[[0.0, -200.0], [0.0, -4.0]], W), residential(90.0, 501.5)], &flat);
    let wc = crossings_of(&c);
    assert_eq!((wc.zones.len(), wc.stats.water_ends), (1, 1));
    assert_eq!(surface_at(&c, [0.0, -12.6]), Some(Surface::Zone as u32));
    assert_eq!(surface_at(&c, [0.0, -13.2]), Some(Surface::Water as u32));
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
}

#[test]
fn a_short_culvert_under_no_way_takes_a_zone_from_each_open_end() {
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
}
