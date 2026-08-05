//! The **cell** bake: named regions in, an [`OBCA`](../../../specs/OBCA_Spec.md) cell
//! store plus an [`OBCC`](../../../specs/OBCC_Spec.md) `schema_version 2` catalog out.
//!
//! `regions.toml` stays the curation surface — a region is still one reviewable line —
//! but it no longer names an artifact. It names a **selection**: the set of grid cells
//! its coverage polygon touches, per band. Two regions that share ground share the
//! same cells and the store pays for them once, which is the epic's headline saving.
//!
//! ```text
//! regions.toml ──▶ .poly ──▶ coverage ──▶ per-band cell sets
//!                    │                         │
//!                    │                    group by source set
//!                    ▼                         ▼
//!             .osm.pbf extracts ──────▶ cut plans ──▶ obc-pack's cutter
//!                                                         │
//!                       cells/<band>/<i>/<j>.obcm + sidecar ◀┘
//!                       regions/<a>/…/{region.json, boundary.poly}
//!                       schema.json, skins/<id>.json
//!                                     │
//!                                     ▼   obc-pack catalog
//!                            catalog.json + satellites
//! ```
//!
//! # The ownership rule, and why a border cell needs one
//!
//! A cell on the German/Swiss border baked from the German extract alone is missing
//! every Swiss side road, and — measured in the epic — only about half of each side's
//! junctions exist in the other's file. Publishing that as canonical coverage is
//! exactly what D3 forbids ([`OBCA_Spec.md` §3.7](../../../specs/OBCA_Spec.md)). So a
//! multi-region bake does not cut each region separately and hope; it computes, for
//! every cell, its **source set**:
//!
//! > A cell's source set is every co-baked extract whose **coverage polygon**
//! > intersects the cell's square.
//!
//! and then cuts each cell exactly once, from an ingest of exactly that source set.
//! `obc-bake bake europe/germany europe/switzerland` therefore runs three cut
//! plans — Germany-only cells, Switzerland-only cells, and the border cells both
//! touch, cut from **both** extracts together so the seam is complete in one file.
//!
//! Three properties make that rule safe rather than merely plausible:
//!
//! - **It is a pure function of (cell, the run's extracts).** No ordering, no
//!   first-writer-wins, no dependence on which region the operator listed first — which
//!   is what `OBCA_Spec.md` §3.2's determinism requirement demands of a tie-break. The
//!   plans themselves are keyed by the sorted source set and executed in that order.
//! - **It never writes a cell twice.** One (band, i, j) is one path, and exactly one
//!   plan owns it, so the "canonical *and* partial for the same id" state
//!   `OBCC_Spec.md` §8 forbids cannot arise from a single run.
//! - **It cannot silently downgrade.** Across runs it can: baking Switzerland alone
//!   after a DE+CH bake would re-cut a border cell from one source. [`install_cell`]
//!   refuses that — a canonical cell on disk is never replaced by a partial one — which
//!   is §3.7's "MUST replace a partial cell when a covering source becomes available"
//!   read in the direction that has teeth.
//!
//! `partial` itself is decided here rather than in the cutter, because the cutter's
//! coverage input is a *box* and a country is not a box: Germany's bounding box
//! contains a slab of Czechia, and a box test would publish those cells as canonical.
//! [`crate::coverage`] does it against the real polygons, and the union of a border
//! cell's whole source set — which is how co-baking makes a cell canonical.
//!
//! # What "unchanged" means
//!
//! The skip state uses two keys, for different reasons, at cell granularity.
//! The **pack key** hashes everything that can move a byte — cell and crop recipe
//! versions, OBCM version, schema config and revision, the band table, the cutter's
//! own description, the plan's crop, and the sorted `(extract id, extract SHA-256)`
//! pairs of the source set. Content hashes, never mtimes. The **sidecar facts** — the
//! sources' snapshot dates — are compared separately, so a mirror re-publishing
//! byte-identical extracts under a new date rewrites four-line JSON files and re-cuts
//! nothing.
//!
//! A plan whose every cell is current is skipped **before its extracts are ingested**,
//! which is the property that makes a re-run of a DACH bake minutes rather than hours.
//!
//! # The crop, and the one determinism caveat
//!
//! Every plan crops its extracts to its supercell square widened by
//! [`CROP_MARGIN_UDEG`] — the relation-complete
//! `--bbox` crop, which keeps every way touching the area, every renderable area
//! relation reached by those ways, and therefore every junction and polygon inside
//! it. The crop is part of the pack key, so it is part of the cell's recipe rather
//! than an invisible variable. A relation can still be incomplete at a Geofabrik
//! extract's own edge; those genuinely missing members remain an input limitation,
//! not something the crop guesses around.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use obc_pack::catalog::CellSource;
use obc_pack::cut::{CellArtifact, CutOptions, CutSummary, SourceExtent};
use obc_pack::grid::{BandTable, CellId};
use obc_pack::ingest::Bbox;
use obc_pack::progress::Progress;
use serde::Serialize;

use crate::cell_store::{
    paths as cell_paths, read_current as read_current_cell, CellSidecar, CellState, ARTIFACT_EXT as CELL_EXT,
    SIDECAR_EXT as CELL_SIDECAR_EXT,
};
use crate::coverage::Coverage;
use crate::known_empty::KnownEmptyIndex;
use crate::presets::StyleDoc;
use crate::regions::Region;
use crate::source::{Extract, ExtractSource};
use crate::util::{human_bytes, write_json};

/// Bumped when a cutter change alters cell bytes for unchanged inputs, forcing a
/// re-cut that content hashing alone would not.
pub const CELL_RECIPE_VERSION: u32 = 1;

/// Bumped only when the in-process bbox selection changes. Planet leaves do not crop
/// in the cutter, so folding this into [`CELL_RECIPE_VERSION`] would force a re-cut
/// whose bytes cannot change.
const CROP_RECIPE_VERSION: u32 = 2;

/// How far past its supercell square a plan crops its extracts, µdeg
/// (0.25° ≈ 28 km at DACH latitudes).
///
/// Comfortably past Geofabrik's complete-ways overhang and past any single way, so
/// the crop cannot change what is a junction inside the plan's cells: a way touching
/// a node in the cell area has that node inside the box, and relation-complete
/// selection keeps both the whole way and every renderable polygon it reaches.
pub const CROP_MARGIN_UDEG: i64 = 250_000;

const CELLS_DIR: &str = "cells";
const REGIONS_DIR: &str = "regions";
const SKINS_DIR: &str = "skins";
const SCHEMA_DOC: &str = "schema.json";
const REGION_DOC: &str = "region.json";
const REGION_POLY: &str = "boundary.poly";

