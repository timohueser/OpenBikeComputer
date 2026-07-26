//! `obc-pack` CLI — the full end-to-end pipeline (`.osm.pbf` → `.obcm`): multi-PBF
//! **merge** → ingest (lines + closed ways + multipolygon relations) → bbox →
//! **land generation** → per-LOD simplify + quadtree → serialize. Positional CLI:
//! `<pbf...> <config.json> <out.obcm>`, plus `--bbox W,S,E,N` (crop the source to
//! a box during ingest — see [`obc_pack::ingest`]), `--chunk-size`, `--no-land`,
//! `--dump-pois` (print the classified POI list for eyeballing), and
//! `--dump-hours` (print each POI's parsed weekly schedule). It
//! prints one stage string per phase ("Cropping", "Merging", "Pass 0/1/2",
//! "Calculating BBox", "Generating land", "Building Quadtree", "Serializing",
//! "Writing") so the web builder UI can show progress — it matches these
//! prefixes, and their order here is the order it expects. `obc-pack schema`
//! prints the config's JSON Schema envelope — the web builder serves it so the
//! editor's capability always matches the binary that packs (`schema --catalog`
//! prints the catalog manifest's schema instead). `obc-pack catalog <bake-tree>`
//! walks a bakery's output tree and writes the map-catalog manifest
//! (`OBCC_Spec.md`).

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, ExitCode};

use rayon::prelude::*;

use obc_pack::config::Config;
use obc_pack::geom::{footprint_below, strip_small_holes, topology_preserve_simplify, Geom};
use obc_pack::ingest::{ingest_osm, Bbox, IngestFeature};
use obc_pack::land;
use obc_pack::merge::{merge_classes, merge_fills, merge_line_classes, merge_lines, MergeStats};
use obc_pack::quadtree::build_lod;
use obc_pack::serialize::serialize_lods_streaming;

// Meters → degrees divisor for simplify tolerance; shared so the packer's scale
// matches the Earth model everything else uses.
use obc_reader::M_PER_DEG;

