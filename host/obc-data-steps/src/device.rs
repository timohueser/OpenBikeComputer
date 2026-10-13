//! The device maps. Each step covers one leaf of the `2^23` grid that the region touches:
//! `device/terrain/<i>-<j>` writes the terrain cells of the region in leaf `(i, j)`, `device/<band>/<i>-<j>`
//! its map cells of one band, and `device/landmarks/<i>-<j>` and `device/peaks/<i>-<j>` its landmark
//! and peak artifacts per network cell. No client reads the files of the intermediate steps:
//! `device/osm` writes the OSM of each leaf, `device/reference/<i>-<j>` the national terrain models
//! (`terrain-reference/<key>`) that the terrain cells of the leaf read, and
//! `device/landmark-content` and `device/peak-content` the landmarks and peaks of the region, compiled
//! from the `wiki` shards.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use obc_bake::coverage::Coverage;
use obc_data::engine::{snapshot_files, Client, Input, Layer, LayerFile, Run, Step};
use obc_data::prepared::{show, Manifest, MANIFEST};
use obc_data::product::{read, version, BlockedLayer, Inputs, Note, Pointer, Product, Steps, Unplanned, Wanted};
use obc_data::regions::{Bbox, Regions};
use obc_data::release::{Output, Release};
use obc_data::sources::{self, attribution, Source};
use obc_data::store::Store;
use obc_dem::bake::{V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::crest::cell_window;
use obc_dem::reference::TILE_LOG2;
use obc_dem::step::GLO30;
use obc_draw::step::LAND;
use obc_formats::{obcm, obct};
use obc_map_core::grid::{id_width, Band, BandTable, CellId};
use obc_osm::LeafId;
use obc_pack::step::WIKI;

use crate::readers;

pub(crate) mod catalog;

/// The cell of the planet bake, and of every device-map layer.
const LEAF_LOG2: u32 = obc_osm::SOURCE_LEAF_LOG2;
pub(crate) const POLY: &str = "geofabrik-poly";
pub const EXTRACTS: &str = "geofabrik-extracts";
/// The `wiki` shards are cells of `2^20` µdeg.
const WIKI_CELL_LOG2: u32 = 20;
/// The names of the GLO-30 tiles: a square that it does not name is sea.
pub const TILE_LIST: &str = "copernicus-glo-30-tiles";

/// The terrain cells of each leaf, and the national terrain models that they read.
pub static DEVICE_TERRAIN: Layer = Layer {
    name: "device/terrain",
    shape: obc_data::engine::Shape::Tiled,
    format: obct::SURFACE_VERSION as u32,
    revision: 1,
    readers: &[&readers::DEVICE, &readers::MAP_BUILDER],
};

/// The map cells of each band in each leaf.
pub static DEVICE_MAP: Layer = Layer {
    name: "device/map",
    shape: obc_data::engine::Shape::Tiled,
    format: obcm::VERSION as u32,
    revision: 1,
    readers: &[&readers::DEVICE, &readers::MAP_BUILDER],
};

/// The landmark and peak artifacts of each leaf, and the content that they are compiled from.
pub static DEVICE_LANDMARKS: Layer = Layer {
    name: "device/landmarks",
    shape: obc_data::engine::Shape::Tiled,
    // The device checks both section versions; each only grows, so the sum grows when either changes.
    format: (obcm::landmarks::SECTION_VERSION + obcm::peaks::VERSION) as u32,
    revision: 1,
    readers: &[&readers::DEVICE],
};

pub struct Device;

impl Product for Device {
    fn name(&self) -> &'static str {
        "device"
    }

    fn pointer(&self) -> Pointer {
        Pointer { key: "cell-catalog/catalog.json", step: catalog::LAYER, path: "catalog.json" }
    }

    fn named(&self, outputs: &[Output]) -> Result<Vec<(String, LayerFile)>, String> {
        catalog::named(outputs)
    }

    fn verify(
        &self,
        _root: &std::path::Path,
        previous: Option<&Release>,
        release: &Release,
        store: &Store,
    ) -> Result<(), String> {
        catalog::verify(previous, release, store)
    }

    fn status_steps(
        &self,
        root: &std::path::Path,
        inputs: &Inputs,
        regions: &Regions,
        store: &Store,
    ) -> Result<Steps, Unplanned> {
        self.steps_with_tool(root, inputs, regions, store, Ok(()))
    }

    fn steps(
        &self,
        root: &std::path::Path,
        inputs: &Inputs,
        regions: &Regions,
        store: &Store,
    ) -> Result<Steps, Unplanned> {
        let osmium = obc_osm::ShardRunner::check(&obc_osm::OsmiumRunner::default());
        self.steps_with_tool(root, inputs, regions, store, osmium)
    }
}

