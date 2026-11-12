//! The heights on synthetic cells: each case's ways on a plane of terrain.

use super::check::{border, edge_steps, scan, terrain_at, Class, Scan};
use super::{height_at, height_at_sides, Kind, Step};
use crate::faces::records::{self, find};
use crate::faces::tests::{at, cell, input_of, line, main_road, mpu, road, track, Line};
use crate::faces::Rules;
use crate::frame::{CellKey, CELL_UNITS};
use crate::graph::build;
use crate::sections::Sections;
use crate::terrain::{StoredPage, Terrain, PAGE};
use crate::CellInput;

/// The terrain of a cell on every page that it reads: z of metres east and south of the centre of
/// `cell()`.
fn plane(key: CellKey, z: &dyn Fn(f64, f64) -> f64) -> Terrain {
    let centre = at([0.0, 0.0]);
    let pages = Terrain::wanted(key)
        .into_iter()
        .map(|k| {
            let s = crate::frame::FRAME_BITS - 8 - u32::from(k.level);
            let texel = |i: usize, base: u32| ((i64::from(base) << 8) + i as i64) << s | 1 << (s - 1);
            let h: Vec<f64> = (0..PAGE * PAGE)
                .map(|t| {
                    let (x, y) = (texel(t % PAGE, k.x), texel(t / PAGE, k.y));
                    z((x - centre.x) as f64 * mpu(), (y - centre.y) as f64 * mpu())
                })
                .collect();
            let (min, max) = h.iter().fold((f64::MAX, f64::MIN), |(a, b), &v| (a.min(v), b.max(v)));
            let scale = ((max - min) / 65534.0).max(1e-4);
            let codes = h.iter().map(|v| ((v - min) / scale).round() as u16).collect();
            StoredPage { level: k.level, x: k.x, y: k.y, offset: min as f32, scale: scale as f32, codes }
        })
        .collect();
    Terrain::new(key, pages)
}

struct Built {
    key: CellKey,
    words: Vec<u32>,
    input: CellInput,
}

fn built(key: CellKey, lines: &[Line], z: &dyn Fn(f64, f64) -> f64, rules: &Rules) -> Built {
    let mut input = input_of(key, lines);
    input.terrain = plane(key, z);
    let graph = build(key, &input, &rules.sections);
    let words = records::build(&graph, &input, rules, key, 0, true).near.words;
    Built { key, words, input }
}

impl Built {
    /// A point in metres east and south of the centre of `cell()`, frame units from this cell's origin.
    fn q(&self, m: [f64; 2]) -> [f64; 2] {
        let (g, o) = (at(m), self.key.origin());
        [(g.x - o.x) as f64, (g.y - o.y) as f64]
    }

    fn t(&self, q: [f64; 2]) -> f64 {
        terrain_at(&self.input.terrain, self.key, q).expect("a known terrain")
    }

    fn h(&self, m: [f64; 2]) -> super::Height {
        let q = self.q(m);
        height_at(&self.words, q, find(&self.words, q), self.t(q))
    }

    fn scan(&self) -> Scan {
        scan(&self.words, &self.input.terrain, self.key, 0.5)
    }
}

fn count(s: &Scan, class: Class) -> usize {
    s.steps.iter().filter(|st| st.class == class).count()
}

fn ways(lines: &[Line], z: &dyn Fn(f64, f64) -> f64) -> Built {
    built(cell(), lines, z, Rules::embedded())
}

#[test]
fn a_road_on_a_hillside_is_exact_in_its_core_and_meets_the_terrain_at_its_band_edge() {
    // Terrain rising 0.3 per metre to the south across a road east to west at its height.
    let z = |_: f64, y: f64| 500.0 + 0.3 * y;
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]])], &z);
    for x in [-40.0, 0.0, 33.3] {
        for y in [-2.4, -1.0, 0.0, 2.4] {
            let h = c.h([x, y]);
            assert_eq!(h.kind, Kind::Paved);
            assert!((h.h - 500.0).abs() < 1e-9, "{x} {y}: {}", h.h);
        }
    }
    let s = c.scan();
    assert_eq!(count(&s, Class::Inside), 0);
    assert_eq!(count(&s, Class::Unnamed), 0);
    // Across the road: the ground at the slope band's outer edge is the terrain's, and the toe and
    // the crest round over metres, not in a kink.
    let section: Vec<(f64, super::Height, f64)> = (-1800..=1800)
        .map(|k| {
            let y = f64::from(k) * 0.01;
            let q = c.q([0.0, y]);
            (y, c.h([0.0, y]), c.t(q))
        })
        .collect();
    let edges: Vec<&(f64, super::Height, f64)> = section
        .windows(2)
        .filter(|w| w[0].1.kind == Kind::Slope && w[1].1.kind == Kind::Terrain)
        .map(|w| &w[0])
        .collect();
    assert!(!edges.is_empty());
    for (y, h, t) in edges {
        assert!((h.h - t).abs() < 0.001, "at {y} m: {} against {t}", h.h);
    }
    // Between the cone's round foot (0.5 m past the core edge at 2.9 m) and the band fade at the band's
    // outer edge: a kink of the toe or the crest would give over 0.1 m.
    let d2 = (section.windows(51).filter(|w| (4.0..9.0).contains(&w[25].0.abs())))
        .map(|w| (w[50].1.h - 2.0 * w[25].1.h + w[0].1.h).abs())
        .fold(0.0, f64::max);
    assert!(d2 < 0.03, "{d2} m over 0.25 m");
}

