//! `cut.rs` — the **cell cutter**: one ingested extract in, the cell artifacts of every band it
//! touches out ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §3).
//!
//! A cell artifact is an ordinary OBCM file that a device could open on its own; what makes it a
//! *cell* is a set of constraints, and every one of them is here:
//!
//! - **The header bbox is the grid square, not the content** (§3.1). This is the one place the
//!   packer's usual "the bbox is what the content covers" rule is deliberately inverted, because the
//!   alignment theorem (§2) needs the box to *be* the cell.
//! - **The complete ladder is written, with out-of-band levels empty** (§3.1), so band membership
//!   never appears in the bytes. A `network` cell carries no geometry at all and a `fine` cell no
//!   nav graph, but both list all seven levels.
//! - **Geometry is clipped at the exact cell edge** (§3.3), and the per-LOD sub-pixel cull runs on
//!   the *clipped* geometry, so a polygon may survive in one cell and be culled in its neighbour.
//! - **The nav graph is cut with deterministic boundary junctions on the edge line** (§3.4), which
//!   is the whole reason routing works across a seam. See [`prepare_nav`].
//! - **Island pruning only touches strictly interior components** (§3.5). The real pruning pass is
//!   the assembler's.
//! - **Provenance is recorded and under-covered cells are marked `partial`** (§3.7).
//!
//! # Why this reads the way it does
//!
//! Two orderings in here are load-bearing rather than incidental, and both exist to make seams meet
//! *exactly* rather than nearly:
//!
//! 1. **Simplify before clipping, always.** A clip puts vertices exactly on the edge line, and both
//!    neighbours clip the *same* simplified segment against the *same* line, so their pieces meet to
//!    the microdegree. Simplifying afterwards would let each neighbour move or drop its own copy of
//!    a seam vertex — a visible crack that no amount of tolerance could fix. The cost is that a
//!    feature straddling `k` cells is simplified `k` times; since cells partition space, the total
//!    is about one pass over the extract either way.
//! 2. **Merge fills/lines once, over the whole extract, before cutting.** The union of a cluster of
//!    parcels must be the same geometry in both neighbours or their clips would not meet, and GEOS
//!    overlay is only guaranteed to agree when handed identical inputs. OBCA §2.4 anticipates the
//!    *pessimistic* case (per-cell unions, so an assembly carries slightly more features than a
//!    single-shot bake); cutting a globally merged set is strictly better than that and never worse.
//!
//! The one thing that is *not* streamed is per-band: geometry work is organised band → LOD → cell so
//! that a level's merged feature set is built once and every cell of the band reads it, and only that
//! band's levels are resident. Cells within a band are cut in parallel; nothing in a cell's bytes
//! depends on which thread produced it.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};

use obc_formats::obcm::VERSION as OBCM_VERSION;
use obc_map_scene::M_PER_DEG;

use crate::config::Config;
use crate::geom::{clip_to_box, footprint_below, strip_small_holes, topology_preserve_simplify, Bounds, Geom};
use crate::grid::{
    cells_intersecting, on_grid_boundary, segment_crossing, Axis, Band, BandTable, CellId, UBox, GRID_ORIGIN,
};
use crate::ingest::{Bbox, Ingested};
use crate::merge::{merge_classes, merge_fills_with, merge_line_classes, merge_lines_with};
use crate::nav::{self, CutRun, JunctionKey, NavGraph, RoutableWay};
use crate::poi::Poi;
use crate::progress::{PackError, Phase, Progress};
use crate::quadtree::build_lod_with;
use crate::serialize::{serialize_lods_streaming, validate_chunk_size, Node};
use crate::terrain::TerrainSet;
use obc_elevation::{ElevationSource, NullElevation};

/// Filename of the cutter's provenance sidecar, written **last** (see [`cut_ingested`]).
pub const MANIFEST_NAME: &str = "cells.json";

/// How many refinement passes the boundary-vertex insertion makes before it gives up.
///
/// One pass inserts every crossing of the *original* segment. A second is needed only because a
/// crossing coordinate is rounded to the µdeg grid, which can move it across another line by at most
/// half a microdegree (~5 cm); a third has never been observed. The cap exists so a pathological
/// input cannot spin, and [`prepare_nav`] counts the (expected zero) non-convergences.
const MAX_CUT_REFINE: usize = 4;

/// One source extract a cell was baked from (OBCA §3.7).
///
/// `coverage` is the extract's own coverage box, and the honest answer to "is this cell canonical?".
/// Without it a cell cannot be shown to be fully covered, so it is marked `partial` — deliberately
/// conservative: presenting an under-covered border cell as canonical coverage is exactly the failure
/// D3 exists to prevent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceExtent {
    /// Extract identifier, e.g. `europe/switzerland`.
    pub id: String,
    /// The extract's snapshot date, as the bakery knows it (e.g. `2026-07-01`).
    pub snapshot: Option<String>,
    /// The ground this extract covers, µdeg, in [`UBox`] order. `None` ⇒ unknown ⇒ nothing is
    /// canonical.
    pub coverage: Option<UBox>,
}

impl SourceExtent {
    /// Parse `<id>[@<snapshot>][=W,S,E,N]` — the CLI spelling. Degrees for the box, as everywhere
    /// else a human types a box.
    pub fn parse(spec: &str) -> Result<Self, String> {
        let (head, coverage) = match spec.split_once('=') {
            None => (spec, None),
            Some((head, box_spec)) => {
                let bb = Bbox::parse(box_spec).map_err(|e| format!("--source {spec:?}: {e}"))?;
                let (w, s, e, n) = bb.to_degrees();
                let udeg = |v: f64| (v * 1e6).round_ties_even() as i64;
                (head, Some((udeg(w), udeg(s), udeg(e), udeg(n))))
            }
        };
        let (id, snapshot) = match head.split_once('@') {
            None => (head, None),
            Some((id, snap)) => (id, Some(snap.to_string())),
        };
        if id.is_empty() {
            return Err(format!("--source {spec:?}: the extract id is empty"));
        }
        Ok(SourceExtent { id: id.to_string(), snapshot, coverage })
    }
}

/// Everything a cut run can be told to do differently.
#[derive(Clone, Debug)]
pub struct CutOptions {
    /// The schema's band table (OBCA §1.2). Cell sizes are **schema data**, never format constants.
    pub bands: BandTable,
    /// Cut exactly these cells rather than everything the extract touches. A cell id names a *size*,
    /// and two bands may share one (`fine` and `network` are both `2^18` in the recommended table), so a
    /// selection is cut for every band of that size unless [`CutOptions::only_bands`] narrows it.
    pub select: Vec<CellId>,
    /// Restrict the run to these band ids. Empty ⇒ every band in the table.
    pub only_bands: Vec<String>,
    /// The sources this run is baking from (OBCA §3.7).
    pub sources: Vec<SourceExtent>,
    /// Override the config's `chunk_size`.
    pub chunk_size: Option<usize>,
    /// Skip land generation even when the config has a land style.
    pub no_land: bool,
    /// Crop the sources to this box during ingest.
    pub bbox: Option<Bbox>,
    /// Baked OBCT terrain (a `.obcd` container or a directory of them) to integrate the OBCM §8.3
    /// per-direction `Ascent M` from. Absent ⇒ every adjacency entry gets `0`.
    ///
    /// **Seam-safe by construction.** The cutter slices edges exactly on cell-edge lines and the
    /// ascent of a piece is integrated from the *global* OBCT lattice, never from anything cell-local
    /// — so the stub the western neighbour bakes and the stub the eastern one bakes are each the
    /// integral of their own geometry over one shared surface, and re-cutting a cell alone
    /// reproduces the identical bytes.
    pub terrain: Option<PathBuf>,
    /// Logical source extent used for land generation and the cut manifest.
    ///
    /// Ordinarily the ingest derives this from the retained features. Planet
    /// leaves state it explicitly: a featureless ocean shard still owns cells,
    /// and a quiet corner of a leaf still needs the global land layer considered.
    pub source_extent: Option<UBox>,
}

