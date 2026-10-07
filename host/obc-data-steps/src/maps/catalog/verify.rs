//! Read pinned catalog metadata and assemble only selections whose binary inputs changed.

use std::collections::BTreeMap;

use obc_data::engine::release::Release;
use obc_data::store::{hash_file, sha256_hex, Store};
use obc_file_source::FileSource;
use obc_formats::io::ByteSource;
use obc_pack::catalog::coverage::{CoverageIndex, IndexedCoverage};
use obc_pack::catalog::*;
use obcm_assemble::{
    assemble_full, Articles, CellInput, KnownEmptyInput, MapStyles, NoClock, Options, Schema, TerrainCellInput,
    TerrainJob, TerrainParams,
};

use super::LAYER;

struct CatalogInputs {
    root: Catalog,
    bands: BTreeMap<String, CellIndexDocument>,
    terrain: TerrainIndexDocument,
    articles: BTreeMap<String, ArticleCellEntry>,
    picks: BTreeMap<String, RegionCellsDocument>,
}

impl CatalogInputs {
    fn read(release: &Release, store: &Store) -> Result<Self, String> {
        let layer = release.layers.iter().find(|layer| layer.step == LAYER).ok_or("release has no maps catalog")?;
        let named = |name: &str| -> Result<Vec<u8>, String> {
            let file =
                layer.files.iter().find(|file| file.path == name).ok_or_else(|| format!("catalog has no `{name}`"))?;
            checked(store, &file.sha256, file.size)
        };
        let root: Catalog = serde_json::from_slice(&named("catalog.json")?).map_err(|e| e.to_string())?;
        if root.schema_version != CATALOG_SCHEMA_VERSION || sha256_hex(&named("schema.json")?) != root.schema.sha256 {
            return Err("catalog schema envelope or named schema digest differs".into());
        }
        let owned = release.objects();
        let pin = |sha256: &str, bytes: u64, url: &str| -> Result<Vec<u8>, String> {
            if owned.get(sha256) != Some(&bytes) || url != format!("/cell-catalog/objects/{sha256}") {
                return Err(format!("catalog pin `{url}` is not a client object of this release"));
            }
            checked(store, sha256, bytes)
        };
        let artifact = |sha256: &str, bytes: u64, url: &str| -> Result<(), String> {
            if owned.get(sha256) != Some(&bytes) || url != format!("/cell-catalog/objects/{sha256}") {
                return Err(format!("artifact pin `{url}` is not a client object of this release"));
            }
            Ok(())
        };
        let mut bands = BTreeMap::new();
        for reference in &root.cell_index {
            let doc: CellIndexDocument =
                serde_json::from_slice(&pin(&reference.sha256, reference.bytes, &reference.url)?)
                    .map_err(|e| e.to_string())?;
            if doc.schema_version != root.schema_version
                || doc.schema_sha256 != root.schema.sha256
                || doc.band != reference.band
            {
                return Err("band index schema digest or identity differs from root".into());
            }
            for cell in &doc.cells {
                artifact(&cell.sha256, cell.bytes, &cell.url)?;
            }
            if bands.insert(doc.band.clone(), doc).is_some() {
                return Err("duplicate band index".into());
            }
        }
        let block = root.terrain.as_ref().ok_or("maps release has no terrain metadata")?;
        let terrain: TerrainIndexDocument =
            serde_json::from_slice(&pin(&block.cell_index.sha256, block.cell_index.bytes, &block.cell_index.url)?)
                .map_err(|e| e.to_string())?;
        if terrain.schema_version != root.schema_version
            || (terrain.dataset_id.as_str(), terrain.dataset_version.as_str(), terrain.posting_log2, terrain.cell_log2)
                != (block.dataset_id.as_str(), block.dataset_version.as_str(), block.posting_log2, block.cell_log2)
        {
            return Err("terrain index dataset or lattice differs from root".into());
        }
        for cell in &terrain.cells {
            artifact(&cell.sha256, cell.bytes, &cell.url)?;
        }
        let reference = root.articles.as_ref().ok_or("maps release has no article metadata")?;
        let articles: ArticleIndexDocument =
            serde_json::from_slice(&pin(&reference.sha256, reference.bytes, &reference.url)?)
                .map_err(|e| e.to_string())?;
        if articles.schema_version != root.schema_version {
            return Err("article index envelope differs".into());
        }
        let mut article_cells = BTreeMap::new();
        for entry in articles.cells {
            for pin in entry.landmarks.iter().chain(entry.peaks.iter()) {
                artifact(&pin.sha256, pin.bytes, &pin.url)?;
            }
            if article_cells.insert(entry.id.clone(), entry).is_some() {
                return Err("duplicate article cell".into());
            }
        }
        let mut picks = BTreeMap::new();
        for reference in &root.regions {
            let doc: RegionCellsDocument =
                serde_json::from_slice(&pin(&reference.cells_sha256, reference.cells_bytes, &reference.cells_url)?)
                    .map_err(|e| e.to_string())?;
            if doc.schema_version != root.schema_version
                || doc.schema_sha256 != root.schema.sha256
                || doc.region_id != reference.id
            {
                return Err("region index schema digest or identity differs from root".into());
            }
            if picks.insert(doc.region_id.clone(), doc).is_some() {
                return Err("duplicate region selection".into());
            }
        }
        if !picks.contains_key(&release.region) {
            return Err("maps release omits its required region".into());
        }
        Ok(Self { root, bands, terrain, articles: article_cells, picks })
    }

