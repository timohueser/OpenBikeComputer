//! The device maps. Each layer covers one leaf of the `2^23` grid that the region touches:
//! `maps/terrain/<i>-<j>` holds the terrain cells of the region in leaf `(i, j)`, `maps/<band>/<i>-<j>`
//! its map cells of one band, and `maps/landmarks/<i>-<j>` and `maps/peaks/<i>-<j>` its landmark
//! and peak artifacts per network cell. `maps/osm` holds the OSM of each leaf, and
//! `maps/landmark-content` and `maps/peak-content` the compiled Wikimedia captures of the region:
//! intermediate layers, which no client reads.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use obc_bake::coverage::Coverage;
use obc_bake::planet::LeafId;
use obc_data::engine::{Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{read, version, Product, Unplanned, Wanted};
use obc_data::regions::{Area, Bbox, Regions};
use obc_data::store::Store;
use obc_dem::bake::{V1_CELL_LOG2, V1_POSTING_LOG2};
use obc_dem::step::GLO30;
use obc_pack::grid::{id_width, Band, BandTable, CellId};
use obc_pack::step::{CAPTURES, LAND};

/// The cell of the planet bake, and of every device-map layer.
const LEAF_LOG2: u32 = obc_bake::planet::SOURCE_LEAF_LOG2;
const POLY: &str = "geofabrik-poly";
pub const EXTRACTS: &str = "geofabrik-extracts";
/// The names of the GLO-30 tiles: a square that it does not name is sea.
pub const TILE_LIST: &str = "copernicus-glo-30-tiles";

pub struct Maps;

impl Product for Maps {
    fn name(&self) -> &'static str {
        "maps"
    }

    fn prefix(&self) -> &'static str {
        "cell-catalog"
    }

    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
        let mut wanted = Vec::new();
        let outlines = outlines(env, regions, store, &mut wanted)?;
        let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
        // The map cells read the OSM of one Geofabrik area: another kind of region has terrain only.
        let geofabrik = regions.get(&env.region).is_some_and(|region| region.area == Area::Geofabrik);
        let area = vec![("area".to_string(), env.region.clone())];
        let osm_sources = match geofabrik {
            true => (
                snapshot_version(env, store, EXTRACTS, &area, &mut wanted)?,
                snapshot_version(env, store, LAND, &[], &mut wanted)?,
            ),
            false => (None, None),
        };
        // The version of the tiles: the fetch of a tile is in the plan of the step that reads it.
        let glo30 = snapshot_version(env, store, GLO30, &[], &mut wanted)?;
        let (Some(outlines), Some(tile_list), Some(glo30), true) = (outlines, tile_list, glo30, wanted.is_empty())
        else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let terrain_cells = leaves(&outlines, V1_CELL_LOG2.into());
        let mut steps: Vec<Step> =
            terrain_cells.iter().map(|(&leaf, cells)| terrain(leaf, cells, &land, &glo30)).collect();
        let (Some(extract), Some(land_polygons)) = osm_sources else {
            return Ok(steps);
        };
        let captures = captures(env, store, &extract, &area)?;
        let mut osm_leaves = BTreeSet::new();
        let mut network = BTreeMap::new();
        for band in BandTable::recommended().bands {
            let reads_terrain = obc_pack::step::reads_terrain(&band).map_err(Unplanned::Failed)?;
            for (leaf, cells) in leaves(&outlines, band.cell_log2) {
                osm_leaves.insert(leaf);
                steps.push(map_cells(&band, leaf, &cells, &land_polygons, reads_terrain));
                if band.has_nav() {
                    network.insert(leaf, cells);
                }
            }
        }
        for (collection, inputs) in captures {
            steps.push(content(collection, inputs));
            for (&leaf, cells) in &network {
                steps.push(artifacts(collection, leaf, cells));
            }
        }
        let extract = Input::Snapshot { source: EXTRACTS.into(), version: extract, params: area, files: Vec::new() };
        steps.push(osm(extract, &osm_leaves));
        Ok(steps)
    }
}

