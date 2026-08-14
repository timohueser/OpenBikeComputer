//! The **cell catalog**: a cell store, one schema, a
//! set of skins, and named regions that are *selections* rather than artifacts.
//! [`OBCC_Spec.md`](../../../../specs/OBCC_Spec.md) is normative and this module
//! is its only sanctioned producer; the grid, band, and cell semantics it publishes
//! are [`OBCA_Spec.md`](../../../../specs/OBCA_Spec.md).
//!
//! The important producer-side guarantees are:
//!
//! - **Cells are the artifacts.** A cell's coverage is *exactly* its grid square, so
//!   a cell entry carries no bbox: the id determines the square to the microdegree
//!   and this generator **verifies the artifact's own header bbox equals it**
//!   (§8). A stored copy could only agree (redundant) or disagree (a lie).
//! - **The document is a root plus digest-pinned satellites.** DACH is thousands of
//!   cells and a planet store is far more, so the cell lists move out of the root —
//!   but each satellite is pinned by `bytes` + `sha256` from the root, preserving
//!   the all-or-nothing guarantee per document (§9).
//! - **A region stores its cell set and carries a drawable boundary.** Deriving the
//!   set from the outline would let a simplification error silently drop a fine cell
//!   — a hole in street detail — so the outline is presentation only (§6, §7).
//! - **Skins cannot lag cells.** A skin is stamped onto ~2 KB at assembly time, so
//!   changing one invalidates no cell (§5).
//!
//! Cell paths are keyed by **band**
//! (`cells/<band>/<i>/<j>.<sha256>.obcm`, `cells/<band>/index.<sha256>.json`)
//! rather than by
//! `<log2(S)>`. The recommended band table gives `fine` and `network` the same `2^18` cell
//! size ([`OBCA_Spec.md` §1.5](../../../../specs/OBCA_Spec.md)), so a `<log2>`-keyed
//! path is not a function of (band, cell) and the two bands' indices and artifacts
//! would collide. Every published `url` is explicit in the manifest, and the band's
//! `cell_log2` is published beside it, so nothing a consumer does depends on the
//! spelling of the path (§2).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use obc_formats::io::rd_i32;
use obc_formats::obcm::{HEADER_LEN, MAGIC};
use obc_formats::obct;

use crate::config::{Config, LineStyle};
use crate::grid::{CellId, UBox, MAX_CELL_LOG2, MIN_CELL_LOG2};

pub mod boundary;

/// This module's envelope version. A consumer MUST reject a `schema_version` it
/// does not implement (`OBCC_Spec.md` §1).
pub const CATALOG_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_MANIFEST_NAME: &str = "catalog.json";

mod cells;
mod model;
mod regions;
mod schema;
mod validate;

pub use model::*;
pub use schema::{
    catalog_schema, catalog_schema_json, CATALOG_EXAMPLE_JSON, CATALOG_SCHEMA_JSON, CELL_INDEX_EXAMPLE_JSON,
    REGION_CELLS_EXAMPLE_JSON, TERRAIN_INDEX_EXAMPLE_JSON,
};
pub use validate::{format_timestamp, now_timestamp, parse_strict_id, validate_date, validate_timestamp};

use cells::{known_empty_count, read_cells, BandIndex};
use regions::read_regions;
use validate::{validate_id, validate_region_id};

// --- generation ---------------------------------------------------------------------------

/// Generator inputs that cannot be derived from the tree.
#[derive(Debug, Clone)]
pub struct CatalogOptions {
    /// Where the tree gets published; every `url` is this plus the object's
    /// digest-addressed publish path. Local bake-tree paths remain stable.
    pub base_url: String,
    /// The root's `generated_at`, RFC 3339 UTC. Passed in so the generator is a pure
    /// function of (tree, options).
    pub generated_at: String,
}

impl CatalogOptions {
    pub fn new(base_url: impl Into<String>, generated_at: impl Into<String>) -> CatalogOptions {
        CatalogOptions { base_url: base_url.into(), generated_at: generated_at.into() }
    }
}

/// A satellite document: its stable local path, immutable published path, and exact
/// bytes — the bytes the root's digest pins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Satellite {
    /// Stable path used inside the local bake tree.
    pub rel_path: String,
    /// Immutable path used below `CatalogOptions::base_url` when publishing.
    pub published_rel_path: String,
    pub body: String,
    pub bytes: u64,
    pub sha256: String,
}

impl Satellite {
    fn new(rel_path: String, body: String) -> Self {
        let (bytes, sha256) = hash_str(&body);
        let published_rel_path = content_addressed_rel_path(&rel_path, &sha256);
        Self { rel_path, published_rel_path, body, bytes, sha256 }
    }
}

/// A digest-pinned file already present at a stable path in the local bake tree.
/// The publisher uploads it under `published_rel_path`, so replacing a catalog
/// root never invalidates a root that a consumer fetched a moment earlier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinnedArtifact {
    pub rel_path: String,
    pub published_rel_path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// A generated catalog: the root, the satellites it pins, and non-fatal
/// observations. Warnings are returned rather than printed so the bakery decides
/// whether a coverage gap is a log line or a failed job.
#[derive(Debug, Clone)]
pub struct GeneratedCatalog {
    pub root: Catalog,
    pub satellites: Vec<Satellite>,
    pub pinned_artifacts: Vec<PinnedArtifact>,
    pub warnings: Vec<String>,
}

const CELLS_DIR: &str = "cells";
const SKINS_DIR: &str = "skins";
const PREVIEWS_DIR: &str = "previews";
const SCHEMA_DOC: &str = "schema.json";
const CELL_INDEX_NAME: &str = "index.json";
/// The reserved directory under `cells/` that holds the terrain artifact class, and
/// therefore a band id no schema may use (§13.1).
pub const TERRAIN_DIR: &str = "terrain";
/// The tree's terrain declaration: dataset, pairing, revision.
const TERRAIN_DOC: &str = "terrain.json";
const TERRAIN_EXT: &str = ".obcd";
const TERRAIN_SIDECAR_EXT: &str = ".obcd.json";
/// Local bakery state for cells whose semantic payload is empty. The leading dot
/// keeps it out of publication; its validated ranges are copied into the pinned
/// band satellite instead.
const KNOWN_EMPTY_STATE_NAME: &str = ".known-empty.json";

