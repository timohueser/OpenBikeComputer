//! `model --out DIR [--runs N] [--samples N]`: the records of every fixture cell. Writes
//! `model.bin` (a table of the cells, then their near blocks) and `model-bench.json`: build times
//! and sizes by profile, and the checks of `faces::records::check`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use obc_ground_plan::faces::check::feature_names;
use obc_ground_plan::faces::records::check::{agreement, border, far_agreement, Miss, Overlay};
use obc_ground_plan::faces::records::{self, BINS, HEADER};
use obc_ground_plan::faces::{self, Rules};
use obc_ground_plan::frame::{CellKey, GlobalPoint};
use obc_ground_plan::graph::{self, WayGraph};
use serde_json::{json, Value};

use crate::faces::{quantiles, TOWN_JUNCTIONS};
use crate::fixture::{default_dir, Fixture};

/// The file's magic: a table of cells, then their blocks.
const FILE_MAGIC: u32 = 0x4f42_5246;
/// A miss of a label this near an edge between two labels is the slice's precision, in units.
const EDGE_UNITS: f64 = 8.0;

struct Cell {
    key: CellKey,
    words: Vec<u32>,
    far: Vec<u32>,
    graph: WayGraph,
    names: Vec<[u32; 3]>,
    town: bool,
    /// Graph, the cell's build (faces, near and far records) and the faces alone, in ms: the least of
    /// the runs.
    ms: [f64; 3],
    agree: [u64; 5],
    area: Vec<Miss>,
    far_agree: [u64; 2],
}

fn time<T>(f: impl FnOnce() -> T) -> (T, f64) {
    let t = Instant::now();
    let v = f();
    (v, t.elapsed().as_secs_f64() * 1e3)
}

/// Words per part of a block, from its head's offsets.
fn parts(w: &[u32]) -> [u64; 10] {
    let bins = w[9] as usize + HEADER * (BINS * BINS) as usize;
    let at = [0, w[4], w[5], w[6], w[7], w[8], w[12], w[11], w[14], w[9], bins as u32, w.len() as u32];
    std::array::from_fn(|k| u64::from(at[k + 1] - at[k]))
}

const PARTS: [&str; 10] = [
    "head",
    "candidates",
    "labels",
    "lines",
    "samples",
    "extras",
    "junctions",
    "blend edges",
    "outline edges",
    "bin headers",
];

