//! `heights --fixture-steps --out DIR [--grid M] [--cells X,Y;...]`: the step scan of
//! `faces::heights::check` over the fixture cells, with the cell borders. Writes `fixture-steps.json`.
//! `heights --gpu-check --out DIR --places A,B [--at NAME=LAT,LON]`: the blocks of the places' cells
//! (`blocks.bin`) and Rust's heights at their paved and slope points (`points.json`) for
//! `tests/browser/trailview/model-probe.mjs`, which compares the viewer's `heights.wgsl` with them.

use std::collections::BTreeMap;
use std::path::PathBuf;

use obc_ground_plan::faces::heights::check::{border, scan, side_name, BorderKey, BorderValue, Class, Step};
use obc_ground_plan::faces::records::{label, records};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{CellKey, GlobalPoint};
use obc_ground_plan::graph;
use serde_json::{json, Value};

use crate::fixture::{default_dir, Fixture};

/// The sides, whether on a face edge, and the rule of a group of steps.
type ClassKey = (String, String, bool, String);

/// Border points every this many units along each cell line.
const BORDER_STEP: i64 = 512;

struct Cell {
    key: CellKey,
    points: u64,
    steps: Vec<(Step, [String; 2], String)>,
    border: Vec<(BorderKey, BorderValue)>,
    ripple: [Vec<f64>; 2],
}

fn round(v: f64, k: f64) -> f64 {
    (v * k).round() / k
}

fn at_json(key: CellKey, q: [f64; 2]) -> Value {
    let o = key.origin();
    let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + q[0] as i64, y: o.y + q[1] as i64 });
    json!([round(lat, 1e7), round(lon, 1e7)])
}

/// A label as surface, height rule and owner kind, for a report.
fn label_name(words: &[u32], id: Option<u32>) -> String {
    id.map_or("terrain".into(), |id| {
        let w = label(words, id)[0];
        format!("surface {} rule {} owner {}", w & 0xff, w >> 24, (w >> 8) & 0xff)
    })
}