/// What actually cuts. A trait so the tests can drive plan building, the ownership
/// rule, installation, the D3 guard and the skip logic without libGEOS and without a
/// multi-gigabyte extract — and so a test can inject the artifact this design most
/// needs to prove it rejects: a corrupt cell.
pub trait CellCutter: Sync {
    /// Identifies the cut recipe in the pack key.
    fn recipe(&self) -> String;
    /// Cut `pbfs` into `out_dir` per `opts`.
    fn cut(
        &self,
        pbfs: &[String],
        config: &obc_pack::config::Config,
        out_dir: &Path,
        opts: &CutOptions,
        progress: &Progress,
    ) -> Result<CutSummary, String>;
}

/// The real thing: `obc_pack::cut::cut`, linked in rather than spawned, for the same
/// reasons the bakery links the pipeline directly.
pub struct ObcCutter {
    /// Skip land generation (a ~950 MB dataset a real bake wants and no test does).
    pub no_land: bool,
    pub chunk_size: Option<usize>,
}

impl CellCutter for ObcCutter {
    fn recipe(&self) -> String {
        format!("obc-pack cut no_land={} chunk_size={:?}", self.no_land, self.chunk_size)
    }

    fn cut(
        &self,
        pbfs: &[String],
        config: &obc_pack::config::Config,
        out_dir: &Path,
        opts: &CutOptions,
        progress: &Progress,
    ) -> Result<CutSummary, String> {
        let mut opts = opts.clone();
        opts.no_land = self.no_land;
        opts.chunk_size = self.chunk_size;
        match obc_pack::cut::cut(pbfs, config, out_dir, &opts, progress) {
            Ok(s) => Ok(s),
            Err(obc_pack::PackError::Failed(e)) => Err(e),
            Err(obc_pack::PackError::Cancelled) => Err("cancelled".into()),
        }
    }
}

/// How a cell run is scoped and where it writes.
#[derive(Clone, Debug)]
pub struct CellBakeOptions {
    /// The cell tree root (`cells/`, `regions/`, `skins/`, `schema.json` live under it).
    pub out: PathBuf,
    /// Re-cut even when the key says nothing changed.
    pub force: bool,
    /// Stop at the first failed plan instead of cutting the rest and reporting at the end.
    pub fail_fast: bool,
    /// The schema's band table (`OBCA_Spec.md` §1.2) — schema data, published in the
    /// catalog and read back by every consumer.
    pub bands: BandTable,
    /// The schema's stable id, e.g. `bikepacking`.
    pub schema_id: String,
    /// The schema revision every cell is baked at. A bump invalidates the whole store.
    pub schema_revision: u32,
    /// The terrain already in the tree, which the nav graph's `Ascent M` is integrated
    /// from (`OBCM_Spec.md` §8.3, epic #1068 EL5) — and the **one** direction the two
    /// revision tracks are coupled in (`OBCC_Spec.md` §13.4). `None` bakes ascents of
    /// zero, which is exactly what a v11 map had and is a decode-valid v12 map.
    ///
    /// It is a terrain *input* to the cell bake and never the reverse: nothing in
    /// [`crate::terrain`] reads a schema fact, so a schema bump re-cuts cells and
    /// leaves every terrain object alone.
    pub terrain: Option<crate::terrain::TerrainInput>,
}

/// How one cell ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    /// Cut, verified, and installed this run.
    Cut,
    /// Inputs unchanged and the artifact on disk still matches its recorded digest.
    Unchanged,
    /// The bytes were current; only the sidecar's snapshot dates needed rewriting.
    SidecarRefreshed,
    /// A canonical cell on disk was **not** replaced by a partial re-cut (D3).
    KeptCanonical,
}

/// One cell's outcome.
#[derive(Debug, Clone, Serialize)]
pub struct CellOutcome {
    pub id: String,
    pub band: String,
    pub bytes: u64,
    pub partial: bool,
    pub status: CellStatus,
}

/// One cut plan's outcome.
#[derive(Debug, Clone, Serialize)]
pub struct PlanOutcome {
    /// The plan's source set, sorted — the cells' provenance and its own identity.
    pub sources: Vec<String>,
    pub cells_planned: usize,
    pub cells: Vec<CellOutcome>,
    pub seconds: f64,
    /// Loud. Any cell already on disk from an earlier run is untouched.
    pub error: Option<String>,
}

/// Everything a cell run did, and everything it did not.
#[derive(Debug, Clone, Serialize)]
pub struct CellRunSummary {
    pub tree: PathBuf,
    pub obcm_version: u8,
    pub recipe_version: u32,
    pub schema_id: String,
    pub schema_revision: u32,
    pub plans: Vec<PlanOutcome>,
    /// Per band: cells and bytes — the numerator of the density re-pin.
    pub bands: Vec<BandStats>,
    /// The ground this run's sources cover, km² — the denominator, and the one number
    /// that makes the table comparable with `OBCA_Spec.md` §1.5's.
    pub covered_km2: f64,
    /// Per region, the same denominator on its own.
    pub regions: Vec<RegionCoverage>,
    /// Regions the run was asked to cover that ended without a complete cell set.
    pub uncovered_regions: Vec<String>,
    pub warnings: Vec<String>,
}

/// One band's published footprint — the numerator of `OBCA_Spec.md` §1.5's density.
#[derive(Debug, Clone, Serialize)]
pub struct BandStats {
    pub band: String,
    pub cell_log2: u32,
    pub cells: usize,
    pub partial_cells: usize,
    pub bytes: u64,
}

impl BandStats {
    /// MiB per 1000 km² of **covered ground** — the latitude-free unit
    /// `OBCA_Spec.md` §1.5 tabulates, over the same denominator its whole-extract
    /// bakes used.
    ///
    /// Deliberately *not* per canonical cell square. A partial cell's bytes are the
    /// bytes of the part of its square the sources actually cover, so dividing the
    /// band's whole byte count by the sources' own area counts each byte once against
    /// the ground it describes. Dividing by canonical squares instead would throw
    /// away every edge cell — and at a `2^20` coarse cell (≈ 9 100 km² at 47°N) a
    /// region the size of Freiburg-Regierungsbezirk has **no** canonical cell at all,
    /// so that estimator has no coarse band to measure.
    pub fn mib_per_1000km2(&self, covered_km2: f64) -> f64 {
        if covered_km2 <= 0.0 {
            return 0.0;
        }
        (self.bytes as f64 / (1024.0 * 1024.0)) / (covered_km2 / 1000.0)
    }
}

/// One region's covered ground, so a per-region density can be computed from the
/// catalog's own per-region byte counts.
#[derive(Debug, Clone, Serialize)]
pub struct RegionCoverage {
    pub id: String,
    pub km2: f64,
}

impl CellRunSummary {
    pub fn failures(&self) -> Vec<&PlanOutcome> {
        self.plans.iter().filter(|p| p.error.is_some()).collect()
    }

    pub fn ok(&self) -> bool {
        self.failures().is_empty() && self.uncovered_regions.is_empty()
    }

    pub fn total_bytes(&self) -> u64 {
        self.bands.iter().map(|b| b.bytes).sum()
    }

