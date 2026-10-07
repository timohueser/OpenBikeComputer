//! The cell cutter: one ingested extract in, the cell artifacts of every band it touches out
//! (`OBCA_Spec.md`).
//!
//! A cell artifact is an ordinary OBCM file a device could open on its own. What makes it a cell is
//! a set of constraints, and every one of them is here. The header bbox is the grid square and not
//! the content, because the alignment theorem needs the box to be the cell. The complete ladder is
//! written with out-of-band levels empty, so band membership never appears in the bytes. Geometry is
//! clipped at the exact cell edge and the per-LOD sub-pixel cull runs on the clipped geometry, so a
//! polygon may survive in one cell and be culled in its neighbour. The nav graph is cut with
//! deterministic boundary junctions on the edge line (see [`prepare_nav`]), island pruning touches
//! only strictly interior components, and an under-covered cell is marked `partial`.
//!
//! Two orderings are load-bearing, and both exist to make seams meet exactly rather than nearly.
//!
//! Simplify before clipping, always. A clip puts vertices exactly on the edge line, and both
//! neighbours clip the same simplified segment against the same line, so their pieces meet to the
//! microdegree. Simplifying afterwards would let each neighbour move or drop its own copy of a seam
//! vertex, and no tolerance could fix the crack.
//!
//! Merge fills and lines once, over the whole extract, before cutting. The union of a cluster of
//! parcels must be the same geometry in both neighbours or their clips would not meet, and GEOS
//! overlay is only guaranteed to agree when handed identical inputs.
//!
//! Work is organised band, then LOD, then cell, so a level's merged feature set is built once and
//! every cell of the band reads it, and only that band's levels are resident. Cells within a band
//! are cut in parallel; nothing in a cell's bytes depends on which thread produced it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::Serialize;
use sha2::{Digest, Sha256};

use obc_formats::obcm::VERSION as OBCM_VERSION;
use obc_map_core::cell::{cell_path, has_contours};

use crate::serialize::serialize_lods_streaming;
use obc_draw::ingest::Ingested;
use obc_draw::semantic::build_semantic_levels;
use obc_draw::serialize::Node;
use obc_elevation::{ElevationSource, NullElevation};
use obc_map_core::config::Config;
use obc_map_core::grid::{cells_intersecting, Band, BandTable, CellId, UBox, GRID_ORIGIN};
use obc_map_core::progress::{PackError, Phase, Progress};
use obc_map_core::serialize::validate_chunk_size;
use obc_map_core::terrain::TerrainSet;
use obc_network::cut::{bucket_pois, prepare_nav};
use obc_network::nav::NavGraph;
use obc_pbf::bbox::Bbox;
use obc_places::metadata::Poi;
use obc_places::routing::RoutableWay;

use obc_draw::cut::{merge_key, prepare_lod, LodSet, PreparedSemantic};

/// Filename of the cutter's provenance sidecar, written **last** (see [`cut_ingested`]).
pub const MANIFEST_NAME: &str = "cells.json";

