//! Catalog documents from selected engine receipts. Payload identities come from the receipts;
//! only headers and small metadata are read here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use obc_data::engine::Request;
use serde::{Deserialize, Serialize};

use super::coverage::IndexedCoverage;
use super::schema::{parse_schema_doc, skin_styles};
use super::*;
use obc_map_core::grid::{BandTable, CellId};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pick {
    pub id: String,
    pub name: String,
    pub parent: Option<String>,
    pub boundary: Boundary,
    pub cells: BTreeMap<String, Vec<String>>,
    pub terrain: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Options {
    pub sources: Vec<CellSource>,
    /// Band and cell id to whether the source coverage is partial.
    pub coverage: BTreeMap<String, BTreeMap<String, bool>>,
    pub picks: Vec<Pick>,
    pub terrain_cell_log2: u8,
    pub posting_log2: u8,
    pub dataset_version: String,
}

pub fn build(request: &Request) -> Result<(), String> {
    let options: Options = serde_json::from_value(request.options.clone()).map_err(|e| e.to_string())?;
    let mut schema: serde_json::Value =
        serde_json::from_str(obc_map_core::config::CELL_SCHEMA).map_err(|e| e.to_string())?;
    schema["_meta"]["bands"] = serde_json::to_value(BandTable::recommended().bands).map_err(|e| e.to_string())?;
    let schema_body = document_json(&schema);
    let schema = parse_schema_doc(&schema_body, Path::new(SCHEMA_DOC))?;
    let core_log2 =
        schema.bands.iter().find(|band| band.role == BandRole::Core).ok_or("schema has no core band")?.cell_log2;
    let pool = request.output.join("objects");
    fs::create_dir_all(&pool).map_err(|e| e.to_string())?;
    let publish = |body: String| -> Result<ArtifactRef, String> {
        let (bytes, sha256) = hash_str(&body);
        fs::write(pool.join(&sha256), body).map_err(|e| e.to_string())?;
        Ok(ArtifactRef { bytes, url: format!("/cell-catalog/objects/{sha256}"), sha256 })
    };
    let mut bands: BTreeMap<String, Vec<CellEntry>> = BTreeMap::new();
    let mut empty: BTreeMap<String, BTreeSet<CellId>> = BTreeMap::new();
    let mut terrain_cells = Vec::new();
    let mut terrain_empty = BTreeSet::new();
    let mut references = BTreeMap::new();
    let mut articles: BTreeMap<String, ArticleCellEntry> = BTreeMap::new();
    for (layer, metadata) in &request.layer_files {
        let paths = request.layers.get(layer).ok_or_else(|| format!("missing paths for layer `{layer}`"))?;
        if paths.len() != metadata.len() || metadata.iter().any(|file| !paths.contains_key(&file.path)) {
            return Err(format!("paths and receipt metadata differ for `{layer}`"));
        }
        for file in metadata {
            let path = &paths[&file.path];
            let logical = if let Some(name) = file.path.strip_prefix("metadata/") {
                let kind = layer.split('/').nth(1).ok_or("catalog input layer has no kind")?;
                if kind == "terrain" {
                    format!("terrain/{name}")
                } else {
                    format!("cells/{kind}/{name}")
                }
            } else {
                file.path.clone()
            };
            let pin = ArtifactRef {
                bytes: file.size,
                sha256: file.sha256.clone(),
                url: format!("/cell-catalog/objects/{}", file.sha256),
            };
            if let Some(tail) = logical.strip_prefix("cells/") {
                let (band, tail) = tail.split_once('/').ok_or("invalid cell path")?;
                let definition = schema.bands.iter().find(|b| b.id == band).ok_or("unknown cell band")?;
                if tail == "empty.json" {
                    for id in ids(path)? {
                        let id = parse_strict_id(&id)?;
                        if id.log2 != u32::from(definition.cell_log2)
                            || options.coverage.get(band).and_then(|cells| cells.get(&id.to_string())) != Some(&false)
                            || !empty.entry(band.into()).or_default().insert(id)
                        {
                            return Err("invalid, partial or duplicate empty cell".into());
                        }
                    }
                } else {
                    let tail = tail.strip_suffix(".obcm").ok_or("invalid map artifact path")?;
                    let id = parse_strict_id(&format!("{}/{tail}", definition.cell_log2))?;
                    let header = read_obcm_header(path)?;
                    if header.version != obc_formats::obcm::VERSION || header.bbox != id.square() {
                        return Err(format!("cell `{id}` has another format or coverage"));
                    }
                    let partial = options
                        .coverage
                        .get(band)
                        .and_then(|cells| cells.get(&id.to_string()))
                        .copied()
                        .ok_or("cell has no source coverage")?;
                    bands.entry(band.into()).or_default().push(CellEntry {
                        id: id.to_string(),
                        bytes: pin.bytes,
                        sha256: pin.sha256,
                        url: pin.url,
                        sources: options.sources.clone(),
                        partial,
                    });
                }
            } else if let Some(tail) = logical.strip_prefix("terrain/") {
                match tail {
                    "empty.json" => {
                        for id in ids(path)? {
                            let id = parse_strict_id(&id)?;
                            if id.log2 != u32::from(options.terrain_cell_log2) || !terrain_empty.insert(id) {
                                return Err("invalid or duplicate empty terrain cell".into());
                            }
                        }
                    }
                    "credits.json" => {
                        let credits: Vec<ReferenceEntry> = json_file(path)?;
                        for credit in credits {
                            if let Some(previous) = references.insert(credit.key.clone(), credit.clone()) {
                                if previous != credit {
                                    return Err("terrain reference credits disagree".into());
                                }
                            }
                        }
                    }
                    _ => {
                        let tail = tail.strip_suffix(".obcd").ok_or("invalid terrain artifact path")?;
                        let id = parse_strict_id(&format!("{}/{tail}", options.terrain_cell_log2))?;
                        let header = terrain::read_obct_header(path)?;
                        if (
                            header.posting_log2,
                            header.cell_log2,
                            header.min_i as i64,
                            header.min_j as i64,
                            header.rows,
                            header.cols,
                        ) != (options.posting_log2, options.terrain_cell_log2, id.i, id.j, 1, 1)
                        {
                            return Err(format!("terrain cell `{id}` has another lattice or coverage"));
                        }
                        terrain_cells.push(TerrainCellEntry {
                            id: id.to_string(),
                            bytes: pin.bytes,
                            sha256: pin.sha256,
                            url: pin.url,
                        });
                    }
                }
            } else {
                let (kind, tail) = logical.split_once('/').ok_or("invalid article path")?;
                if !["landmarks", "peaks"].contains(&kind) {
                    return Err(format!("unexpected catalog input `{}`", file.path));
                }
                let tail = tail.strip_suffix(".bin").ok_or("invalid article artifact path")?;
                let id = parse_strict_id(&format!("{core_log2}/{tail}"))?.to_string();
                if !options.coverage.values().any(|cells| cells.contains_key(&id)) {
                    return Err("article cell lies outside source coverage".into());
                }
                let entry = articles.entry(id.clone()).or_insert(ArticleCellEntry { id, landmarks: None, peaks: None });
                let slot = if kind == "landmarks" { &mut entry.landmarks } else { &mut entry.peaks };
                if slot.replace(pin).is_some() {
                    return Err("duplicate article cell".into());
                }
            }
        }
    }
    let mut indices = Vec::new();
    let mut documents = BTreeMap::new();
    for band in &schema.bands {
        let mut cells = bands.remove(&band.id).unwrap_or_default();
        cells.sort_by(|a, b| a.id.cmp(&b.id));
        if cells.windows(2).any(|pair| pair[0].id == pair[1].id) {
            return Err("duplicate map cell".into());
        }
        let known_empty = empty
            .remove(&band.id)
            .unwrap_or_default()
            .into_iter()
            .map(|id| KnownEmptyRun { start: id.to_string(), end: id.to_string(), sources: options.sources.clone() })
            .collect::<Vec<_>>();
        let known_empty = compact_map(known_empty)?;
        let doc = CellIndexDocument {
            schema_version: CATALOG_SCHEMA_VERSION,
            schema_sha256: schema.sha256.clone(),
            band: band.id.clone(),
            cells,
            known_empty,
        };
        let lookup = build_band_index(&doc.cells, &doc.known_empty)?;
        let empty_lookup = build_band_index(&[], &doc.known_empty)?;
        for cell in &doc.cells {
            if empty_lookup.get(&cell.id)?.is_some() {
                return Err("empty coverage overlaps a map artifact".into());
            }
        }
        for id in options.coverage.get(&band.id).ok_or("missing band coverage")?.keys() {
            if lookup.get(id)?.is_none() {
                return Err(format!("band `{}` has no coverage for `{id}`", band.id));
            }
        }
        let pin = publish(document_json(&doc))?;
        indices.push(CellIndexRef {
            band: band.id.clone(),
            cell_log2: band.cell_log2,
            cell_count: doc.cells.len() as u32,
            known_empty_count: known_empty_count(&doc.known_empty)?,
            bytes: pin.bytes,
            sha256: pin.sha256,
            url: pin.url,
        });
        documents.insert(band.id.clone(), doc);
    }
    indices.sort_by(|a, b| (b.cell_log2, &a.band).cmp(&(a.cell_log2, &b.band)));
    terrain_cells.sort_by(|a, b| a.id.cmp(&b.id));
    if terrain_cells.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err("duplicate terrain cell".into());
    }
    let terrain_doc = TerrainIndexDocument {
        schema_version: CATALOG_SCHEMA_VERSION,
        dataset_id: "copernicus-glo-30".into(),
        dataset_version: options.dataset_version.clone(),
        posting_log2: options.posting_log2,
        cell_log2: options.terrain_cell_log2,
        cells: terrain_cells,
        known_empty: compact_terrain(terrain_empty)?,
    };
    let terrain_lookup = build_terrain_index(&terrain_doc.cells, &terrain_doc.known_empty)?;
    let empty_lookup = build_terrain_index(&[], &terrain_doc.known_empty)?;
    if terrain_doc.cells.iter().any(|cell| matches!(empty_lookup.get(&cell.id), Ok(Some(_)))) {
        return Err("empty terrain coverage overlaps an artifact".into());
    }
    let pin = publish(document_json(&terrain_doc))?;
    let terrain = TerrainEntry {
        dataset_id: terrain_doc.dataset_id.clone(),
        dataset_version: terrain_doc.dataset_version.clone(),
        posting_log2: options.posting_log2,
        cell_log2: options.terrain_cell_log2,
        attribution: obc_data::sources::attribution("copernicus-glo-30").into(),
        references: (!references.is_empty()).then(|| references.into_values().collect()),
        cell_index: TerrainIndexRef {
            cell_count: terrain_doc.cells.len() as u32,
            known_empty_count: inclusive_run_count(
                terrain_doc.known_empty.iter().map(|run| (run.start.as_str(), run.end.as_str())),
            )?,
            bytes: pin.bytes,
            sha256: pin.sha256,
            url: pin.url,
        },
    };
    let mut regions = Vec::new();
    for pick in &options.picks {
        let mut bytes_by_band = BTreeMap::new();
        let mut count = BTreeMap::new();
        let mut partial = BTreeMap::new();
        let mut article_bytes = 0;
        for band in &schema.bands {
            let selected = pick.cells.get(&band.id).ok_or("region omits a band")?;
            let doc = &documents[&band.id];
            let lookup = build_band_index(&doc.cells, &doc.known_empty)?;
            let (mut bytes, mut partial_count) = (0, 0);
            for id in selected {
                match lookup.get(id)? {
                    Some(IndexedCoverage::Artifact(cell)) => {
                        bytes += cell.bytes;
                        partial_count += u32::from(cell.partial);
                    }
                    Some(IndexedCoverage::KnownEmpty) => {}
                    None => return Err(format!("region `{}` selects missing `{}/{id}`", pick.id, band.id)),
                }
                if band.role == BandRole::Core {
                    if let Some(entry) = articles.get(id) {
                        article_bytes +=
                            entry.landmarks.iter().chain(entry.peaks.iter()).map(|pin| pin.bytes).sum::<u64>();
                    }
                }
            }
            bytes_by_band.insert(band.id.clone(), bytes);
            count.insert(band.id.clone(), selected.len() as u32);
            partial.insert(band.id.clone(), partial_count);
        }
        let (mut terrain_bytes, mut terrain_count, mut terrain_empty_count) = (0, 0, 0);
        for id in &pick.terrain {
            match terrain_lookup.get(id)? {
                Some(IndexedCoverage::Artifact(cell)) => {
                    terrain_bytes += cell.bytes;
                    terrain_count += 1;
                }
                Some(IndexedCoverage::KnownEmpty) => terrain_empty_count += 1,
                None => return Err(format!("region `{}` selects missing terrain `{id}`", pick.id)),
            }
        }
        let doc = RegionCellsDocument {
            schema_version: CATALOG_SCHEMA_VERSION,
            schema_sha256: schema.sha256.clone(),
            region_id: pick.id.clone(),
            cells: pick.cells.clone(),
            terrain: pick.terrain.clone(),
        };
        let body = document_json(&doc);
        let pin = publish(body.clone())?;
        write_named(request, &format!("regions/{}/cells.json", pick.id), body)?;
        regions.push(RegionEntry {
            id: pick.id.clone(),
            name: pick.name.clone(),
            parent: pick.parent.clone(),
            boundary: pick.boundary.clone(),
            bytes: bytes_by_band.values().sum(),
            bytes_by_band,
            article_bytes: Some(article_bytes),
            cell_count: count,
            partial_cell_count_by_band: partial,
            terrain: Some(RegionTerrain {
                bytes: terrain_bytes,
                cell_count: terrain_count,
                known_empty_count: terrain_empty_count,
            }),
            cells_url: pin.url,
            cells_bytes: pin.bytes,
            cells_sha256: pin.sha256,
        });
    }
    regions.sort_by(|a, b| a.id.cmp(&b.id));
    let skins = skins(&schema)?;
    let articles = publish(document_json(&ArticleIndexDocument {
        schema_version: CATALOG_SCHEMA_VERSION,
        cells: articles.into_values().collect(),
    }))?;
    let root = Catalog {
        schema_version: CATALOG_SCHEMA_VERSION,
        source: Some(osm_source()),
        schema: SchemaEntry {
            id: schema.id,
            sha256: schema.sha256,
            name: schema.name,
            description: schema.description,
            obcm_version: obc_formats::obcm::VERSION,
            grid: GridEntry { origin_udeg: GRID_ORIGIN_UDEG, world_side_udeg: WORLD_SIDE_UDEG },
            lods: schema.lods,
            bands: schema.bands,
            styles: schema.styles,
            routing: schema.routing,
            chunk_size: schema.chunk_size,
        },
        skins,
        regions,
        cell_index: indices,
        terrain: Some(terrain),
        landmarks: None,
        articles: Some(articles),
    };
    write_named(request, SCHEMA_DOC, schema_body)?;
    write_named(request, "terrain.json", document_json(root.terrain.as_ref().expect("terrain is published")))?;
    write_named(request, LICENSE_NAME, license_txt(&root))?;
    write_named(request, DEFAULT_MANIFEST_NAME, root_json(&root))
}

