//! The **terrain** stage: the curated coverage in, published OBCT cells and a catalog terrain block
//! out ([`OBCC_Spec.md` §13](../../../specs/OBCC_Spec.md), epic #1068 EL3).
//!
//! ```text
//! regions.toml ──▶ .poly ──▶ coverage ──▶ terrain cell set
//!                                              │
//!    source GeoTIFFs ──▶ obc-dem::bake_cell ───┤
//!                                              ▼
//!                 cells/terrain/<i>/<j>.obcd + sidecar
//!                 cells/terrain/.known-empty.json   (all-NODATA ocean)
//!                 terrain.json                      (dataset, pairing, revision)
//!                 regions/<a>/…/region.json         (its `terrain` selection)
//! ```
//!
//! # Why this is a separate stage and not a step of the cell bake
//!
//! Because terrain is on its own revision track, and a stage that ran inside the cell bake would
//! make that untrue in the only way that matters: a schema bump would re-enter the terrain code
//! path, and sooner or later something in it would re-derive a key from a schema fact. The two
//! stages share the tree and nothing else — this module reads no `schema.json`, no band table and
//! no schema revision, and nothing in [`crate::cells`] reads `terrain.json` except the *one*
//! recorded coupling ([`in_tree`]) that §13.4 makes explicit.
//!
//! It is also operationally right. A terrain bake needs a directory of DEM tiles and no OSM extract
//! at all; a DACH terrain bake is minutes over data that changes on a years cadence, against hours
//! over data that changes nightly. Running them together would tie the cheap one to the expensive
//! one's schedule.
//!
//! # What "unchanged" means here
//!
//! One key, hashed over everything that can move a terrain byte: the recipe version, the dataset id
//! and version, the posting/cell pairing, the terrain revision, and the cutter's own description.
//! **No OBCM version, no schema revision, no band table** — their absence from this expression is
//! the independence claim, and [`terrain_key_ignores_the_obcm_store`] is the test that keeps it
//! true.
//!
//! [`terrain_key_ignores_the_obcm_store`]: tests::terrain_key_ignores_the_obcm_store

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use obc_pack::grid::{id_width, CellId};
use obc_pack::progress::Progress;
use serde::{Deserialize, Serialize};

use crate::coverage::Coverage;
use crate::regions::Region;
use crate::source::ExtractSource;
use crate::util::write_json;

/// Bumped when a change in this stage alters published terrain bytes for unchanged inputs.
pub const TERRAIN_RECIPE_VERSION: u32 = 1;

/// The tree's terrain declaration, beside `schema.json` and deliberately not inside it.
pub const TERRAIN_DOC: &str = "terrain.json";
/// The reserved directory under `cells/` (`OBCC_Spec.md` §13.1).
pub const TERRAIN_DIR: &str = "terrain";
const TERRAIN_EXT: &str = ".obcd";
const TERRAIN_SIDECAR_EXT: &str = ".obcd.json";
const KNOWN_EMPTY_STATE: &str = ".known-empty.json";
const REGIONS_DIR: &str = "regions";
const REGION_DOC: &str = "region.json";
const REGION_POLY: &str = "boundary.poly";

/// `terrain.json`: what the raster is, at what resolution, at which revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerrainDoc {
    pub dataset_id: String,
    pub dataset_version: String,
    pub posting_log2: u8,
    pub cell_log2: u8,
    /// The terrain store's own revision. Never a schema revision.
    pub revision: u32,
    /// The source licence's required credit, verbatim (`OBCC_Spec.md` §13.5).
    pub attribution: String,
}

/// The per-cell sidecar the catalog generator reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainCellSidecar {
    terrain_revision: u32,
    dataset_version: String,
    built_at: String,
}

/// The local, unpublished record of what was baked and from what key.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TerrainCellState {
    terrain_key: String,
    sha256: String,
    bytes: u64,
    sidecar: TerrainCellSidecar,
}

