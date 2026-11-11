//! The water crossings over every cell of the TrailView fixture: no water lies in a zone, a zone meets the
//! water and its banks with no step, a zone stays within `REACH_M` of its seed, and two neighbour cells build
//! the same crossings within 20 m of their shared line.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::heights::check::{edge_steps, EDGE_GRID_M};
use obc_ground_plan::faces::records::{self, find};
use obc_ground_plan::faces::{water_crossings, Rules, Surface};
use obc_ground_plan::frame::{metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::graph;
use obc_ground_plan::water_crossings::REACH_M;

/// Two neighbour cells give the same crossings within this of their shared line.
const SHARED_M: f64 = 20.0;

fn fixture_cells(fixture: &Fixture) -> Vec<CellKey> {
    let dir = default_dir();
    (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some()))
        .collect()
}

/// Whether q (cell units) lies in rings that are a union (even-odd).
fn inside(rings: &[Vec<[i32; 2]>], q: [f64; 2]) -> bool {
    let mut odd = false;
    for r in rings {
        for k in 0..r.len() {
            let (a, b) = (r[k].map(f64::from), r[(k + 1) % r.len()].map(f64::from));
            if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                odd = !odd;
            }
        }
    }
    odd
}

#[test]
fn every_zone_holds_no_water_meets_water_with_no_step_stays_near_its_seed_and_agrees_across_cells() {
    let mut fixture = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    let cells = fixture_cells(&fixture);
    assert!(cells.len() > 100, "{} cells", cells.len());
    let out = fixture.map(&cells, |key, input| {
        let g = graph::build(key, &input, &rules.sections);
        let wc = water_crossings(&g, &input, rules, key);
        let words = records::near(&g, &input, rules, key, 0, true).1.words;
        let surface = |l: Option<u32>| l.map_or(Surface::Terrain as u32, |id| records::label(&words, id)[0] & 0xff);
        let o = key.origin();
        let mpu = metres_per_unit(o.y + CELL_UNITS / 2);
        let c = CELL_UNITS as f64;
        let (mut water, mut far, mut zones) = (0, 0, 0);
        for z in &wc.zones {
            let seed = [(z.seed[0] - o.x) as f64, (z.seed[1] - o.y) as f64];
            let pts = || z.rings.iter().flatten().map(|p| p.map(f64::from));
            far += pts().filter(|p| libm::hypot(p[0] - seed[0], p[1] - seed[1]) * mpu > REACH_M).count();
            let (lo, hi) = pts().fold(([c, c], [0.0f64, 0.0f64]), |(lo, hi), p| {
                ([lo[0].min(p[0]), lo[1].min(p[1])], [hi[0].max(p[0]), hi[1].max(p[1])])
            });
            let step = 0.5 / mpu;
            let (mut x, mut n) = (lo[0].max(0.0), 0);
            while x < hi[0].min(c) {
                let mut y = lo[1].max(0.0);
                while y < hi[1].min(c) {
                    if inside(&z.rings, [x, y]) {
                        n += 1;
                        water += usize::from(surface(find(&words, [x, y])) == Surface::Water as u32);
                    }
                    y += step;
                }
                x += step;
            }
            zones += usize::from(n > 0);
        }
        // An edge between a zone and water or a bank with a step.
        let zone = Surface::Zone as u32;
        let wet = |s: u32| s == Surface::Water as u32 || s == Surface::Bank as u32;
        let unnamed = (edge_steps(&words, &input.terrain, key, EDGE_GRID_M).iter())
            .map(|st| st.labels.map(surface))
            .filter(|s| (s[0] == zone && wet(s[1])) || (s[1] == zone && wet(s[0])))
            .count();
        // Every crossing by its seed, as global points.
        let global = |rings: &[Vec<[i32; 2]>]| -> Vec<[i64; 2]> {
            rings.iter().flatten().map(|p| [o.x + i64::from(p[0]), o.y + i64::from(p[1])]).collect()
        };
        let mut built: BTreeMap<[i64; 2], Vec<[i64; 2]>> = BTreeMap::new();
        for z in &wc.zones {
            built.entry(z.seed).or_default().extend(global(&z.rings));
        }
        (key, water, far, unnamed, zones, built, mpu)
    });
    let (mut water, mut far, mut unnamed, mut zones) = (0, 0, 0, 0);
    let mut by_key = BTreeMap::new();
    for (key, w, f, u, z, built, mpu) in out {
        (water, far, unnamed, zones) = (water + w, far + f, unnamed + u, zones + z);
        by_key.insert(key, (built, mpu));
    }
    // Two neighbour cells: each crossing that both build, within `SHARED_M` of their shared line.
    let mut differ = Vec::new();
    for (key, (built, mpu)) in &by_key {
        let o = key.origin();
        let w = SHARED_M / mpu;
        for (next, axis) in [(CellKey { x: key.x + 1, y: key.y }, 0), (CellKey { x: key.x, y: key.y + 1 }, 1)] {
            let Some((other, _)) = by_key.get(&next) else { continue };
            let line = [o.x, o.y][axis] as f64 + CELL_UNITS as f64;
            let near = |pts: &[[i64; 2]]| {
                let mut v: Vec<[i64; 2]> = pts.iter().filter(|p| (p[axis] as f64 - line).abs() <= w).copied().collect();
                v.sort_unstable();
                v.dedup();
                v
            };
            for (seed, pts) in built.iter().filter(|(s, _)| other.contains_key(*s)) {
                if near(pts) != near(&other[seed]) {
                    differ.push((*key, *seed));
                }
            }
        }
    }
    println!("zones in the cells {zones}: water points in them {water}, vertices past the reach {far}");
    println!("zone and water or bank edges with a step: {unnamed}; crossings two cells build apart: {differ:?}");
    assert_eq!((water, far, unnamed, differ.len()), (0, 0, 0, 0));
}
