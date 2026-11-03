use super::*;
use crate::frame::{from_lon_lat, GlobalPoint};
use crate::terrain::Terrain;
use crate::tile::{self, Alignment, Feature, Geometry, Tags};

fn cell() -> CellKey {
    CellKey::of(from_lon_lat(8.40, 46.82))
}

fn mpu() -> f64 {
    metres_per_unit(cell().origin().y + CELL_UNITS / 2)
}

/// A point at metres east and south of the cell's centre.
fn at([x, y]: [f64; 2]) -> GlobalPoint {
    let o = cell().origin();
    let half = CELL_UNITS / 2;
    GlobalPoint { x: o.x + half + libm::round(x / mpu()) as i64, y: o.y + half + libm::round(y / mpu()) as i64 }
}

/// A line feature through points in metres about the centre, at heights z (None: unknown).
#[derive(Clone)]
struct Line {
    kind: &'static str,
    tags: Vec<(&'static str, &'static str)>,
    points: Vec<[f64; 2]>,
    z: Vec<Option<f64>>,
    side: Vec<u8>,
    tangents: Option<Vec<f64>>,
}

fn line(kind: &'static str, tags: &[(&'static str, &'static str)], points: &[[f64; 2]], z: f64) -> Line {
    Line {
        kind,
        tags: tags.to_vec(),
        points: points.to_vec(),
        z: vec![Some(z); points.len()],
        side: vec![0; points.len()],
        tangents: None,
    }
}

fn road(points: &[[f64; 2]], z: f64) -> Line {
    line("road", &[("highway", "residential")], points, z)
}

fn footway(points: &[[f64; 2]], z: f64) -> Line {
    line("path", &[("highway", "footway")], points, z)
}

fn stream(tags: &[(&'static str, &'static str)], points: &[[f64; 2]], z: f64) -> Line {
    let mut all = vec![("waterway", "stream")];
    all.extend_from_slice(tags);
    line("stream", &all, points, z)
}

fn feature(l: &Line, tile: (u32, u32)) -> Feature {
    let points: Vec<GlobalPoint> = l.points.iter().map(|&p| at(p)).collect();
    let n = points.len();
    let chord = |i: usize, j: usize| {
        libm::atan2((points[j].y - points[i].y) as f64, (points[j].x - points[i].x) as f64)
            .rem_euclid(core::f64::consts::TAU)
    };
    let tangent: Vec<f64> = l.tangents.clone().unwrap_or_else(|| {
        (0..n)
            .map(|i| {
                if i == 0 {
                    chord(0, 1)
                } else if i == n - 1 {
                    chord(n - 2, n - 1)
                } else {
                    chord(i - 1, i + 1)
                }
            })
            .collect()
    });
    let water = matches!(l.kind, "river" | "stream");
    let plan = (!water).then(|| Alignment {
        along_m: 0.0,
        backward: false,
        tangent,
        grade: vec![0.0; n],
        bank: vec![0.0; n],
        widen: vec![0.0; n],
        side: l.side.clone(),
        walk: vec![0; n],
    });
    let tags: Tags = l.tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let part = tile::Part { outer: false, tile, own: vec![true; n - 1], points, heights: Some(l.z.clone()), plan };
    Feature { kind: l.kind.into(), tags, geometry: Geometry::Line, parts: vec![part] }
}

fn input(lines: &[Line]) -> CellInput {
    let tile = cell().feature_tiles()[0];
    CellInput {
        key: cell(),
        features: lines.iter().map(|l| feature(l, tile)).collect(),
        terrain: Terrain::new(cell(), Vec::new()),
    }
}

fn graph(lines: &[Line]) -> WayGraph {
    let g = build(cell(), &input(lines), Sections::embedded());
    assert_eq!(check(&g), Vec::new());
    g
}

fn class(g: &WayGraph, arm: &Arm) -> String {
    let w = &g.ways[arm.way as usize];
    Sections::embedded().rank_name(w.family == Family::Water, w.class as usize).to_string()
}