impl Default for CutOptions {
    fn default() -> Self {
        CutOptions {
            bands: BandTable::recommended(),
            select: Vec::new(),
            only_bands: Vec::new(),
            sources: Vec::new(),
            chunk_size: None,
            no_land: false,
            bbox: None,
            terrain: None,
            source_extent: None,
        }
    }
}

/// One written cell artifact.
#[derive(Clone, Debug)]
pub struct CellArtifact {
    pub id: CellId,
    pub band: String,
    /// Path relative to the run's output directory.
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    /// The sources do not demonstrably cover the whole square (OBCA §3.7).
    pub partial: bool,
    /// Features that exceeded `chunk_size` and were dropped — never expected, never silent.
    pub dropped: usize,
    pub pois: usize,
    pub nav_nodes: usize,
    pub nav_edges: usize,
    /// The serialized band carries no geometry, POIs, or navigation content.
    ///
    /// `dropped > 0` always makes this false: losing oversized source content is
    /// not proof that the canonical cell is semantically empty.
    pub empty: bool,
}

/// What a finished cut run produced.
#[derive(Clone, Debug)]
pub struct CutSummary {
    /// Every cell written, in band-table order then ascending `(i, j)`.
    pub cells: Vec<CellArtifact>,
    pub bytes: u64,
    pub dropped: usize,
    /// Cells marked `partial` (OBCA §3.7).
    pub partial: usize,
}

/// Ingest `pbfs` **once** and cut every cell of every band they touch into `out_dir`.
pub fn cut(
    pbfs: &[String],
    config: &Config,
    out_dir: &Path,
    opts: &CutOptions,
    progress: &Progress,
) -> Result<CutSummary, PackError> {
    match run(pbfs, config, out_dir, opts, progress) {
        Ok(summary) => Ok(summary),
        Err(e) => {
            if progress.is_cancelled() {
                // The manifest is written last, so a cancelled run leaves no document claiming the
                // half-written tree is a catalog — the same atomicity trick §5.4 uses for a set.
                let _ = std::fs::remove_file(out_dir.join(MANIFEST_NAME));
                return Err(PackError::Cancelled);
            }
            Err(PackError::Failed(e))
        }
    }
}

fn run(
    pbfs: &[String],
    config: &Config,
    out_dir: &Path,
    opts: &CutOptions,
    progress: &Progress,
) -> Result<CutSummary, String> {
    // The ways, not a graph: the cutter builds one graph per cell (OBCA §3.4).
    let (mut ingested, ways) = crate::ingest::ingest_osm_ways(pbfs, config, opts.bbox, progress)?;
    if ingested.features.is_empty() && ingested.coastlines.is_empty() && opts.source_extent.is_none() {
        return Err("no features found matching config".into());
    }
    progress.check()?;
    progress.stage(Phase::Bbox, "Calculating BBox...");
    let extract = opts.source_extent.unwrap_or_else(|| crate::pipeline::compute_bbox(&ingested));
    crate::pipeline::add_land(&mut ingested, config, extract, opts.no_land, progress)?;
    progress.check()?;
    // Contours are generated **once** over the whole extract and then cut like any other feature,
    // for the same reason land is: a cell's geometry must not depend on which cell asked for it.
    // This opens the terrain set a second time (the cutter opens its own for the §8.3 ascent pass)
    // — a header read and a directory validation per container, and only when contours are on.
    let contour_terrain = match (&opts.terrain, config.contours.enabled) {
        (Some(path), true) => Some(TerrainSet::open(path)?),
        _ => None,
    };
    crate::contour::add_contours(&mut ingested, config, extract, contour_terrain.as_ref(), progress)?;
    progress.check()?;
    cut_ingested(&ingested, &ways, config, out_dir, opts, progress)
}

/// Cut an already-ingested extract — the entry point tests and the bakery both drive.
///
/// `ways` are the routable ways of the **source snapshot** (from [`crate::ingest::ingest_osm_ways`]);
/// they are what junction-ness is classified from, so handing in a subset would quietly change the
/// graph a cell writes.
///
/// Writes `<out_dir>/cells/<band>/<i>/<j>.obcm` plus the provenance sidecar
/// `<out_dir>/`[`MANIFEST_NAME`] — **last**, so an interrupted run publishes nothing.
pub fn cut_ingested(
    ing: &Ingested,
    ways: &[RoutableWay],
    config: &Config,
    out_dir: &Path,
    opts: &CutOptions,
    progress: &Progress,
) -> Result<CutSummary, String> {
    let chunk_size = opts.chunk_size.unwrap_or(config.chunk_size);
    validate_chunk_size(chunk_size)?;
    opts.bands.validate(config.lods.len())?;
    for id in &opts.only_bands {
        if opts.bands.band(id).is_none() {
            return Err(format!("--band {id:?} is not in the band table"));
        }
    }
    for c in &opts.select {
        if !opts.bands.bands.iter().any(|b| b.cell_log2 == c.log2) {
            return Err(format!("--cell {c}: no band in the table uses cell size 2^{}", c.log2));
        }
    }
    let extract = opts.source_extent.unwrap_or_else(|| crate::pipeline::compute_bbox(ing));
    // Opened once for the whole run and shared by every cell: validating a hundred containers per
    // cell would dominate a cut. `sampler_for` is the per-cell part.
    let terrain_set = match &opts.terrain {
        None => None,
        Some(path) => Some(TerrainSet::open(path)?),
    };
    let styles = config.styles();
    let mut artifacts: Vec<CellArtifact> = Vec::new();

    for band in &opts.bands.bands {
        if !opts.only_bands.is_empty() && !opts.only_bands.contains(&band.id) {
            continue;
        }
        let cells = select_cells(band, extract, &opts.select);
        if cells.is_empty() {
            continue;
        }
        progress.stage(
            Phase::Quadtree,
            format!("Cutting band {} (2^{} µdeg): {} cell(s)...", band.id, band.cell_log2, cells.len()),
        );

        // Per-band preparation, done once and read by every cell of the band.
        let lod_sets: Vec<LodSet<'_>> =
            band.lods.iter().map(|&l| prepare_lod(ing, config, l, band.cell_log2, progress)).collect();
        let nav_cut = if band.has_nav() { Some(prepare_nav(ways, band.cell_log2, progress)?) } else { None };
        let poi_cells = if band.has_poi() { bucket_pois(&ing.pois, band.cell_log2) } else { HashMap::new() };
        progress.check()?;

        let written: Vec<Result<CellArtifact, String>> = cells
            .par_iter()
            .map(|cell| {
                if progress.is_cancelled() {
                    return Err("cancelled".into());
                }
                let pois: Vec<Poi> = poi_cells
                    .get(&(cell.i, cell.j))
                    .map(|ix| ix.iter().map(|&k| ing.pois[k as usize].clone()).collect())
                    .unwrap_or_default();
                let graph = match &nav_cut {
                    None => NavGraph::default(),
                    Some(prep) => prep.cell_graph(*cell, config.routing.min_component_edges),
                };
                let trees: Vec<(usize, Node)> =
                    lod_sets.iter().map(|set| (set.lod, set.cell_tree(*cell, chunk_size, progress))).collect();
                // One sampler per cell: it opens only the OBCT containers this square touches, and
                // an `ElevationSource` is `&mut` by design (it caches tiles), so it cannot be shared
                // across the rayon workers. A cell outside the supplied terrain gets an empty
                // sampler, which answers `None` everywhere exactly like `NullElevation`.
                let mut sampler = match &terrain_set {
                    None => None,
                    Some(set) => Some(set.sampler_for(Some(cell.square()))?),
                };
                let mut null = NullElevation;
                let terrain: &mut dyn ElevationSource = match &mut sampler {
                    Some(s) => s,
                    None => &mut null,
                };
                write_cell(
                    cell,
                    band,
                    out_dir,
                    config,
                    &styles,
                    chunk_size,
                    trees,
                    &pois,
                    &graph,
                    terrain,
                    &opts.sources,
                )
            })
            .collect();
        for w in written {
            artifacts.push(w?);
        }
        progress.check()?;
    }

    let summary = CutSummary {
        bytes: artifacts.iter().map(|a| a.bytes).sum(),
        dropped: artifacts.iter().map(|a| a.dropped).sum(),
        partial: artifacts.iter().filter(|a| a.partial).count(),
        cells: artifacts,
    };
    if summary.dropped > 0 {
        progress.warn(format!(
            "warning: {} feature(s) exceeded chunk_size {chunk_size} and were dropped — raise chunk_size or the \
             LOD simplify tolerance",
            summary.dropped
        ));
    }
    write_manifest(out_dir, config, opts, extract, &summary)?;
    progress.stage(
        Phase::Serialize,
        format!("Wrote {} cell(s), {} bytes ({} partial)", summary.cells.len(), summary.bytes, summary.partial),
    );
    Ok(summary)
}