/// One published all-`NODATA` row run, as the catalog publishes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerrainEmptyRun {
    pub start: String,
    pub end: String,
    pub built_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainKnownEmptyState {
    terrain_revision: u32,
    known_empty: Vec<TerrainEmptyRun>,
}

/// What the OBCM cell bake needs to know about the terrain already in the tree — the whole of
/// §13.4's coupling, in one struct, so it is one thing to find rather than a fact spread through
/// [`crate::cells`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerrainInput {
    /// The directory `obc-pack --terrain` is pointed at.
    pub dir: PathBuf,
    /// The revision the resulting cells record having sampled.
    pub revision: u32,
}

/// The terrain a bake tree already holds, if any.
///
/// This is the *only* direction the coupling runs: the cell bake reads the terrain in the tree and
/// records which revision it sampled. Nothing in the terrain stage reads anything the cell bake
/// wrote.
pub fn in_tree(out: &Path) -> Result<Option<TerrainInput>, String> {
    let path = out.join(TERRAIN_DOC);
    if !path.is_file() {
        return Ok(None);
    }
    let doc = read_terrain_doc(&path)?;
    let dir = out.join("cells").join(TERRAIN_DIR);
    if !dir.is_dir() {
        return Err(format!(
            "{}: declares terrain revision {} but {} does not exist — run `obc-bake terrain` before baking cells \
             against it",
            path.display(),
            doc.revision,
            dir.display()
        ));
    }
    Ok(Some(TerrainInput { dir, revision: doc.revision }))
}

pub fn read_terrain_doc(path: &Path) -> Result<TerrainDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: TerrainDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    validate_doc(&doc).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(doc)
}

fn validate_doc(doc: &TerrainDoc) -> Result<(), String> {
    if doc.revision == 0 {
        return Err("`revision` starts at 1 — a terrain store has no revision zero".into());
    }
    if doc.dataset_id.trim().is_empty() || doc.dataset_version.trim().is_empty() {
        return Err("`dataset_id` and `dataset_version` must be non-empty — they are half the lockstep key".into());
    }
    if doc.attribution.trim().is_empty() {
        return Err("`attribution` must be non-empty — the credit travels with the data (OBCC_Spec.md §13.5)".into());
    }
    obc_formats::obct::cell_samples_log2(doc.posting_log2, doc.cell_log2).ok_or_else(|| {
        format!(
            "posting 2^{} µdeg with cell 2^{} µdeg is not a pairing OBCT permits (OBCT_Spec.md §1.3)",
            doc.posting_log2, doc.cell_log2
        )
    })?;
    Ok(())
}

/// What actually rasterises one cell.
///
/// A trait for the same reason [`crate::cells::CellCutter`] is one: the stage's real content is the
/// selection, the skip key, the known-empty bookkeeping and the region wiring, and none of that
/// should need a multi-gigabyte GeoTIFF to test. The production implementation is
/// [`DemCutter`], which is `obc-dem` linked in.
///
/// Deliberately not `Sync`, unlike [`crate::cells::CellCutter`]: the DEM mosaic carries an interior
/// read cursor, and the bake is I/O-bound over a few hundred cells rather than CPU-bound over
/// millions of features. Sequential is fast enough here and one fewer thing to get wrong.
pub trait TerrainCutter {
    /// Identifies the rasterising recipe in the skip key.
    fn recipe(&self) -> String;
    /// One cell's OBCT block, or `None` when every sample is `NODATA` — which is not an error but a
    /// fact about the ground, and the thing the known-empty runs exist to publish.
    fn bake_cell(&self, ci: u32, cj: u32, posting_log2: u8, cell_log2: u8) -> Result<Option<Vec<u8>>, String>;
}

/// The real cutter: `obc-dem`'s own [`bake_cell`](obc_dem::bake::bake_cell) over a directory of
/// source DEM tiles.
///
/// Linked rather than shelled out, for the reason the cell bakery links the packer: the pure
/// function is right there, and a subprocess would put a flat `19_0600_0527.obcd` naming scheme
/// between this stage and the catalog's `<i>/<j>.obcd` layout for no gain.
pub struct DemCutter {
    sources: PathBuf,
    mosaic: obc_dem::geotiff::DemMosaic,
}

