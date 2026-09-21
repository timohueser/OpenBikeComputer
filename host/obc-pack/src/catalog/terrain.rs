//! Terrain-store traversal, validation, and lookup.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use obc_formats::obct;

use crate::grid::CellId;

use super::coverage::{inclusive_run_count, CoverageIndex, IndexedCoverage};
use super::model::{ReferenceEntry, TerrainCellEntry, TerrainEmptyRun};
use super::validate::{parse_strict_id, validate_id, validate_timestamp};
use super::{
    content_addressed_rel_path, file_name, hash_file, rel_url_path, sorted_entries, PinnedArtifact, CELLS_DIR,
    CELL_INDEX_NAME, KNOWN_EMPTY_STATE_NAME, TERRAIN_DIR,
};

pub(super) const TERRAIN_DOC: &str = "terrain.json";
pub(super) const TERRAIN_EXT: &str = ".obcd";
pub(super) const TERRAIN_SIDECAR_EXT: &str = ".obcd.json";

/// The tree's terrain declaration: `terrain.json` beside `schema.json`.
///
/// A separate document from `schema.json` on purpose. The two describe stores on
/// **separate revision tracks** (§13.2), and a single document would be a single thing
/// to edit — the first way for an OBCM bump to look like it touched terrain.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TerrainDoc {
    pub(super) dataset_id: String,
    pub(super) dataset_version: String,
    pub(super) posting_log2: u8,
    pub(super) cell_log2: u8,
    /// The terrain store's own revision. Nothing here is `schema_revision`.
    pub(super) revision: u32,
    /// The source licence's required credit, verbatim. The bakery stamps
    /// `obc_elevation::COPERNICUS_ATTRIBUTION` here; this crate never hard-codes it, because
    /// a generic producer publishing another dataset owes a different notice.
    pub(super) attribution: String,
    /// Every reference model the bake's archive can credit. The published block names the
    /// ones a cell actually used, which the sidecars say — an archive holds sources whose
    /// tiles no published cell read, and a map does not owe those a notice.
    #[serde(default)]
    pub(super) references: Vec<ReferenceEntry>,
}

/// The facts a terrain cell's bytes cannot state. `dataset_version` is per cell as well
/// as in the root block for the same reason a cell's `schema_revision` is: it is what
/// lets the generator *refuse* a tree that mixes two bakes rather than publish one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainCellSidecar {
    terrain_revision: u32,
    dataset_version: String,
    built_at: String,
    /// Keys of the reference models this cell's crest lifts are derived from (§13.1).
    #[serde(default)]
    reference_sources: Vec<String>,
}

/// Local, un-published state behind the published all-`NODATA` runs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainKnownEmptyState {
    terrain_revision: u32,
    known_empty: Vec<TerrainEmptyRun>,
}

/// Everything the tree says about terrain, read once.
pub(super) struct TerrainStore {
    pub(super) doc: TerrainDoc,
    pub(super) cells: Vec<TerrainCellEntry>,
    pub(super) known_empty: Vec<TerrainEmptyRun>,
    pub(super) pinned_artifacts: Vec<PinnedArtifact>,
    /// The credits the published block carries: one entry per reference model a published cell
    /// used, in `key` order. Empty when the store has no crest lifts.
    pub(super) references: Vec<ReferenceEntry>,
}

pub(super) type TerrainIndex<'a> = CoverageIndex<'a, TerrainCellEntry>;
pub(super) type IndexedTerrain<'a> = IndexedCoverage<'a, TerrainCellEntry>;

pub(super) fn build_terrain_index<'a>(
    cells: &'a [TerrainCellEntry],
    known_empty: &[TerrainEmptyRun],
) -> Result<TerrainIndex<'a>, String> {
    CoverageIndex::new(
        cells.iter().map(|cell| (cell.id.as_str(), cell)),
        known_empty.iter().map(|run| (run.start.as_str(), run.end.as_str())),
    )
}