/// The cells of one band this run must emit: the explicit selection filtered to the band's size, or
/// every cell of the band whose square intersects the extract (OBCA §1.2's coverage rule).
fn select_cells(band: &Band, extract: UBox, select: &[CellId]) -> Vec<CellId> {
    let mut cells: Vec<CellId> = if select.is_empty() {
        cells_intersecting(band.cell_log2, extract)
    } else {
        select.iter().copied().filter(|c| c.log2 == band.cell_log2).collect()
    };
    cells.sort_unstable();
    cells.dedup();
    cells
}

// --- geometry ---------------------------------------------------------------------------------

/// One ladder level, prepared once per band: the merged features that reach it, their bounds, and a
/// bucket index from cell to candidate features.
struct LodSet<'a> {
    /// Ladder index.
    lod: usize,
    feats: Vec<(u8, Cow<'a, Geom>)>,
    /// `(i, j)` → indices into `feats`. Membership is decided on **inclusive** bounds, so a feature
    /// reaching a seam line is a candidate on both sides and the two cells clip identical geometry.
    buckets: HashMap<(i64, i64), Vec<u32>>,
    /// Simplify tolerance, degrees (`0.0` ⇒ none).
    tol: f64,
    /// The m/px the footprint cull measures at, `None` ⇒ no cull for this level.
    cull_mpp: Option<f64>,
    min_area_px: f64,
    cell_log2: u32,
}

/// Build a level's feature set exactly as [`crate::pipeline`] does — `min_lod` filter, then the
/// optional fill-dissolve and line-stitch passes — and index it by cell.
///
/// The merges run here, over the whole extract, and not per cell: see the module docs. Simplify does
/// **not** run here, because it must run on the geometry a *cell* clips (also the module docs).
fn prepare_lod<'a>(ing: &'a Ingested, config: &Config, lod: usize, cell_log2: u32, progress: &Progress) -> LodSet<'a> {
    let l = &config.lods[lod];
    // `Geom::bounds` panics on an empty geometry, and a merge pass can hand one back, so empties are
    // dropped here — exactly where `build_lod_with` drops them on the whole-extract path.
    let mut feats: Vec<(u8, Cow<'a, Geom>)> = ing
        .features
        .iter()
        .filter(|f| f.min_lod <= lod && !f.geom.is_empty())
        .map(|f| (f.style_id, Cow::Borrowed(&f.geom)))
        .collect();
    if config.merge_fills || config.merge_lines {
        let styles = config.styles();
        let owned: Vec<(u8, Geom)> = feats.into_iter().map(|(s, g)| (s, g.into_owned())).collect();
        let mut owned = owned;
        if config.merge_fills {
            let (merged, m) = merge_fills_with(owned, &merge_classes(&styles), progress);
            crate::pipeline::report_merge(progress, m, "fill polygon", "into");
            owned = merged;
        }
        if config.merge_lines {
            let (merged, m) = merge_lines_with(owned, &merge_line_classes(&styles), progress);
            crate::pipeline::report_merge(progress, m, "line fragment", "into");
            owned = merged;
        }
        feats = owned.into_iter().filter(|(_, g)| !g.is_empty()).map(|(s, g)| (s, Cow::Owned(g))).collect();
    }

    let bounds: Vec<Bounds> = feats.iter().map(|(_, g)| g.bounds()).collect();
    let mut buckets: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (k, b) in bounds.iter().enumerate() {
        for cell in cells_intersecting(cell_log2, bounds_to_udeg(*b)) {
            buckets.entry((cell.i, cell.j)).or_default().push(k as u32);
        }
    }
    // The cull's reference scale is the next-finer tier's `max_mpp`; the finest tier is never culled
    // (a drop there would erase the feature at every zoom).
    let cull_mpp = (l.min_area_px > 0.0).then(|| config.lods.get(lod + 1).and_then(|n| n.max_mpp)).flatten();
    LodSet {
        lod,
        feats,
        buckets,
        tol: if l.simplify_m > 0.0 { l.simplify_m / M_PER_DEG } else { 0.0 },
        cull_mpp,
        min_area_px: l.min_area_px,
        cell_log2,
    }
}

/// Degree bounds → µdeg, widened outward so a candidate is never missed to a rounding step.
fn bounds_to_udeg(b: Bounds) -> UBox {
    ((b.0 * 1e6).floor() as i64, (b.1 * 1e6).floor() as i64, (b.2 * 1e6).ceil() as i64, (b.3 * 1e6).ceil() as i64)
}