    fn region_areas(&self) -> String {
        self.regions.iter().map(|r| format!("{} {:.0}", r.id, r.km2)).collect::<Vec<_>>().join(", ")
    }

    /// The run report, loud end first, density table included — the measurement
    /// `OBCA_Spec.md` §1.5 marked as owed by P2.
    pub fn render(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "\n=== cell bake summary ({}) ===", self.tree.display());
        let _ = writeln!(
            s,
            "OBCM v{}, recipe v{}, schema {} rev {}",
            self.obcm_version, self.recipe_version, self.schema_id, self.schema_revision
        );
        let (mut cut, mut unchanged, mut refreshed, mut kept) = (0, 0, 0, 0);
        for plan in &self.plans {
            for cell in &plan.cells {
                match cell.status {
                    CellStatus::Cut => cut += 1,
                    CellStatus::Unchanged => unchanged += 1,
                    CellStatus::SidecarRefreshed => refreshed += 1,
                    CellStatus::KeptCanonical => kept += 1,
                }
            }
            let _ = match &plan.error {
                None => writeln!(
                    s,
                    "  {:<48} {:>5} cell(s)  {:>7.1}s",
                    plan.sources.join(" + "),
                    plan.cells.len(),
                    plan.seconds
                ),
                Some(e) => writeln!(s, "  {:<48} FAILED: {e}", plan.sources.join(" + ")),
            };
        }
        let _ = writeln!(
            s,
            "\n{cut} cut, {refreshed} sidecar-only, {unchanged} unchanged, {kept} kept canonical, {} plan(s) failed",
            self.failures().len()
        );

        let _ = writeln!(s, "\ncovered ground: {:.0} km² ({})", self.covered_km2, self.region_areas());
        let _ = writeln!(s, "band       size    cells  partial        bytes   MiB/1000km²");
        for b in &self.bands {
            let _ = writeln!(
                s,
                "{:<10} 2^{:<3} {:>7}  {:>7}  {:>11}  {:>12.2}",
                b.band,
                b.cell_log2,
                b.cells,
                b.partial_cells,
                human_bytes(b.bytes),
                b.mib_per_1000km2(self.covered_km2)
            );
        }
        let whole: f64 = self.bands.iter().map(|b| b.mib_per_1000km2(self.covered_km2)).sum();
        let _ = writeln!(s, "{:<10} {:>32}  {:>12.2}", "whole map", human_bytes(self.total_bytes()), whole);

        for w in &self.warnings {
            let _ = writeln!(s, "\nwarning: {w}");
        }
        if !self.failures().is_empty() {
            let _ = writeln!(s, "\n!!! FAILED PLANS !!!");
            for plan in self.failures() {
                let _ = writeln!(s, "  {}: {}", plan.sources.join(" + "), plan.error.as_deref().unwrap_or(""));
            }
        }
        if !self.uncovered_regions.is_empty() {
            let _ = writeln!(
                s,
                "\n!!! REGIONS WITH AN INCOMPLETE CELL SET !!! (a curated region that does not ship reads to a user \
                 as \"not covered\")"
            );
            for region in &self.uncovered_regions {
                let _ = writeln!(s, "  {region}");
            }
        }
        s
    }
}

/// A region resolved to everything a cut needs.
struct Resolved {
    region: Region,
    extract: Extract,
    extract_sha: String,
    poly: String,
    coverage: Coverage,
    /// Per cell size: the cells this region selects.
    cells: BTreeMap<u32, BTreeSet<CellId>>,
}

/// One cut run: a source set and the cells only that source set can complete.
#[derive(Debug)]
struct Plan {
    /// Indices into the resolved regions, ascending — the plan's identity.
    sources: Vec<usize>,
    cells: BTreeSet<CellId>,
}

/// A configured cell run.
pub struct CellBakery<'a> {
    pub regions: &'a [Region],
    /// The packer config the cells are baked with — the schema, in `OBCC_Spec.md`
    /// §4's sense. Its style-id assignment is baked into every chunk.
    pub schema: &'a StyleDoc,
    /// The skins published beside it. Each must style exactly the schema's feature
    /// types with exactly the schema's ids; [`CellBakery::run`] proves that before it
    /// cuts anything, and the catalog generator proves it again on the finished tree.
    pub skins: &'a [&'a StyleDoc],
    pub source: &'a dyn ExtractSource,
    pub cutter: &'a dyn CellCutter,
    pub opts: CellBakeOptions,
}