/// One source extract a cell was baked from.
///
/// `coverage` is the extract's own coverage box, and the honest answer to "is this cell canonical?".
/// Without it a cell cannot be shown to be fully covered, so it is marked `partial`: presenting an
/// under-covered border cell as canonical coverage is the failure this prevents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceExtent {
    /// Extract identifier, e.g. `europe/switzerland`.
    pub id: String,
    /// The extract's snapshot date, as the bakery knows it (e.g. `2026-07-01`).
    pub snapshot: Option<String>,
    /// The ground this extract covers, µdeg, in [`UBox`] order. `None` means unknown, so nothing is
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
    /// The schema's band table. Cell sizes are schema data, never format constants.
    pub bands: BandTable,
    /// Cut exactly these cells rather than everything the extract touches. A cell id names a size,
    /// and two bands may share one, so a selection is cut for every band of that size unless
    /// [`CutOptions::only_bands`] narrows it.
    pub select: Vec<CellId>,
    /// Restrict the run to these band ids. Empty ⇒ every band in the table.
    pub only_bands: Vec<String>,
    /// The sources this run is baking from.
    pub sources: Vec<SourceExtent>,
    /// Override the config's `chunk_size`.
    pub chunk_size: Option<usize>,
    /// Skip land generation even when the config has a land style.
    pub no_land: bool,
    /// The `land-polygons-split-3857.zip` of the store. Absent, the store fetches the source
    /// `land-polygons` when a map needs land.
    pub land: Option<PathBuf>,
    /// Crop the sources to this box during ingest.
    pub bbox: Option<Bbox>,
    /// Baked OBCT terrain (a `.obcd` container or a directory of them) to integrate the
    /// per-direction `Ascent M` from. Absent means every adjacency entry gets `0`.
    ///
    /// Seam-safe by construction: the cutter slices edges exactly on cell-edge lines and a piece's
    /// ascent is integrated from the global OBCT lattice, never from anything cell-local, so
    /// re-cutting one cell alone reproduces the identical bytes.
    pub terrain: Option<PathBuf>,
    pub landmarks: Vec<PathBuf>,
    pub peaks: Vec<PathBuf>,
    /// Logical source extent used for land generation and the cut manifest.
    ///
    /// Ordinarily the ingest derives this from the retained features. Planet leaves state it
    /// explicitly: a featureless ocean shard still owns cells, and a quiet corner of a leaf still
    /// needs the global land layer considered.
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
            land: None,
            bbox: None,
            terrain: None,
            landmarks: Vec::new(),
            peaks: Vec::new(),
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
    /// The sources do not demonstrably cover the whole square.
    pub partial: bool,
    /// Features that exceeded `chunk_size` and were dropped — never expected, never silent.
    pub dropped: usize,
    pub pois: usize,
    pub nav_nodes: usize,
    pub nav_edges: usize,
    /// The serialized band carries no geometry, POIs, or navigation content. `dropped > 0` always
    /// makes this false: losing oversized source content is not proof that the cell is empty.
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
    /// Cells marked `partial`.
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
                // half-written tree is a catalog.
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
    // The ways, not a graph: the cutter builds one graph per cell (OBCA).
    let (mut ingested, ways) = obc_draw::ingest::ingest_osm_ways(pbfs, config, opts.bbox, progress)?;
    if ingested.features.is_empty() && ingested.coastlines.is_empty() && opts.source_extent.is_none() {
        return Err("no features found matching config".into());
    }
    progress.check()?;
    progress.stage(Phase::Bbox, "Calculating BBox...");
    let extract = opts.source_extent.unwrap_or_else(|| obc_draw::ingest::compute_bbox(&ingested));
    obc_draw::land::add_land(&mut ingested, config, extract, opts.no_land, opts.land.as_deref(), progress)?;
    progress.check()?;
    // Contours are generated once over the whole extract and then cut like any other feature, for
    // the same reason land is: a cell's geometry must not depend on which cell asked for it. This
    // opens the terrain set a second time, which costs a header read and a directory validation per
    // container, and only when a band of the run has contours: a run whose bands hold no contour
    // level traces none and has nothing to warn about.
    if selected_bands(opts).any(|band| has_contours(config, band)) {
        let contour_terrain = opts.terrain.as_deref().map(TerrainSet::open).transpose()?;
        obc_draw::contour::add_contours(&mut ingested, config, extract, contour_terrain.as_ref(), progress)?;
        progress.check()?;
    }
    cut_ingested(&ingested, &ways, config, out_dir, opts, progress)
}

