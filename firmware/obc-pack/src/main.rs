//! `obc-pack` CLI — the full end-to-end pipeline (`.osm.pbf` → `.obcm`), mirroring
//! `packer/pack.py`: multi-PBF **merge** (Stage 5) → ingest (lines + closed ways +
//! multipolygon relations, Stages 3–4) → bbox → **land generation** (Stage 5) →
//! per-LOD simplify+quadtree → serialize. Same positional CLI as `pack.py`
//! (`<pbf...> <config.json> <out.obcm>`) plus `--chunk-size` and `--no-land`, and
//! it prints the stage strings the web builder's `_STAGE_MARKERS` scrapes
//! ("Merging", "Pass 1/2", "Calculating BBox", "Generating land", "Building
//! Quadtree", "Serializing", "Writing"), so it can be dropped behind
//! `OBC_PACK_BACKEND=rust`.
//!
//! Validation is the feature-multiset + render gate (see `lib.rs` / the corpus
//! README), not byte-identity: simplify runs in GEOS 3.14 here vs shapely's 3.13,
//! and feature/ring order is not reproduced. The serializer + quadtree remain
//! byte-exact in isolation (Stages 1–2).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use rayon::prelude::*;

use obc_pack::config::Config;
use obc_pack::geom::{topology_preserve_simplify, Geom};
use obc_pack::ingest::{ingest_osm, IngestFeature};
use obc_pack::land;
use obc_pack::quadtree::build_lod;
use obc_pack::serialize::serialize_lods_streaming;

// Meters → degrees divisor for the simplify tolerance (mirrors `pack.py`). Shared with
// the reader/route/renderer so the packer's simplification scale matches the Earth model
// everything else measures distance against.
use obc_reader::M_PER_DEG;

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
    let config = Config::load(&args.config)?;
    let chunk_size = args.chunk_size.unwrap_or(config.chunk_size);

    // --- Merge: >1 input ⇒ `osmium merge` + `osmium sort` to a temp, then ingest
    // that (mirrors pack.py; the CLI is battle-tested, so we shell out, plan §6).
    // `_temps` keeps the temp files alive until run() returns, then drops them. ---
    let mut _temps: Vec<TempPath> = Vec::new();
    let pbf_to_ingest: String = if args.pbfs.len() > 1 {
        println!("Merging {} files...", args.pbfs.len());
        let merged = TempPath::new("merged")?;
        let sorted = TempPath::new("sorted")?;
        let mut merge_args: Vec<&str> = vec!["merge", "--overwrite"];
        for p in &args.pbfs {
            merge_args.push(p);
        }
        merge_args.push("-o");
        merge_args.push(merged.as_str());
        run_osmium(&merge_args)?;
        run_osmium(&["sort", "--overwrite", merged.as_str(), "-o", sorted.as_str()])?;
        let path = sorted.as_str().to_string();
        _temps.push(merged);
        _temps.push(sorted);
        path
    } else {
        args.pbfs[0].clone()
    };

    // --- Ingest (two passes: nodes, then ways). ---
    println!("Pass 1: reading nodes...");
    println!("Pass 2: processing ways...");
    let mut ingested = ingest_osm(&pbf_to_ingest, &config)?;
    if ingested.features.is_empty() && ingested.coastlines.is_empty() {
        return Err("no features found matching config".into());
    }

    // --- Global bbox over features + coastlines, then TRUNCATE toward zero
    // (`int(v*1e6)`), NOT round — the deliberate asymmetry from plan §4.3. ---
    println!("Calculating BBox...");
    let global_bbox = compute_bbox(&ingested);

    // --- Land: clip the global land-polygon dataset to the bbox and add the
    // faces as features, styled by `natural.land` (Stage 5, mirrors pack.py). ---
    if !args.no_land {
        if let Some(land) = config.land_style() {
            let (lid, lmin) = (land.id, land.min_lod);
            println!("Generating land...");
            let bbox_deg = (
                global_bbox.0 as f64 / 1e6,
                global_bbox.1 as f64 / 1e6,
                global_bbox.2 as f64 / 1e6,
                global_bbox.3 as f64 / 1e6,
            );
            let polys = land::get_land_polygons(bbox_deg)?;
            let n = polys.len();
            for geom in polys {
                ingested.features.push(IngestFeature { style_id: lid, min_lod: lmin, geom });
            }
            println!("Successfully added {n} land polygons.");
        }
    }

    // --- Build + serialize the LOD pyramid in one streaming pass (Stage 6): each
    // LOD's tree is built (cumulative + per-level simplify, like pack.py),
    // serialized, streamed to disk, and dropped before the next — so peak memory
    // is ~one tree instead of all of them plus the whole output buffer. The bytes
    // are identical to the in-memory serializer. ---
    let styles = config.styles();
    let file =
        std::fs::File::create(&args.output).map_err(|e| format!("create {}: {e}", args.output))?;
    let mut w = std::io::BufWriter::new(file);
    let total = serialize_lods_streaming(
        &mut w,
        config.lods.len(),
        &styles,
        config.marker_color,
        global_bbox,
        |i| {
            let lod = &config.lods[i];
            println!("Building Quadtree LOD {i} (simplify {}m)...", lod.simplify_m);
            let tol = if lod.simplify_m > 0.0 { lod.simplify_m / M_PER_DEG } else { 0.0 };
            // Parallel per-feature simplify (rayon). Each closure runs wholly on
            // one worker thread — to_geos → simplify → back — using that thread's
            // own GEOS context, so no geometry crosses threads. `collect` preserves
            // order, so the feature order into `build_lod` (and thus the output) is
            // unchanged. The quadtree build below stays sequential (bounded memory).
            let level: Vec<(u8, Geom)> = ingested
                .features
                .par_iter()
                .filter(|f| f.min_lod <= i)
                .map(|f| {
                    let g =
                        if tol > 0.0 { topology_preserve_simplify(&f.geom, tol) } else { f.geom.clone() };
                    (f.style_id, g)
                })
                .collect();
            (build_lod(level, global_bbox, chunk_size), chunk_size, lod.max_mpp)
        },
    )
    .map_err(|e| format!("write {}: {e}", args.output))?;
    w.flush().map_err(|e| format!("flush {}: {e}", args.output))?;
    println!("Writing {} ({total} bytes)...", args.output);
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

/// Run an `osmium` subcommand (merge/sort), erroring helpfully if the CLI is
/// missing. The Python pipeline shells out the same way (plan §6).
fn run_osmium(args: &[&str]) -> Result<(), String> {
    let status = Command::new("osmium")
        .args(args)
        .status()
        .map_err(|e| format!("failed to run `osmium` ({e}); install osmium-tool"))?;
    if !status.success() {
        return Err(format!("osmium {} failed with {status}", args.first().copied().unwrap_or("")));
    }
    Ok(())
}

/// A temp file path that deletes itself on drop — the merge/sort intermediates,
/// like `pack.py`'s `NamedTemporaryFile`.
struct TempPath(PathBuf);

impl TempPath {
    fn new(tag: &str) -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut p = std::env::temp_dir();
        p.push(format!("obc-pack-{}-{nanos}-{tag}.osm.pbf", std::process::id()));
        Ok(TempPath(p))
    }

    fn as_str(&self) -> &str {
        self.0.to_str().expect("temp path is utf-8")
    }
}

impl Drop for TempPath {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!(
            "obc-pack {} (stage 5: merge + ingest + relations + land + quadtree + serialize)",
            env!("CARGO_PKG_VERSION")
        );
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
