//! The layer `faces` and the face bench with checks 1 and 2 over the fixture.

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::{json, Value};
use tiny_skia::{FillRule, PathBuilder, Transform};

use crate::draw::{cell_square, paint, path, stroke, Canvas};
use crate::fixture::Fixture;
use obc_ground_plan::faces::check::{artefacts, kinks, neighbours, partition, Artefact, Fault, Kink};
use obc_ground_plan::faces::{self, window, FacePlan, Owner, Rules, Surface};
use obc_ground_plan::frame::{CellKey, GlobalPoint};
use obc_ground_plan::graph::{self, WayGraph};

fn colour(f: &obc_ground_plan::faces::Face) -> Option<(u8, u8, u8, u8)> {
    Some(match (f.surface, f.owner) {
        (Surface::Terrain, _) => return None,
        (Surface::Slope, _) => (205, 190, 120, 110),
        (Surface::Carriageway, Owner::Junction(_)) => (150, 90, 190, 255),
        (Surface::Carriageway, _) => (105, 105, 110, 255),
        (Surface::Shoulder, _) if f.rank == 2 => (160, 160, 150, 255),
        (Surface::Shoulder, _) => (190, 182, 160, 255),
        (Surface::Verge, _) => (140, 185, 110, 255),
        (Surface::Crossing, _) => (210, 60, 60, 255),
        (Surface::RailBed, Owner::Junction(_)) => (165, 110, 80, 255),
        (Surface::RailBed, _) => (125, 95, 75, 255),
        _ => (230, 160, 50, 255),
    })
}

fn draw_plan(canvas: &mut Canvas, plan: &FacePlan, graph: &WayGraph, rules: &Rules) {
    let o = plan.cell.origin();
    let g = |p: [i32; 2]| GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) };
    let mask = canvas.mask(cell_square(plan.cell));
    for f in &plan.faces {
        let Some((r, gr, b, a)) = colour(f) else { continue };
        let mut builder = PathBuilder::new();
        for ring in &f.rings {
            for (i, &v) in ring.iter().enumerate() {
                let (x, y) = canvas.px(g(plan.vertices[v as usize]));
                if i == 0 {
                    builder.move_to(x, y);
                } else {
                    builder.line_to(x, y);
                }
            }
            builder.close();
        }
        if let Some(p) = builder.finish() {
            canvas.pixmap.fill_path(&p, &paint(r, gr, b, a), FillRule::EvenOdd, Transform::identity(), mask.as_ref());
        }
    }
    for e in &plan.edges {
        let (a, b) = (g(plan.vertices[e.a as usize]), g(plan.vertices[e.b as usize]));
        if let Some(p) = path(canvas, &[a, b], false) {
            canvas.pixmap.stroke_path(
                &p,
                &paint(20, 20, 20, 150),
                &stroke(0.6, false),
                Transform::identity(),
                mask.as_ref(),
            );
        }
    }
    // Kinks: orange rings.
    let inner = |f: &obc_ground_plan::faces::Face| {
        let class = f.material.map_or(0, |w| graph.ways[w as usize].class as usize);
        rules.sections.class_faces(class).map_or(0.0, |c| c.inner_radius_m)
    };
    for k in kinks(plan, &inner) {
        let (x, y) = canvas.px(g(k.at));
        if let Some(p) = PathBuilder::from_circle(x, y, 9.0) {
            canvas.pixmap.stroke_path(
                &p,
                &paint(240, 140, 0, 255),
                &stroke(2.5, false),
                Transform::identity(),
                mask.as_ref(),
            );
        }
    }
    // Artefacts: magenta rings.
    for a in artefacts(plan, graph) {
        let (x, y) = canvas.px(g(a.at));
        if let Some(p) = PathBuilder::from_circle(x, y, 6.0) {
            canvas.pixmap.stroke_path(
                &p,
                &paint(230, 0, 200, 255),
                &stroke(2.0, false),
                Transform::identity(),
                mask.as_ref(),
            );
        }
    }
    // The circle of each round: blue, with its centre.
    for c in &plan.corners {
        let (x, y) = canvas.px(g(c.centre));
        let r = canvas.metres(c.radius_m);
        for (circle, width) in [(PathBuilder::from_circle(x, y, r), 1.2), (PathBuilder::from_circle(x, y, 2.0), 3.0)] {
            if let Some(p) = circle {
                canvas.pixmap.stroke_path(
                    &p,
                    &paint(30, 80, 220, 220),
                    &stroke(width, false),
                    Transform::identity(),
                    mask.as_ref(),
                );
            }
        }
    }
    for j in &graph.junctions {
        let (x, y) = canvas.px(g(j.pos));
        if let Some(p) = PathBuilder::from_circle(x, y, 2.5) {
            canvas.pixmap.fill_path(
                &p,
                &paint(0, 160, 60, 255),
                FillRule::Winding,
                Transform::identity(),
                mask.as_ref(),
            );
        }
    }
}

