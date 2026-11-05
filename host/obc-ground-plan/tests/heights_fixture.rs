//! The heights over every cell of the TrailView fixture: steps stand on face edges by rules, no cap
//! acts, every paved face finds its host, two cells give a point on their shared line the same bits,
//! and the carriageways are flat at the places of the ripple gate.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::heights::check::{at, border, ripple, scan, Class, RIPPLE_M};
use obc_ground_plan::faces::records;
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::graph;
use obc_ground_plan::CellInput;

/// The grid of the step scan, metres.
const GRID_M: f64 = 0.5;
/// Steps inside faces on the commit of GP6's first step (the prototype's functions as they were):
/// the count must not grow.
const INSIDE_MAX: usize = 140;
/// Steps on face edges with no rule of their size on the commit that first named them by rule; the
/// target is 0.
const UNNAMED_MAX: usize = 346;
/// The places of the ripple gate (latitude, longitude): the p99 of the second differences along and
/// across stays at or below `RIPPLE_GATE_M` at each.
const PLACES: [(&str, f64, f64); 5] = [
    ("v17-sharp-corners", 46.821753, 8.404913),
    ("i07-hairpin", 46.82177, 8.40497),
    ("d-hairpin", 46.827088, 8.391039),
    ("main-road-15m", 46.827847, 8.380036),
    ("valley-junction", 46.828466, 8.378529),
];
/// The ripple gate, metres: roads almost perfectly flat (owner), 2 mm proposed in the sub-issue.
const RIPPLE_GATE_M: f64 = 0.002;
/// The side of a place's square, metres.
const PLACE_M: f64 = 64.0;

struct Cell {
    words: Vec<u32>,
    input: CellInput,
}

#[test]
fn every_fixture_cell_steps_only_by_rules_on_face_edges_and_agrees_with_its_neighbours() {
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
        let s = if ours { scan(&words, &input.terrain, key, GRID_M) } else { Default::default() };
        let line = if ours { border(&words, &input.terrain, key, 512) } else { Vec::new() };
        (key, s, line, Cell { words, input })
    });
    let (mut inside, mut unnamed, mut full, mut missing) = (0, 0, 0, 0);
    let mut lines: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut by_key = BTreeMap::new();
    for (key, s, line, cell) in built {
        inside += s.steps.iter().filter(|st| st.class == Class::Inside).count();
        unnamed += s.steps.iter().filter(|st| st.class == Class::Unnamed).count();
        (full, missing) = (full + s.slots_full.len(), missing + s.host_missing.len());
        for (k, v) in line {
            lines.entry(k).or_default().push(v);
        }
        by_key.insert(key, cell);
    }
    println!("inside {inside}, unnamed {unnamed}, slots full {full}, host missing {missing}");
    assert!(inside <= INSIDE_MAX, "{inside} steps inside faces");
    assert!(unnamed <= UNNAMED_MAX, "{unnamed} steps with no rule");
    assert_eq!((full, missing), (0, 0));
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
        assert!(along <= RIPPLE_GATE_M && across <= RIPPLE_GATE_M, "{name}: p99 {along} m along, {across} m across");
    }
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
