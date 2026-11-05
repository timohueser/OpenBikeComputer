//! `debug mesh`: the mesh stages over fixture cells on one thread, with each stage's time, the
//! counts of the triangulations and heights, and the jump estimate of each view.
//!
//! `debug mesh [--cells all | --places NAME,... [--size-m M]] [--out DIR] [--far-only]`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use obc_ground_plan::faces::{records, FacePlan, Rules};
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::mesh::{self, MeshOptions, Window, STAGES};
use obc_ground_plan::terrain::Terrain;
use serde_json::{json, Value};

use crate::fixture::{default_dir, Fixture};

/// The view radius of the jump gate (m): the mesh range.
const JUMP_M: f64 = 600.0;
/// WASM ÷ native of the cell build (GP3R's records: 1.24–1.30).
const WASM_RATIO: f64 = 1.3;

struct Cell {
    plan: FacePlan,
    words: Vec<u32>,
    terrain: Terrain,
    /// Input, graph, records (ms).
    times: [f64; 3],
}

fn quantiles(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    json!({"median": q(0.5), "p95": q(0.95), "max": q(1.0), "sum": v.iter().sum::<f64>(), "n": v.len()})
}

pub fn run() -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut args = std::env::args().skip(2);
    let (mut all, mut places, mut size_m, mut out, mut near) =
        (false, Vec::new(), 64.0, root.join(".artifacts/ground-plan/mesh"), true);
    let mut angle = obc_ground_plan::mesh::params::MIN_ANGLE_DEG;
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--cells" => all = value()? == "all",
            "--places" => places = value()?.split(',').map(String::from).collect(),
            "--size-m" => size_m = value()?.parse().map_err(|_| "--size-m: a number")?,
            "--out" => out = PathBuf::from(value()?),
            "--far-only" => near = false,
            "--min-angle" => angle = value()?.parse().map_err(|_| "--min-angle: degrees")?,
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let dir = default_dir();
    let mut fixture = Fixture::open(&dir);
    let known = crate::graph::fixture_cells(&fixture, &dir);
    let wanted: Vec<CellKey> = if all {
        known.clone()
    } else {
        let views = root.join("builder/web/tests/browser/trailview/road-views-top.json");
        let chosen = crate::named_places(&views, &places)?;
        let mut keys: Vec<CellKey> =
            chosen.iter().flat_map(|(_, lat, lon)| crate::square_cells(*lat, *lon, size_m)).collect();
        keys.sort();
        keys.dedup();
        keys.retain(|k| known.contains(k));
        keys
    };
    let mut needed: Vec<CellKey> = wanted
        .iter()
        .flat_map(|k| (-1..=1).flat_map(move |dy| (-1..=1).map(move |dx| (k.x as i64 + dx, k.y as i64 + dy))))
        .map(|(x, y)| CellKey { x: x as u32, y: y as u32 })
        .filter(|k| known.contains(k))
        .collect();
    needed.sort();
    needed.dedup();
    let rules = Rules::embedded();
    let mut cells: BTreeMap<CellKey, Cell> = BTreeMap::new();
    for &key in &needed {
        let start = Instant::now();
        let input = fixture.try_cell(key).map_err(|e| format!("{key:?}: {e}"))?;
        let t_input = start.elapsed().as_secs_f64() * 1e3;
        let start = Instant::now();
        let graph = obc_ground_plan::graph::build(key, &input, &rules.sections);
        let t_graph = start.elapsed().as_secs_f64() * 1e3;
        let start = Instant::now();
        let built = records::build(&graph, &input, rules, key, 0, true);
        let t_records = start.elapsed().as_secs_f64() * 1e3;
        cells.insert(
            key,
            Cell {
                plan: built.faces,
                words: built.near.words,
                terrain: input.terrain,
                times: [t_input, t_graph, t_records],
            },
        );
    }
    let clock = Instant::now();
    let mut now = || clock.elapsed().as_secs_f64() * 1e3;
    let mut rows = Vec::new();
    let mut per_stage: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut totals: BTreeMap<CellKey, f64> = BTreeMap::new();
    let (mut calls, mut heights_ms) = ([0u64; 3], 0.0);
    for &key in &wanted {
        let cell = &cells[&key];
        let window = Window::new(key, cells.values().map(|c| &c.plan));
        let built =
            mesh::build(&window, &cell.words, &cell.terrain, MeshOptions { near, min_angle_deg: angle }, &mut now);
        let mut row = json!({"cell": [key.x, key.y]});
        if std::env::var("MESH_PLAN_STATS").is_ok() {
            let p = &cell.plan;
            let mpu = metres_per_unit(key.origin().y);
            let mut l: Vec<f64> = (p.edges.iter())
                .filter(|e| e.left != obc_ground_plan::faces::OUTSIDE && e.right != obc_ground_plan::faces::OUTSIDE)
                .map(|e| {
                    let (a, b) = (p.vertices[e.a as usize], p.vertices[e.b as usize]);
                    libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])) * mpu
                })
                .collect();
            l.sort_by(f64::total_cmp);
            let q = |x: f64| l[((l.len() - 1) as f64 * x) as usize];
            let short = |m: f64| l.iter().filter(|&&v| v < m).count();
            println!(
                "{key:?}: {} faces, {} edges; length q01 {:.4} q10 {:.3} q50 {:.3} q90 {:.3}; under 1 cm {}, 5 cm {}, 20 cm {}",
                p.faces.len(), l.len(), q(0.01), q(0.1), q(0.5), q(0.9), short(0.01), short(0.05), short(0.2)
            );
        }
        for (name, t) in ["input", "graph", "faces+records"].iter().zip(cell.times) {
            per_stage.entry(name).or_default().push(t);
        }
        let mut total = cell.times.iter().sum::<f64>();
        match built {
            Err(e) => row["error"] = json!(format!("{e:?}")),
            Ok(b) => {
                total += b.times.iter().sum::<f64>();
                for (name, &t) in STAGES.iter().zip(&b.times) {
                    per_stage.entry(name).or_default().push(t);
                }
                heights_ms += b.times[2] + b.times[4];
                let mut ranges = serde_json::Map::new();
                for r in &b.ranges {
                    let (s, h) = (&r.tris.stats, &r.heights.stats);
                    calls[0] += h.inside_calls;
                    calls[1] += h.sides_calls;
                    calls[2] += h.normal_calls;
                    ranges.insert(
                        format!("{:?}", r.range).to_lowercase(),
                        json!({
                            "triangles": r.tris.tris.len(),
                            "points": r.tris.points.len(),
                            "corners": r.heights.corners.len(),
                            "constraint_points": s.constraint_points,
                            "grid_points": s.grid_points,
                            "steiner": s.steiner,
                            "segment_splits": s.segment_splits,
                            "held_segments": s.held_segments,
                            "held_locked": s.held_locked,
                            "refine_incomplete": s.refine_incomplete,
                            "small_corner": s.small_corner,
                            "inside_calls": h.inside_calls,
                            "sides_calls": h.sides_calls,
                            "normal_calls": h.normal_calls,
                            "thin_normals": h.thin_normals,
                            "unknown": h.unknown,
                        }),
                    );
                }
                row["ranges"] = Value::Object(ranges);
                row["outside_d"] = json!(b.outside_d);
                row["strip_growths"] = json!(b.region.strip_growths);
                row["times"] = json!(STAGES
                    .iter()
                    .zip(&b.times)
                    .map(|(n, t)| (n.to_string(), json!(t)))
                    .collect::<serde_json::Map<_, _>>());
            }
        }
        row["total_ms"] = json!(total);
        totals.insert(key, total);
        rows.push(row);
    }
    // The jump estimate: the build time of every cell within 600 m of a view, times the WASM ratio.
    let views = std::fs::read_to_string(root.join("builder/web/tests/browser/trailview/road-views.json"))
        .map_err(|e| e.to_string())?;
    let views: Value = serde_json::from_str(&views).map_err(|e| e.to_string())?;
    let mut jumps = Vec::new();
    for view in views.as_array().into_iter().flatten() {
        let (Some(name), Some(q)) = (view["name"].as_str(), view["view"].as_str()) else { continue };
        let field = |k: &str| {
            q.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix(k)?.strip_prefix('='))?.parse::<f64>().ok()
        };
        let (Some(lat), Some(lon)) = (field("lat"), field("lon")) else { continue };
        let c = from_lon_lat(lon, lat);
        let reach = (JUMP_M / metres_per_unit(c.y)) as i64;
        let (lo, hi) = (
            CellKey::of(obc_ground_plan::frame::GlobalPoint { x: c.x - reach, y: c.y - reach }),
            CellKey::of(obc_ground_plan::frame::GlobalPoint { x: c.x + reach, y: c.y + reach }),
        );
        let (mut sum, mut n, mut missing) = (0.0, 0, 0);
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let k = CellKey { x, y };
                let o = k.origin();
                let dx = (c.x - c.x.clamp(o.x, o.x + CELL_UNITS)) as f64;
                let dy = (c.y - c.y.clamp(o.y, o.y + CELL_UNITS)) as f64;
                if libm::hypot(dx, dy) > reach as f64 {
                    continue;
                }
                match totals.get(&k) {
                    Some(t) => {
                        sum += t;
                        n += 1;
                    }
                    None => missing += 1,
                }
            }
        }
        if n > 0 {
            jumps.push(
                json!({"view": name, "cells": n, "missing": missing, "native_ms": sum, "wasm_ms": sum * WASM_RATIO}),
            );
        }
    }
    let total_calls = calls.iter().sum::<u64>().max(1);
    let summary = json!({
        "cells": wanted.len(),
        "near": near,
        "min_angle_deg": angle,
        "stages": per_stage.iter().map(|(k, v)| (k.to_string(), quantiles(v.clone()))).collect::<serde_json::Map<_, _>>(),
        "total": quantiles(totals.values().copied().collect()),
        "calls": {"inside": calls[0], "sides": calls[1], "normals": calls[2]},
        "heights_ns_per_call": heights_ms * 1e6 / total_calls as f64,
        "jumps": jumps,
    });
    let path = out.join("mesh-timing.json");
    std::fs::write(&path, serde_json::to_string_pretty(&json!({"summary": summary, "cells": rows})).unwrap())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    println!("-> {}", path.display());
    Ok(())
}