/// The two Wikimedia captures of a region, `landmarks` and `peaks`, with the files of [`CAPTURES`],
/// and the extract and the `.poly` that each read, by digest. `Err` names the fetches of the
/// captures that the store lacks.
fn captures(
    env: &Env,
    store: &Store,
    extract: &str,
    area: &[(String, String)],
) -> Result<Vec<(&'static str, Vec<Input>)>, Unplanned> {
    let poly = version(env, store, POLY, area).map_err(Unplanned::Failed)?;
    let poly = poly.map_err(|_| Unplanned::Failed(format!("the store has no {POLY} of {area:?}")))?;
    let now = [(EXTRACTS, extract), (POLY, poly.as_str())].map(|(source, version)| file(store, source, version, area));
    let [osm, poly] = now.map(|file| file.map(|(_, sha256)| format!("sha256:{sha256}")));
    let (osm, poly) = (osm?, poly?);
    let mut wanted = Vec::new();
    let mut captures = Vec::new();
    for collection in ["landmarks", "peaks"] {
        let params = capture_params(env, store, collection, &area[0].1, (&osm, &poly))?;
        let mut inputs = Vec::new();
        for source in CAPTURES {
            if let Some(version) = snapshot_version(env, store, source, &params, &mut wanted)? {
                let params = params.clone();
                inputs.push(Input::Snapshot { source: source.into(), version, params, files: Vec::new() });
            }
        }
        // The extract and the `.poly` that the capture read, which need not be those of now. Their
        // input names the file, not the params: a fetch has one version in a release.
        for (source, name) in [(EXTRACTS, "osm"), (POLY, "poly")] {
            let digest = &params.iter().find(|(param, _)| param == name).expect("a capture param").1;
            let (version, file) = by_digest(store, source, area, digest)?;
            inputs.push(Input::Snapshot { source: source.into(), version, params: Vec::new(), files: vec![file] });
        }
        captures.push((collection, inputs));
    }
    match wanted.is_empty() {
        true => Ok(captures),
        false => Err(Unplanned::NeedsFetch(wanted)),
    }
}

