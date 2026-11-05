//! `model`: the records of the far sampling (`faces::records`) over every fixture cell, for the
//! viewer's `ground=model`, with their build times against the whole face stage, their sizes, and the
//! agreement of their labels with the overlay's faces at points of every bin that holds a candidate.
//! Prototype of the one road model; not for merge.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use obc_ground_plan::faces::records::{self, find, BINS, BIN_UNITS, LABEL};
use obc_ground_plan::faces::{self, FacePlan, Owner, Rules};
use obc_ground_plan::frame::{metres_per_unit, CellKey, CELL_UNITS};
use obc_ground_plan::graph;
use serde_json::{json, Value};

use crate::fixture::{self, Fixture};

/// An edge from one point to another, in frame units.
type Edge = ([f64; 2], [f64; 2]);

/// The file's magic: a table of cells, then their blocks.
const MAGIC: u32 = 0x4f42_4d46;
/// Points per bin side of the agreement sample.
const SAMPLES: i32 = 10;
/// A point this near a boundary between two labels is left out of the count: one level-18 texel.
const TEXEL_M: f64 = 0.41;

struct Cell {
    key: CellKey,
    words: Vec<u32>,
    stats: records::RecordStats,
    graph_ms: f64,
    records_ms: f64,
    faces_ms: f64,
    /// The far level of detail: its build time, words and parts; the compact bytes of both levels.
    far_ms: f64,
    far: records::RecordStats,
    compact: [u64; 2],
    /// Points of the near records with a paved label, and of them those whose far label differs.
    far_agree: [u64; 2],
    junctions: u32,
    /// Points sampled, points near a boundary, points that disagree, and some places.
    agree: [u64; 3],
    places: Vec<Value>,
    /// Face edges between two labels, in world units: x0, y0, x1, y1.
    edges: Vec<[i32; 4]>,
}

fn quantiles(mut v: Vec<f64>) -> Value {
    if v.is_empty() {
        return json!(null);
    }
    v.sort_by(f64::total_cmp);
    let q = |f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
    json!({"n": v.len(), "median": q(0.5), "p95": q(0.95), "max": q(1.0)})
}

/// The label words of a face, as the records name them: material and crossing by line.
fn face_label(f: &faces::Face, line_of: &dyn Fn(u32) -> u32) -> [u32; 4] {
    let (kind, id) = match f.owner {
        Owner::Terrain => (records::TERRAIN, 0),
        Owner::Way(w) => (records::WAY_OWNER, w),
        Owner::Junction(j) => (records::JUNCTION, j),
    };
    [
        f.surface as u32 | kind << 8 | u32::from(f.rank) << 16 | (f.rule as u32) << 24,
        id,
        f.material.map_or(u32::MAX, line_of),
        f.crossing.map_or(u32::MAX, line_of),
    ]
}

fn winding(rings: &[Vec<[f64; 2]>], q: [f64; 2]) -> i32 {
    let mut w = 0;
    for ring in rings {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let side = (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0]);
            if a[1] <= q[1] {
                if b[1] > q[1] && side > 0.0 {
                    w += 1;
                }
            } else if b[1] <= q[1] && side < 0.0 {
                w -= 1;
            }
        }
    }
    w
}