/// The faces of each cell inside its square: carriageways grey, junction faces purple, shoulders
/// grey-tan, slopes olive, rail beds brown, crossings red; edges thin; artefacts in magenta rings;
/// junction points green.
pub fn layer(canvas: &mut Canvas, plans: &BTreeMap<CellKey, (FacePlan, WayGraph)>, rules: &Rules) {
    for (plan, graph) in plans.values() {
        draw_plan(canvas, plan, graph, rules);
    }
}

fn quantiles(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort_by(f64::total_cmp);
    let q = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
    json!({"n": v.len(), "median": q(0.5), "p95": q(0.95), "max": q(1.0)})
}

/// A cell with this many junctions or more is a town cell in the profile.
const TOWN_JUNCTIONS: u32 = 20;

/// One cell of the bench: its plan and graph, the build time (the least of `runs`) and its checks.
struct Built {
    plan: FacePlan,
    graph: WayGraph,
    ms: f64,
    faults: Vec<Fault>,
    kinks: Vec<Kink>,
    artefacts: Vec<Artefact>,
}

/// A cell's build of least time of `runs` and its check 1 and kinks; None if a build panics.
fn bench_cell(key: CellKey, input: &obc_ground_plan::CellInput, rules: &Rules, runs: usize) -> Option<Built> {
    let graph = graph::build(key, input, &rules.sections);
    let (mut best, mut plan) = (f64::INFINITY, None);
    for _ in 0..runs {
        let start = Instant::now();
        let built =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| faces::build(&graph, input, rules, key, 0, true)));
        best = best.min(start.elapsed().as_secs_f64() * 1e3);
        plan = Some(built.ok()?);
    }
    let plan = plan?;
    let inner = |f: &obc_ground_plan::faces::Face| {
        let class = f.material.map_or(0, |w| graph.ways[w as usize].class as usize);
        rules.sections.class_faces(class).map_or(0.0, |c| c.inner_radius_m)
    };
    let (faults, kinks, artefacts) =
        (partition(&plan, window(key, 0.0)), kinks(&plan, &inner), artefacts(&plan, &graph));
    Some(Built { plan, graph, ms: best, faults, kinks, artefacts })
}