#[test]
fn a_slope_leaves_its_road_edge_no_faster_than_its_cones() {
    // A road at the height of flat terrain: fill, cut and terrain meet at the core edge (2.9 m), where
    // a toe round wider than the cones' gap would move the ground off the road by centimetres.
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]])], &|_, _| 500.0);
    let slope = find(&c.words, c.q([0.0, 3.5]));
    // The bound is the road's fill cone, cone(2/3, d) = 2/3 d² / (2 round_m) with round_m 0.5.
    for d in [0.01, 0.03, 0.1] {
        let q = c.q([0.0, 2.9 + d]);
        let h = height_at(&c.words, q, slope, c.t(q));
        assert_eq!(h.kind, Kind::Slope);
        assert!((h.h - 500.0).abs() <= 0.67 * d * d + 1e-4, "{d} m past the edge: {}", h.h);
    }
    // No crease out to 2 m, past where the toe width meets the cones' gap (about 0.56 m): across a
    // kink the slope change over a sample pair stays the same as the spacing shrinks; on smooth
    // ground it shrinks with the spacing.
    let edge = c.q([0.0, 2.9]);
    let g: Vec<f64> = (0..=(2.0 / mpu()) as i32)
        .map(|n| {
            let q = [edge[0], edge[1] + f64::from(n)];
            height_at(&c.words, q, slope, c.t(q)).h
        })
        .collect();
    let bend = |s: usize| {
        (s..g.len() - s).map(|n| ((g[n + s] - 2.0 * g[n] + g[n - s]) / (s as f64 * mpu())).abs()).fold(0.0, f64::max)
    };
    let (fine, coarse) = (bend(2), bend(8));
    assert!(fine < 0.4 * coarse, "slope change {fine} at 2 units against {coarse} at 8 units");
}

#[test]
fn a_fill_that_meets_the_terrain_near_its_band_edge_fades_its_lift_with_no_crease() {
    // A road 3 m above terrain falling 0.2 per metre to the south: the fill meets the terrain within
    // a few metres of the slope band's outer edge, where the cone lets go and the toe round widens.
    let mut way = road(&[[-300.0, 0.0], [300.0, 0.0]]);
    way.z = vec![503.0; 2];
    let c = ways(&[way], &|_, y| 500.0 - 0.2 * y);
    let edge = c.q([0.0, 2.9]);
    let moved: Vec<f64> = (0..=(12.0 / mpu()) as i32)
        .map(|n| {
            let q = [edge[0], edge[1] + f64::from(n)];
            height_at(&c.words, q, find(&c.words, q), c.t(q)).h - c.t(q)
        })
        .collect();
    assert!(moved[0] > 2.5 && *moved.last().unwrap() == 0.0, "{} m to {} m", moved[0], moved.last().unwrap());
    // Past the cone's round foot: as in the test above, a crease keeps its slope change as the
    // spacing shrinks.
    let bend = |s: usize| {
        ((1.0 / mpu()) as usize..moved.len() - s)
            .map(|n| ((moved[n + s] - 2.0 * moved[n] + moved[n - s]) / (s as f64 * mpu())).abs())
            .fold(0.0, f64::max)
    };
    let (fine, coarse) = (bend(2), bend(8));
    assert!(fine < 0.4 * coarse, "slope change {fine} at 2 units against {coarse} at 8 units");
}

#[test]
fn a_guest_meets_its_host_at_the_host_core_edge_and_keeps_its_own_height_past_its_ramp() {
    let mut guest = road(&[[0.0, 0.0], [0.0, -200.0]]);
    guest.z = vec![500.4; 2];
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), guest], &|_, _| 500.0);
    assert!((c.h([-60.0, 0.0]).h - 500.0).abs() < 1e-9);
    // The records hold heights in f32.
    assert!((c.h([0.0, -60.0]).h - f64::from(500.4f32)).abs() < 1e-9);
    // Every face edge along the guest steps by under step_m.
    let mut last = find(&c.words, c.q([0.4, -1.0]));
    for k in 100..3000 {
        let q = c.q([0.4, -f64::from(k) * 0.01]);
        let l = find(&c.words, q);
        if l != last {
            let sides = height_at_sides(&c.words, q, [last, l], c.t(q));
            assert!((sides[0].h - sides[1].h).abs() < 0.001, "at {} m: {sides:?}", f64::from(k) * 0.01);
            last = l;
        }
    }
    assert_eq!(count(&c.scan(), Class::Inside), 0);
}