    fn identity(&self, pick: &RegionCellsDocument) -> Result<serde_json::Value, String> {
        let mut files = Vec::new();
        let core =
            self.root.schema.bands.iter().find(|band| band.role == BandRole::Core).ok_or("schema has no core band")?;
        for band in &self.root.schema.bands {
            let doc = self.bands.get(&band.id).ok_or("missing band index")?;
            let index = CoverageIndex::new(
                doc.cells.iter().map(|cell| (cell.id.as_str(), cell)),
                doc.known_empty.iter().map(|run| (run.start.as_str(), run.end.as_str())),
            )?;
            for id in pick.cells.get(&band.id).ok_or("selection omits a band")? {
                match index.get(id)? {
                    Some(IndexedCoverage::Artifact(cell)) => {
                        files.push(serde_json::json!([band.id, id, cell.sha256, cell.partial]))
                    }
                    Some(IndexedCoverage::KnownEmpty) => files.push(serde_json::json!([band.id, id, null])),
                    None => return Err(format!("selection reads missing `{}/{id}`", band.id)),
                }
                if band.id == core.id {
                    if let Some(articles) = self.articles.get(id) {
                        files.push(serde_json::to_value(articles).map_err(|e| e.to_string())?);
                    }
                }
            }
        }
        let terrain = CoverageIndex::new(
            self.terrain.cells.iter().map(|cell| (cell.id.as_str(), cell)),
            self.terrain.known_empty.iter().map(|run| (run.start.as_str(), run.end.as_str())),
        )?;
        for id in &pick.terrain {
            match terrain.get(id)? {
                Some(IndexedCoverage::Artifact(cell)) => files.push(serde_json::json!(["terrain", id, cell.sha256])),
                Some(IndexedCoverage::KnownEmpty) => files.push(serde_json::json!(["terrain", id, null])),
                None => return Err(format!("selection reads missing terrain `{id}`")),
            }
        }
        Ok(serde_json::json!([
            self.root.schema,
            self.root.skins,
            self.terrain.posting_log2,
            self.terrain.cell_log2,
            files
        ]))
    }
}

pub fn verify(previous: Option<&Release>, release: &Release, store: &Store) -> Result<(), String> {
    let current = CatalogInputs::read(release, store)?;
    // Missing previous metadata means all current selections need verification. It never permits
    // a current pin or fetch failure to become an empty selection.
    let previous = previous.and_then(|previous| CatalogInputs::read(previous, store).ok());
    let schema = Schema::parse(&serde_json::to_string(&current.root.schema).map_err(|e| e.to_string())?)?;
    let skin = |id: &str| -> Result<String, String> {
        let skin =
            current.root.skins.iter().find(|skin| skin.id == id).ok_or_else(|| format!("catalog omits `{id}` skin"))?;
        serde_json::to_string(skin).map_err(|e| e.to_string())
    };
    let styles = MapStyles::parse(&skin("default")?, &skin("dusk")?)?;
    for (id, pick) in &current.picks {
        let identity = current.identity(pick)?;
        if previous
            .as_ref()
            .and_then(|previous| previous.picks.get(id).and_then(|pick| previous.identity(pick).ok()))
            .as_ref()
            == Some(&identity)
        {
            continue;
        }
        let dir =
            tempfile::Builder::new().prefix("obc-maps-verify-").tempdir_in(store.root()).map_err(|e| e.to_string())?;
        assemble_pick(&current, pick, store, &schema, &styles, &dir.path().join("map.obcm"))
            .map_err(|e| format!("selection `{id}`: {e}"))?;
    }
    Ok(())
}

/// Materialize the saved region with the same pinned assembly used by verification.
pub(crate) fn assemble(release: &Release, store: &Store, destination: &std::path::Path) -> Result<(), String> {
    let catalog = CatalogInputs::read(release, store)?;
    let pick = catalog.picks.get(&release.region).ok_or("Local catalog has no complete saved region")?;
    let schema = Schema::parse(&serde_json::to_string(&catalog.root.schema).map_err(|e| e.to_string())?)?;
    let skin = |id: &str| -> Result<String, String> {
        serde_json::to_string(catalog.root.skins.iter().find(|skin| skin.id == id).ok_or("catalog lacks a skin")?)
            .map_err(|e| e.to_string())
    };
    let styles = MapStyles::parse(&skin("default")?, &skin("dusk")?)?;
    assemble_pick(&catalog, pick, store, &schema, &styles, destination)
}

