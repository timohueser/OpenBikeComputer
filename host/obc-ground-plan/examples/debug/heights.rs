//! `heights --fixture-steps --out DIR [--grid M] [--cells X,Y;...]`: the step scan of
//! `faces::heights::check` over the fixture cells, with the cell borders, and the steps at label edges
//! (`check::edge_steps`, on a grid of `EDGE_GRID_M`), and the slopes steeper than a rock face. Writes
//! `fixture-steps.json`.
//! `heights --cost DIR`: the cost of the heights per fixture cell on one thread (`cost.json`).
//! `heights --gpu-check --out DIR --places A,B [--at NAME=LAT,LON]`: the blocks of the places' cells
//! (`blocks.bin`) and Rust's heights at their paved and slope points (`points.json`) for
//! `tests/browser/trailview/model-probe.mjs`, which compares the viewer's `heights.wgsl` with them, and
//! `find`'s labels at whole-unit points (`labels.json`): the grid, every candidate edge's vertices, the
//! points one unit beside them and the edges' whole midpoints, for its `model.wgsl` check.

use std::collections::BTreeMap;
use std::path::PathBuf;

use obc_ground_plan::faces::heights::check::{
    border, edge_steps, scan, side_name, BorderKey, BorderValue, Class, Step, EDGE_GRID_M,
};
use obc_ground_plan::faces::records::{self, label};
use obc_ground_plan::faces::Rules;
use obc_ground_plan::frame::{CellKey, GlobalPoint};
use obc_ground_plan::graph;
use serde_json::{json, Value};

use crate::fixture::{default_dir, Fixture};

/// The sides, whether on a face edge, and the rule of a group of steps.
type ClassKey = (String, String, bool, String);

/// A vertex of the cost proxy: its point, label and terrain.
type ProxyPoint = ([f64; 2], Option<u32>, f64);

/// Border points every this many units along each cell line.
const BORDER_STEP: i64 = 512;

