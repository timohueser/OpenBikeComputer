//! The heights over every cell of the TrailView fixture: steps inside faces, steps at label edges
//! with no rule and slopes steeper than a rock face do not grow, no cap acts, every paved face finds
//! its host, and two cells give a point on their shared line the same bits. The scan's steps with
//! no rule and the ripple of the carriageways are guidelines: printed, not gated. A fault within the
//! reach of a water sample whose level lies far under the terrain is the bake's: a level never rises
//! downstream, so a dip upstream holds it under the later terrain (a pit), and where the map's line runs
//! on a slope beside the valley floor, the level is the low ground across the line. Those faults are
//! counted apart, printed and not gated.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::heights::check::{at, border, edge_steps, ripple, scan, Class, EDGE_GRID_M, RIPPLE_M};
use obc_ground_plan::faces::heights::Step;
use obc_ground_plan::faces::records;
use obc_ground_plan::faces::Rules;
use obc_ground_plan::faces::Surface;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use obc_ground_plan::graph::{self, Family, Level, WayGraph};
use obc_ground_plan::CellInput;

/// The grid of the step scan, metres.
const GRID_M: f64 = 0.5;
/// Steps inside faces, measured with the slopes from the paved outline and line water (steps of 3 cm
/// where a released path's distance narrows a river's budget round): the count must not grow.
const INSIDE_MAX: usize = 7;
/// Steps at label edges with no rule (`check::edge_steps`), measured with the slopes from the paved
/// outline, walls up to the highest wall, faces past it as steep as their need, and line water: the count
/// must not grow.
const EDGE_MAX: usize = 125;
/// Pairs of grid points on a slope steeper than a rock face where the terrain is not (`Scan::steep`),
/// measured with cones on their outward side, faces past the highest wall as steep as their need, and the
/// banks of line water, whose cuts climb as steep as the terrain asks beside a gorge: the count must not
/// grow.
const STEEP_MAX: usize = 3914;
/// Of those, the pairs steeper than 6 m/m and 12 m/m, measured the same way: a river's cut in a gorge
/// that its release lifts to the terrain. A near-vertical rim that the total hides must not come back.
const STEEP_6_MAX: usize = 78;
const STEEP_12_MAX: usize = 3;
/// A water sample whose level lies more than this off the terrain at its line has a level that the bake
/// holds away from its line (a pit, or the low ground beside it): the design keeps the water at most
/// `freeboard_m` (0.8 m) under its bank and `depth_m` (1 m) deep. On the fixture every such sample lies
/// under the terrain.
const LEVEL_OFF_M: f64 = 3.0;
/// The faults within this of such a sample are its: a river's bank reach (15 m) and a half-width.
const LEVEL_REACH_M: f64 = 20.0;
/// The places of the ripple report (latitude, longitude).
const PLACES: [(&str, f64, f64); 5] = [
    ("v17-sharp-corners", 46.821753, 8.404913),
    ("i07-hairpin", 46.82177, 8.40497),
    ("d-hairpin", 46.827088, 8.391039),
    ("main-road-15m", 46.827847, 8.380036),
    ("valley-junction", 46.828466, 8.378529),
];
/// The side of a place's square, metres.
const PLACE_M: f64 = 64.0;

struct Cell {
    words: Vec<u32>,
    input: CellInput,
}