fn hosts(g: &WayGraph) -> Vec<(String, bool)> {
    let mut out: Vec<(String, bool)> = g.junctions[0].arms.iter().map(|a| (class(g, a), a.host)).collect();
    out.sort();
    out
}

/// The span metres of the bake: one 5-point Simpson with the chord in metres.
fn bake_arc(chord: [f64; 2], t0: [f64; 2], t1: [f64; 2]) -> f64 {
    let c = (chord[0] * chord[0] + chord[1] * chord[1]).sqrt();
    let speed = |u: f64| {
        let (d0, d1, d2) = (3.0 * u * u - 4.0 * u + 1.0, 6.0 * u - 6.0 * u * u, 3.0 * u * u - 2.0 * u);
        let v = [d1 * chord[0] + c * (d0 * t0[0] + d2 * t1[0]), d1 * chord[1] + c * (d0 * t0[1] + d2 * t1[1])];
        (v[0] * v[0] + v[1] * v[1]).sqrt()
    };
    (speed(0.0) + 4.0 * speed(0.25) + 2.0 * speed(0.5) + 4.0 * speed(0.75) + speed(1.0)) / 12.0
}

#[test]
fn a_span_measures_as_the_bake_and_its_metres_invert() {
    let mut l = road(&[[0.0, 0.0], [40.0, 10.0]], 0.0);
    l.tangents = Some(vec![-0.3, 0.6]);
    let g = graph(&[l]);
    let (part, span) = (&g.parts[0], &g.parts[0].spans[0]);
    let (a, b) = (&part.vertices[0], &part.vertices[1]);
    let scale = metres_per_unit((a.pos[1] + b.pos[1]).div_euclid(2));
    let chord = [(b.pos[0] - a.pos[0]) as f64 * scale, (b.pos[1] - a.pos[1]) as f64 * scale];
    let unit = |t: f64| [libm::cos(t), libm::sin(t)];
    let bake = bake_arc(chord, unit(a.tangent), unit(b.tangent));
    assert!((span.knots[KNOTS] - bake).abs() < 1e-9 * bake, "{} {bake}", span.knots[KNOTS]);
    assert!(span.knots.windows(2).all(|k| k[0] < k[1]));
    for u in [0.0, 0.013, 0.31, 0.5, 0.97, 1.0] {
        assert!((u_at(span, s_at(span, u)) - u).abs() < 1e-12, "{u}");
    }
}

#[test]
fn samples_break_at_cell_lines_and_a_grid_point_near_a_break_goes() {
    let o = cell().origin();
    // A road whose crossing with the cell's east line lies 16.03 m from its start.
    let east_m = (CELL_UNITS / 2) as f64 * mpu();
    let g = graph(&[road(&[[east_m - 16.03, 3.0], [east_m + 30.0, 3.0]], 0.0)]);
    let samples = &g.samples;
    let lines: Vec<&Sample> = samples.iter().filter(|s| s.kind == SampleKind::CellLine).collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(i64::from(lines[0].pos[0]) + o.x, o.x + CELL_UNITS, "exactly on the line");
    assert!(samples.iter().all(|s| s.kind != SampleKind::Grid || (s.s_m - lines[0].s_m).abs() >= MIN_STEP_M / 2.0));
    // A straight road through a cell corner gives one sample there.
    let corner = [east_m, east_m];
    let g = graph(&[road(&[[corner[0] - 20.0, corner[1] - 20.0], [corner[0] + 20.0, corner[1] + 20.0]], 0.0)]);
    let corners: Vec<&Sample> = g.samples.iter().filter(|s| s.kind == SampleKind::CellLine).collect();
    assert_eq!(corners.len(), 1, "{corners:?}");
    assert_eq!(corners[0].pos, [CELL_UNITS as i32, CELL_UNITS as i32]);
}