#[test]
fn a_path_on_a_road_edge_two_metres_up_steps_only_on_face_edges_by_rules_of_its_size() {
    // The path's core overlaps the road's; no junction relates them.
    let mut path = line("road", &[("highway", "path")], &[[-300.0, 3.6], [300.0, 3.6]]);
    path.z = vec![502.0; 2];
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), path], &|_, _| 500.0);
    let s = c.scan();
    assert_eq!(count(&s, Class::Inside), 0);
    assert_eq!(count(&s, Class::Unnamed), 0);
    let at_road: Vec<f64> = (s.steps.iter())
        .filter(|st| matches!(st.class, Class::Named(Step::Held | Step::Unrelated)))
        .map(|st| st.dh.abs())
        .filter(|d| *d > 1.9)
        .collect();
    assert!(at_road.len() > 500 && at_road.iter().all(|&d| d < 2.01), "{} steps of 2 m", at_road.len());
}

#[test]
fn a_bend_of_twelve_metres_is_continuous_and_flat() {
    let r = 12.0;
    let arc: Vec<[f64; 2]> = (0..=18)
        .map(|k| f64::from(k) * std::f64::consts::PI / 18.0)
        .map(|a| [r * libm::cos(a), -r * libm::sin(a)])
        .collect();
    let mut points = vec![[r, 200.0]];
    points.extend(arc);
    points.push([-r, 200.0]);
    let c = ways(&[road(&points)], &|_, _| 500.0);
    let s = c.scan();
    assert_eq!(count(&s, Class::Inside), 0);
    assert_eq!(count(&s, Class::Unnamed), 0);
    let mut d2: Vec<f64> = s.ripple.iter().flatten().filter(|v| !v.is_nan()).map(|v| v.abs()).collect();
    d2.sort_by(f64::total_cmp);
    assert!(d2.len() > 1000);
    assert!(d2[d2.len() * 99 / 100] <= 0.002, "p99 {} m", d2[d2.len() * 99 / 100]);
}

/// The embedded rules with one text of `plan.json` replaced.
fn rules_with(from: &str, to: &str) -> Rules {
    let plan = include_str!("../../../../../data/trailview/plan.json").replacen(from, to, 1);
    Rules::parse(Sections::embedded().clone(), &plan).unwrap()
}

#[test]
fn a_road_across_a_sixty_degree_hillside_walls_both_edges_up_to_the_highest_wall_and_goes_on_as_rock() {
    // A tertiary road on a hillside rising 60° to the south: each side needs 18 m to the terrain where
    // its cone lets go. Each edge's own fit shapes its side: a wall of the highest wall, whatever that
    // is, then a face no steeper than a rock face down to the fill's toe and up to the cut's crest.
    let grade = libm::tan(60f64.to_radians());
    for wall in [4.5, 2.0] {
        let rules = rules_with(r#""wall_max_m": 4.5"#, &format!(r#""wall_max_m": {wall}"#));
        let c = built(cell(), &[main_road(&[[-300.0, 0.0], [300.0, 0.0]])], &|_, y| 500.0 + grade * y, &rules);
        let s = c.scan();
        assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0), "{wall}");
        assert!(s.steep.is_empty(), "{wall}: {:?}", &s.steep[..s.steep.len().min(4)]);
        for x in [-100.0, 0.0, 100.0] {
            let steps: Vec<(f64, f64, f64)> =
                edges_across(&c, x, -20.0, 20.0).into_iter().filter(|e| e.1.abs() > 0.01).collect();
            // Read 5 mm off the edge, where a face at its steepest has climbed 1 cm.
            assert!(
                steps.len() == 2 && steps.iter().all(|e| (e.1 - wall).abs() < 0.02),
                "{wall} at x = {x}: {steps:?}"
            );
        }
    }
}

#[test]
fn a_track_across_a_fifty_degree_hillside_has_no_wall_and_steepens_its_slopes_up_to_a_rock_face() {
    // The fill needs more than the track's steepest fill gives over its run, which a wall class walls.
    let grade = libm::tan(50f64.to_radians());
    let c = ways(&[track(&[[-300.0, 0.0], [300.0, 0.0]])], &|_, y| 500.0 + grade * y);
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    assert!(s.steep.is_empty(), "{:?}", &s.steep[..s.steep.len().min(4)]);
    for x in [-100.0, 0.0, 100.0] {
        let steps: Vec<(f64, f64, f64)> =
            edges_across(&c, x, -20.0, 20.0).into_iter().filter(|e| e.1.abs() > 0.01).collect();
        assert!(steps.is_empty(), "at x = {x}: {steps:?}");
    }
}

#[test]
fn a_rail_beside_a_road_two_metres_up_meets_it_in_a_held_wall_at_its_own_edge() {
    let mut rail = line("railway", &[("railway", "rail")], &[[-300.0, 6.0], [300.0, 6.0]]);
    rail.z = vec![502.0; 2];
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), rail], &|_, _| 500.0);
    let s = c.scan();
    assert_eq!(count(&s, Class::Unnamed), 0);
    // The rail's fill does not count in the road's budget, so the road's cut holds the rail's bank.
    let walls: Vec<f64> =
        s.steps.iter().filter(|st| st.class == Class::Named(Step::Held)).map(|st| st.dh.abs()).collect();
    assert!(walls.len() > 500 && walls.iter().all(|&d| d > 1.0), "{} walls", walls.len());
}