impl DemCutter {
    /// Open every GeoTIFF under `sources`. Eager, so a bad source directory fails before the first
    /// cell rather than in the middle of a run.
    pub fn open(sources: &Path) -> Result<DemCutter, String> {
        let mosaic = obc_dem::geotiff::DemMosaic::open_dir(sources)?;
        Ok(DemCutter { sources: sources.to_path_buf(), mosaic })
    }

    /// Source tiles opened.
    pub fn tiles(&self) -> usize {
        self.mosaic.len()
    }
}

impl TerrainCutter for DemCutter {
    fn recipe(&self) -> String {
        // The tile *set* is not in the recipe: the cells it produces are, through their digests,
        // and a source directory that grew a tile outside the coverage must not re-bake the world.
        format!("obc-dem bake tiles={} from={}", self.mosaic.len(), self.sources.display())
    }

    fn bake_cell(&self, ci: u32, cj: u32, posting_log2: u8, cell_log2: u8) -> Result<Option<Vec<u8>>, String> {
        Ok(obc_dem::bake::bake_cell(&self.mosaic, ci, cj, posting_log2, cell_log2))
    }
}

/// How a terrain run is scoped and where it writes.
#[derive(Debug, Clone)]
pub struct TerrainBakeOptions {
    pub out: PathBuf,
    pub doc: TerrainDoc,
    /// Re-bake even when the key says nothing changed.
    pub force: bool,
}

/// How one terrain cell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TerrainCellStatus {
    /// Rasterised and installed this run.
    Baked,
    /// Every sample is `NODATA`: no object, a known-empty run instead.
    Empty,
    /// The key matched and the artifact on disk still matches its recorded digest.
    Unchanged,
}

#[derive(Debug, Clone, Serialize)]
pub struct TerrainCellOutcome {
    pub id: String,
    pub bytes: u64,
    pub status: TerrainCellStatus,
}

/// Everything a terrain run did.
#[derive(Debug, Clone, Serialize)]
pub struct TerrainRunSummary {
    pub tree: PathBuf,
    pub recipe_version: u32,
    pub dataset_id: String,
    pub dataset_version: String,
    pub terrain_revision: u32,
    pub posting_log2: u8,
    pub cell_log2: u8,
    pub cells: Vec<TerrainCellOutcome>,
    /// Regions whose terrain selection this run wrote, with the cells each selects.
    pub regions: Vec<(String, usize)>,
    pub warnings: Vec<String>,
}

impl TerrainRunSummary {
    pub fn bytes(&self) -> u64 {
        self.cells.iter().map(|c| c.bytes).sum()
    }

    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "\n=== terrain bake summary ({}) ===", self.tree.display());
        let _ = writeln!(
            s,
            "{} {} — terrain revision {}, posting 2^{} µdeg, cell 2^{} µdeg, recipe v{}",
            self.dataset_id,
            self.dataset_version,
            self.terrain_revision,
            self.posting_log2,
            self.cell_log2,
            self.recipe_version
        );
        let count = |want: TerrainCellStatus| self.cells.iter().filter(|c| c.status == want).count();
        let _ = writeln!(
            s,
            "{} cell(s): {} baked, {} unchanged, {} all-NODATA (known-empty, no object), {}",
            self.cells.len(),
            count(TerrainCellStatus::Baked),
            count(TerrainCellStatus::Unchanged),
            count(TerrainCellStatus::Empty),
            crate::util::human_bytes(self.bytes())
        );
        for (id, cells) in &self.regions {
            let _ = writeln!(s, "  {id:<44} {cells:>6} terrain cell(s)");
        }
        for w in &self.warnings {
            let _ = writeln!(s, "\nwarning: {w}");
        }
        s
    }
}

/// A configured terrain run.
pub struct TerrainBakery<'a> {
    pub regions: &'a [Region],
    pub source: &'a dyn ExtractSource,
    pub cutter: &'a dyn TerrainCutter,
    pub opts: TerrainBakeOptions,
}

