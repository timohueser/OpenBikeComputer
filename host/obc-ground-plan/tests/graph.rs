//! The way graph over every cell of the TrailView fixture: the invariants G1 to G4 of `check` and G7
//! to G9 of `check_levels`, equal graphs and levels near the border of two neighbouring cells (G5),
//! the same bytes twice (G6), and the levels that the tiles carry.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::graph::{build, check, check_levels, level_tags, levels, Arm, Sample, Station, WayGraph};
use obc_ground_plan::sections::Sections;
use obc_ground_plan::CellInput;

/// The band on each side of a shared border where two cells must agree.
const BAND_M: f64 = 100.0;

/// The fixture's cells whose feature tiles the fixture holds, built, with their levels.
fn graphs() -> BTreeMap<CellKey, WayGraph> {
    let mut fixture = Fixture::open(&default_dir());
    let dir = default_dir();
    let cells: Vec<CellKey> = (fixture.cells().into_iter())
        .filter(|k| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some()))
        .collect();
    assert!(cells.len() > 100, "{} cells", cells.len());
    cells.into_iter().map(|key| (key, built(key, &fixture.cell(key)))).collect()
}

fn built(key: CellKey, input: &CellInput) -> WayGraph {
    let mut g = build(key, input, Sections::embedded());
    levels(&mut g, input, Sections::embedded(), &Rules::embedded().structures);
    g
}

#[test]
fn every_fixture_cell_holds_g1_to_g4_and_g6() {
    let mut fixture = Fixture::open(&default_dir());
    let graphs = graphs();
    let mut faults = Vec::new();
    let (mut junctions, mut crossings) = (0, 0);
    for (key, graph) in &graphs {
        let input = fixture.cell(*key);
        let lv = check_levels(graph, &input, Sections::embedded(), &Rules::embedded().structures);
        faults.extend(check(graph).into_iter().chain(lv).map(|f| format!("{key:?} {}: {}", f.check, f.what)));
        junctions += graph.junctions.iter().filter(|j| j.owned).count();
        crossings += graph.crossings.iter().filter(|c| c.owned).count();
    }
    println!("{} cells, {junctions} owned junctions, {crossings} owned crossings", graphs.len());
    assert!(faults.is_empty(), "{} faults:\n{}", faults.len(), faults[..faults.len().min(40)].join("\n"));
    for key in graphs.keys().step_by(7) {
        let again = built(*key, &fixture.cell(*key));
        assert_eq!(format!("{again:?}"), format!("{:?}", graphs[key]), "G6: {key:?} builds twice the same");
    }
    assert!(junctions > 100 && crossings > 10);
}

/// What two cells must agree on near a border, keyed by global position, as text.
fn band(graph: &WayGraph, near: &dyn Fn([i64; 2]) -> bool) -> BTreeSet<String> {
    let o = graph.cell.origin();
    let global = |p: [i32; 2]| [o.x + i64::from(p[0]), o.y + i64::from(p[1])];
    let mut out = BTreeSet::new();
    for way in &graph.ways {
        let part = &graph.parts[way.part as usize];
        for s in &graph.samples[way.samples.start as usize..way.samples.end as usize] {
            if near(global(s.pos)) {
                let span = s.span as usize;
                let start = part.vertices[span].pos;
                let end = part.vertices.get(span + 1).map(|v| v.pos);
                out.insert(format!(
                    "sample {start:?} {end:?} {:?} {:?} u {} z {} t {:?} g {} b {} w {} walk {} {:?}",
                    s.s_m.to_bits(),
                    global(s.pos),
                    s.u,
                    s.z_m,
                    s.tangent,
                    s.grade,
                    s.bank,
                    s.widen_m,
                    s.walk,
                    s.kind
                ));
            }
        }
        if graph.decks.iter().any(|d| d.synthetic && d.ways.contains(&way.id)) {
            let first = &graph.samples[way.samples.start as usize];
            let last = &graph.samples[way.samples.end as usize - 1];
            if near(global(first.pos)) || near(global(last.pos)) {
                out.insert(format!("stretch {:?} {:?}", global(first.pos), global(last.pos)));
            }
        }
    }
    let arms = |arms: &[Arm]| -> Vec<String> {
        arms.iter()
            .map(|a| {
                format!("{} {} {:?} {} {}", a.dir, a.host, a.level, a.room_m, graph.ways[a.way as usize].family as u8)
            })
            .collect()
    };
    for j in graph.junctions.iter().filter(|j| near(global(j.pos))) {
        out.insert(format!(
            "junction {:?} {:?} {:?} {} {:?}",
            global(j.pos),
            j.network,
            j.host_rule,
            j.implicit,
            arms(&j.arms)
        ));
    }
    for j in graph.joints.iter().filter(|j| near(global(j.pos))) {
        out.insert(format!(
            "joint {:?} {:?} {} {} {:?}",
            global(j.pos),
            j.network,
            j.turn_rad,
            j.stroke,
            arms(&j.arms)
        ));
    }
    for e in graph.ends.iter().filter(|e| near(global(e.pos))) {
        out.insert(format!("end {:?} {:?} {:?}", global(e.pos), e.network, arms(std::slice::from_ref(&e.arm))));
    }
    for c in graph.crossings.iter().filter(|c| near(global(c.pos))) {
        // The step and whether the upper way is the one that ends.
        let step = c.ladder.as_ref().map(|l| (l.step, l.ends.map(|e| e == l.upper)));
        out.insert(format!("crossing {:?} {:?} {:?} {} {step:?}", global(c.pos), c.kind, c.source, c.angle_rad));
    }
    // The levels: the state at each sample of a way with a state, and the end lines.
    for r in &graph.levels.runs {
        let way = &graph.ways[r.way as usize];
        let inside = |s: &&Sample| (s.span, s.u) >= (r.from.span, r.from.u) && (s.span, s.u) <= (r.to.span, r.to.u);
        for s in graph.samples[way.samples.start as usize..way.samples.end as usize].iter().filter(inside) {
            if near(global(s.pos)) {
                out.insert(format!("state {:?} {:?}", global(s.pos), r.state));
            }
        }
    }
    for l in &graph.levels.end_lines {
        let ends = [global(l.ends[0]), global(l.ends[1])];
        if near(ends[0]) || near(ends[1]) {
            out.insert(format!("end line {:?} {ends:?}", l.kind));
        }
    }
    out
}