/// The face edges that a section at x crosses from y0 to y1 (metres east and south): per edge its y,
/// the rise from the side before it to the side after it, and the side before it, both read there.
fn edges_across(c: &Built, x: f64, y0: f64, y1: f64) -> Vec<(f64, f64, f64)> {
    let mut out = Vec::new();
    let mut last = find(&c.words, c.q([x, y0]));
    for k in 1..=((y1 - y0) / 0.01) as i32 {
        let y = y0 + f64::from(k) * 0.01;
        let l = find(&c.words, c.q([x, y]));
        if l != last {
            let q = c.q([x, y - 0.005]);
            let [a, b] = height_at_sides(&c.words, q, [last, l], c.t(q));
            out.push((y - 0.005, b.h - a.h, a.h));
            last = l;
        }
    }
    out
}

/// A residential road 3 m up north of a road `lower` on flat terrain, parting from it: its centre runs
/// from y = −6 at x = −300 to y = −16 at x = 300. Its bank at 2:3 needs 4.75 m of room.
fn parting(lower: Line) -> Built {
    let mut upper = road(&[[-300.0, -6.0], [300.0, -16.0]]);
    upper.z = vec![503.0; 2];
    ways(&[lower, upper], &|_, _| 500.0)
}

#[test]
fn a_wall_class_road_below_another_walls_only_its_own_edge_with_a_top_that_never_jumps_and_ends_in_a_bank() {
    // The lower road's centre 0.5 m farther south, as its class is 0.5 m wider: the paved edges are
    // 0.2 m apart at x = −300.
    let south = 0.5;
    let c = parting(main_road(&[[-300.0, south], [300.0, south]]));
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    // Per x, from the upper road's centre south to the lower one's: the wall at the lower road's
    // edge with its top, and the greatest step nearer the upper road.
    let xs: Vec<f64> = (0..=1200).map(|k| -200.0 + 0.25 * f64::from(k)).collect();
    let at: Vec<(f64, f64, f64)> = (xs.iter())
        .map(|&x| {
            let y = -6.0 - 10.0 * (x + 300.0) / 600.0;
            let (mut wall, mut top, mut upper) = (0.0, 500.0, 0.0f64);
            for (e, rise, before) in edges_across(&c, x, y, south - 1.0) {
                if e > (y + south) / 2.0 && -rise > wall {
                    (wall, top) = (-rise, before);
                } else if e <= (y + south) / 2.0 {
                    upper = upper.max(rise.abs());
                }
            }
            (wall, top, upper)
        })
        .collect();
    assert!(at[0].0 > 1.5, "the wall at x = −200: {} m", at[0].0);
    let upper = at.iter().map(|a| a.2).fold(0.0, f64::max);
    assert!(upper < 0.01, "a step of {upper} m at the upper road");
    // The top is the bank budget: it changes by at most 2:3 of the metres along the edge, so the wall
    // runs down to 0 with no end face and the bank goes on past it.
    let steepest = at.windows(2).map(|w| (w[1].1 - w[0].1).abs() / 0.25).fold(0.0, f64::max);
    assert!(steepest <= 0.67 + 0.01, "the top rises {steepest} per metre");
    let end = at.iter().position(|a| a.0 < 0.001).expect("the wall ends");
    assert!(xs[end] > -100.0 && xs[end] < 0.0, "the wall ends at x = {}", xs[end]);
    assert!(at[end..].iter().all(|a| a.0 < 0.001), "a wall past the taper");
    assert!(at[end - 1].0 < 0.05, "the last wall step is {} m", at[end - 1].0);
}

#[test]
fn a_wall_class_road_far_below_another_walls_its_edge_up_to_the_highest_wall_and_climbs_on_at_its_steepest() {
    // A 1:1 hillside rising north, a tertiary road on it and a residential road 10 m farther up, its
    // paved edge 3.7 m north of the tertiary road's: its bank asks for a wall of over 6 m.
    let mut upper = road(&[[-300.0, -9.5], [300.0, -9.5]]);
    upper.z = vec![509.5; 2];
    let mut lower = main_road(&[[-300.0, 0.5], [300.0, 0.5]]);
    lower.z = vec![499.5; 2];
    let c = ways(&[lower, upper], &|_, y| 500.0 - y);
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    let most = Rules::embedded().plan.heights.wall_max_m;
    for x in [-100.0, 0.0, 100.0] {
        let steps: Vec<(f64, f64, f64)> =
            edges_across(&c, x, -9.5, 0.0).into_iter().filter(|e| e.1.abs() > 0.01).collect();
        // One step, the wall at the lower road's edge, up to its top and no higher.
        assert!(steps.len() == 1 && (-steps[0].1 - most).abs() < 0.01, "at x = {x}: {steps:?}");
        // Past its top the ground climbs at the steepest cut (2:1) and meets the upper road's bank.
        let h = |y: f64| c.h([x, y]).h;
        let rise = (1..37).map(|k| (h(-3.0 - 0.1 * f64::from(k)) - h(-2.9 - 0.1 * f64::from(k))) / 0.1);
        assert!(rise.fold(0.0, f64::max) <= 2.0 + 0.01, "steeper than the steepest cut at x = {x}");
    }
}

