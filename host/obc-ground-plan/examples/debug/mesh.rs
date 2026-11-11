//! `debug mesh`: the near mesh over fixture cells on one thread, with each stage's time, the counts,
//! the jump of each view, the checks and wireframes.
//!
//! `debug mesh [--cells all | --places NAME,... [--size-m M]] [--out DIR] [--check]`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use obc_ground_plan::faces::{records, FacePlan};
use obc_ground_plan::frame::{from_lon_lat, metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use obc_ground_plan::mesh::{self, check, CellMesh, Reads, Rim, NO_LABEL, STAGES};
use obc_ground_plan::terrain::Terrain;
use serde_json::{json, Value};

use crate::draw::{self, Canvas};
use crate::fixture::{default_dir, Fixture};

/// The view radius of the jump: the mesh range.
const JUMP_M: f64 = 600.0;
/// Develop's road triangles per view (median, max) and road job (ms, median).
const DEVELOP: &str = "develop: 156k / 409k road triangles per view, road job 72-84 ms median";

struct Cell {
    plan: FacePlan,
    words: Vec<u32>,
    terrain: Terrain,
    rim: Rim,
    /// Input, graph, faces and records, rim (ms).
    times: [f64; 4],
}

fn quantiles(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    json!({"median": q(0.5), "p95": q(0.95), "max": q(1.0), "sum": v.iter().sum::<f64>(), "n": v.len()})
}

/// The fill of a label's surface: grey carriageway, light grey shoulder, tan slope, green terrain.
fn colour(words: &[u32], l: u32) -> (u8, u8, u8) {
    if l == NO_LABEL {
        return (190, 215, 170);
    }
    match records::label(words, l)[0] & 0xff {
        1 => (222, 200, 150),
        2 => (150, 150, 150),
        3 => (185, 185, 175),
        4 => (170, 200, 140),
        5 => (150, 160, 200),
        6 => (110, 110, 140),
        7 => (150, 120, 90),
        8 => (190, 160, 160),
        _ => (214, 226, 200),
    }
}

/// The triangles about a place: filled by the label of their first corner, the strip blue, edges
/// dark; the step slots' pieces orange; the terrain vertices as red dots.
fn picture(
    png: &std::path::Path,
    centre: GlobalPoint,
    size_m: f64,
    meshes: &[(CellKey, CellMesh)],
    cells: &BTreeMap<CellKey, Cell>,
) -> Result<(), String> {
    let mpu = metres_per_unit(centre.y);
    let half = size_m / 2.0 / mpu;
    let mut canvas = Canvas::new(1600, centre.x as f64 - half, centre.y as f64 - half, 2.0 * half / 1600.0, mpu);
    let edge = draw::paint(40, 40, 40, 200);
    let thin = draw::stroke(0.6, false);
    let near = |g: &[GlobalPoint]| {
        g.iter().any(|q| (q.x - centre.x).abs() as f64 <= half && (q.y - centre.y).abs() as f64 <= half)
    };
    for (key, m) in meshes {
        let words = &cells[key].words;
        let o = key.origin();
        let at = |k: u32| {
            let p = m.corner(k as usize).0;
            GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) }
        };
        let first_strip = m.triangles.len() - m.strip_triangles as usize;
        for (k, t) in m.triangles.iter().enumerate() {
            let g = t.map(at);
            if !near(&g) {
                continue;
            }
            let (r, gr, b) = if k >= first_strip { (160, 190, 225) } else { colour(words, m.corner(t[0] as usize).1) };
            let Some(path) = draw::path(&canvas, &g, true) else { continue };
            canvas.pixmap.fill_path(
                &path,
                &draw::paint(r, gr, b, 255),
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
            canvas.pixmap.stroke_path(&path, &edge, &thin, tiny_skia::Transform::identity(), None);
        }
        let step = draw::stroke(2.0, false);
        for s in &m.steps {
            let g = [at(s[0]), at(s[1])];
            if let Some(path) = draw::path(&canvas, &g, false).filter(|_| near(&g)) {
                canvas.pixmap.stroke_path(
                    &path,
                    &draw::paint(240, 130, 20, 255),
                    &step,
                    tiny_skia::Transform::identity(),
                    None,
                );
            }
        }
        for k in 0..m.corners.len() {
            let (p, _, terrain) = m.corner(k);
            let g = GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) };
            if terrain && near(&[g]) {
                let (x, y) = canvas.px(g);
                if let Some(dot) = tiny_skia::PathBuilder::from_circle(x, y, 2.5) {
                    canvas.pixmap.fill_path(
                        &dot,
                        &draw::paint(200, 40, 40, 255),
                        tiny_skia::FillRule::Winding,
                        tiny_skia::Transform::identity(),
                        None,
                    );
                }
            }
        }
    }
    let (w, h) = (canvas.pixmap.width(), canvas.pixmap.height());
    image::save_buffer(png, canvas.pixmap.data(), w, h, image::ColorType::Rgba8)
        .map_err(|e| format!("{}: {e}", png.display()))
}