fn assemble_pick(
    catalog: &CatalogInputs,
    pick: &RegionCellsDocument,
    store: &Store,
    schema: &Schema,
    styles: &MapStyles,
    destination: &std::path::Path,
) -> Result<(), String> {
    let mut selected = Vec::new();
    let mut empty = Vec::new();
    for (band, ids) in &pick.cells {
        let doc = catalog.bands.get(band).ok_or("selection names unknown band")?;
        let index = CoverageIndex::new(
            doc.cells.iter().map(|cell| (cell.id.as_str(), cell)),
            doc.known_empty.iter().map(|run| (run.start.as_str(), run.end.as_str())),
        )?;
        for id in ids {
            match index.get(id)? {
                Some(IndexedCoverage::Artifact(cell)) => {
                    selected.push((band.clone(), cell.clone(), open(store, &cell.sha256)?))
                }
                Some(IndexedCoverage::KnownEmpty) => {
                    empty.push(KnownEmptyInput { band: band.clone(), id: obcm_assemble::grid::CellId::parse(id)? })
                }
                None => return Err("selection has a map coverage hole".into()),
            }
        }
    }
    let terrain_index = CoverageIndex::new(
        catalog.terrain.cells.iter().map(|cell| (cell.id.as_str(), cell)),
        catalog.terrain.known_empty.iter().map(|run| (run.start.as_str(), run.end.as_str())),
    )?;
    let mut terrain = Vec::new();
    for id in &pick.terrain {
        match terrain_index.get(id)? {
            Some(IndexedCoverage::Artifact(cell)) => terrain.push((cell.clone(), open(store, &cell.sha256)?)),
            Some(IndexedCoverage::KnownEmpty) => {}
            None => return Err("selection has a terrain coverage hole".into()),
        }
    }
    let core =
        catalog.root.schema.bands.iter().find(|band| band.role == BandRole::Core).ok_or("schema has no core band")?;
    let (mut landmarks, mut peaks) = (Vec::new(), Vec::new());
    for id in &pick.cells[&core.id] {
        if let Some(entry) = catalog.articles.get(id) {
            if let Some(pin) = &entry.landmarks {
                landmarks.push(open(store, &pin.sha256)?);
            }
            if let Some(pin) = &entry.peaks {
                peaks.push(open(store, &pin.sha256)?);
            }
        }
    }
    let inputs = selected
        .iter()
        .map(|(band, cell, src)| {
            Ok(CellInput {
                band: band.clone(),
                id: obcm_assemble::grid::CellId::parse(&cell.id)?,
                src,
                partial: cell.partial,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let terrain = TerrainJob {
        params: TerrainParams { posting_log2: catalog.terrain.posting_log2, cell_log2: catalog.terrain.cell_log2 },
        cells: terrain
            .iter()
            .map(|(cell, src)| {
                Ok(TerrainCellInput {
                    id: obcm_assemble::grid::CellId::parse(&cell.id)?,
                    src,
                    sha256: Some(digest(&cell.sha256)?),
                })
            })
            .collect::<Result<Vec<_>, String>>()?,
    };
    let articles = Articles {
        landmarks: landmarks.iter().map(|source| source as &dyn ByteSource).collect(),
        peaks: peaks.iter().map(|source| source as &dyn ByteSource).collect(),
    };
    let mut sink = obcm_assemble::native::FileStore::new(destination).map_err(|e| e.to_string())?;
    let scratch = obcm_assemble::native::FileScratch::new().map_err(|e| e.to_string())?;
    // The pinned selection declares polygon and partial coverage. Every named input is required
    // above; these flags permit the region's shape, never an absent artifact.
    let options = Options { accept_holes: true, accept_partial: true, ..Options::default() };
    assemble_full(inputs, empty, Some(terrain), articles, schema, styles, &options, &mut sink, &NoClock, &scratch)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn checked(store: &Store, sha256: &str, size: u64) -> Result<Vec<u8>, String> {
    let path = store.object(sha256);
    if hash_file(&path)? != (sha256.into(), size) {
        return Err(format!("catalog metadata `{sha256}` failed its digest pin"));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn open(store: &Store, sha256: &str) -> Result<FileSource, String> {
    FileSource::open(&store.object(sha256)).map_err(|e| format!("artifact `{sha256}`: {e}"))
}

fn digest(text: &str) -> Result<[u8; 32], String> {
    if text.len() != 64 || !text.is_ascii() {
        return Err("artifact digest has another length".into());
    }
    let bytes = (0..64)
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).map_err(|_| "invalid artifact digest".to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    bytes.try_into().map_err(|_| "artifact digest has another length".into())
}
