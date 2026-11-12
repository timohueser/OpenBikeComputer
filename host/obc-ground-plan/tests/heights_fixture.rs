//! The heights over every cell of the TrailView fixture: steps inside faces, steps at label edges
//! with no rule and slopes steeper than a rock face do not grow, no cap acts, every paved face finds
//! its host, and two cells give a point on their shared line the same bits. The scan's steps with
//! no rule and the ripple of the carriageways are guidelines: printed, not gated. A fault within the
//! reach of a tile's water vertex whose level lies far off the terrain is the bake's: a level never rises
//! downstream, so a dip upstream holds it under the later terrain (a pit), and where the map's line runs
//! on a slope beside the valley floor, the level is the low ground across the line. Those faults are
//! counted apart and gated apart: the reader cannot move its own faults out of the gate.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::heights::check::{
    at, border, edge_steps, ripple, scan, slope_steps, wall_steps, Class, EDGE_GRID_M, RIPPLE_M,
};
use obc_ground_plan::faces::heights::Step;
use obc_ground_plan::faces::records;
use obc_ground_plan::faces::Rules;
use obc_ground_plan::faces::Surface;
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint, CELL_UNITS, FRAME_BITS};
use obc_ground_plan::graph;
use obc_ground_plan::sections::WATER_KINDS;
use obc_ground_plan::CellInput;

/// The grid of the step scan, metres.
const GRID_M: f64 = 0.5;
/// Steps inside faces, measured on the revision 16 fixture with the slopes from the paved outline and line
/// water: the count must not grow.
const INSIDE_MAX: usize = 9;
/// Steps at label edges with no rule (`check::edge_steps`), measured on the revision 16 fixture with the
/// slopes from the outline pieces of `records::outline_edges` and the vertex values of `heights::groups`
/// (an end's cone over the fan of its turn only), walls up to the highest wall, faces past it as steep as
/// their need, and line water: the count must not grow.
const EDGE_MAX: usize = 101;
/// Pairs of grid points on a slope steeper than a rock face where the terrain is not (`Scan::steep`),
/// measured on the revision 16 fixture with cones on their outward side, the outline pieces of
/// `records::outline_edges` and the vertex values of `heights::groups`, faces past the highest wall as steep
/// as their need, the faces' rings with a vertex at each tile line, and the banks of line water, whose cuts
/// climb as steep as the terrain asks beside a gorge: the count must not grow.
const STEEP_MAX: usize = 2483;
/// A no-room wall's height that changes along it by more than this per metre is a step in its top
/// (`check::wall_steps`): pairs of points measured on the revision 16 fixture with the slope and the wall of
/// each point from the need at its foot (`heights::fit`) and line water: the count must not grow.
const WALL_STEP_GRADE: f64 = 0.5;
const WALL_STEPS_MAX: usize = 17;
/// The ground's grade along the rim that changes across a piece end by more than this is a shading band
/// (`check::slope_steps`), measured the same way, with the faces' rings with a vertex at each tile line (it
/// moves piece ends by a few units) and line water, whose curved banks end many short pieces: the count must
/// not grow.
const SLOPE_STEP_GRADE: f64 = 0.1;
const SLOPE_STEPS_MAX: usize = 12864;
/// A tile's water vertex whose level lies more than this off the terrain at the vertex has a level that
/// the bake holds away from its line (a pit, or the low ground beside it): the design keeps the water at
/// most `freeboard_m` (0.8 m) under its bank and `depth_m` (1 m) deep.
const LEVEL_OFF_M: f64 = 3.0;
/// The faults within this of such a vertex, or of a tile segment between two such vertices, are its: a
/// river's bank reach (15 m) and a half-width.
const LEVEL_REACH_M: f64 = 20.0;
/// The faults near such levels (steps, edge steps, steep pairs, wall-top steps, slope steps), measured on
/// the revision 16 fixture: the counts must not grow.
const LEVEL_MAX: [usize; 5] = [9, 66, 10929, 0, 1619];
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
        let mut w = if ours { wall_steps(&words, &input.terrain, key, WALL_STEP_GRADE) } else { Vec::new() };
        let mut g = if ours { slope_steps(&words, &input.terrain, key, SLOPE_STEP_GRADE) } else { Vec::new() };
        // The steps between water and a way (the ends that B's crossings close): count and greatest.
        let water = |l: Option<u32>| l.is_some_and(|id| records::label(&words, id)[0] & 0xff == Surface::Water as u32);
        let apart = (s.steps.iter())
            .filter(|st| st.class == Class::Named(Step::Unrelated) && water(st.labels[0]) != water(st.labels[1]))
            .fold((0, 0.0f64), |(n, m), st| (n + 1, m.max(st.dh.abs())));
        // The faults of the bake's water levels, apart: steps, edge steps, steep pairs, wall-top and slope steps.
        let off = off_level(&input, key);
        let n = [s.steps.len(), e.len(), s.steep.len(), w.len(), g.len()];
        s.steps.retain(|st| !off(st.a));
        e.retain(|st| !off(st.a));
        s.steep.retain(|st| !off(st.0));
        w.retain(|st| !off(st.0));
        g.retain(|st| !off(st.0));
        let kept = [s.steps.len(), e.len(), s.steep.len(), w.len(), g.len()];
        let levels: [usize; 5] = std::array::from_fn(|k| n[k] - kept[k]);
        let line = if ours { border(&words, &input.terrain, key, 512) } else { Vec::new() };
        (key, s, [e.len(), w.len(), g.len()], line, levels, apart, Cell { words, input })
    });
    let (mut inside, mut unnamed, mut full, mut missing, mut water_off) = (0, 0, 0, 0, 0);
    let [mut edges, mut walls, mut slopes] = [0; 3];
    let mut levels = [0; 5];
    let mut apart = (0, 0.0f64);
    let mut steep = Vec::new();
    let mut lines: BTreeMap<_, Vec<_>> = BTreeMap::new();
    let mut by_key = BTreeMap::new();
    for (key, s, e, line, l, a, cell) in built {
        apart = (apart.0 + a.0, apart.1.max(a.1));
        levels = std::array::from_fn(|k| levels[k] + l[k]);
        inside += s.steps.iter().filter(|st| st.class == Class::Inside).count();
        (edges, walls, slopes) = (edges + e[0], walls + e[1], slopes + e[2]);
        unnamed += s.steps.iter().filter(|st| st.class == Class::Unnamed).count();
        let o = key.origin();
        steep.extend(s.steep.iter().map(|st| ([o.x as f64 + st.0[0], o.y as f64 + st.0[1]], st.1)));
        (full, missing) = (full + s.slots_full.len(), missing + s.host_missing.len());
        water_off += s.water_off.len();
        for (k, v) in line {
            lines.entry(k).or_default().push(v);
        }
        by_key.insert(key, cell);
    }
    let over = |m: f64| steep.iter().filter(|c| c.1 > m).count();
    let (over_6, over_12) = (over(6.0), over(12.0));
    println!(
        "inside {inside}, unnamed {unnamed}, edge steps {edges}, wall-top steps {walls}, slope steps {slopes}, steep {} (over 6 m/m {over_6}, over 12 m/m {over_12}), slots full {full}, host missing {missing}",
        steep.len()
    );
    println!(
        "near water levels over {LEVEL_OFF_M} m off the terrain: steps {}, edge steps {}, steep {}, wall-top steps {}, slope steps {}",
        levels[0], levels[1], levels[2], levels[3], levels[4]
    );
    println!("steps between water and a way (not gated): {}, greatest {:.2} m", apart.0, apart.1);
    // The five steepest places, each the steepest pair within `LEVEL_REACH_M` of it.
    steep.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut worst: Vec<([f64; 2], f64)> = Vec::new();
    for &(g, climb) in &steep {
        let near = |p: &([f64; 2], f64)| {
            libm::hypot(p.0[0] - g[0], p.0[1] - g[1]) * metres_per_unit(g[1] as i64) < LEVEL_REACH_M
        };
        if worst.len() < 5 && !worst.iter().any(near) {
            worst.push((g, climb));
        }
    }
    for (g, climb) in worst {
        let [lat, lon] = lat_lon(g);
        println!("steep {climb:.1} m/m at {lat:.6},{lon:.6}");
    }
    assert!(inside <= INSIDE_MAX, "{inside} steps inside faces");
    assert!(edges <= EDGE_MAX, "{edges} steps at label edges with no rule");
    assert!(walls <= WALL_STEPS_MAX, "{walls} wall-top steps over {WALL_STEP_GRADE} m/m");
    assert!(slopes <= SLOPE_STEPS_MAX, "{slopes} slope steps over {SLOPE_STEP_GRADE} m/m");
    assert!(steep.len() <= STEEP_MAX, "{} slope pairs steeper than a rock face", steep.len());
    // Measured the same way, no pair is steeper than 6 m/m: a near-vertical rim that the total hides must not
    // come back.
    assert_eq!((over_6, over_12), (0, 0), "slope pairs steeper than 6 m/m and 12 m/m");
    assert!((0..5).all(|k| levels[k] <= LEVEL_MAX[k]), "{levels:?} faults near water levels off the terrain");
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

