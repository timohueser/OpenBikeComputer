//! The heights on synthetic cells: each case's ways on a plane of terrain.

use super::check::{border, scan, terrain_at, Class, Scan};
use super::{height_at, height_at_sides, Kind, Step};
use crate::faces::records::{find, records};
use crate::faces::tests::{at, cell, input_of, line, mpu, road, Line};
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
    let words = records(&graph, &input, rules, key, 0, true).words;
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
        .filter(|st| matches!(st.class, Class::Named(Step::CutWins | Step::Unrelated)))
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

#[test]
fn a_road_across_a_sixty_degree_hillside_walls_the_rest_of_its_fill() {
    let grade = libm::tan(60f64.to_radians());
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]])], &|_, y| 500.0 + grade * y);
    let s = c.scan();
    assert_eq!(count(&s, Class::Inside), 0);
    assert_eq!(count(&s, Class::Unnamed), 0);
    // The fill side is north: the road's edge steps down by a wall there.
    let walls: Vec<f64> = (s.steps.iter())
        .filter(|st| st.class == Class::Named(Step::NoRoom) && st.a[1] < c.q([0.0, 0.0])[1])
        .map(|st| st.dh.abs())
        .collect();
    assert!(walls.len() > 500, "{} walls", walls.len());
    assert!(walls.iter().all(|&d| d > 1.0), "{walls:?}");
}

#[test]
fn a_rail_beside_a_road_two_metres_up_meets_it_in_a_retaining_wall() {
    let mut rail = line("railway", &[("railway", "rail")], &[[-300.0, 6.0], [300.0, 6.0]]);
    rail.z = vec![502.0; 2];
    let c = ways(&[road(&[[-300.0, 0.0], [300.0, 0.0]]), rail], &|_, _| 500.0);
    let s = c.scan();
    assert_eq!(count(&s, Class::Unnamed), 0);
    let walls: Vec<f64> =
        s.steps.iter().filter(|st| st.class == Class::Named(Step::RailRoad)).map(|st| st.dh.abs()).collect();
    assert!(walls.len() > 500 && walls.iter().all(|&d| d > 1.0), "{} walls", walls.len());
}

/// A hairpin of one way whose legs run 7 m apart, 2 m apart in height.
fn hairpin(legs: &str) -> Built {
    let plan = include_str!("../../../../../data/trailview/plan.json").replacen(
        r#""legs": "ramp""#,
        &format!(r#""legs": "{legs}""#),
        1,
    );
    let rules = Rules::parse(Sections::embedded().clone(), &plan).unwrap();
    let mut way = road(&[[-200.0, -3.5], [100.0, -3.5], [103.5, 0.0], [100.0, 3.5], [-200.0, 3.5]]);
    way.z = vec![500.0, 500.0, 501.0, 502.0, 502.0];
    built(cell(), &[way], &|_, _| 501.0, &rules)
}

#[test]
fn two_legs_of_one_way_meet_in_a_ramp_or_in_a_wall_by_the_rule_legs() {
    let between = |s: &Scan, c: &Built| {
        let [x0, y0] = c.q([-150.0, -1.0]);
        let [x1, y1] = c.q([0.0, 1.0]);
        s.steps.iter().filter(|st| (x0..x1).contains(&st.a[0]) && (y0..y1).contains(&st.a[1])).count()
    };
    let ramp = hairpin("ramp");
    assert_eq!(between(&ramp.scan(), &ramp), 0);
    let wall = hairpin("wall");
    let s = wall.scan();
    assert!(between(&s, &wall) > 200, "{} steps between the legs", between(&s, &wall));
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
