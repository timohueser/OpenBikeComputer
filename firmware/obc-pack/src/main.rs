//! `obc-pack` CLI — the Stage-3 end-to-end pipeline (`.osm.pbf` → `.obcm`),
//! mirroring `packer/pack.py`'s single-PBF path **minus multipolygon relations
//! and minus land/merge** (deferred to Stages 4–5). Same positional CLI as
//! `pack.py` (`<pbf...> <config.json> <out.obcm>`) plus `--chunk-size` and
//! `--no-land`, and it prints the stage strings the web builder's `_STAGE_MARKERS`
//! scrapes ("Pass 1/2", "Calculating BBox", "Building Quadtree", "Serializing",
//! "Writing"), so it can be dropped behind `OBC_PACK_BACKEND=rust` later.
//!
//! Validation is the feature-multiset + render gate (see `lib.rs` / the corpus
//! README), not byte-identity: simplify runs in GEOS 3.14 here vs shapely's 3.13,
//! and feature/ring order is not reproduced. The serializer + quadtree remain
//! byte-exact in isolation (Stages 1–2).

use std::process::ExitCode;

use obc_pack::config::Config;
use obc_pack::geom::{topology_preserve_simplify, Geom};
use obc_pack::ingest::ingest_osm;
use obc_pack::quadtree::build_lod;
use obc_pack::serialize::{serialize_lods, LodLayer};

/// Meters → degrees divisor for the simplify tolerance (mirrors `pack.py`).
const M_PER_DEG: f64 = 111_320.0;

struct Args {
    pbfs: Vec<String>,
    config: String,
    output: String,
    chunk_size: Option<usize>,
    no_land: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut chunk_size = None;
    let mut no_land = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--chunk-size" => {
                chunk_size = Some(
                    it.next()
                        .and_then(|s| s.parse().ok())
                        .ok_or("--chunk-size needs a number")?,
                );
            }
            "--no-land" => no_land = true,
            _ => positional.push(a),
        }
    }
    // pack.py contract: `<pbf...> <config.json> <out.obcm>` — last two positionals
    // are config + output, the rest are inputs.
    if positional.len() < 3 {
        return Err("usage: obc-pack <pbf...> <config.json> <out.obcm> [--chunk-size N] [--no-land]".into());
    }
    let output = positional.pop().unwrap();
    let config = positional.pop().unwrap();
    Ok(Args { pbfs: positional, config, output, chunk_size, no_land })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    if args.pbfs.len() > 1 {
        return Err("multi-PBF merge is not ported yet (Stage 5); pass a single .osm.pbf \
                    or use the Python backend"
            .into());
    }
    let pbf = &args.pbfs[0];

    let config = Config::load(&args.config)?;
    let chunk_size = args.chunk_size.unwrap_or(config.chunk_size);

    // --- Ingest (two passes: nodes, then ways). ---
    println!("Pass 1: reading nodes...");
    println!("Pass 2: processing ways...");
    let ingested = ingest_osm(pbf, &config)?;
    if ingested.features.is_empty() && ingested.coastlines.is_empty() {
        return Err("no features found matching config".into());
    }

    // --- Global bbox over features + coastlines, then TRUNCATE toward zero
    // (`int(v*1e6)`), NOT round — the deliberate asymmetry from plan §4.3. ---
    println!("Calculating BBox...");
    let global_bbox = compute_bbox(&ingested);

    // --- Land: deferred to Stage 5. Warn if the config asked for it. ---
    let has_land_cfg = config.features.iter().any(|(k, m)| k == "natural" && m.contains_key("land"));
    if has_land_cfg && !args.no_land {
        eprintln!(
            "note: land generation is not ported yet (Stage 5); omitting land polygons. \
             Pass --no-land to silence."
        );
    }

    // --- One quadtree per LOD (cumulative + per-level simplify), like pack.py. ---
    let mut lods = Vec::with_capacity(config.lods.len());
    for (i, lod) in config.lods.iter().enumerate() {
        println!("Building Quadtree LOD {i} (simplify {}m)...", lod.simplify_m);
        let tol = if lod.simplify_m > 0.0 { lod.simplify_m / M_PER_DEG } else { 0.0 };
        let level: Vec<(u8, Geom)> = ingested
            .features
            .iter()
            .filter(|f| f.min_lod <= i)
            .map(|f| {
                let g = if tol > 0.0 { topology_preserve_simplify(&f.geom, tol) } else { f.geom.clone() };
                (f.style_id, g)
            })
            .collect();
        let root = build_lod(level, global_bbox, chunk_size);
        lods.push(LodLayer { max_mpp: lod.max_mpp, chunk_size, root });
    }

    // --- Serialize the pyramid + write. ---
    println!("Serializing {} LOD level(s)...", lods.len());
    let styles = config.styles();
    let bytes = serialize_lods(&lods, &styles, config.marker_color, global_bbox);
    println!("Writing {} ({} bytes)...", args.output, bytes.len());
    std::fs::write(&args.output, &bytes).map_err(|e| format!("write {}: {e}", args.output))?;
    println!("Done!");
    Ok(())
}

/// `total_bounds(features + coastlines)` then `int(v*1e6)` truncation. The coords
/// are the exact osmium f64s (see `node_probe`), so the bbox matches the oracle's.
fn compute_bbox(ing: &obc_pack::ingest::Ingested) -> (i64, i64, i64, i64) {
    let (mut minx, mut miny, mut maxx, mut maxy) =
        (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    let mut widen = |x: f64, y: f64| {
        minx = minx.min(x);
        miny = miny.min(y);
        maxx = maxx.max(x);
        maxy = maxy.max(y);
    };
    for f in &ing.features {
        let (a, b, c, d) = f.geom.bounds();
        widen(a, b);
        widen(c, d);
    }
    for cl in &ing.coastlines {
        for &(x, y) in cl {
            widen(x, y);
        }
    }
    // `as i64` truncates toward zero — same as Python `int()`.
    ((minx * 1e6) as i64, (miny * 1e6) as i64, (maxx * 1e6) as i64, (maxy * 1e6) as i64)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("obc-pack {} (stage 3: ingest + quadtree + serialize)", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("obc-pack: {e}");
            ExitCode::FAILURE
        }
    }
}