#[test]
fn a_way_of_a_class_without_walls_below_a_road_gets_a_bank_up_to_its_steepest_cut() {
    // A track is 0.5 m wider than a residential road: its centre 0.5 m farther south.
    let c = parting(track(&[[-300.0, 0.5], [300.0, 0.5]]));
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    // No step at the lower road's edge: its cut rises at most at its steepest cut (cut_max). Where that
    // leaves the upper road's bank no room, the step stands at the upper road's edge, held.
    for x in [-200.0, -100.0, 0.0, 100.0, 200.0] {
        let y = -6.0 - 10.0 * (x + 300.0) / 600.0;
        for (e, rise, _) in edges_across(&c, x, y, -1.0) {
            assert!(e <= (y + 0.5) / 2.0 || rise.abs() < 0.01, "a step of {rise} m at y = {e} by the track at x = {x}");
        }
    }
}

#[test]
fn a_hairpin_of_a_wall_class_a_step_high_gets_one_wall_at_its_lower_leg_from_the_upper_legs_bank() {
    let c = hairpin(TERTIARY, "ramp", 2.0);
    for x in [-150.0, -100.0, -50.0] {
        let walls: Vec<(f64, f64, f64)> =
            edges_across(&c, x, -2.0, 2.0).into_iter().filter(|e| e.1.abs() > 0.01).collect();
        // Up from the lower leg (north) to the bank of the upper one, which runs into its edge.
        assert!(walls.len() == 1 && walls[0].0 < 0.0 && (0.5..1.5).contains(&walls[0].1), "at x = {x}: {walls:?}");
    }
}

/// A way of a class and the metres off its centre where a hairpin's legs leave 1.2 m between their
/// paved edges (the half-width and the shoulder, and half the gap).
type Hairpin = (fn(&[[f64; 2]]) -> Line, f64);
const TRACK: Hairpin = (track, 4.0);
const TERTIARY: Hairpin = (main_road, 4.0);

/// A hairpin of one way whose legs run 1.2 m apart at their paved edges, `dz` metres apart in height.
fn hairpin((class, half): Hairpin, legs: &str, dz: f64) -> Built {
    let rules = rules_with(r#""legs": "ramp""#, &format!(r#""legs": "{legs}""#));
    let mut way = class(&[[-200.0, -half], [100.0, -half], [100.0 + half, 0.0], [100.0, half], [-200.0, half]]);
    way.z = vec![500.0, 500.0, 500.0 + dz / 2.0, 500.0 + dz, 500.0 + dz];
    built(cell(), &[way], &|_, _| 500.0 + dz / 2.0, &rules)
}

#[test]
fn a_hairpin_a_step_high_meets_in_named_steps_and_a_lower_one_in_its_slope_by_any_legs_rule() {
    // 2 m, a wall class: two legs, a line each, whatever the rule, and the walls stand on face edges by
    // a rule.
    for rule in ["ramp", "wall"] {
        let s = hairpin(TERTIARY, rule, 2.0).scan();
        let named = s.steps.iter().filter(|st| matches!(st.class, Class::Named(_))).count();
        assert!(named > 200, "{rule}: {named} named steps");
        assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0), "{rule}");
    }
    // 2 m of a class without walls, and 0.4 m: one line, whose two parts meet in the slope between them
    // with no step (the lower part's steepest cut climbs over the gap). The legs rule shapes the paved
    // field only: the slope reads the paved outline, and a wall needs the budget.
    for (dz, rule) in [(2.0, "ramp"), (0.4, "ramp"), (0.4, "wall")] {
        let c = hairpin(TRACK, rule, dz);
        let [x0, y0] = c.q([-150.0, -1.0]);
        let [x1, y1] = c.q([0.0, 1.0]);
        let between = |a: [f64; 2]| (x0..x1).contains(&a[0]) && (y0..y1).contains(&a[1]);
        assert_eq!(c.scan().steps.iter().filter(|st| between(st.a)).count(), 0, "{dz} {rule}");
        let edges = edge_steps(&c.words, &c.input.terrain, c.key, 0.25);
        assert_eq!(edges.iter().filter(|st| between(st.a)).count(), 0, "{dz} {rule}: {edges:?}");
    }
}

#[test]
fn a_point_on_a_cell_line_gets_the_same_bits_in_both_cells() {
    // A road across the east cell line, and one in the east cell whose segments the east bins list
    // and the west bins do not.
    let x = CELL_UNITS as f64 / 2.0 * mpu();
    let lines = [road(&[[x - 300.0, 0.0], [x + 300.0, 2.0]]), road(&[[x + 15.0, -250.0], [x + 15.0, 250.0]])];
    let z = |x: f64, y: f64| 500.0 + 0.1 * x + 0.05 * y;
    let east = CellKey { x: cell().x + 1, y: cell().y };
    let (a, b) = (ways(&lines, &z), built(east, &lines, &z, Rules::embedded()));
    let (pa, pb) = (border(&a.words, &a.input.terrain, a.key, 64), border(&b.words, &b.input.terrain, b.key, 64));
    let pb: std::collections::BTreeMap<_, _> = pb.into_iter().collect();
    let shared: Vec<_> = pa.iter().filter_map(|(k, v)| pb.get(k).map(|w| (v, w))).collect();
    assert!(shared.len() > 3000, "{} shared points", shared.len());
    assert!(shared.iter().all(|(v, w)| v.h == w.h && v.t == w.t));
}

