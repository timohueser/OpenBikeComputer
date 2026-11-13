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

fn feature(l: &Line, tile: (u32, u32), index: u32) -> Feature {
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
    Feature { index, kind: l.kind.into(), tags, geometry: Geometry::Line, parts: vec![part] }
}

/// The input of a cell: the lines from one tile, all owned.
fn input_of(key: CellKey, lines: &[Line]) -> CellInput {
    let tile = cell().feature_tiles()[0];
    CellInput {
        key,
        features: lines.iter().zip(0..).map(|(l, index)| feature(l, tile, index)).collect(),
        terrain: Terrain::new(key, Vec::new()),
    }
}

fn graph(lines: &[Line]) -> WayGraph {
    graph_of(cell(), lines)
}

fn graph_of(key: CellKey, lines: &[Line]) -> WayGraph {
    let input = input_of(key, lines);
    let mut g = build(key, &input, Sections::embedded());
    assert_eq!(check(&g), Vec::new());
    let structures = &crate::faces::Rules::embedded().structures;
    levels(&mut g, &input, Sections::embedded(), structures);
    assert_eq!(check_levels(&g, &input, Sections::embedded(), structures), Vec::new());
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
fn a_stream_is_a_curve_through_its_vertices_with_its_levels_smooth_and_falling_along_it() {
    // Chords of 30 m and 60 m: the tangent bisects their directions and does not lean to the longer.
    let g = graph(&[Line {
        z: vec![Some(10.0), Some(8.0), Some(7.0)],
        ..stream(&[], &[[-30.0, 0.0], [0.0, 5.0], [60.0, 0.0]], 0.0)
    }]);
    let part = &g.parts[0];
    let (v, s) = (&part.vertices, &part.spans);
    assert!(s.iter().all(|s| !s.chord && s.water_z));
    let direction = |a: [i64; 2], b: [i64; 2]| libm::atan2((b[1] - a[1]) as f64, (b[0] - a[0]) as f64);
    let half = (direction(v[0].pos, v[1].pos) + direction(v[1].pos, v[2].pos)) / 2.0;
    assert!((v[1].tangent - half).abs() < 1e-12, "the bisector of its chords");
    assert!((v[0].tangent - direction(v[0].pos, v[1].pos)).abs() < 1e-12, "an end takes its one chord");
    for (span, u, vertex) in [(0, 0.0, 0), (0, 1.0, 1), (1, 0.0, 1), (1, 1.0, 2)] {
        let p = point(part, span, u);
        let at = [v[span].pos[0] as f64 + p.d[0], v[span].pos[1] as f64 + p.d[1]];
        assert!((at[0] - v[vertex].pos[0] as f64).abs() < 1e-6 && (at[1] - v[vertex].pos[1] as f64).abs() < 1e-6);
        let t = [libm::cos(v[vertex].tangent), libm::sin(v[vertex].tangent)];
        assert!((p.tangent[0] - t[0]).abs() < 1e-9 && (p.tangent[1] - t[1]).abs() < 1e-9, "{span} {u}");
        assert!((p.z_m - v[vertex].z_m).abs() < 1e-9);
    }
    // The level falls all along, and its grade has no break at the middle vertex.
    let z: Vec<f64> = (0..=40).map(|j| point(part, j / 21, (j % 21) as f64 / 20.0).z_m).collect();
    assert!(z.windows(2).all(|w| w[1] <= w[0] + 1e-12), "{z:?}");
    assert!((point(part, 0, 1.0).grade - point(part, 1, 0.0).grade).abs() < 1e-9);
    assert!(point(part, 1, 0.0).grade < 0.0, "a vertex between two falls keeps falling");
    assert!(g.samples.iter().any(|s| s.kind == SampleKind::Grid), "the sag rule refines it");
    // A turn over `curve_turn_max_deg` (60°) is a corner: both its spans are chords, the next one not, and
    // that curve leaves vertex 2 along the chord before it, so the line does not kink there.
    let g = graph(&[stream(&[], &[[-30.0, 0.0], [0.0, 0.0], [-20.0, 25.0], [-25.0, 60.0]], 0.0)]);
    let (v, s) = (&g.parts[0].vertices, &g.parts[0].spans);
    assert_eq!(s.iter().map(|s| s.chord).collect::<Vec<_>>(), [true, true, false]);
    let t = point(&g.parts[0], 2, 0.0).tangent;
    let chord = direction(v[1].pos, v[2].pos);
    assert!((t[0] - libm::cos(chord)).abs() < 1e-9 && (t[1] - libm::sin(chord)).abs() < 1e-9);
    // Spans of 300 m whose curves would leave their chords by more than `curve_off_max_m` (2 m).
    let g = graph(&[stream(&[], &[[-300.0, -60.0], [0.0, 0.0], [300.0, -60.0]], 0.0)]);
    assert!(g.parts[0].spans.iter().all(|s| s.chord));
}

#[test]
fn a_waters_end_at_a_tile_line_reads_the_tiles_next_vertex() {
    // A stream across a zoom-13 tile line, 150 m spans. The cell two west of the line reads only the
    // west tile; the cell on the line reads both.
    let line_x = cell().feature_tiles().iter().map(|t| tile::tile_square(*t)[2]).min().unwrap();
    let both = CellKey::of(GlobalPoint { x: line_x, y: cell().origin().y });
    let west = CellKey { x: both.x - 2, y: both.y };
    let unit = (150.0 / metres_per_unit(both.origin().y)) as i64;
    let p = |k: i64| GlobalPoint { x: line_x + k * unit + 7, y: both.origin().y + 1000 + (k & 1) * unit / 8 };
    let tags: Tags = [("waterway".to_string(), "stream".to_string())].into();
    let copy = |tiles: &[(u32, u32)], tile: (u32, u32), ks: std::ops::RangeInclusive<i64>| {
        let points: Vec<GlobalPoint> = ks.clone().map(p).collect();
        let at = tiles.iter().position(|&t| t == tile).unwrap();
        let own = tile::own(Geometry::Line, &points, tiles, at);
        let heights = Some(ks.map(|k| Some(100.0 - k as f64)).collect());
        let part = tile::Part { outer: false, tile, own, points, heights, plan: None };
        Feature { index: 0, kind: "stream".into(), tags: tags.clone(), geometry: Geometry::Line, parts: vec![part] }
    };
    let holding = |tiles: &[(u32, u32)], q: GlobalPoint| {
        *tiles.iter().find(|&&t| tile::tile_square(t)[0] <= q.x && q.x < tile::tile_square(t)[2]).unwrap()
    };
    let tangent_at = |key: CellKey| {
        let tiles = key.feature_tiles();
        let (w, e) = (holding(&tiles, p(-1)), tiles.iter().copied().find(|t| tile::tile_square(*t)[0] == line_x));
        let mut features = vec![copy(&tiles, w, -6..=2)];
        if let Some(e) = e {
            features.push(copy(&tiles, e, -2..=3));
        }
        let g = build(key, &CellInput { key, features, terrain: Terrain::new(key, Vec::new()) }, Sections::embedded());
        let v = g.parts.iter().flat_map(|part| &part.vertices).find(|v| v.pos == [p(0).x, p(0).y]).unwrap();
        v.tangent
    };
    assert_eq!(tangent_at(west), tangent_at(both));
}

#[test]
fn water_reads_on_through_a_vertex_where_the_map_splits_it_into_two_ways_but_not_at_a_confluence() {
    // A stream mapped as two ways that meet at (0, 0): the curve and the grade at the shared vertex are
    // those of one line, so the water turns and falls smoothly there.
    let up = Line { z: vec![Some(12.0), Some(10.0)], ..stream(&[], &[[-40.0, -10.0], [0.0, 0.0]], 0.0) };
    let down = Line { z: vec![Some(10.0), Some(9.0)], ..stream(&[("name", "Bach")], &[[0.0, 0.0], [50.0, -5.0]], 0.0) };
    let joint = |g: &WayGraph| -> Vec<(f64, f64)> {
        let o = at([0.0, 0.0]);
        (g.parts.iter().flat_map(|p| &p.vertices))
            .filter(|v| v.pos == [o.x, o.y])
            .map(|v| (v.tangent, v.grade))
            .collect()
    };
    let one = joint(&graph(&[up.clone(), down.clone()]));
    assert_eq!(one.len(), 2);
    assert!((one[0].0 - one[1].0).abs() < 1e-12 && (one[0].1 - one[1].1).abs() < 1e-12, "{one:?}");
    let whole = joint(&graph(&[Line {
        z: vec![Some(12.0), Some(10.0), Some(9.0)],
        ..stream(&[], &[[-40.0, -10.0], [0.0, 0.0], [50.0, -5.0]], 0.0)
    }]));
    assert!((one[0].0 - whole[0].0).abs() < 1e-12 && (one[0].1 - whole[0].1).abs() < 1e-12);
    // A third line that ends there makes a confluence: each line keeps its own chord.
    let side = stream(&[], &[[0.0, 0.0], [10.0, 40.0]], 0.0);
    let three = joint(&graph(&[up, down, side]));
    assert!(three.iter().all(|t| (t.0 - one[0].0).abs() > 1e-3), "{three:?}");
}

#[test]
fn cleaning_flattens_spikes_and_folded_spans_and_removes_repeated_vertices() {
    // A spike stays a vertex; its two spans are chords.
    let g = graph(&[road(&[[-30.0, 0.0], [20.0, 0.0], [-10.0, 0.3], [-10.0, 30.0]], 0.0)]);
    let chords: Vec<bool> = g.parts[0].spans.iter().map(|s| s.chord).collect();
    assert_eq!((g.stats.spikes, chords), (1, vec![true, true, false]));
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
        Feature { index: 0, kind: "road".into(), tags: tags.clone(), geometry: Geometry::Line, parts: vec![part] }
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

/// The tangent at the lower vertex of each of the two shared segments, of the part that keeps it.
fn keepers(g: &WayGraph) -> Vec<f64> {
    keepers_of(g, &[[[-30.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [30.0, 0.0]]])
}

fn keepers_of(g: &WayGraph, segments: &[[[f64; 2]; 2]]) -> Vec<f64> {
    let segments: Vec<[GlobalPoint; 2]> = segments.iter().map(|[a, b]| [at(*a), at(*b)]).collect();
    segments
        .iter()
        .map(|[a, b]| {
            let (a, b) = ([a.x, a.y], [b.x, b.y]);
            let kept: Vec<f64> = (g.ways.iter())
                .flat_map(|w| {
                    let part = &g.parts[w.part as usize];
                    (w.from.span..w.to.span.max(w.from.span + 1)).filter_map(move |k| {
                        let (p, q) = (part.vertices.get(k as usize)?, part.vertices.get(k as usize + 1)?);
                        let lower = if p.pos <= q.pos { p } else { q };
                        let mut ends = [p.pos, q.pos];
                        ends.sort();
                        (ends == [a.min(b), a.max(b)] && (w.to.span > k || w.to.u == 1.0)).then_some(lower.tangent)
                    })
                })
                .collect();
            assert_eq!(kept.len(), 1, "one part keeps each shared segment");
            kept[0]
        })
        .collect()
}

#[test]
fn a_shared_run_of_equal_ranks_is_decided_by_each_segment_alone() {
    // Two service roads on one run, with mirrored exits and their own tangents on it.
    let service = |points: &[[f64; 2]], tangents: &[f64]| {
        let mut l = line("road", &[("highway", "service")], points, 0.0);
        l.tangents = Some(tangents.to_vec());
        l
    };
    let a = |far: bool| {
        let (mut p, mut t) =
            (vec![[-60.0, -20.0], [-30.0, 0.0], [0.0, 0.0], [30.0, 0.0], [60.0, 20.0]], vec![0.6, 0.3, 0.0, 0.2, 0.5]);
        if !far {
            p.remove(0);
            t.remove(0);
        }
        service(&p, &t)
    };
    let b = service(
        &[[-60.0, 20.0], [-30.0, 0.0], [0.0, 0.0], [30.0, 0.0], [60.0, -20.0]],
        &[-0.6, -0.3, 0.05, -0.2, -0.5],
    );
    let full = keepers(&graph(&[a(true), b.clone()]));
    assert_eq!(full, keepers(&graph(&[b.clone(), a(true)])), "the input order decides nothing");
    assert_eq!(full, keepers(&graph(&[a(false), b.clone()])), "a far vertex decides nothing");
    // The same run near the east border, built by the cell and by its east neighbour.
    let shift = (CELL_UNITS / 2) as f64 * mpu() - 60.0;
    let moved = |l: &Line| Line { points: l.points.iter().map(|p| [p[0] + shift, p[1]]).collect(), ..l.clone() };
    let lines = [moved(&a(true)), moved(&b)];
    let east = CellKey { x: cell().x + 1, y: cell().y };
    let keep = |g: &WayGraph| {
        let segments = [[[shift - 30.0, 0.0], [shift, 0.0]], [[shift, 0.0], [shift + 30.0, 0.0]]];
        keepers_of(g, &segments)
    };
    assert_eq!(keep(&graph_of(cell(), &lines)), keep(&graph_of(east, &lines)), "two cells decide alike");
    // An exact duplicate the other way round: the part along the lower position first keeps it.
    let back = |l: &Line| {
        let mut r = l.clone();
        r.points.reverse();
        r.tangents = l.tangents.as_ref().map(|t| t.iter().rev().map(|a| a + core::f64::consts::PI).collect());
        r
    };
    let one = keepers(&graph(&[b.clone(), back(&b)]));
    assert_eq!(one, keepers(&graph(&[back(&b), b.clone()])));
    // The same way twice, one way: the copy of the lower position in its tile keeps every span.
    let g = graph(&[b.clone(), b.clone()]);
    assert_eq!((g.ways.len(), g.junctions.len(), g.joints.len()), (1, 0, 0));
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
    // A flight of steps with a short footway stub: the steps run through.
    let g = graph(&[
        line("path", &[("highway", "steps")], &[[-20.0, 0.0], [0.0, 0.0], [20.0, 0.0]], 0.0),
        footway(&[[0.0, 0.0], [0.0, 2.5]], 0.0),
    ]);
    assert_eq!(hosts(&g), [("footway".into(), false), ("steps".into(), true), ("steps".into(), true)]);
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
fn a_short_culvert_chain_under_a_road_opens_on_one_straight_line_and_a_long_one_stays_buried() {
    // Two pipes of `half` metres meet end to end under a road, between open water at 10 m and at 8 m.
    let pipe =
        |a: f64, b: f64| Line { z: vec![None, None], ..stream(&[("tunnel", "culvert")], &[[a, 0.0], [b, 0.0]], 0.0) };
    let chain = |half: f64| {
        graph(&[
            road(&[[-1.0, -40.0], [-1.0, 40.0]], 12.0),
            stream(&[], &[[-half - 30.0, 0.0], [-half, 0.0]], 10.0),
            pipe(-half, 0.0),
            pipe(0.0, half),
            stream(&[], &[[half, 0.0], [half + 30.0, 0.0]], 8.0),
        ])
    };
    let east = |s: &Sample| f64::from(s.pos[0] - (CELL_UNITS / 2) as i32) * mpu();
    let g = chain(25.0);
    let water: Vec<&Way> = g.ways.iter().filter(|w| w.family == Family::Water).collect();
    assert!(water.iter().all(|w| w.level == Level::Ground), "the chain of 50 m lies open");
    for w in water {
        for s in &g.samples[w.samples.start as usize..w.samples.end as usize] {
            let x = east(s).clamp(-25.0, 25.0);
            let line = 10.0 - 2.0 * (x + 25.0) / 50.0;
            assert!((s.z_m - line).abs() < 0.05, "{x} m east: {} m, the line {line} m", s.z_m);
        }
    }
    let g = chain(35.0);
    assert_eq!((g.crossings[0].kind, g.crossings[0].source), (CrossingKind::Culvert, Source::Fallback));
    let tunnels = g.ways.iter().filter(|w| w.family == Family::Water && w.level == Level::Tunnel).count();
    assert_eq!(tunnels, 2, "the chain of 70 m stays buried");
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
            "a track's ford over a river",
            vec![
                line("road", &[("highway", "track"), ("ford", "yes")], &[[0.0, -40.0], [0.0, 40.0]], 0.0),
                along("river", &[("waterway", "river")], 0.0),
            ],
            Some((Ford, Source::Tag)),
        ),
        (
            "a path's ford over a stream",
            vec![
                line("path", &[("highway", "footway"), ("ford", "yes")], &[[0.0, -40.0], [0.0, 40.0]], 0.0),
                stream(&[], &[[-40.0, 0.0], [40.0, 0.0]], 0.0),
            ],
            Some((Ford, Source::Tag)),
        ),
        (
            "a track's ford over a stream: a culvert, as the bake lays it",
            vec![
                line("road", &[("highway", "track"), ("ford", "yes")], &[[0.0, -40.0], [0.0, 40.0]], 0.0),
                stream(&[], &[[-40.0, 0.0], [40.0, 0.0]], 0.0),
            ],
            Some((Culvert, Source::Rule)),
        ),
        (
            "a track's ford over a stream at 19°",
            vec![
                line("road", &[("highway", "track"), ("ford", "yes")], &[[0.0, -40.0], [0.0, 40.0]], 0.0),
                at_angle(71.0, stream(&[], &[[0.0, 0.0]], 0.0)),
            ],
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
            "a road over a wide river at 25°",
            vec![across(0.0), at_angle(65.0, along("river", &[("waterway", "river"), ("width", "30")], 0.0))],
            Some((Stacked, Source::Fallback)),
        ),
        (
            "a bridge over a road at 15°",
            vec![
                line("road", &bridge, &[[0.0, -40.0], [0.0, 40.0]], 6.0),
                at_angle(75.0, along("road", &[("highway", "residential")], 0.0)),
            ],
            Some((Deck, Source::Tag)),
        ),
        (
            "a road that ends on a stream",
            vec![road(&[[0.0, 0.0], [0.0, 40.0]], 0.0), along("stream", &[("waterway", "stream")], 0.0)],
            None,
        ),
        (
            "the same road reversed",
            vec![road(&[[0.0, 40.0], [0.0, 0.0]], 0.0), along("stream", &[("waterway", "stream")], 0.0)],
            None,
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
    let wide = graph(&[across(0.0), at_angle(65.0, along("river", &[("waterway", "river"), ("width", "30")], 0.0))]);
    assert_eq!(wide.stats.deck_fallback, 1);
    // A deck stretch that would hold a junction of the upper road falls back.
    let g = graph(&[
        road(&[[0.0, -40.0], [0.0, -3.0], [0.0, 40.0]], 0.0),
        road(&[[0.0, -3.0], [30.0, -3.0]], 0.0),
        along("river", &[("waterway", "river")], 0.0),
    ]);
    assert_eq!((g.stats.deck_fallback, g.crossings[0].kind), (1, Stacked));
    // A deck stretch past a way's end at the work area's edge is cut there.
    let east = (work_area(cell())[2] - cell().origin().x - CELL_UNITS / 2) as f64 * mpu();
    let g = graph(&[
        road(&[[east - 60.0, 0.0], [east - 20.0, 0.0], [east + 3.0, 0.0], [east + 200.0, 0.0]], 0.0),
        line("river", &[("waterway", "river")], &[[east - 1.0, -40.0], [east - 1.0, 40.0]], 0.0),
    ]);
    assert_eq!((g.stats.deck_cut, g.crossings[0].kind, g.decks.len()), (1, Deck, 1));
    // A culvert chain runs on through the joint of two pipes: 70 m is a buried stream.
    let pipe =
        |a: f64, b: f64| Line { z: vec![None, None], ..stream(&[("tunnel", "culvert")], &[[a, 0.0], [b, 0.0]], 0.0) };
    let lines = vec![
        road(&[[-1.0, -40.0], [-1.0, 40.0]], 0.0),
        stream(&[], &[[-60.0, 0.0], [-35.0, 0.0]], 0.0),
        pipe(-35.0, 0.0),
        pipe(0.0, 35.0),
        stream(&[], &[[35.0, 0.0], [60.0, 0.0]], 0.0),
    ];
    assert_eq!(crossing(&lines), Some((Culvert, Source::Fallback)));
    // A crossing at a shared vertex reads its arms by position: a far vertex changes nothing.
    let shared = |far: bool| {
        let mut street = vec![[0.0, -40.0], [0.0, 0.0], [0.0, 40.0]];
        if far {
            street.insert(0, [-60.0, -120.0]);
        }
        let g = graph(&[
            road(&street, 0.0),
            footway(&[[-40.0, -40.0], [0.0, 0.0], [40.0, 40.0]], 0.0),
            stream(&[], &[[-40.0, 0.0], [0.0, 0.0], [40.0, 0.0]], 0.0),
        ]);
        let c = &g.crossings[0];
        (c.kind, c.source, c.angle_rad, class(&g, &g.junctions[0].arms[0]))
    };
    assert_eq!(shared(false), shared(true));
    assert_eq!((shared(false).0, shared(false).2), (Culvert, core::f64::consts::FRAC_PI_2));
    // A path that shares a vertex with a stream fords it.
    let g = graph(&[
        footway(&[[0.0, -40.0], [0.0, 0.0], [0.0, 40.0]], 0.0),
        stream(&[], &[[-40.0, 0.0], [0.0, 0.0], [40.0, 0.0]], 0.0),
    ]);
    assert_eq!((g.crossings[0].kind, g.crossings[0].source), (Ford, Source::SharedVertex));
    // A culvert's mouth on a road vertex: a culvert, as one water arm there is in a tunnel.
    let g = graph(&[
        road(&[[0.0, -40.0], [0.0, 0.0], [0.0, 40.0]], 0.0),
        stream(&[], &[[-40.0, 0.0], [0.0, 0.0]], 0.0),
        Line { z: vec![None, None], ..stream(&[("tunnel", "culvert")], &[[0.0, 0.0], [40.0, 0.0]], 0.0) },
    ]);
    assert_eq!((g.crossings[0].kind, g.crossings[0].source), (Culvert, Source::SharedVertex));
    // A curved stream: the crossing is decided on its chord and placed where the curves cross.
    let g = graph(&[
        road(&[[-30.0, -40.0], [-30.0, 60.0]], 0.0),
        stream(&[], &[[-60.0, 20.0], [0.0, -10.0], [60.0, 20.0]], 0.0),
    ]);
    let c = &g.crossings[0];
    let on = |st: &Station| {
        let part = &g.parts[st.part as usize];
        let (p, p0) = (point(part, st.span as usize, st.u), part.vertices[st.span as usize].pos);
        [(p0[0] as f64 + p.d[0]) * mpu(), (p0[1] as f64 + p.d[1]) * mpu()]
    };
    let (over, under) = (on(&c.over), on(&c.under));
    let d = [over[0] - under[0], over[1] - under[1]];
    let apart = (d[0] * d[0] + d[1] * d[1]).sqrt();
    assert!(apart < 0.05, "the stations lie where the curves cross: {apart} m apart");
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

#[test]
fn the_ladder_follows_the_room_by_the_class_rows_at_every_boundary() {
    use Step::*;
    let upper = |kind, tags: &[(&'static str, &'static str)], z| line(kind, tags, &[[0.0, -40.0], [0.0, 40.0]], z);
    let lower = |kind, tags: &[(&'static str, &'static str)], z| line(kind, tags, &[[-40.0, 0.0], [40.0, 0.0]], z);
    let bridge = [("highway", "residential"), ("bridge", "yes"), ("deck", "1")];
    let footbridge = [("highway", "footway"), ("bridge", "yes"), ("deck", "0.4")];
    let street = [("highway", "residential")];
    let foot = [("highway", "footway")];
    let rail = [("railway", "rail")];
    let tunnel = [("highway", "residential"), ("tunnel", "yes")];
    let rail_tunnel = [("railway", "rail"), ("tunnel", "yes")];
    // (what, upper, lower, step, whether the upper way ends on step 3). A residential deck over a
    // residential road: full 4.5 + 1.0, least 3.0 + 0.4; a residential bore: full 5.0 + 1.0, least
    // 3.0 + 0.3; snap 0.5 everywhere.
    type Row = (&'static str, Line, Line, Step, bool);
    let rows: Vec<Row> = vec![
        ("deck, full", upper("road", &bridge, 5.5), lower("road", &street, 0.0), Full, false),
        ("deck, thin", upper("road", &bridge, 5.45), lower("road", &street, 0.0), Thin, false),
        ("deck, least", upper("road", &bridge, 3.4), lower("road", &street, 0.0), Thin, false),
        (
            "deck, closed: the lower way of one class ends",
            upper("road", &bridge, 3.35),
            lower("road", &street, 0.0),
            Closed,
            false,
        ),
        ("deck, closed over snap", upper("road", &bridge, 0.55), lower("road", &street, 0.0), Closed, false),
        ("deck, at grade", upper("road", &bridge, 0.5), lower("road", &street, 0.0), AtGrade, false),
        ("deck, at grade under", upper("road", &bridge, -0.5), lower("road", &street, 0.0), AtGrade, false),
        ("deck, inverted", upper("road", &bridge, -0.55), lower("road", &street, 0.0), Inverted, false),
        (
            "footbridge over a railway, closed: the footway ends",
            upper("path", &footbridge, 4.95),
            lower("railway", &rail, 0.0),
            Closed,
            true,
        ),
        (
            "footbridge over a railway, least",
            upper("path", &footbridge, 5.0),
            lower("railway", &rail, 0.0),
            Thin,
            false,
        ),
        (
            "railway bridge over a road, closed: the road ends",
            upper("railway", &[("railway", "rail"), ("bridge", "yes"), ("deck", "1")], 3.0),
            lower("road", &street, 0.0),
            Closed,
            false,
        ),
        ("bore, full", upper("road", &street, 6.0), lower("tunnel", &tunnel, 0.0), Full, false),
        ("bore, thin", upper("road", &street, 5.95), lower("tunnel", &tunnel, 0.0), Thin, false),
        ("bore, least", upper("road", &street, 3.3), lower("tunnel", &tunnel, 0.0), Thin, false),
        (
            "bore, closed: the tunnel way ends",
            upper("road", &street, 3.25),
            lower("tunnel", &tunnel, 0.0),
            Closed,
            false,
        ),
        ("bore, inverted", upper("road", &street, -0.55), lower("tunnel", &tunnel, 0.0), Inverted, false),
        (
            "railway bore under a footway, closed: the railway ends",
            upper("path", &foot, 5.05),
            lower("tunnel", &rail_tunnel, 0.0),
            Closed,
            false,
        ),
    ];
    for (what, u, l, want, upper_ends) in rows {
        let g = graph(&[u, l]);
        let c = g.crossings.iter().find(|c| c.ladder.is_some()).unwrap_or_else(|| panic!("{what}: {:?}", g.crossings));
        let ladder = c.ladder.as_ref().unwrap();
        assert_eq!(ladder.step, want, "{what}: room {}", ladder.room_m);
        if matches!(want, Closed | Inverted) {
            let ends = if upper_ends { ladder.upper } else { ladder.lower };
            assert_eq!(ladder.ends, Some(ends), "{what}");
        }
    }
}

#[test]
fn a_deck_over_another_deck_lies_on_storey_2() {
    // Two residential bridges, square to each other, the upper one 6 m over the lower: room for a full deck (4.5 + 1.0).
    let bridge = [("highway", "residential"), ("bridge", "yes"), ("deck", "1")];
    let upper = line("road", &bridge, &[[0.0, -40.0], [0.0, 40.0]], 16.0);
    let lower = line("road", &bridge, &[[-40.0, 0.0], [40.0, 0.0]], 10.0);
    let g = graph(&[upper, lower]);
    let storey = super::level_tags::storeys(&g);
    let of = |feature: u32| {
        let ways = g.ways.iter().filter(|w| g.parts[w.part as usize].feature.0 == feature);
        ways.map(|w| storey[w.id as usize]).max()
    };
    assert_eq!((of(0), of(1)), (Some(2), Some(1)));
}

#[test]
fn the_room_reads_each_way_on_its_own_curve() {
    // A residential deck on a hairpin of radius 12 m over a road, both level: the room at every corner
    // of the overlap is the height difference, though the hairpin's far leg passes within 24 m.
    let arc: Vec<[f64; 2]> = (0..=12)
        .map(|i| {
            let a = core::f64::consts::PI * f64::from(i) / 12.0;
            [12.0 * libm::cos(a) - 12.0, 12.0 * libm::sin(a) - 30.0]
        })
        .collect();
    let mut points = vec![[0.0, -60.0]];
    points.extend(arc.iter().rev().map(|p| [p[0] + 24.0, -p[1] - 60.0]));
    let deck = line("road", &[("highway", "residential"), ("bridge", "yes"), ("deck", "1")], &points, 6.0);
    let road = line("road", &[("highway", "residential")], &[[-40.0, -40.0], [40.0, -40.0]], 0.0);
    let g = graph(&[deck, road]);
    for c in g.crossings.iter().filter_map(|c| c.ladder.as_ref()) {
        assert!((c.room_m - 6.0).abs() < 0.05, "room {}", c.room_m);
        assert_eq!(c.step, Step::Full);
    }
    assert!(g.crossings.iter().any(|c| c.ladder.is_some()));
}
