//! The end-to-end pack: `.osm.pbf`(s) → `.obcm`, as one function.
//!
//! This used to live in `main.rs`, which was fine while the CLI was the only way
//! to run it. The desktop app links the packer in rather than spawning it (#906),
//! and two implementations of "the pipeline" — one in a binary, one in a Tauri
//! command — would drift the first time a stage moved. So the pipeline is here and
//! [`pack`] is the only entry point: `main.rs` parses flags and calls it, the
//! desktop app builds the same [`PackOptions`] and calls it, and
//! `tests/cli_library_parity.rs` packs the same fixture both ways and compares the
//! bytes.
//!
//! Everything the run wants to say, and the only way to stop it, arrive together
//! in [`Progress`] — see [`crate::progress`].

use std::io::Write;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::config::Config;
use crate::geom::{footprint_below, strip_small_holes, topology_preserve_simplify, Geom, LodFeature};
use crate::ingest::{ingest_osm, Bbox, IngestFeature, Ingested};
use crate::land;
use crate::merge::{merge_classes, merge_fills_with, merge_line_classes, merge_lines_with, MergeStats};
use crate::progress::{PackError, Phase, Progress};
use crate::quadtree::build_lod_with;
use crate::serialize::serialize_lods_streaming;
use crate::terrain::TerrainSet;
use obc_elevation::{ElevationSource, NullElevation};

// Meters → degrees divisor for simplify tolerance; shared so the packer's scale
// matches the Earth model everything else uses.
use obc_map_scene::M_PER_DEG;

/// Everything a pack run can be told to do differently. The defaults are the
/// plain `obc-pack <pbf> <config> <out>` build.
#[derive(Clone, Debug, Default)]
pub struct PackOptions {
    /// Crop the sources to this box during ingest (pass 0).
    pub bbox: Option<Bbox>,
    /// Override the config's `chunk_size`.
    pub chunk_size: Option<usize>,
    /// Skip land generation even when the config has a land style.
    pub no_land: bool,
    /// Print the classified POI list. A CLI eyeball aid (#422) — it writes to
    /// stdout directly rather than through the progress sink, because a host with
    /// a log pane has no use for a few thousand POI lines.
    pub dump_pois: bool,
    /// Print each POI's parsed weekly schedule (#440). Same stdout caveat.
    pub dump_hours: bool,
    /// Baked OBCT terrain (a `.obcd` container or a directory of them) to integrate the OBCM §8.3
    /// per-direction `Ascent M` from. Absent ⇒ every adjacency entry gets `0`, which is a
    /// decode-valid v12 map that routes exactly as v11 did.
    pub terrain: Option<PathBuf>,
}

/// What a finished run produced.
#[derive(Clone, Copy, Debug)]
pub struct PackSummary {
    /// Size of the written `.obcm`.
    pub bytes: u64,
    /// Features that exceeded `chunk_size` and were dropped. Non-zero means real
    /// map content is missing.
    pub dropped: usize,
}

/// Pack `pbfs` into `output` using `config`.
///
/// The one code path both hosts run. Cancellation is decided by the token inside
/// `progress`, never by an error string: any failure that lands while the token is
/// set is reported as [`PackError::Cancelled`], and the partial output is removed
/// so a cancelled build leaves nothing behind that looks like a map.
pub fn pack(
    pbfs: &[String],
    config: &Config,
    output: &Path,
    opts: &PackOptions,
    progress: &Progress,
) -> Result<PackSummary, PackError> {
    match run(pbfs, config, output, opts, progress) {
        Ok(summary) => Ok(summary),
        Err(e) => {
            if progress.is_cancelled() {
                // A half-written .obcm has a valid header and truncated LODs; the
                // reader would accept the first tree and show a partial map.
                let _ = std::fs::remove_file(output);
                return Err(PackError::Cancelled);
            }
            Err(PackError::Failed(e))
        }
    }
}