fn seg_dist(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> f64 {
    let (u, v) = ([b[0] - a[0], b[1] - a[1]], [q[0] - a[0], q[1] - a[1]]);
    let l2 = u[0] * u[0] + u[1] * u[1];
    let t = if l2 > 0.0 { ((u[0] * v[0] + u[1] * v[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (dx, dy) = (v[0] - t * u[0], v[1] - t * u[1]);
    (dx * dx + dy * dy).sqrt()
}

/// The far level's labels against the near ones at 8 × 8 points of every bin with a candidate,
/// where the near label is a carriageway: they must be the same label (by its words, as the two
/// blocks number their labels apart).
fn far_agreement(near: &[u32], far: &[u32]) -> [u64; 2] {
    let words = |w: &[u32], id: Option<u32>| {
        id.map(|id| {
            let at = w[5] as usize + LABEL * id as usize;
            [w[at], w[at + 1], w[at + 2], w[at + 3]]
        })
    };
    let heads = near[9] as usize;
    let mut count = [0u64; 2];
    for b in 0..(BINS * BINS) as usize {
        if near[heads + records::HEADER * b + 1] >> 16 == 0 {
            continue;
        }
        let (c, r) = (b as i32 % BINS, b as i32 / BINS);
        for j in 0..8 {
            for i in 0..8 {
                let q = [
                    f64::from(c * BIN_UNITS) + (f64::from(i) + 0.37) * f64::from(BIN_UNITS) / 8.0,
                    f64::from(r * BIN_UNITS) + (f64::from(j) + 0.61) * f64::from(BIN_UNITS) / 8.0,
                ];
                let n = words(near, find(near, q).label);
                if n.is_none_or(|n| n[0] & 0xff != faces::Surface::Carriageway as u32) {
                    continue;
                }
                count[0] += 1;
                if words(far, find(far, q).label) != n {
                    count[1] += 1;
                }
            }
        }
    }
    count
}

/// The agreement of the records with the faces at SAMPLES² points of each bin with a candidate.
fn agreement(key: CellKey, words: &[u32], plan: &FacePlan, lines: &[u32]) -> ([u64; 3], Vec<Value>, Vec<[i32; 4]>) {
    let line_of = |w: u32| lines.iter().position(|&l| l == w).map_or(u32::MAX, |i| i as u32);
    let labels: Vec<[u32; 4]> = plan.faces.iter().map(|f| face_label(f, &line_of)).collect();
    let pt = |v: u32| {
        let p = plan.vertices[v as usize];
        [f64::from(p[0]), f64::from(p[1])]
    };
    let rings: Vec<Vec<Vec<[f64; 2]>>> =
        plan.faces.iter().map(|f| f.rings.iter().map(|r| r.iter().map(|&v| pt(v)).collect()).collect()).collect();
    let boxes: Vec<[f64; 4]> = rings
        .iter()
        .map(|r| {
            r[0].iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            })
        })
        .collect();
    // The edges between two labels, by bin.
    let mut by_bin: BTreeMap<(i32, i32), Vec<Edge>> = BTreeMap::new();
    let mpu = metres_per_unit(key.origin().y + CELL_UNITS / 2);
    let near = TEXEL_M / mpu;
    let mut world = Vec::new();
    let o = key.origin();
    for e in &plan.edges {
        let differs = match (labels.get(e.left as usize), labels.get(e.right as usize)) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        };
        if !differs {
            continue;
        }
        let (a, b) = (pt(e.a), pt(e.b));
        world.push([
            ((o.x + a[0] as i64) >> 4) as i32,
            ((o.y + a[1] as i64) >> 4) as i32,
            ((o.x + b[0] as i64) >> 4) as i32,
            ((o.y + b[1] as i64) >> 4) as i32,
        ]);
        let bin = |v: f64| (v / f64::from(BIN_UNITS)).floor() as i32;
        for r in bin(a[1].min(b[1]) - near)..=bin(a[1].max(b[1]) + near) {
            for c in bin(a[0].min(b[0]) - near)..=bin(a[0].max(b[0]) + near) {
                by_bin.entry((c, r)).or_default().push((a, b));
            }
        }
    }
    let label_words = |id: u32| {
        let at = words[5] as usize + LABEL * id as usize;
        [words[at], words[at + 1], words[at + 2], words[at + 3]]
    };
    let terrain = [faces::Surface::Terrain as u32 | u32::from(u8::MAX) << 16, 0, u32::MAX, u32::MAX];
    let heads = words[9] as usize;
    let (mut count, mut places) = ([0u64; 3], Vec::new());
    for r in 0..BINS {
        for c in 0..BINS {
            if words[heads + records::HEADER * (r * BINS + c) as usize + 1] >> 16 == 0 {
                continue;
            }
            let edges = by_bin.get(&(c, r));
            for j in 0..SAMPLES {
                for i in 0..SAMPLES {
                    let q = [
                        f64::from(c * BIN_UNITS) + (f64::from(i) + 0.37) * f64::from(BIN_UNITS) / f64::from(SAMPLES),
                        f64::from(r * BIN_UNITS) + (f64::from(j) + 0.61) * f64::from(BIN_UNITS) / f64::from(SAMPLES),
                    ];
                    count[0] += 1;
                    if edges.is_some_and(|es| es.iter().any(|&(a, b)| seg_dist(a, b, q) < near)) {
                        count[1] += 1;
                        continue;
                    }
                    let face = (0..plan.faces.len())
                        .find(|&k| {
                            let b = boxes[k];
                            q[0] >= b[0] && q[0] <= b[2] && q[1] >= b[1] && q[1] <= b[3] && winding(&rings[k], q) != 0
                        })
                        .map_or(terrain, |k| labels[k]);
                    let found = find(words, q).label.map_or(terrain, label_words);
                    // Terrain faces carry rank 255 in the plan; the records give no label there.
                    let same = found == face || (found[0] & 0xff == 0 && face[0] & 0xff == 0);
                    if !same {
                        count[2] += 1;
                        if places.len() < 6 {
                            let g = obc_ground_plan::frame::GlobalPoint { x: o.x + q[0] as i64, y: o.y + q[1] as i64 };
                            let (lon, lat) = crate::lon_lat(g);
                            places.push(json!({"lat": lat, "lon": lon, "face": face, "records": found}));
                        }
                    }
                }
            }
        }
    }
    (count, places, world)
}

fn build(key: CellKey, input: &obc_ground_plan::CellInput, rules: &Rules, runs: usize) -> Option<Cell> {
    let mut best = [f64::INFINITY; 4];
    let mut out = None;
    for _ in 0..runs {
        let t = Instant::now();
        let graph = graph::build(key, input, &rules.sections);
        let g = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let rec = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            records::records(&graph, input, rules, key, 0, true)
        }))
        .ok()?;
        let r = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let plan =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| faces::build(&graph, input, rules, key, 0, true)))
                .ok()?;
        let f = t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let far = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            records::records_far(&graph, input, rules, key, 0)
        }))
        .ok()?;
        let fm = t.elapsed().as_secs_f64() * 1e3;
        best = [best[0].min(g), best[1].min(r), best[2].min(f), best[3].min(fm)];
        out = Some((graph, rec, plan, far));
    }
    let (graph, rec, plan, far) = out?;
    let lines = faces::level_way_ids(&graph, input, rules, key, 0);
    let (agree, places, edges) = agreement(key, &rec.words, &plan, &lines);
    let compact = [records::compact_bytes(&rec.words), records::compact_bytes(&far.words)];
    let far_agree = far_agreement(&rec.words, &far.words);
    Some(Cell {
        key,
        words: rec.words,
        stats: rec.stats,
        graph_ms: best[0],
        records_ms: best[1],
        faces_ms: best[2],
        far_ms: best[3],
        compact,
        far_agree,
        far: far.stats,
        junctions: plan.stats.junctions,
        agree: [agree[0], agree[1], agree[2]],
        places,
        edges,
    })
}