impl LodSet<'_> {
    /// This level's quadtree for one cell: simplify → clip at the exact cell edge → cull the
    /// **clipped** geometry (OBCA §3.3) → build the tree over the cell square.
    fn cell_tree(&self, cell: CellId, chunk_size: usize, progress: &Progress) -> Node {
        debug_assert_eq!(cell.log2, self.cell_log2);
        let square = cell.square();
        let dbox = (square.0 as f64 / 1e6, square.1 as f64 / 1e6, square.2 as f64 / 1e6, square.3 as f64 / 1e6);
        let candidates = self.buckets.get(&(cell.i, cell.j)).map(Vec::as_slice).unwrap_or(&[]);
        let mut out: Vec<(u8, Geom)> = Vec::new();
        for &k in candidates {
            let (style_id, geom) = &self.feats[k as usize];
            let simplified =
                if self.tol > 0.0 { topology_preserve_simplify(geom, self.tol) } else { geom.as_ref().clone() };
            if simplified.is_empty() {
                continue;
            }
            let b = simplified.bounds();
            let clipped = if b.0 >= dbox.0 && b.2 <= dbox.2 && b.1 >= dbox.1 && b.3 <= dbox.3 {
                simplified // wholly inside: no clip, no vertex touched
            } else if b.2 < dbox.0 || b.0 > dbox.2 || b.3 < dbox.1 || b.1 > dbox.3 {
                continue; // a bounds-only candidate that the simplify moved out of reach
            } else {
                clip_to_box(&simplified, square)
            };
            flatten_culled(*style_id, clipped, self.cull_mpp, self.min_area_px, &mut out);
        }
        build_lod_with(out, square, chunk_size, progress)
    }
}

/// Append `geom`'s simple parts to `out`, dropping the ones the sub-pixel footprint cull rejects and
/// trimming sub-pixel holes from the survivors — the pipeline's cull, applied to clipped geometry.
fn flatten_culled(style_id: u8, geom: Geom, cull_mpp: Option<f64>, min_area_px: f64, out: &mut Vec<(u8, Geom)>) {
    match geom {
        Geom::Empty => {}
        Geom::Multi(parts) => {
            for p in parts {
                flatten_culled(style_id, p, cull_mpp, min_area_px, out);
            }
        }
        mut simple => {
            if let Some(mpp) = cull_mpp {
                if footprint_below(&simple, mpp, min_area_px) {
                    return;
                }
                strip_small_holes(&mut simple, mpp, min_area_px);
            }
            out.push((style_id, simple));
        }
    }
}

// --- POIs -------------------------------------------------------------------------------------

/// Bucket POIs by the one cell whose half-open square contains them (OBCA §3.6). Indices into
/// `pois`, in input order, so a cell's records are ordered deterministically.
fn bucket_pois(pois: &[Poi], cell_log2: u32) -> HashMap<(i64, i64), Vec<u32>> {
    let mut out: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    for (k, p) in pois.iter().enumerate() {
        let cell = CellId::containing(cell_log2, p.lat_udeg as i64, p.lon_udeg as i64);
        out.entry((cell.i, cell.j)).or_default().push(k as u32);
    }
    out
}

// --- the nav cut (OBCA §3.4) ------------------------------------------------------------------

/// One source way with a boundary vertex inserted at every crossing of the nav band's grid.
struct PreparedWay {
    keys: Vec<JunctionKey>,
    /// µdeg `(lon, lat)`, parallel to `keys`.
    coords: Vec<(i32, i32)>,
    kind: u8,
}

/// The whole extract's routable ways, cut-ready: boundary vertices inserted, junction touch counts
/// taken over the **source snapshot**, and an index from cell to the ways that reach it.
struct NavCut {
    log2: u32,
    ways: Vec<PreparedWay>,
    /// OSM node id → how many routable ways of the source touch it. Junction-ness is classified from
    /// this, never from the ways that survive inside a cell (§3.4).
    touch: HashMap<i64, u32>,
    cells: HashMap<(i64, i64), Vec<u32>>,
}

/// Prepare the nav cut: insert the deterministic boundary junctions and index the ways by cell.
///
/// The insertion is the heart of the seam contract. For every routable way and every cell-edge line
/// it crosses, a vertex is materialised at the crossing coordinate, computed by
/// [`segment_crossing`] — exact `i128` interpolation with banker's rounding over canonically ordered
/// endpoints. Both neighbours run that computation over the same two source vertices and the same
/// line, so they mint **the same integer pair**, which is what lets an assembler unify the two stubs
/// by exact coordinate equality and nothing weaker (§3.4's epsilon rule).
///
/// A vertex that already lies exactly on a line is *itself* the boundary junction (§3.4(1)) — no
/// interpolation, and no new key: it keeps its OSM identity and becomes a junction because
/// [`NavCut::cell_graph`]'s predicate tests the coordinate.
///
/// A [`RoutableWay`] whose `coords` and `node_ids` are not two parallel lists of at least two
/// entries is rejected rather than indexed: every step below reads the two positionally, so a
/// malformed one would have been an index panic or a length underflow deep inside the cut — and
/// [`cut_ingested`] is a `pub` entry point the bakery hands ingested data to.
fn prepare_nav(ways: &[RoutableWay], log2: u32, progress: &Progress) -> Result<NavCut, String> {
    let mut touch: HashMap<i64, u32> = HashMap::new();
    for w in ways {
        if w.coords.len() < 2 || w.node_ids.len() != w.coords.len() {
            return Err(format!(
                "malformed routable way: {} coordinate(s) and {} node id(s) — a nav way needs at least two of \
                 each, paired",
                w.coords.len(),
                w.node_ids.len()
            ));
        }
        for &nid in &w.node_ids {
            *touch.entry(nid).or_insert(0) += 1;
        }
    }
    let mut prepared = Vec::with_capacity(ways.len());
    let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
    let mut inserted = 0usize;
    let mut unconverged = 0usize;
    for w in ways {
        let mut keys: Vec<JunctionKey> = Vec::with_capacity(w.coords.len());
        let mut coords: Vec<(i32, i32)> = Vec::with_capacity(w.coords.len());
        keys.push(JunctionKey::Osm(w.node_ids[0]));
        coords.push(w.coords[0]);
        for k in 0..w.coords.len() - 1 {
            let (cuts, converged) = segment_cuts(w.coords[k], w.coords[k + 1], log2);
            if !converged {
                unconverged += 1;
            }
            for c in cuts {
                keys.push(JunctionKey::Boundary(c.0, c.1));
                coords.push(c);
                inserted += 1;
            }
            keys.push(JunctionKey::Osm(w.node_ids[k + 1]));
            coords.push(w.coords[k + 1]);
        }
        let idx = prepared.len() as u32;
        let mut owners: Vec<(i64, i64)> = coords.windows(2).map(|s| segment_owner(s[0], s[1], log2)).collect();
        owners.sort_unstable();
        owners.dedup();
        for o in owners {
            cells.entry(o).or_default().push(idx);
        }
        prepared.push(PreparedWay { keys, coords, kind: w.kind });
    }
    progress.log(format!("nav cut: {inserted} boundary junction(s) inserted across {} routable way(s)", ways.len()));
    if unconverged > 0 {
        // Never observed; a segment whose rounded crossings keep landing across another line would
        // leave sub-µdeg geometry on the wrong side of an edge. Loud rather than silent.
        progress.warn(format!(
            "warning: {unconverged} segment(s) did not converge in {MAX_CUT_REFINE} boundary-cut refinements"
        ));
    }
    Ok(NavCut { log2, ways: prepared, touch, cells })
}

