//! The storeys over the ground over every cell of the TrailView fixture: storey 0 reads the same with them, each
//! storey is a planar partition, each shell part closes, no parapet or railing stands in a paved face of its
//! storey, storey 0 goes on past every open end, and two neighbouring cells give a storey the same corners and heights
//! on their shared line.
#![cfg(feature = "storeys")]

#[path = "fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;

use fixture::{default_dir, Fixture};
use obc_ground_plan::cell::{build_request, build_storeys, CellBuild};
use obc_ground_plan::faces::check::partition;
use obc_ground_plan::faces::heights::height_at;
use obc_ground_plan::faces::records::find;
use obc_ground_plan::faces::{window, Rules};
use obc_ground_plan::frame::{metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::graph::{EndKind, Family, Level, State, Station, WayGraph};
use obc_ground_plan::mesh::shell::{border, closure, mesh_storeys, on_paved, StoreyMesh};
use obc_ground_plan::tile::TileCache;

/// Points per side of the grid at which storey 0 must read the same with the storeys' records.
const GRID: i32 = 32;
/// An open piece lies on an end when its centre is this near the end's line across the way (metres).
const ON_END_M: f64 = 0.5;
/// And at most this far from the way there (metres): half the widest deck stroke.
const END_REACH_M: f64 = 20.0;

/// The open pieces off the cell lines past which nothing of storey 0 goes on. An open piece lies on an A line or
/// across a deck way's own end, and behind it storey 0 goes on: a ground run of the way, or a ground way that meets
/// the way there.
fn open_without_ground(key: CellKey, g: &WayGraph, open: &[[[i32; 2]; 2]]) -> Vec<String> {
    let c = CELL_UNITS as i32;
    let mpu = metres_per_unit(key.origin().y + CELL_UNITS / 2);
    let st = |s: &Station| (s.span, s.u);
    let ground = |w: u32, at: &Station| {
        let arms = g.junctions.iter().map(|j| &j.arms[..]).chain(g.joints.iter().map(|j| &j.arms[..]));
        (g.levels.runs.iter())
            .any(|r| r.way == w && r.state == State::Open && (st(&r.from) == st(at) || st(&r.to) == st(at)))
            || arms.into_iter().any(|arms| {
                arms.iter().any(|a| a.way == w && st(&a.at) == st(at))
                    && arms.iter().any(|a| a.level == Level::Ground && g.ways[a.way as usize].family != Family::Water)
            })
    };
    let unit = |a: [f64; 2], b: [f64; 2]| {
        let l = libm::hypot(b[0] - a[0], b[1] - a[1]);
        [(b[0] - a[0]) / l, (b[1] - a[1]) / l]
    };
    // Each end: its centre, the way's direction there, the way and its station.
    let mut ends = Vec::new();
    for l in g.levels.end_lines.iter().filter(|l| l.kind == EndKind::A) {
        let [a, b] = l.ends.map(|p| p.map(f64::from));
        let across = unit(a, b);
        ends.push(([(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0], [-across[1], across[0]], l.way, l.at));
    }
    for w in g.ways.iter().filter(|w| w.level == Level::Deck && w.family != Family::Water && w.samples.len() >= 2) {
        let p = |k: u32| g.samples[k as usize].pos.map(f64::from);
        let (s, e) = (w.samples.start, w.samples.end - 1);
        ends.push((p(s), unit(p(s), p(s + 1)), w.id, w.from));
        ends.push((p(e), unit(p(e - 1), p(e)), w.id, w.to));
    }
    let on_cell_line = |[a, b]: &[[i32; 2]; 2]| (0..2).any(|k| a[k] == b[k] && (a[k] == 0 || a[k] == c));
    (open.iter().filter(|p| !on_cell_line(p)))
        .filter(|[a, b]| {
            let m = [f64::from(a[0] + b[0]) / 2.0, f64::from(a[1] + b[1]) / 2.0];
            !ends.iter().any(|(o, d, w, at)| {
                let v = [m[0] - o[0], m[1] - o[1]];
                (v[0] * d[0] + v[1] * d[1]).abs() * mpu <= ON_END_M
                    && libm::hypot(v[0], v[1]) * mpu <= END_REACH_M
                    && ground(*w, at)
            })
        })
        .map(|p| format!("{key:?}: the open piece {p:?} has nothing of storey 0 behind it"))
        .collect()
}

#[test]
fn the_storeys_close_stand_on_their_sheets_meet_their_neighbours_and_leave_storey_0_as_it_was() {
    let mut fix = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    let mut cache = TileCache::new(16);
    let mut faults = Vec::new();
    let mut built: BTreeMap<CellKey, (CellBuild, StoreyMesh)> = BTreeMap::new();
    for key in fix.cells() {
        let without = build_request(fix.request(key), &mut cache, rules, &mut || 0.0).unwrap();
        let with = build_storeys(fix.request(key), &mut cache, rules, &mut || 0.0).unwrap();
        assert_eq!(without.faces, with.faces, "{key:?}: storey 0's faces");
        assert_eq!(without.far, with.far, "{key:?}: the far block");
        if with.storeys.plans.is_empty() {
            assert_eq!(without.records, with.records, "{key:?}: a cell with no deck has the records of storey 0 alone");
            continue;
        }
        // Storey 0 reads the same label and height at a grid of points.
        let c = f64::from(CELL_UNITS as i32);
        for j in 0..GRID {
            for i in 0..GRID {
                let q = [(f64::from(i) + 0.37) * c / f64::from(GRID), (f64::from(j) + 0.61) * c / f64::from(GRID)];
                let (a, b) = (find(&without.records, q), find(&with.records, q));
                let (ha, hb) = (height_at(&without.records, q, a, 500.0).h, height_at(&with.records, q, b, 500.0).h);
                if a != b || ha.to_bits() != hb.to_bits() {
                    faults.push(format!(
                        "{key:?} at {q:?}: storey 0 reads {a:?} {ha} without and {b:?} {hb} with storeys"
                    ));
                }
            }
        }
        for plan in &with.storeys.plans {
            faults.extend(
                partition(plan, window(key, 0.0)).into_iter().map(|f| format!("{key:?} storey {}: {f:?}", plan.level)),
            );
        }
        let mesh = mesh_storeys(key, &with.storeys, &with.records, &rules.structures).unwrap();
        assert_eq!(mesh.stats.unlabelled, 0, "{key:?}: a sheet face has no label");
        faults.extend(closure(&mesh));
        faults.extend(on_paved(&mesh, &with.storeys, &with.records));
        faults.extend(open_without_ground(key, &with.graph, &mesh.open));
        built.insert(key, (with, mesh));
    }
    for (&key, (b, m)) in &built {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if let Some((ob, om)) = built.get(&other) {
                faults.extend(border((m, &b.records), (om, &ob.records), east));
            }
        }
    }
    let triangles: u32 = built.values().map(|(_, m)| m.triangles.len() as u32).sum();
    println!("{} cells with storeys, {triangles} storey triangles, {} faults", built.len(), faults.len());
    assert!(built.len() > 10, "the fixture's decks");
    assert!(faults.is_empty(), "{} faults: {:#?}", faults.len(), &faults[..faults.len().min(20)]);
}