fn run(
    pbfs: &[String],
    config: &Config,
    output: &Path,
    opts: &PackOptions,
    progress: &Progress,
) -> Result<PackSummary, String> {
    let out_name = output.display().to_string();
    let chunk_size = opts.chunk_size.unwrap_or(config.chunk_size);
    // Fail loud before any work if chunk_size would let a feature outgrow the reader's cap.
    crate::serialize::validate_chunk_size(chunk_size)?;

    // --- Ingest (two passes: nodes, then ways — three with a bbox, which adds the
    // id-only crop selection; reports its own Merging / Pass 0/1/2 stages and the
    // per-category POI counts line). Several `.pbf`s are merged *inside* those
    // passes — no external tool, no merged intermediate on disk (see
    // [`crate::ingest`]). ---
    let mut ingested = ingest_osm(pbfs, config, opts.bbox, progress)?;
    if ingested.features.is_empty() && ingested.coastlines.is_empty() {
        return Err("no features found matching config".into());
    }
    if opts.dump_pois {
        crate::poi::dump(&ingested.pois);
    }
    if opts.dump_hours {
        crate::poi::dump_hours(&ingested.pois);
    }
    progress.check()?;

    // --- Global bbox over features + coastlines, TRUNCATED toward zero (not rounded)
    // — a deliberate asymmetry with the serializer's round-to-nearest. ---
    progress.stage(Phase::Bbox, "Calculating BBox...");
    let global_bbox = compute_bbox(&ingested);

    // --- Land: clip the global land-polygon dataset to the bbox and add the
    // faces as features, styled by `natural.land`. ---
    add_land(&mut ingested, config, global_bbox, opts.no_land, progress)?;
    progress.check()?;

    // --- Build + serialize the LOD pyramid in one streaming pass: each LOD's tree
    // is built, serialized, streamed to disk, and dropped before the next, so peak
    // memory is ~one tree. ---
    let styles = config.styles();
    // Fill-dissolve / line-stitch equivalence classes over the style table, computed
    // once. Read only when their respective `merge_*` flag is on.
    let fill_classes = merge_classes(&styles);
    let line_classes = merge_line_classes(&styles);
    // Terrain, if the operator supplied any: opened before the output file so a bad `--terrain`
    // fails the run rather than leaving a half-written map behind.
    let terrain_set = match &opts.terrain {
        None => None,
        Some(path) => Some(TerrainSet::open(path)?),
    };
    // Contours: traced out of that same terrain and appended to `ingested` as ordinary line
    // features, before anything looks at a LOD. Everything downstream — simplify, cull, quadtree,
    // serialize — treats them as geometry it has always had, which is the point (#1094).
    crate::contour::add_contours(&mut ingested, config, global_bbox, terrain_set.as_ref(), progress)?;
    progress.check()?;
    let mut sampler = match &terrain_set {
        None => None,
        Some(set) => {
            let s = set.sampler_for(Some(global_bbox))?;
            progress.stage(
                Phase::Serialize,
                format!(
                    "Terrain: {} container(s), {} covering this extract",
                    set.len(),
                    if s.is_empty() { 0 } else { 1 }
                ),
            );
            Some(s)
        }
    };
    let mut null = NullElevation;
    let terrain: &mut dyn ElevationSource = match &mut sampler {
        Some(s) => s,
        None => &mut null,
    };
    let file = std::fs::File::create(output).map_err(|e| format!("create {out_name}: {e}"))?;
    let mut w = std::io::BufWriter::new(file);
    // The per-LOD closure runs inside the serializer, which has no error channel
    // for a *caller's* failure — so a cancellation noticed in here is recorded and
    // re-raised the moment the streaming call returns.
    let (total, dropped) = serialize_lods_streaming(
        &mut w,
        config.lods.len(),
        &styles,
        config.marker_color,
        global_bbox,
        &ingested.pois,
        &ingested.nav_graph,
        &config.routing.profiles,
        terrain,
        |i| {
            let lod = &config.lods[i];
            progress.stage(Phase::Quadtree, format!("Building Quadtree LOD {i} (simplify {}m)...", lod.simplify_m));
            // The cheapest and most valuable checkpoint in the whole pipeline: a
            // cancelled build has one to three of these LODs still ahead of it,
            // each a merge + a simplify + a tree, and skipping them outright is
            // the difference between a cancel that lands in a moment and one that
            // lands in a minute. The empty level still goes through the same
            // build+serialize so the streaming serializer's contract is unchanged.
            if progress.is_cancelled() {
                return (
                    Some(build_lod_with(Vec::<LodFeature>::new(), global_bbox, chunk_size, progress)),
                    chunk_size,
                    lod.max_mpp,
                );
            }
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
            //
            // The cancellation check is here, per feature, rather than only between
            // LODs: this closure is where a country-scale build spends its GEOS
            // time, and a `None` return drains the remaining rayon items in the time
            // it takes to walk them. What is left running after a cancel is at most
            // one `topology_preserve_simplify` per busy worker.
            let simplify_cull = |style_id: u8, level: Option<i16>, geom: &Geom| -> Option<LodFeature> {
                if progress.is_cancelled() {
                    return None;
                }
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
                Some(LodFeature::new(style_id, level, g))
            };
            // Optionally dissolve pixel-identical fill polygons and/or stitch
            // same-styled line fragments BEFORE simplify (see `crate::merge`):
            // merging first deletes shared parcel boundaries / duplicated way
            // endpoints exactly, where simplifying first would move each copy
            // independently (fills: seam cracks) or retain the now-interior junction
            // vertex (lines: less reduction). The two passes are orthogonal (polygon
            // vs line kind), so they compose. Off ⇒ the original filter→simplify→cull
            // path, byte-identical to before.
            let tier: Vec<LodFeature> = if config.merge_fills || config.merge_lines {
                let mut feats: Vec<LodFeature> = ingested
                    .features
                    .iter()
                    .filter(|f| f.min_lod <= i)
                    .map(|f| LodFeature::new(f.style_id, f.level, f.geom.clone()))
                    .collect();
                if config.merge_fills {
                    let (merged, m) = merge_fills_with(feats, &fill_classes, progress);
                    report_merge(progress, m, "fill polygon", "into");
                    feats = merged;
                }
                if config.merge_lines {
                    let (merged, m) = merge_lines_with(feats, &line_classes, progress);
                    report_merge(progress, m, "line fragment", "into");
                    feats = merged;
                }
                feats.par_iter().filter_map(|f| simplify_cull(f.style_id, f.level, &f.geom)).collect()
            } else {
                ingested
                    .features
                    .par_iter()
                    .filter(|f| f.min_lod <= i)
                    .filter_map(|f| simplify_cull(f.style_id, f.level, &f.geom))
                    .collect()
            };
            let culled = culled.load(std::sync::atomic::Ordering::Relaxed);
            if culled > 0 {
                progress.log(format!("  culled {culled} feature(s) below {} px² footprint", lod.min_area_px));
            }
            let holes_stripped = holes_stripped.load(std::sync::atomic::Ordering::Relaxed);
            if holes_stripped > 0 {
                progress.log(format!("  stripped {holes_stripped} sub-pixel hole(s) from surviving polygons"));
            }
            // Always `Some`: a whole-extract pack writes every ladder level as a real (possibly
            // featureless) tree. `None` is the cell cutter's empty out-of-band region (§3.1).
            (Some(build_lod_with(tier, global_bbox, chunk_size, progress)), chunk_size, lod.max_mpp)
        },
    )
    .map_err(|e| format!("write {out_name}: {e}"))?;
    w.flush().map_err(|e| format!("flush {out_name}: {e}"))?;
    // A cancel noticed inside the per-LOD closure produced an empty level rather
    // than an error, so the streaming call returned "successfully" with a stunted
    // map. Catch it here, before anyone is told a file was written.
    progress.check()?;
    // With densify-aware quadtree budgeting this should stay zero; a non-zero count
    // means real map content is missing (a feature too big for its chunk even at
    // the 10-µdeg split floor) and must not pass silently.
    if dropped > 0 {
        progress.warn(format!(
            "warning: {dropped} feature(s) exceeded chunk_size {chunk_size} and were dropped — \
             raise chunk_size or the LOD simplify tolerance"
        ));
    }
    progress.stage(Phase::Serialize, format!("Writing {out_name} ({total} bytes)..."));
    progress.log("Done!");
    Ok(PackSummary { bytes: total, dropped })
}