struct Cell {
    key: CellKey,
    points: u64,
    steps: Vec<(Step, [String; 2], String)>,
    border: Vec<(BorderKey, BorderValue)>,
    ripple: [Vec<f64>; 2],
    /// Grid points with a full cap, and with the host missing.
    audit: [Vec<Value>; 2],
    /// The steps at label edges with no rule, with their sides.
    edges: Vec<(Step, [String; 2])>,
    /// The slope pairs steeper than a rock face (`Scan::steep`): where, the climb per metre.
    steep: Vec<(Value, f64)>,
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
            "--cost" => {
                let out = args.next().ok_or("--cost DIR")?;
                return cost(std::path::Path::new(&out));
            }
            "--why-g" => {
                for at in value()?.split(';') {
                    let (x, y) = at.split_once(',').ok_or("--why-g X,Y;...")?;
                    why_global([x.parse().map_err(|_| "--why-g: x")?, y.parse().map_err(|_| "--why-g: y")?])?;
                }
                return Ok(());
            }
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
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        let words = records::build(&graph, &input, rules, key, 0, true).near.words;
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
        let missing = s
            .host_missing
            .iter()
            .map(|(q, l)| json!({"at": at_json(key, *q), "label": label_name(&words, *l)}))
            .collect();
        let full = s.slots_full.iter().map(|q| at_json(key, *q)).collect();
        let edges = (edge_steps(&words, &input.terrain, key, EDGE_GRID_M).into_iter())
            .map(|st| {
                let mut sides = st.sides.map(|h| side_name(&words, &h));
                sides.sort();
                (st, sides)
            })
            .collect();
        let steep = s.steep.iter().map(|&(q, c)| (at_json(key, q), c)).collect();
        Cell { key, points: s.points, steps, border, ripple: s.ripple, audit: [full, missing], edges, steep }
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
                let o = c.key.origin();
                let g = [st.a, st.b].map(|q| [o.x as f64 + q[0], o.y as f64 + q[1]]);
                unnamed.push(json!({"at": at_json(c.key, st.a), "dh": round(st.dh, 1e3), "sides": sides, "h": [st.sides[0].h, st.sides[1].h], "label": lab, "g": g}));
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
    let (mut worst, mut differ) = (Vec::new(), Vec::new());
    for (k, v) in pairs.iter().filter(|(_, v)| v.len() == 2) {
        both += 1;
        equal += u64::from(v[0].h == v[1].h);
        if v[0].h != v[1].h {
            let g = if k.0 == 0 { [k.1, k.2] } else { [k.2, k.1] };
            let d = f64::from_bits(v[0].h) - f64::from_bits(v[1].h);
            differ.push(json!({"g": g, "dh": d, "sides": [v[0].name, v[1].name]}));
        }
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
    let audit = [0, 1].map(|k| scans.iter().map(|c| c.audit[k].len()).sum::<usize>());
    // The steps at label edges: counts by class and size, and each one.
    let mut edge_by: BTreeMap<String, (u64, f64)> = BTreeMap::new();
    let mut edge_list = Vec::new();
    for c in &scans {
        for (st, sides) in &c.edges {
            let size = match st.dh.abs() {
                d if d < 0.05 => "a: 1-5 cm",
                d if d < 0.2 => "b: 5-20 cm",
                d if d < 0.5 => "c: 20-50 cm",
                _ => "d: over 50 cm",
            };
            for k in [format!("{:?}", st.class), format!("{:?} {size}", st.class)] {
                let e = edge_by.entry(k).or_default();
                (e.0, e.1) = (e.0 + 1, e.1.max(st.dh.abs()));
            }
            edge_list.push(json!({"at": at_json(c.key, st.a), "dh": round(st.dh, 1e3), "class": format!("{:?}", st.class), "sides": sides}));
        }
    }
    let edges = json!({
        "grid_m": EDGE_GRID_M,
        "no_rule": edge_list.len(),
        "by_class": edge_by.iter().map(|(k, (n, m))| (k.clone(), json!({"count": n, "max_m": round(*m, 1e3)}))).collect::<serde_json::Map<_, _>>(),
    });
    // The slope pairs steeper than a rock face: counts over a climb per metre, and the steepest.
    let mut steep: Vec<&(Value, f64)> = scans.iter().flat_map(|c| c.steep.iter()).collect();
    steep.sort_by(|a, b| b.1.total_cmp(&a.1));
    let past = |m: f64| steep.iter().filter(|s| s.1 > m).count();
    let steep = json!({"count": steep.len(), "over_3": past(3.0), "over_6": past(6.0), "over_12": past(12.0),
        "worst": steep.iter().take(10).map(|s| json!({"at": s.0, "per_m": round(s.1, 1e2)})).collect::<Vec<_>>()});
    let lists: [Value; 2] =
        [0, 1].map(|k| Value::Array(scans.iter().flat_map(|c| c.audit[k].iter().cloned()).collect()));
    let report = json!({
        "slots_full": audit[0], "host_missing": audit[1],
        "ripple_mm": {"along": ripple[0], "across": ripple[1]},
        "cells": scans.len(), "grid_m": grid, "points": points,
        "steps": total, "on_edges": on_edges, "inside": total - on_edges,
        "inside_by_label": inside_by,
        "by_rule": by_rule.iter().map(|(k, (n, m))| (k.clone(), json!({"count": n, "max_m": round(*m, 1e3)}))).collect::<serde_json::Map<_, _>>(),
        "border": {"points": both, "bit_equal": equal, "terrain_bit_equal": t_equal, "over_5mm": over, "max_dh_m": max_dh, "worst": worst},
        "edges": edges,
        "steep": steep,
        "classes": classes,
    });
    let write = |name: &str, v: &Value| {
        std::fs::write(out.join(name), serde_json::to_string_pretty(v).unwrap()).map_err(|e| e.to_string())
    };
    write("fixture-steps.json", &report)?;
    write("inside.json", &Value::Array(inside))?;
    write("unnamed.json", &Value::Array(unnamed))?;
    write("slots-full.json", &lists[0])?;
    write("border-differ.json", &Value::Array(differ))?;
    write("host-missing.json", &lists[1])?;
    write("edge-steps.json", &Value::Array(edge_list))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"slots_full": audit[0], "host_missing": audit[1], "ripple_mm": report["ripple_mm"], "steps": total, "on_edges": on_edges, "inside": total - on_edges, "by_rule": report["by_rule"], "inside_by_label": report["inside_by_label"], "border": {"points": both, "bit_equal": equal, "terrain_bit_equal": t_equal, "over_5mm": over, "max_dh_m": max_dh}, "edges": report["edges"], "steep": report["steep"]})).unwrap()
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
    let mut labels = Vec::new();
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
                    let graph = graph::build(key, &input, &rules.sections, &rules.structures);
                    let words = records::build(&graph, &input, rules, key, 0, true).near.words;
                    e.insert((blocks.len(), input));
                    blocks.extend(words);
                }
                let (offset, input) = &at[&key];
                let o = key.origin();
                let q = [g[0] - o.x as f64, g[1] - o.y as f64];
                let words = &blocks[*offset..];
                let whole = [q[0].round(), q[1].round()];
                labels.push(json!([offset, whole[0], whole[1], records::find(words, whole).unwrap_or(u32::MAX)]));
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
    // On and beside the candidates' edges in the places' cells.
    for (offset, _) in at.values() {
        let words = &blocks[*offset..];
        for v in edge_points(words) {
            let q = [f64::from(v[0]), f64::from(v[1])];
            if (0.0..=f64::from(1 << 18)).contains(&q[0]) && (0.0..=f64::from(1 << 18)).contains(&q[1]) {
                labels.push(json!([offset, q[0], q[1], records::find(words, q).unwrap_or(u32::MAX)]));
            }
        }
    }
    let bytes: Vec<u8> = blocks.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(out.join("blocks.bin"), bytes).map_err(|e| e.to_string())?;
    std::fs::write(out.join("points.json"), serde_json::to_string(&points).unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(out.join("labels.json"), serde_json::to_string(&labels).unwrap()).map_err(|e| e.to_string())?;
    println!("{} label points", labels.len());
    println!("{} points in {} cells", points.len(), at.len());
    Ok(())
}