/// Walk `terrain.json` + `cells/terrain/` into the terrain store, or `None` when the
/// tree publishes no terrain at all.
pub(super) fn read_terrain(tree: &Path, base_url: &str) -> Result<Option<TerrainStore>, String> {
    let doc_path = tree.join(TERRAIN_DOC);
    let dir = tree.join(CELLS_DIR).join(TERRAIN_DIR);
    if !doc_path.is_file() {
        if dir.is_dir() {
            return Err(format!(
                "{}: terrain cells are in the tree but there is no `{TERRAIN_DOC}` — a terrain cell is only \
                 interpretable beside the dataset, pairing and revision it was baked at (OBCC_Spec.md §13.1)",
                dir.display()
            ));
        }
        return Ok(None);
    }
    let text = std::fs::read_to_string(&doc_path).map_err(|e| format!("{}: {e}", doc_path.display()))?;
    let doc: TerrainDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", doc_path.display()))?;
    let at = || doc_path.display().to_string();
    validate_id(&doc.dataset_id).map_err(|e| format!("{}: dataset_id {e}", at()))?;
    if doc.dataset_version.trim().is_empty() {
        return Err(format!("{}: `dataset_version` must be non-empty — it is half the lockstep key", at()));
    }
    if doc.attribution.trim().is_empty() {
        return Err(format!(
            "{}: `attribution` must be non-empty. The credit travels with the data as a licence obligation, and a \
             consumer reads it from the catalog rather than hard-coding it (OBCC_Spec.md §13.5).",
            at()
        ));
    }
    if doc.revision == 0 {
        return Err(format!("{}: `revision` starts at 1 — a terrain store has no revision zero", at()));
    }
    // A reference entry is three licence obligations and a key. Checked here, where the file can be
    // named, rather than left to a consumer: a blank credit reads as authoritative and shows a
    // rider nothing, and a generic producer's hand-written `terrain.json` is the likely source.
    for reference in &doc.references {
        validate_id(&reference.key).map_err(|e| format!("{}: reference key {e}", at()))?;
        for (field, value) in
            [("product", &reference.product), ("attribution", &reference.attribution), ("licence", &reference.licence)]
        {
            if value.trim().is_empty() {
                return Err(format!(
                    "{}: reference `{}` has an empty `{field}`. Every field of a listed reference is part of the \
                     credit §13.5 requires a consumer to display.",
                    at(),
                    reference.key
                ));
            }
        }
    }
    // One call validates both ranges *and* the pairing: a cell smaller than one tile,
    // or one whose block would outrun the directory's `uint32` offsets, is not a
    // terrain store OBCT can express.
    obct::cell_samples_log2(doc.posting_log2, doc.cell_log2).ok_or_else(|| {
        format!(
            "{}: posting 2^{} µdeg with cell 2^{} µdeg is not a pairing OBCT permits (OBCT_Spec.md §1.3)",
            at(),
            doc.posting_log2,
            doc.cell_log2
        )
    })?;

    let known_empty = read_terrain_known_empty(&dir.join(KNOWN_EMPTY_STATE_NAME), &doc)?;
    let mut cells = Vec::new();
    let mut pinned_artifacts = Vec::new();
    let mut used = BTreeSet::new();
    let mut surface = None;
    if dir.is_dir() {
        for i_dir in sorted_entries(&dir)? {
            let name = file_name(&i_dir)?;
            if name.starts_with('.') || name == CELL_INDEX_NAME {
                continue;
            }
            if !i_dir.is_dir() {
                return Err(format!(
                    "{}: expected a `<i>` directory or the generated `{CELL_INDEX_NAME}`",
                    i_dir.display()
                ));
            }
            let mut sink = RowSink { cells: &mut cells, pinned_artifacts: &mut pinned_artifacts, used: &mut used };
            let row = read_terrain_row(&i_dir, &name, &doc, tree, base_url, &mut sink)?;
            check_surface_encoding(&mut surface, row, &i_dir)?;
        }
    }
    cells.sort_by(|a, b| a.id.cmp(&b.id));

    // §13.5's obligation is per source a cell actually read, so the published list is the sidecars'
    // keys resolved against what the tree can credit. A key `terrain.json` cannot resolve is a
    // refusal rather than an omission: the map would ship derived national elevation with no notice.
    let references: Vec<ReferenceEntry> = used
        .iter()
        .map(|key| {
            doc.references.iter().find(|entry| &entry.key == key).cloned().ok_or_else(|| {
                format!(
                    "{}: terrain cells credit reference source `{key}`, which `{TERRAIN_DOC}` does not declare. \
                     Those cells are derived from it and cannot be published without its notice (OBCC_Spec.md \
                     §13.5). Re-run `obc-bake terrain --reference <mirror>` against the archive they were baked \
                     from, which is where the wording lives.",
                    dir.display()
                )
            })
        })
        .collect::<Result<_, String>>()?;

    // The same rule §8 states for a band: a square is an artifact or it is empty, never
    // both. A catalog that said both would leave a consumer to pick one.
    let index = build_terrain_index(&[], &known_empty)?;
    for cell in &cells {
        if matches!(index.get(&cell.id)?, Some(IndexedTerrain::KnownEmpty)) {
            return Err(format!(
                "{}: terrain cell `{}` is both an OBCT artifact and known empty",
                dir.display(),
                cell.id
            ));
        }
    }
    if cells.is_empty() && known_empty.is_empty() {
        return Err(format!(
            "{}: `{TERRAIN_DOC}` declares a terrain store with no cells and no known-empty coverage — publish the \
             cells or drop the document",
            at()
        ));
    }
    Ok(Some(TerrainStore { doc, cells, known_empty, pinned_artifacts, references }))
}