/// Clip the global land-polygon dataset to `global_bbox` and append the faces to `ingested` as
/// `natural.land` features. A no-op when the config has no land style or `no_land` is set.
///
/// Shared with the cell cutter ([`crate::cut`]): land is generated **once** over the whole extract
/// and then cut like any other feature, so a cell's coastline geometry cannot depend on which cell
/// asked for it.
pub(crate) fn add_land(
    ingested: &mut Ingested,
    config: &Config,
    global_bbox: (i64, i64, i64, i64),
    no_land: bool,
    progress: &Progress,
) -> Result<(), String> {
    if no_land {
        return Ok(());
    }
    let Some(land) = config.land_style() else { return Ok(()) };
    let (lid, lmin) = (land.id, land.min_lod);
    progress.stage(Phase::Land, "Generating land...");
    let bbox_deg = (
        global_bbox.0 as f64 / 1e6,
        global_bbox.1 as f64 / 1e6,
        global_bbox.2 as f64 / 1e6,
        global_bbox.3 as f64 / 1e6,
    );
    let polys = land::get_land_polygons(bbox_deg, progress)?;
    let n = polys.len();
    for geom in polys {
        ingested.features.push(IngestFeature { style_id: lid, min_lod: lmin, level: None, geom });
    }
    progress.log(format!("Successfully added {n} land polygons."));
    Ok(())
}

/// One-line per-LOD merge report (fills or lines), reported only when something
/// actually merged. `noun` names the consumed input ("fill polygon" / "line
/// fragment"); `verb` bridges to the output count ("into").
pub(crate) fn report_merge(progress: &Progress, m: MergeStats, noun: &str, verb: &str) {
    if m.merged_inputs == 0 && m.fallbacks == 0 {
        return;
    }
    let mut line = format!(
        "  merged {} {noun}(s) {verb} {} across {} style class(es)",
        m.merged_inputs, m.merged_outputs, m.merged_classes
    );
    if m.fallbacks > 0 {
        line.push_str(&format!(" ({} group(s) fell back unmerged)", m.fallbacks));
    }
    progress.log(line);
}

/// Total bounds over features + coastlines, then truncate `v*1e6` toward zero. The
/// coords are the exact osmium f64s, so the bbox is stable across runs. Truncation
/// pulls the max edges (and, for negative coordinates, the min edges) inward by
/// under 1 µdeg (~0.11 m); vertices past the shrunken edge are clipped at the root.
pub(crate) fn compute_bbox(ing: &Ingested) -> (i64, i64, i64, i64) {
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