/// Walk a bake tree and build the root plus its satellites.
///
/// ```text
/// <tree>/
///   schema.json                        the packer config the cells were baked with; `_meta` adds the band table
///   skins/<skin_id>.json               one packer config per skin (same style ids, different values)
///   cells/<band>/<i>/<j>.obcm          a cell artifact
///   cells/<band>/<i>/<j>.obcm.json     its sidecar (schema revision, build time, sources, partial)
///   regions/<a>/…/region.json          the curated selection: display name + its cell ids per band
///   regions/<a>/…/boundary.poly        that region's Geofabrik .poly, simplified into the outline
///   cells/<band>/index.json            generated (§8)
///   regions/<a>/…/cells.json           generated (§6)
///   catalog.json                       generated root
/// ```
///
/// The generated documents are written *into* the tree, so re-running over a tree the
/// generator has already visited is normal and its own output is skipped by name.
pub fn generate(tree: &Path, opts: &CatalogOptions) -> Result<GeneratedCatalog, String> {
    let base_url = normalize_base_url(&opts.base_url)?;
    validate_timestamp(&opts.generated_at).map_err(|e| format!("generated_at: {e}"))?;

    let schema = read_schema_doc(&tree.join(SCHEMA_DOC))?;
    let mut warnings = Vec::new();

    // Cells first: the schema's `obcm_version` is read out of their headers, so there
    // is no schema entry to publish until every cell has agreed.
    let cells = read_cells(tree, &schema, &base_url)?;
    let obcm_version = cells.obcm_version;
    // The other artifact class, read on its own terms: nothing above is an input to it
    // and nothing in it is an input to the above (§13.2).
    let terrain = read_terrain(tree, &base_url)?;

    let (skins, mut pinned_artifacts) =
        read_skins(&tree.join(SKINS_DIR), &tree.join(PREVIEWS_DIR), &schema, &base_url)?;
    pinned_artifacts.extend(cells.pinned_artifacts.iter().cloned());
    if let Some(store) = &terrain {
        pinned_artifacts.extend(store.pinned_artifacts.iter().cloned());
    }

    let mut satellites = Vec::new();
    let mut cell_index = Vec::new();
    let mut by_band: BTreeMap<&str, BandIndex<'_>> = BTreeMap::new();
    for band in &schema.bands {
        let entries = cells.by_band.get(band.id.as_str()).map_or(&[][..], Vec::as_slice);
        let known_empty = cells.known_empty_by_band.get(band.id.as_str()).map_or(&[][..], Vec::as_slice);
        if entries.is_empty() && known_empty.is_empty() {
            warnings.push(format!(
                "band `{}` has no published or known-empty cells — every assembly at this schema will be missing \
                 what that band carries",
                band.id
            ));
        }
        let doc = CellIndexDocument {
            schema_version: CATALOG_SCHEMA_VERSION,
            schema_revision: schema.revision,
            band: band.id.clone(),
            cells: entries.to_vec(),
            known_empty: known_empty.to_vec(),
        };
        let rel_path = format!("{CELLS_DIR}/{}/{CELL_INDEX_NAME}", band.id);
        let body = document_json(&doc);
        let satellite = Satellite::new(rel_path, body);
        cell_index.push(CellIndexRef {
            band: band.id.clone(),
            cell_log2: band.cell_log2,
            cell_count: entries.len() as u32,
            known_empty_count: known_empty_count(known_empty)?,
            bytes: satellite.bytes,
            sha256: satellite.sha256.clone(),
            url: format!("{base_url}/{}", satellite.published_rel_path),
        });
        satellites.push(satellite);
        by_band.insert(band.id.as_str(), BandIndex::new(entries, known_empty)?);
    }
    // §3: sorted by `cell_log2` descending — coarse first. Two bands may share a
    // size (`fine` and `network` are both 2^18), so the band id breaks the tie
    // and the order stays total.
    cell_index.sort_by(|a, b| (b.cell_log2, &a.band).cmp(&(a.cell_log2, &b.band)));

    // The terrain block and its one pinned index — the §8 machinery, reused whole.
    let mut terrain_index = None;
    let terrain_entry = match &terrain {
        None => None,
        Some(store) => {
            let doc = TerrainIndexDocument {
                schema_version: CATALOG_SCHEMA_VERSION,
                terrain_revision: store.doc.revision,
                dataset_id: store.doc.dataset_id.clone(),
                dataset_version: store.doc.dataset_version.clone(),
                posting_log2: store.doc.posting_log2,
                cell_log2: store.doc.cell_log2,
                cells: store.cells.clone(),
                known_empty: store.known_empty.clone(),
            };
            let rel_path = format!("{CELLS_DIR}/{TERRAIN_DIR}/{CELL_INDEX_NAME}");
            let satellite = Satellite::new(rel_path, document_json(&doc));
            let entry = TerrainEntry {
                dataset_id: store.doc.dataset_id.clone(),
                dataset_version: store.doc.dataset_version.clone(),
                posting_log2: store.doc.posting_log2,
                cell_log2: store.doc.cell_log2,
                terrain_revision: store.doc.revision,
                attribution: store.doc.attribution.clone(),
                cell_index: TerrainIndexRef {
                    cell_count: store.cells.len() as u32,
                    known_empty_count: inclusive_run_count(
                        store.known_empty.iter().map(|r| (r.start.as_str(), r.end.as_str())),
                    )?,
                    bytes: satellite.bytes,
                    sha256: satellite.sha256.clone(),
                    url: format!("{base_url}/{}", satellite.published_rel_path),
                },
            };
            satellites.push(satellite);
            terrain_index = Some(TerrainIndex::new(&store.cells, &store.known_empty)?);
            Some(entry)
        }
    };

    // §13.4, the one coupling — reported, never silently reconciled. The bake guard
    // turns this into a refusal to publish; the generator still produces the document
    // so an operator can see exactly what drifted.
    match (&terrain_entry, cells.terrain_revision) {
        (Some(t), Some(baked)) if baked != t.terrain_revision => warnings.push(format!(
            "the network band was baked against terrain revision {baked}, but this catalog publishes terrain \
             revision {} — the router's baked ascents and the published raster are two different surfaces. Re-bake \
             the network band against the current terrain (OBCC_Spec.md §13.4).",
            t.terrain_revision
        )),
        (Some(t), None) => warnings.push(format!(
            "this catalog publishes terrain revision {} but the cell store was baked without terrain — every nav \
             edge's ascent is zero, so climb-aware routing is off while the raster the device draws is not",
            t.terrain_revision
        )),
        (None, Some(baked)) => warnings.push(format!(
            "the cell store was baked against terrain revision {baked}, but this catalog publishes no terrain — the \
             raster those ascents came from is not being published"
        )),
        _ => {}
    }

    let regions =
        read_regions(tree, &schema, &by_band, terrain_index.as_ref(), &base_url, &mut satellites, &mut warnings)?;
    if regions.is_empty() {
        warnings.push(
            "no regions — the catalog offers cells but no named selection, so a builder has nothing to pick from"
                .to_string(),
        );
    }

    let root = Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        generated_at: opts.generated_at.clone(),
        source: Some(osm_source()),
        schema: SchemaEntry {
            id: schema.id,
            revision: schema.revision,
            name: schema.name,
            description: schema.description,
            obcm_version,
            grid: GridEntry { origin_udeg: GRID_ORIGIN_UDEG, world_side_udeg: WORLD_SIDE_UDEG },
            lods: schema.lods,
            bands: schema.bands,
            styles: schema.styles,
            routing: schema.routing,
            chunk_size: schema.chunk_size,
        },
        skins,
        regions,
        cell_index,
        terrain: terrain_entry,
        network_terrain_revision: cells.terrain_revision,
    };
    pinned_artifacts.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(GeneratedCatalog { root, satellites, pinned_artifacts, warnings })
}