impl CellBakery<'_> {
    /// Bake every named region's cells, then write the tree the catalog generator
    /// walks.
    pub fn run(&self, progress: &Progress) -> Result<CellRunSummary, String> {
        self.opts.bands.validate(self.schema.config.lods.len())?;
        if self.opts.schema_revision == 0 {
            return Err("--schema-revision starts at 1 — a cell store has no revision zero".into());
        }
        self.check_skins()?;
        check_schema_id(self.schema, &self.opts)?;
        progress.log(format!("cell bakery: {} region(s), schema `{}`", self.regions.len(), self.opts.schema_id));
        progress.log(format!("  source:  {}", self.source.describe()));
        progress.log(format!("  tree:    {}", self.opts.out.display()));

        let mut warnings = Vec::new();
        let mut uncovered = Vec::new();
        let resolved = self.resolve(progress, &mut uncovered, &mut warnings);
        if resolved.is_empty() {
            return Err("no region resolved to an extract and a coverage polygon — nothing to cut".into());
        }

        let plans = build_plans(&resolved, &self.opts.bands);
        progress.log(format!("  {} cut plan(s) over {} resolved region(s)", plans.len(), resolved.len()));
        let mut known_empty = KnownEmptyIndex::load(&self.opts.out, &self.opts.bands, self.opts.schema_revision)?;

        let mut outcomes = Vec::new();
        for plan in &plans {
            let started = Instant::now();
            let names: Vec<String> = plan.sources.iter().map(|&k| resolved[k].region.id.clone()).collect();
            progress.log(format!("\n--- {} ({} cells) ---", names.join(" + "), plan.cells.len()));
            let mut outcome = match self.run_plan(plan, &resolved, &mut known_empty, progress) {
                Ok(cells) => {
                    PlanOutcome { sources: names, cells_planned: plan.cells.len(), cells, seconds: 0.0, error: None }
                }
                Err(error) => {
                    progress.warn(format!("  FAILED: {error}"));
                    PlanOutcome {
                        sources: names,
                        cells_planned: plan.cells.len(),
                        cells: Vec::new(),
                        seconds: 0.0,
                        error: Some(error),
                    }
                }
            };
            outcome.seconds = started.elapsed().as_secs_f64();
            let failed = outcome.error.is_some();
            outcomes.push(outcome);
            if failed && self.opts.fail_fast {
                break;
            }
        }

        self.write_schema_and_skins()?;
        for r in &resolved {
            match self.write_region(r, progress)? {
                true => {}
                false => uncovered.push(r.region.id.clone()),
            }
        }

        let bands = self.measure(progress)?;
        // The density denominator: the union of the run's coverage polygons, so ground
        // two regions share is counted once, exactly as its cells are stored once.
        let all: Vec<&Coverage> = resolved.iter().map(|r| &r.coverage).collect();
        let covered_km2 = Coverage::union(&all).map(|c| c.area_km2()).unwrap_or(0.0);
        Ok(CellRunSummary {
            tree: self.opts.out.clone(),
            obcm_version: obc_formats::obcm::VERSION,
            recipe_version: CELL_RECIPE_VERSION,
            schema_id: self.opts.schema_id.clone(),
            schema_revision: self.opts.schema_revision,
            plans: outcomes,
            bands,
            covered_km2,
            regions: resolved
                .iter()
                .map(|r| RegionCoverage { id: r.region.id.clone(), km2: r.coverage.area_km2() })
                .collect(),
            uncovered_regions: uncovered,
            warnings,
        })
    }

    /// Every skin must fit the schema — checked **before** the first extract is
    /// fetched.
    ///
    /// The catalog generator makes the same check on the finished tree
    /// ([`obc_pack::catalog::check_skin`] is literally the same function), so this
    /// one buys nothing but the moment it fails at: a run that publishes a skin the
    /// generator will refuse has otherwise spent hours cutting cells first, and ends
    /// with a tree that has no catalog. A skin that does not fit is never a small
    /// mistake either — it means the document is a different *schema*, and the answer
    /// is a schema revision and a re-bake (epic #1016 D2), not a retry.
    fn check_skins(&self) -> Result<(), String> {
        for skin in self.skins {
            // Two different questions, and a document can fail either. First: is it
            // presentation only? That is a question about the *text*, because a config
            // cannot tell an absent `lods` from one restating the defaults. Second:
            // does it style exactly the schema's feature types, with exactly its ids?
            obc_pack::catalog::check_skin_document(&skin.json, &skin.path.display().to_string())?;
            obc_pack::catalog::check_skin(&self.schema.config, &skin.config).map_err(|e| {
                format!(
                    "skin `{}` ({}) is not a skin over schema `{}` ({}): {e}",
                    skin.id,
                    skin.path.display(),
                    self.opts.schema_id,
                    self.schema.path.display()
                )
            })?;
        }
        Ok(())
    }

    /// Fetch each region's extract and polygon, and derive its per-size cell sets.
    fn resolve(&self, progress: &Progress, uncovered: &mut Vec<String>, warnings: &mut Vec<String>) -> Vec<Resolved> {
        let sizes: BTreeSet<u32> = self.opts.bands.bands.iter().map(|b| b.cell_log2).collect();
        let mut out = Vec::new();
        for region in self.regions {
            progress.log(format!("\n--- resolving {} ({}) ---", region.id, region.name));
            let resolved = self
                .source
                .fetch(region, progress)
                .and_then(|extract| {
                    let (_, sha) = crate::hash::file(&extract.path)?;
                    Ok((extract, sha))
                })
                .and_then(|(extract, sha)| {
                    let poly = self.source.fetch_poly(region, progress)?;
                    let coverage = Coverage::parse_poly(&poly).map_err(|e| format!("{}.poly: {e}", region.id))?;
                    Ok((extract, sha, poly, coverage))
                });
            let (extract, extract_sha, poly, coverage) = match resolved {
                Ok(v) => v,
                Err(e) => {
                    progress.warn(format!("  {}: {e}", region.id));
                    warnings.push(format!("{}: {e}", region.id));
                    uncovered.push(region.id.clone());
                    continue;
                }
            };
            let cells: BTreeMap<u32, BTreeSet<CellId>> =
                sizes.iter().map(|&log2| (log2, coverage.cells(log2))).collect();
            let counts: Vec<String> = cells.iter().map(|(log2, set)| format!("2^{log2}: {}", set.len())).collect();
            progress.log(format!(
                "  extract {} ({}); cells — {}",
                human_bytes(extract.bytes),
                extract.snapshot,
                counts.join(", ")
            ));
            out.push(Resolved { region: region.clone(), extract, extract_sha, poly, coverage, cells });
        }
        out
    }

    /// Cut (or skip) one plan and install what it produced.
    fn run_plan(
        &self,
        plan: &Plan,
        resolved: &[Resolved],
        known_empty: &mut KnownEmptyIndex,
        progress: &Progress,
    ) -> Result<Vec<CellOutcome>, String> {
        let sources: Vec<&Resolved> = plan.sources.iter().map(|&k| &resolved[k]).collect();
        let coverage = Coverage::union(&sources.iter().map(|r| &r.coverage).collect::<Vec<_>>());
        // The edge set of the plan's combined coverage, once per cell size rather than
        // once per cell: it is one walk over every ring and thousands of cells ask.
        let boundaries: BTreeMap<u32, BTreeSet<CellId>> = match &coverage {
            None => BTreeMap::new(),
            Some(c) => self
                .opts
                .bands
                .bands
                .iter()
                .map(|b| b.cell_log2)
                .collect::<BTreeSet<u32>>()
                .into_iter()
                .map(|log2| (log2, c.boundary_cells(log2)))
                .collect(),
        };
        // Every plan crops — single-source plans included (without a crop, an interior plan
        // ingested its whole extract per bucket: the exact whole-country peak the span split
        // exists to prevent). The box is the plan's SUPERCELL square, not its cells' union:
        // the bucket is a pure function of each cell, so a cell's crop — and therefore its
        // pack key — cannot depend on which neighbours happened to be selected or stale.
        // Cropping to the cell union would re-cut a cell whenever the co-baked context
        // changed its plan's shape, which is exactly the incrementality the skip state
        // promises ("a re-run of a DACH bake is minutes"). A cell cut under the old
        // uncropped or union-cropped rule is stale by key and re-cuts once — no mixed
        // provenance.
        let crop = crop_box(&plan.cells)?;
        let pack_key = self.pack_key(&sources, crop.as_deref());

        // Which cells this plan still owes, and which only need a sidecar rewrite.
        let mut stale: Vec<CellId> = Vec::new();
        let mut done: Vec<CellOutcome> = Vec::new();
        for band in &self.opts.bands.bands {
            for cell in plan.cells.iter().filter(|c| c.log2 == band.cell_log2) {
                let want = self.sidecar_for(*cell, &sources, coverage.as_ref(), &boundaries);
                match self.reusable(*cell, &band.id, &pack_key) {
                    Some(state) if !self.opts.force => {
                        let outcome = self.refresh_sidecar(*cell, &band.id, state, &want)?;
                        done.push(outcome);
                    }
                    _ => stale.push(*cell),
                }
            }
        }
        stale.sort_unstable();
        stale.dedup();
        if stale.is_empty() {
            self.clear_known_empty(plan, known_empty)?;
            progress.log("    every cell current — not ingesting");
            return Ok(done);
        }

        let tmp = self.opts.out.join(format!(".cut-{}", pack_key.get(..16).unwrap_or("run")));
        let _ = std::fs::remove_dir_all(&tmp);
        let pbfs: Vec<String> = sources.iter().map(|r| r.extract.path.to_string_lossy().into_owned()).collect();
        let opts = CutOptions {
            bands: self.opts.bands.clone(),
            select: stale.clone(),
            only_bands: Vec::new(),
            // Provenance without a coverage box: a country is not a box, so the
            // canonical/`partial` decision is taken here against the real polygons
            // (see the module docs) and the cutter is told only who the sources are.
            sources: sources
                .iter()
                .map(|r| SourceExtent {
                    id: r.region.id.clone(),
                    snapshot: Some(r.extract.snapshot.clone()),
                    coverage: None,
                })
                .collect(),
            chunk_size: None,
            no_land: false,
            // The terrain already published in this tree, or nothing. A tree with no terrain bakes
            // `Ascent M = 0` throughout, which is a decode-valid v12 map and exactly what v11 was.
            terrain: self.opts.terrain.as_ref().map(|t| t.dir.clone()),
            bbox: crop.as_deref().map(Bbox::parse).transpose()?,
            source_extent: None,
        };
        let summary = self.cutter.cut(&pbfs, &self.schema.config, &tmp, &opts, progress).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&tmp);
        })?;
        let expected: BTreeSet<(String, CellId)> = self
            .opts
            .bands
            .bands
            .iter()
            .flat_map(|band| {
                stale.iter().filter(move |cell| cell.log2 == band.cell_log2).map(|cell| (band.id.clone(), *cell))
            })
            .collect();
        let produced: BTreeSet<(String, CellId)> =
            summary.cells.iter().map(|artifact| (artifact.band.clone(), artifact.id)).collect();
        if produced != expected || summary.cells.len() != expected.len() {
            let _ = std::fs::remove_dir_all(&tmp);
            return Err(format!(
                "cutter returned {} distinct cell-band output(s) for {} expected ({} raw); refusing a tree with \
                 silent holes or duplicates",
                produced.len(),
                expected.len(),
                summary.cells.len()
            ));
        }

        let mut installed = Vec::new();
        for artifact in &summary.cells {
            let sidecar = self.sidecar_for(artifact.id, &sources, coverage.as_ref(), &boundaries);
            installed.push(self.install_cell(&tmp, artifact, &sidecar, &pack_key, progress)?);
        }
        let _ = std::fs::remove_dir_all(&tmp);
        done.extend(installed);
        self.clear_known_empty(plan, known_empty)?;
        Ok(done)
    }

    fn clear_known_empty(&self, plan: &Plan, known_empty: &mut KnownEmptyIndex) -> Result<(), String> {
        let mut by_band: BTreeMap<String, BTreeSet<CellId>> = BTreeMap::new();
        for band in &self.opts.bands.bands {
            by_band.insert(
                band.id.clone(),
                plan.cells.iter().filter(|cell| cell.log2 == band.cell_log2).copied().collect(),
            );
        }
        known_empty.clear_cells(&by_band)?;
        known_empty.write_all(&self.opts.out, self.opts.schema_revision)
    }

    /// Everything that can change a cell's **bytes**, hashed into one key.
    ///
    /// Deliberately *not* in here: the extracts' snapshot dates, and the schema
    /// document's `_meta` block. Both are published facts that must not go stale, and
    /// neither can move a byte — see the module docs, [`sidecar_drift`], and
    /// [`crate::presets`] on why the key is the document's body rather than its file
    /// text. (The schema's `_meta.revision` and band table *are* in the key, but as
    /// the run's own `--schema-revision` and `--bands`, which is where they come from
    /// on this path.)
    fn pack_key(&self, sources: &[&Resolved], crop: Option<&str>) -> String {
        let ids: Vec<String> = sources
            .iter()
            .map(|r| format!("{}:{}", r.region.id, r.extract_sha))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let bands = serde_json::to_string(&self.opts.bands).unwrap_or_default();
        let crop_recipe = crop.map(|_| format!("crop-recipe={CROP_RECIPE_VERSION}\n")).unwrap_or_default();
        // The terrain revision is in the key because it moves bytes: the nav graph's per-edge
        // `Ascent M` is integrated from that raster. Only the revision, never the terrain files'
        // digests — the revision *is* the terrain store's content identity (`OBCC_Spec.md` §13.2),
        // and hashing thousands of 2 MiB rasters on every run to rediscover it would be absurd.
        let terrain = self.opts.terrain.as_ref().map_or("none".to_string(), |t| t.revision.to_string());
        crate::hash::text(&format!(
            "recipe={CELL_RECIPE_VERSION}\n{crop_recipe}obcm={}\ncutter={}\nschema={}\nrevision={}\nbands={bands}\nterrain={terrain}\ncrop={}\nsources={}\n",
            obc_formats::obcm::VERSION,
            self.cutter.recipe(),
            self.schema.body_sha256,
            self.opts.schema_revision,
            crop.unwrap_or("none"),
            ids.join(","),
        ))
    }

    /// The sidecar this run would write for a cell: who baked it, when they were
    /// published, and whether their combined coverage contains the whole square.
    fn sidecar_for(
        &self,
        cell: CellId,
        sources: &[&Resolved],
        coverage: Option<&Coverage>,
        boundaries: &BTreeMap<u32, BTreeSet<CellId>>,
    ) -> CellSidecar {
        let partial = match (coverage, boundaries.get(&cell.log2)) {
            // No coverage at all (a GEOS union failure) means nothing is demonstrably
            // covered, which is the conservative answer D3 asks for.
            (Some(c), Some(edge)) => !c.covers(cell, edge),
            _ => true,
        };
        let mut sources: Vec<CellSource> = sources
            .iter()
            .map(|r| CellSource { extract_id: r.region.id.clone(), snapshot: r.extract.snapshot.clone() })
            .collect();
        sources.sort();
        CellSidecar {
            schema_revision: self.opts.schema_revision,
            built_at: obc_pack::catalog::now_timestamp(),
            sources,
            partial,
            terrain_revision: self.opts.terrain.as_ref().map(|t| t.revision),
        }
    }

    /// The recorded state, when it still describes the cell on disk.
    ///
    /// Three things must agree, and the third is what catches rot: the pack key, the
    /// presence of both artifact and sidecar, and the artifact's *current* digest
    /// against the recorded one.
    fn reusable(&self, cell: CellId, band: &str, pack_key: &str) -> Option<CellState> {
        read_current_cell(&self.opts.out, band, cell, pack_key).ok().flatten()
    }

    /// A current cell whose published snapshot dates moved: rewrite four lines of
    /// JSON, carry `built_at` forward, re-cut nothing.
    fn refresh_sidecar(
        &self,
        cell: CellId,
        band: &str,
        state: CellState,
        want: &CellSidecar,
    ) -> Result<CellOutcome, String> {
        let outcome = |status| CellOutcome {
            id: cell.to_string(),
            band: band.to_string(),
            bytes: state.bytes,
            partial: state.sidecar.partial,
            status,
        };
        if !sidecar_drift(&state.sidecar, want) {
            return Ok(outcome(CellStatus::Unchanged));
        }
        let (_, sidecar_path, state_path) = cell_paths(&self.opts.out, band, cell);
        // `built_at` describes when the bytes were cut, and they were not re-cut.
        let refreshed =
            CellSidecar { built_at: state.sidecar.built_at.clone(), sources: want.sources.clone(), ..state.sidecar };
        write_json(&sidecar_path, &refreshed)?;
        write_json(&state_path, &CellState { sidecar: refreshed, ..state })?;
        Ok(outcome(CellStatus::SidecarRefreshed))
    }

    /// Verify a freshly cut cell and move it into the tree.
    ///
    /// The order is the contract: read the artifact
    /// back with the **real reader** first, refuse to downgrade a canonical cell
    /// second, write the sidecar third, and rename last — so nothing unverified and
    /// nothing less covered than what is already published ever exists under a name
    /// the catalog generator walks.
    fn install_cell(
        &self,
        tmp: &Path,
        artifact: &CellArtifact,
        sidecar: &CellSidecar,
        pack_key: &str,
        progress: &Progress,
    ) -> Result<CellOutcome, String> {
        let src = obc_pack::cut::artifact_path(tmp, artifact);
        let verified = crate::verify::verify_cell(&src, artifact.id.square())?;
        let (dest, sidecar_path, state_path) = cell_paths(&self.opts.out, &artifact.band, artifact.id);

        // D3: a covering bake already happened here. Publishing a thinner cell over it
        // would take coverage away silently, which is the failure `OBCA_Spec.md` §3.7
        // names — and unlike a partial that is merely stale, this one is a regression
        // the operator can fix by co-baking the neighbour again.
        if sidecar.partial {
            if let Ok(text) = std::fs::read_to_string(&state_path) {
                if let Ok(existing) = serde_json::from_str::<CellState>(&text) {
                    if !existing.sidecar.partial && dest.is_file() {
                        progress.warn(format!(
                            "  {} [{}]: kept the canonical cell — this run's sources ({}) do not cover its square, \
                             and a partial cell must never replace a covering bake (OBCA_Spec.md §3.7). Co-bake {} \
                             to refresh it.",
                            artifact.id,
                            artifact.band,
                            sidecar.sources.iter().map(|s| s.extract_id.as_str()).collect::<Vec<_>>().join(" + "),
                            existing
                                .sidecar
                                .sources
                                .iter()
                                .map(|s| s.extract_id.as_str())
                                .collect::<Vec<_>>()
                                .join(" + ")
                        ));
                        return Ok(CellOutcome {
                            id: artifact.id.to_string(),
                            band: artifact.band.clone(),
                            bytes: existing.bytes,
                            partial: false,
                            status: CellStatus::KeptCanonical,
                        });
                    }
                }
            }
        }

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        write_json(&sidecar_path, sidecar)?;
        // A sidecar with no cell fails generation just as loudly as a cell with no
        // sidecar (`OBCC_Spec.md` §8 rejects both), so a failed rename must not
        // leave one behind. Dropping the state file too makes the next run re-cut
        // rather than trust a record of bytes that never landed.
        if let Err(e) = std::fs::rename(&src, &dest) {
            let _ = std::fs::remove_file(&sidecar_path);
            let _ = std::fs::remove_file(&state_path);
            return Err(format!("{} -> {}: {e}", src.display(), dest.display()));
        }
        write_json(
            &state_path,
            &CellState {
                pack_key: pack_key.to_string(),
                sha256: artifact.sha256.clone(),
                bytes: artifact.bytes,
                sidecar: sidecar.clone(),
            },
        )?;
        debug_assert_eq!(verified.bbox.min_lat as i64, artifact.id.square().1);
        Ok(CellOutcome {
            id: artifact.id.to_string(),
            band: artifact.band.clone(),
            bytes: artifact.bytes,
            partial: sidecar.partial,
            status: CellStatus::Cut,
        })
    }

    /// Write `regions/<a>/…/{region.json, boundary.poly}`.
    ///
    /// The cell list is what the region **selects** and what exists: a cell the run
    /// failed to produce is dropped from the list and reported, rather than named in a
    /// document the generator would then refuse whole (`OBCC_Spec.md` §10). Returns
    /// whether the region's selection came out complete.
    fn write_region(&self, r: &Resolved, progress: &Progress) -> Result<bool, String> {
        let dir = r.region.segments().iter().fold(self.opts.out.join(REGIONS_DIR), |p, seg| p.join(seg));
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

        let mut cells: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut missing = 0usize;
        for band in &self.opts.bands.bands {
            let selected = r.cells.get(&band.cell_log2).map(BTreeSet::len).unwrap_or(0);
            let mut ids = Vec::new();
            for cell in r.cells.get(&band.cell_log2).into_iter().flatten() {
                if cell_paths(&self.opts.out, &band.id, *cell).0.is_file() {
                    ids.push(cell.to_string());
                }
            }
            missing += selected - ids.len();
            cells.insert(band.id.clone(), ids);
        }
        if missing > 0 {
            progress.warn(format!(
                "  {}: {missing} selected cell(s) are not in the tree — the region ships with holes",
                r.region.id
            ));
        }

        write_region_selection(&self.opts.out, &r.region, &r.poly, cells)?;
        Ok(missing == 0)
    }

    /// Write `schema.json` and `skins/<id>.json`.
    ///
    /// `schema.json` is the packer config the cells were cut with, with a `_meta`
    /// block carrying the schema's id, revision and band table — one document rather
    /// than two, so the style-id assignment the catalog publishes and the one baked
    /// into the chunks cannot disagree (`OBCC_Spec.md` §4).
    ///
    /// The id is **checked**, not written. `--schema-id` and the document's own
    /// `_meta.id` are two statements of one fact, and overwriting the document with
    /// the flag makes the disagreement unobservable: a typo in `--schema-id` would
    /// publish the bikepacking schema's cells under some other name, and a store's id
    /// is the identity a rider's already-downloaded cells are matched against. Both
    /// spellings exist because the flag scopes the run and the document ships in the
    /// repo, so the only safe relationship between them is agreement.
    fn write_schema_and_skins(&self) -> Result<(), String> {
        write_schema_and_skins(&self.opts.out, self.schema, self.skins, &self.opts)
    }

    /// Walk the finished tree and measure it: cells and bytes, per band.
    fn measure(&self, progress: &Progress) -> Result<Vec<BandStats>, String> {
        let mut out = Vec::new();
        for band in &self.opts.bands.bands {
            let dir = self.opts.out.join(CELLS_DIR).join(&band.id);
            let mut stats =
                BandStats { band: band.id.clone(), cell_log2: band.cell_log2, cells: 0, partial_cells: 0, bytes: 0 };
            for (_, path) in walk_cells(&dir, band.cell_log2)? {
                let bytes = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
                let sidecar: CellSidecar = match std::fs::read_to_string(path.with_file_name(sidecar_name(&path)))
                    .ok()
                    .and_then(|t| serde_json::from_str(&t).ok())
                {
                    Some(s) => s,
                    None => {
                        progress.warn(format!("{}: no readable sidecar — not counted", path.display()));
                        continue;
                    }
                };
                stats.cells += 1;
                stats.bytes += bytes;
                stats.partial_cells += usize::from(sidecar.partial);
            }
            out.push(stats);
        }
        Ok(out)
    }
}

