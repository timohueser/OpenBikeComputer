//! Region-tree scanning, selection validation, and region satellite generation.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::{
    boundary, document_json, file_name, parse_strict_id, sorted_entries, validate_id, BandIndex, Boundary, IndexedCell,
    IndexedTerrain, RegionCellsDocument, RegionEntry, RegionTerrain, Satellite, SchemaDoc, TerrainIndex,
    CATALOG_SCHEMA_VERSION, SCHEMA_DOC,
};

/// A boundary bigger than this is worth a warning: §7 budgets "a few KB" per
/// region, and the root carries one per region *before* a consumer knows anything
/// else about the catalog.
const BOUNDARY_WARN_BYTES: usize = 16 * 1024;

pub(super) const REGIONS_DIR: &str = "regions";
pub(super) const REGION_DOC: &str = "region.json";
pub(super) const REGION_POLY: &str = "boundary.poly";
const REGION_CELLS_NAME: &str = "cells.json";

/// The curated selection, as the tree states it. `parent` is **not** a field: it is
/// the nearest ancestor directory that is itself a region, so the curation cannot
/// declare a nesting the tree contradicts.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionDoc {
    name: String,
    /// Band id → the cell ids this region selects in that band. **Stored, not derived
    /// from the boundary** (§6): a simplification error must not be able to drop an
    /// edge cell, and two consumers with different point-in-polygon edge handling must
    /// not be able to disagree about what a region is.
    cells: BTreeMap<String, Vec<String>>,
    /// The terrain cell ids this region selects, by the same intersect rule applied to
    /// the terrain grid (§13.3). Empty or absent for a terrain-less catalog.
    #[serde(default)]
    terrain: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn read_regions(
    tree: &Path,
    schema: &SchemaDoc,
    by_band: &BTreeMap<&str, BandIndex<'_>>,
    terrain: Option<&TerrainIndex<'_>>,
    base_url: &str,
    satellites: &mut Vec<Satellite>,
    warnings: &mut Vec<String>,
) -> Result<Vec<RegionEntry>, String> {
    let root = tree.join(REGIONS_DIR);
    if !root.is_dir() {
        return Ok(Vec::new());
    }
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    collect_region_dirs(&root, &mut Vec::new(), &mut found)?;

    let ids: BTreeSet<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    let mut out = Vec::new();
    for (id, dir) in &found {
        // The nearest enclosing *region* — not merely the parent directory, which for
        // `europe/switzerland` is the uncurated `europe`.
        let parent = ancestors(id).find(|a| ids.contains(*a)).map(str::to_string);
        out.push(read_region(id, parent, dir, schema, by_band, terrain, base_url, satellites, warnings)?);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// `europe/germany/bayern` → `europe/germany`, `europe`.
fn ancestors(id: &str) -> impl Iterator<Item = &str> {
    let mut rest = Some(id);
    std::iter::from_fn(move || {
        let current = rest?;
        let cut = current.rfind('/')?;
        rest = Some(&current[..cut]);
        Some(&current[..cut])
    })
}

fn collect_region_dirs(dir: &Path, segments: &mut Vec<String>, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let mut subdirs = Vec::new();
    let mut is_region = false;
    for entry in sorted_entries(dir)? {
        let name = file_name(&entry)?;
        if name.starts_with('.') {
            continue;
        }
        if entry.is_dir() {
            validate_id(&name).map_err(|e| format!("{}: region path segment {e}", entry.display()))?;
            subdirs.push((name, entry));
        } else if name == REGION_DOC {
            is_region = true;
        } else if name == REGION_POLY || name == REGION_CELLS_NAME {
            // The outline's source, and this generator's own output.
        } else {
            return Err(format!(
                "{}: unexpected entry in a region tree (expected `{REGION_DOC}`, `{REGION_POLY}`, the generated \
                 `{REGION_CELLS_NAME}`, or a sub-region directory)",
                entry.display()
            ));
        }
    }
    if is_region {
        if segments.is_empty() {
            return Err(format!("{}: a region must live in a directory under `{REGIONS_DIR}/`", dir.display()));
        }
        out.push((segments.join("/"), dir.to_path_buf()));
    }
    for (name, path) in subdirs {
        segments.push(name);
        collect_region_dirs(&path, segments, out)?;
        segments.pop();
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn read_region(
    id: &str,
    parent: Option<String>,
    dir: &Path,
    schema: &SchemaDoc,
    by_band: &BTreeMap<&str, BandIndex<'_>>,
    terrain: Option<&TerrainIndex<'_>>,
    base_url: &str,
    satellites: &mut Vec<Satellite>,
    warnings: &mut Vec<String>,
) -> Result<RegionEntry, String> {
    let doc_path = dir.join(REGION_DOC);
    let text = std::fs::read_to_string(&doc_path).map_err(|e| format!("{}: {e}", doc_path.display()))?;
    let doc: RegionDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", doc_path.display()))?;
    if doc.name.trim().is_empty() {
        return Err(format!("{}: name is empty", doc_path.display()));
    }

    let mut cells: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut bytes_by_band = BTreeMap::new();
    let mut cell_count = BTreeMap::new();
    let mut partial_cell_count_by_band = BTreeMap::new();
    let mut total = 0u64;
    for band in &schema.bands {
        let listed = doc.cells.get(&band.id).map_or(&[][..], Vec::as_slice);
        let index = by_band.get(band.id.as_str()).expect("every band has an index");
        let mut ids = BTreeSet::new();
        let mut band_bytes = 0u64;
        let mut band_partial_cell_count = 0u32;
        for text in listed {
            let cell = parse_strict_id(text).map_err(|e| format!("{}: {e}", doc_path.display()))?;
            if cell.log2 != u32::from(band.cell_log2) {
                return Err(format!(
                    "{}: cell `{text}` is 2^{} µdeg but band `{}` is 2^{}",
                    doc_path.display(),
                    cell.log2,
                    band.id,
                    band.cell_log2
                ));
            }
            let canonical = cell.to_string();
            if !ids.insert(canonical.clone()) {
                return Err(format!(
                    "{}: cell `{canonical}` is listed twice in band `{}`",
                    doc_path.display(),
                    band.id
                ));
            }
            // §6: a region MUST NOT name a cell absent from the band's index. A
            // consumer would reject the pair, so publishing it is not an option.
            let entry =
                index.get(&canonical).map_err(|e| format!("{}: {e}", doc_path.display()))?.ok_or_else(|| {
                    format!(
                    "{}: band `{}` names cell `{canonical}`, which is not published — either bake it or drop it from \
                     the selection (OBCC_Spec.md §6)",
                    doc_path.display(),
                    band.id
                )
                })?;
            if let IndexedCell::Artifact(entry) = entry {
                band_bytes += entry.bytes;
                if entry.partial {
                    band_partial_cell_count += 1;
                }
            }
        }
        if ids.is_empty() {
            warnings.push(format!(
                "region `{id}` names no cells in band `{}` — that band's content will be missing from every assembly \
                 of this region",
                band.id
            ));
        }
        total += band_bytes;
        bytes_by_band.insert(band.id.clone(), band_bytes);
        cell_count.insert(band.id.clone(), ids.len() as u32);
        partial_cell_count_by_band.insert(band.id.clone(), band_partial_cell_count);
        cells.insert(band.id.clone(), ids.into_iter().collect());
    }
    for band in doc.cells.keys() {
        if !schema.bands.iter().any(|b| &b.id == band) {
            return Err(format!("{}: band `{band}` is not in `{SCHEMA_DOC}`'s band table", doc_path.display()));
        }
    }

    // §13.3: the terrain selection, resolved against the terrain index by exactly the
    // rule a band's is resolved against its own.
    let terrain_selection = read_region_terrain(id, &doc, &doc_path, terrain, warnings)?;

    let poly_path = dir.join(REGION_POLY);
    let poly = std::fs::read_to_string(&poly_path).map_err(|e| {
        format!(
            "{}: {e} — a region's outline comes from its Geofabrik `.poly` (`https://download.geofabrik.de/{id}.poly`)",
            poly_path.display()
        )
    })?;
    // One tolerance, [`boundary::DEFAULT_TOLERANCE_UDEG`], and it is published in the
    // document beside the rings it produced — a consumer reads what it was simplified at
    // rather than assuming.
    let rings = boundary::simplified_rings(&poly, boundary::DEFAULT_TOLERANCE_UDEG)
        .map_err(|e| format!("{}: {e}", poly_path.display()))?;
    let boundary = Boundary { tolerance_udeg: boundary::DEFAULT_TOLERANCE_UDEG, rings };
    let boundary_bytes = serde_json::to_string(&boundary).map_or(0, |json| json.len());
    if boundary_bytes > BOUNDARY_WARN_BYTES {
        warnings.push(format!(
            "region `{id}`: its outline is {boundary_bytes} bytes at a {} µdeg tolerance — §7 budgets a few KB per \
             region, and every one of them is in the root a consumer reads first",
            boundary::DEFAULT_TOLERANCE_UDEG
        ));
    }

    let cells_doc = RegionCellsDocument {
        schema_version: CATALOG_SCHEMA_VERSION,
        schema_revision: schema.revision,
        region_id: id.to_string(),
        cells,
        terrain: terrain_selection.as_ref().map(|s| s.ids.clone()).unwrap_or_default(),
    };
    let rel_path = format!("{REGIONS_DIR}/{id}/{REGION_CELLS_NAME}");
    let body = document_json(&cells_doc);
    let satellite = Satellite::new(rel_path, body);
    let cells_url = format!("{base_url}/{}", satellite.published_rel_path);
    let cells_bytes = satellite.bytes;
    let cells_sha256 = satellite.sha256.clone();
    satellites.push(satellite);

    Ok(RegionEntry {
        id: id.to_string(),
        name: doc.name,
        parent,
        boundary,
        bytes: total,
        bytes_by_band,
        cell_count,
        partial_cell_count_by_band,
        terrain: terrain_selection.map(|s| s.footprint),
        cells_url,
        cells_bytes,
        cells_sha256,
    })
}

/// One region's resolved terrain selection: the sorted ids for its satellite and the
/// priced footprint for the root.
struct RegionTerrainSelection {
    ids: Vec<String>,
    footprint: RegionTerrain,
}

fn read_region_terrain(
    id: &str,
    doc: &RegionDoc,
    doc_path: &Path,
    terrain: Option<&TerrainIndex<'_>>,
    warnings: &mut Vec<String>,
) -> Result<Option<RegionTerrainSelection>, String> {
    let Some(index) = terrain else {
        if !doc.terrain.is_empty() {
            return Err(format!(
                "{}: the region selects {} terrain cell(s) but the tree publishes no terrain — either bake it or drop \
                 the selection (OBCC_Spec.md §13.3)",
                doc_path.display(),
                doc.terrain.len()
            ));
        }
        return Ok(None);
    };
    if doc.terrain.is_empty() {
        warnings.push(format!(
            "region `{id}` selects no terrain cells, but this catalog publishes terrain — a rider choosing this \
             region gets a map with no elevation anywhere in it"
        ));
    }

    let mut ids = BTreeSet::new();
    let mut footprint = RegionTerrain { cell_count: 0, known_empty_count: 0, bytes: 0 };
    for text in &doc.terrain {
        let cell = parse_strict_id(text).map_err(|e| format!("{}: terrain: {e}", doc_path.display()))?;
        let canonical = cell.to_string();
        if !ids.insert(canonical.clone()) {
            return Err(format!("{}: terrain cell `{canonical}` is listed twice", doc_path.display()));
        }
        match index.get(&canonical).map_err(|e| format!("{}: {e}", doc_path.display()))? {
            Some(IndexedTerrain::Artifact(entry)) => {
                footprint.cell_count += 1;
                footprint.bytes += entry.bytes;
            }
            Some(IndexedTerrain::KnownEmpty) => footprint.known_empty_count += 1,
            None => {
                return Err(format!(
                    "{}: terrain cell `{canonical}` is not published — either bake it or drop it from the selection \
                     (OBCC_Spec.md §13.3)",
                    doc_path.display()
                ))
            }
        }
    }
    Ok(Some(RegionTerrainSelection { ids: ids.into_iter().collect(), footprint }))
}
