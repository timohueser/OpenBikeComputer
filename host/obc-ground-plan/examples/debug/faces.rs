//! The layer `faces`, the junction contact sheets and the face bench.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Instant;

use serde_json::{json, Value};
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::draw::{cell_square, paint, path, stroke, Canvas};
use crate::fixture::Fixture;
use obc_ground_plan::faces::{build_faces, FacePlan, Owner, Rules, Surface, SEAM, UNRELATED};
use obc_ground_plan::frame::{metres_per_unit, CellKey, GlobalPoint};
use obc_ground_plan::graph::{build, WayGraph};

fn colour(f: &obc_ground_plan::faces::Face) -> Option<(u8, u8, u8, u8)> {
    Some(match (f.surface, f.owner) {
        (Surface::Terrain, _) => return None,
        (Surface::Slope, _) => (205, 190, 120, 110),
        (Surface::Carriageway, Owner::Junction(_)) => (150, 90, 190, 255),
        (Surface::Carriageway, _) => (105, 105, 110, 255),
        (Surface::Shoulder, Owner::Junction(_) | Owner::Bend(..)) => (215, 175, 120, 255),
        (Surface::Shoulder, _) => (185, 182, 170, 255),
        (Surface::Verge, _) => (140, 185, 110, 255),
        (Surface::CornerFill, Owner::Bend(..)) => (30, 170, 190, 255),
        (Surface::CornerFill, _) => (60, 110, 215, 255),
        (Surface::Crossing, _) => (210, 60, 60, 255),
        (Surface::RailBed, _) => (125, 95, 75, 255),
        (Surface::Platform, _) => (230, 160, 50, 255),
    })
}