#[test]
fn cleaning_removes_spikes_and_repeated_vertices_and_flattens_folded_spans() {
    let g = graph(&[road(&[[-30.0, 0.0], [20.0, 0.0], [-10.0, 0.3], [-10.0, 30.0]], 0.0)]);
    assert_eq!((g.stats.spikes, g.parts[0].vertices.len()), (1, 3));
    let g = graph(&[road(&[[0.0, 0.0], [10.0, 0.0], [10.0, 0.0], [20.0, 0.0]], 0.0)]);
    assert_eq!((g.stats.repeated, g.parts[0].vertices.len()), (1, 3));
    let mut folded = road(&[[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]], 0.0);
    folded.tangents = Some(vec![0.0, core::f64::consts::PI, 0.0]);
    let g = graph(&[folded]);
    assert_eq!(g.stats.folded, 2);
    assert!(g.parts[0].spans.iter().all(|s| s.chord));
}

#[test]
fn a_way_across_a_tile_line_is_one_part_with_each_spans_along_from_its_owner() {
    // A cell on a zoom-13 tile line: the way runs from the west tile into the east one.
    let line_x = cell().feature_tiles().iter().map(|t| tile::tile_square(*t)[2]).min().unwrap();
    let key = CellKey::of(GlobalPoint { x: line_x, y: cell().origin().y });
    let tiles = key.feature_tiles();
    assert!(tiles.len() >= 2);
    let y = key.origin().y + 1000;
    let unit = (10.0 / metres_per_unit(y)) as i64;
    let p = |k: i64| GlobalPoint { x: line_x + k * unit + 7, y };
    let tags: Tags = [("highway".to_string(), "residential".to_string())].into();
    let copy = |tile: (u32, u32), ks: std::ops::RangeInclusive<i64>, along_m: f64| {
        let points: Vec<GlobalPoint> = ks.map(p).collect();
        let n = points.len();
        let own = tile::own(Geometry::Line, &points, &tiles, tiles.iter().position(|&t| t == tile).unwrap());
        let plan = Alignment {
            along_m,
            backward: false,
            tangent: vec![0.0; n],
            grade: vec![0.0; n],
            bank: vec![0.0; n],
            widen: vec![0.0; n],
            side: vec![0; n],
            walk: vec![0; n],
        };
        let part = tile::Part { outer: false, tile, own, points, heights: Some(vec![Some(1.0); n]), plan: Some(plan) };
        Feature { kind: "road".into(), tags: tags.clone(), geometry: Geometry::Line, parts: vec![part] }
    };
    let holding = |q: GlobalPoint| {
        *tiles.iter().find(|&&t| tile::tile_square(t)[0] <= q.x && q.x < tile::tile_square(t)[2]).unwrap()
    };
    let (west, east) = (holding(p(-1)), holding(p(1)));
    let features = vec![copy(west, -4..=1, 0.0), copy(east, -1..=4, 30.1)];
    let g = build(key, &CellInput { key, features, terrain: Terrain::new(key, Vec::new()) }, Sections::embedded());
    assert_eq!((g.parts.len(), g.joints.len()), (1, 0));
    let along: Vec<f64> = g.parts[0].spans.iter().map(|s| s.along0_m).collect();
    // The west tile owns the four spans before the line and the one that meets it.
    assert!((along[3] - 30.0).abs() < 0.01 && (along[4] - 40.1).abs() < 0.01, "{along:?}");
}

#[test]
fn a_footway_on_a_roads_vertices_loses_the_run_in_either_direction() {
    for reversed in [false, true] {
        let mut walk = vec![[-30.0, -10.0], [-10.0, 0.0], [10.0, 0.0], [30.0, -10.0]];
        if reversed {
            walk.reverse();
        }
        let g = graph(&[road(&[[-40.0, 0.0], [-10.0, 0.0], [10.0, 0.0], [40.0, 0.0]], 0.0), footway(&walk, 0.0)]);
        assert_eq!(g.stats.shared_runs, 1);
        assert_eq!(g.junctions.len(), 2, "{reversed}");
        assert_eq!(g.ways.iter().filter(|w| w.family == Family::Path).count(), 2);
    }
}

