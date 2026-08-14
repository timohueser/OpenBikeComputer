//! Cell artifact, sidecar, provenance, and known-empty store scanning.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use obc_formats::obcm::VERSION as OBCM_VERSION;

use crate::grid::CellId;

use super::{
    content_addressed_rel_path, file_name, hash_file, inclusive_run_count, parse_strict_id, read_obcm_header,
    rel_url_path, sorted_entries, validate_date, validate_region_id, validate_timestamp, BandEntry, CellEntry,
    CellSource, KnownEmptyRun, PinnedArtifact, SchemaDoc, CELLS_DIR, CELL_INDEX_NAME, KNOWN_EMPTY_STATE_NAME,
    SCHEMA_DOC, TERRAIN_DIR,
};

pub(super) const CELL_EXT: &str = ".obcm";
pub(super) const CELL_SIDECAR_EXT: &str = ".obcm.json";

/// Every published cell, grouped by band, plus the one OBCM version they all agree on.
///
/// §8's "MUST NOT publish a canonical cell and a partial cell for the same `id` at
/// the same `schema_revision`" holds by construction here: a (band, i, j) is one path
/// in the tree, so a re-bake that finds a covering source overwrites the partial cell
/// rather than adding a second entry beside it.
pub(super) struct Cells {
    pub(super) by_band: BTreeMap<String, Vec<CellEntry>>,
    pub(super) known_empty_by_band: BTreeMap<String, Vec<KnownEmptyRun>>,
    pub(super) obcm_version: u8,
    /// The terrain revision every cell in the store was baked against, read from the
    /// sidecars the way `obcm_version` is read from the headers. `None` when the store
    /// was baked with no terrain at all; a store where some cells sampled terrain and
    /// others did not is refused rather than published (§13.4).
    pub(super) terrain_revision: Option<u32>,
    pub(super) pinned_artifacts: Vec<PinnedArtifact>,
}

/// Local, un-published state from which the generator builds one band's compact
/// known-empty ranges.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct KnownEmptyState {
    pub(super) schema_revision: u32,
    pub(super) band: String,
    pub(super) known_empty: Vec<KnownEmptyRun>,
}

/// Lookup used while validating region satellites. A present cell is either a
/// downloadable artifact or a verified-empty grid square; only the former has
/// bytes or can be partial.
pub(super) struct BandIndex<'a> {
    cells: BTreeMap<String, &'a CellEntry>,
    empty_by_row: BTreeMap<i64, Vec<(i64, i64)>>,
}

pub(super) enum IndexedCell<'a> {
    Artifact(&'a CellEntry),
    KnownEmpty,
}

impl<'a> BandIndex<'a> {
    pub(super) fn new(cells: &'a [CellEntry], known_empty: &[KnownEmptyRun]) -> Result<Self, String> {
        let mut empty_by_row: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
        for run in known_empty {
            let start = parse_strict_id(&run.start)?;
            let end = parse_strict_id(&run.end)?;
            empty_by_row.entry(start.i).or_default().push((start.j, end.j));
        }
        Ok(Self { cells: cells.iter().map(|cell| (cell.id.clone(), cell)).collect(), empty_by_row })
    }

    pub(super) fn get(&self, id: &str) -> Result<Option<IndexedCell<'_>>, String> {
        if let Some(cell) = self.cells.get(id) {
            return Ok(Some(IndexedCell::Artifact(cell)));
        }
        let cell = parse_strict_id(id)?;
        let Some(runs) = self.empty_by_row.get(&cell.i) else { return Ok(None) };
        let at = runs.partition_point(|(_, end)| *end < cell.j);
        Ok(runs.get(at).filter(|(start, end)| *start <= cell.j && cell.j <= *end).map(|_| IndexedCell::KnownEmpty))
    }
}

/// The facts a cell's bytes cannot state. Band is **not** among them: band membership
/// is a property of the schema revision, and a cell writes the full ladder with the
/// LODs outside its band empty, so a legitimately empty cell is indistinguishable
/// from an out-of-band one (`OBCA_Spec.md` §3.1). The tree's path says the band.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CellSidecar {
    /// The schema revision this cell was baked at. Recorded per cell so the generator
    /// can refuse a tree that mixes revisions (`OBCA_Spec.md` §6.3).
    schema_revision: u32,
    built_at: String,
    /// Every extract this cell was baked from, with that extract's snapshot date.
    sources: Vec<CellSource>,
    /// Whether the sources fully cover the cell's square (`OBCA_Spec.md` §3.7).
    partial: bool,
    /// The terrain revision the cell's nav ascents were integrated from (§13.4).
    /// Absent for a terrain-less bake, which is every cell before epic #1068.
    #[serde(default)]
    terrain_revision: Option<u32>,
}