pub fn run() -> Result<(), String> {
    let (mut out, mut grid, mut only) = (None::<PathBuf>, 0.5f64, None::<Vec<(u32, u32)>>);
    let (mut gpu, mut scan_all, mut names, mut at) = (false, false, Vec::new(), Vec::new());
    let (mut size_m, mut px, mut sections) = (64.0, 1280u32, Vec::new());
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--fixture-steps" => scan_all = true,
            "--size-m" => size_m = value()?.parse().map_err(|_| "--size-m: metres")?,
            "--px" => px = value()?.parse().map_err(|_| "--px: a count")?,
            "--sections" => {
                sections = value()?
                    .split(',')
                    .map(|h| h.parse().map_err(|_| "--sections: headings"))
                    .collect::<Result<_, _>>()?
            }
            "--gpu-check" => gpu = true,
            "--why" => {
                for at in value()?.split(';') {
                    let (lat, lon) = at.split_once(',').ok_or("--why LAT,LON;...")?;
                    why(lat.parse().map_err(|_| "--why: a latitude")?, lon.parse().map_err(|_| "--why: a longitude")?)?;
                }
                return Ok(());
            }
            "--places" => names = value()?.split(',').map(String::from).collect(),
            "--at" => {
                let text = value()?;
                let parse = || -> Option<(String, f64, f64)> {
                    let (name, ll) = text.split_once('=')?;
                    let (lat, lon) = ll.split_once(',')?;
                    Some((name.to_string(), lat.parse().ok()?, lon.parse().ok()?))
                };
                at.push(parse().ok_or("--at NAME=LAT,LON")?);
            }
            "--out" => out = Some(value()?.into()),
            "--grid" => grid = value()?.parse().map_err(|_| "--grid: metres")?,
            "--cells" => {
                let cells = value()?;
                let parse = |c: &str| -> Option<(u32, u32)> {
                    let (x, y) = c.split_once(',')?;
                    Some((x.parse().ok()?, y.parse().ok()?))
                };
                only = Some(cells.split(';').map(|c| parse(c).ok_or("--cells X,Y;...")).collect::<Result<_, _>>()?);
            }
            _ => return Err(format!("heights: unknown option {arg}")),
        }
    }
    let out =
        out.ok_or("heights --out DIR (--fixture-steps | --gpu-check | --why LAT,LON | --places A,B --at N=LAT,LON)")?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    if !scan_all {
        let views = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../builder/web/tests/browser/trailview/road-views-top.json");
        let mut places = crate::named_places(&views, &names)?;
        places.extend(at);
        return if gpu {
            gpu_check(&out, &places)
        } else {
            crate::heights_view::run(&out, &places, size_m, px, &sections)
        };
    }
    let dir = default_dir();
    let mut fix = Fixture::open(&dir);
    let mut cells = crate::graph::fixture_cells(&fix, &dir);
    if let Some(only) = &only {
        cells.retain(|k| only.contains(&(k.x, k.y)));
    }
    let rules = Rules::embedded();
    let scans: Vec<Cell> = fix.map(&cells, |key, input| {
        let graph = graph::build(key, &input, &rules.sections);
        let words = records(&graph, &input, rules, key, 0, true).words;
        let s = scan(&words, &input.terrain, key, grid);
        let steps = (s.steps.into_iter())
            .map(|st| {
                let mut sides = st.sides.map(|h| side_name(&words, &h));
                sides.sort();
                let lab = format!("{} | {}", label_name(&words, st.labels[0]), label_name(&words, st.labels[1]));
                (st, sides, lab)
            })
            .collect();
        let border = border(&words, &input.terrain, key, BORDER_STEP);
        Cell { key, points: s.points, steps, border, ripple: s.ripple }
    });
    let mut by: BTreeMap<ClassKey, (u64, f64, Value)> = BTreeMap::new();
    let mut by_rule: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    let mut unnamed = Vec::new();
    let mut inside_by: BTreeMap<String, u64> = BTreeMap::new();
    let mut inside = Vec::new();
    let (mut points, mut total, mut on_edges) = (0u64, 0u64, 0u64);
    for c in &scans {
        points += c.points;
        for (st, sides, lab) in &c.steps {
            total += 1;
            let edge = st.class != Class::Inside;
            on_edges += u64::from(edge);
            let rule = match st.class {
                Class::Named(r) => format!("{r:?}"),
                c => format!("{c:?}"),
            };
            let r = by_rule.entry(rule.clone()).or_default();
            (r.0, r.1) = (r.0 + 1, r.1.max(st.dh.abs()));
            if st.class == Class::Unnamed {
                unnamed.push(json!({"at": at_json(c.key, st.a), "dh": round(st.dh, 1e3), "sides": sides, "h": [st.sides[0].h, st.sides[1].h], "label": lab}));
            }
            let e = by.entry((sides[0].clone(), sides[1].clone(), edge, rule)).or_insert((0, 0.0, Value::Null));
            e.0 += 1;
            if st.dh.abs() > e.1 {
                (e.1, e.2) = (st.dh.abs(), at_json(c.key, st.a));
            }
            if !edge {
                *inside_by.entry(lab.clone()).or_default() += 1;
                inside.push(json!({"at": at_json(c.key, st.a), "dh": round(st.dh, 1e3), "label": lab, "cell": [c.key.x, c.key.y], "q": st.a}));
            }
        }
    }
    let classes: Vec<Value> = (by.iter())
        .map(|((a, b, e, r), (n, m, at))| json!({"sides": [a, b], "edge": e, "rule": r, "count": n, "max_m": round(*m, 1e3), "worst_at": at}))
        .collect();
    let mut pairs: BTreeMap<BorderKey, Vec<&BorderValue>> = BTreeMap::new();
    for c in &scans {
        for (k, v) in &c.border {
            pairs.entry(*k).or_default().push(v);
        }
    }
    let (mut both, mut equal, mut t_equal, mut over, mut max_dh) = (0u64, 0u64, 0u64, 0u64, 0.0f64);
    let mut worst = Vec::new();
    for (k, v) in pairs.iter().filter(|(_, v)| v.len() == 2) {
        both += 1;
        equal += u64::from(v[0].h == v[1].h);
        t_equal += u64::from(v[0].t == v[1].t);
        let d = (f64::from_bits(v[0].h) - f64::from_bits(v[1].h)).abs();
        max_dh = max_dh.max(d);
        if d > 0.005 {
            over += 1;
            let g = if k.0 == 0 { GlobalPoint { x: k.1, y: k.2 } } else { GlobalPoint { x: k.2, y: k.1 } };
            let (lon, lat) = crate::lon_lat(g);
            worst.push(
                json!({"at": [round(lat, 1e7), round(lon, 1e7)], "dh": round(d, 1e4), "sides": [v[0].name, v[1].name]}),
            );
        }
    }
    let ripple: [Value; 2] = [0, 1].map(|a| {
        let v: Vec<f64> = scans.iter().flat_map(|c| c.ripple[a].iter().copied()).filter(|x| !x.is_nan()).collect();
        ripple_stats(v)
    });
    let report = json!({
        "ripple_mm": {"along": ripple[0], "across": ripple[1]},
        "cells": scans.len(), "grid_m": grid, "points": points,
        "steps": total, "on_edges": on_edges, "inside": total - on_edges,
        "inside_by_label": inside_by,
        "by_rule": by_rule.iter().map(|(k, (n, m))| (k.clone(), json!({"count": n, "max_m": round(*m, 1e3)}))).collect::<serde_json::Map<_, _>>(),
        "border": {"points": both, "bit_equal": equal, "terrain_bit_equal": t_equal, "over_5mm": over, "max_dh_m": max_dh, "worst": worst},
        "classes": classes,
    });
    let write = |name: &str, v: &Value| {
        std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap()).map_err(|e| e.to_string())
    };
    write("fixture-steps.json", &report)?;
    write("inside.json", &Value::Array(inside))?;
    write("unnamed.json", &Value::Array(unnamed))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"ripple_mm": report["ripple_mm"], "steps": total, "on_edges": on_edges, "inside": total - on_edges, "by_rule": report["by_rule"], "inside_by_label": report["inside_by_label"], "border": {"points": both, "bit_equal": equal, "terrain_bit_equal": t_equal, "over_5mm": over, "max_dh_m": max_dh}})).unwrap()
    );
    Ok(())
}