fn draw_plan(canvas: &mut Canvas, plan: &FacePlan, graph: &WayGraph, only_inside: bool) {
    let o = plan.cell.origin();
    let g = |p: [i32; 2]| GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) };
    let mask = if only_inside { canvas.mask(cell_square(plan.cell)) } else { None };
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
        let (colour, width): (Paint, f32) = if e.flags & SEAM != 0 {
            (paint(250, 220, 0, 255), 2.5)
        } else if e.flags & UNRELATED != 0 {
            (paint(220, 30, 30, 220), 1.2)
        } else {
            (paint(20, 20, 20, 150), 0.6)
        };
        if let Some(p) = path(canvas, &[a, b], false) {
            canvas.pixmap.stroke_path(&p, &colour, &stroke(width, false), Transform::identity(), mask.as_ref());
        }
    }
    for hull in &plan.stats.thin_pieces {
        let pts: Vec<GlobalPoint> = hull.iter().map(|&p| g(p)).collect();
        if let Some(p) = path(canvas, &pts, true) {
            canvas.pixmap.stroke_path(
                &p,
                &paint(230, 0, 200, 255),
                &stroke(1.0, false),
                Transform::identity(),
                mask.as_ref(),
            );
        }
    }
    for arc in &plan.corners {
        let c = g(arc.centre);
        let (x, y) = canvas.px(c);
        let r = canvas.metres(arc.radius_m);
        if let Some(p) = PathBuilder::from_circle(x, y, r) {
            canvas.pixmap.stroke_path(
                &p,
                &paint(20, 60, 200, 120),
                &stroke(0.8, true),
                Transform::identity(),
                mask.as_ref(),
            );
        }
        if let Some(p) = PathBuilder::from_circle(x, y, 1.5) {
            canvas.pixmap.fill_path(
                &p,
                &paint(20, 60, 200, 255),
                FillRule::Winding,
                Transform::identity(),
                mask.as_ref(),
            );
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

/// The faces of each cell inside its square: surfaces by colour (purple a blend part, blue a corner
/// fill, cyan a tip fill, tan a ring, olive a slope), edges thin, seams yellow, unrelated edges red,
/// corner circles dashed blue, junction points green.
pub fn layer(canvas: &mut Canvas, plans: &BTreeMap<CellKey, (FacePlan, WayGraph)>) {
    for (plan, graph) in plans.values() {
        draw_plan(canvas, plan, graph, true);
    }
}

/// The groups of a junction's arms: host groups, then guest groups, each sorted.
fn pair_name(graph: &WayGraph, rules: &Rules, input: &obc_ground_plan::CellInput, j: usize) -> String {
    let group = |w: u32| {
        let part = &graph.parts[graph.ways[w as usize].part as usize];
        let f = &input.features[part.feature.0 as usize];
        rules.sections.way(&f.kind, &f.tags, false).group.to_string()
    };
    let arms = &graph.junctions[j].arms;
    let mut hosts: Vec<String> = arms.iter().filter(|a| a.host).map(|a| group(a.way)).collect();
    let mut guests: Vec<String> = arms.iter().filter(|a| !a.host).map(|a| group(a.way)).collect();
    hosts.sort();
    hosts.dedup();
    guests.sort();
    guests.dedup();
    format!("{}-{}", hosts.join("+"), guests.join("+"))
}

const TILE_M: f64 = 40.0;
const TILE_PX: u32 = 240;
const SHEET: u32 = 8;

/// Contact sheets of every owned junction of the fixture: a 40 m tile each, by the groups of its
/// hosts and guests, 64 tiles a sheet, with `junctions.json` listing each tile.
pub fn sheets(fixture: &mut Fixture, cells: &[CellKey], rules: &Rules, out: &Path) -> Result<Value, String> {
    let mut tiles: BTreeMap<String, Vec<(Pixmap, Value)>> = BTreeMap::new();
    for &key in cells {
        let input = fixture.cell(key);
        let graph = build(key, &input, &rules.sections);
        let plan = build_faces(&graph, &input, rules, key, 0, true);
        let o = key.origin();
        for (j, junction) in graph.junctions.iter().enumerate().filter(|(_, j)| j.owned) {
            let centre = GlobalPoint { x: o.x + i64::from(junction.pos[0]), y: o.y + i64::from(junction.pos[1]) };
            let mpu = metres_per_unit(centre.y);
            let half = TILE_M / 2.0 / mpu;
            let mut canvas = Canvas::new(
                TILE_PX,
                centre.x as f64 - half,
                centre.y as f64 - half,
                2.0 * half / f64::from(TILE_PX),
                mpu,
            );
            draw_plan(&mut canvas, &plan, &graph, false);
            let name = pair_name(&graph, rules, &input, j);
            let (lon, lat) = crate::lon_lat(centre);
            let corners: Vec<Value> = (plan.corners.iter())
                .filter(|c| c.owner == Owner::Junction(j as u32))
                .map(|c| json!((c.radius_m * 100.0).round() / 100.0))
                .collect();
            let info =
                json!({"cell": [key.x, key.y], "junction": j, "lat": lat, "lon": lon, "corner_radii_m": corners});
            tiles.entry(name).or_default().push((canvas.pixmap, info));
        }
    }
    let mut index = serde_json::Map::new();
    for (name, list) in &tiles {
        let mut entries = Vec::new();
        for (sheet, chunk) in list.chunks((SHEET * SHEET) as usize).enumerate() {
            let cols = SHEET.min(chunk.len() as u32);
            let rows = (chunk.len() as u32).div_ceil(SHEET);
            let gap = 4;
            let mut pix = Pixmap::new(cols * (TILE_PX + gap), rows * (TILE_PX + gap)).unwrap();
            pix.fill(tiny_skia::Color::from_rgba8(255, 255, 255, 255));
            for (i, (tile, info)) in chunk.iter().enumerate() {
                let (c, r) = (i as u32 % SHEET, i as u32 / SHEET);
                pix.draw_pixmap(
                    (c * (TILE_PX + gap)) as i32,
                    (r * (TILE_PX + gap)) as i32,
                    tile.as_ref(),
                    &tiny_skia::PixmapPaint::default(),
                    Transform::identity(),
                    None,
                );
                let mut info = info.clone();
                info["sheet"] = json!(sheet);
                info["row"] = json!(r);
                info["column"] = json!(c);
                entries.push(info);
            }
            let file = out.join(format!("junctions-{name}-{sheet}.png"));
            image::save_buffer(&file, pix.data(), pix.width(), pix.height(), image::ColorType::Rgba8)
                .map_err(|e| format!("{}: {e}", file.display()))?;
        }
        index.insert(name.clone(), json!({"count": list.len(), "tiles": entries}));
    }
    Ok(Value::Object(index))
}

fn quantiles(mut v: Vec<f64>) -> Value {
    v.sort_by(f64::total_cmp);
    let q = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
    json!({"median": q(0.5), "p95": q(0.95), "max": q(1.0), "sum": v.iter().sum::<f64>()})
}

/// The face build per fixture cell: its time (the median of `runs`), and its sizes and stats.
pub fn bench(fixture: &mut Fixture, cells: &[CellKey], rules: &Rules, runs: usize) -> Value {
    let mut ms = Vec::new();
    let mut sizes: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut slowest: Vec<(f64, [u32; 2])> = Vec::new();
    let mut totals: BTreeMap<String, f64> = BTreeMap::new();
    let mut radii: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for &key in cells {
        let input = fixture.cell(key);
        let graph = build(key, &input, &rules.sections);
        let mut times = Vec::new();
        let mut plan = None;
        for _ in 0..runs {
            let start = Instant::now();
            plan = Some(build_faces(&graph, &input, rules, key, 0, true));
            times.push(start.elapsed().as_secs_f64() * 1e3);
        }
        times.sort_by(f64::total_cmp);
        let t = times[times.len() / 2];
        ms.push(t);
        slowest.push((t, [key.x, key.y]));
        let plan = plan.unwrap();
        let s = &plan.stats;
        for (name, v) in [
            ("candidates", s.candidates),
            ("pieces", s.pieces),
            ("slice_segments", s.slice_segments),
            ("slices", s.slices),
            ("faces", s.faces),
            ("vertices", s.vertices),
            ("edges", s.edges),
        ] {
            sizes.entry(name).or_default().push(f64::from(v));
        }
        for (name, v) in [
            ("seam_missing", s.seam_missing),
            ("ramp_capped", s.ramp_capped),
            ("platforms_skipped", s.platforms_skipped),
            ("pinches_left", s.pinches_left),
            ("extent_faults", s.extent_faults),
        ] {
            *totals.entry(name.into()).or_default() += f64::from(v);
        }
        for (pair, [built, shrunk, dropped]) in &s.corners {
            for (what, v) in [("built", built), ("shrunk", shrunk), ("dropped", dropped)] {
                *totals.entry(format!("corners {pair} {what}")).or_default() += f64::from(*v);
            }
        }
        for ((a, b), v) in &s.thin {
            *totals.entry(format!("thin {a:?}->{b:?}")).or_default() += f64::from(*v);
        }
        for c in &plan.corners {
            let pair = match c.owner {
                Owner::Junction(_) => "junction",
                _ => "bend or kink",
            };
            radii.entry(pair.into()).or_default().push(c.radius_m);
        }
    }
    slowest.sort_by(|a, b| b.0.total_cmp(&a.0));
    json!({
        "cells": cells.len(),
        "ms": quantiles(ms),
        "slowest": slowest.iter().take(5).map(|(t, k)| json!({"cell": k, "ms": t})).collect::<Vec<_>>(),
        "sizes": sizes.into_iter().map(|(k, v)| (k.to_string(), quantiles(v))).collect::<serde_json::Map<_, _>>(),
        "totals": totals,
        "radii_m": radii.into_iter().map(|(k, v)| (k, quantiles(v))).collect::<serde_json::Map<_, _>>(),
    })
}