impl TerrainBakery<'_> {
    /// Bake every terrain cell the curated coverage touches, and wire the selections into the tree.
    pub fn run(&self, progress: &Progress) -> Result<TerrainRunSummary, String> {
        let doc = &self.opts.doc;
        validate_doc(doc)?;
        if self.regions.is_empty() {
            return Err("no regions — a terrain bake covers the curated coverage, so it needs one".into());
        }
        progress.log(format!(
            "terrain bakery: {} region(s), {} {} at terrain revision {}",
            self.regions.len(),
            doc.dataset_id,
            doc.dataset_version,
            doc.revision
        ));
        progress.log(format!("  tree:    {}", self.opts.out.display()));

        // One timestamp for the whole run, so the known-empty runs a single bake produces share
        // their provenance and therefore merge into the compact ranges §13.1 requires. Per-cell
        // timestamps would leave a catalog full of one-cell runs that differ only by a second.
        let built_at = obc_pack::catalog::now_timestamp();
        let key = self.terrain_key();

        let mut warnings = Vec::new();
        let mut selections: Vec<(String, String, BTreeSet<CellId>)> = Vec::new();
        for region in self.regions {
            let poly = match self.source.fetch_poly(region, progress) {
                Ok(poly) => poly,
                Err(e) => {
                    progress.warn(format!("  {}: {e}", region.id));
                    warnings.push(format!("{}: {e}", region.id));
                    continue;
                }
            };
            let coverage = Coverage::parse_poly(&poly).map_err(|e| format!("{}.poly: {e}", region.id))?;
            // The same intersect rule a band's cell set uses, applied to the terrain grid — one
            // implementation, so a terrain cell set cannot come out different from a band's over
            // the same outline.
            let cells = coverage.cells(u32::from(doc.cell_log2));
            progress.log(format!("  {}: {} terrain cell(s)", region.id, cells.len()));
            selections.push((region.id.clone(), poly, cells));
        }
        if selections.is_empty() {
            return Err("no region resolved to a coverage polygon — nothing to bake".into());
        }

        let wanted: BTreeSet<CellId> = selections.iter().flat_map(|(_, _, cells)| cells.iter().copied()).collect();
        progress.log(format!("\n--- {} distinct terrain cell(s) ---", wanted.len()));

        let mut empties = TerrainEmpties::load(&self.opts.out, doc.revision, u32::from(doc.cell_log2))?;
        let mut outcomes = Vec::new();
        for (index, cell) in wanted.iter().enumerate() {
            let outcome = self.bake_one(*cell, &key, &built_at, &mut empties)?;
            if index % 64 == 0 || outcome.status == TerrainCellStatus::Baked {
                progress.log(format!("  [{}/{}] {} {:?}", index + 1, wanted.len(), outcome.id, outcome.status));
            }
            outcomes.push(outcome);
        }
        empties.write(&self.opts.out, doc.revision)?;
        write_json(&self.opts.out.join(TERRAIN_DOC), doc)?;

        let mut regions = Vec::new();
        for (id, poly, cells) in &selections {
            let region = self.regions.iter().find(|r| &r.id == id).expect("selection came from the region list");
            write_region_terrain(&self.opts.out, region, poly, cells)?;
            regions.push((id.clone(), cells.len()));
        }

        Ok(TerrainRunSummary {
            tree: self.opts.out.clone(),
            recipe_version: TERRAIN_RECIPE_VERSION,
            dataset_id: doc.dataset_id.clone(),
            dataset_version: doc.dataset_version.clone(),
            terrain_revision: doc.revision,
            posting_log2: doc.posting_log2,
            cell_log2: doc.cell_log2,
            cells: outcomes,
            regions,
            warnings,
        })
    }

    /// Everything that can change a terrain cell's **bytes**, hashed into one key.
    ///
    /// The OBCM store is absent from this expression on purpose (module docs). So is the region
    /// list: a cell is a pure function of its own square and the source, so baking one more country
    /// must not re-bake the cells the previous run already published.
    fn terrain_key(&self) -> String {
        let doc = &self.opts.doc;
        crate::hash::text(&format!(
            "terrain-recipe={TERRAIN_RECIPE_VERSION}\nobct={}\ndataset={}:{}\nposting={}\ncell={}\nrevision={}\ncutter={}\n",
            obc_formats::obct::VERSION,
            doc.dataset_id,
            doc.dataset_version,
            doc.posting_log2,
            doc.cell_log2,
            doc.revision,
            self.cutter.recipe(),
        ))
    }

    fn bake_one(
        &self,
        cell: CellId,
        key: &str,
        built_at: &str,
        empties: &mut TerrainEmpties,
    ) -> Result<TerrainCellOutcome, String> {
        let doc = &self.opts.doc;
        let (artifact, sidecar_path, state_path) = paths(&self.opts.out, cell);
        let (ci, cj) = indices(cell)?;

        if !self.opts.force {
            if let Some(state) = read_current(&artifact, &sidecar_path, &state_path, key)? {
                return Ok(TerrainCellOutcome {
                    id: cell.to_string(),
                    bytes: state.bytes,
                    status: TerrainCellStatus::Unchanged,
                });
            }
            // An all-`NODATA` square is recorded too, or every re-run would re-rasterise the ocean.
            if empties.is_current(cell, key) && !artifact.is_file() {
                return Ok(TerrainCellOutcome { id: cell.to_string(), bytes: 0, status: TerrainCellStatus::Empty });
            }
        }

        let block = self.cutter.bake_cell(ci, cj, doc.posting_log2, doc.cell_log2)?;
        let Some(block) = block else {
            // No object at all: §13.1's known-empty run says the square is canonically void, which
            // is a different statement from "not published".
            for path in [&artifact, &sidecar_path, &state_path] {
                let _ = std::fs::remove_file(path);
            }
            empties.set(cell, Some(EmptyFact { built_at: built_at.to_string(), key: key.to_string() }));
            return Ok(TerrainCellOutcome { id: cell.to_string(), bytes: 0, status: TerrainCellStatus::Empty });
        };

        // Written through `obc-dem`'s own container writer — one writer for the published cell and
        // the assembled shard, which is what keeps them one format (`OBCT_Spec.md` §4.1).
        obc_dem::bake::write_cell_file(&artifact, doc.posting_log2, doc.cell_log2, ci, cj, &block)?;
        let (bytes, sha256) = crate::hash::file(&artifact)?;
        let sidecar = TerrainCellSidecar {
            terrain_revision: doc.revision,
            dataset_version: doc.dataset_version.clone(),
            built_at: built_at.to_string(),
        };
        write_json(&sidecar_path, &sidecar)?;
        write_json(&state_path, &TerrainCellState { terrain_key: key.to_string(), sha256, bytes, sidecar })?;
        empties.set(cell, None);
        Ok(TerrainCellOutcome { id: cell.to_string(), bytes, status: TerrainCellStatus::Baked })
    }
}