/// Shared final tree metadata for curated and planet bakes.
pub(crate) fn write_schema_and_skins(
    out: &Path,
    schema: &StyleDoc,
    selected_skins: &[&StyleDoc],
    opts: &CellBakeOptions,
) -> Result<(), String> {
    check_schema_id(schema, opts)?;
    let mut doc: serde_json::Value =
        serde_json::from_str(&schema.json).map_err(|e| format!("{}: {e}", schema.path.display()))?;
    let meta = doc
        .get_mut("_meta")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| format!("{}: no `_meta` block", schema.path.display()))?;
    meta.insert("revision".into(), serde_json::json!(opts.schema_revision));
    meta.insert("bands".into(), serde_json::to_value(&opts.bands.bands).map_err(|e| e.to_string())?);
    write_json(&out.join(SCHEMA_DOC), &doc)?;

    let skins = out.join(SKINS_DIR);
    std::fs::create_dir_all(&skins).map_err(|e| format!("{}: {e}", skins.display()))?;
    for path in std::fs::read_dir(&skins).map_err(|e| format!("{}: {e}", skins.display()))? {
        let path = path.map_err(|e| format!("{}: {e}", skins.display()))?.path();
        let Some(stem) = path.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else {
            continue;
        };
        if !selected_skins.iter().any(|s| s.id == stem) {
            std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    for skin in selected_skins {
        let dest = skins.join(format!("{}.json", skin.id));
        if std::fs::read_to_string(&dest).ok().as_deref() == Some(skin.json.as_str()) {
            continue;
        }
        std::fs::write(&dest, &skin.json).map_err(|e| format!("{}: {e}", dest.display()))?;
    }
    Ok(())
}

pub(crate) fn check_schema_id(schema: &StyleDoc, opts: &CellBakeOptions) -> Result<(), String> {
    let doc: serde_json::Value =
        serde_json::from_str(&schema.json).map_err(|e| format!("{}: {e}", schema.path.display()))?;
    let declared =
        doc.get("_meta").and_then(|meta| meta.get("id")).and_then(serde_json::Value::as_str).unwrap_or_default();
    if declared != opts.schema_id {
        return Err(format!(
            "{}: `_meta.id` is `{declared}` but --schema-id says `{}`. The document's id and the run's are the \
             same fact stated twice — fix whichever is wrong rather than letting the bake publish this schema's \
             cells under another schema's name.",
            schema.path.display(),
            opts.schema_id
        ));
    }
    Ok(())
}

/// Write `regions/<a>/…/{region.json, boundary.poly}`, preserving the terrain selection.
///
/// Read-modify-write rather than a plain write, because two stages own different keys of one
/// document: this one owns `cells`, [`crate::terrain`] owns `terrain`. A stage that rewrote the
/// whole file would silently drop the other's work whenever they ran in the wrong order — and
/// "wrong order" here means the perfectly ordinary "bake terrain on Monday, refresh the map on
/// Tuesday", which is the entire point of separate revision tracks.
pub(crate) fn write_region_selection(
    out: &Path,
    region: &Region,
    poly: &str,
    cells: BTreeMap<String, Vec<String>>,
) -> Result<(), String> {
    let dir = region.segments().iter().fold(out.join(REGIONS_DIR), |path, segment| path.join(segment));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(REGION_DOC);
    let mut doc: serde_json::Value = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?,
        Err(_) => serde_json::json!({}),
    };
    let object =
        doc.as_object_mut().ok_or_else(|| format!("{}: a region document is a JSON object", path.display()))?;
    object.insert("name".into(), serde_json::Value::String(region.name.clone()));
    object.insert("cells".into(), serde_json::to_value(cells).map_err(|e| e.to_string())?);
    write_json(&path, &doc)?;
    std::fs::write(dir.join(REGION_POLY), poly).map_err(|e| format!("{}: {e}", dir.display()))
}