/// Every grid line of size `2^log2` strictly between `v0` and `v1`, ascending.
fn lines_strictly_between(v0: i64, v1: i64, log2: u32) -> impl Iterator<Item = i64> {
    let s = 1i64 << log2;
    let (lo, hi) = (v0.min(v1), v0.max(v1));
    let first = GRID_ORIGIN + ((lo - GRID_ORIGIN).div_euclid(s) + 1) * s;
    std::iter::successors(Some(first), move |v| Some(v + s)).take_while(move |v| *v < hi)
}

/// The boundary junctions on segment `a`–`b` (µdeg `(lon, lat)`), ordered along the segment.
///
/// Returns `(cuts, converged)`. A crossing coordinate is rounded to the µdeg grid, which can in
/// principle push it across a *different* line by half a microdegree, so the segment is re-scanned
/// until no proper crossing is left (or [`MAX_CUT_REFINE`] passes have run).
fn segment_cuts(a: (i32, i32), b: (i32, i32), log2: u32) -> (Vec<(i32, i32)>, bool) {
    // Fast path, and it is the overwhelmingly common one: an OSM segment is metres long and crosses
    // nothing, so the first pass is also the only pass and no chain is ever built.
    let first = crossings(a, b, log2);
    if first.is_empty() {
        return (Vec::new(), true);
    }
    let mut chain = Vec::with_capacity(first.len() + 2);
    chain.push(a);
    chain.extend(first);
    chain.push(b);
    let mut converged = false;
    for _ in 1..MAX_CUT_REFINE {
        let mut next: Vec<(i32, i32)> = Vec::with_capacity(chain.len() + 4);
        next.push(chain[0]);
        let mut added = 0usize;
        for w in chain.windows(2) {
            let cuts = crossings(w[0], w[1], log2);
            added += cuts.len();
            next.extend(cuts);
            next.push(w[1]);
        }
        chain = next;
        if added == 0 {
            converged = true;
            break;
        }
    }
    let n = chain.len();
    (chain[1..n - 1].to_vec(), converged)
}

/// The proper crossings of one segment with the grid, ordered along the segment. Endpoints and
/// duplicates are excluded: a vertex already on a line needs no interpolation (§3.4(1)).
fn crossings(a: (i32, i32), b: (i32, i32), log2: u32) -> Vec<(i32, i32)> {
    // §3.4's formula is written in (lat, lon); the packer's coordinates are (lon, lat).
    let (p, q) = ((a.1 as i64, a.0 as i64), (b.1 as i64, b.0 as i64));
    // Each crossing carries its position along the segment as an exact rational `num/den`, so
    // crossings of the two axes sort into one order without a float anywhere.
    let mut found: Vec<(i128, i128, (i32, i32))> = Vec::new();
    for c in lines_strictly_between(p.0, q.0, log2) {
        if let Some((lat, lon)) = segment_crossing(p, q, Axis::Lat, c) {
            found.push(((c - p.0) as i128, (q.0 - p.0) as i128, (lon as i32, lat as i32)));
        }
    }
    for c in lines_strictly_between(p.1, q.1, log2) {
        if let Some((lat, lon)) = segment_crossing(p, q, Axis::Lon, c) {
            found.push(((c - p.1) as i128, (q.1 - p.1) as i128, (lon as i32, lat as i32)));
        }
    }
    for f in &mut found {
        if f.1 < 0 {
            (f.0, f.1) = (-f.0, -f.1);
        }
    }
    found.sort_by(|x, y| (x.0 * y.1).cmp(&(y.0 * x.1)).then(x.2.cmp(&y.2)));
    let mut out: Vec<(i32, i32)> = Vec::with_capacity(found.len());
    for (_, _, pt) in found {
        if pt == a || pt == b || out.last() == Some(&pt) {
            continue;
        }
        out.push(pt);
    }
    out
}

/// The cell that owns segment `(a, b)` — valid once the segment crosses no grid line.
///
/// Per axis it is `div_euclid(min − origin, S)`, which is the half-open convention read off the
/// segment: a segment sitting exactly **on** an edge line belongs to the cell for which that line is
/// a `min` edge (OBCA §3.4(3)), so it is written once and never twice.
fn segment_owner(a: (i32, i32), b: (i32, i32), log2: u32) -> (i64, i64) {
    let s = 1i64 << log2;
    ((a.1.min(b.1) as i64 - GRID_ORIGIN).div_euclid(s), (a.0.min(b.0) as i64 - GRID_ORIGIN).div_euclid(s))
}

impl NavCut {
    /// The runs of source ways this cell owns: maximal chains of segments whose owner is `cell`.
    ///
    /// A run therefore ends only at a boundary junction or at a way's own end, which is why
    /// [`nav::build_graph_cut`] can treat every run endpoint as a junction.
    fn cell_runs(&self, cell: CellId) -> Vec<CutRun> {
        let key = (cell.i, cell.j);
        let mut runs = Vec::new();
        let Some(ways) = self.cells.get(&key) else { return runs };
        for &wi in ways {
            let w = &self.ways[wi as usize];
            let mut start: Option<usize> = None;
            for k in 0..w.coords.len() - 1 {
                let mine = segment_owner(w.coords[k], w.coords[k + 1], self.log2) == key;
                match (mine, start) {
                    (true, None) => start = Some(k),
                    (false, Some(s)) => {
                        runs.push(CutRun {
                            keys: w.keys[s..=k].to_vec(),
                            coords: w.coords[s..=k].to_vec(),
                            kind: w.kind,
                        });
                        start = None;
                    }
                    _ => {}
                }
            }
            if let Some(s) = start {
                runs.push(CutRun { keys: w.keys[s..].to_vec(), coords: w.coords[s..].to_vec(), kind: w.kind });
            }
        }
        runs
    }

    /// This cell's nav graph: junction-ness from the source snapshot plus every vertex on a boundary
    /// line, and pruning restricted to strictly interior components (OBCA §3.4/§3.5).
    fn cell_graph(&self, cell: CellId, min_component_edges: usize) -> NavGraph {
        let runs = self.cell_runs(cell);
        let log2 = self.log2;
        let is_junction = |key: JunctionKey, coord: (i32, i32)| match key {
            // Minted on the edge line: a junction in both neighbours, by construction.
            JunctionKey::Boundary(..) => true,
            // A real OSM node is a junction if the source's way set makes it one — or if it happens
            // to sit exactly on a boundary line, which is the §3.4(1) case.
            JunctionKey::Osm(id) => {
                on_grid_boundary(coord.1 as i64, coord.0 as i64, log2) || self.touch.get(&id).copied().unwrap_or(0) >= 2
            }
        };
        let on_boundary = |coord: (i32, i32)| on_grid_boundary(coord.1 as i64, coord.0 as i64, log2);
        let (graph, _stats) = nav::build_graph_cut(&runs, min_component_edges, &is_junction, &on_boundary);
        graph
    }
}