impl Device {
    /// Its steps. `osmium` says whether Osmium runs here: without it, the OSM steps are blocked.
    pub fn steps_with_tool(
        &self,
        _root: &std::path::Path,
        inputs: &Inputs,
        regions: &Regions,
        store: &Store,
        osmium: Result<(), String>,
    ) -> Result<Steps, Unplanned> {
        let mut wanted = Vec::new();
        let selection = crate::region_sources::resolve(inputs, regions, store, &mut wanted)?;
        let tile_list = text(inputs, store, TILE_LIST, &[], &mut wanted)?;
        let land_polygons = snapshot_version(inputs, store, LAND, &[], &mut wanted)?;
        let body = text(inputs, store, catalog::INDEX, &[], &mut wanted)?;
        let index_version =
            if body.is_some() { snapshot_version(inputs, store, catalog::INDEX, &[], &mut wanted)? } else { None };
        let catalog_index = body.zip(index_version);
        // The version of the tiles: the fetch of a tile is in the plan of the step that reads it.
        let glo30 = snapshot_version(inputs, store, GLO30, &[], &mut wanted)?;
        let (Some(selection), Some(tile_list), Some(glo30), true) = (selection, tile_list, glo30, wanted.is_empty())
        else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        let outlines = &selection.outlines;
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let (mut steps, mut blocked, mut notes, mut shapes) = (Vec::new(), Vec::new(), Vec::new(), BTreeMap::new());
        for (&leaf, cells) in &leaves(outlines, V1_CELL_LOG2.into()) {
            let reference = reference(inputs, store, (leaf, cells), &mut shapes, &mut wanted, &mut notes)?;
            let terrain = terrain(leaf, cells, &land, &glo30, reference.as_ref());
            steps.extend(reference);
            steps.push(terrain);
        }
        if !wanted.is_empty() {
            return Err(Unplanned::NeedsFetch(wanted));
        }
        let land_polygons = land_polygons.ok_or_else(|| Unplanned::Failed("land polygons were not fetched".into()))?;
        let content = content_declarations(inputs, store, &selection)?;
        blocked.extend(content.blocked);
        let mut content_names: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for source in &selection.sources {
            for collection in ["landmarks", "peaks"] {
                let name = if selection.direct {
                    content_layer(collection)
                } else {
                    format!("{}/{}", content_layer(collection), source.id)
                };
                content_names.entry(collection).or_default().push(name);
            }
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
                steps.push(map_cells(&band, leaf, &cells, partial, &land_polygons, reads_terrain));
                if band.has_nav() {
                    network.insert(leaf, cells);
                }
            }
        }
        steps.extend(content.steps);
        for collection in ["landmarks", "peaks"] {
            for (&leaf, cells) in &network {
                let mut step = artifacts(collection, leaf, cells);
                if !selection.direct {
                    step.inputs.retain(
                        |input| !matches!(input, Input::Step { name, .. } if name == &content_layer(collection)),
                    );
                    step.inputs.extend(content_names[collection].iter().map(Input::step));
                }
                steps.push(step);
            }
        }
        let source_steps = crate::region_sources::inputs("device", &selection);
        let extract = if selection.sources.len() == 1 {
            Input::Step { name: source_steps[0].name.clone(), files: vec!["source.osm.pbf".into()] }
        } else {
            match &osmium {
                Ok(()) => steps.push(crate::region_sources::combined("device/region-osm", &source_steps)),
                Err(reason) => {
                    blocked.push(BlockedLayer::new("device/region-osm", &crate::region_sources::OSM, reason.clone()))
                }
            }
            Input::Step { name: "device/region-osm".into(), files: vec!["osm.pbf".into()] }
        };
        steps.extend(source_steps);
        match osmium {
            Ok(()) => steps.push(osm(extract, &osm_leaves)),
            Err(reason) => blocked.push(BlockedLayer::new("device/osm", &crate::region_sources::OSM, reason)),
        }
        if blocked.is_empty() {
            let (body, version) =
                catalog_index.ok_or_else(|| Unplanned::Failed("catalog index was not fetched".into()))?;
            match catalog::step(
                inputs,
                regions.get(&inputs.settings.region).expect("the settings name a region"),
                store,
                &selection,
                &steps,
                &glo30,
                version,
                &body,
            ) {
                Ok(step) => steps.push(step),
                Err(Unplanned::Invalid(reason)) => {
                    blocked.push(BlockedLayer::new(catalog::LAYER, &catalog::DEVICE_CATALOG, reason))
                }
                Err(error) => return Err(error),
            }
        } else {
            let reasons = blocked.iter().map(|blocked| blocked.reason.as_str()).collect::<Vec<_>>().join("; ");
            let reason = format!("catalog requires all map, terrain and article layers: {reasons}");
            blocked.push(BlockedLayer::new(catalog::LAYER, &catalog::DEVICE_CATALOG, reason));
        }
        let mut listed = Steps { steps, blocked, notes };
        listed.block_dependents();
        Ok(listed)
    }
}

/// The landmark and peak content of each region source: its extract, its `.poly`, and the shards
/// of `wiki` that it needs, `common.tar.zst` and those of the cells that its polygon touches, at
/// the version that the target reads. Without a version of `wiki`, the content layers block.
pub(crate) fn content_declarations(
    inputs: &Inputs,
    store: &Store,
    selection: &crate::region_sources::Selection,
) -> Result<Steps, Unplanned> {
    let shards = match version(inputs, store, WIKI, &[]).map_err(Unplanned::Failed)? {
        Ok(pinned) => {
            let manifest = snapshot_files(store, WIKI, &pinned, &[], &[MANIFEST.into()]).map_err(Unplanned::Failed)?;
            let Some(manifest) = manifest else {
                let fetch = Wanted { source: WIKI.into(), version: Some(pinned), params: Vec::new() };
                return Err(Unplanned::NeedsFetch(vec![fetch]));
            };
            let path = &manifest[MANIFEST];
            let bytes = std::fs::read(path).map_err(|e| Unplanned::Failed(format!("{}: {e}", path.display())))?;
            let manifest: Manifest =
                serde_json::from_slice(&bytes).map_err(|e| Unplanned::Failed(format!("{WIKI}@{pinned}: {e}")))?;
            Some((pinned, manifest.files.into_iter().map(|file| file.path).collect::<BTreeSet<_>>()))
        }
        Err(_) => None,
    };
    let (mut steps, mut blocked) = (Vec::new(), Vec::new());
    let width = id_width(WIKI_CELL_LOG2);
    for source in &selection.sources {
        let area = vec![("area".to_string(), source.id.clone())];
        let cells = source.coverage.cells(WIKI_CELL_LOG2).into_iter();
        let cells = cells.map(|cell| format!("cells/{:0width$}-{:0width$}.tar.zst", cell.i, cell.j));
        let names: Vec<String> = std::iter::once("common.tar.zst".to_string()).chain(cells).collect();
        for collection in ["landmarks", "peaks"] {
            let name = if selection.direct {
                content_layer(collection)
            } else {
                format!("{}/{}", content_layer(collection), source.id)
            };
            let Some((version, paths)) = &shards else {
                let reason =
                    format!("no target lists a version of `{WIKI}`. Fix: Run `obc data source prepare {WIKI}`.");
                blocked.push(BlockedLayer::new(name, &DEVICE_LANDMARKS, reason));
                continue;
            };
            // A snapshot input without files reads every file of the version.
            let files: Vec<String> = names.iter().filter(|path| paths.contains(*path)).cloned().collect();
            if files.is_empty() {
                let reason = format!("`{WIKI}` {} lists no shard of `{}`", show(version), source.id);
                blocked.push(BlockedLayer::new(name, &DEVICE_LANDMARKS, reason));
                continue;
            }
            let wiki = Input::Snapshot { source: WIKI.into(), version: version.clone(), params: Vec::new(), files };
            let regional = |source: &str, version: &String| Input::Snapshot {
                source: source.into(),
                version: version.clone(),
                params: area.clone(),
                files: Vec::new(),
            };
            let reads = vec![regional(EXTRACTS, &source.extract), regional(POLY, &source.poly), wiki];
            steps.push(Step { name, ..content(collection, reads) });
        }
    }
    Ok(Steps { steps, blocked, notes: Vec::new() })
}