struct Args {
    pbfs: Vec<String>,
    config: String,
    output: String,
    bbox: Option<Bbox>,
    chunk_size: Option<usize>,
    no_land: bool,
    dump_pois: bool,
    dump_hours: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut bbox = None;
    let mut chunk_size = None;
    let mut no_land = false;
    let mut dump_pois = false;
    let mut dump_hours = false;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            // Validated here, before any file is opened: a malformed or inside-out
            // box must fail with a sentence, not with an empty map an hour later.
            "--bbox" => bbox = Some(Bbox::parse(&it.next().ok_or("--bbox needs W,S,E,N in degrees")?)?),
            "--chunk-size" => {
                chunk_size = Some(it.next().and_then(|s| s.parse().ok()).ok_or("--chunk-size needs a number")?);
            }
            "--no-land" => no_land = true,
            "--dump-pois" => dump_pois = true,
            "--dump-hours" => dump_hours = true,
            _ => positional.push(a),
        }
    }
    // `<pbf...> <config.json> <out.obcm>`: last two positionals are config + output.
    if positional.len() < 3 {
        return Err("usage: obc-pack <pbf...> <config.json> <out.obcm> [--bbox W,S,E,N] [--chunk-size N] [--no-land] \
                    [--dump-pois] [--dump-hours]\n       \
                    obc-pack schema                                 (print the config JSON Schema envelope)\n       \
                    obc-pack catalog <bake-tree> --base-url <url>   (write a bake tree's catalog manifest)"
            .into());
    }
    let output = positional.pop().unwrap();
    let config = positional.pop().unwrap();
    Ok(Args { pbfs: positional, config, output, bbox, chunk_size, no_land, dump_pois, dump_hours })
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let config = Config::load(&args.config)?;
    let chunk_size = args.chunk_size.unwrap_or(config.chunk_size);
    // Fail loud before any work if chunk_size would let a feature outgrow the reader's cap.
    obc_pack::serialize::validate_chunk_size(chunk_size)?;

    // --- Merge: >1 input ⇒ shell out to `osmium merge` + `osmium sort`, then ingest.
    // `_temps` keeps the intermediates alive until run() returns. ---
    let mut _temps: Vec<TempPath> = Vec::new();
    let pbf_to_ingest: String = if args.pbfs.len() > 1 {
        // Cropping each source FIRST when `--bbox` is set is a size decision, not a
        // semantic one. Merging whole countries and cropping afterwards would hand
        // `osmium sort` gigabytes it holds in memory; osmium is already mandatory
        // on this branch (id-wise dedupe + re-sorting are not things ingest can
        // do), so spending it on the crop too costs nothing new. The in-ingest
        // filter still runs over the merged crop and is a no-op there:
        // `complete_ways` is idempotent, because an extract's nodes are exactly
        // "inside the box" plus the halo its own kept ways need. So the result
        // does not depend on which side did the cropping.
        let sources: Vec<String> = match args.bbox {
            Some(bb) => {
                println!("Cropping {} files to bbox...", args.pbfs.len());
                let (w, s, e, n) = bb.to_degrees();
                let spec = format!("{w},{s},{e},{n}");
                let mut cropped = Vec::with_capacity(args.pbfs.len());
                for (i, src) in args.pbfs.iter().enumerate() {
                    let out = TempPath::new(&format!("crop{i}"))?;
                    run_osmium(&["extract", "--overwrite", "--bbox", &spec, src, "-o", out.as_str()])?;
                    cropped.push(out.as_str().to_string());
                    _temps.push(out);
                }
                cropped
            }
            None => args.pbfs.clone(),
        };
        println!("Merging {} files...", sources.len());
        let merged = TempPath::new("merged")?;
        let sorted = TempPath::new("sorted")?;
        let mut merge_args: Vec<&str> = vec!["merge", "--overwrite"];
        for p in &sources {
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

    // --- Ingest (two passes: nodes, then ways — three with `--bbox`, which adds
    // the id-only crop selection; prints its own Pass 0/1/2 stages and the
    // per-category POI counts line). ---
    let mut ingested = ingest_osm(&pbf_to_ingest, &config, args.bbox)?;
    if ingested.features.is_empty() && ingested.coastlines.is_empty() {
        return Err("no features found matching config".into());
    }
    // The classified POIs are serialized into the v6 POI section below; the dump
    // flag is the eyeball-against-a-known-extract debug aid from #422.
    if args.dump_pois {
        obc_pack::poi::dump(&ingested.pois);
    }
    // The parsed opening_hours schedules are pooled + stored by P2 (#441); this
    // flag eyeballs the parsed weekly hours against the raw extract (#440).
    if args.dump_hours {
        obc_pack::poi::dump_hours(&ingested.pois);
    }

    // --- Global bbox over features + coastlines, TRUNCATED toward zero (not rounded)
    // — a deliberate asymmetry with the serializer's round-to-nearest. ---
    println!("Calculating BBox...");
    let global_bbox = compute_bbox(&ingested);

    // --- Land: clip the global land-polygon dataset to the bbox and add the
    // faces as features, styled by `natural.land`. ---
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

    // --- Build + serialize the LOD pyramid in one streaming pass: each LOD's tree
    // is built, serialized, streamed to disk, and dropped before the next, so peak
    // memory is ~one tree. ---
    let styles = config.styles();
    // Fill-dissolve / line-stitch equivalence classes over the style table, computed
    // once. Read only when their respective `merge_*` flag is on.
    let fill_classes = merge_classes(&styles);
    let line_classes = merge_line_classes(&styles);
    let file = std::fs::File::create(&args.output).map_err(|e| format!("create {}: {e}", args.output))?;
    let mut w = std::io::BufWriter::new(file);
    let (total, dropped) = serialize_lods_streaming(
        &mut w,
        config.lods.len(),
        &styles,
        config.marker_color,
        global_bbox,
        &ingested.pois,
        &ingested.nav_graph,
        &config.routing.profiles,
        |i| {
            let lod = &config.lods[i];
            println!("Building Quadtree LOD {i} (simplify {}m)...", lod.simplify_m);
            let tol = if lod.simplify_m > 0.0 { lod.simplify_m / M_PER_DEG } else { 0.0 };
            // Coarse-LOD footprint cull: after simplify, drop features too small to
            // render at the finest scale this tier is ever shown at — the next-finer
            // tier's `max_mpp`. The finest tier has no finer fallback (a drop there
            // would erase the feature at every zoom), so it is never culled and its
            // `min_area_px` is ignored. Off (`None`) ⇒ byte-identical to before.
            let cull_mpp = (lod.min_area_px > 0.0).then(|| config.lods.get(i + 1).and_then(|l| l.max_mpp)).flatten();
            let culled = std::sync::atomic::AtomicUsize::new(0);
            let holes_stripped = std::sync::atomic::AtomicUsize::new(0);
            let min_area_px = lod.min_area_px;
            // Per-feature simplify + coarse-LOD footprint cull + sub-pixel hole trim. Each call runs
            // wholly on one thread using that thread's own GEOS context, so no geometry crosses
            // threads; rayon's `collect` preserves order. The quadtree build stays sequential.
            let simplify_cull = |style_id: u8, geom: &Geom| -> Option<(u8, Geom)> {
                let mut g = if tol > 0.0 { topology_preserve_simplify(geom, tol) } else { geom.clone() };
                if let Some(mpp) = cull_mpp {
                    if footprint_below(&g, mpp, min_area_px) {
                        culled.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        return None;
                    }
                    // Survivors: trim sub-pixel holes (invisible; frees a ring + its vertices in the
                    // render scratch, on the same tier gate + threshold as the footprint cull).
                    let n = strip_small_holes(&mut g, mpp, min_area_px);
                    if n > 0 {
                        holes_stripped.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                Some((style_id, g))
            };
            // Optionally dissolve pixel-identical fill polygons and/or stitch
            // same-styled line fragments BEFORE simplify (see `obc_pack::merge`):
            // merging first deletes shared parcel boundaries / duplicated way
            // endpoints exactly, where simplifying first would move each copy
            // independently (fills: seam cracks) or retain the now-interior junction
            // vertex (lines: less reduction). The two passes are orthogonal (polygon
            // vs line kind), so they compose. Off ⇒ the original filter→simplify→cull
            // path, byte-identical to before.
            let level: Vec<(u8, Geom)> = if config.merge_fills || config.merge_lines {
                let mut feats: Vec<(u8, Geom)> =
                    ingested.features.iter().filter(|f| f.min_lod <= i).map(|f| (f.style_id, f.geom.clone())).collect();
                if config.merge_fills {
                    let (merged, m) = merge_fills(feats, &fill_classes);
                    report_merge(m, "fill polygon", "into");
                    feats = merged;
                }
                if config.merge_lines {
                    let (merged, m) = merge_lines(feats, &line_classes);
                    report_merge(m, "line fragment", "into");
                    feats = merged;
                }
                feats.par_iter().filter_map(|(sid, g)| simplify_cull(*sid, g)).collect()
            } else {
                ingested
                    .features
                    .par_iter()
                    .filter(|f| f.min_lod <= i)
                    .filter_map(|f| simplify_cull(f.style_id, &f.geom))
                    .collect()
            };
            let culled = culled.load(std::sync::atomic::Ordering::Relaxed);
            if culled > 0 {
                println!("  culled {culled} feature(s) below {} px² footprint", lod.min_area_px);
            }
            let holes_stripped = holes_stripped.load(std::sync::atomic::Ordering::Relaxed);
            if holes_stripped > 0 {
                println!("  stripped {holes_stripped} sub-pixel hole(s) from surviving polygons");
            }
            (build_lod(level, global_bbox, chunk_size), chunk_size, lod.max_mpp)
        },
    )
    .map_err(|e| format!("write {}: {e}", args.output))?;
    w.flush().map_err(|e| format!("flush {}: {e}", args.output))?;
    // With densify-aware quadtree budgeting this should stay zero; a non-zero count
    // means real map content is missing (a feature too big for its chunk even at
    // the 10-µdeg split floor) and must not pass silently.
    if dropped > 0 {
        eprintln!(
            "warning: {dropped} feature(s) exceeded chunk_size {chunk_size} and were dropped — \
             raise chunk_size or the LOD simplify tolerance"
        );
    }
    println!("Writing {} ({total} bytes)...", args.output);
    println!("Done!");
    Ok(())
}

/// Total bounds over features + coastlines, then truncate `v*1e6` toward zero. The
/// coords are the exact osmium f64s, so the bbox is stable across runs. Truncation
/// pulls the max edges (and, for negative coordinates, the min edges) inward by
/// under 1 µdeg (~0.11 m); vertices past the shrunken edge are clipped at the root.
/// One-line per-LOD merge report (fills or lines), printed only when something
/// actually merged. `noun` names the consumed input ("fill polygon" / "line
/// fragment"); `verb` bridges to the output count ("into").
fn report_merge(m: MergeStats, noun: &str, verb: &str) {
    if m.merged_inputs == 0 && m.fallbacks == 0 {
        return;
    }
    print!(
        "  merged {} {noun}(s) {verb} {} across {} style class(es)",
        m.merged_inputs, m.merged_outputs, m.merged_classes
    );
    if m.fallbacks > 0 {
        print!(" ({} group(s) fell back unmerged)", m.fallbacks);
    }
    println!();
}

fn compute_bbox(ing: &obc_pack::ingest::Ingested) -> (i64, i64, i64, i64) {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
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
    // `as i64` truncates toward zero — NOT a floor for negatives; see the doc above.
    ((minx * 1e6) as i64, (miny * 1e6) as i64, (maxx * 1e6) as i64, (maxy * 1e6) as i64)
}

/// Run an `osmium` subcommand (merge/sort), erroring helpfully if the CLI is
/// missing.
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

/// A temp file path that deletes itself on drop — the merge/sort intermediates.
struct TempPath(PathBuf);

impl TempPath {
    fn new(tag: &str) -> Result<Self, String> {
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
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

/// `obc-pack catalog <bake-tree> --base-url <url> [--out <path>|-] [--generated-at <ts>]`
///
/// Walks a bake output tree and writes the map-catalog manifest (`OBCC_Spec.md`).
/// Kept in the packer rather than a separate tool because the fact it exists to state
/// — the OBCM version an artifact carries — is this binary's own format version, and a
/// bakery already has `obc-pack` on the box.
fn run_catalog(args: &[String]) -> Result<(), String> {
    const USAGE: &str = "usage: obc-pack catalog <bake-tree> --base-url <url> [--out <path>|-] \
                         [--generated-at <YYYY-MM-DDTHH:MM:SSZ>]";
    let mut tree: Option<PathBuf> = None;
    let mut base_url: Option<String> = None;
    let mut out: Option<String> = None;
    let mut generated_at: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--base-url" => base_url = Some(it.next().ok_or("--base-url needs a URL")?.clone()),
            "--out" => out = Some(it.next().ok_or("--out needs a path (or `-` for stdout)")?.clone()),
            "--generated-at" => {
                generated_at = Some(it.next().ok_or("--generated-at needs an RFC 3339 UTC instant")?.clone());
            }
            other if other.starts_with("--") => return Err(format!("unknown flag `{other}`\n{USAGE}")),
            other if tree.replace(PathBuf::from(other)).is_some() => {
                return Err(format!("only one bake tree can be walked\n{USAGE}"));
            }
            _ => {}
        }
    }
    let tree = tree.ok_or_else(|| USAGE.to_string())?;
    let base_url =
        base_url.ok_or_else(|| format!("--base-url is required — it is where this tree gets published\n{USAGE}"))?;

    // No `--generated-at` ⇒ the system clock, the single wall-clock read on this path.
    // CI should pass it explicitly so a re-run of the same bake is byte-reproducible.
    let opts = obc_pack::catalog::CatalogOptions {
        base_url,
        generated_at: generated_at.unwrap_or_else(obc_pack::catalog::now_timestamp),
    };
    let generated = obc_pack::catalog::generate(&tree, &opts)?;
    // Coverage holes are not fatal, but they must never be silent: a region that
    // quietly failed one preset looks exactly like a deliberate curation choice.
    for w in &generated.warnings {
        eprintln!("obc-pack catalog: warning: {w}");
    }
    match out.as_deref() {
        // Inspection only — stdout cannot be swapped in atomically, so it is never a
        // publish target.
        Some("-") => print!("{}", obc_pack::catalog::manifest_json(&generated.manifest)),
        _ => {
            let path = out.map_or_else(|| tree.join(obc_pack::catalog::DEFAULT_MANIFEST_NAME), PathBuf::from);
            obc_pack::catalog::write_atomic(&path, &generated.manifest)?;
            println!(
                "{}: {} artifacts across {} presets",
                path.display(),
                generated.manifest.artifacts.len(),
                generated.manifest.presets.len()
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version") {
        println!("obc-pack {} (merge + ingest + relations + land + quadtree + serialize)", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("schema") {
        match args.get(1).map(String::as_str) {
            Some("--config") => print!("{}", obc_pack::config::config_schema_json()),
            Some("--catalog") => print!("{}", obc_pack::catalog::catalog_schema_json()),
            _ => println!("{}", obc_pack::config::schema_envelope()),
        }
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("catalog") {
        return match run_catalog(&args[1..]) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("obc-pack catalog: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("obc-pack: {e}");
            ExitCode::FAILURE
        }
    }
}