/// Pretty JSON with a trailing newline, so a published document diffs line-by-line
/// and generation is byte-reproducible.
pub fn root_json(root: &Catalog) -> String {
    let mut text = serde_json::to_string_pretty(root).expect("catalog root serializes");
    text.push('\n');
    text
}

/// The store's stable-keyed `LICENSE.txt` name (§2, §11).
pub const LICENSE_NAME: &str = "LICENSE.txt";

/// §3.1's human-readable twin: the provenance and licence statement published at the
/// store root beside `catalog.json`. Derived from the root's `source` block — and from
/// the terrain block's §13.5 attribution when one is published — so the machine-readable
/// declaration and the text a person reads can never disagree.
pub fn license_txt(root: &Catalog) -> String {
    let source = root.source.as_ref().expect("a generated root always carries a source block (§3.1)");
    let mut text = format!(
        "The map cells (*.obcm) and documents in this store are derived from\n\
         OpenStreetMap data.\n\
         \n\
         {attribution}\n\
         \n\
         As a derivative database, the store is made available under the\n\
         {license} license: {url}\n\
         See https://www.openstreetmap.org/copyright for details.\n",
        attribution = source.attribution,
        license = source.license,
        url = source.license_url,
    );
    if let Some(terrain) = &root.terrain {
        text.push_str(&format!(
            "\nThe terrain artifacts (*.obcd) are a separate artifact class,\n{attribution}.\n",
            attribution = terrain.attribution,
        ));
    }
    text
}

fn document_json<T: Serialize>(doc: &T) -> String {
    let mut text = serde_json::to_string_pretty(doc).expect("catalog satellite serializes");
    text.push('\n');
    text
}

fn hash_str(body: &str) -> (u64, String) {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(body.as_bytes());
    (body.len() as u64, hex(&hasher.finalize()))
}

/// Insert an object's digest before its final extension. The local bake tree keeps
/// stable, human-readable paths, while published references are immutable:
/// `cells/fine/1204/1052.obcm` becomes
/// `cells/fine/1204/1052.<sha256>.obcm`.
fn content_addressed_rel_path(rel_path: &str, sha256: &str) -> String {
    let (prefix, name) = rel_path.rsplit_once('/').map_or(("", rel_path), |(prefix, name)| (prefix, name));
    let (stem, extension) = name.rsplit_once('.').map_or((name, ""), |(stem, extension)| (stem, extension));
    let addressed =
        if extension.is_empty() { format!("{stem}.{sha256}") } else { format!("{stem}.{sha256}.{extension}") };
    if prefix.is_empty() {
        addressed
    } else {
        format!("{prefix}/{addressed}")
    }
}

/// Write the whole catalog into the tree: **satellites first, root last**.
///
/// That order is §9's digest pinning made operational — the root is the document
/// that claims a satellite exists with a given digest, so it must not become visible
/// until every satellite it names is on disk with those exact bytes. Each file is
/// written temp-then-`rename`, so no reader
/// ever sees a half-written document.
pub fn write_all_atomic(tree: &Path, generated: &GeneratedCatalog) -> Result<(), String> {
    for satellite in &generated.satellites {
        let path = tree.join(satellite.rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        write_atomic_bytes(&path, &satellite.body)?;
    }
    // §3.1's human-readable twin, at its stable key beside the root.
    write_atomic_bytes(&tree.join(LICENSE_NAME), &license_txt(&generated.root))?;
    write_atomic_bytes(&tree.join(DEFAULT_MANIFEST_NAME), &root_json(&generated.root))
}

// --- the schema document ------------------------------------------------------------------

/// The schema as the tree states it: the packer config the cells were baked with,
/// whose `_meta` adds the revision and the band table.
///
/// One document rather than two because the parts must not be able to disagree: the
/// style-id assignment, the LOD ladder, `chunk_size` and the routing table are read
/// out of the very config that produced the cells' bytes, not out of a hand-written
/// description of it.
struct SchemaDoc {
    id: String,
    revision: u32,
    name: String,
    description: String,
    lods: Vec<LodEntry>,
    bands: Vec<BandEntry>,
    styles: Vec<StyleAssignment>,
    routing: RoutingEntry,
    chunk_size: u32,
    /// Feature type → its style values, for checking a skin covers exactly this set.
    feature_types: BTreeMap<String, u8>,
}

#[derive(Debug, Deserialize)]
struct SchemaMetaDoc {
    #[serde(rename = "_meta")]
    meta: Option<SchemaMeta>,
}

#[derive(Debug, Deserialize)]
struct SchemaMeta {
    id: String,
    name: String,
    description: String,
    /// The cell store's identity. Bumping it invalidates every cell
    /// (`OBCA_Spec.md` §6.3), which is why it is stated here and recorded in every
    /// cell sidecar: the generator can then refuse a tree that mixes revisions.
    revision: u32,
    bands: Vec<BandDoc>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BandDoc {
    id: String,
    cell_log2: u8,
    #[serde(default)]
    lods: Vec<u32>,
    #[serde(default)]
    sections: Vec<BandSection>,
    role: BandRole,
}

fn read_schema_doc(path: &Path) -> Result<SchemaDoc, String> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        format!(
            "{}: {e} — a bake tree's `{SCHEMA_DOC}` is the packer config its cells were baked with, plus a `_meta` \
             block carrying `revision` and `bands`",
            path.display()
        )
    })?;
    let doc: SchemaMetaDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = doc
        .meta
        .ok_or_else(|| format!("{}: no `_meta` block (id, name, description, revision, bands)", path.display()))?;
    validate_id(&meta.id).map_err(|e| format!("{}: schema id {e}", path.display()))?;
    if meta.name.trim().is_empty() || meta.description.trim().is_empty() {
        return Err(format!("{}: `_meta.name` and `_meta.description` must be non-empty", path.display()));
    }
    if meta.revision == 0 {
        return Err(format!("{}: `_meta.revision` starts at 1 — a cell store has no revision zero", path.display()));
    }

    let config = Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let bands = check_band_table(&meta.bands, config.lods.len(), path)?;
    let lods = ladder(&config, &bands, path)?;
    let (styles, feature_types) = style_assignment(&config, path)?;

    Ok(SchemaDoc {
        id: meta.id,
        revision: meta.revision,
        name: meta.name,
        description: meta.description,
        lods,
        bands,
        styles,
        routing: RoutingEntry {
            min_component_edges: config.routing.min_component_edges as u32,
            profiles: config.routing.profiles.iter().map(|p| p.name.clone()).collect(),
        },
        chunk_size: config.chunk_size as u32,
        feature_types,
    })
}