/// The name and the SHA-256 of the one file that the fetch of `source@version` with `params` gave.
fn file(
    store: &Store,
    source: &str,
    version: &str,
    params: &[(String, String)],
) -> Result<(String, String), Unplanned> {
    let missing = || Unplanned::Failed(format!("the store has no file of {source}@{version} {params:?}"));
    let names = store.requested(source, version, params).map_err(Unplanned::Failed)?.ok_or_else(missing)?;
    let snapshot = store.snapshot(source, version).map_err(Unplanned::Failed)?.ok_or_else(missing)?;
    let [name] = &names[..] else {
        return Err(Unplanned::Failed(format!("a fetch of {source} with {params:?} gives {} files", names.len())));
    };
    let file = snapshot.files.into_iter().find(|file| &file.name == name).ok_or_else(missing)?;
    Ok((file.name, file.sha256))
}

/// The compiled landmarks or peaks of the region, from the `wiki` shards, the `.poly` and the extract.
fn content(collection: &str, inputs: Vec<Input>) -> Step {
    let run = match collection {
        "landmarks" => obc_pack::step::landmark_content,
        _ => obc_pack::step::peak_content,
    };
    Step {
        layer: &DEVICE_LANDMARKS,
        name: content_layer(collection),
        inputs,
        options: serde_json::json!({}),
        outputs: vec![collection.into()],
        run: Run::Rust(run),
        client: Client::None,
    }
}

/// `device/landmark-content` or `device/peak-content`.
fn content_layer(collection: &str) -> String {
    format!("device/{}-content", collection.trim_end_matches('s'))
}

