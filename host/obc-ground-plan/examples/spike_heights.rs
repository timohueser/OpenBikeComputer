//! Spike 2 (never merges): `height_at` over every corner of the mesh cells of a fixture, on one thread,
//! one JSON line per cell. Also writes each cell's request bytes and the rims it reads, for the WASM
//! bench (`builder/ground-plan-wasm`'s `HeightBench`).
//!
//! `spike_heights --fixture DIR --cells FILE|x/y,... [--mesh-only] --out FILE.jsonl [--dump DIR] [--runs 3]`

#[path = "../tests/fixture/mod.rs"]
mod fixture;

use std::collections::BTreeMap;
use std::io::Write;
use std::time::Instant;

use obc_ground_plan::faces::{records, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::mesh::{mesh_cell, Reads, Rim};
use serde_json::{json, Value};

fn key_of(s: &str) -> CellKey {
    let v: Vec<u32> = s.split(['/', '-']).filter_map(|p| p.parse().ok()).collect();
    let n = v.len();
    CellKey { x: v[n - 2], y: v[n - 1] }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut dir, mut cells, mut mesh_only, mut out, mut dump, mut runs) =
        (fixture::default_dir(), String::new(), false, String::new(), None::<String>, 3usize);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--fixture" => dir = args.next().unwrap().into(),
            "--cells" => cells = args.next().unwrap(),
            "--mesh-only" => mesh_only = true,
            "--out" => out = args.next().unwrap(),
            "--dump" => dump = args.next(),
            "--runs" => runs = args.next().unwrap().parse().unwrap(),
            _ => panic!("unknown option {a}"),
        }
    }
    let wanted: Vec<CellKey> = if cells.ends_with(".json") {
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cells).unwrap()).unwrap();
        (v.as_array().unwrap().iter())
            .filter(|c| !mesh_only || c["mesh"].as_bool() == Some(true))
            .map(|c| key_of(c["cell"].as_str().unwrap()))
            .collect()
    } else {
        cells.split(',').map(key_of).collect()
    };
    let mut fix = fixture::Fixture::open(&dir);
    let rules = Rules::embedded();
    let clock = Instant::now();
    let mut now = || clock.elapsed().as_secs_f64() * 1e3;
    let has_tiles = |k: CellKey| k.feature_tiles().iter().all(|&(c, r)| fixture::feature_tile(&dir, c, r).is_some());
    let mut rims: BTreeMap<CellKey, Option<Rim>> = BTreeMap::new();
    let mut file = std::fs::File::create(&out).unwrap();
    if let Some(d) = &dump {
        std::fs::create_dir_all(d).unwrap();
    }
    for &key in &wanted {
        if !has_tiles(key) {
            println!("{}/{}: feature tiles missing, skipped", key.x, key.y);
            continue;
        }
        let t0 = now();
        let input = fix.cell(key);
        let graph = obc_ground_plan::graph::build(key, &input, &rules.sections);
        let (plan, near) = records::near(&graph, &input, rules, key, 0, true);
        let build_ms = now() - t0;
        drop(graph);
        rims.insert(key, Some(Rim::of(&plan)));
        let mut missing = 0;
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                let k = CellKey { x: (key.x as i64 + dx) as u32, y: (key.y as i64 + dy) as u32 };
                if rims.contains_key(&k) {
                    continue;
                }
                let rim = has_tiles(k).then(|| {
                    let input = fix.cell(k);
                    let graph = obc_ground_plan::graph::build(k, &input, &rules.sections);
                    Rim::of(&records::near(&graph, &input, rules, k, 0, true).0)
                });
                rims.insert(k, rim);
            }
        }
        let around: Vec<Rim> = (-1..=1i64)
            .flat_map(|dy| (-1..=1i64).map(move |dx| (dx, dy)))
            .filter_map(|(dx, dy)| {
                let k = CellKey { x: (key.x as i64 + dx) as u32, y: (key.y as i64 + dy) as u32 };
                let r = rims[&k].clone();
                if r.is_none() {
                    missing += 1;
                }
                r
            })
            .collect();
        let words = near.words;
        let t1 = now();
        let reads = Reads::of(&plan, &around);
        let mesh = mesh_cell(&plan, &words, &reads, &mut || 0.0);
        let mesh_ms = now() - t1;
        let mesh = match mesh {
            Ok(m) => m,
            Err(e) => {
                println!("{}/{}: mesh error {e:?}", key.x, key.y);
                continue;
            }
        };
        let sharp = (0..mesh.corners.len()).filter(|&k| mesh.corner(k).1 != obc_ground_plan::mesh::NO_LABEL).count();
        if sharp == 0 {
            println!("{}/{}: no labelled corner ({} corners), skipped", key.x, key.y, mesh.corners.len());
            continue;
        }
        if let Some(d) = &dump {
            std::fs::write(format!("{d}/{}-{}.req", key.x, key.y), fix.request(key).encode()).unwrap();
            let w: Vec<u8> = around.iter().flat_map(|r| r.to_words()).flat_map(u32::to_le_bytes).collect();
            std::fs::write(format!("{d}/{}-{}.rims", key.x, key.y), w).unwrap();
        }
        let mut row = obc_ground_plan::spike::best_of(&mesh, &words, &input.terrain, &mut now, runs);
        row["cell"] = json!(format!("16/{}/{}", key.x, key.y));
        row["build_ms"] = json!(build_ms);
        row["mesh_ms"] = json!(mesh_ms);
        row["missing_rims"] = json!(missing);
        row["records_words"] = json!(words.len());
        let n = row["corners"].as_f64().unwrap();
        println!(
            "{}/{}: {} corners, {} tris, all {:.2} ms ({:.3} µs/corner), terrain {:.2} ms, mesh {:.1} ms, build {:.0} ms",
            key.x,
            key.y,
            row["corners"],
            row["triangles"],
            row["all_ms"].as_f64().unwrap(),
            row["all_ms"].as_f64().unwrap() * 1e3 / n,
            row["terrain_ms"].as_f64().unwrap(),
            mesh_ms,
            build_ms
        );
        writeln!(file, "{}", serde_json::to_string(&row).unwrap()).unwrap();
    }
}
