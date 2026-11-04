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
    ((v[0] - t * u[0]).powi(2) + (v[1] - t * u[1]).powi(2)).sqrt()
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
    let mut by_bin: BTreeMap<(i32, i32), Vec<([f64; 2], [f64; 2])>> = BTreeMap::new();
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
            if words[heads + 2 * (r * BINS + c) as usize + 1] >> 16 == 0 {
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
    let mut best = [f64::INFINITY; 3];
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
        best = [best[0].min(g), best[1].min(r), best[2].min(f)];
        out = Some((graph, rec, plan));
    }
    let (graph, rec, plan) = out?;
    let lines = faces::level_way_ids(&graph, input, rules, key, 0);
    let (agree, places, edges) = agreement(key, &rec.words, &plan, &lines);
    Some(Cell {
        key,
        words: rec.words,
        stats: rec.stats,
        graph_ms: best[0],
        records_ms: best[1],
        faces_ms: best[2],
        junctions: plan.stats.junctions,
        agree: [agree[0], agree[1], agree[2]],
        places,
        edges,
    })
}

pub fn run() -> Result<(), String> {
    let (mut out, mut runs) = (None::<PathBuf>, 1usize);
    let dir = fixture::default_dir();
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?.into()),
            "--runs" => runs = value()?.parse().map_err(|_| "--runs: a count")?,
            _ => return Err(format!("model: unknown option {arg}")),
        }
    }
    let out = out.ok_or("model --out DIR [--runs N]")?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut fix = Fixture::open(&dir);
    let cells = crate::graph::fixture_cells(&fix, &dir);
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
    for c in &built {
        let profile = if c.junctions >= 20 { "town" } else { "rural" };
        let e = by.entry(profile).or_default();
        e.entry("graph_ms").or_default().push(c.graph_ms);
        e.entry("records_ms").or_default().push(c.records_ms);
        e.entry("faces_ms").or_default().push(c.faces_ms);
        e.entry("kB").or_default().push(c.words.len() as f64 * 4.0 / 1024.0);
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
    let report = json!({
        "cells": cells.len(),
        "built": built.len(),
        "file_bytes": bytes.len(),
        "profiles": by.into_iter().map(|(k, v)| (k.to_string(), Value::Object(v.into_iter().map(|(n, x)| (n.to_string(), quantiles(x))).collect()))).collect::<serde_json::Map<_, _>>(),
        "agreement": {"points": sampled, "near_boundary": near, "disagree": wrong, "places": places},
    });
    std::fs::write(out.join("model-bench.json"), serde_json::to_string_pretty(&report).unwrap())
        .map_err(|e| e.to_string())?;
    println!("{}", serde_json::to_string_pretty(&report["agreement"]).unwrap());
    println!("{}", serde_json::to_string_pretty(&report["profiles"]).unwrap());
    Ok(())
}