/// Points per side of a place's square for the GPU check, and the square's side.
const GPU_SIDE: usize = 160;
const GPU_SIZE_M: f64 = 60.0;

fn gpu_check(out: &std::path::Path, places: &[(String, f64, f64)]) -> Result<(), String> {
    let dir = default_dir();
    let mut fix = Fixture::open(&dir);
    let rules = Rules::embedded();
    let mut blocks: Vec<u32> = Vec::new();
    let mut at: BTreeMap<CellKey, (usize, obc_ground_plan::CellInput)> = BTreeMap::new();
    let mut points = Vec::new();
    for (name, lat, lon) in places {
        let centre = obc_ground_plan::frame::from_lon_lat(*lon, *lat);
        let upm = 1.0 / obc_ground_plan::frame::metres_per_unit(centre.y);
        let mut n = [0u32; 3];
        for j in 0..GPU_SIDE {
            for i in 0..GPU_SIDE {
                let off = |k: usize| (k as f64 / (GPU_SIDE - 1) as f64 - 0.5) * GPU_SIZE_M * upm;
                let g = [centre.x as f64 + off(i) + 0.37, centre.y as f64 + off(j) + 0.21];
                let key = CellKey::of(GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
                if let std::collections::btree_map::Entry::Vacant(e) = at.entry(key) {
                    let Ok(input) = fix.try_cell(key) else { continue };
                    let graph = graph::build(key, &input, &rules.sections);
                    let words = records(&graph, &input, rules, key, 0, true).words;
                    e.insert((blocks.len(), input));
                    blocks.extend(words);
                }
                let (offset, input) = &at[&key];
                let o = key.origin();
                let q = [g[0] - o.x as f64, g[1] - o.y as f64];
                let words = &blocks[*offset..];
                let Some((h, label)) = obc_ground_plan::faces::heights::check::at(words, &input.terrain, key, q) else {
                    continue;
                };
                let t = obc_ground_plan::faces::heights::check::terrain_at(&input.terrain, key, q).unwrap_or(0.0);
                let kind = h.kind as u32;
                n[kind as usize] += 1;
                if kind > 0 {
                    points.push(json!([offset, q[0], q[1], label.unwrap_or(u32::MAX), t, h.h, kind, name]));
                }
            }
        }
        println!("{name}: terrain {} slope {} paved {}", n[0], n[1], n[2]);
    }
    let bytes: Vec<u8> = blocks.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(out.join("blocks.bin"), bytes).map_err(|e| e.to_string())?;
    std::fs::write(out.join("points.json"), serde_json::to_string(&points).unwrap()).map_err(|e| e.to_string())?;
    println!("{} points in {} cells", points.len(), at.len());
    Ok(())
}

/// The records and both sides' causes at a point and at the points 2 units about it along x and y.
fn why(lat: f64, lon: f64) -> Result<(), String> {
    use obc_ground_plan::faces::heights::check::{explain, terrain_at};
    use obc_ground_plan::faces::records::find;
    let g = obc_ground_plan::frame::from_lon_lat(lon, lat);
    let key = CellKey::of(g);
    let mut fix = Fixture::open(&default_dir());
    let input = fix.try_cell(key).map_err(|e| format!("{key:?}: {e:?}"))?;
    let rules = Rules::embedded();
    let graph = graph::build(key, &input, &rules.sections);
    let words = records(&graph, &input, rules, key, 0, true).words;
    let o = key.origin();
    let q = [(g.x - o.x) as f64, (g.y - o.y) as f64];
    println!("{lat},{lon}: cell {key:?}");
    // The nearest face edge along x or y within 32 units: explained at its first point.
    let edge = (1..=32).flat_map(|k| [[k, 0], [0, k]]).find_map(|[dx, dy]| {
        let (a, b) = ([q[0] - f64::from(dx), q[1] - f64::from(dy)], [q[0] + f64::from(dx), q[1] + f64::from(dy)]);
        let labels = [find(&words, a), find(&words, b)];
        (labels[0] != labels[1]).then_some((a, labels))
    });
    let (a, labels) = edge.unwrap_or((q, [find(&words, q); 2]));
    println!("{} | {}", label_name(&words, labels[0]), label_name(&words, labels[1]));
    print!("{}", explain(&words, a, labels, terrain_at(&input.terrain, key, a).unwrap_or(f64::NAN)));
    Ok(())
}

/// Quantiles of second differences, in mm.
pub fn ripple_stats(v: Vec<f64>) -> Value {
    let mut v: Vec<f64> = v.into_iter().map(|x| x.abs() * 1000.0).collect();
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f64::total_cmp);
    let q = |f: f64| round(v[((v.len() - 1) as f64 * f).round() as usize], 100.0);
    json!({"n": v.len(), "p50": q(0.5), "p95": q(0.95), "p99": q(0.99), "max": q(1.0)})
}