/// The vertices of every candidate edge that a block's bins list, the points one unit beside each and the
/// edges' whole midpoints, each once.
fn edge_points(words: &[u32]) -> std::collections::BTreeSet<[i32; 2]> {
    let mut out = std::collections::BTreeSet::new();
    for b in 0..(records::BINS * records::BINS) as usize {
        let head = words[9] as usize + records::HEADER * b;
        let mut at = words[head] as usize;
        if at == 0 {
            continue;
        }
        for _ in 0..words[head + 1] >> 16 {
            let runs = words[at + 1];
            at += 2;
            for _ in 0..runs {
                let n = words[at] as usize;
                let v = |k: usize| [words[at + 1 + 2 * k] as i32, words[at + 2 + 2 * k] as i32];
                for k in 0..n {
                    let p = v(k);
                    out.extend([p, [p[0] + 1, p[1]], [p[0], p[1] + 1], [p[0] - 1, p[1] - 1]]);
                    if k + 1 < n && (p[0] + v(k + 1)[0]) % 2 == 0 && (p[1] + v(k + 1)[1]) % 2 == 0 {
                        out.insert([(p[0] + v(k + 1)[0]) / 2, (p[1] + v(k + 1)[1]) / 2]);
                    }
                }
                at += 1 + 2 * n;
            }
        }
    }
    out
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
    let graph = graph::build(key, &input, &rules.sections, &rules.structures);
    let words = records::build(&graph, &input, rules, key, 0, true).near.words;
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

/// The records at a global frame point in every cell whose closed square holds it, with the label of
/// each cell there.
fn why_global(g: [f64; 2]) -> Result<(), String> {
    use obc_ground_plan::faces::heights::check::{explain, terrain_at};
    use obc_ground_plan::faces::records::find;
    use obc_ground_plan::frame::CELL_UNITS;
    let mut fix = Fixture::open(&default_dir());
    let rules = Rules::embedded();
    let base = CellKey::of(GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
    for key in [base, CellKey { x: base.x - 1, ..base }, CellKey { y: base.y - 1, ..base }] {
        let o = key.origin();
        let q = [g[0] - o.x as f64, g[1] - o.y as f64];
        if !(0.0..=CELL_UNITS as f64).contains(&q[0]) || !(0.0..=CELL_UNITS as f64).contains(&q[1]) {
            continue;
        }
        let Ok(input) = fix.try_cell(key) else { continue };
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        let words = records::build(&graph, &input, rules, key, 0, true).near.words;
        let l = find(&words, q);
        println!("cell {key:?} q {q:?} label {}", label_name(&words, l));
        print!("{}", explain(&words, q, [l, l], terrain_at(&input.terrain, key, q).unwrap_or(f64::NAN)));
    }
    Ok(())
}

/// The vertex proxy of the near mesh: a grid on paved labels and a coarser one (level 18) on slope
/// labels, and every vertex of the cell's faces that are no terrain, which takes both sides.
const PAVED_GRID_M: f64 = 0.8;
const SLOPE_GRID_M: f64 = 1.63;
/// The least time of this many passes.
const COST_RUNS: usize = 2;

/// The cost of the heights per fixture cell on one thread: labels and terrain first, then the
/// heights alone over the vertex proxy, the least of `COST_RUNS` passes.
fn cost(out: &std::path::Path) -> Result<(), String> {
    use obc_ground_plan::faces::heights::check::terrain_at;
    use obc_ground_plan::faces::heights::{height_at, height_at_sides};
    use obc_ground_plan::faces::records::find;
    use obc_ground_plan::frame::{metres_per_unit, CELL_UNITS};
    use std::time::Instant;
    std::fs::create_dir_all(out).map_err(|e| e.to_string())?;
    let dir = default_dir();
    let mut fix = Fixture::open(&dir);
    let cells = crate::graph::fixture_cells(&fix, &dir);
    let rules = Rules::embedded();
    let mut rows = Vec::new();
    for key in cells {
        let input = fix.cell(key);
        let graph = graph::build(key, &input, &rules.sections, &rules.structures);
        let words = records::build(&graph, &input, rules, key, 0, true).near.words;
        let plan = obc_ground_plan::faces::build(&graph, &input, rules, key, 0, true);
        let mpu = metres_per_unit(key.origin().y + CELL_UNITS / 2);
        let mut grid: [Vec<ProxyPoint>; 2] = [Vec::new(), Vec::new()];
        for (k, step_m) in [(0, PAVED_GRID_M), (1, SLOPE_GRID_M)] {
            let step = step_m / mpu;
            let n = (CELL_UNITS as f64 / step) as usize;
            for j in 0..n {
                for i in 0..n {
                    let q = [(i as f64 + 0.5) * step, (j as f64 + 0.5) * step];
                    let l = find(&words, q);
                    let rule = l.map_or(0, |id| label(&words, id)[0] >> 24);
                    let Some(t) = terrain_at(&input.terrain, key, q) else { continue };
                    if (k == 0 && rule >= 2) || (k == 1 && rule == 1) {
                        grid[k].push((q, l, t));
                    }
                }
            }
        }
        let mut sides: Vec<([f64; 2], [Option<u32>; 2], f64)> = Vec::new();
        for f in plan.faces.iter().filter(|f| f.surface != obc_ground_plan::faces::Surface::Terrain) {
            for ring in &f.rings {
                for &v in ring {
                    let p = plan.vertices[v as usize];
                    let q = [f64::from(p[0]) / 2.0, f64::from(p[1]) / 2.0];
                    if !(0.0..CELL_UNITS as f64).contains(&q[0]) || !(0.0..CELL_UNITS as f64).contains(&q[1]) {
                        continue;
                    }
                    let Some(t) = terrain_at(&input.terrain, key, q) else { continue };
                    sides.push((q, [find(&words, q), find(&words, [q[0] + 0.5, q[1] + 0.5])], t));
                }
            }
        }
        let best = |f: &dyn Fn() -> f64| {
            (0..COST_RUNS)
                .map(|_| {
                    let t0 = Instant::now();
                    std::hint::black_box(f());
                    t0.elapsed().as_secs_f64() * 1e3
                })
                .fold(f64::INFINITY, f64::min)
        };
        let ms: [f64; 2] =
            [0, 1].map(|k| best(&|| grid[k].iter().map(|&(q, l, t)| height_at(&words, q, l, t).h).sum()));
        let ms_sides = best(&|| sides.iter().map(|&(q, l, t)| height_at_sides(&words, q, l, t)[0].h).sum());
        let ms_two = best(&|| {
            sides.iter().map(|&(q, l, t)| height_at(&words, q, l[0], t).h + height_at(&words, q, l[1], t).h).sum()
        });
        let ns = |ms: f64, n: usize| if n == 0 { 0.0 } else { ms * 1e6 / n as f64 };
        rows.push(json!({"cell": [key.x, key.y], "ms": ms[0] + ms[1] + ms_sides, "paved": [grid[0].len(), ns(ms[0], grid[0].len())],
            "slope": [grid[1].len(), ns(ms[1], grid[1].len())], "vertices": [sides.len(), ns(ms_sides, sides.len()), ns(ms_two, sides.len())]}));
    }
    let q = |f: &dyn Fn(&Value) -> f64| {
        let mut v: Vec<f64> = rows.iter().map(f).filter(|x| *x > 0.0).collect();
        v.sort_by(f64::total_cmp);
        let at = |p: f64| round(v[((v.len() - 1) as f64 * p).round() as usize], 1000.0);
        json!({"median": at(0.5), "p95": at(0.95), "max": at(1.0)})
    };
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let report = json!({
        "cells": rows.len(), "load": load.trim(),
        "cell_ms": q(&|r| r["ms"].as_f64().unwrap()),
        "ns_paved": q(&|r| r["paved"][1].as_f64().unwrap()),
        "ns_slope": q(&|r| r["slope"][1].as_f64().unwrap()),
        "ns_sides": q(&|r| r["vertices"][1].as_f64().unwrap()),
        "sides_vs_two_calls": q(&|r| r["vertices"][1].as_f64().unwrap() / r["vertices"][2].as_f64().unwrap().max(1e-9)),
        "rows": rows,
    });
    std::fs::write(out.join("cost.json"), serde_json::to_string_pretty(&report).unwrap()).map_err(|e| e.to_string())?;
    let mut summary = report.clone();
    summary.as_object_mut().unwrap().remove("rows");
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
    Ok(())
}