pub(super) fn read_cells(tree: &Path, schema: &SchemaDoc, base_url: &str) -> Result<Cells, String> {
    let root = tree.join(CELLS_DIR);
    if !root.is_dir() {
        return Err(format!("{}: no `{CELLS_DIR}/` directory — is this a bake tree?", root.display()));
    }
    let bands: BTreeMap<&str, &BandEntry> = schema.bands.iter().map(|b| (b.id.as_str(), b)).collect();
    let mut by_band: BTreeMap<String, Vec<CellEntry>> = BTreeMap::new();
    let mut known_empty_by_band: BTreeMap<String, Vec<KnownEmptyRun>> = BTreeMap::new();
    let mut obcm_version = None;
    let mut terrain_revision: Option<Option<u32>> = None;
    let mut pinned_artifacts = Vec::new();

    for band_dir in sorted_entries(&root)? {
        let name = file_name(&band_dir)?;
        if name.starts_with('.') || name == TERRAIN_DIR {
            // `cells/terrain/` is the other artifact class, on its own revision track
            // and read by `read_terrain`. It is deliberately not a band (§13.1).
            continue;
        }
        if !band_dir.is_dir() {
            return Err(format!(
                "{}: `{CELLS_DIR}/` holds one directory per band; loose files do not belong here",
                band_dir.display()
            ));
        }
        let band = bands.get(name.as_str()).ok_or_else(|| {
            format!(
                "{}: `{name}` is not a band in `{SCHEMA_DOC}`'s band table. Cell paths are keyed by band because two \
                 bands may share a cell size.",
                band_dir.display()
            )
        })?;

        let mut entries = Vec::new();
        let known_empty = read_known_empty_state(&band_dir.join(KNOWN_EMPTY_STATE_NAME), band, schema)?;
        for i_dir in sorted_entries(&band_dir)? {
            let name = file_name(&i_dir)?;
            if name.starts_with('.') || name == CELL_INDEX_NAME {
                // `index.json` is this generator's own output, written into the tree.
                continue;
            }
            if !i_dir.is_dir() {
                return Err(format!(
                    "{}: expected a `<i>` directory or the generated `{CELL_INDEX_NAME}`",
                    i_dir.display()
                ));
            }
            read_cell_row(
                &i_dir,
                &name,
                band,
                tree,
                schema,
                base_url,
                &mut entries,
                &mut obcm_version,
                &mut terrain_revision,
                &mut pinned_artifacts,
            )?;
        }
        entries.sort_by(|a, b| a.id.cmp(&b.id));
        reject_known_empty_artifact_overlap(&band_dir, &entries, &known_empty)?;
        by_band.insert(band.id.clone(), entries);
        known_empty_by_band.insert(band.id.clone(), known_empty);
    }

    let total: usize = by_band.values().map(Vec::len).sum();
    if total == 0 {
        return Err(format!("{}: no cells found — refusing to publish an empty cell store", root.display()));
    }
    Ok(Cells {
        by_band,
        known_empty_by_band,
        obcm_version: obcm_version.expect("a cell was read"),
        terrain_revision: terrain_revision.expect("a cell was read"),
        pinned_artifacts,
    })
}