fn json_file<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| format!("{}: {e}", path.display()))
}

fn ids(path: &Path) -> Result<Vec<String>, String> {
    json_file(path)
}

fn write_named(request: &Request, name: &str, body: String) -> Result<(), String> {
    let path = request.output.join(name);
    fs::create_dir_all(path.parent().ok_or("document has no parent")?).map_err(|e| e.to_string())?;
    fs::write(path, body).map_err(|e| e.to_string())
}

fn skins(schema: &schema::SchemaDoc) -> Result<Vec<SkinEntry>, String> {
    [
        include_str!("../../../../builder/presets/skins/default.json"),
        include_str!("../../../../builder/presets/skins/dusk.json"),
    ]
    .into_iter()
    .map(|body| {
        let doc: serde_json::Value = serde_json::from_str(body).map_err(|e| e.to_string())?;
        let text =
            |name: &str| doc["_meta"][name].as_str().map(str::to_string).ok_or_else(|| format!("skin has no `{name}`"));
        let id = text("id")?;
        check_skin_document(body, &id)?;
        let config = Config::parse(body)?;
        Ok(SkinEntry {
            id: id.clone(),
            name: text("name")?,
            description: text("description")?,
            version: doc["_meta"]["version"].as_u64().ok_or("skin has no version")? as u32,
            marker_color: config.marker_color,
            styles: skin_styles(&config, schema, Path::new(&id))?,
            preview: None,
        })
    })
    .collect()
}

