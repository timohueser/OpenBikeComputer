//! The way graph over every cell of the TrailView fixture: the invariants G1 to G4 of `check` and G7
//! to G9 of `check_levels`, equal graphs and levels near the border of two neighbouring cells (G5),
//! the same bytes twice (G6), and the levels that the tiles carry.

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::{BTreeMap, BTreeSet};

use fixture::{default_dir, Fixture};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::graph::{
    build, build_levelled, check, check_levels, level_tags, levels, Arm, Level, Sample, State, Station, WayGraph,
};
use obc_ground_plan::sections::Sections;
use obc_ground_plan::tile::Tags;
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

/// The levels that the fixture's tiles carry are those that `levels` gives the cells: the tags of each feature are
/// those that the bake's merge (`level_tags::merge`) gives of what the cells find, storey and slab included; and the
/// graph of the storeys (`build_levelled`) gives every way inside a cell's square the level of its stretch's state,
/// with no way on a HIDDEN stretch. A change of the levels or of their rules takes a new bake of the fixture.
#[test]
fn the_tiles_carry_the_levels_of_each_cell() {
    let mut fixture = Fixture::open(&default_dir());
    let (mut faults, mut ways) = (Vec::new(), 0);
    // A feature by its kind and tags, the same in each tile that holds it; and its level tags.
    let (mut ids, mut carried_tags) = (BTreeMap::<String, u32>::new(), BTreeMap::<u32, Tags>::new());
    let (mut found, mut groups, mut over) = (BTreeMap::<u32, Vec<level_tags::Found>>::new(), Vec::new(), Vec::new());
    for (key, computed) in graphs() {
        let input = fixture.cell(key);
        let cell = level_tags::found(&computed, &input, Sections::embedded());
        let mut id = |[c, r, index]: [u32; 3]| {
            let f = input.features.iter().find(|f| f.parts[0].tile == (c, r) && f.index == index).unwrap();
            let next = ids.len() as u32;
            let id = *ids.entry(format!("{} {:?}", f.kind, f.tags)).or_insert(next);
            let own = [level_tags::LEVELS, level_tags::STOREY, level_tags::SLAB];
            let tags = f.tags.iter().filter(|(k, _)| own.contains(&k.as_str()));
            carried_tags.insert(id, tags.map(|(k, v)| (k.clone(), v.clone())).collect());
            id
        };
        for (k, f) in cell.found {
            found.entry(id(k)).or_default().push(f);
        }
        groups.extend(cell.groups.into_iter().map(|g| g.into_iter().map(&mut id).collect::<Vec<_>>()));
        over.extend(cell.over.into_iter().map(|(l, u)| (id(l), id(u))));
        // The graph of the storeys gives each stretch the level of its state, and holds no HIDDEN stretch.
        let levelled = build_levelled(key, &input, Sections::embedded());
        let k = |st: &Station| (st.span, st.u);
        let state = |st: &Station| {
            let run = computed
                .levels
                .runs
                .iter()
                .find(|r| r.from.part == st.part && k(&r.from) <= k(st) && k(st) <= k(&r.to));
            run.map(|r| r.state)
        };
        // A point strictly inside the way: an end sample lies on a run boundary, where rounding picks either run.
        let middle = |g: &WayGraph, w: u32| {
            let (a, b) = (g.ways[w as usize].from, g.ways[w as usize].to);
            let u = if a.span == b.span { (a.u + b.u) / 2.0 } else { (a.u + 1.0) / 2.0 };
            Station { u, ..a }
        };
        for w in (0..levelled.ways.len() as u32).filter(|&w| level_tags::inside(&levelled, w)) {
            let way = &levelled.ways[w as usize];
            let part = &levelled.parts[way.part as usize];
            if part.level == Level::Tunnel || (way.level == Level::Deck && part.level != Level::Deck) {
                continue;
            }
            let want = match state(&middle(&levelled, w)) {
                Some(State::Deck) => Level::Deck,
                Some(State::Open) => Level::Ground,
                Some(State::Hidden) => {
                    faults.push(format!("{key:?} way {w}: it lies on a HIDDEN stretch"));
                    continue;
                }
                _ => continue,
            };
            ways += 1;
            if way.level != want {
                faults.push(format!("{key:?} way {w}: on {:?}, its stretch's state gives {want:?}", way.level));
            }
        }
        for r in
            computed.levels.runs.iter().filter(|r| r.state == State::Hidden && level_tags::inside(&computed, r.way))
        {
            let st = Station { part: r.from.part, span: (r.from.span + r.to.span) / 2, u: 0.5 };
            if k(&st) > k(&r.from) && k(&st) < k(&r.to) && levelled.way_at(&st, Level::Ground).is_some() {
                faults.push(format!("{key:?} way {}: a way of the storeys' graph lies on its HIDDEN stretch", r.way));
            }
        }
    }
    let merged = level_tags::merge(&found, &groups, &over);
    let mut counts = BTreeMap::<String, usize>::new();
    for (id, carried) in &carried_tags {
        let want = merged.get(id).cloned().unwrap_or_default();
        for k in want.keys() {
            *counts.entry(k.clone()).or_default() += 1;
        }
        if want != *carried {
            faults.push(format!("feature {id}: the cells give {want:?}, the tiles carry {carried:?}"));
        }
    }
    println!("{ways} ways of the storeys' graph on a carried stretch; tags of the cells' features {counts:?}");
    assert!(ways > 10, "the fixture's bridges and tunnels");
    assert!(faults.is_empty(), "{} faults:\n{}", faults.len(), faults[..faults.len().min(10)].join("\n"));
}