/// `cells/terrain/<i>/<j>.obcd`, its sidecar, and its local state file.
fn paths(out: &Path, cell: CellId) -> (PathBuf, PathBuf, PathBuf) {
    let width = id_width(cell.log2);
    let dir = out.join("cells").join(TERRAIN_DIR).join(format!("{:0width$}", cell.i));
    let stem = format!("{:0width$}", cell.j);
    (
        dir.join(format!("{stem}{TERRAIN_EXT}")),
        dir.join(format!("{stem}{TERRAIN_SIDECAR_EXT}")),
        dir.join(format!(".{stem}.terrain.json")),
    )
}

/// A grid cell's `(ci, cj)` as OBCT addresses them: unsigned, and never negative because a cell id
/// is an index into the world box.
fn indices(cell: CellId) -> Result<(u32, u32), String> {
    let to_u32 = |v: i64| u32::try_from(v).map_err(|_| format!("terrain cell `{cell}` is off the world grid"));
    Ok((to_u32(cell.i)?, to_u32(cell.j)?))
}

fn read_current(
    artifact: &Path,
    sidecar: &Path,
    state_path: &Path,
    key: &str,
) -> Result<Option<TerrainCellState>, String> {
    let Ok(text) = std::fs::read_to_string(state_path) else { return Ok(None) };
    let Ok(state) = serde_json::from_str::<TerrainCellState>(&text) else { return Ok(None) };
    if state.terrain_key != key || !artifact.is_file() || !sidecar.is_file() {
        return Ok(None);
    }
    let (bytes, sha256) = crate::hash::file(artifact)?;
    Ok((bytes == state.bytes && sha256 == state.sha256).then_some(state))
}