/// Whether the published facts drifted — the only thing that can change without the
/// bytes changing. Extract ids are part of the pack key, so only the dates can move.
fn sidecar_drift(have: &CellSidecar, want: &CellSidecar) -> bool {
    have.sources != want.sources
}

/// Group every selected cell by its **source set**: the co-baked extracts whose
/// coverage polygon touches its square.
///
/// The grouping is the ownership rule (module docs) and is a pure function of its
/// input — the key is the sorted index list, the output is ordered by it — so two runs
/// over the same regions produce the same plans in the same order.
///
/// A plan is additionally **split by locality** into [`RUN_SPAN_UDEG`] buckets. One source set
/// can own a country's whole interior, and the packer ingests and prepares a plan's entire crop
/// box before it writes a cell — whole-Germany in one plan is how the first DACH bake earned a
/// `Killed: 9` on a 16 GB machine. The split changes **nothing** about a cell's identity: its
/// source set (the bake key's `sources=`) is the pure function above, untouched; only *which
/// neighbours share its ingest* shrinks, and that co-ingest bbox has always been a plan-shaped
/// choice (`crop_box` is the union of the plan's own cells). Bucketing is by the cell square's
/// min corner in supercell units — `BTreeMap` all the way down, so the plan list stays a pure
/// deterministic function of the inputs.
fn build_plans(resolved: &[Resolved], bands: &BandTable) -> Vec<Plan> {
    let sizes: BTreeSet<u32> = bands.bands.iter().map(|b| b.cell_log2).collect();
    let mut owners: BTreeMap<CellId, Vec<usize>> = BTreeMap::new();
    for log2 in sizes {
        for (k, r) in resolved.iter().enumerate() {
            for cell in r.cells.get(&log2).into_iter().flatten() {
                owners.entry(*cell).or_default().push(k);
            }
        }
    }
    // (sorted source indices, supercell bucket) — one plan per distinct pair.
    type PlanKey = (Vec<usize>, (i64, i64));
    let mut grouped: BTreeMap<PlanKey, BTreeSet<CellId>> = BTreeMap::new();
    for (cell, mut sources) in owners {
        sources.sort_unstable();
        sources.dedup();
        let (min_lon, min_lat, _, _) = cell.square();
        let bucket = (min_lat.div_euclid(RUN_SPAN_UDEG), min_lon.div_euclid(RUN_SPAN_UDEG));
        grouped.entry((sources, bucket)).or_default().insert(cell);
    }
    grouped.into_iter().map(|((sources, _), cells)| Plan { sources, cells }).collect()
}