#[test]
fn hosts_follow_side_then_the_fallback_rule() {
    // A side road whose end carries its side bit: the through road hosts.
    let mut side = road(&[[0.0, 40.0], [0.0, 0.0]], 0.0);
    side.side = vec![0, SIDE_BACK];
    let g = graph(&[road(&[[-40.0, 0.0], [0.0, 0.0], [40.0, 0.0]], 0.0), side]);
    assert_eq!(g.junctions[0].host_rule, HostRule::Side);
    assert_eq!(hosts(&g), [("residential".into(), false), ("residential".into(), true), ("residential".into(), true)]);
    let straight = |g: &WayGraph| {
        let h: Vec<&Arm> = g.junctions[0].arms.iter().filter(|a| a.host).collect();
        h[0].heading[0] * h[1].heading[0] + h[0].heading[1] * h[1].heading[1]
    };
    // No side: the pair most nearly straight on.
    let g = graph(&[
        road(&[[-40.0, 0.0], [0.0, 0.0]], 0.0),
        road(&[[0.0, 0.0], [40.0, 3.0]], 0.0),
        road(&[[0.0, 0.0], [10.0, 40.0]], 0.0),
    ]);
    assert_eq!(g.junctions[0].host_rule, HostRule::Fallback);
    assert!(straight(&g) < -0.99);
    // A road that ends where two footways run on is the only host.
    let g = graph(&[
        road(&[[-40.0, 0.0], [0.0, 0.0]], 0.0),
        footway(&[[0.0, 0.0], [40.0, 0.0]], 0.0),
        footway(&[[0.0, 0.0], [0.0, 40.0]], 0.0),
    ]);
    assert_eq!(hosts(&g), [("footway".into(), false), ("footway".into(), false), ("residential".into(), true)]);
    // Footways alone: the straight pair.
    let g = graph(&[
        footway(&[[-40.0, 0.0], [0.0, 0.0]], 0.0),
        footway(&[[0.0, 0.0], [40.0, 0.0]], 0.0),
        footway(&[[0.0, 0.0], [0.0, 40.0]], 0.0),
    ]);
    assert!(straight(&g) < -0.99);
    // A footway that ends on a street vertex, and one across it at a shared vertex: the street hosts.
    for walk in [vec![[0.0, 0.0], [0.0, 40.0]], vec![[0.0, -40.0], [0.0, 0.0], [0.0, 40.0]]] {
        let g = graph(&[road(&[[-40.0, 0.0], [0.0, 0.0], [40.0, 0.0]], 0.0), footway(&walk, 0.0)]);
        assert!(g.junctions[0].arms.iter().all(|a| a.host == (class(&g, a) == "residential")));
    }
}