pub fn run() -> Result<(), String> {
    let (mut out, mut runs, mut n) = (None::<PathBuf>, 1usize, 10i32);
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--out" => out = Some(value()?.into()),
            "--runs" => runs = value()?.parse().map_err(|_| "--runs: a count")?,
            "--samples" => n = value()?.parse().map_err(|_| "--samples: a count")?,
            _ => return Err(format!("model: unknown option {arg}")),
        }
    }
    let out = out.ok_or("model --out DIR [--runs N] [--samples N]")?;
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let dir = default_dir();
    let mut fix = Fixture::open(&dir);
    let cells = crate::graph::fixture_cells(&fix, &dir);
    let rules = Rules::embedded();
    let built: Vec<Cell> = fix.map(&cells, |key, input| {
        let mut ms = [f64::INFINITY; 3];
        let mut last = None;
        for _ in 0..runs {
            let (graph, g) = time(|| graph::build(key, &input, &rules.sections));
            let (built, b) = time(|| records::build(&graph, &input, rules, key, 0, true));
            let (_, p) = time(|| faces::build(&graph, &input, rules, key, 0, true));
            ms = [ms[0].min(g), ms[1].min(b), ms[2].min(p)];
            last = Some((graph, built));
        }
        let (graph, records::Built { faces: plan, near, far, .. }) = last.unwrap();
        let a = agreement(&near.words, &plan, &Overlay::new(&graph, &input, rules, key, true), n, EDGE_UNITS);
        let far_agree = far_agreement(&near.words, &far.words, n);
        eprintln!(
            "cell {} {}: build {:.1} ms, {} words, area misses {}",
            key.x,
            key.y,
            ms[1],
            near.words.len(),
            a.area.len()
        );
        Cell {
            key,
            town: plan.stats.junctions >= TOWN_JUNCTIONS,
            words: near.words,
            far: far.words,
            graph,
            names: feature_names(&input),
            ms,
            agree: [a.points, a.edge, a.arc, a.patch, a.rail],
            area: a.area,
            far_agree,
        }
    });
    let by_key: BTreeMap<CellKey, &Cell> = built.iter().map(|c| (c.key, c)).collect();
    let (mut border_points, mut border_blend, mut border_differ, mut border_places) = (0u64, 0u64, 0usize, Vec::new());
    for (&key, a) in &by_key {
        for (east, other) in [(true, CellKey { x: key.x + 1, y: key.y }), (false, CellKey { x: key.x, y: key.y + 1 })] {
            let Some(b) = by_key.get(&other) else { continue };
            let (points, blend, differ) =
                border((&a.words, &a.graph, &a.names), (&b.words, &b.graph, &b.names), east, 64);
            border_points += points;
            border_blend += blend;
            border_differ += differ.len();
            border_places.extend(differ.into_iter().take(4).map(|d| json!({"cells": [[key.x, key.y], [other.x, other.y]], "along": d.0, "west_or_north": d.1, "east_or_south": d.2})));
        }
    }
    let x0 = built.iter().map(|c| c.key.x).min().unwrap_or(0);
    let y0 = built.iter().map(|c| c.key.y).min().unwrap_or(0);
    let w = built.iter().map(|c| c.key.x).max().unwrap_or(0) - x0 + 1;
    let h = built.iter().map(|c| c.key.y).max().unwrap_or(0) - y0 + 1;
    let mut file = vec![FILE_MAGIC, x0, y0, w, h];
    file.resize(5 + (w * h) as usize, 0);
    for c in &built {
        file[5 + ((c.key.y - y0) * w + c.key.x - x0) as usize] = file.len() as u32;
        file.extend(&c.words);
    }
    let bytes: Vec<u8> = file.iter().flat_map(|w| w.to_le_bytes()).collect();
    std::fs::write(out.join("model.bin"), &bytes).map_err(|e| e.to_string())?;
    let mut profiles: BTreeMap<&str, BTreeMap<&str, Vec<f64>>> = BTreeMap::new();
    let mut words: BTreeMap<String, u64> = BTreeMap::new();
    for c in &built {
        let p = profiles.entry(if c.town { "town" } else { "rural" }).or_default();
        for (k, v) in ["graph_ms", "build_ms", "faces_ms"].iter().zip(c.ms) {
            p.entry(k).or_default().push(v);
        }
        p.entry("kB").or_default().push(c.words.len() as f64 / 256.0);
        p.entry("far_kB").or_default().push(c.far.len() as f64 / 256.0);
        let total = c.words.len() as u64;
        let listed = parts(&c.words);
        for (name, v) in PARTS.iter().zip(listed) {
            *words.entry(name.to_string()).or_default() += v;
        }
        *words.entry("bin items".into()).or_default() += total - listed.iter().sum::<u64>();
    }
    let all: u64 = words.values().sum();
    let shares: BTreeMap<String, String> =
        words.iter().map(|(k, &v)| (k.clone(), format!("{:.1} %", 100.0 * v as f64 / all.max(1) as f64))).collect();
    let sum = |f: &dyn Fn(&Cell) -> u64| built.iter().map(f).sum::<u64>();
    let area: Vec<Value> = (built.iter())
        .flat_map(|c| c.area.iter().take(8).map(move |m| (c.key, m)))
        .take(60)
        .map(|(k, m)| {
            let o = k.origin();
            let (lon, lat) = crate::lon_lat(GlobalPoint { x: o.x + m.q[0] as i64, y: o.y + m.q[1] as i64 });
            json!({"cell": [k.x, k.y], "q": m.q, "lat": lat, "lon": lon, "face": m.face, "records": m.records, "edge_units": m.edge})
        })
        .collect();
    let report = json!({
        "cells": built.len(),
        "file_bytes": bytes.len(),
        "far_bytes": 4 * built.iter().map(|c| c.far.len()).sum::<usize>(),
        "profiles": profiles.into_iter().map(|(k, v)| (k, v.into_iter().map(|(n, x)| (n, quantiles(x))).collect::<BTreeMap<_, _>>())).collect::<BTreeMap<_, _>>(),
        "words": shares,
        "agreement": {
            "samples per bin side": n,
            "points": sum(&|c| c.agree[0]),
            "near an edge": sum(&|c| c.agree[1]),
            "edge_units": EDGE_UNITS,
            "on an arc": sum(&|c| c.agree[2]),
            "patch rules": sum(&|c| c.agree[3]),
            "patch rules, rail bed": sum(&|c| c.agree[4]),
            "patch rules by cell": built.iter().filter(|c| c.agree[3] > 0).map(|c| json!([[c.key.x, c.key.y], c.agree[3]])).collect::<Vec<_>>(),
            "other": sum(&|c| c.area.len() as u64),
            "other by cell": built.iter().filter(|c| !c.area.is_empty()).map(|c| json!([[c.key.x, c.key.y], c.area.len()])).collect::<Vec<_>>(),
            "places": area,
        },
        "far": {"points": sum(&|c| c.far_agree[0]), "differ": sum(&|c| c.far_agree[1])},
        "border": {"points": border_points, "with a blend edge near": border_blend, "differ": border_differ, "places": border_places},
    });
    let text = serde_json::to_string_pretty(&report).unwrap();
    std::fs::write(out.join("model-bench.json"), &text).map_err(|e| e.to_string())?;
    println!("{text}");
    Ok(())
}