pub fn run() -> Result<(), String> {
    let (mut out, mut runs) = (None::<PathBuf>, 1usize);
    let (mut at, mut size_m, mut px) = (Vec::new(), 64.0f64, 1200u32);
    let dir = fixture::default_dir();
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?.into()),
            "--runs" => runs = value()?.parse().map_err(|_| "--runs: a count")?,
            "--check-probe" => {
                let probe = value()?;
                let file = value()?;
                return check_probe(&probe, &file);
            }
            "--size-m" => size_m = value()?.parse().map_err(|_| "--size-m: a number")?,
            "--px" => px = value()?.parse().map_err(|_| "--px: a number")?,
            "--at" => {
                let text = value()?;
                let (name, ll) = text.split_once('=').ok_or("--at NAME=LAT,LON")?;
                let (lat, lon) = ll.split_once(',').ok_or("--at NAME=LAT,LON")?;
                at.push((
                    name.to_string(),
                    lat.parse::<f64>().map_err(|e| e.to_string())?,
                    lon.parse::<f64>().map_err(|e| e.to_string())?,
                ));
            }
            _ => return Err(format!("model: unknown option {arg}")),
        }
    }
    let out = out.ok_or("model --out DIR [--runs N] [--at NAME=LAT,LON --size-m M --px N]")?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut fix = Fixture::open(&dir);
    if !at.is_empty() {
        for (name, lat, lon) in &at {
            picture(&mut fix, &out, name, *lat, *lon, size_m, px)?;
        }
        return Ok(());
    }
    // The stats cover the fixture cells with all their tiles; the file also holds the cells at the
    // fixture's edge, which the viewer shows.
    let full: std::collections::BTreeSet<CellKey> = crate::graph::fixture_cells(&fix, &dir).into_iter().collect();
    let [west, south, east, north] = fix.bounds;
    let (a, b) = (
        CellKey::of(obc_ground_plan::frame::from_lon_lat(west, north)),
        CellKey::of(obc_ground_plan::frame::from_lon_lat(east, south)),
    );
    let cells: Vec<CellKey> = (a.y..=b.y).flat_map(|y| (a.x..=b.x).map(move |x| CellKey { x, y })).collect();
    let rules = Rules::embedded();
    let built = fix.map(&cells, |key, input| {
        let c = build(key, &input, rules, runs);
        match &c {
            Some(c) => eprintln!(
                "cell {} {} graph {:.1} records {:.1} faces {:.1} ms, {} words, disagree {}/{}",
                key.x,
                key.y,
                c.graph_ms,
                c.records_ms,
                c.faces_ms,
                c.words.len(),
                c.agree[2],
                c.agree[0] - c.agree[1]
            ),
            None => eprintln!("cell {} {} crash", key.x, key.y),
        }
        c
    });
    let built: Vec<Cell> = built.into_iter().flatten().collect();
    let x0 = built.iter().map(|c| c.key.x).min().unwrap_or(0);
    let y0 = built.iter().map(|c| c.key.y).min().unwrap_or(0);
    let w = built.iter().map(|c| c.key.x).max().unwrap_or(0) - x0 + 1;
    let h = built.iter().map(|c| c.key.y).max().unwrap_or(0) - y0 + 1;
    let mut file = vec![MAGIC, x0, y0, w, h];
    let table = file.len();
    file.extend(std::iter::repeat_n(0u32, (w * h) as usize));
    let mut edges: Vec<i32> = Vec::new();
    for c in &built {
        if c.stats.bins_used == 0 {
            continue;
        }
        file[table + ((c.key.y - y0) * w + c.key.x - x0) as usize] = file.len() as u32;
        file.extend(&c.words);
        edges.extend(c.edges.iter().flatten());
    }
    let bytes: Vec<u8> = file.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(out.join("model.bin"), &bytes).map_err(|e| e.to_string())?;
    let edge_bytes: Vec<u8> = edges.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(out.join("model-faces.bin"), &edge_bytes).map_err(|e| e.to_string())?;
    let mut by: BTreeMap<&str, BTreeMap<&str, Vec<f64>>> = BTreeMap::new();
    let (mut sampled, mut near, mut wrong) = (0u64, 0u64, 0u64);
    let mut places = Vec::new();
    for c in built.iter().filter(|c| full.contains(&c.key)) {
        let profile = if c.junctions >= 20 { "town" } else { "rural" };
        let e = by.entry(profile).or_default();
        e.entry("graph_ms").or_default().push(c.graph_ms);
        e.entry("records_ms").or_default().push(c.records_ms);
        e.entry("faces_ms").or_default().push(c.faces_ms);
        e.entry("kB").or_default().push(c.words.len() as f64 * 4.0 / 1024.0);
        e.entry("far_ms").or_default().push(c.far_ms);
        e.entry("far_kB").or_default().push(f64::from(c.far.words) * 4.0 / 1024.0);
        e.entry("compact_kB").or_default().push(c.compact[0] as f64 / 1024.0);
        e.entry("far_compact_kB").or_default().push(c.compact[1] as f64 / 1024.0);
        e.entry("cands").or_default().push(f64::from(c.stats.cands));
        e.entry("bins_used").or_default().push(f64::from(c.stats.bins_used));
        e.entry("bin_edges").or_default().push(f64::from(c.stats.bin_edges));
        e.entry("max_bin_words").or_default().push(f64::from(c.stats.max_bin_words));
        e.entry("ring_vertices").or_default().push(f64::from(c.stats.rings_vertices));
        sampled += c.agree[0];
        near += c.agree[1];
        wrong += c.agree[2];
        for p in &c.places {
            if places.len() < 40 {
                places.push(json!({"cell": [c.key.x, c.key.y], "at": p}));
            }
        }
    }
    // Where the bytes go, per profile and level: words of each part over all cells.
    let mut parts = serde_json::Map::new();
    for profile in ["town", "rural"] {
        let of = |c: &&Cell| (c.junctions >= 20) == (profile == "town");
        let cells: Vec<&Cell> = built.iter().filter(|c| full.contains(&c.key)).filter(of).collect();
        for (level, stats) in [("near", false), ("far", true)] {
            let mut m = serde_json::Map::new();
            let total: u64 = cells.iter().map(|c| u64::from(if stats { c.far.words } else { c.stats.words })).sum();
            for (i, name) in records::SECTIONS.iter().enumerate() {
                let w: u64 =
                    cells.iter().map(|c| u64::from(if stats { c.far.sections[i] } else { c.stats.sections[i] })).sum();
                m.insert(name.to_string(), json!(format!("{:.1} %", 100.0 * w as f64 / total.max(1) as f64)));
            }
            let listed: u64 = cells
                .iter()
                .map(|c| {
                    (if stats { c.far.sections } else { c.stats.sections }).iter().map(|&w| u64::from(w)).sum::<u64>()
                })
                .sum();
            m.insert(
                records::HEIGHT_LISTS.to_string(),
                json!(format!("{:.1} %", 100.0 * (total - listed) as f64 / total.max(1) as f64)),
            );
            parts.insert(format!("{profile} {level}"), Value::Object(m));
        }
    }
    // Bytes per square kilometre of each level and layout over the fixture's full cells, and their sum
    // over a disc about the fixture's middle: near records nearer than R_NEAR, far ones beyond.
    let area_km2 = |k: CellKey| {
        let m = CELL_UNITS as f64 * metres_per_unit(k.origin().y + CELL_UNITS / 2);
        m * m / 1e6
    };
    let fulls: Vec<&Cell> = built.iter().filter(|c| full.contains(&c.key)).collect();
    let km2: f64 = fulls.iter().map(|c| area_km2(c.key)).sum();
    let per_km2 = |bytes: &dyn Fn(&Cell) -> f64| fulls.iter().map(|c| bytes(c)).sum::<f64>() / km2 / 1e6;
    let dens = [
        per_km2(&|c| f64::from(c.stats.words) * 4.0),
        per_km2(&|c| c.compact[0] as f64),
        per_km2(&|c| f64::from(c.far.words) * 4.0),
        per_km2(&|c| c.compact[1] as f64),
    ];
    let ms_km2 = [per_km2(&|c| (c.graph_ms + c.records_ms) * 1e6), per_km2(&|c| (c.graph_ms + c.far_ms) * 1e6)];
    let mut discs = serde_json::Map::new();
    for r_km in [2.0f64, 5.0] {
        for near_km in [0.6f64, 1.0, 1.5] {
            let (a_near, a_all) = (std::f64::consts::PI * near_km * near_km, std::f64::consts::PI * r_km * r_km);
            discs.insert(
                format!("to {r_km} km, near records to {near_km} km"),
                json!({
                    "MB layout as now": dens[0] * a_near + dens[2] * (a_all - a_near),
                    "MB compact": dens[1] * a_near + dens[3] * (a_all - a_near),
                    "MB near records everywhere, as now": dens[0] * a_all,
                    "build s, one thread": (ms_km2[0] * a_near + ms_km2[1] * (a_all - a_near)) / 1e3,
                }),
            );
        }
    }
    let fa = fulls.iter().fold([0u64; 2], |a, c| [a[0] + c.far_agree[0], a[1] + c.far_agree[1]]);
    let report = json!({
        "far_labels": {"paved points": fa[0], "far label differs": fa[1]},
        "parts": parts,
        "per_km2": {"km2": km2, "MB near": dens[0], "MB near compact": dens[1], "MB far": dens[2], "MB far compact": dens[3], "build ms near": ms_km2[0], "build ms far": ms_km2[1]},
        "discs": discs,
        "cells": full.len(),
        "file_cells": cells.len(),
        "built": built.len(),
        "file_bytes": bytes.len(),
        "profiles": by.into_iter().map(|(k, v)| (k.to_string(), Value::Object(v.into_iter().map(|(n, x)| (n.to_string(), quantiles(x))).collect()))).collect::<serde_json::Map<_, _>>(),
        "agreement": {"points": sampled, "near_boundary": near, "disagree": wrong, "places": places},
    });
    std::fs::write(out.join("model-bench.json"), serde_json::to_string_pretty(&report).unwrap())
        .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&report["agreement"]).unwrap());
    println!("{}", serde_json::to_string_pretty(&report["profiles"]).unwrap());
    for k in ["far_labels", "parts", "per_km2", "discs"] {
        println!("{}", serde_json::to_string_pretty(&report[k]).unwrap());
    }
    Ok(())
}