/// The provenance an all-`NODATA` square carries locally: what the catalog publishes (`built_at`)
/// plus the key that established it, so a re-run does not re-rasterise the ocean.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct EmptyFact {
    built_at: String,
    key: String,
}

/// The known-empty set, kept per cell locally and published as compact row runs.
///
/// Per cell rather than as runs, because every operation here is per cell — a square becomes empty
/// or stops being empty — and re-deriving the runs at write time is both simpler and the only way
/// to guarantee the merged, sorted, non-overlapping shape §13.1 requires.
struct TerrainEmpties {
    /// `(i, j) → fact`, and the cell size they are on.
    cells: BTreeMap<(i64, i64), EmptyFact>,
    log2: u32,
}

/// The local state file: the published runs plus the per-cell keys, which are not published.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainEmptyState {
    terrain_revision: u32,
    cell_log2: u32,
    /// `<cell id>` → the fact that established it.
    cells: BTreeMap<String, EmptyFact>,
}

impl TerrainEmpties {
    /// Load the state, or start empty. A state written at a different revision **or a different
    /// cell size** is discarded rather than carried: emptiness was established against a particular
    /// raster on a particular lattice, and re-establishing it is a bake, not a copy.
    fn load(out: &Path, revision: u32, log2: u32) -> Result<TerrainEmpties, String> {
        let fresh = TerrainEmpties { cells: BTreeMap::new(), log2 };
        let path = out.join("cells").join(TERRAIN_DIR).join(format!(".state{KNOWN_EMPTY_STATE}"));
        if !path.is_file() {
            return Ok(fresh);
        }
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let state: TerrainEmptyState = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if state.terrain_revision != revision || state.cell_log2 != log2 {
            return Ok(fresh);
        }
        let mut cells = BTreeMap::new();
        for (id, fact) in state.cells {
            let cell = CellId::parse(&id).map_err(|e| format!("{}: {e}", path.display()))?;
            cells.insert((cell.i, cell.j), fact);
        }
        Ok(TerrainEmpties { cells, log2 })
    }

    fn is_current(&self, cell: CellId, key: &str) -> bool {
        self.log2 == cell.log2 && self.cells.get(&(cell.i, cell.j)).is_some_and(|fact| fact.key == key)
    }

    fn set(&mut self, cell: CellId, fact: Option<EmptyFact>) {
        debug_assert_eq!(self.log2, cell.log2, "one terrain grid per run");
        match fact {
            Some(fact) => {
                self.cells.insert((cell.i, cell.j), fact);
            }
            None => {
                self.cells.remove(&(cell.i, cell.j));
            }
        }
    }

    /// Write both halves: the local per-cell state, and the published compact runs.
    fn write(&self, out: &Path, revision: u32) -> Result<(), String> {
        let dir = out.join("cells").join(TERRAIN_DIR);
        let log2 = self.log2;
        let state = TerrainEmptyState {
            terrain_revision: revision,
            cell_log2: log2,
            cells: self
                .cells
                .iter()
                .map(|(&(i, j), fact)| Ok((CellId::new(log2, i, j)?.to_string(), fact.clone())))
                .collect::<Result<_, String>>()?,
        };
        write_json(&dir.join(format!(".state{KNOWN_EMPTY_STATE}")), &state)?;
        write_json(
            &dir.join(KNOWN_EMPTY_STATE),
            &TerrainKnownEmptyState { terrain_revision: revision, known_empty: self.runs()? },
        )
    }