#[test]
fn every_fixture_cell_keeps_its_steps_on_face_edges_and_agrees_with_its_neighbours() {
    let dir = default_dir();
    let mut fixture = Fixture::open(&dir);
    let rules = Rules::embedded();
    let mut cells: Vec<CellKey> = (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some()))
        .collect();
    assert!(cells.len() > 100, "{} cells", cells.len());
    // The cells of the places past the fixture's bounds: for their ripple only.
    let mut places: Vec<CellKey> = PLACES.iter().flat_map(|&(_, lat, lon)| place_cells(lat, lon)).collect();
    places.retain(|k| !cells.contains(k));
    places.sort();
    places.dedup();
    let scanned = cells.len();
    cells.extend(places);
    let built = fixture.map(&cells, |key, input| {
        let graph = graph::build(key, &input, &rules.sections);
        let words = records::build(&graph, &input, rules, key, 0, true).near.words;
        let ours = cells[..scanned].contains(&key);
        let mut s = if ours { scan(&words, &input.terrain, key, GRID_M) } else { Default::default() };
        let mut e = if ours { edge_steps(&words, &input.terrain, key, EDGE_GRID_M) } else { Vec::new() };
        // The steps between water and a way (the ends that B's crossings close): count and greatest.
        let water = |l: Option<u32>| l.is_some_and(|id| records::label(&words, id)[0] & 0xff == Surface::Water as u32);
        let apart = (s.steps.iter())
            .filter(|st| st.class == Class::Named(Step::Unrelated) && water(st.labels[0]) != water(st.labels[1]))
            .fold((0, 0.0f64), |(n, m), st| (n + 1, m.max(st.dh.abs())));
        // The faults of the bake's water levels, apart: steps, edge steps, steep pairs.
        let off = off_level(&graph, &input, key);
        let n = (s.steps.len(), e.len(), s.steep.len());
        s.steps.retain(|st| !off(st.a));
        e.retain(|st| !off(st.a));
        s.steep.retain(|st| !off(st.0));
        let levels = [n.0 - s.steps.len(), n.1 - e.len(), n.2 - s.steep.len()];
        let edges = e.len();
        let line = if ours { border(&words, &input.terrain, key, 512) } else { Vec::new() };
        (key, s, edges, line, levels, apart, Cell { words, input })
    });
    let (mut inside, mut unnamed, mut edges, mut full, mut missing, mut water_off) = (0, 0, 0, 0, 0, 0);
    let mut levels = [0; 3];
    let mut apart = (0, 0.0f64);
    let mut steep = Vec::new();
    let mut lines: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut by_key = BTreeMap::new();
    for (key, s, e, line, l, a, cell) in built {
        apart = (apart.0 + a.0, apart.1.max(a.1));
        levels = [0, 1, 2].map(|k| levels[k] + l[k]);
        inside += s.steps.iter().filter(|st| st.class == Class::Inside).count();
        edges += e;
        unnamed += s.steps.iter().filter(|st| st.class == Class::Unnamed).count();
        steep.extend(s.steep.iter().map(|st| st.1));
        (full, missing) = (full + s.slots_full.len(), missing + s.host_missing.len());
        water_off += s.water_off.len();
        for (k, v) in line {
            lines.entry(k).or_default().push(v);
        }
        by_key.insert(key, cell);
    }
    let over = |m: f64| steep.iter().filter(|&&c| c > m).count();
    let (over_6, over_12) = (over(6.0), over(12.0));
    println!(
        "inside {inside}, unnamed {unnamed}, edge steps {edges}, steep {} (over 6 m/m {over_6}, over 12 m/m {over_12}), slots full {full}, host missing {missing}",
        steep.len()
    );
    println!(
        "near water levels over {LEVEL_OFF_M} m off the terrain (not gated): steps {}, edge steps {}, steep {}",
        levels[0], levels[1], levels[2]
    );
    println!("steps between water and a way (not gated): {}, greatest {:.2} m", apart.0, apart.1);
    assert!(inside <= INSIDE_MAX, "{inside} steps inside faces");
    assert!(edges <= EDGE_MAX, "{edges} steps at label edges with no rule");
    assert!(steep.len() <= STEEP_MAX, "{} slope pairs steeper than a rock face", steep.len());
    assert!(over_6 <= STEEP_6_MAX, "{over_6} slope pairs steeper than 6 m/m");
    assert!(over_12 <= STEEP_12_MAX, "{over_12} slope pairs steeper than 12 m/m");
    assert_eq!((full, missing, water_off), (0, 0, 0), "slots full, host missing, water off its level");
    let shared: Vec<_> = lines.values().filter(|v| v.len() == 2).collect();
    assert!(shared.len() > 100_000, "{} shared points", shared.len());
    let differ: Vec<_> = shared.iter().filter(|v| v[0].h != v[1].h || v[0].t != v[1].t).collect();
    assert!(
        differ.is_empty(),
        "{} of {} border points differ: {:?}",
        differ.len(),
        shared.len(),
        &differ[..differ.len().min(3)]
    );
    for (name, lat, lon) in PLACES {
        let [along, across] = place_ripple(&by_key, lat, lon);
        println!("{name}: ripple p99 along {:.2} mm, across {:.2} mm", along * 1e3, across * 1e3);
    }
}