/// The bytes of a mesh on the GPU: corners, triangles, step slots and sunk bits.
fn bytes(m: &CellMesh) -> usize {
    m.to_bytes().len()
}

pub fn run() -> Result<(), String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut args = std::env::args().skip(2);
    let (mut all, mut places, mut size_m, mut out, mut checks) =
        (false, Vec::new(), 64.0, root.join(".artifacts/ground-plan/mesh"), false);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--cells" => all = value()? == "all",
            "--places" => places = value()?.split(',').map(String::from).collect(),
            "--size-m" => size_m = value()?.parse().map_err(|_| "--size-m: a number")?,
            "--out" => out = PathBuf::from(value()?),
            "--check" => checks = true,
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
    let rules = obc_ground_plan::faces::Rules::embedded();
    let mut cells: BTreeMap<CellKey, Cell> = BTreeMap::new();
    let clock = Instant::now();
    let now = || clock.elapsed().as_secs_f64() * 1e3;
    for &key in &needed {
        let t0 = now();
        let input = fixture.try_cell(key).map_err(|e| format!("{key:?}: {e}"))?;
        let t1 = now();
        let graph = obc_ground_plan::graph::build(key, &input, &rules.sections);
        let t2 = now();
        let built = records::build(&graph, &input, rules, key, 0, true);
        let t3 = now();
        let rim = Rim::of(&built.faces);
        let t4 = now();
        cells.insert(
            key,
            Cell {
                plan: built.faces,
                words: built.near.words,
                terrain: input.terrain,
                rim,
                times: [t1 - t0, t2 - t1, t3 - t2, t4 - t3],
            },
        );
    }
    let mut rows = Vec::new();
    let mut per_stage: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let (mut totals, mut mesh_ms, mut sizes) = (BTreeMap::new(), BTreeMap::new(), BTreeMap::new());
    let mut meshes: Vec<(CellKey, CellMesh)> = Vec::new();
    let (mut faults, mut labels) = (Vec::new(), check::Labels::default());
    for &key in &wanted {
        let cell = &cells[&key];
        let rims: Vec<Rim> = (-1..=1i64)
            .flat_map(|dy| {
                (-1..=1i64).map(move |dx| CellKey { x: (key.x as i64 + dx) as u32, y: (key.y as i64 + dy) as u32 })
            })
            .filter_map(|k| cells.get(&k).map(|c| c.rim.clone()))
            .collect();
        let t0 = now();
        let reads = Reads::of(&cell.plan, &rims);
        let region_ms = now() - t0;
        let mut clock = now;
        let built = mesh::mesh_cell(&cell.plan, &cell.words, &reads, &mut clock);
        let mut row = json!({"cell": [key.x, key.y]});
        for (name, t) in ["input", "graph", "faces+records", "rim"].iter().zip(cell.times) {
            per_stage.entry(name).or_default().push(t);
        }
        let mut total = cell.times.iter().sum::<f64>();
        match built {
            Err(e) => row["error"] = json!(format!("{e:?}")),
            Ok(m) => {
                let t = region_ms + m.times.iter().sum::<f64>();
                total += t;
                mesh_ms.insert(key, t);
                per_stage.entry("mesh").or_default().push(t);
                per_stage.entry("region").or_default().push(region_ms);
                for (name, &t) in STAGES.iter().zip(&m.times) {
                    per_stage.entry(name).or_default().push(t);
                }
                let s = &m.stats;
                sizes.insert(key, (s.triangles + 2 * s.steps, s.corners, bytes(&m)));
                row["stats"] = json!({
                    "cells_r_s_d": s.cells, "points": s.points, "grid_points": s.grid_points, "corners": s.corners,
                    "triangles": s.triangles, "strip_triangles": s.strip_triangles, "steps": s.steps,
                    "outside_d": s.outside_d, "unlabelled": s.unlabelled, "split_edges": s.split_edges, "bytes": bytes(&m),
                    "runs": [m.triangle_runs.len(), m.step_runs.len()],
                    "level18_triangles": s.level18_triangles,
                });
                row["times"] = json!(STAGES
                    .iter()
                    .zip(&m.times)
                    .map(|(n, t)| (n.to_string(), json!(t)))
                    .collect::<serde_json::Map<_, _>>());
                if checks {
                    faults.extend(check::validity(&m).into_iter().map(|f| format!("{key:?}: {f}")));
                    let l = check::labels(&m, &cell.words);
                    labels.sharp_wrong += l.sharp_wrong;
                    labels.ground_wrong += l.ground_wrong;
                    labels.ground_other += l.ground_other;
                    labels.first.extend(l.first.into_iter().take(4).map(|f| format!("{key:?} {f}")));
                }
                meshes.push((key, m));
            }
        }
        row["total_ms"] = json!(total);
        totals.insert(key, total);
        rows.push(row);
    }
    let (mut borders, mut sub_mm) = (0, 0);
    if checks {
        let by: BTreeMap<CellKey, &CellMesh> = meshes.iter().map(|(k, m)| (*k, m)).collect();
        for (&key, a) in &by {
            for (east, other) in
                [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })]
            {
                let Some(b) = by.get(&other) else { continue };
                let cell = |k: CellKey, m| check::Cell {
                    key: k,
                    mesh: m,
                    words: &cells[&k].words,
                    terrain: &cells[&k].terrain,
                };
                let (f, s) = check::border(&cell(key, a), &cell(other, b), east);
                faults.extend(f);
                borders += 1;
                sub_mm += s;
            }
        }
    }
    // The jump: every cell within 600 m of a view, where all of them were meshed.
    let views = std::fs::read_to_string(root.join("builder/web/tests/browser/trailview/road-views.json"))
        .map_err(|e| e.to_string())?;
    let views: Value = serde_json::from_str(&views).map_err(|e| e.to_string())?;
    let (mut jumps, mut jump_total, mut jump_mesh, mut view_tris, mut view_corners, mut view_bytes) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for view in views.as_array().into_iter().flatten() {
        let (Some(name), Some(q)) = (view["name"].as_str(), view["view"].as_str()) else { continue };
        let field = |k: &str| {
            q.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix(k)?.strip_prefix('='))?.parse::<f64>().ok()
        };
        let (Some(lat), Some(lon)) = (field("lat"), field("lon")) else { continue };
        let c = from_lon_lat(lon, lat);
        let reach = (JUMP_M / metres_per_unit(c.y)) as i64;
        let (lo, hi) = (
            CellKey::of(GlobalPoint { x: c.x - reach, y: c.y - reach }),
            CellKey::of(GlobalPoint { x: c.x + reach, y: c.y + reach }),
        );
        let (mut sum, mut mesh_sum, mut tris, mut corners, mut size, mut n, mut missing) = (0.0, 0.0, 0, 0, 0, 0, 0);
        let mut within = Vec::new();
        for y in lo.y..=hi.y {
            for x in lo.x..=hi.x {
                let k = CellKey { x, y };
                let o = k.origin();
                let (dx, dy) =
                    ((c.x - c.x.clamp(o.x, o.x + CELL_UNITS)) as f64, (c.y - c.y.clamp(o.y, o.y + CELL_UNITS)) as f64);
                if libm::hypot(dx, dy) > reach as f64 {
                    continue;
                }
                match (totals.get(&k), mesh_ms.get(&k), sizes.get(&k)) {
                    (Some(t), Some(m), Some(&(tr, co, by))) => {
                        (sum, mesh_sum, tris, corners, size, n) =
                            (sum + t, mesh_sum + m, tris + tr, corners + co, size + by, n + 1);
                        within.push([x, y]);
                    }
                    _ => missing += 1,
                }
            }
        }
        if n > 0 && missing == 0 {
            jumps.push(json!({"view": name, "cells": within, "native_ms": sum, "mesh_ms": mesh_sum, "triangles": tris, "corners": corners, "bytes": size}));
            jump_total.push(sum);
            jump_mesh.push(mesh_sum);
            view_tris.push(f64::from(tris));
            view_corners.push(f64::from(corners));
            view_bytes.push(size as f64);
        }
    }
    let summary = json!({
        "cells": wanted.len(),
        "meshed": meshes.len(),
        "stages": per_stage.iter().map(|(k, v)| (k.to_string(), quantiles(v.clone()))).collect::<serde_json::Map<_, _>>(),
        "total": quantiles(totals.values().copied().collect()),
        "jump_views": jumps.len(),
        "jump_native_ms": quantiles(jump_total),
        "jump_mesh_ms": quantiles(jump_mesh),
        "view_triangles_with_steps": quantiles(view_tris),
        "view_corners": quantiles(view_corners),
        "view_bytes": quantiles(view_bytes),
        "reference": DEVELOP,
        "checks": checks.then(|| json!({"faults": faults.len(), "borders": borders, "sub_mm": sub_mm, "labels": format!("{labels:?}"), "first": faults.iter().take(20).collect::<Vec<_>>()})),
    });
    if !all {
        let views = root.join("builder/web/tests/browser/trailview/road-views-top.json");
        for (name, lat, lon) in crate::named_places(&views, &places)? {
            picture(&out.join(format!("{name}.png")), from_lon_lat(lon, lat), size_m, &meshes, &cells)?;
        }
    }
    let path = out.join("mesh-timing.json");
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&json!({"summary": summary, "cells": rows, "jumps": jumps})).unwrap(),
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    println!("-> {}", path.display());
    Ok(())
}