/// The colour of a label, as the faces layer draws faces, the carriageway by its finish.
fn colour(words: &[u32], id: Option<u32>) -> [u8; 3] {
    let Some(id) = id else { return [236, 236, 228] };
    let at = words[5] as usize + LABEL * id as usize;
    let (w0, line) = (words[at], words[at + 2]);
    let (surface, kind, rank) = (w0 & 0xff, (w0 >> 8) & 0xff, (w0 >> 16) & 0xff);
    let finish = if line == u32::MAX { 0 } else { words[words[6] as usize + records::LINE * line as usize + 1] & 0xff };
    match surface {
        0 => [236, 236, 228],
        1 => [205, 196, 140],
        2 => {
            let base = match finish {
                1 | 7 => [95, 95, 102],
                2 => [125, 125, 130],
                3 => [170, 150, 115],
                4 | 5 => [150, 120, 80],
                6 => [190, 150, 110],
                8 => [160, 150, 150],
                _ => [120, 100, 90],
            };
            if kind == records::JUNCTION {
                [base[0] / 2 + 70, base[1] / 2 + 30, base[2] / 2 + 95]
            } else {
                base
            }
        }
        3 if rank == 2 => [165, 165, 158],
        3 => [196, 186, 160],
        4 => [140, 185, 110],
        6 => [210, 60, 60],
        7 => [125, 95, 75],
        _ => [230, 160, 50],
    }
}

