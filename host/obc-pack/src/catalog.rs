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

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::grid::UBox;
use obc_formats::io::rd_i32;
use obc_formats::obcm::{HEADER_LEN, MAGIC};

pub mod boundary;

/// This module's envelope version. A consumer MUST reject a `schema_version` it
/// does not implement (`OBCC_Spec.md` §1).
pub const CATALOG_SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_MANIFEST_NAME: &str = "catalog.json";

mod cells;
mod coverage;
mod model;
mod regions;
mod schema;
mod terrain;
mod validate;

pub use model::*;
pub use schema::{
    catalog_schema, catalog_schema_json, CATALOG_EXAMPLE_JSON, CATALOG_SCHEMA_JSON, CELL_INDEX_EXAMPLE_JSON,
    REGION_CELLS_EXAMPLE_JSON, TERRAIN_INDEX_EXAMPLE_JSON,
};
pub use validate::{format_timestamp, now_timestamp, parse_strict_id, validate_date, validate_timestamp};

use cells::{build_band_index, known_empty_count, read_cells, BandIndex};
use coverage::inclusive_run_count;
use regions::read_regions;
use schema::{read_schema_doc, read_skins};
use terrain::{build_terrain_index, read_terrain};

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
        by_band.insert(band.id.as_str(), build_band_index(entries, known_empty)?);
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
            terrain_index = Some(build_terrain_index(&store.cells, &store.known_empty)?);
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

// --- schema/skin compatibility façade -----------------------------------------------------

/// A config's canonical `feature_type → style id` assignment.
///
/// Public because producers must use the same 1-based document order the cell
/// feature headers carry. The private schema owner supplies the implementation.
pub fn feature_type_ids(config: &Config) -> BTreeMap<String, u8> {
    schema::feature_type_ids(config)
}

/// Prove that `skin` is a presentation-only skin over `schema`: it has exactly
/// the same feature types and style ids.
pub fn check_skin(schema: &Config, skin: &Config) -> Result<(), String> {
    schema::check_skin(schema, skin)
}

/// Refuse schema-producing keys in a skin document before parsing erases the
/// difference between an absent key and a restated default.
pub fn check_skin_document(json: &str, at: &str) -> Result<(), String> {
    schema::check_skin_document(json, at)
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