// --- writing ----------------------------------------------------------------------------------

/// Serialize and write one cell artifact.
///
/// The header bbox **is** the cell square (§3.1), the ladder is complete with out-of-band levels
/// written empty, and the POI/nav sections are present but empty unless the band carries them.
#[allow(clippy::too_many_arguments)]
fn write_cell(
    cell: &CellId,
    band: &Band,
    out_dir: &Path,
    config: &Config,
    styles: &[crate::serialize::Style],
    chunk_size: usize,
    trees: Vec<(usize, Node)>,
    pois: &[Poi],
    graph: &NavGraph,
    terrain: &mut dyn ElevationSource,
    sources: &[SourceExtent],
) -> Result<CellArtifact, String> {
    let square = cell.square();
    let rel = cell_path(band, cell);
    let path = out_dir.join(&rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let file = std::fs::File::create(&path).map_err(|e| format!("create {}: {e}", path.display()))?;
    let mut w = std::io::BufWriter::new(file);
    let had_geometry = trees.iter().any(|(_, tree)| node_has_features(tree));
    let mut trees: Vec<Option<(usize, Node)>> = trees.into_iter().map(Some).collect();
    let (bytes, dropped) = serialize_lods_streaming(
        &mut w,
        config.lods.len(),
        styles,
        config.marker_color,
        square,
        pois,
        graph,
        &config.routing.profiles,
        terrain,
        |i| {
            // In band ⇒ its tree; out of band ⇒ an empty region, so band membership never shows up
            // in the bytes (§3.1).
            let root = trees.iter_mut().find(|t| t.as_ref().is_some_and(|(l, _)| *l == i)).and_then(Option::take);
            (root.map(|(_, n)| n), chunk_size, config.lods[i].max_mpp)
        },
    )
    .map_err(|e| format!("write {}: {e}", path.display()))?;
    use std::io::Write;
    w.flush().map_err(|e| format!("flush {}: {e}", path.display()))?;
    drop(w);

    let digest = sha256_file(&path)?;
    Ok(CellArtifact {
        id: *cell,
        band: band.id.clone(),
        path: rel,
        bytes,
        sha256: digest,
        partial: !sources_cover(square, sources),
        dropped,
        pois: pois.len(),
        nav_nodes: graph.nodes.len(),
        nav_edges: graph.edges.len(),
        empty: !had_geometry && pois.is_empty() && graph.nodes.is_empty() && dropped == 0,
    })
}

fn node_has_features(node: &Node) -> bool {
    match node {
        Node::Leaf { features, .. } => !features.is_empty(),
        Node::Branch(children) => children.iter().any(node_has_features),
    }
}

/// A cell artifact's path inside the run's output directory.
///
/// Keyed by **band**, not by `log2`: two bands may legitimately share a cell size (`fine` and
/// `network` are both `2^18` in the recommended table), and `OBCC_Spec.md` §2's cell path
/// sketch collides for exactly that pair. Every cell's path is stated explicitly in the manifest, so
/// a publisher never has to infer it.
fn cell_path(band: &Band, cell: &CellId) -> String {
    let w = crate::grid::id_width(cell.log2);
    format!("cells/{}/{:0w$}/{:0w$}.obcm", band.id, cell.i, cell.j, w = w)
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether the declared source coverage contains the whole square (OBCA §3.7).
///
/// Coverage is the union of the sources' boxes, so two co-baked neighbours can make a border cell
/// canonical between them. The test is exact: the square is decomposed on the boxes' own coordinates
/// and every elementary piece must be inside some box — a "mostly covered" square is `partial`.
fn sources_cover(square: UBox, sources: &[SourceExtent]) -> bool {
    let boxes: Vec<UBox> = sources.iter().filter_map(|s| s.coverage).collect();
    if boxes.is_empty() {
        return false;
    }
    let (min_lon, min_lat, max_lon, max_lat) = square;
    let mut xs = vec![min_lon, max_lon];
    let mut ys = vec![min_lat, max_lat];
    for b in &boxes {
        for v in [b.0, b.2] {
            if v > min_lon && v < max_lon {
                xs.push(v);
            }
        }
        for v in [b.1, b.3] {
            if v > min_lat && v < max_lat {
                ys.push(v);
            }
        }
    }
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let covered = boxes.iter().any(|b| b.0 <= x[0] && b.2 >= x[1] && b.1 <= y[0] && b.3 >= y[1]);
            if !covered {
                return false;
            }
        }
    }
    true
}

// --- the provenance sidecar -------------------------------------------------------------------

#[derive(Serialize)]
struct ManifestLod {
    index: usize,
    max_mpp: Option<f64>,
    band: String,
}

#[derive(Serialize)]
struct ManifestRouting {
    min_component_edges: usize,
}

#[derive(Serialize)]
struct ManifestSchema<'a> {
    obcm_version: u8,
    chunk_size: usize,
    grid: ManifestGrid,
    lods: Vec<ManifestLod>,
    bands: &'a [Band],
    routing: ManifestRouting,
}

#[derive(Serialize)]
struct ManifestGrid {
    origin_udeg: i64,
    world_side_udeg: i64,
}

#[derive(Serialize)]
struct ManifestSource<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot: Option<&'a str>,
    /// `[min_lat, min_lon, max_lat, max_lon]` µdeg — the OBCM header's own order.
    #[serde(skip_serializing_if = "Option::is_none")]
    coverage: Option<[i64; 4]>,
}

#[derive(Serialize)]
struct ManifestCell<'a> {
    id: String,
    band: &'a str,
    path: &'a str,
    bytes: u64,
    sha256: &'a str,
    partial: bool,
    pois: usize,
    nav_nodes: usize,
    nav_edges: usize,
    empty: bool,
}

#[derive(Serialize)]
struct Manifest<'a> {
    cutter: String,
    schema: ManifestSchema<'a>,
    sources: Vec<ManifestSource<'a>>,
    /// The ingested extract's content box, `[min_lat, min_lon, max_lat, max_lon]` µdeg.
    extract_bbox: [i64; 4],
    cells: Vec<ManifestCell<'a>>,
}