/// A top-down picture of a place: per pixel the records' label at the pixel's middle (the far
/// sampling's CPU reference), with the overlay's face edges between two labels drawn over it in black.
fn picture(
    fix: &mut Fixture,
    out: &std::path::Path,
    name: &str,
    lat: f64,
    lon: f64,
    size_m: f64,
    px: u32,
) -> Result<(), String> {
    let rules = Rules::embedded();
    let centre = obc_ground_plan::frame::from_lon_lat(lon, lat);
    let upp = size_m / f64::from(px) / metres_per_unit(centre.y);
    let corner = [centre.x as f64 - upp * f64::from(px) / 2.0, centre.y as f64 - upp * f64::from(px) / 2.0];
    let cells = crate::square_cells(lat, lon, size_m);
    let mut blocks = BTreeMap::new();
    let mut lines_out: Vec<[i32; 4]> = Vec::new();
    for key in cells {
        let Ok(input) = fix.try_cell(key) else { continue };
        let graph = graph::build(key, &input, &rules.sections);
        let rec = records::records(&graph, &input, rules, key, 0, true);
        let plan = faces::build(&graph, &input, rules, key, 0, true);
        let ids = faces::level_way_ids(&graph, &input, rules, key, 0);
        let (_, _, edges) = agreement(key, &rec.words, &plan, &ids);
        lines_out.extend(edges);
        blocks.insert(key, rec.words);
    }
    let mut img = vec![0u8; (px * px * 3) as usize];
    for j in 0..px {
        for i in 0..px {
            let g = [corner[0] + (f64::from(i) + 0.5) * upp, corner[1] + (f64::from(j) + 0.5) * upp];
            let key =
                CellKey::of(obc_ground_plan::frame::GlobalPoint { x: g[0].floor() as i64, y: g[1].floor() as i64 });
            let c = match blocks.get(&key) {
                Some(words) => {
                    let o = key.origin();
                    colour(words, find(words, [g[0] - o.x as f64, g[1] - o.y as f64]).label)
                }
                None => [236, 236, 228],
            };
            let k = ((j * px + i) * 3) as usize;
            img[k..k + 3].copy_from_slice(&c);
        }
    }
    // The face edges, in world units (16 frame units).
    for e in &lines_out {
        let p = |x: i32, y: i32| [(f64::from(x) * 16.0 - corner[0]) / upp, (f64::from(y) * 16.0 - corner[1]) / upp];
        let (a, b) = (p(e[0], e[1]), p(e[2], e[3]));
        let n = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as i32).max(1);
        for t in 0..=n {
            let f = f64::from(t) / f64::from(n);
            let (x, y) = ((a[0] + f * (b[0] - a[0])).floor() as i64, (a[1] + f * (b[1] - a[1])).floor() as i64);
            if x >= 0 && y >= 0 && x < i64::from(px) && y < i64::from(px) {
                let k = ((y as u32 * px + x as u32) * 3) as usize;
                img[k..k + 3].copy_from_slice(&[20, 20, 20]);
            }
        }
    }
    let path = out.join(format!("{name}.png"));
    image::save_buffer(&path, &img, px, px, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
    println!("{}", path.display());
    Ok(())
}