/// The band table's rules (§4, `OBCA_Spec.md` §1.2/§5.1), all of which a consumer
/// must reject and which therefore must not be publishable in the first place:
/// every ladder LOD in exactly one band, the nav and POI sections in exactly one
/// band, exactly one `core` band carrying the sections and no LOD, at most one
/// `coarse` band, everything else `geometry`.
fn check_band_table(bands: &[BandDoc], lod_count: usize, path: &Path) -> Result<Vec<BandEntry>, String> {
    let at = || path.display().to_string();
    if bands.is_empty() {
        return Err(format!("{}: `_meta.bands` is empty — a cell store needs a band table", at()));
    }
    let mut ids = BTreeSet::new();
    let mut lod_owner: BTreeMap<u32, &str> = BTreeMap::new();
    let mut section_owner: BTreeMap<BandSection, &str> = BTreeMap::new();
    let mut cores = Vec::new();
    let mut coarses = Vec::new();
    let mut out = Vec::new();
    for band in bands {
        validate_id(&band.id).map_err(|e| format!("{}: band id {e}", at()))?;
        if band.id == TERRAIN_DIR {
            return Err(format!(
                "{}: `{TERRAIN_DIR}` is reserved — `cells/{TERRAIN_DIR}/` holds the terrain artifact class, which is \
                 on its own revision track and is not a band (OBCC_Spec.md §13.1)",
                at()
            ));
        }
        if !ids.insert(band.id.as_str()) {
            return Err(format!("{}: band `{}` is listed twice", at(), band.id));
        }
        if !(MIN_CELL_LOG2..=MAX_CELL_LOG2).contains(&u32::from(band.cell_log2)) {
            return Err(format!(
                "{}: band `{}`: cell size 2^{} µdeg is outside 2^{MIN_CELL_LOG2}..=2^{MAX_CELL_LOG2}",
                at(),
                band.id,
                band.cell_log2
            ));
        }
        let mut lods = band.lods.clone();
        lods.sort_unstable();
        lods.dedup();
        if lods.len() != band.lods.len() {
            return Err(format!("{}: band `{}` lists a LOD twice", at(), band.id));
        }
        for &lod in &lods {
            if lod as usize >= lod_count {
                return Err(format!(
                    "{}: band `{}` claims LOD {lod}, but the ladder has {lod_count} level(s)",
                    at(),
                    band.id
                ));
            }
            if let Some(other) = lod_owner.insert(lod, &band.id) {
                return Err(format!(
                    "{}: LOD {lod} is in both band `{other}` and band `{}` — it would be written twice \
                     (OBCA_Spec.md §1.2)",
                    at(),
                    band.id
                ));
            }
        }
        let mut sections = band.sections.clone();
        sections.sort_unstable();
        sections.dedup();
        if sections.len() != band.sections.len() {
            return Err(format!("{}: band `{}` lists a section twice", at(), band.id));
        }
        for &section in &sections {
            if let Some(other) = section_owner.insert(section, &band.id) {
                return Err(format!(
                    "{}: the {section:?} section is in both band `{other}` and band `{}` — it belongs to exactly one",
                    at(),
                    band.id
                ));
            }
        }
        match band.role {
            BandRole::Core => {
                cores.push(band.id.as_str());
                if !lods.is_empty() {
                    return Err(format!(
                        "{}: the `core` band `{}` carries LOD(s) {lods:?}. The core file is the one file of a volume \
                         set that cannot be split by bbox, so no geometry may live in it (OBCA_Spec.md §5.1) — its \
                         headroom under 4 GiB is the design's hard limit.",
                        at(),
                        band.id
                    ));
                }
                if sections != [BandSection::Nav, BandSection::Poi] {
                    return Err(format!(
                        "{}: the `core` band `{}` must carry both the `nav` and `poi` sections (got {sections:?})",
                        at(),
                        band.id
                    ));
                }
            }
            BandRole::Coarse | BandRole::Geometry => {
                coarses.extend(matches!(band.role, BandRole::Coarse).then_some(band.id.as_str()));
                if lods.is_empty() {
                    return Err(format!("{}: band `{}` carries no LOD and is not the `core` band", at(), band.id));
                }
                if !sections.is_empty() {
                    return Err(format!(
                        "{}: band `{}` carries {sections:?}, but only the `core` band may carry a section",
                        at(),
                        band.id
                    ));
                }
            }
        }
        out.push(BandEntry { id: band.id.clone(), cell_log2: band.cell_log2, lods, sections, role: band.role });
    }

    if cores.len() != 1 {
        return Err(format!("{}: exactly one band must have `role: core` (got {cores:?})", at()));
    }
    if coarses.len() > 1 {
        return Err(format!("{}: at most one band may have `role: coarse` (got {coarses:?})", at()));
    }
    for section in [BandSection::Nav, BandSection::Poi] {
        if !section_owner.contains_key(&section) {
            return Err(format!("{}: no band carries the {section:?} section", at()));
        }
    }
    let missing: Vec<usize> = (0..lod_count).filter(|l| !lod_owner.contains_key(&(*l as u32))).collect();
    if !missing.is_empty() {
        return Err(format!(
            "{}: ladder LOD(s) {missing:?} are in no band — a map would be blank at that zoom (OBCA_Spec.md §1.2)",
            at()
        ));
    }

    // Published in band order as authored, which is the coarse→fine reading order of
    // the table; determinism comes from the file, not from a map.
    Ok(out)
}

