//! The device maps. Each layer covers one leaf of the `2^23` grid that the region touches:
//! `maps/terrain/<i>-<j>` holds the terrain cells of the region in leaf `(i, j)`, `maps/<band>/<i>-<j>`
//! its map cells of one band, and `maps/landmarks/<i>-<j>` and `maps/peaks/<i>-<j>` its landmark
//! and peak artifacts per network cell. Four layers are intermediates, which no client reads:
//! `maps/osm` holds the OSM of each leaf, `maps/reference/<i>-<j>` the national terrain models that
//! the terrain cells of the leaf read, and `maps/landmark-content` and `maps/peak-content` the
//! compiled Wikimedia captures of the region.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use obc_bake::coverage::Coverage;
use obc_data::engine::{snapshot_files, Client, Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{read, version, BlockedLayer, Product, Steps, Unplanned, Wanted};
use obc_data::regions::{Bbox, Regions};
use obc_data::sources::{self, attribution, Source};
use obc_data::store::Store;
use obc_dem::bake::{V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::crest::cell_window;
use obc_dem::step::GLO30;
use obc_draw::step::LAND;
use obc_map_core::grid::{id_width, Band, BandTable, CellId};
use obc_osm::LeafId;
use obc_pack::step::CAPTURES;

mod captured;
pub(crate) mod catalog;
pub(crate) use captured::recipes as fixture_recipes;

/// The cell of the planet bake, and of every device-map layer.
const LEAF_LOG2: u32 = obc_osm::SOURCE_LEAF_LOG2;
pub(crate) const POLY: &str = "geofabrik-poly";
pub const EXTRACTS: &str = "geofabrik-extracts";
/// The names of the GLO-30 tiles: a square that it does not name is sea.
pub const TILE_LIST: &str = "copernicus-glo-30-tiles";

pub struct Maps;

struct RecipeInputs {
    selection: crate::region_sources::Selection,
    tile_list: String,
    land_polygons: Option<String>,
    glo30: String,
    catalog_index: Option<(String, String)>,
    content: Option<Vec<Step>>,
}

impl Product for Maps {
    fn name(&self) -> &'static str {
        "maps"
    }

    fn planning_code(&self, _env: &Env) -> Result<Option<obc_data::engine::OwnerCode>, String> {
        Ok(Some(obc_data::engine::OwnerCode {
            crate_name: "obc-data-steps".into(),
            code: Code {
                paths: [
                    "host/obc-data-steps/src/maps.rs",
                    "host/obc-data-steps/src/maps",
                    "host/obc-data-steps/src/region_sources.rs",
                    "host/obc-data-steps/src/lib.rs",
                ]
                .map(String::from)
                .into(),
                libraries: obc_pack::step::geos_libraries()?,
                ..Default::default()
            },
        }))
    }

    fn portable(&self, step: &Step) -> bool {
        step.name.starts_with("maps/") && !step.client.is_none()
    }

    fn prefix(&self) -> &'static str {
        "cell-catalog"
    }

    fn pointer(&self) -> Option<obc_data::product::PointerFn> {
        Some(catalog::pointer())
    }

    fn named(&self, release: &obc_data::engine::release::Release) -> Result<Vec<obc_data::engine::LayerFile>, String> {
        catalog::named(release)
    }

    fn verify(
        &self,
        _root: &std::path::Path,
        previous: Option<&obc_data::engine::release::Release>,
        release: &obc_data::engine::release::Release,
        store: &Store,
    ) -> Result<(), String> {
        catalog::verify(previous, release, store)
    }

    fn steps(&self, root: &std::path::Path, env: &Env, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        self.steps_with_tool(root, env, regions, store, obc_osm::OsmiumRunner::default().binding())
    }
}

impl Maps {
    /// Plan with an explicit prepared-tool observation. A failed observation blocks OSM layers.
    /// This also lets a copy-based fixture declare its authored tool without changing the process environment.
    pub fn steps_with_tool(
        &self,
        _root: &std::path::Path,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<obc_data::engine::Library, String>,
    ) -> Result<Steps, Unplanned> {
        self.steps_with_bindings(env, regions, store, tool.map(Some), obc_pack::step::geos_libraries())
    }

    pub(crate) fn declarations(
        &self,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<Option<obc_data::engine::Library>, String>,
    ) -> Result<Steps, Unplanned> {
        self.steps_with_bindings(env, regions, store, tool, obc_pack::step::geos_libraries())
    }

    fn steps_with_bindings(
        &self,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<Option<obc_data::engine::Library>, String>,
        libraries: Result<Vec<obc_data::engine::Library>, String>,
    ) -> Result<Steps, Unplanned> {
        let mut wanted = Vec::new();
        let selection = crate::region_sources::resolve(env, regions, store, &mut wanted)?;
        let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
        let land_polygons = snapshot_version(env, store, LAND, &[], &mut wanted)?;
        let body = text(env, store, catalog::INDEX, &[], &mut wanted)?;
        let index_version =
            if body.is_some() { snapshot_version(env, store, catalog::INDEX, &[], &mut wanted)? } else { None };
        let catalog_index = body.zip(index_version);
        // The version of the tiles: the fetch of a tile is in the plan of the step that reads it.
        let glo30 = snapshot_version(env, store, GLO30, &[], &mut wanted)?;
        let (Some(selection), Some(tile_list), Some(glo30), true) = (selection, tile_list, glo30, wanted.is_empty())
        else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        self.recipes(
            env,
            regions,
            store,
            tool,
            libraries,
            RecipeInputs { selection, tile_list, land_polygons, glo30, catalog_index, content: None },
        )
    }