#[test]
fn ends_snap_by_the_table() {
    // A residential road, 2.5 m half-width, along x; a footway end beside it.
    let street = || road(&[[-50.0, 0.0], [0.0, 0.0], [50.0, 0.0]], 0.0);
    let end = |y: f64, z: f64| footway(&[[0.0, -30.0], [0.0, y]], z);
    let links = |g: &WayGraph| (g.stats.links_carriageway, g.stats.links_end, g.stats.link_crossed);
    for (what, lines, expect) in [
        ("0.9 m from the edge, 0.4 m up", vec![street(), end(-3.4, 0.4)], (1, 0, 0)),
        ("1.1 m from the edge", vec![street(), end(-3.6, 0.0)], (0, 0, 0)),
        ("0.9 m from the edge, 0.6 m up", vec![street(), end(-3.4, 0.6)], (0, 0, 0)),
        ("inside the carriageway", vec![street(), end(-1.0, 0.0)], (1, 0, 0)),
        (
            "two ends 5 cm apart",
            vec![footway(&[[-30.0, -20.0], [0.0, -20.0]], 0.0), footway(&[[0.05, -20.0], [30.0, -20.0]], 0.0)],
            (0, 1, 0),
        ),
        (
            "an end near its own way along the graph",
            vec![street(), footway(&[[0.0, 0.0], [0.0, -8.0], [6.0, -8.0], [6.0, -3.0]], 0.0)],
            (0, 0, 0),
        ),
        (
            "a link across a stream",
            vec![street(), end(-3.4, 0.0), stream(&[], &[[-20.0, -2.0], [20.0, -2.0]], 0.0)],
            (0, 0, 1),
        ),
    ] {
        let g = graph(&lines);
        assert_eq!(links(&g), expect, "{what}");
        if expect.0 == 1 {
            let j = g.junctions.iter().find(|j| j.arms.iter().any(|a| g.ways[a.way as usize].link.is_some())).unwrap();
            assert!(j.arms.iter().all(|a| a.host == (g.ways[a.way as usize].link.is_none())), "{what}");
        }
    }
    // A foot within `snap_m` of a junction of the street attaches there.
    let g = graph(&[street(), road(&[[0.0, 0.0], [0.0, 40.0]], 0.0), footway(&[[0.6, -30.0], [0.6, -3.0]], 0.0)]);
    assert_eq!(g.stats.links_carriageway, 1);
    assert_eq!(g.junctions.len(), 1);
    assert_eq!(g.junctions[0].arms.len(), 4);
}

/// The one crossing of two lines, as (kind, source), or None.
fn crossing(lines: &[Line]) -> Option<(CrossingKind, Source)> {
    let g = graph(lines);
    assert!(g.crossings.len() <= 1, "{:?}", g.crossings);
    g.crossings.first().map(|c| (c.kind, c.source))
}