    /// The per-cell set as sorted, non-overlapping, maximally merged inclusive row runs.
    fn runs(&self) -> Result<Vec<TerrainEmptyRun>, String> {
        let mut out: Vec<TerrainEmptyRun> = Vec::new();
        let mut open: Option<(i64, i64, i64, &EmptyFact)> = None;
        for (&(i, j), fact) in &self.cells {
            match open {
                // Adjacent in the same row *and* the same provenance: one run. Different
                // provenance breaks the run, which is what makes a merged run's `built_at` true of
                // every cell in it.
                Some((row, start, end, prev)) if row == i && end + 1 == j && prev == fact => {
                    open = Some((row, start, j, fact));
                }
                Some((row, start, end, prev)) => {
                    out.push(run(self.log2, row, start, end, prev)?);
                    open = Some((i, j, j, fact));
                }
                None => open = Some((i, j, j, fact)),
            }
        }
        if let Some((row, start, end, fact)) = open {
            out.push(run(self.log2, row, start, end, fact)?);
        }
        Ok(out)
    }
}

fn run(log2: u32, i: i64, j0: i64, j1: i64, fact: &EmptyFact) -> Result<TerrainEmptyRun, String> {
    Ok(TerrainEmptyRun {
        start: CellId::new(log2, i, j0)?.to_string(),
        end: CellId::new(log2, i, j1)?.to_string(),
        built_at: fact.built_at.clone(),
    })
}