/// The LOD ladder as the catalog publishes it, cross-checked against `OBCM_Spec.md`
/// §3: exactly one `+inf` level and it is index 0, strictly decreasing after.
fn ladder(config: &Config, bands: &[BandEntry], path: &Path) -> Result<Vec<LodEntry>, String> {
    let mut out = Vec::with_capacity(config.lods.len());
    let mut previous: Option<f64> = None;
    for (index, lod) in config.lods.iter().enumerate() {
        match (index, lod.max_mpp) {
            (0, None) => {}
            (0, Some(mpp)) => {
                return Err(format!(
                    "{}: ladder LOD 0 has max_mpp {mpp} — the coarsest level is +inf (`null`), OBCM_Spec.md §3",
                    path.display()
                ))
            }
            (_, None) => {
                return Err(format!("{}: ladder LOD {index} is +inf; only LOD 0 may be", path.display()));
            }
            (_, Some(mpp)) => {
                if let Some(prev) = previous {
                    if mpp >= prev {
                        return Err(format!(
                            "{}: ladder max_mpp must strictly decrease ({prev} then {mpp} at LOD {index})",
                            path.display()
                        ));
                    }
                }
                previous = Some(mpp);
            }
        }
        let band = bands
            .iter()
            .find(|b| b.lods.contains(&(index as u32)))
            .expect("check_band_table proved every LOD has a band");
        out.push(LodEntry { index: index as u32, max_mpp: lod.max_mpp, band: band.id.clone() });
    }
    Ok(out)
}

/// The canonical style-id assignment, read out of the config that assigned it —
/// [`feature_type_ids`], plus the duplicate-id check and the id-keyed view of it.
///
/// Returned sorted by id, which is also the order a style table is written in
/// (`OBCM_Spec.md` §2) and the order a skin's entries follow.
fn style_assignment(config: &Config, path: &Path) -> Result<(Vec<StyleAssignment>, BTreeMap<String, u8>), String> {
    let by_type = feature_type_ids(config);
    if by_type.is_empty() {
        return Err(format!("{}: no feature types — a schema with no styles draws nothing", path.display()));
    }
    // The same assignment read the other way round, which is also where a collision
    // shows up: two feature types on one id would make the published style table
    // ambiguous about which one a chunk's feature header meant.
    let mut by_id: BTreeMap<u8, String> = BTreeMap::new();
    for (feature_type, &id) in &by_type {
        if let Some(other) = by_id.insert(id, feature_type.clone()) {
            return Err(format!(
                "{}: style id {id} is assigned to both `{other}` and `{feature_type}`",
                path.display()
            ));
        }
    }
    let styles = by_id.into_iter().map(|(id, feature_type)| StyleAssignment { id, feature_type }).collect();
    Ok((styles, by_type))
}

/// A config's `feature_type → style id` assignment: `highway.primary → 3`, and so on
/// for every `(tag key, tag value)` pair it styles.
///
/// Public because it is the thing a **producer** has to agree with this generator
/// about. `obc-pack` numbers feature types 1-based in config document order and those
/// ids are referenced by every feature header in every chunk (`OBCM_Spec.md` §5.2), so
/// the assignment is part of the cells' bytes.
///
/// The one place this walk lives: [`style_assignment`] and [`check_skin`] both read
/// the assignment through here, so the generator and the producer-side check cannot
/// come to disagree about what a config assigns.
pub fn feature_type_ids(config: &Config) -> BTreeMap<String, u8> {
    let mut by_type = BTreeMap::new();
    for (tag_key, values) in &config.features {
        for (tag_value, style) in values {
            by_type.insert(format!("{tag_key}.{tag_value}"), style.id);
        }
    }
    by_type
}

/// Prove a config **is a skin over** `schema`: same feature types, same style ids
/// (`OBCC_Spec.md` §5, `OBCA_Spec.md` §4.7).
///
/// This is the check [`generate`] applies to every document in a tree's `skins/`, and
/// it is public so a producer can apply the *same* one before it spends hours cutting
/// cells a skin turns out not to fit. A skin may change only the presentation values
/// of a style record: introducing, dropping, reordering or renumbering a feature type
/// is a new schema and therefore a re-bake, because those ids are already baked into
/// every chunk of every cell.
pub fn check_skin(schema: &Config, skin: &Config) -> Result<(), String> {
    check_skin_ids(&feature_type_ids(schema), &feature_type_ids(skin))
}

/// The presentation-only keys a style record in a skin may carry (`OBCC_Spec.md`
/// §5). Everything else in a packer config decides which bytes get written, and a
/// skin is stamped onto bytes that already exist.
/// `fixed_width` and `terrain_layer` (#1095) are on the list for the same reason `line_style` is:
/// they are **flag bits of the 8-byte style record**, which is precisely the ≈ 2 KB a skin stamps.
/// Neither decides which bytes get written — a contour is cut into the same cells whether it later
/// draws hairline or ramped — so both restyle without a re-bake.
const SKIN_STYLE_KEYS: &[&str] =
    &["color", "color2", "weight", "z_index", "priority", "line_style", "fixed_width", "terrain_layer"];