/// The labels of a GPU probe (`model-probe.mjs`) against `find` on the same words: per point the cell,
/// q in frame units and the GPU's label.
fn check_probe(probe: &str, model: &str) -> Result<(), String> {
    let bytes = std::fs::read(model).map_err(|e| format!("{model}: {e}"))?;
    let words: Vec<u32> = bytes.as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    let rows: Value =
        serde_json::from_slice(&std::fs::read(probe).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let (x0, y0, w, h) = (words[1], words[2], words[3], words[4]);
    let (mut same, mut differ, mut examples) = (0, 0, Vec::new());
    for r in rows.as_array().into_iter().flatten() {
        let (cx, cy) = (r["cell"][0].as_u64().unwrap() as u32, r["cell"][1].as_u64().unwrap() as u32);
        let q = [r["q"][0].as_f64().unwrap(), r["q"][1].as_f64().unwrap()];
        if cx < x0 || cy < y0 || cx - x0 >= w || cy - y0 >= h {
            continue;
        }
        let at = words[5 + ((cy - y0) * w + cx - x0) as usize] as usize;
        if at == 0 {
            continue;
        }
        let label = find(&words[at..], q).label;
        let gpu = r["label"].as_u64().map(|l| l as u32);
        if label == gpu {
            same += 1;
        } else {
            differ += 1;
            if examples.len() < 10 {
                examples.push(json!({"cell": [cx, cy], "q": q, "cpu": label, "gpu": gpu}));
            }
        }
    }
    println!("{}", json!({"same": same, "differ": differ, "examples": examples}));
    Ok(())
}