#[test]
fn neighbouring_cells_agree_near_their_border_g5() {
    let graphs = graphs();
    let (mut pairs, mut compared, mut faults) = (0, 0, Vec::new());
    for (&a, ga) in &graphs {
        for b in [CellKey { x: a.x + 1, y: a.y }, CellKey { x: a.x, y: a.y + 1 }] {
            let Some(gb) = graphs.get(&b) else { continue };
            let o = b.origin();
            let band_units = (BAND_M / metres_per_unit(o.y)) as i64;
            let east = b.x != a.x;
            // Within the band of the shared border line, along the border's length.
            let near = |p: [i64; 2]| {
                let (across, along, start) = if east { (p[0] - o.x, p[1], o.y) } else { (p[1] - o.y, p[0], o.x) };
                across.abs() <= band_units && (start - band_units..=start + CELL_UNITS + band_units).contains(&along)
            };
            let (sa, sb) = (band(ga, &near), band(gb, &near));
            pairs += 1;
            compared += sa.len();
            for only in sa.symmetric_difference(&sb).take(6) {
                let side = if sa.contains(only) { "only in" } else { "not in" };
                faults.push(format!("{a:?} | {b:?}: {side} {a:?}: {only}"));
            }
        }
    }
    println!("{pairs} borders, {compared} elements compared");
    assert!(pairs > 100 && compared > 10_000);
    assert!(faults.is_empty(), "{} differences:\n{}", faults.len(), faults[..faults.len().min(40)].join("\n"));
}

/// The levels that the fixture's tiles carry (`level_tags::read`) are those that `levels` gives each cell on every way
/// inside the cell's square: the same states and end lines, their stations to 1e-9 of a span. A change of the levels
/// takes a new bake of the fixture.
#[test]
fn the_tiles_carry_the_levels_of_each_cell() {
    let mut fixture = Fixture::open(&default_dir());
    let structures = &Rules::embedded().structures;
    let near = |a: &Station, b: &Station| a.span == b.span && (a.u - b.u).abs() <= 1e-9;
    let (mut faults, mut ways) = (Vec::new(), 0);
    for (key, computed) in graphs() {
        let input = fixture.cell(key);
        let mut carried = build(key, &input, Sections::embedded());
        level_tags::read(&mut carried, &input, Sections::embedded(), structures.edge_band_m);
        for w in (0..computed.ways.len() as u32).filter(|&w| level_tags::inside(&computed, w)) {
            let runs = |g: &WayGraph| g.levels.runs.iter().filter(|r| r.way == w).cloned().collect::<Vec<_>>();
            let lines = |g: &WayGraph| g.levels.end_lines.iter().filter(|l| l.way == w).cloned().collect::<Vec<_>>();
            let (a, b) = (runs(&computed), runs(&carried));
            let (la, lb) = (lines(&computed), lines(&carried));
            if a.is_empty() && b.is_empty() {
                continue;
            }
            ways += 1;
            let same_runs = a.len() == b.len()
                && a.iter().zip(&b).all(|(x, y)| x.state == y.state && near(&x.from, &y.from) && near(&x.to, &y.to));
            let same_lines = la.len() == lb.len()
                && la.iter().all(|x| {
                    lb.iter().any(|y| {
                        let close = (0..2).all(|e| (0..2).all(|k| (x.ends[e][k] - y.ends[e][k]).abs() <= 1));
                        x.kind == y.kind && near(&x.at, &y.at) && close
                    })
                });
            if !same_runs || !same_lines {
                faults.push(format!("{key:?} way {w}: computed {a:?} {la:?}, carried {b:?} {lb:?}"));
            }
        }
    }
    println!("{ways} ways with levels inside their cells");
    assert!(ways > 10, "the fixture's bridges and tunnels");
    assert!(faults.is_empty(), "{} faults:\n{}", faults.len(), faults[..faults.len().min(10)].join("\n"));
}