/// Prove a skin **document** is presentation only: no schema keys, at either level.
///
/// [`check_skin`] compares two parsed [`Config`]s and therefore cannot see this at
/// all — by the time a config exists, a missing `lods` and a `lods` restating the
/// defaults are the same value, so a skin carrying a whole LOD ladder parses into
/// something that looks exactly like a skin that carries none. The keys have to be
/// caught in the JSON, before that information is thrown away.
///
/// Silently dropping them would be the worse failure: a skin is stamped onto cells
/// that were cut at the *schema's* ladder, tolerances, merge passes and routing table,
/// so a skin that thinks it changes any of those is a document whose author believes
/// something false. The values would have no effect, the author would have no way to
/// find that out, and the map would quietly not be the one they wrote. That is a new
/// schema revision and a re-bake (epic #1016 D2), and the error says so — naming every
/// offending key rather than the first, so one edit fixes the document.
///
/// `min_lod` is in the list for the same reason `lods` is: it decides the level a
/// feature is first written at, which is a decision already baked into every cell.
pub fn check_skin_document(json: &str, at: &str) -> Result<(), String> {
    let doc: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("{at}: {e}"))?;
    let obj = doc.as_object().ok_or_else(|| format!("{at}: a skin document is a JSON object"))?;

    let mut offenders: Vec<String> = obj
        .keys()
        .filter(|k| !matches!(k.as_str(), "_meta" | "features" | "marker"))
        .map(|k| format!("`{k}`"))
        .collect();
    // `min_lod` hides one level down, per style record, and is the one a hand-written
    // skin picks up most easily — it is on nearly every line of the schema it was
    // copied from.
    let mut culled: BTreeSet<&str> = BTreeSet::new();
    if let Some(features) = obj.get("features").and_then(serde_json::Value::as_object) {
        for values in features.values().filter_map(serde_json::Value::as_object) {
            for style in values.values().filter_map(serde_json::Value::as_object) {
                for key in style.keys() {
                    if !SKIN_STYLE_KEYS.contains(&key.as_str()) {
                        culled.insert(key.as_str());
                    }
                }
            }
        }
    }
    offenders.extend(culled.into_iter().map(|k| format!("`features.*.*.{k}`")));

    if offenders.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{at}: a skin is presentation only, and this one carries schema key(s): {}. A skin is stamped onto cells \
         already cut at the schema's ladder, tolerances, merge passes and routing table, so these would have no \
         effect — changing any of them is a new schema revision and a re-bake (OBCC_Spec.md §5). Remove them; a \
         style record may carry {}.",
        offenders.join(", "),
        SKIN_STYLE_KEYS.iter().map(|k| format!("`{k}`")).collect::<Vec<_>>().join(", ")
    ))
}

/// [`check_skin`] against the assignments themselves, so the generator can reuse it
/// with the one it already read out of the tree's `schema.json`.
fn check_skin_ids(want: &BTreeMap<String, u8>, have: &BTreeMap<String, u8>) -> Result<(), String> {
    let unknown: Vec<&str> = have.keys().filter(|t| !want.contains_key(*t)).map(String::as_str).collect();
    if !unknown.is_empty() {
        return Err(format!(
            "this skin styles feature type(s) the schema does not have: {}. A skin is a recolor of one schema — a new \
             feature type is a new schema revision and a re-bake.",
            joined(unknown.into_iter())
        ));
    }
    let missing: Vec<&str> = want.keys().filter(|t| !have.contains_key(*t)).map(String::as_str).collect();
    if !missing.is_empty() {
        return Err(format!(
            "this skin has no style for {}. A missing style would ship a map with an invisible layer \
             (OBCC_Spec.md §5).",
            joined(missing.into_iter())
        ));
    }
    let renumbered: Vec<String> = have
        .iter()
        .filter(|(feature_type, id)| want[*feature_type] != **id)
        .map(|(feature_type, id)| format!("`{feature_type}` is id {id} here but {} in the schema", want[feature_type]))
        .collect();
    if !renumbered.is_empty() {
        return Err(format!(
            "a skin MUST NOT renumber style ids — every feature header in every baked chunk references them \
             (OBCA_Spec.md §4.7): {}",
            renumbered.join("; ")
        ));
    }
    Ok(())
}

// --- skins --------------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct SkinMetaDoc {
    #[serde(rename = "_meta")]
    meta: Option<SkinMeta>,
}

#[derive(Debug, Deserialize)]
struct SkinMeta {
    id: String,
    name: String,
    description: String,
    version: u32,
}

fn read_skins(
    dir: &Path,
    previews_dir: &Path,
    schema: &SchemaDoc,
    base_url: &str,
) -> Result<(Vec<SkinEntry>, Vec<PinnedArtifact>), String> {
    if !dir.is_dir() {
        return Err(format!("{}: no `{SKINS_DIR}/` directory — a catalog offers at least one skin", dir.display()));
    }
    let mut skins = Vec::new();
    let mut pinned_artifacts = Vec::new();
    for path in sorted_entries(dir)? {
        let name = file_name(&path)?;
        if name.starts_with('.') || path.is_dir() {
            continue;
        }
        let Some(stem) = name.strip_suffix(".json") else {
            return Err(format!("{}: only `<skin_id>.json` skin configs belong in `{SKINS_DIR}/`", path.display()));
        };
        validate_id(stem).map_err(|e| format!("{}: skin id {e}", path.display()))?;
        let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: SkinMetaDoc = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let meta =
            doc.meta.ok_or_else(|| format!("{}: no `_meta` block (id, name, description, version)", path.display()))?;
        if meta.id != stem {
            return Err(format!("{}: `_meta.id` is `{}` but the filename says `{stem}`", path.display(), meta.id));
        }
        if meta.name.trim().is_empty() || meta.description.trim().is_empty() {
            return Err(format!("{}: `_meta.name` and `_meta.description` must be non-empty", path.display()));
        }
        check_skin_document(&text, &path.display().to_string())?;
        let config = Config::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let styles = skin_styles(&config, schema, &path)?;
        let preview_path = previews_dir.join(format!("{}.png", meta.id));
        let preview = if preview_path.exists() {
            let (bytes, sha256) = hash_file(&preview_path)?;
            let rel_path = format!("{PREVIEWS_DIR}/{}.png", meta.id);
            let published_rel_path = content_addressed_rel_path(&rel_path, &sha256);
            let url = format!("{base_url}/{published_rel_path}");
            pinned_artifacts.push(PinnedArtifact { rel_path, published_rel_path, bytes, sha256: sha256.clone() });
            Some(SkinPreview { url, bytes, sha256 })
        } else {
            None
        };
        skins.push(SkinEntry {
            id: meta.id,
            name: meta.name,
            description: meta.description,
            version: meta.version,
            marker_color: config.marker_color,
            styles,
            preview,
        });
    }
    if skins.is_empty() {
        return Err(format!("{}: no skin configs found", dir.display()));
    }
    Ok((skins, pinned_artifacts))
}