/// What one row of the walk appends to: the published entries, their pins, and the reference source
/// keys the row's cells credit.
struct RowSink<'a> {
    cells: &'a mut Vec<TerrainCellEntry>,
    pinned_artifacts: &'a mut Vec<PinnedArtifact>,
    used: &'a mut BTreeSet<String>,
}

fn read_terrain_row(
    dir: &Path,
    i_text: &str,
    doc: &TerrainDoc,
    tree: &Path,
    base_url: &str,
    sink: &mut RowSink<'_>,
) -> Result<Option<bool>, String> {
    let mut sidecars: Vec<String> = Vec::new();
    let mut artifacts: Vec<(String, PathBuf)> = Vec::new();
    let mut surface = None;
    for entry in sorted_entries(dir)? {
        let name = file_name(&entry)?;
        if name.starts_with('.') {
            continue;
        }
        if let Some(stem) = name.strip_suffix(TERRAIN_SIDECAR_EXT) {
            sidecars.push(stem.to_string());
        } else if let Some(stem) = name.strip_suffix(TERRAIN_EXT) {
            artifacts.push((stem.to_string(), entry));
        } else {
            return Err(format!(
                "{}: unexpected entry in a terrain row (expected `<j>{TERRAIN_EXT}` and `<j>{TERRAIN_SIDECAR_EXT}`)",
                entry.display()
            ));
        }
    }

    for (j_text, path) in artifacts {
        let id = parse_strict_id(&format!("{}/{i_text}/{j_text}", doc.cell_log2))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let sidecar_path = path.with_file_name(format!("{j_text}{TERRAIN_SIDECAR_EXT}"));
        let sidecar_text = std::fs::read_to_string(&sidecar_path).map_err(|e| {
            format!(
                "{}: {e} — every terrain cell needs a sidecar (terrain_revision, dataset_version, built_at)",
                sidecar_path.display()
            )
        })?;
        let sidecar: TerrainCellSidecar =
            serde_json::from_str(&sidecar_text).map_err(|e| format!("{}: {e}", sidecar_path.display()))?;
        validate_timestamp(&sidecar.built_at).map_err(|e| format!("{}: built_at {e}", sidecar_path.display()))?;
        sink.used.extend(sidecar.reference_sources.iter().cloned());
        sidecars.retain(|s| s != &j_text);

        // §13.2's lockstep, per cell. Two of the four keys are in the bytes and checked
        // below; these two cannot be, so they are recorded and compared here.
        if sidecar.terrain_revision != doc.revision {
            return Err(format!(
                "{}: terrain cell was baked at terrain revision {} but `{TERRAIN_DOC}` is revision {}. Terrain is \
                 lockstep within its own track (OBCC_Spec.md §13.2) — re-bake, or publish the revision the cells \
                 actually carry.",
                path.display(),
                sidecar.terrain_revision,
                doc.revision
            ));
        }
        if sidecar.dataset_version != doc.dataset_version {
            return Err(format!(
                "{}: terrain cell was baked from dataset version `{}` but `{TERRAIN_DOC}` says `{}` — a mixed-dataset \
                 raster has a discontinuity at every seam between the two",
                path.display(),
                sidecar.dataset_version,
                doc.dataset_version
            ));
        }

        // The OBCT analogue of "a cell's header bbox is exactly its square": the
        // container states its own rectangle, and a 1 × 1 rectangle at (i, j) is the
        // only thing a cell named `<log2>/<i>/<j>` may be.
        let header = read_obct_header(&path)?;
        if (header.posting_log2, header.cell_log2) != (doc.posting_log2, doc.cell_log2) {
            return Err(format!(
                "{}: cell is posting 2^{} / cell 2^{} but `{TERRAIN_DOC}` declares 2^{} / 2^{} — one lattice per \
                 terrain revision (OBCC_Spec.md §13.2)",
                path.display(),
                header.posting_log2,
                header.cell_log2,
                doc.posting_log2,
                doc.cell_log2
            ));
        }
        if (header.rows, header.cols) != (1, 1) {
            return Err(format!(
                "{}: a published terrain cell is a container whose rectangle is 1 × 1 (OBCT_Spec.md §4.1), but this \
                 one is {} × {} — that is a shard, not a cell",
                path.display(),
                header.rows,
                header.cols
            ));
        }
        if (i64::from(header.min_i), i64::from(header.min_j)) != (id.i, id.j) {
            return Err(format!(
                "{}: cell `{id}`'s container covers ({}, {}) — a cell whose header disagrees with its id would place \
                 its raster somewhere else, silently",
                path.display(),
                header.min_i,
                header.min_j
            ));
        }
        check_surface_encoding(&mut surface, Some(header.surface), &path)?;

        let (bytes, sha256) = hash_file(&path)?;
        let rel = path
            .strip_prefix(tree)
            .map_err(|_| format!("{}: terrain cell is outside the tree root", path.display()))?;
        let rel_path = rel_url_path(rel)?;
        let published_rel_path = content_addressed_rel_path(&rel_path, &sha256);
        sink.cells.push(TerrainCellEntry {
            id: id.to_string(),
            bytes,
            sha256: sha256.clone(),
            url: format!("{base_url}/{published_rel_path}"),
            built_at: sidecar.built_at,
        });
        sink.pinned_artifacts.push(PinnedArtifact { rel_path, published_rel_path, bytes, sha256 });
    }

    if let Some(orphan) = sidecars.first() {
        return Err(format!(
            "{}: sidecar with no terrain cell — `{orphan}{TERRAIN_EXT}` is missing",
            dir.join(format!("{orphan}{TERRAIN_SIDECAR_EXT}")).display()
        ));
    }
    Ok(surface)
}