/// Write the run's provenance sidecar.
///
/// This is **not** the OBCC catalog — the bakery builds that. It is what a bakery needs and the
/// artifacts themselves cannot say: which band each cell belongs to (band membership is deliberately
/// absent from the bytes, §3.1), which sources and snapshots it was baked from, and whether its
/// square was fully covered (§3.7). It carries no wall clock, so two identical runs write identical
/// bytes.
fn write_manifest(
    out_dir: &Path,
    config: &Config,
    opts: &CutOptions,
    extract: UBox,
    summary: &CutSummary,
) -> Result<(), String> {
    let lod_band =
        |i: usize| opts.bands.bands.iter().find(|b| b.lods.contains(&i)).map(|b| b.id.clone()).unwrap_or_default();
    let manifest = Manifest {
        cutter: format!("obc-pack {}", env!("CARGO_PKG_VERSION")),
        schema: ManifestSchema {
            obcm_version: OBCM_VERSION,
            chunk_size: opts.chunk_size.unwrap_or(config.chunk_size),
            grid: ManifestGrid { origin_udeg: GRID_ORIGIN, world_side_udeg: crate::grid::WORLD_SIDE },
            lods: config
                .lods
                .iter()
                .enumerate()
                .map(|(i, l)| ManifestLod { index: i, max_mpp: l.max_mpp, band: lod_band(i) })
                .collect(),
            bands: &opts.bands.bands,
            routing: ManifestRouting { min_component_edges: config.routing.min_component_edges },
        },
        sources: opts
            .sources
            .iter()
            .map(|s| ManifestSource {
                id: &s.id,
                snapshot: s.snapshot.as_deref(),
                coverage: s.coverage.map(|c| [c.1, c.0, c.3, c.2]),
            })
            .collect(),
        extract_bbox: [extract.1, extract.0, extract.3, extract.2],
        cells: summary
            .cells
            .iter()
            .map(|c| ManifestCell {
                id: c.id.to_string(),
                band: &c.band,
                path: &c.path,
                bytes: c.bytes,
                sha256: &c.sha256,
                partial: c.partial,
                pois: c.pois,
                nav_nodes: c.nav_nodes,
                nav_edges: c.nav_edges,
                empty: c.empty,
            })
            .collect(),
    };
    let mut json = serde_json::to_string_pretty(&manifest).map_err(|e| format!("manifest: {e}"))?;
    json.push('\n');
    std::fs::create_dir_all(out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let path = out_dir.join(MANIFEST_NAME);
    std::fs::write(&path, json).map_err(|e| format!("write {}: {e}", path.display()))
}

/// The output path of a cell artifact, for a caller that has a [`CutSummary`] and wants the file.
pub fn artifact_path(out_dir: &Path, artifact: &CellArtifact) -> PathBuf {
    out_dir.join(&artifact.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::{on_grid_line, GRID_ORIGIN};

    const LOG2: u32 = 18;
    const S: i64 = 1 << LOG2;

    /// A coordinate on the `2^18` grid: cell (i, j)'s min corner plus offsets, as `(lon, lat)`.
    fn at(i: i64, j: i64, dlat: i64, dlon: i64) -> (i32, i32) {
        (((GRID_ORIGIN + j * S + dlon) as i32), ((GRID_ORIGIN + i * S + dlat) as i32))
    }

    /// A `(lon, lat)` point from absolute µdeg.
    fn pt(lat: i64, lon: i64) -> (i32, i32) {
        (lon as i32, lat as i32)
    }

    /// The lon line between cells `(_, 100)` and `(_, 101)` — every seam test's shared edge.
    const fn seam_lon() -> i64 {
        GRID_ORIGIN + 101 * S
    }

    /// A latitude inside row 100, well away from its own edges.
    const fn row_lat() -> i64 {
        GRID_ORIGIN + 100 * S + 5_000
    }

    #[test]
    fn lines_between_are_the_grid_lines() {
        let lo = GRID_ORIGIN + 5 * S;
        let got: Vec<i64> = lines_strictly_between(lo + 10, lo + 3 * S - 10, LOG2).collect();
        assert_eq!(got, vec![lo + S, lo + 2 * S]);
        // Endpoints exactly on lines are excluded — such a vertex IS the junction (§3.4(1)).
        let got: Vec<i64> = lines_strictly_between(lo, lo + S, LOG2).collect();
        assert!(got.is_empty(), "no line strictly between two adjacent lines");
        // Direction-independent.
        let a: Vec<i64> = lines_strictly_between(lo + 10, lo + 2 * S, LOG2).collect();
        let b: Vec<i64> = lines_strictly_between(lo + 2 * S, lo + 10, LOG2).collect();
        assert_eq!(a, b);
    }

    /// A segment crossing one seam gets exactly one boundary vertex, on the line, and the reversed
    /// segment gets the same one.
    #[test]
    fn one_crossing_one_boundary_vertex() {
        let a = pt(row_lat(), seam_lon() - 5_000);
        let b = pt(row_lat(), seam_lon() + 5_000); // same lat, crosses only the shared lon line
        let (cuts, ok) = segment_cuts(a, b, LOG2);
        assert!(ok);
        assert_eq!(cuts.len(), 1, "one line crossed ⇒ one junction");
        assert_eq!(cuts[0].0 as i64, seam_lon(), "on the line, exactly");
        assert!(on_grid_line(cuts[0].0 as i64, LOG2));
        let (rev, _) = segment_cuts(b, a, LOG2);
        assert_eq!(rev, cuts, "a reversed segment cuts at the identical coordinate");
    }

    /// A long diagonal crosses several lines of both axes; the cuts come out ordered along the
    /// segment, all on lines, and each in the right cell.
    #[test]
    fn multiple_crossings_are_ordered_along_the_segment() {
        let a = at(100, 100, 1_000, 1_000);
        let b = at(103, 102, 1_000, 1_000);
        let (cuts, ok) = segment_cuts(a, b, LOG2);
        assert!(ok, "converged");
        assert_eq!(cuts.len(), 3 + 2, "three lat lines + two lon lines");
        for c in &cuts {
            assert!(on_grid_boundary(c.1 as i64, c.0 as i64, LOG2), "every cut is on a grid line");
        }
        // Monotone in both axes (the segment is), so ordering along it is ordering per axis.
        assert!(cuts.windows(2).all(|w| w[0].0 <= w[1].0 && w[0].1 <= w[1].1), "ordered: {cuts:?}");
        let (rev, _) = segment_cuts(b, a, LOG2);
        let mut rev_sorted = rev;
        rev_sorted.sort();
        let mut fwd_sorted = cuts;
        fwd_sorted.sort();
        assert_eq!(rev_sorted, fwd_sorted, "direction changes the order, never the coordinates");
    }

    /// Segment ownership is the half-open rule read off a segment, including the collinear case
    /// (§3.4(3)): a segment lying **on** a line belongs to the cell above/east of it, once.
    #[test]
    fn segment_ownership_is_half_open() {
        let inside = (at(100, 100, 10, 10), at(100, 100, 20, 20));
        assert_eq!(segment_owner(inside.0, inside.1, LOG2), (100, 100));
        // Touching the min edge from inside.
        let on_min = (at(100, 100, 0, 10), at(100, 100, 50, 20));
        assert_eq!(segment_owner(on_min.0, on_min.1, LOG2), (100, 100));
        // Touching the max edge from inside ⇒ still this cell.
        let to_max = (at(100, 100, S - 50, 10), at(101, 100, 0, 20));
        assert_eq!(segment_owner(to_max.0, to_max.1, LOG2), (100, 100));
        // Wholly on the shared lon line ⇒ the cell for which it is a `min` edge.
        let along = (at(100, 101, 10, 0), at(100, 101, 20, 0));
        assert_eq!(segment_owner(along.0, along.1, LOG2), (100, 101));
        // Just past the line ⇒ the next cell.
        let past = (at(100, 101, 10, 1), at(100, 101, 20, 2));
        assert_eq!(segment_owner(past.0, past.1, LOG2), (100, 101));
        let before = (at(100, 100, 10, S - 2), at(100, 100, 20, S - 1));
        assert_eq!(segment_owner(before.0, before.1, LOG2), (100, 100));
    }

    fn way(nodes: &[(i64, (i32, i32))]) -> RoutableWay {
        RoutableWay {
            node_ids: nodes.iter().map(|(id, _)| *id).collect(),
            coords: nodes.iter().map(|(_, c)| *c).collect(),
            kind: 7,
        }
    }

    /// The seam property, at the level of one prepared way: both cells see a junction at the **same**
    /// coordinate on the shared edge, and each carries its own stub inward (§3.4(4)).
    #[test]
    fn neighbours_agree_on_the_boundary_junction() {
        // One short road running west→east across the line between cells (100, 100) and (100, 101).
        // Short on purpose: a way spanning a whole cell would be split by the §8.3 `i16` bound into
        // pieces, which is orthogonal to what this test is about.
        let seam = seam_lon();
        let w = way(&[(1, pt(row_lat(), seam - 5_000)), (2, pt(row_lat(), seam + 5_000))]);
        let prep = prepare_nav(&[w], LOG2, &Progress::silent()).expect("prepare");
        let west = CellId::new(LOG2, 100, 100).unwrap();
        let east = CellId::new(LOG2, 100, 101).unwrap();
        let gw = prep.cell_graph(west, 50);
        let ge = prep.cell_graph(east, 50);
        assert_eq!(gw.edges.len(), 1, "the western stub");
        assert_eq!(ge.edges.len(), 1, "the eastern stub");
        let on_seam = |g: &NavGraph| -> Vec<(i32, i32)> {
            g.nodes.iter().filter(|n| n.coord.0 as i64 == seam).map(|n| n.coord).collect()
        };
        let (a, b) = (on_seam(&gw), on_seam(&ge));
        assert_eq!(a.len(), 1, "exactly one boundary junction per side");
        assert_eq!(a, b, "and it is the SAME coordinate — this is what an assembler unifies");
        // Each side's stub really does reach the seam.
        assert_eq!(gw.edges[0].polyline.last().map(|p| p.0 as i64), Some(seam));
        assert_eq!(ge.edges[0].polyline.first().map(|p| p.0 as i64), Some(seam));
    }

    /// A vertex that already sits exactly on the line is the junction — no interpolation, no extra
    /// node (§3.4(1)).
    #[test]
    fn a_vertex_on_the_line_is_the_junction() {
        let seam = seam_lon();
        let w = way(&[
            (1, pt(row_lat(), seam - 5_000)),
            (2, pt(row_lat(), seam)), // an OSM node sitting exactly on the edge line
            (3, pt(row_lat(), seam + 5_000)),
        ]);
        let prep = prepare_nav(&[w], LOG2, &Progress::silent()).expect("prepare");
        assert_eq!(prep.ways[0].coords.len(), 3, "nothing was inserted");
        assert!(matches!(prep.ways[0].keys[1], JunctionKey::Osm(2)), "the OSM node keeps its identity");
        let gw = prep.cell_graph(CellId::new(LOG2, 100, 100).unwrap(), 50);
        let ge = prep.cell_graph(CellId::new(LOG2, 100, 101).unwrap(), 50);
        for g in [&gw, &ge] {
            assert_eq!(g.edges.len(), 1);
            assert!(g.nodes.iter().any(|n| n.coord.0 as i64 == seam), "both sides carry the on-line junction");
        }
    }

    /// Island pruning is strictly interior (§3.5): a stub touching the cell edge survives however
    /// small, while an equally small component in the middle of the cell does not.
    #[test]
    fn pruning_spares_components_touching_the_edge() {
        // A boundary-crossing stub (one edge per side) and a tiny interior islet (one edge).
        let seam = seam_lon();
        let crossing = way(&[(1, pt(row_lat(), seam - 5_000)), (2, pt(row_lat(), seam + 5_000))]);
        let islet = way(&[(10, at(100, 100, 100_000, 100_000)), (11, at(100, 100, 100_500, 100_500))]);
        let prep = prepare_nav(&[crossing, islet], LOG2, &Progress::silent()).expect("prepare");
        let g = prep.cell_graph(CellId::new(LOG2, 100, 100).unwrap(), 50);
        assert!(g.nodes.iter().any(|n| n.coord.0 as i64 == seam), "the boundary stub survived pruning");
        assert!(
            !g.nodes.iter().any(|n| n.coord.1 as i64 == GRID_ORIGIN + 100 * S + 100_000),
            "the interior islet was pruned"
        );
    }

    /// Coverage is exact, and the union of two extracts can make a border cell canonical.
    #[test]
    fn partial_marking_needs_real_coverage() {
        let cell = CellId::new(LOG2, 100, 100).unwrap();
        let (min_lon, min_lat, max_lon, max_lat) = cell.square();
        let src = |cov: Option<UBox>| SourceExtent { id: "x".into(), snapshot: None, coverage: cov };
        assert!(!sources_cover(cell.square(), &[]), "no declared coverage ⇒ nothing is canonical");
        assert!(!sources_cover(cell.square(), &[src(None)]));
        assert!(sources_cover(cell.square(), &[src(Some((min_lon, min_lat, max_lon, max_lat)))]), "exact fit covers");
        assert!(sources_cover(cell.square(), &[src(Some((min_lon - 1, min_lat - 1, max_lon + 1, max_lat + 1)))]));
        // One microdegree short on one edge is `partial`, not "close enough".
        assert!(!sources_cover(cell.square(), &[src(Some((min_lon, min_lat, max_lon - 1, max_lat)))]));
        // Two co-baked halves cover it together.
        let mid = (min_lon + max_lon) / 2;
        assert!(sources_cover(
            cell.square(),
            &[src(Some((min_lon, min_lat, mid, max_lat))), src(Some((mid, min_lat, max_lon, max_lat)))]
        ));
        // …but not if they leave a gap.
        assert!(!sources_cover(
            cell.square(),
            &[src(Some((min_lon, min_lat, mid - 10, max_lat))), src(Some((mid, min_lat, max_lon, max_lat)))]
        ));
    }

    #[test]
    fn source_spec_parsing() {
        let s = SourceExtent::parse("europe/switzerland@2026-07-01=7.0,45.5,10.5,48.0").expect("parse");
        assert_eq!(s.id, "europe/switzerland");
        assert_eq!(s.snapshot.as_deref(), Some("2026-07-01"));
        assert_eq!(s.coverage, Some((7_000_000, 45_500_000, 10_500_000, 48_000_000)));
        let bare = SourceExtent::parse("planet").expect("parse");
        assert_eq!((bare.snapshot, bare.coverage), (None, None));
        assert!(SourceExtent::parse("x=1,2,3").is_err(), "a malformed box is an error, not a silent None");
        assert!(SourceExtent::parse("@2026-01-01").is_err());
    }

    #[test]
    fn cell_paths_are_band_keyed_and_padded() {
        let band = BandTable::recommended();
        let fine = band.band("fine").unwrap();
        let network = band.band("network").unwrap();
        let c = CellId::new(18, 7, 9).unwrap();
        assert_eq!(cell_path(fine, &c), "cells/fine/0007/0009.obcm");
        // The two `2^18` bands must not collide — which `cells/<log2>/…` would.
        assert_ne!(cell_path(fine, &c), cell_path(network, &c));
    }
}