#[allow(clippy::too_many_arguments)]
fn read_cell_row(
    dir: &Path,
    i_text: &str,
    band: &BandEntry,
    tree: &Path,
    schema: &SchemaDoc,
    base_url: &str,
    out: &mut Vec<CellEntry>,
    obcm_version: &mut Option<u8>,
    terrain_revision: &mut Option<Option<u32>>,
    pinned_artifacts: &mut Vec<PinnedArtifact>,
) -> Result<(), String> {
    let mut sidecars: Vec<String> = Vec::new();
    let mut artifacts: Vec<(String, PathBuf)> = Vec::new();
    for entry in sorted_entries(dir)? {
        let name = file_name(&entry)?;
        if name.starts_with('.') {
            continue;
        }
        if let Some(stem) = name.strip_suffix(CELL_SIDECAR_EXT) {
            sidecars.push(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(CELL_EXT) {
            artifacts.push((stem.to_string(), entry));
        } else {
            return Err(format!(
                "{}: unexpected entry in a cell row (expected `<j>{CELL_EXT}` and `<j>{CELL_SIDECAR_EXT}`)",
                entry.display()
            ));
        }
    }

    for (j_text, path) in artifacts {
        let id = parse_strict_id(&format!("{}/{i_text}/{j_text}", band.cell_log2))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let sidecar_path = path.with_file_name(format!("{j_text}{CELL_SIDECAR_EXT}"));
        let sidecar = read_cell_sidecar(&sidecar_path)?;
        sidecars.retain(|s| s != &j_text);

        // A schema-revision bump is as hard a cut as an OBCM bump: assembly copies
        // chunk bytes between files, which is only meaningful within one revision.
        if sidecar.schema_revision != schema.revision {
            return Err(format!(
                "{}: cell was baked at schema revision {} but `{SCHEMA_DOC}` is revision {}. A schema-revision bump \
                 invalidates every cell (OBCA_Spec.md §6.3) — re-bake, or publish the revision the cells actually \
                 carry. There is no mixed-revision catalog.",
                path.display(),
                sidecar.schema_revision,
                schema.revision
            ));
        }

        // The world box is wider than the geographic domain and a cell may legally
        // overhang ±90°/±180° (OBCA_Spec.md §1.4), so this is the one header read
        // that must not apply the ordinary geographic-domain clamp.
        let header = read_obcm_header(&path)?;
        if header.version != OBCM_VERSION {
            return Err(format!(
                "{}: cell is OBCM v{} but this obc-pack writes v{OBCM_VERSION}. An OBCM format bump invalidates every \
                 baked cell (OBCC_Spec.md §10) — re-bake the store with the current packer.",
                path.display(),
                header.version
            ));
        }
        match obcm_version {
            None => *obcm_version = Some(header.version),
            Some(v) if *v != header.version => {
                return Err(format!("{}: cell is OBCM v{} but the store is v{v}", path.display(), header.version));
            }
            Some(_) => {}
        }

        // §13.4: the OBCM store as a whole was baked against one terrain revision or
        // against none. Two cells disagreeing means half the nav graph's ascents came
        // from one raster and half from another — a router that is right nowhere.
        match terrain_revision {
            None => *terrain_revision = Some(sidecar.terrain_revision),
            Some(have) if *have != sidecar.terrain_revision => {
                let name = |v: Option<u32>| v.map_or("none".to_string(), |r| r.to_string());
                return Err(format!(
                    "{}: cell was baked against terrain revision {} but the store was baked against {}. A cell store \
                     samples one terrain revision or none (OBCC_Spec.md §13.4) — re-bake the store.",
                    path.display(),
                    name(sidecar.terrain_revision),
                    name(*have)
                ));
            }
            Some(_) => {}
        }

        // §8: no bbox is stored, so this is where the identifier and the bytes are
        // made to agree. A cell whose header is not exactly its grid square would
        // graft into an assembly at the wrong place, silently.
        let (sq_min_lon, sq_min_lat, sq_max_lon, sq_max_lat) = id.square();
        let (hd_min_lon, hd_min_lat, hd_max_lon, hd_max_lat) = header.bbox;
        let got = (hd_min_lat, hd_min_lon, hd_max_lat, hd_max_lon);
        let want = (sq_min_lat, sq_min_lon, sq_max_lat, sq_max_lon);
        if got != want {
            return Err(format!(
                "{}: cell `{id}`'s header bbox is {got:?} but its grid square is {want:?}. A cell's bbox MUST be \
                 exactly its square (OBCA_Spec.md §3.1) — that is what lets an assembler copy its chunk bytes \
                 verbatim, and it is why the catalog stores no bbox for a cell.",
                path.display()
            ));
        }

        let (bytes, sha256) = hash_file(&path)?;
        let rel = path.strip_prefix(tree).map_err(|_| format!("{}: cell is outside the tree root", path.display()))?;
        let rel_path = rel_url_path(rel)?;
        let published_rel_path = content_addressed_rel_path(&rel_path, &sha256);
        out.push(CellEntry {
            id: id.to_string(),
            bytes,
            sha256: sha256.clone(),
            url: format!("{base_url}/{published_rel_path}"),
            built_at: sidecar.built_at,
            sources: sidecar.sources,
            partial: sidecar.partial,
        });
        pinned_artifacts.push(PinnedArtifact { rel_path, published_rel_path, bytes, sha256 });
    }

    if let Some(orphan) = sidecars.first() {
        return Err(format!(
            "{}: sidecar with no cell — `{orphan}{CELL_EXT}` is missing",
            dir.join(format!("{orphan}{CELL_SIDECAR_EXT}")).display()
        ));
    }
    Ok(())
}

fn read_cell_sidecar(path: &Path) -> Result<CellSidecar, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!("{}: {e} — every cell needs a sidecar (schema_revision, built_at, sources, partial)", path.display())
    })?;
    let mut sidecar: CellSidecar = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    validate_timestamp(&sidecar.built_at).map_err(|e| format!("{}: built_at {e}", path.display()))?;
    validate_sources(path, &mut sidecar.sources)?;
    Ok(sidecar)
}