#[test]
fn crossings_take_the_first_row_of_the_table() {
    use CrossingKind::*;
    let across = |z: f64| road(&[[0.0, -40.0], [0.0, 40.0]], z);
    let along = |kind, tags: &[(&'static str, &'static str)], z| line(kind, tags, &[[-40.0, 0.0], [40.0, 0.0]], z);
    let at_angle = |deg: f64, l: Line| {
        let (c, s) = (libm::cos(deg.to_radians()) * 40.0, libm::sin(deg.to_radians()) * 40.0);
        line(l.kind, &l.tags, &[[-c, -s], [c, s]], l.z.first().copied().flatten().unwrap_or(0.0))
    };
    let bridge = [("highway", "residential"), ("bridge", "yes"), ("deck", "1")];
    // An open stream, a culvert of 2 × half metres under the road, and the open stream again.
    let culvert = |half: f64| {
        let pipe = Line { z: vec![None, None], ..stream(&[("tunnel", "culvert")], &[[-half, 0.0], [half, 0.0]], 0.0) };
        vec![
            across(0.0),
            stream(&[], &[[-40.0, 0.0], [-half, 0.0]], 0.0),
            pipe,
            stream(&[], &[[half, 0.0], [40.0, 0.0]], 0.0),
        ]
    };
    type Row = (&'static str, Vec<Line>, Option<(CrossingKind, Source)>);
    let rows: Vec<Row> = vec![
        (
            "a deck",
            vec![
                line("road", &bridge, &[[0.0, -40.0], [0.0, 40.0]], 6.0),
                along("road", &[("highway", "residential")], 0.0),
            ],
            Some((Deck, Source::Tag)),
        ),
        ("a short culvert", culvert(5.0), Some((Culvert, Source::Tag))),
        ("a buried stream", culvert(35.0), Some((Culvert, Source::Fallback))),
        (
            "a ford",
            vec![
                line("road", &[("highway", "track"), ("ford", "yes")], &[[0.0, -40.0], [0.0, 40.0]], 0.0),
                stream(&[], &[[-40.0, 0.0], [40.0, 0.0]], 0.0),
            ],
            Some((Ford, Source::Tag)),
        ),
        (
            "a road along a stream",
            vec![across(0.0), at_angle(75.0, stream(&[], &[[0.0, 0.0]], 0.0))],
            Some((Stacked, Source::Fallback)),
        ),
        (
            "a road over a stream",
            vec![across(0.0), stream(&[], &[[-40.0, 0.0], [40.0, 0.0]], 0.0)],
            Some((Culvert, Source::Rule)),
        ),
        (
            "a road over a river",
            vec![across(0.0), along("river", &[("waterway", "river")], 0.0)],
            Some((Deck, Source::Rule)),
        ),
        (
            "a path over a ditch",
            vec![
                footway(&[[0.0, -40.0], [0.0, 40.0]], 0.0),
                stream(&[("width", "0.5")], &[[-40.0, 0.0], [40.0, 0.0]], 0.0),
            ],
            Some((PipeInStones, Source::Rule)),
        ),
        (
            "a path over a stream",
            vec![footway(&[[0.0, -40.0], [0.0, 40.0]], 0.0), stream(&[], &[[-40.0, 0.0], [40.0, 0.0]], 0.0)],
            Some((SteppingStones, Source::Rule)),
        ),
        (
            "a path over a brook",
            vec![
                footway(&[[0.0, -40.0], [0.0, 40.0]], 0.0),
                stream(&[("width", "5")], &[[-40.0, 0.0], [40.0, 0.0]], 0.0),
            ],
            Some((Footbridge, Source::Rule)),
        ),
        (
            "a road over a railway",
            vec![across(0.2), along("railway", &[("railway", "rail")], 0.0)],
            Some((LevelCrossing, Source::Rule)),
        ),
        (
            "a road over a road",
            vec![across(5.0), along("road", &[("highway", "residential")], 0.0)],
            Some((Deck, Source::Rule)),
        ),
        (
            "a road just over a road",
            vec![across(2.0), along("road", &[("highway", "residential")], 0.0)],
            Some((Stacked, Source::Fallback)),
        ),
        (
            "a road over a wide river at 20°",
            vec![across(0.0), at_angle(70.0, along("river", &[("waterway", "river"), ("width", "30")], 0.0))],
            Some((Stacked, Source::Fallback)),
        ),
        (
            "a road over a road at 19°",
            vec![across(5.0), at_angle(71.0, along("road", &[("highway", "residential")], 0.0))],
            None,
        ),
        (
            "a road over a stream at 19°",
            vec![across(0.0), at_angle(71.0, stream(&[], &[[0.0, 0.0]], 0.0))],
            Some((Stacked, Source::Fallback)),
        ),
        (
            "a road and a railway at a shared vertex",
            vec![
                road(&[[0.0, -40.0], [0.0, 0.0], [0.0, 40.0]], 0.0),
                line("railway", &[("railway", "rail")], &[[-40.0, 0.0], [0.0, 0.0], [40.0, 0.0]], 0.0),
            ],
            Some((LevelCrossing, Source::SharedVertex)),
        ),
    ];
    for (what, lines, expect) in rows {
        assert_eq!(crossing(&lines), expect, "{what}");
    }
    // Two roads at one height: an implicit junction, and a synthetic deck splits the upper road.
    let g = graph(&[across(0.0), along("road", &[("highway", "residential")], 0.2)]);
    assert!(
        g.crossings.is_empty() && g.junctions.len() == 1 && g.junctions[0].implicit && g.junctions[0].arms.len() == 4
    );
    let g = graph(&[across(5.0), along("road", &[("highway", "residential")], 0.0)]);
    let deck = &g.decks[0];
    assert!(deck.synthetic && g.ways[deck.ways[0] as usize].level == Level::Deck);
    assert_eq!(g.joints.len(), 2, "the deck's two ends");
}