/// Whether a point of a cell (frame units) lies within `LEVEL_REACH_M` of a tile's ground water vertex
/// whose own level is more than `LEVEL_OFF_M` off the terrain there, or of a tile segment between two
/// such vertices. It reads the input only, never the reader's levels.
fn off_level(input: &CellInput, key: CellKey) -> impl Fn([f64; 2]) -> bool {
    let o = key.origin();
    let reach = LEVEL_REACH_M / metres_per_unit(o.y + CELL_UNITS / 2);
    let ground = |f: &&obc_ground_plan::tile::Feature| {
        WATER_KINDS.contains(&f.kind.as_str()) && f.tags.get("tunnel").is_none_or(|t| t == "no")
    };
    let mut off: Vec<[[f64; 2]; 2]> = Vec::new();
    for part in input.features.iter().filter(ground).flat_map(|f| &f.parts) {
        let Some(heights) = &part.heights else { continue };
        let far: Vec<bool> = (part.points.iter().zip(heights))
            .map(|(&g, z)| z.zip(input.terrain.height(g)).is_some_and(|(z, t)| (z - t).abs() > LEVEL_OFF_M))
            .collect();
        let local = |i: usize| [(part.points[i].x - o.x) as f64, (part.points[i].y - o.y) as f64];
        for i in (0..far.len()).filter(|&i| far[i]) {
            let next = if far.get(i + 1) == Some(&true) { i + 1 } else { i };
            off.push([local(i), local(next)]);
        }
    }
    move |q| off.iter().any(|&[a, b]| distance(q, a, b) < reach)
}

/// The distance of q from the segment ab.
fn distance(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let len2 = d[0] * d[0] + d[1] * d[1];
    let t = if len2 > 0.0 { (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / len2).clamp(0.0, 1.0) } else { 0.0 };
    libm::hypot(a[0] + t * d[0] - q[0], a[1] + t * d[1] - q[1])
}

/// The latitude and longitude of a global point in frame units.
fn lat_lon(g: [f64; 2]) -> [f64; 2] {
    let square = (1u64 << FRAME_BITS) as f64;
    let lat = libm::atan(libm::sinh(std::f64::consts::PI * (1.0 - 2.0 * g[1] / square))).to_degrees();
    [lat, g[0] / square * 360.0 - 180.0]
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