/// Merge a region's terrain selection into its `region.json`, leaving its band cell lists alone.
///
/// Read-modify-write rather than rewrite, because the two stages own different keys of one
/// document: [`crate::cells`] owns `cells`, this owns `terrain`, and a stage that rewrote the whole
/// file would silently drop the other's work whenever they ran in the wrong order.
fn write_region_terrain(out: &Path, region: &Region, poly: &str, cells: &BTreeSet<CellId>) -> Result<(), String> {
    let dir = region.segments().iter().fold(out.join(REGIONS_DIR), |path, segment| path.join(segment));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(REGION_DOC);

    let mut doc: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => serde_json::json!({ "name": region.name, "cells": {} }),
    };
    let object =
        doc.as_object_mut().ok_or_else(|| format!("{}: a region document is a JSON object", path.display()))?;
    let ids: Vec<String> = cells.iter().map(CellId::to_string).collect();
    object.insert("terrain".into(), serde_json::to_value(ids).map_err(|e| e.to_string())?);
    write_json(&path, &doc)?;

    // The outline the catalog reduces comes from this file; the cell bake writes it too, and the
    // two write identical bytes because it is the same download.
    let poly_path = dir.join(REGION_POLY);
    if std::fs::read_to_string(&poly_path).ok().as_deref() != Some(poly) {
        std::fs::write(&poly_path, poly).map_err(|e| format!("{}: {e}", poly_path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> TerrainDoc {
        TerrainDoc {
            dataset_id: "copernicus-glo-30".into(),
            dataset_version: "2021-1".into(),
            posting_log2: 9,
            cell_log2: 19,
            revision: 1,
            attribution: obc_dem::COPERNICUS_ATTRIBUTION.into(),
        }
    }

    struct NoCutter;
    impl TerrainCutter for NoCutter {
        fn recipe(&self) -> String {
            "test".into()
        }
        fn bake_cell(&self, _: u32, _: u32, _: u8, _: u8) -> Result<Option<Vec<u8>>, String> {
            Ok(None)
        }
    }

    fn key_for(doc: TerrainDoc) -> String {
        let regions: Vec<Region> = Vec::new();
        TerrainBakery {
            regions: &regions,
            source: &crate::source::GeofabrikExtracts::new(
                crate::source::GeofabrikExtracts::DEFAULT_BASE_URL,
                Path::new("/nonexistent"),
            ),
            cutter: &NoCutter,
            opts: TerrainBakeOptions { out: PathBuf::from("/nonexistent"), doc, force: false },
        }
        .terrain_key()
    }

    /// The headline property of the whole issue, at the level where it is cheapest to check: the
    /// terrain skip key is a function of terrain facts only. If an OBCM or schema fact ever leaks
    /// into it, a schema bump starts re-baking the raster — which is the failure #1071 exists to
    /// prevent — and no integration test would say why.
    #[test]
    fn terrain_key_ignores_the_obcm_store() {
        let base = key_for(doc());
        // Every terrain fact moves it…
        for changed in [
            TerrainDoc { revision: 2, ..doc() },
            TerrainDoc { dataset_version: "2022-1".into(), ..doc() },
            TerrainDoc { posting_log2: 10, ..doc() },
            TerrainDoc { cell_log2: 20, ..doc() },
            TerrainDoc { dataset_id: "other-dem".into(), ..doc() },
        ] {
            assert_ne!(key_for(changed), base);
        }
        // …and the expression names nothing from the other track. Asserted on the text the key is
        // hashed over, because that is where a leak would appear.
        let text = format!(
            "terrain-recipe={TERRAIN_RECIPE_VERSION}\nobct={}\ndataset={}:{}\nposting={}\ncell={}\nrevision={}\ncutter=test\n",
            obc_formats::obct::VERSION,
            doc().dataset_id,
            doc().dataset_version,
            doc().posting_log2,
            doc().cell_log2,
            doc().revision,
        );
        assert_eq!(crate::hash::text(&text), base);
        for forbidden in ["obcm", "schema", "band"] {
            assert!(!text.contains(forbidden), "`{forbidden}` must not be in the terrain key: {text}");
        }
    }

    /// The published runs are the compact, merged shape `OBCC_Spec.md` §13.1 requires — and a
    /// break in provenance really does break the run, or a merged run's `built_at` would be a
    /// claim about cells it was never true of.
    #[test]
    fn empty_cells_publish_as_merged_row_runs() {
        let fact = |t: &str| EmptyFact { built_at: t.into(), key: "k".into() };
        let mut empties = TerrainEmpties { cells: BTreeMap::new(), log2: 19 };
        for j in [10, 11, 12, 14] {
            empties.set(CellId::new(19, 600, j).unwrap(), Some(fact("2026-08-02T00:00:00Z")));
        }
        empties.set(CellId::new(19, 601, 10).unwrap(), Some(fact("2026-08-02T00:00:00Z")));
        // Same row, adjacent to the 10..12 run, but a different bake.
        empties.set(CellId::new(19, 600, 13).unwrap(), Some(fact("2026-08-01T00:00:00Z")));

        let runs = empties.runs().unwrap();
        assert_eq!(
            runs.iter().map(|r| (r.start.as_str(), r.end.as_str())).collect::<Vec<_>>(),
            [
                ("19/0600/0010", "19/0600/0012"),
                ("19/0600/0013", "19/0600/0013"),
                ("19/0600/0014", "19/0600/0014"),
                ("19/0601/0010", "19/0601/0010")
            ]
        );
        // Removing the middle splits the run rather than losing it.
        empties.set(CellId::new(19, 600, 11).unwrap(), None);
        let runs = empties.runs().unwrap();
        assert_eq!(runs[0].start, "19/0600/0010");
        assert_eq!(runs[0].end, "19/0600/0010");
        assert_eq!(runs[1].start, "19/0600/0012");
    }

    #[test]
    fn a_terrain_document_states_a_pairing_obct_permits() {
        assert!(validate_doc(&doc()).is_ok());
        assert!(validate_doc(&TerrainDoc { revision: 0, ..doc() }).is_err());
        assert!(validate_doc(&TerrainDoc { attribution: "  ".into(), ..doc() }).is_err());
        // A cell smaller than one tile is not a pairing OBCT permits.
        assert!(validate_doc(&TerrainDoc { posting_log2: 9, cell_log2: 12, ..doc() }).is_err());
    }
}