/// Per fixture cell: the face build's total time (the run of least time of `runs`), its sizes,
/// check 1 (partition faults and artefacts) and check 2 (each shared border), with panics caught. The
/// cells build on the workers of `Fixture::map`; each prints its line as it ends.
pub fn bench(fixture: &mut Fixture, cells: &[CellKey], rules: &Rules, runs: usize) -> Value {
    let built = fixture.map(cells, |key, input| {
        let b = bench_cell(key, &input, rules, runs);
        match &b {
            Some(b) => {
                let s = &b.plan.stats;
                eprintln!(
                    "cell {} {} {:.1} ms, {} candidates, {} segments, {} faces, {} vertices",
                    key.x, key.y, b.ms, s.candidates, s.slice_segments, s.faces, s.vertices
                );
            }
            None => eprintln!("cell {} {} crash", key.x, key.y),
        }
        b
    });
    let mut total_ms: BTreeMap<bool, Vec<f64>> = BTreeMap::new();
    let mut sizes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut slowest: Vec<(f64, [u32; 2])> = Vec::new();
    let mut plans: BTreeMap<CellKey, (FacePlan, WayGraph)> = BTreeMap::new();
    let (mut partition_faults, mut artefact_cells, mut crashes) = (Vec::new(), Vec::new(), Vec::new());
    let mut kinds: BTreeMap<String, u32> = BTreeMap::new();
    let mut causes: BTreeMap<String, (u32, Vec<Value>)> = BTreeMap::new();
    let (mut kink_count, mut kink_examples) = (0, Vec::new());
    for (&key, b) in cells.iter().zip(built) {
        let Some(Built { plan, graph, ms: total, faults, kinks: bent, artefacts: found }) = b else {
            crashes.push([key.x, key.y]);
            continue;
        };
        let town = plan.stats.junctions >= TOWN_JUNCTIONS;
        total_ms.entry(town).or_default().push(total);
        slowest.push((total, [key.x, key.y]));
        let s = &plan.stats;
        for (name, v) in [
            ("ways", s.ways),
            ("points", s.junctions + s.joints + s.turns),
            ("candidates", s.candidates),
            ("slice_segments", s.slice_segments),
            ("faces", s.faces),
            ("vertices", s.vertices),
        ] {
            sizes.entry(name).or_default().push(f64::from(v));
        }
        for f in faults {
            partition_faults.push(format!("{key:?}: {}", f.what));
        }
        kink_count += bent.len();
        for k in bent.iter().take(2) {
            let o = key.origin();
            let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + i64::from(k.at[0]), y: o.y + i64::from(k.at[1]) });
            if kink_examples.len() < 40 {
                kink_examples.push(json!({"cell": [key.x, key.y], "lat": lat, "lon": lon, "radius_m": k.radius_m}));
            }
        }
        // One count per face and kind.
        let mut seen = std::collections::BTreeSet::new();
        let mut here = 0;
        for x in &found {
            let f = &plan.faces[x.face as usize];
            let owner = match f.owner {
                Owner::Terrain => "terrain",
                Owner::Way(_) => "way",
                Owner::Junction(_) => "junction",
            };
            if !seen.insert((x.face, x.kind)) {
                continue;
            }
            here += 1;
            let o = key.origin();
            let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + i64::from(x.at[0]), y: o.y + i64::from(x.at[1]) });
            let e = causes.entry(format!("{:?} {:?} {owner}", x.kind, f.surface)).or_insert((0, Vec::new()));
            e.0 += 1;
            if e.1.len() < 8 {
                e.1.push(json!({"cell": [key.x, key.y], "lat": lat, "lon": lon}));
            }
            *kinds.entry(format!("{:?}", x.kind)).or_insert(0) += 1;
        }
        if here > 0 {
            artefact_cells.push(json!({"cell": [key.x, key.y], "artefacts": here}));
        }
        plans.insert(key, (plan, graph));
    }
    let mut border_faults = Vec::new();
    let mut borders = 0;
    for (&key, a) in &plans {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            if let Some(b) = plans.get(&other) {
                borders += 1;
                for f in neighbours((&a.0, &a.1), (&b.0, &b.1), east) {
                    border_faults.push(format!("{key:?}|{other:?}: {}", f.what));
                }
            }
        }
    }
    slowest.sort_by(|a, b| b.0.total_cmp(&a.0));
    let profile: serde_json::Map<String, Value> = total_ms
        .into_iter()
        .map(|(town, v)| (format!("{} total", if town { "town" } else { "rural" }), quantiles(v)))
        .collect();
    json!({
        "cells": cells.len(),
        "crashes": crashes,
        "check1_partition_faults": partition_faults.len(),
        "check1_partition_examples": partition_faults.iter().take(10).collect::<Vec<_>>(),
        "check1_artefact_faces": kinds,
        "check1_artefact_cells": artefact_cells,
        "kinks": kink_count,
        "kink_examples": kink_examples,
        "check1_artefact_causes": causes.into_iter().map(|(k, (n, ex))| json!({"cause": k, "faces": n, "examples": ex})).collect::<Vec<_>>(),
        "borders": borders,
        "check2_faults": border_faults.len(),
        "check2_examples": border_faults.iter().take(10).collect::<Vec<_>>(),
        "ms": profile,
        "slowest": slowest.iter().take(5).map(|(t, k)| json!({"cell": k, "ms": t})).collect::<Vec<_>>(),
        "sizes": sizes.into_iter().map(|(k, v)| (k.to_string(), quantiles(v))).collect::<serde_json::Map<_, _>>(),
    })
}

/// The rounds in the picture: per round, its centre in pixels, its radius and its owner's kind.
pub fn corners(canvas: &Canvas, plans: &BTreeMap<CellKey, (FacePlan, WayGraph)>) -> Value {
    let size = canvas.pixmap.width() as f32;
    let mut out = Vec::new();
    for (plan, _) in plans.values() {
        let o = plan.cell.origin();
        for c in &plan.corners {
            let (x, y) = canvas.px(GlobalPoint { x: o.x + i64::from(c.centre[0]), y: o.y + i64::from(c.centre[1]) });
            if (0.0..size).contains(&x) && (0.0..size).contains(&y) {
                let owner = match c.owner {
                    Owner::Junction(_) => "junction",
                    Owner::Way(_) => "way",
                    Owner::Terrain => "terrain",
                };
                out.push(json!({"at": [x.round(), y.round()], "radius_m": c.radius_m, "owner": owner}));
            }
        }
    }
    Value::Array(out)
}

/// One plan per cell for the picture layer.
pub fn plans(
    cells: &BTreeMap<CellKey, obc_ground_plan::CellInput>,
    rules: &Rules,
) -> BTreeMap<CellKey, (FacePlan, WayGraph)> {
    cells
        .iter()
        .map(|(&k, input)| {
            let graph = graph::build(k, input, &rules.sections);
            (k, (faces::build(&graph, input, rules, k, 0, true), graph))
        })
        .collect()
}