fn check_surface_encoding(expected: &mut Option<bool>, current: Option<bool>, path: &Path) -> Result<(), String> {
    if let Some(current) = current {
        if *expected.get_or_insert(current) != current {
            return Err(format!(
                "{}: terrain store mixes native and indexed cell blocks; re-bake all published terrain cells \
                 with the same baker before publishing",
                path.display()
            ));
        }
    }
    Ok(())
}

fn read_terrain_known_empty(path: &Path, doc: &TerrainDoc) -> Result<Vec<TerrainEmptyRun>, String> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let state: TerrainKnownEmptyState = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    if state.terrain_revision != doc.revision {
        return Err(format!(
            "{}: known-empty state is terrain revision {} but `{TERRAIN_DOC}` is revision {}",
            path.display(),
            state.terrain_revision,
            doc.revision
        ));
    }

    let mut previous: Option<(CellId, &TerrainEmptyRun)> = None;
    for run in &state.known_empty {
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
        if start.log2 != u32::from(doc.cell_log2) || end.log2 != u32::from(doc.cell_log2) {
            return Err(format!(
                "{}: known-empty run {}..{} is not the terrain grid's 2^{} cells",
                path.display(),
                run.start,
                run.end,
                doc.cell_log2
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
        if let Some((prev_end, prev)) = previous {
            if start.i < prev_end.i || (start.i == prev_end.i && start.j <= prev_end.j) {
                return Err(format!(
                    "{}: known-empty runs overlap or are out of order at {}..{}",
                    path.display(),
                    run.start,
                    run.end
                ));
            }
            if start.i == prev_end.i && start.j == prev_end.j + 1 && run.built_at == prev.built_at {
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
        previous = Some((end, run));
    }
    inclusive_run_count(state.known_empty.iter().map(|r| (r.start.as_str(), r.end.as_str())))?;
    Ok(state.known_empty)
}

/// The container's identity, after validation through the device's terrain reader.
struct ObctHeader {
    posting_log2: u8,
    cell_log2: u8,
    min_i: u32,
    min_j: u32,
    rows: u16,
    cols: u16,
    surface: bool,
}

fn read_obct_header(path: &Path) -> Result<ObctHeader, String> {
    let source = crate::terrain::open_obct(path)?;
    let reader = obc_elevation::TerrainReader::parse(&source)
        .map_err(|e| format!("{}: not a usable OBCT artifact ({e:?})", path.display()))?;
    let header = reader.header();
    Ok(ObctHeader {
        posting_log2: header.posting_log2,
        cell_log2: header.cell_log2,
        min_i: header.cell_min_i,
        min_j: header.cell_min_j,
        rows: header.cell_rows,
        cols: header.cell_cols,
        surface: header.flags & obct::SURFACE_FLAG != 0,
    })
}