#[test]
fn a_junction_region_meets_the_faces_beyond_its_edges_with_no_unnamed_step() {
    // A T junction beside a rail 6 m off and 0.8 m lower: the rail partly agrees with both roads, and
    // the region's own painter, which relates the arms, differs from the arms' faces at its edges.
    let mut rail = line("railway", &[("railway", "rail")], &[[-300.0, 6.0], [300.0, 6.0]]);
    rail.z = vec![499.2; 2];
    let lines = [road(&[[-300.0, 0.0], [300.0, 0.0]]), road(&[[0.0, 0.0], [0.0, -200.0]]), rail];
    // With no fade, the region's own painter steps on its edges.
    let plan = include_str!("../../../../../data/trailview/plan.json");
    let at = plan.find(r#""region_fade_m": "#).unwrap() + r#""region_fade_m": "#.len();
    let end = at + plan[at..].find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap();
    let none = Rules::parse(Sections::embedded().clone(), &format!("{}1e-9{}", &plan[..at], &plan[end..])).unwrap();
    assert!(count(&built(cell(), &lines, &|_, _| 500.0, &none).scan(), Class::Unnamed) > 0);
    let s = ways(&lines, &|_, _| 500.0).scan();
    assert_eq!(count(&s, Class::Unnamed), 0);
    assert_eq!(count(&s, Class::Inside), 0);
}

#[test]
fn a_level_t_junction_gets_no_berm_in_its_corners() {
    // Roads at the height of flat terrain: the fills of the two arms tie along the corner, and the
    // envelope of the fills must not rise above them.
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), road(&[[0.0, 0.0], [0.0, -200.0]])], &|_, _| 500.0);
    let mut highest = 0.0f64;
    for i in 0..200 {
        for j in 0..200 {
            let h = c.h([0.5 + f64::from(i) * 0.1, -0.5 - f64::from(j) * 0.1]);
            if h.kind == Kind::Slope {
                highest = highest.max(h.h - 500.0);
            }
        }
    }
    assert!(highest < 0.01, "the corner rises {highest} m above the terrain");
}

#[test]
fn an_upper_road_beside_a_junction_above_a_lower_road_has_no_step_at_its_own_edge() {
    // A T junction 3 m up (a through road and a side road) and a road at the terrain 8 m south. The
    // lower road's cut rises to the budget, whose greatest fill is the through road's: at that road's
    // own south edge the ground starts at the road, whatever the side road's fill adds.
    let mut through = road(&[[-300.0, 0.0], [300.0, 0.0]]);
    through.z = vec![503.0; 2];
    let mut side = road(&[[0.0, 0.0], [0.0, -200.0]]);
    side.z = vec![503.0; 2];
    let c = ways(&[through, side, road(&[[-300.0, 8.0], [300.0, 8.0]])], &|_, _| 500.0);
    let q0 = c.q([0.0, 0.0]);
    let at = |q: [f64; 2]| [(q[0] - q0[0]) * mpu(), (q[1] - q0[1]) * mpu()];
    let s = c.scan();
    let steps: Vec<_> = (s.steps.iter())
        .filter(|st| {
            let m = at(st.a);
            m[0].abs() < 8.0 && (1.0..4.5).contains(&m[1]) && st.dh.abs() > 0.01
        })
        .map(|st| (at(st.a), st.dh))
        .collect();
    assert!(steps.is_empty(), "steps at the upper road's edge: {steps:?}");
}

#[test]
fn a_rail_bed_above_a_road_gives_the_road_no_wall() {
    // A road at the terrain and a rail bed 1.2 m up beside it with no room for a 2:3 bank between: the
    // budget comes from road surfaces only, so the step stands at the rail bed, not at the road's edge.
    let mut rail = line("railway", &[("railway", "rail")], &[[-300.0, 6.0], [300.0, 6.0]]);
    rail.z = vec![501.2; 2];
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), rail], &|_, _| 500.0);
    let q0 = c.q([0.0, 0.0]);
    let at = |q: [f64; 2]| [(q[0] - q0[0]) * mpu(), (q[1] - q0[1]) * mpu()];
    let s = c.scan();
    let road_edge: Vec<_> = (s.steps.iter())
        .filter(|st| {
            let m = at(st.a);
            m[0].abs() < 100.0 && (2.0..3.5).contains(&m[1]) && st.dh.abs() > 0.05
        })
        .map(|st| (at(st.a), st.dh))
        .collect();
    assert!(road_edge.is_empty(), "steps at the road's edge: {:?}", &road_edge[..road_edge.len().min(4)]);
}

/// A stream (W = `level`) east to west through the centre of `cell()`.
fn stream(points: &[[f64; 2]], level: f64) -> Line {
    let mut s = line("stream", &[("waterway", "stream")], points);
    s.z = vec![level; points.len()];
    s
}

/// The surface code of the label at q (none for terrain).
fn surface_at(c: &Built, m: [f64; 2]) -> Option<u32> {
    find(&c.words, c.q(m)).map(|id| records::label(&c.words, id)[0] & 0xff)
}