/// A skin's style values, in the schema's id order, after [`check_skin`] has proved
/// the skin is a skin: same feature types, same ids.
fn skin_styles(config: &Config, schema: &SchemaDoc, path: &Path) -> Result<Vec<SkinStyle>, String> {
    let mut by_type = BTreeMap::new();
    for (tag_key, values) in &config.features {
        for (tag_value, style) in values {
            by_type.insert(format!("{tag_key}.{tag_value}"), style.clone());
        }
    }
    check_skin_ids(&schema.feature_types, &feature_type_ids(config)).map_err(|e| format!("{}: {e}", path.display()))?;

    // Schema order, so `skins[].styles[k]` and `schema.styles[k]` describe the same
    // feature type without a consumer having to join on the name.
    let mut styles: Vec<SkinStyle> = by_type
        .into_iter()
        .map(|(feature_type, s)| SkinStyle {
            feature_type,
            color: s.color,
            weight: s.weight,
            z_index: s.z_index,
            priority: s.priority,
            dashed: s.line_style == LineStyle::Dashed,
            fixed_width: s.fixed_width,
            terrain_layer: s.terrain_layer,
            color2: s.color2,
        })
        .collect();
    styles.sort_by_key(|s| schema.feature_types[&s.feature_type]);
    Ok(styles)
}

fn joined<'a>(items: impl Iterator<Item = &'a str>) -> String {
    items.collect::<Vec<_>>().join(", ")
}

/// The cells an inclusive-row-run list covers. Shared by the band indexes and the
/// terrain index so the two cannot come to count a run differently.
fn inclusive_run_count<'a>(runs: impl Iterator<Item = (&'a str, &'a str)>) -> Result<u32, String> {
    let mut total = 0u32;
    for (start, end) in runs {
        let start = parse_strict_id(start)?;
        let end = parse_strict_id(end)?;
        let width = end
            .j
            .checked_sub(start.j)
            .and_then(|n| n.checked_add(1))
            .and_then(|n| u32::try_from(n).ok())
            .ok_or("known-empty run overflow")?;
        total = total.checked_add(width).ok_or("known-empty cell count exceeds u32")?;
    }
    Ok(total)
}

// --- terrain (§13) --------------------------------------------------------------------------

/// The tree's terrain declaration: `terrain.json` beside `schema.json`.
///
/// A separate document from `schema.json` on purpose. The two describe stores on
/// **separate revision tracks** (§13.2), and a single document would be a single thing
/// to edit — the first way for an OBCM bump to look like it touched terrain.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainDoc {
    dataset_id: String,
    dataset_version: String,
    posting_log2: u8,
    cell_log2: u8,
    /// The terrain store's own revision. Nothing here is `schema_revision`.
    revision: u32,
    /// The source licence's required credit, verbatim. The bakery stamps
    /// `obc_dem::COPERNICUS_ATTRIBUTION` here; this crate never hard-codes it, because
    /// a generic producer publishing another dataset owes a different notice.
    attribution: String,
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
}

/// Local, un-published state behind the published all-`NODATA` runs.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TerrainKnownEmptyState {
    terrain_revision: u32,
    known_empty: Vec<TerrainEmptyRun>,
}

/// Everything the tree says about terrain, read once.
struct TerrainStore {
    doc: TerrainDoc,
    cells: Vec<TerrainCellEntry>,
    known_empty: Vec<TerrainEmptyRun>,
    pinned_artifacts: Vec<PinnedArtifact>,
}

/// Lookup over the terrain index, the same shape [`BandIndex`] has: an artifact or a
/// verified-empty square, and nothing else is a published cell.
struct TerrainIndex<'a> {
    cells: BTreeMap<&'a str, &'a TerrainCellEntry>,
    empty_by_row: BTreeMap<i64, Vec<(i64, i64)>>,
}

enum IndexedTerrain<'a> {
    Artifact(&'a TerrainCellEntry),
    KnownEmpty,
}

impl<'a> TerrainIndex<'a> {
    fn new(cells: &'a [TerrainCellEntry], known_empty: &[TerrainEmptyRun]) -> Result<Self, String> {
        let mut empty_by_row: BTreeMap<i64, Vec<(i64, i64)>> = BTreeMap::new();
        for run in known_empty {
            let start = parse_strict_id(&run.start)?;
            let end = parse_strict_id(&run.end)?;
            empty_by_row.entry(start.i).or_default().push((start.j, end.j));
        }
        Ok(Self { cells: cells.iter().map(|cell| (cell.id.as_str(), cell)).collect(), empty_by_row })
    }

    fn get(&self, id: &str) -> Result<Option<IndexedTerrain<'_>>, String> {
        if let Some(cell) = self.cells.get(id) {
            return Ok(Some(IndexedTerrain::Artifact(cell)));
        }
        let cell = parse_strict_id(id)?;
        let Some(runs) = self.empty_by_row.get(&cell.i) else { return Ok(None) };
        let at = runs.partition_point(|(_, end)| *end < cell.j);
        Ok(runs.get(at).filter(|(start, end)| *start <= cell.j && cell.j <= *end).map(|_| IndexedTerrain::KnownEmpty))
    }
}

/// Walk `terrain.json` + `cells/terrain/` into the terrain store, or `None` when the
/// tree publishes no terrain at all.
fn read_terrain(tree: &Path, base_url: &str) -> Result<Option<TerrainStore>, String> {
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
            read_terrain_row(&i_dir, &name, &doc, tree, base_url, &mut cells, &mut pinned_artifacts)?;
        }
    }
    cells.sort_by(|a, b| a.id.cmp(&b.id));

    // The same rule §8 states for a band: a square is an artifact or it is empty, never
    // both. A catalog that said both would leave a consumer to pick one.
    let index = TerrainIndex::new(&[], &known_empty)?;
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
    Ok(Some(TerrainStore { doc, cells, known_empty, pinned_artifacts }))
}