/// How much ground one cut run may span, µdeg: 2^21 ≈ 2.1°, roughly a Bundesland quarter — the
/// scale the packer has demonstrably cut in 16 GB. The trade is more runs, each re-reading its
/// extracts over a smaller `--bbox` crop; a bake is hours and re-reads are minutes, and a run
/// the machine survives beats one it does not.
const RUN_SPAN_UDEG: i64 = 1 << 21;

/// The `--bbox` a plan crops its extracts to: its [`RUN_SPAN_UDEG`] **supercell square**,
/// widened by [`CROP_MARGIN_UDEG`], as the `W,S,E,N` degrees spelling the packer parses.
///
/// The bucket square rather than the cells' union, deliberately: every cell lies wholly
/// inside its min-corner bucket (cell sizes are ≤ 2^20, grid-aligned, and the span is 2^21),
/// and the bucket is a pure function of the cell — so the crop a cell was cut under, which
/// is in its pack key, is identical in every bake that selects it. A union box would vary
/// with the plan's shape and re-cut cells whose own recipe never changed.
fn crop_box(cells: &BTreeSet<CellId>) -> Result<Option<String>, String> {
    let mut b: Option<(i64, i64, i64, i64)> = None;
    for cell in cells {
        let (cell_lon, cell_lat, _, _) = cell.square();
        let (blat, blon) = (cell_lat.div_euclid(RUN_SPAN_UDEG), cell_lon.div_euclid(RUN_SPAN_UDEG));
        let square =
            (blon * RUN_SPAN_UDEG, blat * RUN_SPAN_UDEG, (blon + 1) * RUN_SPAN_UDEG, (blat + 1) * RUN_SPAN_UDEG);
        b = Some(match b {
            None => square,
            // Straddling buckets cannot happen for a single plan (`build_plans` buckets by the
            // same function), but the box stays correct if a caller ever passes a mixed set.
            Some(v) => (v.0.min(square.0), v.1.min(square.1), v.2.max(square.2), v.3.max(square.3)),
        });
    }
    let Some((min_lon, min_lat, max_lon, max_lat)) = b else { return Ok(None) };
    // Clamped to the geographic domain, which the grid's world box is wider than: the
    // packer's `--bbox` parser is a geographic one and rejects anything outside ±180/±90.
    let deg = |v: i64, limit: f64| (v as f64 / 1e6).clamp(-limit, limit);
    Ok(Some(format!(
        "{:.6},{:.6},{:.6},{:.6}",
        deg(min_lon - CROP_MARGIN_UDEG, 180.0),
        deg(min_lat - CROP_MARGIN_UDEG, 90.0),
        deg(max_lon + CROP_MARGIN_UDEG, 180.0),
        deg(max_lat + CROP_MARGIN_UDEG, 90.0),
    )))
}