    fn recipes(
        &self,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<obc_data::engine::Library, String>,
        libraries: Result<Vec<obc_data::engine::Library>, String>,
        inputs: RecipeInputs,
    ) -> Result<Steps, Unplanned> {
        let RecipeInputs { selection, tile_list, land_polygons, glo30, catalog_index, content } = inputs;
        let mut wanted = Vec::new();
        let outlines = &selection.outlines;
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let mut steps = Vec::new();
        let mut blocked = Vec::new();
        for (&leaf, cells) in &leaves(outlines, V1_CELL_LOG2.into()) {
            let mut leaf_wanted = Vec::new();
            let reference = match reference(env, store, leaf, cells, &mut leaf_wanted) {
                Ok(reference) => {
                    wanted.extend(leaf_wanted);
                    reference
                }
                Err(Unplanned::Invalid(reason)) => {
                    blocked.push(BlockedLayer { layer: leaf_layer("maps/reference", leaf), reason: reason.clone() });
                    blocked.push(BlockedLayer { layer: leaf_layer("maps/terrain", leaf), reason });
                    continue;
                }
                Err(error) => return Err(error),
            };
            let terrain = terrain(leaf, cells, &land, &glo30, reference.as_ref());
            steps.extend(reference);
            steps.push(terrain);
        }
        if !wanted.is_empty() {
            return Err(Unplanned::NeedsFetch(wanted));
        }
        let land_polygons = land_polygons.ok_or_else(|| Unplanned::Failed("land polygons were not fetched".into()))?;
        let captured_content = content.is_some();
        let mut content_steps = content.unwrap_or_default();
        let mut content_names: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        if captured_content {
            for collection in ["landmarks", "peaks"] {
                content_names.insert(collection, vec![content_layer(collection)]);
            }
        }
        for source in selection.sources.iter().filter(|_| !captured_content) {
            let area = vec![("area".to_string(), source.id.clone())];
            for collection in ["landmarks", "peaks"] {
                let name = if selection.direct {
                    content_layer(collection)
                } else {
                    format!("{}/{}", content_layer(collection), source.id)
                };
                content_names.entry(collection).or_default().push(name.clone());
                if let Err(reason) = &libraries {
                    blocked.push(BlockedLayer { layer: name, reason: reason.clone() });
                    continue;
                }
                let capture = match captures_at(env, store, &source.extract, &area, &mut blocked, (collection, &name)) {
                    Ok(capture) => capture,
                    Err(Unplanned::NeedsFetch(fetches)) => {
                        wanted.extend(fetches);
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                for (_, inputs) in capture {
                    let mut step = content(collection, inputs);
                    step.name = name.clone();
                    add_geos(step, &libraries, &mut content_steps, &mut blocked);
                }
            }
        }
        if !wanted.is_empty() {
            return Err(Unplanned::NeedsFetch(wanted));
        }
        let source_coverage = &selection.coverage;
        let mut osm_leaves = BTreeSet::new();
        let mut network = BTreeMap::new();
        for band in BandTable::recommended().bands {
            let config =
                obc_map_core::config::Config::parse(obc_map_core::config::CELL_SCHEMA).map_err(Unplanned::Failed)?;
            let reads_terrain = obc_map_core::cell::has_contours(&config, &band) || band.has_nav() || band.has_poi();
            let boundary = source_coverage.boundary_cells(band.cell_log2);
            for (leaf, cells) in leaves(outlines, band.cell_log2) {
                osm_leaves.insert(leaf);
                let partial = cells
                    .iter()
                    .filter(|cell| !source_coverage.covers(**cell, &boundary))
                    .map(ToString::to_string)
                    .collect::<Vec<_>>();
                add_geos(
                    map_cells(&band, leaf, &cells, partial, &land_polygons, reads_terrain),
                    &libraries,
                    &mut steps,
                    &mut blocked,
                );
                if band.has_nav() {
                    network.insert(leaf, cells);
                }
            }
        }
        steps.extend(content_steps);
        for collection in ["landmarks", "peaks"] {
            for (&leaf, cells) in &network {
                let mut step = artifacts(collection, leaf, cells);
                if !selection.direct {
                    step.inputs.retain(
                        |input| !matches!(input, Input::Layer { name, .. } if name == &content_layer(collection)),
                    );
                    step.inputs.extend(content_names[collection].iter().map(Input::layer));
                }
                if collection == "landmarks" {
                    add_geos(step, &libraries, &mut steps, &mut blocked);
                } else {
                    steps.push(step);
                }
            }
        }
        let inputs = crate::region_sources::inputs("maps", &selection);
        let extract = if selection.sources.len() == 1 {
            Input::Layer { name: inputs[0].name.clone(), files: vec!["source.osm.pbf".into()] }
        } else {
            match tool
                .as_ref()
                .map_err(|reason| Unplanned::Invalid(reason.clone()))
                .and_then(|tool| crate::region_sources::combined("maps/region-osm", &inputs, tool.as_ref()))
            {
                Ok(combined) => steps.push(combined),
                Err(Unplanned::Invalid(reason)) => {
                    blocked.push(BlockedLayer { layer: "maps/region-osm".into(), reason })
                }
                Err(error) => return Err(error),
            }
            Input::Layer { name: "maps/region-osm".into(), files: vec!["osm.pbf".into()] }
        };
        steps.extend(inputs);
        match tool {
            Ok(tool) => steps.push(osm(extract, &osm_leaves, tool)),
            Err(reason) => blocked.push(BlockedLayer { layer: "maps/osm".into(), reason }),
        }
        if blocked.is_empty() {
            if catalog_index.is_none() && selection.sources.iter().any(|source| source.prepared.is_none()) {
                return Err(Unplanned::Failed("catalog index was not fetched".into()));
            }
            match catalog::step(
                env,
                regions.get(&env.region).expect("the environment names a region"),
                store,
                &selection,
                &steps,
                &glo30,
                catalog_index.as_ref().map(|(body, version)| (version.clone(), body.as_str())),
            ) {
                Ok(step) => steps.push(step),
                Err(Unplanned::Invalid(reason)) => blocked.push(BlockedLayer { layer: catalog::LAYER.into(), reason }),
                Err(error) => return Err(error),
            }
        } else {
            blocked.push(BlockedLayer {
                layer: catalog::LAYER.into(),
                reason: format!(
                    "catalog requires all map, terrain and article layers: {}",
                    blocked.iter().map(|layer| layer.reason.as_str()).collect::<Vec<_>>().join("; ")
                ),
            });
        }
        let mut listed = Steps { steps, blocked };
        listed.block_dependents();
        Ok(listed)
    }
}

fn add_geos(
    mut step: Step,
    libraries: &Result<Vec<obc_data::engine::Library>, String>,
    steps: &mut Vec<Step>,
    blocked: &mut Vec<BlockedLayer>,
) {
    match libraries {
        Ok(libraries) => {
            step.code.libraries = libraries.clone();
            steps.push(step);
        }
        Err(reason) => blocked.push(BlockedLayer { layer: step.name, reason: reason.clone() }),
    }
}

/// The capture of one area's collection with its exact held extract and polygon inputs.
fn captures_at(
    env: &Env,
    store: &Store,
    extract: &str,
    area: &[(String, String)],
    blocked: &mut Vec<BlockedLayer>,
    selected: (&'static str, &str),
) -> Result<Vec<(&'static str, Vec<Input>)>, Unplanned> {
    let poly = version(env, store, POLY, area).map_err(Unplanned::Failed)?;
    let poly = poly.map_err(|_| Unplanned::Failed(format!("the store has no {POLY} of {area:?}")))?;
    let now =
        [(EXTRACTS, extract), (POLY, poly.as_str())].map(|(source, version)| file(env, store, source, version, area));
    let [osm, poly] = now.map(|file| file.map(|(_, sha256)| format!("sha256:{sha256}")));
    let (osm, poly) = (osm?, poly?);
    let (collection, layer) = selected;
    let layer = layer.to_string();
    let (params, read) = match capture_params_at(env, store, collection, area, (&osm, &poly), &layer) {
        Ok(params) => params,
        Err(Unplanned::Invalid(reason)) => {
            blocked.push(BlockedLayer { layer: layer.clone(), reason });
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    if let Some((_, error)) = env
        .fetch_failures
        .iter()
        .find(|(wanted, _)| CAPTURES.contains(&wanted.source.as_str()) && wanted.params == params)
    {
        blocked.push(BlockedLayer {
            layer: layer.clone(),
            reason: format!("capture fetch failed: {error}; plan with `--move wikidata`"),
        });
        return Ok(Vec::new());
    }
    let mut inputs = Vec::new();
    let mut missing = Vec::new();
    for source in CAPTURES {
        if let Some(version) = snapshot_version(env, store, source, &params, &mut missing)? {
            if snapshot_files(store, source, &version, &params, &[]).map_err(Unplanned::Failed)?.is_none() {
                missing.push(Wanted { source: source.into(), version: Some(version), params: params.clone() });
                continue;
            }
            let params = params.clone();
            inputs.push(Input::Snapshot { source: source.into(), version, params, files: Vec::new() });
        }
    }
    if !missing.is_empty() {
        let retained = missing.iter().all(|wanted| {
            wanted.version.as_ref().is_some_and(|version| {
                env.retained.iter().any(|read| {
                    read.key.source == wanted.source
                        && read.key.version == *version
                        && read.params == obc_data::store::sorted(&wanted.params)
                })
            })
        });
        if retained || CAPTURES.iter().any(|source| env.moves.contains_key(*source)) {
            return Err(Unplanned::NeedsFetch(missing));
        } else {
            let reason = format!("{collection} capture missing; plan with `--move wikidata`");
            blocked.push(BlockedLayer { layer: layer.clone(), reason });
        }
        return Ok(Vec::new());
    }
    inputs.extend(read);
    Ok(vec![(collection, inputs)])
}

/// The params of a fetch.
type Params = Vec<(String, String)>;

/// The params of the capture of `collection` for the region `area`, and the extract and the `.poly`
/// that it reads: those of the capture that `env` reads (a saved plan, or live), or else of the
/// newest capture in the store. Missing inputs or stale discovery code block the capture until
/// an explicit source move. A new extract alone asks for no new capture. A moved capture reads
/// the extract and the `.poly` of `now`, with the code of now.
#[cfg(test)]
fn capture_params(
    env: &Env,
    store: &Store,
    collection: &str,
    area: &[(String, String)],
    now: (&str, &str),
) -> Result<(Params, Vec<Input>), Unplanned> {
    capture_params_at(env, store, collection, area, now, &content_layer(collection))
}

fn capture_params_at(
    env: &Env,
    store: &Store,
    collection: &str,
    area: &[(String, String)],
    now: (&str, &str),
    layer: &str,
) -> Result<(Params, Vec<Input>), Unplanned> {
    let value = |params: &[(String, String)], name: &str| {
        params.iter().find(|(param, _)| param == name).map(|(_, value)| value.clone())
    };
    let ours = |params: &[(String, String)]| {
        value(params, "collection").as_deref() == Some(collection)
            && value(params, "area").as_deref() == Some(area[0].1.as_str())
    };
    let mut kept = None;
    let moved = CAPTURES.iter().any(|source| env.moves.contains_key(*source) && !env.stale.contains(*source));
    if !moved {
        let named: Vec<&Vec<(String, String)>> = match &env.planned {
            Some(planned) => planned.keys().filter(|(source, _)| source == CAPTURES[0]).map(|(_, p)| p).collect(),
            None => env.live.keys().filter(|(source, _)| source == CAPTURES[0]).map(|(_, p)| p).collect(),
        };
        kept = named.into_iter().find(|params| ours(params)).cloned();
        if kept.is_none() && env.planned.is_none() {
            let stored = store.requests_of(CAPTURES[0]).map_err(Unplanned::Failed)?;
            kept = stored
                .into_iter()
                .filter(|request| ours(&request.params))
                .max_by(|a, b| a.version.cmp(&b.version))
                .map(|request| request.params);
        }
    }
    // The extract and the `.poly` that the capture read, which need not be those of now. Their
    // input names the file, not the params: a fetch has one version in live.
    let reads = |osm: &str, poly: &str, held: bool| -> Result<Option<Vec<Input>>, Unplanned> {
        let mut inputs = Vec::new();
        for (source, digest) in [(EXTRACTS, osm), (POLY, poly)] {
            let Some((version, file)) =
                by_digest(env, store, source, area, digest, held.then_some((layer, collection)))?
            else {
                return Ok(None);
            };
            inputs.push(Input::Snapshot { source: source.into(), version, params: Vec::new(), files: vec![file] });
        }
        Ok(Some(inputs))
    };
    if let Some(params) = kept {
        if !moved
            && CAPTURES
                .iter()
                .any(|source| env.stale_requests.contains(&(source.to_string(), obc_data::store::sorted(&params))))
        {
            return Err(invalid(format!("{collection} capture stale; plan with `--move wikidata`")));
        }
        for source in CAPTURES {
            let _ = version(env, store, source, &params).map_err(Unplanned::Failed)?;
        }
        let (osm, poly) = (value(&params, "osm").unwrap_or_default(), value(&params, "poly").unwrap_or_default());
        if let Some(read) = reads(&osm, &poly, true)? {
            return Ok((params, read));
        }
        return Err(invalid(format!("{collection} capture inputs missing; plan with `--move wikidata`")));
    }
    let read = reads(now.0, now.1, false)?
        .ok_or(Unplanned::Failed(format!("the store has no extract or `.poly` of {area:?}")))?;
    let code = obc_pack::step::capture_code().map_err(Unplanned::Invalid)?;
    let pairs = [("collection", collection), ("area", &area[0].1), ("osm", now.0), ("poly", now.1), ("code", &code)];
    Ok((pairs.map(|(name, value)| (name.to_string(), value.to_string())).to_vec(), read))
}

/// The name and the SHA-256 of the one file that the fetch of `source@version` with `params` gave.
fn file(
    env: &Env,
    store: &Store,
    source: &str,
    version: &str,
    params: &[(String, String)],
) -> Result<(String, String), Unplanned> {
    let missing = || Unplanned::Failed(format!("the store has no file of {source}@{version} {params:?}"));
    let names = store.requested(source, version, params).map_err(Unplanned::Failed)?;
    let snapshot = store.snapshot(source, version).map_err(Unplanned::Failed)?;
    if let (Some(names), Some(snapshot)) = (names, snapshot) {
        let file = match &names[..] {
            [name] => snapshot.files.into_iter().find(|file| &file.name == name).ok_or_else(missing)?,
            _ => {
                return Err(Unplanned::Failed(format!(
                    "a fetch of {source} with {params:?} gives {} files",
                    names.len()
                )));
            }
        };
        return Ok((file.name, file.sha256));
    }
    let params = obc_data::store::sorted(params);
    let files: BTreeSet<_> = env
        .retained
        .iter()
        .filter(|read| read.key.source == source && read.key.version == version && read.params == params)
        .flat_map(|read| &read.record.files)
        .map(|file| (file.name.clone(), file.sha256.clone(), file.url.clone(), file.size))
        .collect();
    match files.into_iter().collect::<Vec<_>>().as_slice() {
        [(name, digest, _, _)] => Ok((name.clone(), digest.clone())),
        _ => Err(missing()),
    }
}

/// The version and the name of the file of a fetch of `source` with `params` whose digest is
/// `sha256:<hex>`, or `None` when the store has it no more: a clean keeps the request record of a
/// version whose snapshot record it deletes.
fn by_digest(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
    digest: &str,
    held_scope: Option<(&str, &str)>,
) -> Result<Option<(String, String)>, Unplanned> {
    let area = params.iter().find(|(name, _)| name == "area").map(|(_, value)| value.as_str());
    let mut retained: Vec<_> = env
        .retained
        .iter()
        .filter(|read| read.key.source == source && held_scope.is_some_and(|(step, _)| step == read.step))
        .collect();
    if retained.is_empty() {
        if let Some((_, collection)) = held_scope {
            fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
                params.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
            }
            let pin = if source == POLY { "poly" } else { "osm" };
            let consumers: BTreeSet<_> = env
                .retained
                .iter()
                .filter(|read| {
                    CAPTURES.contains(&read.key.source.as_str())
                        && param(&read.params, "collection") == Some(collection)
                        && param(&read.params, "area") == area
                        && param(&read.params, pin) == Some(digest)
                })
                .map(|read| &read.step)
                .collect();
            retained = env
                .retained
                .iter()
                .filter(|read| read.key.source == source && consumers.contains(&read.step))
                .collect();
        }
    }
    let held: BTreeSet<_> = retained
        .iter()
        .copied()
        .flat_map(|read| read.record.files.iter().map(move |file| (read, file)))
        .filter(|(_, file)| digest.strip_prefix("sha256:") == Some(file.sha256.as_str()))
        .map(|(read, file)| (read.key.version.clone(), file.name.clone()))
        .collect();
    if held.len() > 1 {
        return Err(Unplanned::Failed(format!("{source}: retained capture input {digest} has ambiguous versions")));
    }
    if let Some(held) = held.into_iter().next() {
        return Ok(Some(held));
    }
    if !retained.is_empty() {
        return Err(Unplanned::Failed(format!(
            "{source}: retained capture input {digest} disagrees with its content layer"
        )));
    }
    for request in store.requests(source, params).map_err(Unplanned::Failed)? {
        if store.snapshot(source, &request.version).map_err(Unplanned::Failed)?.is_none() {
            continue;
        }
        let (name, sha256) = file(env, store, source, &request.version, params)?;
        if digest.strip_prefix("sha256:") == Some(sha256.as_str()) {
            return Ok(Some((request.version, name)));
        }
    }
    Ok(None)
}

/// The compiled landmarks or peaks of the region, from their capture, the `.poly` and the extract.
fn content(collection: &str, inputs: Vec<Input>) -> Step {
    let run = match collection {
        "landmarks" => obc_pack::step::landmark_content,
        _ => obc_pack::step::peak_content,
    };
    Step {
        name: content_layer(collection),
        inputs,
        options: serde_json::json!({}),
        code: Code { paths: Vec::new(), crates: vec!["obc-pack".into()], ..Default::default() },
        outputs: vec![collection.into()],
        run: Run::Rust(run),
        client: Client::None,
    }
}

/// `maps/landmark-content` or `maps/peak-content`.
fn content_layer(collection: &str) -> String {
    format!("maps/{}-content", collection.trim_end_matches('s'))
}

/// The landmark or peak artifacts of the network `cells` of one leaf. A landmark joins the OSM
/// objects that name it, so the landmarks read the OSM of the leaf too.
fn artifacts(collection: &str, leaf: LeafId, cells: &[CellId]) -> Step {
    let mut inputs = Vec::new();
    let run = match collection {
        "landmarks" => {
            inputs.push(Input::Layer { name: "maps/osm".into(), files: vec![obc_osm::step::leaf_pbf(leaf)] });
            obc_pack::step::landmarks
        }
        _ => obc_pack::step::peaks,
    };
    inputs.push(Input::layer(content_layer(collection)));
    Step {
        name: leaf_layer(&format!("maps/{collection}"), leaf),
        inputs,
        options: serde_json::json!({
            "cell_log2": cells[0].log2,
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        code: Code { paths: Vec::new(), crates: vec!["obc-pack".into()], ..Default::default() },
        outputs: vec![collection.into()],
        run: Run::Rust(run),
        client: Client::All,
    }
}

/// The cells of size `2^log2` that an outline touches, by leaf.
fn leaves(outlines: &[Coverage], log2: u32) -> BTreeMap<LeafId, Vec<CellId>> {
    let cells: BTreeSet<CellId> = outlines.iter().flat_map(|outline| outline.cells(log2)).collect();
    let mut leaves: BTreeMap<LeafId, Vec<CellId>> = BTreeMap::new();
    let shift = LEAF_LOG2 - log2;
    for cell in cells {
        leaves.entry(LeafId { i: cell.i >> shift, j: cell.j >> shift }).or_default().push(cell);
    }
    leaves
}

/// The name of the layer of `leaf` below `prefix`.
fn leaf_layer(prefix: &str, leaf: LeafId) -> String {
    let width = id_width(LEAF_LOG2);
    format!("{prefix}/{:0width$}-{:0width$}", leaf.i, leaf.j)
}

/// The version of the fetch of `source` with `params` that the step list reads, or `None` while
/// the store has no fetch of it; then `wanted` has its fetch.
fn snapshot_version(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<String>, Unplanned> {
    Ok(match version(env, store, source, params).map_err(Unplanned::Failed)? {
        Ok(version) => Some(version),
        Err(fetch) => {
            wanted.push(fetch);
            None
        }
    })
}

/// The OSM of each leaf: the extract of the region, cut to the square of the leaf and a halo. Only
/// the map cells read it.
fn osm(extract: Input, leaves: &BTreeSet<LeafId>, binding: Option<obc_data::engine::Library>) -> Step {
    Step {
        name: "maps/osm".into(),
        inputs: vec![extract],
        options: serde_json::json!({"leaves": leaves.iter().map(|leaf| [leaf.i, leaf.j]).collect::<Vec<_>>()}),
        code: Code { crates: vec!["obc-osm".into()], libraries: binding.into_iter().collect(), ..Default::default() },
        outputs: vec!["osm".into()],
        run: Run::Rust(obc_osm::step::osm),
        client: Client::None,
    }
}

/// The map cells of one band in one leaf, from the OSM of the leaf, the land polygons and, for a
/// band whose bytes read heights, the terrain of the leaf.
fn map_cells(
    band: &Band,
    leaf: LeafId,
    cells: &[CellId],
    partial: Vec<String>,
    land_polygons: &str,
    reads_terrain: bool,
) -> Step {
    let land_polygons =
        Input::Snapshot { source: LAND.into(), version: land_polygons.into(), params: Vec::new(), files: Vec::new() };
    let osm = Input::Layer { name: "maps/osm".into(), files: vec![obc_osm::step::leaf_pbf(leaf)] };
    let mut inputs = vec![osm];
    if !band.lods.is_empty() {
        inputs.push(land_polygons);
    }
    if reads_terrain {
        inputs.push(Input::layer(leaf_layer("maps/terrain", leaf)));
    }
    Step {
        name: leaf_layer(&format!("maps/{}", band.id), leaf),
        inputs,
        options: serde_json::json!({
            "band": band.id,
            "partial_cells": partial,
            "leaf": [i64::from(LEAF_LOG2), leaf.i, leaf.j],
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        code: Code {
            paths: Vec::new(),
            crates: vec![if band.has_nav() || band.has_poi() { "obc-network".into() } else { "obc-draw".into() }],
            ..Default::default()
        },
        outputs: vec!["cells".into(), "metadata".into()],
        run: Run::Rust(if band.has_nav() || band.has_poi() { obc_network::step::cells } else { obc_draw::step::cells }),
        client: Client::Paths(vec!["cells".into()]),
    }
}

/// The national terrain models whose box meets the terrain cells of `leaf` with the halo of the
/// crest rule, merged into the reference archive of the leaf: the tiles that those cells read. Each
/// model is the fetch of that box; a fetch without files, of a box where the model has no data,
/// adds nothing. `None` when no model has data for the cells, or while the store lacks a fetch;
/// then `wanted` has it. A missing fetch of a model behind a credential that this machine lacks
/// blocks the reference layer and its dependents.
fn reference(
    env: &Env,
    store: &Store,
    leaf: LeafId,
    cells: &[CellId],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Step>, Unplanned> {
    let window_of = |cell: &CellId| {
        let (ci, cj) = (u32::try_from(cell.i).ok()?, u32::try_from(cell.j).ok()?);
        cell_window(ci, cj, V1_POSTING_LOG2, V1_CELL_LOG2)
    };
    let windows: Vec<_> = cells.iter().map(|cell| window_of(cell).expect("a terrain cell has a window")).collect();
    let tiles: BTreeSet<(u32, u32)> = windows.iter().flat_map(|window| window.tiles()).collect();
    let edge = |side: fn(&obc_dem::reference::Window) -> i64, max: bool| {
        let sides = windows.iter().map(side);
        let udeg = if max { sides.max() } else { sides.min() };
        udeg.expect("a leaf has a cell") as f64 / 1e6
    };
    let (west, south) = (edge(|w| w.lon_lo, false), edge(|w| w.lat_lo, false));
    let (east, north) = (edge(|w| w.lon_hi, true), edge(|w| w.lat_hi, true));
    let params = vec![("bbox".to_string(), format!("{west},{south},{east},{north}"))];
    let name = leaf_layer("maps/reference", leaf);
    let (mut inputs, mut models, mut missing) = (Vec::new(), Vec::new(), false);
    let meets = |source: &&Source| source.id.starts_with("dtm-") && source.meets([west, south, east, north]);
    for source in sources::all().iter().filter(meets) {
        match version(env, store, &source.id, &params).map_err(Unplanned::Failed)? {
            Ok(version) => {
                let files = snapshot_files(store, &source.id, &version, &params, &[]).map_err(Unplanned::Failed)?;
                if files.as_ref().is_some_and(|files| files.is_empty()) {
                    continue;
                }
                if files.is_none()
                    && !env.retained.iter().any(|read| {
                        read.key.source == source.id
                            && read.key.version == version
                            && read.params == obc_data::store::sorted(&params)
                    })
                {
                    if let Some(credential) = source.credential.as_ref().filter(|credential| !credential.present()) {
                        return Err(invalid(format!(
                            "{name} reads `{}`, which is blocked: credential missing: {}",
                            source.id,
                            credential.describe()
                        )));
                    }
                }
                models.push(serde_json::json!({
                    "source": source.id,
                    "version": version,
                    "credit": attribution(&source.id),
                }));
                inputs.push(Input::Snapshot {
                    source: source.id.clone(),
                    version,
                    params: params.clone(),
                    files: Vec::new(),
                });
            }
            Err(fetch) => {
                if let Some(credential) = source.credential.as_ref().filter(|credential| !credential.present()) {
                    let source = &source.id;
                    let credential = credential.describe();
                    return Err(invalid(format!(
                        "{name} reads `{source}`, which is blocked: credential missing: {credential}"
                    )));
                }
                wanted.push(fetch);
                missing = true;
            }
        }
    }
    if models.is_empty() || missing {
        return Ok(None);
    }
    let tiles: Vec<String> = tiles.iter().map(|(ti, tj)| format!("{ti:04}/{tj:04}")).collect();
    Ok(Some(Step {
        client: Client::None,
        ..crate::python(
            &name,
            inputs,
            serde_json::json!({"models": models, "tiles": tiles}),
            ("host/obc-dem/reference/merge.py", Some("terrain-reference")),
            &REFERENCE_CODE,
            &["reference"],
        )
    }))
}

/// The code of a reference step besides the uv environment: the merge, the ingest tool that it
/// loads, and the modules of `tools/` that the tool imports.
const REFERENCE_CODE: [&str; 4] =
    ["host/obc-dem/reference/merge.py", "host/obc-dem/reference/ingest", "tools/data_registry.py", "tools/r2.py"];

/// The terrain cells of one leaf, from the GLO-30 tiles that the square of each cell reaches and
/// the reference archive of the leaf. A square that the tile list does not name is sea, and has no
/// tile to read.
fn terrain(leaf: LeafId, cells: &[CellId], land: &HashSet<&str>, glo30: &str, reference: Option<&Step>) -> Step {
    let source_box = |cell: &CellId| obc_bake::terrain::source_bbox([*cell]).expect("a cell has a box");
    let tiles = cells.iter().flat_map(|cell| obc_dem::fetch::tiles_for(source_box(cell)));
    let tiles: BTreeSet<String> = tiles.map(|tile| tile.stem()).filter(|tile| land.contains(tile.as_str())).collect();
    let params: Vec<_> = tiles.into_iter().map(|tile| ("tile".to_string(), tile)).collect();
    // A leaf at sea reads no snapshot: an input without params reads every file of its version.
    let mut inputs = match params.is_empty() {
        true => Vec::new(),
        false => vec![Input::Snapshot { source: GLO30.into(), version: glo30.into(), params, files: Vec::new() }],
    };
    inputs.extend(reference.map(|reference| Input::layer(reference.name.clone())));
    Step {
        name: leaf_layer("maps/terrain", leaf),
        inputs,
        options: serde_json::json!({
            "posting_log2": V1_POSTING_LOG2,
            "cell_log2": V1_CELL_LOG2,
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        code: Code { paths: Vec::new(), crates: vec!["obc-dem".into()], ..Default::default() },
        outputs: vec!["terrain".into(), "metadata".into()],
        run: Run::Rust(obc_dem::step::terrain),
        client: Client::Paths(vec!["terrain".into()]),
    }
}

/// The text of the one file that a fetch of `source` with `params` gives, or `None` while the
/// store lacks it; then `wanted` has its fetch.
pub(crate) fn text(
    env: &Env,
    store: &Store,
    source: &str,
    params: &[(String, String)],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<String>, Unplanned> {
    let files = match read(env, store, source, params).map_err(Unplanned::Failed)? {
        Ok(files) => files,
        Err(fetch) => {
            wanted.push(fetch);
            return Ok(None);
        }
    };
    let [path] = files.values().collect::<Vec<_>>()[..] else {
        let count = files.len();
        return Err(Unplanned::Failed(format!("a fetch of {source} with {params:?} gives {count} files, not one")));
    };
    std::fs::read_to_string(path).map(Some).map_err(|e| Unplanned::Failed(format!("{}: {e}", path.display())))
}

pub(crate) fn invalid(message: String) -> Unplanned {
    Unplanned::Invalid(message)
}

/// A box as an Osmosis `.poly`, the outline that `obc-bake` reads.
pub fn box_poly(bbox: &Bbox) -> String {
    let Bbox { west, south, east, north } = *bbox;
    format!(
        "box\n1\n   {west} {south}\n   {east} {south}\n   {east} {north}\n   {west} {north}\n   {west} {south}\nEND\nEND\n"
    )
}

#[cfg(test)]
pub(crate) mod tests {
    mod code;
    use std::path::{Path, PathBuf};

    use obc_data::engine::plan::{plan, Plan};
    use obc_data::engine::runs::{Context, Limits, Run as RunLog};
    use obc_data::engine::Request;
    use obc_data::fetch::http::Http;
    use obc_data::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot};
    use obc_pack::step::CAPTURES;

    use super::*;
    use obc_data::regions::Area;

    fn map_steps(root: &Path, env: &Env, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        Maps.steps_with_tool(
            root,
            env,
            regions,
            store,
            Ok(obc_data::engine::Library {
                name: "osmium".into(),
                path: std::path::PathBuf::from("/authored-copy-osmium"),
                sha256: "0".repeat(64),
            }),
        )
    }

    pub(crate) fn authored_tool(directory: &Path) -> obc_data::engine::Library {
        std::fs::create_dir_all(directory).unwrap();
        let path = directory.join("authored-osmium");
        std::fs::write(&path, "authored fixture provider").unwrap();
        obc_data::engine::Library {
            name: "osmium".into(),
            path: path.canonicalize().unwrap(),
            sha256: obc_data::store::hash_file(&path).unwrap().0,
        }
    }

    pub(crate) struct Temp(pub(crate) PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn temp(name: &str) -> Temp {
        Temp(std::env::temp_dir().join(format!("obc-data-steps-{name}-{}", std::process::id())))
    }

    pub(crate) fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    /// The Grimsel box of `data/regions/`, which the leaf edge at 8.388608° cuts in two.
    fn grimsel(store: &Store, glo30: &str) -> (Env, Regions) {
        let live = BTreeMap::from([
            ((GLO30.into(), Vec::new()), [glo30.to_string()].into()),
            ((TILE_LIST.into(), Vec::new()), ["1".to_string()].into()),
        ]);
        let env = Env { name: "test".into(), region: "grimsel".into(), live, ..Env::default() };
        let regions = Regions::load(&root()).unwrap();
        let Area::Box { bbox } = &regions.get("grimsel").unwrap().area else { unreachable!() };
        let poly = box_poly(bbox);
        let index = serde_json::json!({"type":"FeatureCollection", "features":[{"type":"Feature",
            "properties":{"id":"grimsel","name":"Grimsel","parent":null,"urls":{"pbf":"https://download.geofabrik.de/europe/grimsel-latest.osm.pbf"}},
            "geometry": serde_json::from_str::<serde_json::Value>(&Coverage::parse_poly(&poly).unwrap().geojson()).unwrap()}]});
        fetched(store, catalog::INDEX, "1", &[], &[("index.json".into(), index.to_string())]);
        let area = [("area".into(), "europe/grimsel".into())];
        fetched(store, POLY, "1", &area, &[("europe/grimsel.poly".into(), poly)]);
        fetched(store, EXTRACTS, "1", &area, &[("europe/grimsel.osm.pbf".into(), "osm".into())]);
        fetched(store, LAND, "1", &[], &[("land.zip".into(), "land".into())]);
        (env, regions)
    }

    const FREIBURG: &str =
        "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";

    /// A Geofabrik region about Freiburg, in leaf `0037-0032`, whose `.poly` the store has, and the
    /// tile list.
    fn freiburg(store: &Store) -> (Env, Regions) {
        let area = [("area".to_string(), "europe/test".to_string())];
        fetched(store, POLY, "1", &area, &[("europe/test.poly".into(), FREIBURG.into())]);
        with_index(store);
        with_tile_list(store, &["N47_00_E007", "N48_00_E007"]);
        let region = obc_data::regions::parse_region(
            "europe/test",
            "name = \"Test\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\n",
        )
        .unwrap();
        let live = BTreeMap::from([
            ((GLO30.into(), Vec::new()), ["1".to_string()].into()),
            ((TILE_LIST.into(), Vec::new()), ["1".to_string()].into()),
        ]);
        let env = Env { name: "test".into(), region: "europe/test".into(), live, ..Env::default() };
        (env, Regions::new(vec![region]).unwrap())
    }

    pub(crate) fn with_index(store: &Store) {
        let index = serde_json::json!({"type":"FeatureCollection", "features":[{"type":"Feature",
            "properties":{"id":"test","name":"Test","parent":null,"urls":{"pbf":"https://download.geofabrik.de/europe/test-latest.osm.pbf"}},
            "geometry":{"type":"Polygon","coordinates":[[[7.79,47.99],[7.82,47.99],[7.82,48.02],[7.79,48.02],[7.79,47.99]]]}}]});
        fetched(store, catalog::INDEX, "1", &[], &[("index-v1.json".into(), index.to_string())]);
    }

    /// The extract of the Freiburg region and the land polygons.
    fn with_osm(store: &Store) {
        let area = [("area".to_string(), "europe/test".to_string())];
        fetched(store, EXTRACTS, "1", &area, &[("europe/test.osm.pbf".into(), "osm".into())]);
        fetched(store, LAND, "1", &[], &[("land-polygons-split-3857.zip".into(), "land".into())]);
    }

    /// The params of the Wikimedia capture of `collection` of the extract and the `.poly` with
    /// these texts.
    fn capture_params(collection: &str, area: &str, osm: &str, poly: &str) -> Vec<(String, String)> {
        vec![
            ("collection".into(), collection.into()),
            ("area".into(), area.into()),
            ("osm".into(), format!("sha256:{}", sha256_hex(osm.as_bytes()))),
            ("poly".into(), format!("sha256:{}", sha256_hex(poly.as_bytes()))),
            ("code".into(), obc_pack::step::capture_code().unwrap()),
        ]
    }

    /// The landmark and peak captures at `version` for the region `area`, of the extract and the
    /// `.poly` with these texts.
    pub(crate) fn captured(store: &Store, version: &str, area: &str, osm: &str, poly: &str) {
        for collection in ["landmarks", "peaks"] {
            let params = capture_params(collection, area, osm, poly);
            let recipe = format!("#{collection}=0/recipe.json");
            for source in CAPTURES {
                fetched(store, source, version, &params, &[(recipe.clone(), version.into())]);
            }
        }
    }

    /// The landmark and peak captures of the Freiburg region at `version`.
    fn with_captures(store: &Store, version: &str) {
        captured(store, version, "europe/test", "osm", FREIBURG);
    }

    /// Add the files `(name, text)` to the record of `source@version`, as a fetch with `params`
    /// gives them.
    pub(crate) fn fetched(
        store: &Store,
        source: &str,
        version: &str,
        params: &[(String, String)],
        files: &[(String, String)],
    ) {
        let mut snapshot = store.snapshot(source, version).unwrap().unwrap_or_else(|| Snapshot {
            source: source.into(),
            version: version.into(),
            files: Vec::new(),
        });
        for (name, text) in files {
            let (sha256, file) = (sha256_hex(text.as_bytes()), store.partial("file"));
            write_atomic(&file, text.as_bytes()).unwrap();
            store.insert(&file, &sha256).unwrap();
            let (url, size) = (format!("https://example.org/{name}"), text.len() as u64);
            if snapshot.file(&url).is_none() {
                snapshot.files.push(FileRecord { name: name.clone(), url, size, sha256, retrieved: String::new() });
            }
        }
        store.put_snapshot(&snapshot).unwrap();
        if !params.is_empty() {
            let names = files.iter().map(|(name, _)| name.clone()).collect();
            let requested = Requested { version: version.into(), params: params.to_vec(), files: names };
            store.put_requested(source, &requested).unwrap();
        }
    }

    /// A store whose tile list names the Grimsel tiles in `land`.
    fn with_tile_list(store: &Store, land: &[&str]) {
        let list: String = land.iter().map(|tile| format!("Copernicus_DSM_COG_10_{tile}_00_DEM\n")).collect();
        fetched(store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list)]);
    }

    /// Record a fetch without files of each national model that the step list asks for, as of a
    /// box where the model has no data.
    pub(crate) fn without_models(store: &Store, env: &Env, regions: &Regions) {
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), env, regions, store) else { return };
        for fetch in wanted.iter().filter(|fetch| fetch.source.starts_with("dtm-")) {
            fetched(store, &fetch.source, "1", &fetch.params, &[]);
        }
    }

    fn terrain_steps(steps: &[Step]) -> impl Iterator<Item = &Step> {
        steps.iter().filter(|step| step.name.starts_with("maps/terrain/"))
    }

    /// Record the GLO-30 tiles that `steps` read at `version`, each with the text `text(tile)`.
    fn glo30_fetched(store: &Store, steps: &[Step], version: &str, text: impl Fn(&str) -> String) {
        for step in terrain_steps(steps) {
            let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
            let files: Vec<_> = params.iter().map(|(_, tile)| (format!("{tile}/{tile}.tif"), text(tile))).collect();
            fetched(store, GLO30, version, params, &files);
        }
    }

    fn fake(request: &Request) -> Result<(), String> {
        std::fs::create_dir(request.output.join("terrain")).map_err(|e| e.to_string())?;
        std::fs::create_dir(request.output.join("metadata")).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("metadata/empty.json"), "[]").map_err(|e| e.to_string())
    }

    fn builds(plan: &Plan) -> Vec<&str> {
        plan.groups.iter().flat_map(|group| &group.builds).map(|build| build.step.as_str()).collect()
    }

    fn tiles(step: &Step) -> Vec<&str> {
        let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
        params.iter().map(|(_, tile)| &tile["Copernicus_DSM_COG_10_".len()..][..11]).collect()
    }

    #[test]
    fn a_saved_id_and_two_area_versions_keep_exact_current_and_held_capture_inputs() {
        let temp = temp("saved-source-area");
        let store = Store::at(temp.0.join("store"));
        let (mut env, _) = freiburg(&store);
        with_osm(&store);
        with_captures(&store, "1");
        env.region = "ride/freiburg".into();
        let region = obc_data::regions::parse_region(
            &env.region,
            "name = \"My ride\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\n",
        )
        .unwrap();
        let regions = Regions::new(vec![region]).unwrap();
        without_models(&store, &env, &regions);
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert!(listed.blocked.is_empty(), "{:?}", listed.blocked);
        let osm = listed.steps.iter().find(|step| step.name == "maps/source/europe/test").unwrap();
        let Input::Snapshot { params, .. } = &osm.inputs[0] else {
            panic!("OSM snapshot");
        };
        assert_eq!(params, &[("area".into(), "europe/test".into())]);
        let catalog = listed.steps.iter().find(|step| step.name == catalog::LAYER).unwrap();
        assert_eq!(catalog.options["sources"][0]["extract_id"], "europe/test");
        let primary =
            catalog.options["picks"].as_array().unwrap().iter().find(|pick| pick["id"] == env.region).unwrap();
        assert_eq!(primary["name"], "My ride");

        fetched(
            &store,
            POLY,
            "1",
            &[("area".into(), "europe/second".into())],
            &[("europe/second.poly".into(), FREIBURG.into())],
        );
        let second = [("area".into(), "europe/second".into())];
        fetched(&store, EXTRACTS, "1", &second, &[("europe/second.osm.pbf".into(), "old second osm".into())]);
        captured(&store, "1", "europe/second", "old second osm", FREIBURG);
        fetched(&store, POLY, "2", &second, &[("europe/second.poly".into(), FREIBURG.replace("47.99", "47.98"))]);
        fetched(&store, EXTRACTS, "2", &second, &[("europe/second-2.osm.pbf".into(), "new second osm".into())]);
        // Include the second source in the index without changing the requested ground.
        let mut index: serde_json::Value =
            serde_json::from_str(&text(&env, &store, catalog::INDEX, &[], &mut Vec::new()).unwrap().unwrap()).unwrap();
        let mut feature = index["features"][0].clone();
        feature["properties"]["id"] = "second".into();
        feature["properties"]["urls"]["pbf"] = "https://download.geofabrik.de/europe/second-latest.osm.pbf".into();
        index["features"].as_array_mut().unwrap().push(feature);
        fetched(&store, catalog::INDEX, "2", &[], &[("index-2.json".into(), index.to_string())]);
        let region = obc_data::regions::parse_region(
            &env.region,
            "name = \"Two areas\"\nkind = \"geofabrik\"\nareas = [\"europe/test\",\"europe/second\"]\n",
        )
        .unwrap();
        let regions = Regions::new(vec![region]).unwrap();
        without_models(&store, &env, &regions);
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert!(listed.blocked.is_empty(), "{:?}", listed.blocked);
        let raw = |name: &str| listed.steps.iter().find(|step| step.name == format!("maps/source/{name}")).unwrap();
        assert!(
            matches!(&raw("europe/test").inputs[0], Input::Snapshot { version, params, .. } if version == "1" && params[0].1 == "europe/test")
        );
        assert!(
            matches!(&raw("europe/second").inputs[0], Input::Snapshot { version, params, .. } if version == "2" && params[0].1 == "europe/second")
        );
        assert!(matches!(&raw("europe/second").inputs[1], Input::Snapshot { version, .. } if version == "2"));
        for collection in ["landmark", "peak"] {
            let content = listed
                .steps
                .iter()
                .find(|step| step.name == format!("maps/{collection}-content/europe/second"))
                .unwrap();
            assert!(content.inputs.iter().any(|input| matches!(input, Input::Snapshot { source, version, params, files } if source == EXTRACTS && version == "1" && params.is_empty() && files == &["europe/second.osm.pbf"])));
            assert!(content.inputs.iter().any(|input| matches!(input, Input::Snapshot { source, version, params, files } if source == POLY && version == "1" && params.is_empty() && files == &["europe/second.poly"])));
        }
        let catalog = listed.steps.iter().find(|step| step.name == catalog::LAYER).unwrap();
        assert_eq!(
            catalog.options["sources"],
            serde_json::json!([
            {"extract_id":"europe/second", "snapshot":"2"}, {"extract_id":"europe/test", "snapshot":"1"}])
        );
        assert!(catalog.options["picks"].as_array().unwrap().iter().any(|pick| pick["id"] == env.region));
        assert!(env.read.borrow().contains_key(&(EXTRACTS.into(), obc_data::store::sorted(&second))));
        env.moves.extend(CAPTURES.map(|source| (source.into(), Some("3".into()))));
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else {
            panic!("explicit capture moves prepare every area's two collections")
        };
        let groups = wanted
            .iter()
            .map(|fetch| {
                let param = |name| fetch.params.iter().find(|(key, _)| key == name).unwrap().1.clone();
                (param("area"), param("collection"), fetch.source.clone())
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(wanted.len(), 12);
        assert_eq!(groups.len(), 12, "each selected request appears once");
        assert!(groups
            .iter()
            .all(|(area, collection, source)| ["europe/test", "europe/second"].contains(&area.as_str())
                && ["landmarks", "peaks"].contains(&collection.as_str())
                && CAPTURES.contains(&source.as_str())));
    }

    #[test]
    fn a_new_glo30_tile_in_one_leaf_rebuilds_that_leaf_only() {
        let temp = temp("leaves");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E007", "N47_00_E008"]);
        let (env, regions) = grimsel(&store, "1");
        without_models(&store, &env, &regions);
        let steps = |version: &str| {
            let (env, regions) = grimsel(&store, version);
            let mut steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
            steps.retain(|step| step.name.starts_with("maps/terrain/"));
            steps.iter_mut().for_each(|step| step.run = Run::Rust(fake));
            steps
        };
        let names: Vec<String> = steps("1").iter().map(|step| step.name.clone()).collect();
        assert_eq!(names, ["maps/terrain/0037-0032", "maps/terrain/0037-0033"], "no model has data there");
        let west = "Copernicus_DSM_COG_10_N46_00_E007_00_DEM";
        glo30_fetched(&store, &steps("1"), "1", |tile| format!("{tile} 1"));
        glo30_fetched(&store, &steps("2"), "2", |tile| format!("{tile} {}", if tile == west { 2 } else { 1 }));

        let (root, http) = (root(), Http::new());
        let context =
            Context { store: &store, root: &root, sources: &[], http: &http, copies: None, limits: Limits::machine() };
        let first = plan(&store, &root, &steps("1")).unwrap();
        assert_eq!(builds(&first), names);
        let mut run = RunLog::create(&store, "build test").unwrap();
        run.build(&context, &steps("1"), &first).unwrap();
        run.finish(None).unwrap();
        assert_eq!(builds(&plan(&store, &root, &steps("2")).unwrap()), [&names[0]], "the new tile is west of the edge");
    }

    #[test]
    fn a_leaf_reads_the_tiles_of_its_cells_that_the_tile_list_names() {
        let temp = temp("tile-list");
        let store = Store::at(temp.0.join("store"));
        let (env, regions) = grimsel(&store, "1");
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else {
            panic!("no tile list")
        };
        assert_eq!(wanted, [Wanted { source: TILE_LIST.into(), version: Some("1".into()), params: Vec::new() }]);

        // As if N47_00_E007 were sea.
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        without_models(&store, &env, &regions);
        let steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
        let terrain: Vec<&Step> = terrain_steps(&steps).collect();
        assert_eq!(tiles(terrain[0]), ["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        // The squares of the eastern cells reach no tile west of 8°.
        assert_eq!(tiles(terrain[1]), ["N46_00_E008", "N47_00_E008"]);
    }

    #[test]
    fn a_band_reads_the_osm_of_its_leaf_the_land_and_the_terrain_when_its_bytes_have_heights() {
        let temp = temp("bands");
        let store = Store::at(temp.0.join("store"));
        let (env, regions) = freiburg(&store);
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else {
            panic!("no extract")
        };
        assert_eq!(wanted.iter().map(|fetch| fetch.source.as_str()).collect::<Vec<_>>(), [EXTRACTS, LAND]);
        with_osm(&store);
        without_models(&store, &env, &regions);
        with_captures(&store, "1");
        let steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
        let reads = |name: &str| -> Vec<String> {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = |input: &Input| match input {
                Input::Snapshot { source, .. } => source.clone(),
                Input::Layer { name, files } => format!("{name} {files:?}"),
            };
            step.inputs.iter().map(read).collect()
        };
        let osm = r#"maps/osm ["osm/0037-0032.osm.pbf"]"#;
        assert_eq!(reads("maps/osm"), ["maps/source/europe/test [\"source.osm.pbf\"]"]);
        let intermediate =
            |name: &str| name == "maps/osm" || name.ends_with("-content") || name.starts_with("maps/source/");
        assert!(
            steps.iter().all(|step| step.client.is_none() == intermediate(&step.name)),
            "no client reads an intermediate"
        );
        assert_eq!(reads("maps/coarse/0037-0032"), [osm, LAND]);
        for band in ["mid", "fine"] {
            assert_eq!(reads(&format!("maps/{band}/0037-0032")), [osm, LAND, "maps/terrain/0037-0032 []"], "{band}");
        }
        assert_eq!(reads("maps/network/0037-0032"), [osm, "maps/terrain/0037-0032 []"]);
        assert!(steps.iter().all(|step| !step.code.paths.iter().any(|path| path == "Cargo.lock")));
    }

    #[test]
    fn unavailable_geos_blocks_its_producers_and_keeps_independent_terrain() {
        for held in [false, true] {
            let temp = temp("geos-blocked");
            let store = Store::at(temp.0.join("store"));
            let (env, regions) = freiburg(&store);
            with_osm(&store);
            without_models(&store, &env, &regions);
            if held {
                with_captures(&store, "1");
            }
            let listed = Maps
                .steps_with_bindings(
                    &env,
                    &regions,
                    &store,
                    Ok(Some(obc_data::engine::Library {
                        name: "osmium".into(),
                        path: std::path::PathBuf::from("/authored-copy-osmium"),
                        sha256: "0".repeat(64),
                    })),
                    Err("GEOS library missing; start a fresh worker".into()),
                )
                .unwrap();
            for name in ["maps/network/0037-0032", "maps/landmark-content", "maps/peak-content", "maps/catalog"] {
                assert!(
                    listed.blocked.iter().any(|layer| layer.layer == name && layer.reason.contains("GEOS")),
                    "{name}"
                );
                assert!(!listed.steps.iter().any(|step| step.name == name), "{name}");
            }
            let terrain = listed.steps.iter().find(|step| step.name == "maps/terrain/0037-0032").unwrap();
            assert!(terrain.code.libraries.is_empty());
            assert!(listed.steps.iter().any(|step| step.name == "maps/osm"));
            assert!(
                !env.requests.borrow().iter().any(|(source, _)| CAPTURES.contains(&source.as_str())),
                "unavailable GEOS starts no capture discovery"
            );
        }
    }

    #[test]
    fn missing_or_failed_capture_blocks_only_its_content_and_artifacts() {
        let temp = temp("capture-blocked");
        let store = Store::at(temp.0.join("store"));
        let (mut env, regions) = freiburg(&store);
        with_osm(&store);
        without_models(&store, &env, &regions);
        let params = capture_params("peaks", "europe/test", "osm", FREIBURG);
        for source in CAPTURES {
            fetched(&store, source, "1", &params, &[("#peaks=0/recipe.json".into(), "1".into())]);
        }
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert_eq!(
            listed.blocked.iter().map(|b| b.layer.as_str()).collect::<Vec<_>>(),
            ["maps/landmark-content", "maps/catalog", "maps/landmarks/0037-0032"]
        );
        assert!(listed.blocked.iter().all(|b| b.reason.contains("--move wikidata")));
        for name in ["maps/terrain/0037-0032", "maps/network/0037-0032", "maps/peak-content", "maps/peaks/0037-0032"] {
            assert!(listed.steps.iter().any(|step| step.name == name), "{name}");
        }
        env.moves.insert("wikidata".into(), Some("1".into()));
        let failed = Wanted {
            source: "wikidata".into(),
            version: None,
            params: capture_params("landmarks", "europe/test", "osm", FREIBURG),
        };
        env.fetch_failures.push((failed, "service unavailable".into()));
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert!(listed.blocked.iter().any(|b| b.reason.contains("service unavailable")));
        assert!(listed.steps.iter().any(|step| step.name == "maps/network/0037-0032"));
    }

    #[test]
    fn stale_capture_requires_a_move_and_explicit_intent_overrides_related_staleness() {
        let temp = temp("capture-code");
        let store = Store::at(temp.0.join("store"));
        let (mut env, regions) = freiburg(&store);
        with_osm(&store);
        without_models(&store, &env, &regions);
        with_captures(&store, "1");
        let current = capture_params("landmarks", "europe/test", "osm", FREIBURG);
        let mut params = current.clone();
        params.iter_mut().find(|(name, _)| name == "code").unwrap().1 = "old".into();
        for source in CAPTURES {
            let requested = Requested {
                version: "1".into(),
                params: params.clone(),
                files: store.requested(source, "1", &current).unwrap().unwrap(),
            };
            store.put_requested(source, &requested).unwrap();
            env.live.insert((source.into(), params.clone()), ["1".into()].into());
        }
        env.requests.borrow_mut().clear();
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert!(!listed.blocked.iter().any(|b| b.layer == "maps/landmark-content"));
        let content = listed.steps.iter().find(|s| s.name == "maps/landmark-content").unwrap();
        for source in CAPTURES {
            assert!(
                content.inputs.iter().any(|input| matches!(input,
                    Input::Snapshot { source: found, version, params: kept, .. }
                        if found == source && version == "1" && kept == &params
                )),
                "{source} retains the exact held request until content checks its pins"
            );
            assert!(env.requests.borrow().contains(&(source.into(), obc_data::store::sorted(&params))));
            assert!(
                !env.requests.borrow().contains(&(source.into(), obc_data::store::sorted(&current))),
                "no replacement capture request is selected"
            );
        }
        assert!(listed.steps.iter().any(|step| step.name == "maps/peak-content"));
        env.stale.insert("wikidata".into());
        env.stale_requests.insert(("wikidata".into(), obc_data::store::sorted(&params)));
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert!(listed.blocked.iter().all(|b| b.reason.contains("capture stale")));
        assert!(listed.steps.iter().any(|step| step.name == "maps/network/0037-0032"));
        assert!(listed.steps.iter().any(|step| step.name == "maps/peak-content"), "fresh collection stays available");
        env.stale = ["wikipedia".into(), "commons".into()].into();
        env.moves.insert("wikidata".into(), None);
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else {
            panic!("an explicit move overrides stale sibling capture blocking")
        };
        assert_eq!(wanted.len(), 2);
        assert!(
            wanted.iter().all(|fetch| fetch.source == "wikidata"),
            "automatic sibling markers do not force newest fetches"
        );
        env.stale.clear();
        env.moves = CAPTURES.map(|source| (source.into(), None)).into();
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else {
            panic!("an explicit move prepares captures")
        };
        assert_eq!(wanted.len(), 6);
        assert!(wanted.iter().any(|fetch| fetch.source == "wikidata"));
    }

    #[test]
    fn a_capture_names_the_extract_and_the_poly_by_digest_and_its_steps_read_it() {
        let temp = temp("captures");
        let store = Store::at(temp.0.join("store"));
        let (mut env, regions) = freiburg(&store);
        with_osm(&store);
        without_models(&store, &env, &regions);
        env.moves.insert("wikidata".into(), None);
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else {
            panic!("no capture")
        };
        let fetch = |collection, source: &str| Wanted {
            source: source.into(),
            version: None,
            params: capture_params(collection, "europe/test", "osm", FREIBURG),
        };
        let captures =
            ["landmarks", "peaks"].into_iter().flat_map(|collection| CAPTURES.map(|source| fetch(collection, source)));
        assert_eq!(wanted, captures.collect::<Vec<_>>());
        with_captures(&store, "1");
        env.moves.clear();
        let steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
        let reads = |name: &str| -> Vec<String> {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = |input: &Input| match input {
                Input::Snapshot { source, .. } => source.clone(),
                Input::Layer { name, .. } => name.clone(),
            };
            step.inputs.iter().map(read).collect()
        };
        let content = ["wikidata", "wikipedia", "commons", EXTRACTS, POLY];
        assert_eq!(reads("maps/landmark-content"), content);
        assert_eq!(reads("maps/peak-content"), content);
        assert_eq!(reads("maps/landmarks/0037-0032"), ["maps/osm", "maps/landmark-content"]);
        assert_eq!(reads("maps/peaks/0037-0032"), ["maps/peak-content"]);
        let network = steps.iter().find(|step| step.name == "maps/network/0037-0032").unwrap();
        let landmarks = steps.iter().find(|step| step.name == "maps/landmarks/0037-0032").unwrap();
        assert_eq!(landmarks.options["cells"], network.options["cells"], "an artifact per network cell");
    }

    /// A new extract asks for no new capture: the compile keeps reading the extract that its
    /// capture read, and the map cells read the new one. An explicit move reads the new extract;
    /// a deleted capture input blocks the content until that move.
    #[test]
    fn a_capture_keeps_its_extract_until_it_moves_or_a_clean_deletes_the_extract() {
        let temp = temp("capture-extract");
        let store = Store::at(temp.0.join("store"));
        let (mut env, regions) = freiburg(&store);
        with_osm(&store);
        with_captures(&store, "1");
        without_models(&store, &env, &regions);
        let area = [("area".to_string(), "europe/test".to_string())];
        for day in ["0", "2"] {
            fetched(&store, EXTRACTS, day, &area, &[(format!("europe/test-{day}.osm.pbf"), format!("osm {day}"))]);
        }
        let clean = |day: &str| std::fs::remove_file(store.root().join(format!("snapshots/{EXTRACTS}/{day}.json")));
        clean("0").unwrap();
        let steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
        let extract = |name: &str| {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = step.inputs.iter().find_map(|input| match input {
                Input::Snapshot { source, version, files, .. } if source == EXTRACTS => Some((version, files)),
                _ => None,
            });
            read.map(|(version, files)| (version.clone(), files.clone()))
        };
        assert_eq!(extract("maps/source/europe/test"), Some(("2".into(), Vec::new())));
        for content in ["maps/landmark-content", "maps/peak-content"] {
            assert_eq!(extract(content), Some(("1".into(), vec!["europe/test.osm.pbf".into()])), "{content}");
        }

        let moved = capture_params("landmarks", "europe/test", "osm 2", FREIBURG);
        let asks_for_a_new_capture = |env: &Env| {
            let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), env, &regions, &store) else {
                panic!("a capture")
            };
            assert!(wanted.iter().any(|fetch| fetch.source == "wikidata" && fetch.params == moved), "{wanted:?}");
        };
        env.moves.insert("wikidata".into(), None);
        asks_for_a_new_capture(&env);
        env.moves.clear();
        clean("1").unwrap();
        let listed = map_steps(&root(), &env, &regions, &store).unwrap();
        assert!(listed.blocked.iter().any(|layer| layer.reason.contains("capture inputs missing")));
        assert!(listed.steps.iter().any(|step| step.name == "maps/network/0037-0032"));
        // The live copy metadata identifies the held input without its local request or bytes.
        for (source, name, text) in
            [(EXTRACTS, "europe/test-260901.osm.pbf", "osm"), (POLY, "europe/test.poly", FREIBURG)]
        {
            let file = obc_data::input_copy::File {
                name: name.into(),
                url: format!("https://example.org/{name}"),
                size: text.len() as u64,
                sha256: sha256_hex(text.as_bytes()),
            };
            env.retained.push(obc_data::input_copy::Retained {
                step: content_layer("landmarks"),
                key: obc_data::input_copy::Key {
                    source: source.into(),
                    version: "1".into(),
                    digest: obc_data::engine::digest([(file.name.as_str(), file.sha256.as_str())]),
                },
                params: Vec::new(),
                record: obc_data::input_copy::Record { source: source.into(), version: "1".into(), files: vec![file] },
            });
        }
        let params = capture_params("landmarks", "europe/test", "osm", FREIBURG);
        for source in CAPTURES {
            env.live.insert((source.into(), params.clone()), ["1".into()].into());
        }
        let fresh = Store::at(temp.0.join("fresh"));
        env.requests.borrow_mut().clear();
        let (params, inputs) =
            super::capture_params(&env, &fresh, "landmarks", &area, ("new osm", "new poly")).unwrap();
        assert_eq!(params, capture_params("landmarks", "europe/test", "osm", FREIBURG));
        assert_eq!(env.requests.borrow().len(), CAPTURES.len(), "fresh-store capture lookup remains active");
        assert!(
            env.requests.borrow().iter().all(|(source, _)| CAPTURES.contains(&source.as_str())),
            "held extract and poly lookups are provenance only"
        );
        assert!(inputs.iter().all(|input| matches!(input, Input::Snapshot { version, params, files, .. } if version == "1" && params.is_empty() && files.len() == 1)));
        assert!(!fresh.root().exists(), "metadata lookup downloads no old extract or poly");
        // Request metadata identifies the collection's consumer across a region edit.
        let file = store
            .snapshot(CAPTURES[0], "1")
            .unwrap()
            .unwrap()
            .files
            .into_iter()
            .find(|file| file.name.starts_with("#landmarks"))
            .unwrap();
        let file = obc_data::input_copy::File { name: file.name, url: file.url, size: file.size, sha256: file.sha256 };
        env.retained.push(obc_data::input_copy::Retained {
            step: content_layer("landmarks"),
            params: params.clone(),
            key: obc_data::input_copy::Key {
                source: CAPTURES[0].into(),
                version: "1".into(),
                digest: obc_data::engine::digest([(file.name.as_str(), file.sha256.as_str())]),
            },
            record: obc_data::input_copy::Record { source: CAPTURES[0].into(), version: "1".into(), files: vec![file] },
        });
        let (kept, reads) = capture_params_at(
            &env,
            &fresh,
            "landmarks",
            &area,
            ("new osm", "new poly"),
            "maps/landmark-content/europe/test",
        )
        .unwrap();
        assert_eq!(kept, params);
        assert!(reads.iter().any(|input| matches!(input, Input::Snapshot { source, files, .. } if source == EXTRACTS && files == &["europe/test-260901.osm.pbf"])));
        assert!(reads.iter().all(|input| matches!(input, Input::Snapshot { version, .. } if version == "1")));
        assert!(!fresh.root().exists(), "selection changes load no held payload");
        let mut peak_poly = env.retained[1].clone();
        peak_poly.step = content_layer("peaks");
        peak_poly.key.version = "2".into();
        peak_poly.record.version = "2".into();
        let mut peak_osm = env.retained[0].clone();
        peak_osm.step = content_layer("peaks");
        env.retained.extend([peak_osm, peak_poly.clone()]);
        let peak_params = capture_params("peaks", "europe/test", "osm", FREIBURG);
        for source in CAPTURES {
            env.live.insert((source.into(), peak_params.clone()), ["1".into()].into());
        }
        let (_, peak_inputs) = super::capture_params(&env, &fresh, "peaks", &area, ("new osm", "new poly")).unwrap();
        assert!(peak_inputs
            .iter()
            .any(|input| matches!(input, Input::Snapshot { source, version, .. } if source == POLY && version == "2")));
        let (_, landmark_inputs) =
            super::capture_params(&env, &fresh, "landmarks", &area, ("new osm", "new poly")).unwrap();
        assert!(landmark_inputs
            .iter()
            .any(|input| matches!(input, Input::Snapshot { source, version, .. } if source == POLY && version == "1")));
        peak_poly.step = content_layer("landmarks");
        env.retained.push(peak_poly);
        assert!(
            matches!(super::capture_params(&env, &fresh, "landmarks", &area, ("new osm", "new poly")), Err(Unplanned::Failed(message)) if message.contains("ambiguous versions"))
        );
    }

    /// A refresh rebuilds the layers that read the refreshed source: a step that reads none of it,
    /// directly or through another layer, keeps its key.
    #[test]
    fn a_capture_refresh_rebuilds_no_map_cell_and_an_osm_refresh_no_terrain_cell() {
        let sources = obc_data::sources::parse_sources(include_str!("../../../data/sources.toml")).unwrap();
        let osm =
            |id: &str| sources.iter().any(|source| source.id == id && source.licence.as_deref() == Some("ODbL-1.0"));
        let temp = temp("refresh");
        let store = Store::at(temp.0.join("store"));
        let (env, regions) = freiburg(&store);
        with_osm(&store);
        with_captures(&store, "1");
        without_models(&store, &env, &regions);
        let steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
        let by_name: BTreeMap<&str, &Step> = steps.iter().map(|step| (step.name.as_str(), step)).collect();
        let reads = |step: &Step| {
            let (mut read, mut pending) = (BTreeSet::new(), vec![step]);
            while let Some(step) = pending.pop() {
                for input in &step.inputs {
                    match input {
                        Input::Snapshot { source, .. } => drop(read.insert(source.clone())),
                        Input::Layer { name, .. } => pending.push(by_name[name.as_str()]),
                    }
                }
            }
            read
        };
        let named =
            |prefix: &str| -> Vec<&Step> { steps.iter().filter(|step| step.name.starts_with(prefix)).collect() };
        let cells = ["coarse", "mid", "fine", "network"].map(|band| format!("maps/{band}/"));
        for step in cells.iter().flat_map(|prefix| named(prefix)) {
            assert!(!reads(step).iter().any(|source| CAPTURES.contains(&source.as_str())), "{}", step.name);
        }
        assert!(!named("maps/terrain/").is_empty());
        for step in named("maps/terrain/") {
            assert!(!reads(step).iter().any(|source| osm(source)), "{}", step.name);
        }
        for step in named("maps/landmarks/").into_iter().chain(named("maps/peaks/")) {
            assert!(CAPTURES.iter().all(|source| reads(step).contains(*source)), "{}", step.name);
        }
    }

    #[test]
    fn a_leaf_merges_each_national_model_whose_box_meets_its_cells() {
        let temp = temp("models");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E008"]);
        let (env, regions) = grimsel(&store, "1");
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &env, &regions, &store) else { panic!("no model") };
        // Switzerland, and France, whose box reaches 9.6° east: each fetched with the box of a leaf.
        let fetches: BTreeSet<(&str, &str)> =
            wanted.iter().map(|fetch| (fetch.source.as_str(), fetch.params[0].1.as_str())).collect();
        let boxes: BTreeSet<&str> = fetches.iter().map(|(_, bbox)| *bbox).collect();
        assert_eq!((fetches.len(), boxes.len()), (4, 2), "{fetches:?}");
        assert!(fetches.iter().all(|(source, _)| ["dtm-ch", "dtm-fr"].contains(source)));

        // Data of Switzerland, and none of France.
        let dem = [("dem.tif".to_string(), "dem".to_string())];
        let swiss = wanted.iter().filter(|fetch| fetch.source == "dtm-ch");
        swiss.for_each(|fetch| fetched(&store, &fetch.source, "1", &fetch.params, &dem));
        without_models(&store, &env, &regions);
        let steps = map_steps(&root(), &env, &regions, &store).unwrap().steps;
        let reference = steps.iter().find(|step| step.name == "maps/reference/0037-0033").unwrap();
        let models = reference.options["models"].as_array().unwrap();
        let read: Vec<(&str, &str)> = models
            .iter()
            .map(|model| (model["source"].as_str().unwrap(), model["version"].as_str().unwrap()))
            .collect();
        assert_eq!(read, [("dtm-ch", "1")]);
        assert_eq!(reference.inputs.len(), 1, "a model without data adds nothing");
        assert_eq!(models[0]["credit"], "© swisstopo");
        assert!(reference.client.is_none(), "only the terrain step reads the national models");
        assert!(!reference.options["tiles"].as_array().unwrap().is_empty());
        let terrain = steps.iter().find(|step| step.name == "maps/terrain/0037-0033").unwrap();
        assert!(matches!(terrain.inputs.last(), Some(Input::Layer { name, .. }) if *name == reference.name));
    }

    #[test]
    fn a_model_whose_credential_this_machine_lacks_blocks_its_reference_and_terrain() {
        let temp = temp("credential");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &[]);
        // The leaf meets Niedersachsen first, then Denmark, whose credential is required.
        let region = "name = \"Jutland\"\nkind = \"box\"\nbox = [8.2, 53.8, 8.3, 56.1]\n";
        let regions = Regions::new(vec![obc_data::regions::parse_region("jutland", region).unwrap()]).unwrap();
        let (env, _) = grimsel(&store, "1");
        let env = Env { region: "jutland".into(), ..env };
        let bounds = Bbox { west: 8.2, south: 53.8, east: 8.3, north: 56.1 };
        let poly = box_poly(&bounds);
        let coverage = Coverage::parse_poly(&poly).unwrap();
        let index = serde_json::json!({"type":"FeatureCollection", "features":[{"type":"Feature",
            "properties":{"id":"jutland","name":"Jutland","parent":null,
                "urls":{"pbf":"https://download.geofabrik.de/europe/jutland-latest.osm.pbf"}},
            "geometry":serde_json::from_str::<serde_json::Value>(&coverage.geojson()).unwrap()}]});
        fetched(&store, catalog::INDEX, "2", &[], &[("index-2.json".into(), index.to_string())]);
        let area = [("area".into(), "europe/jutland".into())];
        fetched(&store, POLY, "1", &area, &[("europe/jutland.poly".into(), poly.clone())]);
        fetched(&store, EXTRACTS, "1", &area, &[("europe/jutland.osm.pbf".into(), "osm".into())]);
        captured(&store, "1", "europe/jutland", "osm", &poly);
        let token = sources::embedded("dtm-dk").credential.as_ref().unwrap();
        match map_steps(&root(), &env, &regions, &store) {
            Ok(listed) => {
                assert!(!token.present());
                for layer in ["maps/reference/0038-0032", "maps/terrain/0038-0032"] {
                    assert!(listed
                        .blocked
                        .iter()
                        .any(|b| b.layer == layer && b.reason.contains("OBC_REFERENCE_DK_TOKEN")));
                }
                assert!(listed.blocked.iter().any(|b| b.layer == "maps/catalog"));
                assert!(!listed
                    .steps
                    .iter()
                    .any(|step| step.name == "maps/catalog" || step.name == "maps/network/0038-0032"));
                assert!(
                    listed.steps.iter().any(|step| step.name == "maps/coarse/0038-0032"),
                    "independent bands stay usable"
                );
                let mut restored_env = env.clone();
                let (leaf, cells) = leaves(&[coverage], V1_CELL_LOG2.into()).into_iter().next().unwrap();
                // Pin the request before reference preflight; the missing credential matters only upstream.
                let windows: Vec<_> = cells
                    .iter()
                    .map(|c| cell_window(c.i as u32, c.j as u32, V1_POSTING_LOG2, V1_CELL_LOG2).unwrap())
                    .collect();
                let bbox = format!(
                    "{},{},{},{}",
                    windows.iter().map(|w| w.lon_lo).min().unwrap() as f64 / 1e6,
                    windows.iter().map(|w| w.lat_lo).min().unwrap() as f64 / 1e6,
                    windows.iter().map(|w| w.lon_hi).max().unwrap() as f64 / 1e6,
                    windows.iter().map(|w| w.lat_hi).max().unwrap() as f64 / 1e6
                );
                let params = vec![("bbox".to_string(), bbox)];
                for source in ["dtm-de-ni", "dtm-dk"] {
                    restored_env.live.insert((source.into(), params.clone()), ["1".into()].into());
                    restored_env.retained.push(obc_data::input_copy::Retained {
                        step: leaf_layer("maps/reference", leaf),
                        key: obc_data::input_copy::Key {
                            source: source.into(),
                            version: "1".into(),
                            digest: obc_data::engine::digest([]),
                        },
                        params: params.clone(),
                        record: obc_data::input_copy::Record {
                            source: source.into(),
                            version: "1".into(),
                            files: Vec::new(),
                        },
                    });
                }
                let mut wanted = Vec::new();
                let restored = reference(&restored_env, &store, leaf, &cells, &mut wanted).unwrap().unwrap();
                assert!(wanted.is_empty());
                assert_eq!(
                    restored.inputs.len(),
                    2,
                    "verified retained reads defer the credential check until restoration"
                );
                assert_eq!(plan(&store, &root(), &[restored]).unwrap().fetches().len(), 2);
            }
            // A machine with the token fetches the model.
            Err(Unplanned::NeedsFetch(wanted)) => {
                assert!(token.present());
                assert!(wanted.iter().any(|fetch| fetch.source == "dtm-dk"));
                assert!(wanted.iter().any(|fetch| fetch.source == "dtm-de-ni"));
            }
            other => panic!("{:?}", other.map(|steps| steps.steps.len())),
        }
    }

    /// Each module that a Python file of the reference step imports from the repository: a
    /// module of `tools/`, or the ingest package.
    fn reference_imports(file: &Path) -> Vec<String> {
        let text = std::fs::read_to_string(file).unwrap();
        let mut found = Vec::new();
        for line in text.lines().map(|line| line.split('#').next().unwrap_or_default().trim()) {
            let (module, names) = match line.strip_prefix("from ").and_then(|rest| rest.split_once(" import ")) {
                Some((module, names)) => (module, names),
                None => match line.strip_prefix("import ") {
                    Some(module) => (module, ""),
                    None => continue,
                },
            };
            let top = module.split('.').next().unwrap_or_default();
            if top == "tools" {
                found.extend(names.split(',').map(|name| format!("tools/{}.py", name.trim())));
            } else if top == "ingest" {
                found.push("host/obc-dem/reference/ingest".into());
            } else if !top.is_empty() && root().join(format!("tools/{top}.py")).is_file() {
                found.push(format!("tools/{top}.py"));
            }
        }
        found
    }

    #[test]
    fn the_reference_step_declares_each_module_that_it_imports() {
        let mut files = vec![root().join("host/obc-dem/reference/merge.py")];
        let mut pending = vec![root().join("host/obc-dem/reference/ingest")];
        while let Some(dir) = pending.pop() {
            for path in std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()) {
                match (path.is_dir(), path.extension().is_some_and(|ext| ext == "py")) {
                    (true, _) => pending.push(path),
                    (false, true) => files.push(path),
                    _ => {}
                }
            }
        }
        let declared: Vec<&str> = crate::PYTHON.iter().chain(&REFERENCE_CODE).copied().collect();
        for file in &files {
            for module in reference_imports(file) {
                assert!(declared.contains(&module.as_str()), "{} imports {module}", file.display());
            }
        }
    }
}