/// The landmark or peak artifacts of the network `cells` of one leaf. A landmark joins the OSM
/// objects that name it, so the landmarks read the OSM of the leaf too.
fn artifacts(collection: &str, leaf: LeafId, cells: &[CellId]) -> Step {
    let mut inputs = Vec::new();
    let run = match collection {
        "landmarks" => {
            inputs.push(Input::Step { name: "device/osm".into(), files: vec![obc_osm::step::leaf_pbf(leaf)] });
            obc_pack::step::landmarks
        }
        _ => obc_pack::step::peaks,
    };
    inputs.push(Input::step(content_layer(collection)));
    Step {
        layer: &DEVICE_LANDMARKS,
        name: leaf_layer(&format!("device/{collection}"), leaf),
        inputs,
        options: serde_json::json!({
            "cell_log2": cells[0].log2,
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
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
    inputs: &Inputs,
    store: &Store,
    source: &str,
    params: &[(String, String)],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<String>, Unplanned> {
    Ok(match version(inputs, store, source, params).map_err(Unplanned::Failed)? {
        Ok(version) => Some(version),
        Err(fetch) => {
            wanted.push(fetch);
            None
        }
    })
}

/// The OSM of each leaf: the extract of the region, cut to the square of the leaf and a halo. Only
/// the map cells read it.
fn osm(extract: Input, leaves: &BTreeSet<LeafId>) -> Step {
    Step {
        layer: &crate::region_sources::OSM,
        name: "device/osm".into(),
        inputs: vec![extract],
        options: serde_json::json!({"leaves": leaves.iter().map(|leaf| [leaf.i, leaf.j]).collect::<Vec<_>>()}),
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
    let osm = Input::Step { name: "device/osm".into(), files: vec![obc_osm::step::leaf_pbf(leaf)] };
    let mut inputs = vec![osm];
    if !band.lods.is_empty() {
        inputs.push(land_polygons);
    }
    if reads_terrain {
        inputs.push(Input::step(leaf_layer("device/terrain", leaf)));
    }
    Step {
        layer: &DEVICE_MAP,
        name: leaf_layer(&format!("device/{}", band.id), leaf),
        inputs,
        options: serde_json::json!({
            "band": band.id,
            "partial_cells": partial,
            "leaf": [i64::from(LEAF_LOG2), leaf.i, leaf.j],
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        outputs: vec!["cells".into(), "metadata".into()],
        run: Run::Rust(if band.has_nav() || band.has_poi() { obc_network::step::cells } else { obc_draw::step::cells }),
        // The catalog reads the metadata: a target holds it for the bake of another machine.
        client: Client::Paths(vec!["cells".into(), "metadata".into()]),
    }
}

/// The polygon of a Geofabrik area that holds a national model, with the archive tiles that its
/// boundary crosses. A step list reads each area once.
type Shape = (Coverage, BTreeSet<CellId>);

/// Whether one of `tiles` is in the polygon of one of the `areas` of `source`: the boundary of the
/// polygon crosses the tile, or the polygon holds the centre of the tile. `None` while the store
/// lacks a polygon; then `wanted` has it.
fn in_areas(
    inputs: &Inputs,
    store: &Store,
    (source, tiles): (&Source, &[CellId]),
    shapes: &mut BTreeMap<String, Shape>,
    wanted: &mut Vec<Wanted>,
) -> Result<Option<bool>, Unplanned> {
    for area in &source.areas {
        if shapes.contains_key(area) {
            continue;
        }
        let Some(poly) = text(inputs, store, POLY, &[("area".into(), area.clone())], wanted)? else { continue };
        let coverage = Coverage::parse_poly(&poly).map_err(Unplanned::Failed)?;
        let boundary = coverage.boundary_cells(TILE_LOG2.into());
        shapes.insert(area.clone(), (coverage, boundary));
    }
    if source.areas.iter().any(|area| !shapes.contains_key(area)) {
        return Ok(None);
    }
    let inside = |tile: &CellId, (coverage, boundary): &Shape| {
        let (west, south, _, _) = tile.square();
        boundary.contains(tile) || coverage.contains(south + tile.size() / 2, west + tile.size() / 2)
    };
    let any = tiles.iter().any(|tile| source.areas.iter().any(|area| inside(tile, &shapes[area])));
    Ok(Some(source.areas.is_empty() || any))
}

/// The national terrain models of the archive tiles that the terrain cells of `leaf` read with the
/// halo of the crest rule, merged into the reference archive of the leaf. A model is the prepared
/// source `terrain-reference/<key>` at the version that the target reads, and the leaf reads the
/// tiles of its manifest among those. `None` when no model has one, or while the store lacks a
/// manifest or a polygon; then `wanted` has its fetch. A model without a version adds nothing, and
/// `notes` say so where it can have a height: a new version is a new input of the leaf.
fn reference(
    inputs: &Inputs,
    store: &Store,
    (leaf, cells): (LeafId, &[CellId]),
    shapes: &mut BTreeMap<String, Shape>,
    wanted: &mut Vec<Wanted>,
    notes: &mut Vec<Note>,
) -> Result<Option<Step>, Unplanned> {
    let window_of = |cell: &CellId| {
        let (ci, cj) = (u32::try_from(cell.i).ok()?, u32::try_from(cell.j).ok()?);
        cell_window(ci, cj, V1_POSTING_LOG2, V1_CELL_LOG2)
    };
    let windows = cells.iter().map(|cell| window_of(cell).expect("a terrain cell has a window"));
    let tile = |(ti, tj): (u32, u32)| CellId::new(TILE_LOG2.into(), ti.into(), tj.into()).expect("a tile is a cell");
    let tiles: BTreeSet<CellId> = windows.flat_map(|window| window.tiles()).map(tile).collect();
    let paths: BTreeSet<String> = tiles.iter().map(|tile| format!("16/{:04}/{:04}.tif", tile.i, tile.j)).collect();
    let square = |tile: &&CellId| {
        let (west, south, east, north) = tile.square();
        [west, south, east, north].map(|udeg| udeg as f64 / 1e6)
    };
    let (mut reads, mut models) = (Vec::new(), Vec::new());
    for source in sources::all().iter().filter(|source| source.id.starts_with("terrain-reference/")) {
        let near: Vec<CellId> = tiles.iter().filter(|tile| source.meets(square(tile))).copied().collect();
        if near.is_empty() {
            continue;
        }
        let id = &source.id;
        let Ok(pinned) = version(inputs, store, id, &[]).map_err(Unplanned::Failed)? else {
            if in_areas(inputs, store, (source, &near), shapes, wanted)? == Some(true) {
                let text =
                    format!("{id} has no version: device/terrain uses GLO-30 there. Fix: obc data source prepare {id}");
                let note = Note { layer: DEVICE_TERRAIN.name.into(), text };
                if !notes.contains(&note) {
                    notes.push(note);
                }
            }
            continue;
        };
        let manifest = snapshot_files(store, id, &pinned, &[], &[MANIFEST.into()]).map_err(Unplanned::Failed)?;
        let Some(manifest) = manifest else {
            let fetch = Wanted { source: id.clone(), version: Some(pinned), params: Vec::new() };
            if !wanted.contains(&fetch) {
                wanted.push(fetch);
            }
            continue;
        };
        let path = &manifest[MANIFEST];
        let bytes = std::fs::read(path).map_err(|e| Unplanned::Failed(format!("{}: {e}", path.display())))?;
        let manifest: Manifest =
            serde_json::from_slice(&bytes).map_err(|e| Unplanned::Failed(format!("{id}@{pinned}: {e}")))?;
        let files: Vec<String> =
            manifest.files.into_iter().map(|file| file.path).filter(|path| paths.contains(path)).collect();
        if !files.is_empty() {
            // The credit names the day of the data.
            models.push(serde_json::json!({"source": id, "version": manifest.version, "credit": attribution(id)}));
            reads.push(Input::Snapshot { source: id.clone(), version: pinned, params: Vec::new(), files });
        }
    }
    if models.is_empty() {
        return Ok(None);
    }
    let tiles: Vec<String> = tiles.iter().map(|tile| format!("{:04}/{:04}", tile.i, tile.j)).collect();
    Ok(Some(Step {
        client: Client::None,
        ..crate::python(
            &DEVICE_TERRAIN,
            &leaf_layer("device/reference", leaf),
            reads,
            serde_json::json!({"models": models, "tiles": tiles}),
            ("host/obc-dem/reference/merge.py", Some("terrain-reference")),
            &["reference"],
        )
    }))
}

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
    inputs.extend(reference.map(|reference| Input::step(reference.name.clone())));
    Step {
        layer: &DEVICE_TERRAIN,
        name: leaf_layer("device/terrain", leaf),
        inputs,
        options: serde_json::json!({
            "posting_log2": V1_POSTING_LOG2,
            "cell_log2": V1_CELL_LOG2,
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        outputs: vec!["terrain".into(), "metadata".into()],
        run: Run::Rust(obc_dem::step::terrain),
        // The map cells and the catalog read the metadata: a target holds it for another machine.
        client: Client::Paths(vec!["metadata".into(), "terrain".into()]),
    }
}

/// The text of the one file that a fetch of `source` with `params` gives, or `None` while the
/// store lacks it; then `wanted` has its fetch.
pub(crate) fn text(
    inputs: &Inputs,
    store: &Store,
    source: &str,
    params: &[(String, String)],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<String>, Unplanned> {
    let files = match read(inputs, store, source, params).map_err(Unplanned::Failed)? {
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
    use std::path::{Path, PathBuf};

    use obc_data::engine::plan::{plan, Kept, Plan};
    use obc_data::engine::runs::{Context, Limits, Run as RunLog};
    use obc_data::engine::Request;
    use obc_data::fetch::http::Http;
    use obc_data::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot};

    use super::*;
    use obc_data::regions::Area;

    fn map_steps(root: &Path, inputs: &Inputs, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        Device.steps_with_tool(root, inputs, regions, store, Ok(()))
    }

    /// The inputs of `region` whose serving release reads the GLO-30 tiles at `glo30` and the tile
    /// list at `1`.
    pub(crate) fn serving(region: &str, glo30: &str) -> Inputs {
        let settings = obc_data::settings::Settings { region: region.into(), ..Default::default() };
        let read = |source: &str, version: &str| ((source.to_string(), Vec::new()), version.to_string());
        Inputs { settings, serving: [read(GLO30, glo30), read(TILE_LIST, "1")].into(), ..Inputs::default() }
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
    fn grimsel(store: &Store, glo30: &str) -> (Inputs, Regions) {
        with_model_areas(store);
        let mut inputs = serving("grimsel", glo30);
        with_wiki(store, &mut inputs, "2026-10-01");
        let regions = Regions::load(&root()).unwrap();
        let Area::Box { bbox } = &regions.get("grimsel").unwrap().area else { unreachable!() };
        let poly = box_poly(bbox);
        let index = serde_json::json!({"type":"FeatureCollection", "features":[{"type":"Feature",
            "properties":{"id":"grimsel","name":"Grimsel","parent":null,"urls":{"pbf":"https://download.geofabrik.de/europe/grimsel-latest.osm.pbf"}},
            "geometry": serde_json::from_str::<serde_json::Value>(&Coverage::parse_poly(&poly).unwrap().geojson()).unwrap()}]});
        fetched(store, catalog::INDEX, "1", &[], &[("index.json".into(), index.to_string())]);
        let area = [("area".into(), "europe/grimsel".into())];
        fetched(store, POLY, "1", &area, &[("europe/grimsel.poly".into(), poly)]);
        extracted(store, "1", &area, "europe/grimsel.osm.pbf");
        fetched(store, LAND, "1", &[], &[("land.zip".into(), "land".into())]);
        (inputs, regions)
    }

    const FREIBURG: &str =
        "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";

    /// A Geofabrik region about Freiburg, in leaf `0037-0032`, whose `.poly` the store has, and the
    /// tile list.
    fn freiburg(store: &Store) -> (Inputs, Regions) {
        with_model_areas(store);
        let area = [("area".to_string(), "europe/test".to_string())];
        fetched(store, POLY, "1", &area, &[("europe/test.poly".into(), FREIBURG.into())]);
        with_index(store);
        with_tile_list(store, &["N47_00_E007", "N48_00_E007"]);
        let region = obc_data::regions::parse_region(
            "europe/test",
            "name = \"Test\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\n",
        )
        .unwrap();
        let mut inputs = serving("europe/test", "1");
        with_wiki(store, &mut inputs, "2026-10-01");
        (inputs, Regions::new(vec![region]).unwrap())
    }

    /// A version of the prepared source `id` of `day` whose manifest names `paths`: its pin and the
    /// bytes of its manifest.
    fn prepared(id: &str, day: &str, paths: &[&str]) -> (String, Vec<u8>) {
        let file = |path: &str| LayerFile { path: path.into(), size: 1, sha256: sha256_hex(path.as_bytes()) };
        let manifest = Manifest {
            source: id.into(),
            version: day.into(),
            format: 1,
            revision: 1,
            inputs: Vec::new(),
            files: paths.iter().map(|path| file(path)).collect(),
            scope: None,
            licence: None,
            attribution: None,
            machine: "test".into(),
            time: format!("{day}T00:00:00Z"),
            cost: obc_data::prepared::Cost { wall_ms: 0, peak_rss_bytes: None },
        };
        let bytes = manifest.canonical();
        (format!("{day}/{}", sha256_hex(&bytes)), bytes)
    }

    /// The version of `wiki` of `day` in the tests: its pin and its manifest, which names the
    /// common shard, the shards of the cells of the Grimsel and of Freiburg, and one far away.
    pub(crate) fn wiki(day: &str) -> (String, Vec<u8>) {
        let paths = [
            "cells/0000-0000.tar.zst",
            "cells/0300-0263.tar.zst",
            "cells/0300-0264.tar.zst",
            "cells/0301-0263.tar.zst",
            "common.tar.zst",
            "index.json",
        ];
        prepared(WIKI, day, &paths)
    }

    /// Add the manifest of [`wiki`] of `day` to the store, and give its pin.
    pub(crate) fn wiki_in(store: &Store, day: &str) -> String {
        let (pinned, manifest) = wiki(day);
        stored(store, WIKI, &pinned, &[], &[(MANIFEST.into(), manifest)]);
        pinned
    }

    /// `inputs` read the version of `wiki` of `day`, which the store has.
    fn with_wiki(store: &Store, inputs: &mut Inputs, day: &str) {
        inputs.resolved.insert((WIKI.into(), Vec::new()), wiki_in(store, day));
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
        extracted(store, "1", &area, "europe/test.osm.pbf");
        fetched(store, LAND, "1", &[], &[("land-polygons-split-3857.zip".into(), "land".into())]);
    }

    /// The extract `name` of the fetch with `params` at `version`: a real one without a Wikimedia
    /// link, so its content steps fetch no shared content.
    pub(crate) fn extracted(store: &Store, version: &str, params: &[(String, String)], name: &str) {
        let pbf = std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/planner.osm.pbf")).unwrap();
        stored(store, EXTRACTS, version, params, &[(name.into(), pbf)]);
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
        let files: Vec<(String, Vec<u8>)> =
            files.iter().map(|(name, text)| (name.clone(), text.clone().into())).collect();
        stored(store, source, version, params, &files);
    }

    /// [`fetched`] with the bytes of each file.
    fn stored(store: &Store, source: &str, version: &str, params: &[(String, String)], files: &[(String, Vec<u8>)]) {
        let mut snapshot = store.snapshot(source, version).unwrap().unwrap_or_else(|| Snapshot {
            source: source.into(),
            version: version.into(),
            files: Vec::new(),
        });
        for (name, bytes) in files {
            let (sha256, file) = (sha256_hex(bytes), store.partial("file"));
            write_atomic(&file, bytes).unwrap();
            store.insert(&file, &sha256).unwrap();
            let (url, size) = (format!("https://example.org/{name}"), bytes.len() as u64);
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

    /// The polygon of each Geofabrik area of a national model: the box of its `extent`.
    pub(crate) fn with_model_areas(store: &Store) {
        for source in sources::all().iter().filter(|source| source.id.starts_with("dtm-")) {
            let [west, south, east, north] = source.extent.unwrap();
            let poly = box_poly(&Bbox { west, south, east, north });
            for area in &source.areas {
                fetched(store, POLY, "1", &[("area".into(), area.clone())], &[(format!("{area}.poly"), poly.clone())]);
            }
        }
    }

    /// A store whose tile list names the Grimsel tiles in `land`.
    fn with_tile_list(store: &Store, land: &[&str]) {
        let list: String = land.iter().map(|tile| format!("Copernicus_DSM_COG_10_{tile}_00_DEM\n")).collect();
        fetched(store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list)]);
    }

    fn terrain_steps(steps: &[Step]) -> impl Iterator<Item = &Step> {
        steps.iter().filter(|step| step.name.starts_with("device/terrain/"))
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
    fn a_saved_id_and_two_areas_read_the_newest_version_of_each_area() {
        let temp = temp("saved-source-area");
        let store = Store::at(temp.0.join("store"));
        let (mut inputs, _) = freiburg(&store);
        with_osm(&store);
        inputs.settings.region = "ride/freiburg".into();
        let region = obc_data::regions::parse_region(
            &inputs.settings.region,
            "name = \"My ride\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\n",
        )
        .unwrap();
        let regions = Regions::new(vec![region]).unwrap();
        let listed = map_steps(&root(), &inputs, &regions, &store).unwrap();
        assert!(listed.blocked.is_empty(), "{:?}", listed.blocked);
        let osm = listed.steps.iter().find(|step| step.name == "device/source/europe/test").unwrap();
        let Input::Snapshot { params, .. } = &osm.inputs[0] else {
            panic!("OSM snapshot");
        };
        assert_eq!(params, &[("area".into(), "europe/test".into())]);
        let catalog = listed.steps.iter().find(|step| step.name == catalog::LAYER).unwrap();
        assert_eq!(catalog.options["sources"][0]["extract_id"], "europe/test");
        let primary = catalog.options["picks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|pick| pick["id"] == inputs.settings.region)
            .unwrap();
        assert_eq!(primary["name"], "My ride");

        fetched(
            &store,
            POLY,
            "1",
            &[("area".into(), "europe/second".into())],
            &[("europe/second.poly".into(), FREIBURG.into())],
        );
        let second = [("area".into(), "europe/second".into())];
        extracted(&store, "1", &second, "europe/second.osm.pbf");
        fetched(&store, POLY, "2", &second, &[("europe/second.poly".into(), FREIBURG.replace("47.99", "47.98"))]);
        extracted(&store, "2", &second, "europe/second-2.osm.pbf");
        // Include the second source in the index without changing the requested ground.
        let mut index: serde_json::Value =
            serde_json::from_str(&text(&inputs, &store, catalog::INDEX, &[], &mut Vec::new()).unwrap().unwrap())
                .unwrap();
        let mut feature = index["features"][0].clone();
        feature["properties"]["id"] = "second".into();
        feature["properties"]["urls"]["pbf"] = "https://download.geofabrik.de/europe/second-latest.osm.pbf".into();
        index["features"].as_array_mut().unwrap().push(feature);
        fetched(&store, catalog::INDEX, "2", &[], &[("index-2.json".into(), index.to_string())]);
        let region = obc_data::regions::parse_region(
            &inputs.settings.region,
            "name = \"Two areas\"\nkind = \"geofabrik\"\nareas = [\"europe/test\",\"europe/second\"]\n",
        )
        .unwrap();
        let regions = Regions::new(vec![region]).unwrap();
        let listed = map_steps(&root(), &inputs, &regions, &store).unwrap();
        assert!(listed.blocked.is_empty(), "{:?}", listed.blocked);
        let raw = |name: &str| listed.steps.iter().find(|step| step.name == format!("device/source/{name}")).unwrap();
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
                .find(|step| step.name == format!("device/{collection}-content/europe/second"))
                .unwrap();
            for source in [EXTRACTS, POLY] {
                let read = |input: &Input| matches!(input, Input::Snapshot { source: read, version, params, .. } if read == source && version == "2" && params == &second);
                assert!(content.inputs.iter().any(read), "{collection} content reads {source} 2 of the area");
            }
        }
        let catalog = listed.steps.iter().find(|step| step.name == catalog::LAYER).unwrap();
        assert_eq!(
            catalog.options["sources"],
            serde_json::json!([
            {"extract_id":"europe/second", "snapshot":"2"}, {"extract_id":"europe/test", "snapshot":"1"}])
        );
        assert!(catalog.options["picks"].as_array().unwrap().iter().any(|pick| pick["id"] == inputs.settings.region));
        assert!(inputs.requests.borrow().contains(&(EXTRACTS.into(), obc_data::store::sorted(&second))));
    }

    /// The operator page reads a leaf from the last part of a step name (builder/web/src/lib/data/leaf.ts).
    #[test]
    fn a_leaf_names_its_steps_with_its_indices() {
        assert_eq!(leaf_layer("x", LeafId { i: 37, j: 32 }), "x/0037-0032");
    }

    #[test]
    fn a_new_glo30_tile_in_one_leaf_rebuilds_that_leaf_only() {
        let temp = temp("leaves");
        let store = Store::at(temp.0.join("store"));
        // Two versions after the `start` of GLO-30.
        const OLD: &str = "2023-01-01";
        const NEW: &str = "2024-01-01";
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E007", "N47_00_E008"]);
        let steps = |version: &str| {
            let (inputs, regions) = grimsel(&store, version);
            let mut steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
            steps.retain(|step| step.name.starts_with("device/terrain/"));
            steps.iter_mut().for_each(|step| step.run = Run::Rust(fake));
            steps
        };
        let names: Vec<String> = steps(OLD).iter().map(|step| step.name.clone()).collect();
        assert_eq!(names, ["device/terrain/0037-0032", "device/terrain/0037-0033"], "no model has data there");
        let west = "Copernicus_DSM_COG_10_N46_00_E007_00_DEM";
        glo30_fetched(&store, &steps(OLD), OLD, |tile| format!("{tile} 1"));

        let (root, http) = (root(), Http::new());
        let kept = Kept::new();
        let context = Context {
            store: &store,
            root: &root,
            sources: &[],
            http: &http,
            kept: &kept,
            served: &Default::default(),
            limits: Limits::machine(),
        };
        let first = plan(&store, &steps(OLD)).unwrap();
        assert_eq!(builds(&first), names);
        let mut run = RunLog::create(&store, ["build", "test"]).unwrap();
        run.build(&context, &steps(OLD), &first).unwrap();
        run.finish(None).unwrap();
        glo30_fetched(&store, &steps(NEW), NEW, |tile| format!("{tile} {}", if tile == west { 2 } else { 1 }));
        assert_eq!(builds(&plan(&store, &steps(NEW)).unwrap()), [&names[0]], "the new tile is west of the edge");
    }

    #[test]
    fn a_leaf_reads_the_tiles_of_its_cells_that_the_tile_list_names() {
        let temp = temp("tile-list");
        let store = Store::at(temp.0.join("store"));
        let (inputs, regions) = grimsel(&store, "1");
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &inputs, &regions, &store) else {
            panic!("no tile list")
        };
        assert_eq!(wanted, [Wanted { source: TILE_LIST.into(), version: Some("1".into()), params: Vec::new() }]);

        // As if N47_00_E007 were sea.
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        let steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
        let terrain: Vec<&Step> = terrain_steps(&steps).collect();
        assert_eq!(tiles(terrain[0]), ["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        // The squares of the eastern cells reach no tile west of 8°.
        assert_eq!(tiles(terrain[1]), ["N46_00_E008", "N47_00_E008"]);
    }

    #[test]
    fn a_band_reads_the_osm_of_its_leaf_the_land_and_the_terrain_when_its_bytes_have_heights() {
        let temp = temp("bands");
        let store = Store::at(temp.0.join("store"));
        let (inputs, regions) = freiburg(&store);
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &inputs, &regions, &store) else {
            panic!("no extract")
        };
        assert_eq!(wanted.iter().map(|fetch| fetch.source.as_str()).collect::<Vec<_>>(), [EXTRACTS, LAND]);
        with_osm(&store);
        let steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
        let reads = |name: &str| -> Vec<String> {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = |input: &Input| match input {
                Input::Snapshot { source, .. } => source.clone(),
                Input::Step { name, files } => format!("{name} {files:?}"),
            };
            step.inputs.iter().map(read).collect()
        };
        let osm = r#"device/osm ["osm/0037-0032.osm.pbf"]"#;
        assert_eq!(reads("device/osm"), ["device/source/europe/test [\"source.osm.pbf\"]"]);
        let intermediate =
            |name: &str| name == "device/osm" || name.ends_with("-content") || name.starts_with("device/source/");
        assert!(steps.iter().all(|step| step.client.is_none() == intermediate(&step.name)));
        assert_eq!(reads("device/coarse/0037-0032"), [osm, LAND]);
        for band in ["mid", "fine"] {
            assert_eq!(
                reads(&format!("device/{band}/0037-0032")),
                [osm, LAND, "device/terrain/0037-0032 []"],
                "{band}"
            );
        }
        assert_eq!(reads("device/network/0037-0032"), [osm, "device/terrain/0037-0032 []"]);
    }

    /// The content of a region reads its extract, its `.poly`, and the `wiki` shards of its cells:
    /// the common one and those that its polygon touches. Without a version of `wiki`, or without a
    /// shard of the region, the content layers block, and the catalog with them.
    #[test]
    fn content_reads_the_extract_the_poly_and_the_wiki_shards_of_its_cells() {
        let temp = temp("content");
        let store = Store::at(temp.0.join("store"));
        let (mut inputs, regions) = freiburg(&store);
        with_osm(&store);
        let steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
        let reads = |name: &str| -> Vec<String> {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = |input: &Input| match input {
                Input::Snapshot { source, files, .. } if files.is_empty() => source.clone(),
                Input::Snapshot { source, files, .. } => format!("{source} {}", files.join(" ")),
                Input::Step { name, .. } => name.clone(),
            };
            step.inputs.iter().map(read).collect()
        };
        let content = [EXTRACTS, POLY, "wiki common.tar.zst cells/0301-0263.tar.zst"];
        assert_eq!(reads("device/landmark-content"), content);
        assert_eq!(reads("device/peak-content"), content);
        assert_eq!(reads("device/landmarks/0037-0032"), ["device/osm", "device/landmark-content"]);
        assert_eq!(reads("device/peaks/0037-0032"), ["device/peak-content"]);
        let network = steps.iter().find(|step| step.name == "device/network/0037-0032").unwrap();
        let landmarks = steps.iter().find(|step| step.name == "device/landmarks/0037-0032").unwrap();
        assert_eq!(landmarks.options["cells"], network.options["cells"], "an artifact per network cell");
        let wikimedia = |input: &Input| matches!(input, Input::Snapshot { source, .. } if ["wikidata", "wikipedia", "commons"].contains(&source.as_str()));
        assert!(!steps.iter().flat_map(|step| &step.inputs).any(wikimedia), "only `wiki` reads Wikimedia");

        inputs.resolved.remove(&(WIKI.into(), Vec::new()));
        let listed = map_steps(&root(), &inputs, &regions, &store).unwrap();
        let blocked: Vec<(&str, &str)> =
            listed.blocked.iter().map(|blocked| (blocked.step.as_str(), blocked.reason.as_str())).collect();
        let reason = "no target lists a version of `wiki`. Fix: Run `obc data source prepare wiki`.";
        assert_eq!(&blocked[..2], [("device/landmark-content", reason), ("device/peak-content", reason)]);
        assert_eq!(blocked[2].0, catalog::LAYER);
        assert!(!listed.steps.iter().any(|step| step.name.starts_with("device/landmarks/")));

        // A version without a shard of the region blocks too: a read without files reads them all.
        let (pinned, manifest) = prepared(WIKI, "2026-11-01", &["cells/0000-0000.tar.zst"]);
        stored(&store, WIKI, &pinned, &[], &[(MANIFEST.into(), manifest)]);
        inputs.resolved.insert((WIKI.into(), Vec::new()), pinned);
        let listed = map_steps(&root(), &inputs, &regions, &store).unwrap();
        assert!(listed.blocked[0].reason.ends_with("lists no shard of `europe/test`"), "{:?}", listed.blocked);
    }

    /// The release output of `step` as it is declared, without files, at `format`.
    pub(crate) fn declared(step: &Step, format: u32) -> Output {
        use obc_data::engine::{InputKind, InputRecord};
        use obc_data::release::{Cost, SnapshotRead};
        let no_files = obc_data::engine::digest(std::iter::empty());
        let record = |kind, name: &String, digest: &str, files: &[String]| InputRecord {
            kind,
            name: name.clone(),
            digest: digest.into(),
            files: files.to_vec(),
        };
        let inputs = step.inputs.iter().map(|input| match input {
            Input::Snapshot { source, .. } => record(InputKind::Snapshot, source, "", &[]),
            Input::Step { name, files } => record(InputKind::Step, name, &no_files, files),
        });
        let snapshots = step.inputs.iter().filter_map(|input| match input {
            Input::Snapshot { source, version, params, .. } => {
                let read = SnapshotRead { version: version.clone(), params: obc_data::store::sorted(params) };
                Some((source.clone(), read))
            }
            Input::Step { .. } => None,
        });
        Output {
            layer: step.layer.name.into(),
            step: step.name.clone(),
            format,
            revision: step.layer.revision,
            key: String::new(),
            inputs: inputs.collect(),
            options: step.options.clone(),
            digest: String::new(),
            files: Vec::new(),
            snapshots: snapshots.collect(),
            client: step.client.sorted(),
            cost: Cost { wall_ms: 0, peak_rss_bytes: None, bytes_out: 0 },
        }
    }

    #[test]
    fn a_new_map_format_makes_device_map_stale_and_the_layers_that_read_it_follow() {
        use obc_data::engine::state::{state, Environment, Newer, Verdict};
        let temp = temp("format");
        let store = Store::at(temp.0.join("store"));
        let (inputs, regions) = freiburg(&store);
        with_osm(&store);
        let steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
        // The release of the firmware before: device/map one format older, the rest as declared.
        let older = obcm::VERSION as u32 - 1;
        let format = |step: &Step| if step.layer == &DEVICE_MAP { older } else { step.layer.format };
        let release = steps.iter().map(|step| (step.name.clone(), declared(step, format(step)))).collect();
        let environment = Environment { release, newer: Newer::new() };
        // The judge compares no stored file: the release has none.
        let states = state(&Store::at(temp.0.join("empty")), &steps, &environment).unwrap();
        let stale: Vec<(&str, &str)> = states
            .iter()
            .filter(|layer| layer.state != Verdict::Ok)
            .map(|layer| (layer.layer.as_str(), layer.text.as_deref().unwrap()))
            .collect();
        let text = format!("format {older} → {}: readers need new data", obcm::VERSION);
        assert_eq!(stale, [("device/map", text.as_str()), ("device/catalog", "follows device/map")]);
    }

    /// A refresh rebuilds the layers that read the refreshed source: a step that reads none of it,
    /// directly or through another step, keeps its key.
    #[test]
    fn a_content_refresh_rebuilds_no_map_cell_and_an_osm_refresh_no_terrain_cell() {
        let sources = obc_data::sources::parse_sources(include_str!("../../../data/sources.toml")).unwrap();
        let osm =
            |id: &str| sources.iter().any(|source| source.id == id && source.licence.as_deref() == Some("ODbL-1.0"));
        let temp = temp("refresh");
        let store = Store::at(temp.0.join("store"));
        let (inputs, regions) = freiburg(&store);
        with_osm(&store);
        let steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
        let by_name: BTreeMap<&str, &Step> = steps.iter().map(|step| (step.name.as_str(), step)).collect();
        // The sources and the steps that a step reads at any depth.
        let reads = |step: &Step| {
            let (mut read, mut pending) = (BTreeSet::new(), vec![step]);
            while let Some(step) = pending.pop() {
                for input in &step.inputs {
                    match input {
                        Input::Snapshot { source, .. } => drop(read.insert(source.clone())),
                        Input::Step { name, .. } => {
                            read.insert(name.clone());
                            pending.push(by_name[name.as_str()]);
                        }
                    }
                }
            }
            read
        };
        let content = |read: &String| read.ends_with("-content") || read == WIKI;
        let named =
            |prefix: &str| -> Vec<&Step> { steps.iter().filter(|step| step.name.starts_with(prefix)).collect() };
        let cells = ["coarse", "mid", "fine", "network"].map(|band| format!("device/{band}/"));
        for step in cells.iter().flat_map(|prefix| named(prefix)) {
            assert!(!reads(step).iter().any(content), "{}", step.name);
        }
        assert!(!named("device/terrain/").is_empty());
        for step in named("device/terrain/") {
            assert!(!reads(step).iter().any(|source| osm(source)), "{}", step.name);
        }
        for step in named("device/landmarks/").into_iter().chain(named("device/peaks/")) {
            assert!(reads(step).iter().any(content), "{}", step.name);
        }
    }

    /// A version of `terrain-reference/ch` whose manifest names `paths`, for the inputs of the
    /// Grimsel: its pin and the bytes of its manifest.
    fn swiss_version(inputs: &mut Inputs, paths: &[&str]) -> (String, Vec<u8>) {
        let (pinned, bytes) = prepared("terrain-reference/ch", "2026-10-01", paths);
        inputs.resolved.insert(("terrain-reference/ch".into(), Vec::new()), pinned.clone());
        (pinned, bytes)
    }

    #[test]
    fn a_leaf_plans_from_the_manifest_of_a_model_and_reads_only_the_tiles_of_its_window() {
        let temp = temp("models");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E008"]);
        let (mut inputs, regions) = grimsel(&store, "1");
        let (pinned, manifest) =
            swiss_version(&mut inputs, &["16/4000/4000.tif", "16/4808/4215.tif", "16/4816/4232.tif"]);
        let Err(Unplanned::NeedsFetch(wanted)) = map_steps(&root(), &inputs, &regions, &store) else {
            panic!("no manifest")
        };
        let swiss = "terrain-reference/ch".to_string();
        assert_eq!(wanted, [Wanted { source: swiss.clone(), version: Some(pinned.clone()), params: Vec::new() }]);

        stored(&store, &swiss, &pinned, &[], &[(MANIFEST.into(), manifest)]);
        let steps = map_steps(&root(), &inputs, &regions, &store).unwrap().steps;
        let reads = |leaf: &str| {
            let reference = steps.iter().find(|step| step.name == format!("device/reference/{leaf}")).unwrap();
            let [Input::Snapshot { source, version, files, .. }] = &reference.inputs[..] else { panic!() };
            assert_eq!((source, version), (&swiss, &pinned));
            files.clone()
        };
        assert_eq!(reads("0037-0032"), ["16/4808/4215.tif"]);
        assert_eq!(reads("0037-0033"), ["16/4816/4232.tif"]);
        let reference = steps.iter().find(|step| step.name == "device/reference/0037-0032").unwrap();
        let models = serde_json::json!([{"source": swiss, "version": "2026-10-01", "credit": "© swisstopo"}]);
        assert_eq!(reference.options["models"], models);
        assert!(reference.client.is_none(), "only the terrain step reads the national models");
        let terrain = steps.iter().find(|step| step.name == "device/terrain/0037-0032").unwrap();
        assert!(matches!(terrain.inputs.last(), Some(Input::Step { name, .. }) if *name == reference.name));
        let national = |input: &Input| matches!(input, Input::Snapshot { source, .. } if source.starts_with("dtm-"));
        assert!(!steps.iter().flat_map(|step| &step.inputs).any(national), "no step reads a national model");
    }

    #[test]
    fn a_leaf_over_a_model_without_a_version_reads_glo30_alone_and_its_first_version_stales_it() {
        use obc_data::engine::{changes, state::Newer};
        let temp = temp("unprepared");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E008"]);
        let (mut inputs, regions) = grimsel(&store, "1");
        // The boxes of `dtm-fr` and `dtm-de-by` reach the Grimsel; their polygons stop west and east of it.
        for (area, [west, south, east, north]) in
            [("europe/france", [-5.0, 42.0, 7.5, 51.0]), ("europe/germany/bayern", [8.97, 47.27, 13.84, 50.56])]
        {
            let poly = box_poly(&Bbox { west, south, east, north });
            fetched(&store, POLY, "2", &[("area".into(), area.into())], &[(format!("{area}.poly"), poly)]);
        }
        let listed = map_steps(&root(), &inputs, &regions, &store).unwrap();
        assert!(listed.blocked.is_empty(), "{:?}", listed.blocked);
        assert!(!listed.steps.iter().any(|step| step.name.starts_with("device/reference/")));
        let text = "terrain-reference/ch has no version: device/terrain uses GLO-30 there. \
                    Fix: obc data source prepare terrain-reference/ch";
        assert_eq!(listed.notes, [Note { layer: "device/terrain".into(), text: text.into() }]);
        let declared: Vec<Output> = listed.steps.iter().map(|step| declared(step, step.layer.format)).collect();
        let release = declared.iter().map(|output| (output.step.as_str(), output)).collect();

        // Swiss heights in the west leaf only.
        let (pinned, manifest) = swiss_version(&mut inputs, &["16/4000/4000.tif", "16/4808/4215.tif"]);
        stored(&store, "terrain-reference/ch", &pinned, &[], &[(MANIFEST.into(), manifest)]);
        let listed = map_steps(&root(), &inputs, &regions, &store).unwrap();
        assert_eq!(listed.notes, []);
        let update = changes::update(&Store::at(temp.0.join("empty")), &listed.steps, &release, &Newer::new(), &[]);
        let update = update.unwrap();
        let terrain: BTreeSet<&str> =
            builds(&update.plan).into_iter().filter(|step| step.starts_with("device/terrain/")).collect();
        assert_eq!(terrain, ["device/terrain/0037-0032"].into(), "the leaf without a Swiss tile is kept");
    }
}