fn read_terrain_row(
    dir: &Path,
    i_text: &str,
    doc: &TerrainDoc,
    tree: &Path,
    base_url: &str,
    out: &mut Vec<TerrainCellEntry>,
    pinned_artifacts: &mut Vec<PinnedArtifact>,
) -> Result<(), String> {
    let mut sidecars: Vec<String> = Vec::new();
    let mut artifacts: Vec<(String, PathBuf)> = Vec::new();
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

        let (bytes, sha256) = hash_file(&path)?;
        let rel = path
            .strip_prefix(tree)
            .map_err(|_| format!("{}: terrain cell is outside the tree root", path.display()))?;
        let rel_path = rel_url_path(rel)?;
        let published_rel_path = content_addressed_rel_path(&rel_path, &sha256);
        out.push(TerrainCellEntry {
            id: id.to_string(),
            bytes,
            sha256: sha256.clone(),
            url: format!("{base_url}/{published_rel_path}"),
            built_at: sidecar.built_at,
        });
        pinned_artifacts.push(PinnedArtifact { rel_path, published_rel_path, bytes, sha256 });
    }

    if let Some(orphan) = sidecars.first() {
        return Err(format!(
            "{}: sidecar with no terrain cell — `{orphan}{TERRAIN_EXT}` is missing",
            dir.join(format!("{orphan}{TERRAIN_SIDECAR_EXT}")).display()
        ));
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

/// What a terrain container states about itself (`OBCT_Spec.md` §4.2). Read directly
/// rather than through `obc-elevation`'s reader: the generator's job is to check the
/// header against the id, and the whole 2 MiB block is not needed to do it.
struct ObctHeader {
    posting_log2: u8,
    cell_log2: u8,
    min_i: u32,
    min_j: u32,
    rows: u16,
    cols: u16,
}

fn read_obct_header(path: &Path) -> Result<ObctHeader, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut header = [0u8; obct::HEADER_LEN];
    file.read_exact(&mut header).map_err(|e| {
        format!("{}: {e} — too short to be an OBCT artifact ({}-byte header)", path.display(), obct::HEADER_LEN)
    })?;
    obct::validate_header_prefix(&header)
        .map_err(|e| format!("{}: not an OBCT v{} artifact ({e:?})", path.display(), obct::VERSION))?;
    if header[obct::HDR_FLAGS] != 0 || header[obct::HDR_RESERVED..].iter().any(|&b| b != 0) {
        return Err(format!(
            "{}: OBCT flags/reserved bytes are not zero — a v1 reader MUST refuse the file (OBCT_Spec.md §4.5)",
            path.display()
        ));
    }
    let u32_at = |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().expect("4 bytes"));
    let u16_at = |at: usize| u16::from_le_bytes(header[at..at + 2].try_into().expect("2 bytes"));
    Ok(ObctHeader {
        posting_log2: header[obct::HDR_POSTING_LOG2],
        cell_log2: header[obct::HDR_CELL_LOG2],
        min_i: u32_at(obct::HDR_CELL_MIN_I),
        min_j: u32_at(obct::HDR_CELL_MIN_J),
        rows: u16_at(obct::HDR_CELL_ROWS),
        cols: u16_at(obct::HDR_CELL_COLS),
    })
}

// --- shared producer mechanics -----------------------------------------------------------

struct ObcmHeader {
    version: u8,
    /// The header's own bbox, in [`UBox`] order (`min_lon, min_lat, max_lon, max_lat`) so it
    /// compares directly against [`CellId::square`].
    bbox: UBox,
}

fn read_obcm_header(path: &Path) -> Result<ObcmHeader, String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header).map_err(|e| {
        format!("{}: {e} — too short to be an OBCM artifact ({HEADER_LEN}-byte header)", path.display())
    })?;
    if header[..4] != MAGIC {
        return Err(format!("{}: not an OBCM file (bad magic)", path.display()));
    }
    let bbox: UBox = (
        i64::from(rd_i32(&header, 9)),
        i64::from(rd_i32(&header, 5)),
        i64::from(rd_i32(&header, 17)),
        i64::from(rd_i32(&header, 13)),
    );
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    if min_lat > max_lat || min_lon > max_lon {
        return Err(format!("{}: header bbox is inverted ({bbox:?}, lon/lat µdeg)", path.display()));
    }
    let limit = i64::from(-GRID_ORIGIN_UDEG);
    if min_lat < -limit || max_lat > limit || min_lon < -limit || max_lon > limit {
        return Err(format!("{}: header bbox is outside the world ({bbox:?}, lon/lat µdeg)", path.display()));
    }
    Ok(ObcmHeader { version: header[4], bbox })
}

fn hash_file(path: &Path) -> Result<(u64, String), String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let declared = file.metadata().map_err(|e| format!("{}: {e}", path.display()))?.len();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    if total != declared {
        return Err(format!(
            "{}: changed while being hashed ({declared} bytes declared, {total} read)",
            path.display()
        ));
    }
    Ok((total, hex(&hasher.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        paths.push(entry.map_err(|e| format!("{}: {e}", dir.display()))?.path());
    }
    paths.sort();
    Ok(paths)
}

fn file_name(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_string)
        .ok_or_else(|| format!("{}: non-UTF-8 filename", path.display()))
}

fn rel_url_path(rel: &Path) -> Result<String, String> {
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(segment) => {
                parts.push(segment.to_str().ok_or_else(|| format!("{}: non-UTF-8 path", rel.display()))?)
            }
            _ => return Err(format!("{}: unexpected path component", rel.display())),
        }
    }
    Ok(parts.join("/"))
}

fn normalize_base_url(base: &str) -> Result<String, String> {
    let trimmed = base.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("base URL is empty".to_string());
    }
    if !trimmed.starts_with("https://") && !trimmed.starts_with("http://") && !trimmed.starts_with('/') {
        return Err(format!("base URL `{base}` must be absolute (`https://…`) or root-relative (`/…`)"));
    }
    if trimmed.chars().any(char::is_whitespace) {
        return Err(format!("base URL `{base}` contains whitespace"));
    }
    Ok(trimmed.to_string())
}

fn write_atomic_bytes(path: &Path, body: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("tmp");
    let result = (|| {
        let mut file = File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.write_all(body.as_bytes()).map_err(|e| format!("{}: {e}", tmp.display()))?;
        file.sync_all().map_err(|e| format!("{}: {e}", tmp.display()))?;
        fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests;