fn compact_map(runs: Vec<KnownEmptyRun>) -> Result<Vec<KnownEmptyRun>, String> {
    let mut result: Vec<KnownEmptyRun> = Vec::new();
    for run in runs {
        if let Some(previous) = result.last_mut() {
            let end = parse_strict_id(&previous.end)?;
            let start = parse_strict_id(&run.start)?;
            if end.i == start.i && end.j + 1 == start.j && previous.sources == run.sources {
                previous.end = run.end;
                continue;
            }
        }
        result.push(run);
    }
    Ok(result)
}

fn compact_terrain(cells: BTreeSet<CellId>) -> Result<Vec<TerrainEmptyRun>, String> {
    Ok(compact_map(
        cells
            .into_iter()
            .map(|id| KnownEmptyRun { start: id.to_string(), end: id.to_string(), sources: Vec::new() })
            .collect(),
    )?
    .into_iter()
    .map(|run| TerrainEmptyRun { start: run.start, end: run.end })
    .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_data::engine::LayerFile;

    #[test]
    fn empty_metadata_requires_full_source_coverage() {
        let dir = obcm_testkit::scratch::scratch_dir("catalog", "empty-coverage");
        let id = CellId::new(18, 1204, 1052).unwrap().to_string();
        let body = serde_json::to_vec(&vec![id.clone()]).unwrap();
        let path = dir.join("empty.json");
        fs::write(&path, &body).unwrap();
        let mut options = Options {
            sources: vec![CellSource { extract_id: "test".into(), snapshot: "2026-10-01".into() }],
            coverage: BandTable::recommended().bands.into_iter().map(|band| (band.id, BTreeMap::new())).collect(),
            picks: Vec::new(),
            terrain_cell_log2: 19,
            posting_log2: 9,
            dataset_version: "1".into(),
        };
        options.coverage.get_mut("fine").unwrap().insert(id.clone(), false);
        let mut request = Request {
            step: "maps/catalog".into(),
            snapshots: BTreeMap::new(),
            layers: BTreeMap::from([("maps/fine/leaf".into(), BTreeMap::from([("metadata/empty.json".into(), path)]))]),
            layer_files: BTreeMap::from([(
                "maps/fine/leaf".into(),
                vec![LayerFile {
                    path: "metadata/empty.json".into(),
                    size: body.len() as u64,
                    sha256: obc_data::store::sha256_hex(&body),
                }],
            )]),
            options: serde_json::to_value(&options).unwrap(),
            output: dir.join("output"),
            metrics: dir.join("metrics"),
        };
        build(&request).unwrap();
        let root: Catalog = serde_json::from_slice(&fs::read(request.output.join("catalog.json")).unwrap()).unwrap();
        let fine = root.cell_index.iter().find(|band| band.band == "fine").unwrap();
        assert_eq!((fine.cell_count, fine.known_empty_count), (0, 1));
        options.coverage.get_mut("fine").unwrap().insert(id, true);
        request.options = serde_json::to_value(options).unwrap();
        assert!(build(&request).unwrap_err().contains("partial"));
    }
}