#[test]
fn a_stream_across_a_slope_lies_level_with_a_bank_to_its_rise_and_a_dyke_where_the_terrain_is_lower() {
    // Terrain rising 0.3 per metre to the south, a stream east to west at the terrain of its line: the
    // north bank's terrain lies under the rise (W + 0.4 m), the south bank's rises over it.
    let c = ways(&[stream(&[[-300.0, 0.0], [300.0, 0.0]], 500.0)], &|_, y| 500.0 + 0.3 * y);
    let water = crate::faces::Surface::Water as u32;
    for x in [-40.0, 0.0, 33.3] {
        for y in [-0.45, -0.2, 0.0, 0.3, 0.45] {
            let h = c.h([x, y]);
            assert_eq!((surface_at(&c, [x, y]), h.kind), (Some(water), Kind::Paved), "{x} {y}");
            assert!((h.h - 500.0).abs() < 1e-9, "{x} {y}: {}", h.h);
        }
    }
    // The bank top on level ground: the cut cone from the waterline (0.5 m) meets the rise there.
    let rules = &Rules::embedded().plan.heights;
    let top = 0.5 + super::cone_reach(1.2, 0.4, rules);
    assert!((c.h([0.0, top]).h - 500.4).abs() < 0.002, "south bank top {}", c.h([0.0, top]).h);
    assert_eq!(surface_at(&c, [0.0, top - 0.05]), Some(crate::faces::Surface::Bank as u32));
    // North: up to the rise, level to the crest (0.6 m past the waterline), then a fill down to the terrain.
    let north = |y: f64| c.h([0.0, -y]).h;
    assert!((north(top) - 500.4).abs() < 0.002 && (north(1.1) - 500.4).abs() < 0.002, "{} {}", north(top), north(1.1));
    let fall = (north(1.6) - north(2.0)) / 0.4;
    assert!((0.5..=2.0).contains(&fall), "the dyke falls {fall} per metre");
    assert!((north(4.0) - c.t(c.q([0.0, -4.0]))).abs() < 0.002, "{}", north(4.0));
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    assert!(edge_steps(&c.words, &c.input.terrain, c.key, 0.25).is_empty());
}

#[test]
fn a_road_keeps_its_paved_heights_bit_for_bit_beside_a_stream_and_the_stream_its_level() {
    // A T junction 2 m over flat terrain and a stream 0.4 m under it, 5 m south of the through road and
    // past the side road's end: the painter reads only its own kind.
    let raise = |mut l: Line| {
        l.z = vec![502.0; l.z.len()];
        l
    };
    let roads = [raise(road(&[[-300.0, 0.0], [300.0, 0.0]])), raise(road(&[[0.0, 0.0], [0.0, -200.0]]))];
    let dry = ways(&roads, &|_, _| 500.0);
    let mut with = roads.to_vec();
    with.push(stream(&[[-300.0, 5.0], [300.0, 5.0]], 499.6));
    let wet = ways(&with, &|_, _| 500.0);
    let mut paved = 0;
    for i in -60..=60 {
        for j in -40..=12 {
            let m = [f64::from(i) * 0.5, f64::from(j) * 0.25];
            let (a, b) = (dry.h(m), wet.h(m));
            if a.kind == Kind::Paved && surface_at(&dry, m) != Some(crate::faces::Surface::Shoulder as u32) {
                paved += 1;
                assert_eq!((a.h.to_bits(), a.kind), (b.h.to_bits(), b.kind), "at {m:?}");
            }
        }
    }
    assert!(paved > 2000, "{paved} paved points");
    for x in [-20.0, 0.0, 15.0] {
        let h = wet.h([x, 5.0]);
        assert!(h.kind == Kind::Paved && (h.h - f64::from(499.6f32)).abs() < 1e-9, "{x}: {:?}", h);
    }
}

#[test]
fn a_crossings_approach_has_no_step_at_the_road_edge_and_a_bank_below_a_road_fill_steepens_with_no_step() {
    // A road crosses a stream at a shared vertex (the road 0.2 m under W at the vertex): within the
    // stream's reach past its bank, the road's edge has no step, as the stream's dyke counts for no road's
    // budget.
    let mut r = road(&[[-200.0, 0.0], [0.0, 0.0], [200.0, 0.0]]);
    r.z = vec![500.0, 499.8, 500.0];
    let c = ways(&[r, stream(&[[0.0, -200.0], [0.0, 0.0], [0.0, 200.0]], 500.0)], &|_, _| 500.4);
    let q0 = c.q([0.0, 0.0]);
    let at = |q: [f64; 2]| [(q[0] - q0[0]) * mpu(), (q[1] - q0[1]) * mpu()];
    let approach = |q: [f64; 2]| {
        let m = at(q);
        (1.2..5.0).contains(&m[0].abs()) && m[1].abs() < 8.0
    };
    let steps: Vec<_> = c.scan().steps.into_iter().filter(|st| approach(st.a) && st.dh.abs() > 0.01).collect();
    assert!(steps.is_empty(), "{:?}", steps.iter().map(|st| (at(st.a), st.dh, st.class)).collect::<Vec<_>>());
    assert!(edge_steps(&c.words, &c.input.terrain, c.key, 0.25).iter().all(|st| !approach(st.a)));
    // A stream 6 m north of a road 1.5 m over flat terrain: the road's fill counts for the stream's budget,
    // so the stream's south bank climbs toward it no steeper than its steepest slope, with no step.
    let mut up = road(&[[-300.0, 0.0], [300.0, 0.0]]);
    up.z = vec![501.5; 2];
    let c = ways(&[up, stream(&[[-300.0, -6.0], [300.0, -6.0]], 499.6)], &|_, _| 500.0);
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    let north = |st: &&super::check::Step| at(st.a)[1] < -3.0 && st.dh.abs() > 0.01;
    assert_eq!(s.steps.iter().filter(north).count(), 0, "steps at the stream");
    let bank: Vec<f64> = (0..40).map(|k| c.h([0.0, -5.5 + 0.05 * f64::from(k)]).h).collect();
    let steepest = bank.windows(2).map(|w| (w[1] - w[0]).abs() / 0.05).fold(0.0, f64::max);
    assert!(steepest <= 2.0 + 0.05, "{steepest} per metre");
}