fn sidecar_name(artifact: &Path) -> String {
    let stem = artifact.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    format!("{}{CELL_SIDECAR_EXT}", stem.trim_end_matches(CELL_EXT))
}

/// Every `<i>/<j>.obcm` under a band directory, with its cell id.
fn walk_cells(dir: &Path, log2: u32) -> Result<Vec<(CellId, PathBuf)>, String> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    for i_dir in sorted_dir(dir)? {
        let Some(i_name) = i_dir.file_name().and_then(|n| n.to_str()) else { continue };
        if i_name.starts_with('.') || !i_dir.is_dir() {
            continue;
        }
        for path in sorted_dir(&i_dir)? {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with('.') || !name.ends_with(CELL_EXT) {
                continue;
            }
            let j_name = name.trim_end_matches(CELL_EXT);
            let cell =
                CellId::parse(&format!("{log2}/{i_name}/{j_name}")).map_err(|e| format!("{}: {e}", path.display()))?;
            out.push((cell, path));
        }
    }
    Ok(out)
}

fn sorted_dir(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("{}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    entries.sort();
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **The run-span split.** One source set can own a country's interior, and the packer
    /// ingests a plan's whole crop box before writing a cell — unbounded plans are how the DACH
    /// bake earned a `Killed: 9`. Two cells of one region far apart must land in different
    /// plans; two in the same [`RUN_SPAN_UDEG`] supercell must share one, and every plan keeps
    /// the cell's true source set.
    #[test]
    fn plans_split_by_locality_but_never_by_source_set() {
        let poly = "test\n1\n  5.0 45.0\n  18.0 45.0\n  18.0 56.0\n  5.0 56.0\n  5.0 45.0\nEND\nEND\n";
        let coverage = Coverage::parse_poly(poly).expect("a coverage");
        // Two neighbouring fine cells in one supercell, and one far away (≈ München vs Hamburg).
        let near_a = CellId::parse("18/183/35").unwrap();
        let near_b = CellId::parse("18/183/36").unwrap();
        let far = CellId::parse("18/204/38").unwrap();
        let resolved = vec![Resolved {
            region: Region { id: "europe/germany".into(), name: "Germany".into() },
            extract: Extract { path: "de.osm.pbf".into(), snapshot: "2026-08-01".into(), bytes: 1, downloaded: false },
            extract_sha: "0".repeat(64),
            poly: poly.to_string(),
            coverage,
            cells: BTreeMap::from([(18, BTreeSet::from([near_a, near_b, far]))]),
        }];
        let bands = BandTable::recommended();
        let plans = build_plans(&resolved, &bands);
        let with_cells: Vec<&Plan> = plans.iter().filter(|p| !p.cells.is_empty()).collect();
        assert_eq!(with_cells.len(), 2, "one supercell holds the neighbours, one the far cell");
        for plan in &with_cells {
            assert_eq!(plan.sources, vec![0], "locality must never change a cell's source set");
        }
        let sizes: BTreeSet<usize> = with_cells.iter().map(|p| p.cells.len()).collect();
        assert_eq!(sizes, BTreeSet::from([1, 2]), "{:?}", with_cells);
    }

    #[test]
    fn a_crop_box_wraps_the_plans_cells_with_a_margin() {
        let cell = CellId::parse("18/1204/1052").unwrap();
        let spec = crop_box(&BTreeSet::from([cell])).unwrap().expect("a box");
        let bbox = Bbox::parse(&spec).expect("the packer parses it");
        let (w, s, e, n) = bbox.to_degrees();
        let (min_lon, min_lat, max_lon, max_lat) = cell.square();
        let margin = CROP_MARGIN_UDEG as f64 / 1e6;
        assert!(w <= min_lon as f64 / 1e6 - margin + 1e-9 && e >= max_lon as f64 / 1e6 + margin - 1e-9);
        assert!(s <= min_lat as f64 / 1e6 - margin + 1e-9 && n >= max_lat as f64 / 1e6 + margin - 1e-9);
        assert_eq!(crop_box(&BTreeSet::new()).unwrap(), None);
    }
}