/// The params of the capture of `collection` for the region `area`: those of the capture that `env`
/// reads (a saved plan, or live), or else of the newest capture in the store, until a source of
/// [`CAPTURES`] moves. A new extract alone asks for no new capture. A moved capture, or the first
/// one, reads the extract and the `.poly` of `now`.
fn capture_params(
    env: &Env,
    store: &Store,
    collection: &str,
    area: &str,
    now: (&str, &str),
) -> Result<Vec<(String, String)>, Unplanned> {
    let value = |params: &[(String, String)], name: &str| {
        params.iter().find(|(param, _)| param == name).map(|(_, value)| value.clone())
    };
    let ours = |params: &[(String, String)]| {
        value(params, "collection").as_deref() == Some(collection) && value(params, "area").as_deref() == Some(area)
    };
    let mut kept = None;
    if !CAPTURES.iter().any(|source| env.moves.contains_key(*source)) {
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
    let (osm, poly) = match &kept {
        Some(params) => (value(params, "osm").unwrap_or_default(), value(params, "poly").unwrap_or_default()),
        None => (now.0.to_string(), now.1.to_string()),
    };
    let pairs = [("collection", collection), ("area", area), ("osm", &osm), ("poly", &poly)];
    Ok(pairs.map(|(name, value)| (name.to_string(), value.to_string())).to_vec())
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
    let file = match &names[..] {
        [name] => snapshot.files.into_iter().find(|file| &file.name == name).ok_or_else(missing)?,
        _ => return Err(Unplanned::Failed(format!("a fetch of {source} with {params:?} gives {} files", names.len()))),
    };
    Ok((file.name, file.sha256))
}

/// The version and the name of the file of a fetch of `source` with `params` whose digest is
/// `sha256:<hex>`.
fn by_digest(
    store: &Store,
    source: &str,
    params: &[(String, String)],
    digest: &str,
) -> Result<(String, String), Unplanned> {
    for request in store.requests(source, params).map_err(Unplanned::Failed)? {
        let (name, sha256) = file(store, source, &request.version, params)?;
        if digest.strip_prefix("sha256:") == Some(sha256.as_str()) {
            return Ok((request.version, name));
        }
    }
    Err(Unplanned::Failed(format!("the store has no file of {source} {params:?} with {digest}, which a capture read")))
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
        code: Code { paths: Vec::new(), crates: vec!["obc-pack".into()] },
        outputs: vec![collection.into()],
        run: Run::Rust(run),
        client: false,
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
            inputs.push(Input::Layer { name: "maps/osm".into(), files: vec![obc_bake::step::leaf_pbf(leaf)] });
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
        code: Code { paths: Vec::new(), crates: vec!["obc-pack".into()] },
        outputs: vec![collection.into()],
        run: Run::Rust(run),
        client: true,
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
fn osm(extract: Input, leaves: &BTreeSet<LeafId>) -> Step {
    Step {
        name: "maps/osm".into(),
        inputs: vec![extract],
        options: serde_json::json!({"leaves": leaves.iter().map(|leaf| [leaf.i, leaf.j]).collect::<Vec<_>>()}),
        code: Code { paths: Vec::new(), crates: vec!["obc-bake".into()] },
        outputs: vec!["osm".into()],
        run: Run::Rust(obc_bake::step::osm),
        client: false,
    }
}

/// The map cells of one band in one leaf, from the OSM of the leaf, the land polygons and, for a
/// band whose bytes read heights, the terrain of the leaf.
fn map_cells(band: &Band, leaf: LeafId, cells: &[CellId], land_polygons: &str, reads_terrain: bool) -> Step {
    let land_polygons =
        Input::Snapshot { source: LAND.into(), version: land_polygons.into(), params: Vec::new(), files: Vec::new() };
    let osm = Input::Layer { name: "maps/osm".into(), files: vec![obc_bake::step::leaf_pbf(leaf)] };
    let mut inputs = vec![osm, land_polygons];
    if reads_terrain {
        inputs.push(Input::layer(leaf_layer("maps/terrain", leaf)));
    }
    Step {
        name: leaf_layer(&format!("maps/{}", band.id), leaf),
        inputs,
        options: serde_json::json!({
            "band": band.id,
            "leaf": [i64::from(LEAF_LOG2), leaf.i, leaf.j],
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        code: Code { paths: Vec::new(), crates: vec!["obc-pack".into()] },
        outputs: vec!["cells".into()],
        run: Run::Rust(obc_pack::step::cells),
        client: true,
    }
}

/// The terrain cells of one leaf, from the GLO-30 tiles that the square of each cell reaches. A
/// square that the tile list does not name is sea, and has no tile to read.
fn terrain(leaf: LeafId, cells: &[CellId], land: &HashSet<&str>, glo30: &str) -> Step {
    let source_box = |cell: &CellId| obc_bake::terrain::source_bbox([*cell]).expect("a cell has a box");
    let tiles = cells.iter().flat_map(|cell| obc_dem::fetch::tiles_for(source_box(cell)));
    let tiles: BTreeSet<String> = tiles.map(|tile| tile.stem()).filter(|tile| land.contains(tile.as_str())).collect();
    let params: Vec<_> = tiles.into_iter().map(|tile| ("tile".to_string(), tile)).collect();
    // A leaf at sea reads no snapshot: an input without params reads every file of its version.
    let inputs = match params.is_empty() {
        true => Vec::new(),
        false => vec![Input::Snapshot { source: GLO30.into(), version: glo30.into(), params, files: Vec::new() }],
    };
    Step {
        name: leaf_layer("maps/terrain", leaf),
        inputs,
        options: serde_json::json!({
            "posting_log2": V1_POSTING_LOG2,
            "cell_log2": V1_CELL_LOG2,
            "cells": cells.iter().map(|cell| [cell.i, cell.j]).collect::<Vec<_>>(),
        }),
        code: Code { paths: Vec::new(), crates: vec!["obc-dem".into()] },
        outputs: vec!["terrain".into()],
        run: Run::Rust(obc_dem::step::terrain),
        client: true,
    }
}

/// The outline of each region that `env.region` resolves to, or `None` while the store lacks the
/// `.poly` of a Geofabrik area; then `wanted` has its fetch.
pub(crate) fn outlines(
    env: &Env,
    regions: &Regions,
    store: &Store,
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Vec<Coverage>>, Unplanned> {
    let mut outlines = Some(Vec::new());
    for id in regions.leaves(&env.region).map_err(Unplanned::Failed)? {
        let poly = match &regions.get(id).expect("a leaf region exists").area {
            Area::Box { bbox } => Some(box_poly(bbox)),
            Area::Geofabrik => text(env, store, POLY, &[("area".to_string(), id.to_string())], wanted)?,
            Area::Polygon { .. } => return Err(invalid(format!("region `{id}`: the device maps read no polygon yet"))),
            Area::Union { .. } => unreachable!("a leaf region is not a union"),
        };
        let outline =
            poly.map(|poly| Coverage::parse_poly(&poly).map_err(|e| Unplanned::Failed(format!("{id}.poly: {e}"))));
        match (outline.transpose()?, &mut outlines) {
            (Some(outline), Some(outlines)) => outlines.push(outline),
            _ => outlines = None,
        }
    }
    Ok(outlines)
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
    format!("box\n1\n   {west} {south}\n   {east} {south}\n   {east} {north}\n   {west} {north}\n   {west} {south}\nEND\nEND\n")
}

#[cfg(test)]
pub(crate) mod tests {
    use std::path::{Path, PathBuf};

    use obc_data::engine::plan::{plan, Plan};
    use obc_data::engine::runs::{Context, Limits, Run as RunLog};
    use obc_data::engine::Request;
    use obc_data::fetch::http::Http;
    use obc_data::store::{sha256_hex, write_atomic, FileRecord, Requested, Snapshot};
    use obc_pack::step::CAPTURES;

    use super::*;

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
    fn grimsel(glo30: &str) -> (Env, Regions) {
        let live = BTreeMap::from([
            ((GLO30.into(), Vec::new()), [glo30.to_string()].into()),
            ((TILE_LIST.into(), Vec::new()), ["1".to_string()].into()),
        ]);
        let env = Env { name: "test".into(), region: "grimsel".into(), live, ..Env::default() };
        (env, Regions::load(&root()).unwrap())
    }

    const FREIBURG: &str =
        "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";

    /// A Geofabrik region about Freiburg, in leaf `0037-0032`, whose `.poly` the store has, and the
    /// tile list.
    fn freiburg(store: &Store) -> (Env, Regions) {
        let area = [("area".to_string(), "europe/test".to_string())];
        fetched(store, POLY, "1", &area, &[("europe/test.poly".into(), FREIBURG.into())]);
        with_tile_list(store, &["N47_00_E007", "N48_00_E007"]);
        let region = obc_data::regions::parse_region("europe/test", "name = \"Test\"\nkind = \"geofabrik\"\n").unwrap();
        let live = BTreeMap::from([
            ((GLO30.into(), Vec::new()), ["1".to_string()].into()),
            ((TILE_LIST.into(), Vec::new()), ["1".to_string()].into()),
        ]);
        let env = Env { name: "test".into(), region: "europe/test".into(), live, ..Env::default() };
        (env, Regions::new(vec![region]).unwrap())
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

    /// Record the GLO-30 tiles that `steps` read at `version`, each with the text `text(tile)`.
    fn glo30_fetched(store: &Store, steps: &[Step], version: &str, text: impl Fn(&str) -> String) {
        for step in steps {
            let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
            let files: Vec<_> = params.iter().map(|(_, tile)| (format!("{tile}/{tile}.tif"), text(tile))).collect();
            fetched(store, GLO30, version, params, &files);
        }
    }

    fn fake(request: &Request) -> Result<(), String> {
        std::fs::create_dir(request.output.join("terrain")).map_err(|e| e.to_string())?;
        std::fs::write(request.output.join("terrain/empty.json"), "[]").map_err(|e| e.to_string())
    }

    fn builds(plan: &Plan) -> Vec<&str> {
        plan.groups.iter().flat_map(|group| &group.builds).map(|build| build.step.as_str()).collect()
    }

    fn tiles(step: &Step) -> Vec<&str> {
        let Input::Snapshot { params, .. } = &step.inputs[0] else { unreachable!() };
        params.iter().map(|(_, tile)| &tile["Copernicus_DSM_COG_10_".len()..][..11]).collect()
    }

    #[test]
    fn a_new_glo30_tile_in_one_leaf_rebuilds_that_leaf_only() {
        let temp = temp("leaves");
        let store = Store::at(temp.0.join("store"));
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E007", "N47_00_E008"]);
        let steps = |version: &str| {
            let (env, regions) = grimsel(version);
            let mut steps = Maps.steps(&env, &regions, &store).unwrap();
            steps.iter_mut().for_each(|step| step.run = Run::Rust(fake));
            steps
        };
        let names: Vec<String> = steps("1").iter().map(|step| step.name.clone()).collect();
        assert_eq!(names, ["maps/terrain/0037-0032", "maps/terrain/0037-0033"]);
        let west = "Copernicus_DSM_COG_10_N46_00_E007_00_DEM";
        glo30_fetched(&store, &steps("1"), "1", |tile| format!("{tile} 1"));
        glo30_fetched(&store, &steps("2"), "2", |tile| format!("{tile} {}", if tile == west { 2 } else { 1 }));

        let (root, http) = (root(), Http::new());
        let context = Context { store: &store, root: &root, sources: &[], http: &http, limits: Limits::machine() };
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
        let (env, regions) = grimsel("1");
        let Err(Unplanned::NeedsFetch(wanted)) = Maps.steps(&env, &regions, &store) else { panic!("no tile list") };
        assert_eq!(wanted, [Wanted { source: TILE_LIST.into(), version: Some("1".into()), params: Vec::new() }]);

        // As if N47_00_E007 were sea.
        with_tile_list(&store, &["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        let steps = Maps.steps(&env, &regions, &store).unwrap();
        assert_eq!(tiles(&steps[0]), ["N46_00_E007", "N46_00_E008", "N47_00_E008"]);
        // The squares of the eastern cells reach no tile west of 8°.
        assert_eq!(tiles(&steps[1]), ["N46_00_E008", "N47_00_E008"]);
    }

    #[test]
    fn a_band_reads_the_osm_of_its_leaf_the_land_and_the_terrain_when_its_bytes_have_heights() {
        let temp = temp("bands");
        let store = Store::at(temp.0.join("store"));
        let (env, regions) = freiburg(&store);
        let Err(Unplanned::NeedsFetch(wanted)) = Maps.steps(&env, &regions, &store) else { panic!("no extract") };
        assert_eq!(wanted.iter().map(|fetch| fetch.source.as_str()).collect::<Vec<_>>(), [EXTRACTS, LAND]);
        with_osm(&store);
        with_captures(&store, "1");
        let steps = Maps.steps(&env, &regions, &store).unwrap();
        let reads = |name: &str| -> Vec<String> {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = |input: &Input| match input {
                Input::Snapshot { source, .. } => source.clone(),
                Input::Layer { name, files } => format!("{name} {files:?}"),
            };
            step.inputs.iter().map(read).collect()
        };
        let osm = r#"maps/osm ["osm/0037-0032.osm.pbf"]"#;
        assert_eq!(reads("maps/osm"), [EXTRACTS]);
        let intermediate = |name: &str| name == "maps/osm" || name.ends_with("-content");
        assert!(steps.iter().all(|step| step.client != intermediate(&step.name)), "no client reads an intermediate");
        assert_eq!(reads("maps/coarse/0037-0032"), [osm, LAND]);
        for band in ["mid", "fine", "network"] {
            assert_eq!(reads(&format!("maps/{band}/0037-0032")), [osm, LAND, "maps/terrain/0037-0032 []"], "{band}");
        }
        assert!(steps.iter().all(|step| !step.code.paths.iter().any(|path| path == "Cargo.lock")));
    }

    #[test]
    fn a_capture_names_the_extract_and_the_poly_by_digest_and_its_steps_read_it() {
        let temp = temp("captures");
        let store = Store::at(temp.0.join("store"));
        let (env, regions) = freiburg(&store);
        with_osm(&store);
        let Err(Unplanned::NeedsFetch(wanted)) = Maps.steps(&env, &regions, &store) else { panic!("no capture") };
        let fetch = |collection, source: &str| Wanted {
            source: source.into(),
            version: None,
            params: capture_params(collection, "europe/test", "osm", FREIBURG),
        };
        let captures =
            ["landmarks", "peaks"].into_iter().flat_map(|collection| CAPTURES.map(|source| fetch(collection, source)));
        assert_eq!(wanted, captures.collect::<Vec<_>>());
        with_captures(&store, "1");
        let steps = Maps.steps(&env, &regions, &store).unwrap();
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
    /// capture read, and the map cells read the new one. A capture that moves reads the new one.
    #[test]
    fn a_capture_keeps_its_extract_until_it_moves() {
        let temp = temp("capture-extract");
        let store = Store::at(temp.0.join("store"));
        let (mut env, regions) = freiburg(&store);
        with_osm(&store);
        with_captures(&store, "1");
        let area = [("area".to_string(), "europe/test".to_string())];
        fetched(&store, EXTRACTS, "2", &area, &[("europe/test-2.osm.pbf".into(), "osm 2".into())]);
        let steps = Maps.steps(&env, &regions, &store).unwrap();
        let extract = |name: &str| {
            let step = steps.iter().find(|step| step.name == name).unwrap();
            let read = step.inputs.iter().find_map(|input| match input {
                Input::Snapshot { source, version, files, .. } if source == EXTRACTS => Some((version, files)),
                _ => None,
            });
            read.map(|(version, files)| (version.clone(), files.clone()))
        };
        assert_eq!(extract("maps/osm"), Some(("2".into(), Vec::new())));
        for content in ["maps/landmark-content", "maps/peak-content"] {
            assert_eq!(extract(content), Some(("1".into(), vec!["europe/test.osm.pbf".into()])), "{content}");
        }

        env.moves.insert("wikidata".into(), None);
        let Err(Unplanned::NeedsFetch(wanted)) = Maps.steps(&env, &regions, &store) else { panic!("a new capture") };
        let moved = capture_params("landmarks", "europe/test", "osm 2", FREIBURG);
        assert!(wanted.iter().any(|fetch| fetch.source == "wikidata" && fetch.params == moved), "{wanted:?}");
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
        let steps = Maps.steps(&env, &regions, &store).unwrap();
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
}
