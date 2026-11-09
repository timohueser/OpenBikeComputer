//! Spike (far strokes): per cell of a list the native stage times of the full build (`cell::build_cell`) and
//! the time of the record lines and samples alone (`faces::far_lines`), median of RUNS runs.
//!
//!   cargo run --release --example far_cost -- CELLS.txt OUT.json   # CELLS.txt: one "x y" of zoom 16 per line

#[path = "../tests/fixture/mod.rs"]
mod fixture;

use std::time::Instant;

use obc_ground_plan::cell::{build_cell, STAGES};
use obc_ground_plan::faces::{far_lines, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::graph;
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::transport::Request;
use serde_json::json;

const RUNS: usize = 3;

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cells: Vec<CellKey> = std::fs::read_to_string(&args[1])
        .unwrap()
        .lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace().map(|v| v.parse::<u32>().unwrap());
            Some(CellKey { x: it.next()?, y: it.next()? })
        })
        .collect();
    let mut fix = fixture::Fixture::open(&fixture::default_dir());
    let rules = Rules::embedded();
    let start = Instant::now();
    let mut clock = || start.elapsed().as_secs_f64() * 1e3;
    let mut rows = Vec::new();
    for key in cells {
        let bytes = fix.request(key).encode();
        let mut stages = vec![Vec::new(); STAGES.len()];
        let mut words = 0;
        for _ in 0..RUNS {
            let b = build_cell(&bytes, rules, &mut clock).unwrap();
            for (i, t) in b.times.iter().enumerate() {
                stages[i].push(*t);
            }
            words = b.records.len();
        }
        let mut far = Vec::new();
        let mut far_words = 0;
        for _ in 0..RUNS {
            let input = Request::decode(&bytes).unwrap().input(&mut TileCache::new(4)).unwrap();
            let g = graph::build(key, &input, &rules.sections);
            let t = clock();
            far_words = far_lines(&g, &input, rules, key);
            far.push(clock() - t);
        }
        let st: Vec<f64> = stages.into_iter().map(median).collect();
        eprintln!("cell {} {}: {:?} lines+samples {:.1} ms", key.x, key.y, st, median(far.clone()));
        rows.push(json!({"cell": [key.x, key.y], "input": st[0], "graph": st[1], "faces": st[2], "lines_samples": median(far), "block_words": words, "line_sample_words": far_words}));
    }
    std::fs::write(&args[2], serde_json::to_string_pretty(&rows).unwrap()).unwrap();
}