fn validate_sources(path: &Path, sources: &mut [CellSource]) -> Result<(), String> {
    if sources.is_empty() {
        return Err(format!(
            "{}: `sources` is empty — a cell must record what it was baked from (OBCA_Spec.md §3.7)",
            path.display()
        ));
    }
    let mut seen = BTreeSet::new();
    for source in sources.iter() {
        validate_region_id(&source.extract_id).map_err(|e| format!("{}: sources.extract_id {e}", path.display()))?;
        validate_date(&source.snapshot).map_err(|e| format!("{}: sources.snapshot {e}", path.display()))?;
        if !seen.insert(source.extract_id.as_str()) {
            return Err(format!("{}: extract `{}` is listed twice", path.display(), source.extract_id));
        }
    }
    // §8 publishes sources sorted by extract_id; the order a bake job happened to
    // write them in is not content.
    sources.sort();
    Ok(())
}

fn read_known_empty_state(path: &Path, band: &BandEntry, schema: &SchemaDoc) -> Result<Vec<KnownEmptyRun>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut state: KnownEmptyState = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if state.schema_revision != schema.revision {
        return Err(format!(
            "{}: known-empty state is schema revision {} but `{SCHEMA_DOC}` is revision {}",
            path.display(),
            state.schema_revision,
            schema.revision
        ));
    }
    if state.band != band.id {
        return Err(format!(
            "{}: known-empty state says band `{}` but lives under `{}`",
            path.display(),
            state.band,
            band.id
        ));
    }

    let mut previous: Option<(CellId, KnownEmptyRun)> = None;
    for run in &mut state.known_empty {
        let start = parse_strict_id(&run.start).map_err(|e| format!("{}: {e}", path.display()))?;
        let end = parse_strict_id(&run.end).map_err(|e| format!("{}: {e}", path.display()))?;
        if start.to_string() != run.start || end.to_string() != run.end {
            return Err(format!(
                "{}: known-empty run {}..{} does not use canonical padded cell ids",
                path.display(),
                run.start,
                run.end
            ));
        }
        if start.log2 != u32::from(band.cell_log2) || end.log2 != u32::from(band.cell_log2) {
            return Err(format!(
                "{}: known-empty run {}..{} is not band `{}`'s 2^{} grid",
                path.display(),
                run.start,
                run.end,
                band.id,
                band.cell_log2
            ));
        }
        if start.i != end.i || start.j > end.j {
            return Err(format!(
                "{}: known-empty run {}..{} must be one non-empty inclusive row range",
                path.display(),
                run.start,
                run.end
            ));
        }
        validate_timestamp(&run.built_at).map_err(|e| format!("{}: built_at {e}", path.display()))?;
        validate_sources(path, &mut run.sources)?;
        if let Some((prev_end, ref prev)) = previous {
            if start.i < prev_end.i || (start.i == prev_end.i && start.j <= prev_end.j) {
                return Err(format!(
                    "{}: known-empty runs overlap or are out of order at {}..{}",
                    path.display(),
                    run.start,
                    run.end
                ));
            }
            if start.i == prev_end.i
                && start.j == prev_end.j + 1
                && run.built_at == prev.built_at
                && run.sources == prev.sources
            {
                return Err(format!(
                    "{}: adjacent known-empty runs {}..{} and {}..{} have identical provenance; merge them",
                    path.display(),
                    prev.start,
                    prev.end,
                    run.start,
                    run.end
                ));
            }
        }
        previous = Some((end, run.clone()));
    }
    known_empty_count(&state.known_empty)?;
    Ok(state.known_empty)
}

pub(super) fn known_empty_count(runs: &[KnownEmptyRun]) -> Result<u32, String> {
    inclusive_run_count(runs.iter().map(|run| (run.start.as_str(), run.end.as_str())))
}

fn reject_known_empty_artifact_overlap(
    band_dir: &Path,
    entries: &[CellEntry],
    runs: &[KnownEmptyRun],
) -> Result<(), String> {
    let index = BandIndex::new(&[], runs)?;
    for entry in entries {
        if matches!(index.get(&entry.id)?, Some(IndexedCell::KnownEmpty)) {
            return Err(format!(
                "{}: cell `{}` is both an OBCM artifact and known empty",
                band_dir.display(),
                entry.id
            ));
        }
    }
    Ok(())
}