/// Cut an already-ingested extract — the entry point tests and the bakery both drive.
///
/// `ways` are the routable ways of the source snapshot; they are what junction-ness is classified
/// from, so handing in a subset would quietly change the graph a cell writes.
///
/// Writes `<out_dir>/cells/<band>/<i>/<j>.obcm` plus the provenance sidecar
/// `<out_dir>/`[`MANIFEST_NAME`], written last, so an interrupted run publishes nothing.
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
    let extract = opts.source_extent.unwrap_or_else(|| obc_draw::ingest::compute_bbox(ing));
    let landmark_bounds = opts.bbox.map_or_else(
        || {
            opts.select
                .iter()
                .copied()
                .map(CellId::square)
                .reduce(|a, b| (a.0.min(b.0), a.1.min(b.1), a.2.max(b.2), a.3.max(b.3)))
                .unwrap_or(extract)
        },
        Bbox::microdegree_bounds,
    );
    let landmarks = obc_pack::landmark_map::load(&opts.landmarks, &ing.landmark_links, landmark_bounds)?;
    let peaks = obc_pack::peak_map::load(&opts.peaks)?;
    // Opened once for the whole run and shared by every cell: validating a hundred containers per
    // cell would dominate a cut. `sampler_for` is the per-cell part.
    let terrain_set = match &opts.terrain {
        Some(path) if selected_bands(opts).any(|band| reads_terrain(config, band)) => Some(TerrainSet::open(path)?),
        _ => None,
    };
    let styles = config.styles();
    let semantic_scheme = config.semantic_scheme();
    // Build the same finer-to-coarser semantic ladder as the monolithic packer before it is clipped
    // into canonical cells. Fine-only and network-only jobs skip this expensive pass, since neither
    // selected band can consume its output.
    let needs_semantic =
        selected_bands(opts).flat_map(|band| band.lods.iter()).any(|&lod| config.lods[lod].semantic_coverage);
    let semantic_levels = if needs_semantic {
        build_semantic_levels(&ing.features, &config.lods, &semantic_scheme, extract, progress)?
    } else {
        vec![None; config.lods.len()]
    };
    let mut artifacts: Vec<CellArtifact> = Vec::new();

    let bands: Vec<_> = selected_bands(opts)
        .map(|band| (band, select_cells(band, extract, &opts.select)))
        .filter(|(_, cells)| !cells.is_empty())
        .collect();
    let mut last_band = HashMap::new();
    for (index, (band, _)) in bands.iter().enumerate() {
        for &lod in &band.lods {
            last_band.insert(merge_key(ing, config, lod, &semantic_scheme), index);
        }
    }
    let mut merged_sets = HashMap::new();
    for (index, (band, cells)) in bands.into_iter().enumerate() {
        progress.stage(
            Phase::Quadtree,
            format!("Cutting band {} (2^{} µdeg): {} cell(s)...", band.id, band.cell_log2, cells.len()),
        );

        // Per-band preparation, done once and read by every cell of the band.
        let lod_sets: Vec<LodSet<'_>> = band
            .lods
            .iter()
            .map(|&l| {
                prepare_lod(
                    ing,
                    config,
                    l,
                    band.cell_log2,
                    PreparedSemantic { features: semantic_levels[l].as_deref(), scheme: &semantic_scheme },
                    &mut merged_sets,
                    progress,
                )
            })
            .collect();
        merged_sets.retain(|key, _| last_band[key] > index);
        let nav_cut = if band.has_nav() { Some(prepare_nav(ways, band.cell_log2, progress)?) } else { None };
        let band_terrain = terrain_set.as_ref().filter(|_| reads_terrain(config, band));
        let poi_cells = if band.has_poi() { bucket_pois(&ing.pois, band.cell_log2) } else { HashMap::new() };
        progress.check()?;

        let written: Vec<Result<CellArtifact, String>> = cells
            .par_iter()
            .map(|cell| {
                if progress.is_cancelled() {
                    return Err("cancelled".into());
                }
                let mut pois: Vec<Poi> = poi_cells
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
                // across rayon workers. A cell outside the supplied terrain gets an empty sampler,
                // which answers `None` everywhere.
                let mut sampler = match band_terrain {
                    None => None,
                    Some(set) => Some(set.sampler_for(Some(cell.square()))?),
                };
                let mut null = NullElevation;
                let terrain: &mut dyn ElevationSource = match &mut sampler {
                    Some(s) => s,
                    None => &mut null,
                };
                obc_places::metadata::fill_summit_elevations(&mut pois, terrain);
                let square = cell.square();
                let cell_landmarks: Vec<_> = landmarks
                    .iter()
                    .filter(|landmark| {
                        let (lon, lat) = (i64::from(landmark.record.lon), i64::from(landmark.record.lat));
                        band.has_poi() && lon >= square.0 && lon < square.2 && lat >= square.1 && lat < square.3
                    })
                    .cloned()
                    .collect();
                write_cell(
                    cell,
                    band,
                    out_dir,
                    config,
                    &styles,
                    chunk_size,
                    trees,
                    &pois,
                    &cell_landmarks,
                    &peaks.select(&pois),
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

/// The bands of the run: the table's, or those of [`CutOptions::only_bands`].
fn selected_bands(opts: &CutOptions) -> impl Iterator<Item = &Band> {
    opts.bands.bands.iter().filter(|band| opts.only_bands.is_empty() || opts.only_bands.contains(&band.id))
}

/// Whether the cells of `band` read terrain: for the contours of its levels, or for the ascent of
/// its nav graph and the heights of its summits.
pub fn reads_terrain(config: &Config, band: &Band) -> bool {
    has_contours(config, band) || band.has_nav() || band.has_poi()
}

/// The cells of one band this run must emit: the explicit selection filtered to the band's size, or
/// every cell of the band whose square intersects the extract.
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

// --- writing ----------------------------------------------------------------------------------

/// Serialize and write one cell artifact.
///
/// The header bbox is the cell square, the ladder is complete with out-of-band levels written empty,
/// and the POI and nav sections are present but empty unless the band carries them.
#[allow(clippy::too_many_arguments)]
fn write_cell(
    cell: &CellId,
    band: &Band,
    out_dir: &Path,
    config: &Config,
    styles: &[obc_map_core::serialize::Style],
    chunk_size: usize,
    trees: Vec<(usize, Node)>,
    pois: &[Poi],
    landmarks: &[obc_pack::landmark_map::Landmark],
    peaks: &obc_pack::peak_map::Peaks,
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
        landmarks,
        peaks,
        graph,
        &config.routing.profiles,
        terrain,
        |i| {
            // In band gives its tree; out of band gives an empty region, so band membership
            // never shows up in the bytes.
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
        empty: !had_geometry && pois.is_empty() && landmarks.is_empty() && graph.nodes.is_empty() && dropped == 0,
    })
}

fn node_has_features(node: &Node) -> bool {
    match node {
        Node::Leaf { features, .. } => !features.is_empty(),
        Node::Branch(children) => children.iter().any(node_has_features),
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let mut h = Sha256::new();
    h.update(&bytes);
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Whether the declared source coverage contains the whole square.
///
/// Coverage is the union of the sources' boxes, so two co-baked neighbours can make a border cell
/// canonical between them. The test is exact: the square is decomposed on the boxes' own coordinates
/// and every elementary piece must be inside some box, so a "mostly covered" square is `partial`.
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
/// This is not the OBCC catalog — the bakery builds that. It is what a bakery needs and the
/// artifacts themselves cannot say: which band each cell belongs to (band membership is deliberately
/// absent from the bytes), which sources and snapshots it was baked from, and whether its square was
/// fully covered. It carries no wall clock, so two identical runs write identical bytes.
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
            grid: ManifestGrid { origin_udeg: GRID_ORIGIN, world_side_udeg: obc_map_core::grid::WORLD_SIDE },
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
/// A cell artifact's path inside the run's output directory.
///
/// Keyed by band, not by `log2`: two bands may legitimately share a cell size (`fine` and `network`
/// are both `2^18` in the recommended table). Every cell's path is stated explicitly in the
/// manifest, so a publisher never has to infer it.
pub fn artifact_path(out_dir: &Path, artifact: &CellArtifact) -> PathBuf {
    out_dir.join(&artifact.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    const LOG2: u32 = 18;
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
    #[test]
    fn geometry_bands_share_source_merges() {
        use obc_draw::{geom::Geom, ingest::IngestFeature};
        use std::sync::Arc;
        let config = Config::parse(
            r#"{
            "lods":[{"simplify":10}, {"max_mpp":4,"simplify":1}, {"max_mpp":2,"simplify":0.5}],
            "merge_lines":true,
            "features":{"highway":{"residential":{"color":"0xffff"}}}
        }"#,
        )
        .unwrap();
        let line = |min_lod, lat| IngestFeature {
            style_id: 1,
            min_lod,
            geom: Geom::Line(vec![(7.59, lat), (7.602, lat + 0.00004), (7.62, lat)]),
        };
        let ing = Ingested {
            features: vec![
                line(0, 47.3),
                line(0, 47.31),
                IngestFeature { style_id: 1, min_lod: 2, geom: Geom::Line(vec![(7.63, 47.4), (7.64, 47.4)]) },
                IngestFeature { style_id: 1, min_lod: 2, geom: Geom::Line(vec![(7.59, 47.33), (7.602176, 47.33)]) },
            ],
            landmark_links: Vec::new(),
            coastlines: Vec::new(),
            pois: Vec::new(),
        };
        let bands = BandTable::parse(
            r#"{"bands":[
            {"id":"coarse","cell_log2":20,"lods":[0],"role":"coarse"},
            {"id":"fine","cell_log2":18,"lods":[1,2],"role":"geometry"},
            {"id":"network","cell_log2":18,"lods":[],"sections":["nav","poi"],"role":"core"}
        ]}"#,
        )
        .unwrap();
        let merges = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = Arc::clone(&merges);
        let progress = Progress::new(obc_map_core::progress::CancelToken::new(), move |_, line| {
            if line.contains("line fragment(s)") {
                count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        });
        let out = obcm_testkit::scratch::scratch_dir("obc-cut", "merge-memo");
        cut_ingested(&ing, &[], &config, &out, &CutOptions { bands, ..Default::default() }, &progress).unwrap();
        std::fs::remove_dir_all(out).unwrap();
        assert_eq!(
            merges.load(std::sync::atomic::Ordering::Relaxed),
            2,
            "the two input sets merge once each across both geometry bands"
        );
    }
}