#[test]
fn a_confluence_blends_in_one_region_and_two_streams_that_cross_apart_meet_in_a_named_step() {
    // Two streams join at a shared vertex at one level; elsewhere a stream crosses another with no
    // shared vertex, 0.5 m lower.
    let join = [stream(&[[-200.0, 0.0], [0.0, 0.0], [200.0, 0.0]], 500.0), stream(&[[0.0, 0.0], [0.0, -200.0]], 500.0)];
    let c = ways(&join, &|_, _| 500.4);
    let region = find(&c.words, c.q([0.0, 0.0])).map(|id| records::label(&c.words, id)[0]);
    assert_eq!(region.map(|w| (w & 0xff, w >> 24)), Some((crate::faces::Surface::Water as u32, 3)));
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    let apart = [stream(&[[-200.0, 0.0], [200.0, 0.0]], 500.0), stream(&[[0.0, -200.0], [0.0, 200.0]], 499.5)];
    let c = ways(&apart, &|_, _| 500.4);
    let s = c.scan();
    assert_eq!((count(&s, Class::Inside), count(&s, Class::Unnamed)), (0, 0));
    assert!(s.steps.iter().any(|st| st.class == Class::Named(Step::Unrelated) && (st.dh.abs() - 0.5).abs() < 0.01));
}

#[test]
fn an_edge_cone_with_a_crest_is_level_within_it_and_falls_at_its_slope_past_it() {
    let cone =
        |along: f64, out: f64, top: [f64; 2]| super::edge_cone(along, out, 4.0, top, 0.5, 1.0, [f64::NAN; 2], 2.5, 0.6);
    // Across a level edge: the top within the crest, then the slope from the crest's line.
    assert_eq!(cone(2.0, 0.4, [500.4; 2]), 500.4);
    assert!((cone(2.0, 1.6, [500.4; 2]) - 499.9).abs() < 1e-12);
    // Across a rising edge: the top at q's foot. Past a free end: the end's round cone grown by the crest.
    assert!((cone(2.0, 0.3, [500.0, 500.4]) - 500.2).abs() < 1e-12);
    assert!((cone(4.3, 0.4, [500.0, 500.4]) - 500.4).abs() < 1e-12);
    assert!((cone(4.6, 0.8, [500.0, 500.4]) - 500.2).abs() < 1e-12);
}

/// A rim whose cut points are rounded off its line by half a unit (0.8 mm) turns a little at every cut. The
/// cones of its ends cover the thin fans of those turns, so the ground beside it is the straight rim's: no
/// groove in a fill, no ridge in a cut.
#[test]
fn a_rim_bent_by_its_cuts_gives_the_ground_of_the_straight_rim() {
    let s = 0.667;
    // Metres; the slope lies on the side of −y (out of a to b on the right).
    let pts: Vec<[f64; 2]> = (0..8).map(|k| [0.158 * f64::from(k), if k % 2 == 1 { 0.0008 } else { 0.0 }]).collect();
    let dir = |k: usize| super::unit(super::sub(pts[k + 1], pts[k]));
    let turn = |a: [f64; 2], b: [f64; 2]| libm::atan2(super::cross(a, b), super::dot(a, b));
    let edges = pts.len() - 1;
    for sign in [1.0, -1.0] {
        for off in [1.0, 2.0, 5.0, 10.0] {
            for i in 0..=400 {
                let q = [0.2 + 0.002 * f64::from(i), -off];
                let ground = (0..edges)
                    .map(|k| {
                        let (u, v) = (dir(k), super::sub(q, pts[k]));
                        let len = super::norm(super::sub(pts[k + 1], pts[k]));
                        let t = [
                            if k > 0 { turn(dir(k - 1), u) } else { f64::NAN },
                            if k + 1 < edges { turn(u, dir(k + 1)) } else { f64::NAN },
                        ];
                        let out = -super::cross(u, v);
                        super::edge_cone(super::dot(v, u), out, len, [0.0; 2], s, sign, t, 2.5, 0.0)
                    })
                    .fold(-sign * f64::INFINITY, |m, h| if sign > 0.0 { m.max(h) } else { m.min(h) });
                assert!(
                    (ground + sign * s * off).abs() < 1e-3,
                    "sign {sign} at {q:?}: {ground} for {}",
                    -sign * s * off
                );
            }
        }
    }
}