/// Whether a point of a cell (frame units) lies within `LEVEL_REACH_M` of a ground water sample whose
/// level is more than `LEVEL_OFF_M` off the terrain there.
fn off_level(graph: &WayGraph, input: &CellInput, key: CellKey) -> impl Fn([f64; 2]) -> bool {
    let o = key.origin();
    let reach = LEVEL_REACH_M / metres_per_unit(o.y + CELL_UNITS / 2);
    let off: Vec<[f64; 2]> = (graph.ways.iter())
        .filter(|w| w.family == Family::Water && w.level == Level::Ground)
        .flat_map(|w| &graph.samples[w.samples.start as usize..w.samples.end as usize])
        .filter(|s| {
            let g = GlobalPoint { x: o.x + i64::from(s.pos[0]), y: o.y + i64::from(s.pos[1]) };
            input.terrain.height(g).is_some_and(|t| (s.z_m - t).abs() > LEVEL_OFF_M)
        })
        .map(|s| [f64::from(s.pos[0]), f64::from(s.pos[1])])
        .collect();
    move |q| off.iter().any(|p| libm::hypot(p[0] - q[0], p[1] - q[1]) < reach)
}

/// The cells of a place's square.
fn place_cells(lat: f64, lon: f64) -> Vec<CellKey> {
    let centre = from_lon_lat(lon, lat);
    let half = (PLACE_M / 2.0 / metres_per_unit(centre.y)) as i64;
    let (a, b) = (
        CellKey::of(GlobalPoint { x: centre.x - half, y: centre.y - half }),
        CellKey::of(GlobalPoint { x: centre.x + half, y: centre.y + half }),
    );
    (a.y..=b.y).flat_map(|y| (a.x..=b.x).map(move |x| CellKey { x, y })).collect()
}

/// The p99 of the carriageway's second differences along and across at a place, metres.
fn place_ripple(cells: &BTreeMap<CellKey, Cell>, lat: f64, lon: f64) -> [f64; 2] {
    let centre = from_lon_lat(lon, lat);
    let upm = 1.0 / metres_per_unit(centre.y);
    let n = (PLACE_M / RIPPLE_M) as i64;
    let mut d2: [Vec<f64>; 2] = [Vec::new(), Vec::new()];
    for j in 0..n {
        for i in 0..n {
            let g = [
                centre.x as f64 + ((i - n / 2) as f64 * RIPPLE_M) * upm,
                centre.y as f64 + ((j - n / 2) as f64 * RIPPLE_M) * upm,
            ];
            let key = CellKey::of(GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
            let Some(c) = cells.get(&key) else { continue };
            let o = key.origin();
            let q = [g[0] - o.x as f64, g[1] - o.y as f64];
            let Some(h) = at(&c.words, &c.input.terrain, key, q) else { continue };
            if let Some(r) = ripple(&c.words, &c.input.terrain, key, q, h) {
                for (k, v) in r.into_iter().enumerate().filter(|(_, v)| !v.is_nan()) {
                    d2[k].push(v.abs());
                }
            }
        }
    }
    d2.map(|mut v| {
        assert!(v.len() > 500, "{} points", v.len());
        v.sort_by(f64::total_cmp);
        v[v.len() * 99 / 100]
    })
}
