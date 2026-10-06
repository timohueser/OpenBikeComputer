//! The planner. Its layers cover the region of the environment: `planner/osm` is the OSM of the
//! region, `planner/terrain` the terrain of the maps, `planner/routing` the routing package with
//! its grid, and `planner/search/dump` the search records of the OSM. `planner/overlays`,
//! `planner/assets`, `planner/model`, `planner/places`, the other `planner/search/*` layers and
//! the optional layers `planner/climate`, `planner/snow` and `planner/sun` are Python steps.
//! `planner/osm`, `planner/search/policy`, `planner/search/dump` and `planner/search/records` are
//! intermediate layers: no client reads them. `data/planner.toml` holds the options that are the
//! same for each region.

use std::collections::HashSet;

use obc_data::engine::{Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{version, Product, Unplanned, Wanted};
use obc_data::regions::{Area, Regions};
use obc_data::sources::attribution;
use obc_data::store::Store;
use obc_dem::step::GLO30;
use route_build::grid::{mercator, tile_bounds};
use serde::Deserialize;
use serde_json::json;

use crate::maps::{invalid, outlines, text, EXTRACTS, TILE_LIST};
use crate::python;

const SEARCH: &str = "apps/planner-search";
/// `apps/planner-search/records.py` and the files that it reads: the data kinds of the query
/// contract, and the POI kinds of the web planner, which the places also read.
const RECORDS: [&str; 3] = ["apps/planner-search/records.py", "apps/planner-search/query/contract.json", POI_KINDS];
const POI_KINDS: &str = "builder/app/src/lib/planner/poi-kinds.json";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    terrain: Terrain,
    routing: Routing,
    climate: Climate,
    snow: Snow,
    sun: Sun,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Climate {
    first_year: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Snow {
    seasons: [u16; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Sun {
    horizon_samples: u8,
    horizon_directions: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Terrain {
    margin_m: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Routing {
    profiles: Vec<String>,
}

pub struct Planner;

impl Product for Planner {
    fn name(&self) -> &'static str {
        "planner"
    }

    fn optional(&self) -> &'static [&'static str] {
        &["climate", "snow", "sun"]
    }

    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
        let config: Config = toml::from_str(include_str!("../../../data/planner.toml"))
            .map_err(|e| Unplanned::Failed(format!("data/planner.toml: {e}")))?;
        let region =
            regions.get(&env.region).ok_or_else(|| Unplanned::Failed(format!("no region `{}`", env.region)))?;
        if region.area != Area::Geofabrik {
            return Err(invalid(format!(
                "region `{}`: the planner reads the OSM of one Geofabrik area only",
                region.id
            )));
        }
        if region.countries.is_empty() {
            return Err(invalid(format!("region `{}` names no `countries`, which the route catalog needs", region.id)));
        }
        let Some(time_zone) = &region.time_zone else {
            return Err(invalid(format!("region `{}` names no `time_zone`, which the search needs", region.id)));
        };
        let on = |layer: &str| env.layers.iter().any(|name| name == layer);
        let mut wanted = Vec::new();
        let glo30 = version(env, store, GLO30, &[]).map_err(Unplanned::Failed)?.map_err(|fetch| wanted.push(fetch));
        let outlines = outlines(env, regions, store, &mut wanted)?;
        let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
        let area = vec![("area".to_string(), region.id.clone())];
        let extract = snapshot(env, store, EXTRACTS, area, &mut wanted)?;
        let assets = vec![
            snapshot(env, store, "protomaps-assets", Vec::new(), &mut wanted)?,
            snapshot(env, store, "tangrams-icons", Vec::new(), &mut wanted)?,
        ];
        let model = snapshot(env, store, "query-model", Vec::new(), &mut wanted)?;
        let country_data = snapshot(env, store, "nominatim-country-data", Vec::new(), &mut wanted)?;
        let (Some(outlines), Some(tile_list), Ok(glo30)) = (outlines, tile_list, glo30) else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let (west, south, east, north) = outlines[0].bbox();
        let bounds = [west, south, east, north].map(|udeg| udeg as f64 / 1e6);
        let bbox = ("bbox".to_string(), bounds.map(|degrees| degrees.to_string()).join(","));
        let coverage = terrain_bounds(bounds, config.terrain.margin_m);
        let osm = Step {
            name: "planner/osm".into(),
            inputs: vec![extract],
            options: json!({"path": "osm.pbf"}),
            code: Code { paths: Vec::new(), crates: vec!["obc-data".into()] },
            outputs: vec!["osm.pbf".into()],
            run: Run::Rust(obc_data::engine::pass),
            client: false,
        };
        let terrain = Step {
            name: "planner/terrain".into(),
            inputs: tiles(coverage, &land, &glo30),
            options: json!({"bounds": coverage}),
            code: Code { paths: vec!["data/sources.toml".into()], crates: vec!["obc-dem".into()] },
            outputs: vec!["terrain.mbtiles".into()],
            run: Run::Rust(obc_dem::step::planner_terrain),
            client: true,
        };
        // The last part of the id: the old planner names its files after it.
        let name = region.id.rsplit('/').next();
        let routing = Step {
            name: "planner/routing".into(),
            inputs: std::iter::once(Input::layer(osm.name.clone())).chain(tiles(bounds, &land, &glo30)).collect(),
            options: json!({
                "region": name,
                "bounds": bounds,
                "profiles": config.routing.profiles,
                "countries": region.countries,
            }),
            code: Code { paths: vec!["data/sources.toml".into()], crates: vec!["route-build".into()] },
            outputs: vec!["routing".into(), "blocks".into(), "routes".into()],
            run: Run::Rust(route_build::step::step),
            client: true,
        };
        let overlays = python(
            "planner/overlays",
            vec![Input::layer(routing.name.clone())],
            json!({"attribution": attribution("osm-planet")}),
            ("tools.planner_overlays", Some("planner-maps")),
            &["tools/planner_overlays.py", "tools/planner_geo.py", "tools/planner_mvt.py"],
            &["overlays.pmtiles"],
        );
        let assets = python(
            "planner/assets",
            assets,
            json!({}),
            ("tools.planner_assets", None),
            &["tools/planner_assets.py"],
            &["assets"],
        );
        let model = python(
            "planner/model",
            vec![model],
            json!({}),
            ("apps/planner-search/setup.py", None),
            &[
                "apps/planner-search/setup.py",
                "apps/planner-search/query/artifacts.py",
                "apps/planner-search/query/schema.py",
                "apps/planner-search/query/contract.json",
            ],
            &["model"],
        );
        let policy = Step {
            client: false,
            ..python(
                "planner/search/policy",
                vec![country_data],
                json!({}),
                ("host/obc-search-bake/policy.py", Some("planner-search")),
                &["host/obc-search-bake/policy.py"],
                &["policy.json", "country_osm_grid.sql.gz"],
            )
        };
        let dump = Step {
            name: "planner/search/dump".into(),
            inputs: vec![Input::layer(osm.name.clone()), Input::layer(policy.name.clone())],
            options: json!({"country": region.countries[0].to_lowercase()}),
            code: Code { paths: Vec::new(), crates: vec!["obc-search-bake".into()] },
            outputs: vec!["search.jsonl.zst".into()],
            run: Run::Rust(obc_search_bake::step::step),
            client: false,
        };
        let records = Step {
            client: false,
            ..python(
                "planner/search/records",
                vec![Input::layer(dump.name.clone())],
                json!({}),
                ("apps/planner-search/split.py", Some("planner-search")),
                &[["apps/planner-search/split.py"].as_slice(), &RECORDS].concat(),
                &["pois.jsonl.zst", "addresses.jsonl.zst"],
            )
        };
        // One search database per component: the POIs and the addresses.
        let search = |component: &str| {
            // writer.py imports pois.py or addresses.py by the component.
            let files = ["build.py", "writer.py", "storage.py", "index.py", "pois.py", "addresses.py"];
            let data = ["schema.sql", "indexes.sql", "web/address-terms.json"];
            let files: Vec<String> = files.iter().chain(&data).map(|file| format!("{SEARCH}/{file}")).collect();
            let files: Vec<&str> = files.iter().map(String::as_str).chain(RECORDS).collect();
            python(
                &format!("planner/search/{component}"),
                vec![Input::layer(records.name.clone())],
                json!({
                    "component": component,
                    "region": name,
                    "bounds": bounds,
                    "countries": region.countries,
                    "time_zone": time_zone,
                    "attribution": attribution("osm-planet"),
                }),
                (&format!("{SEARCH}/build.py"), Some("planner-search")),
                &files,
                &[component],
            )
        };
        let (pois, addresses) = (search("pois"), search("addresses"));
        let places = python(
            "planner/places",
            vec![Input::layer(pois.name.clone())],
            json!({}),
            ("tools.planner_places", Some("planner-maps")),
            &["tools/planner_places.py", "tools/planner_mvt.py", POI_KINDS],
            &["places.pmtiles"],
        );
        let mut steps =
            vec![osm, terrain, routing, overlays, assets, model, policy, dump, records, pois, addresses, places];
        if on("climate") {
            let first_year = config.climate.first_year;
            let params = vec![bbox.clone(), ("first-year".to_string(), first_year.to_string())];
            steps.push(python(
                "planner/climate",
                vec![snapshot(env, store, "era5-land", params, &mut wanted)?],
                // `{year}` is the year after the ten years.
                json!({"bounds": bounds, "first_year": first_year, "attribution": attribution("era5-land")}),
                ("tools.planner_climate", Some("planner-climate")),
                &["tools/planner_climate.py", "tools/planner_geo.py"],
                &["climate.pmtiles"],
            ));
        }
        if on("snow") {
            let [first, last] = config.snow.seasons;
            let seasons = ("seasons".to_string(), format!("{first}-{last}"));
            let snow = snapshot(env, store, "hr-wsi", vec![bbox, seasons], &mut wanted)?;
            // The credit of HR-WSI names a year: the year of the capture.
            let Input::Snapshot { version, .. } = &snow else { unreachable!("a fetch is a snapshot input") };
            let year = version.get(..4).and_then(|year| year.parse::<u16>().ok());
            steps.push(python(
                "planner/snow",
                vec![snow],
                json!({"bounds": bounds, "seasons": [first, last], "year": year, "attribution": attribution("hr-wsi")}),
                ("tools.planner_snow", Some("planner-snow")),
                &["tools/planner_snow.py", "tools/planner_geo.py"],
                &["snow.pmtiles"],
            ));
        }
        if on("sun") {
            steps.push(python(
                "planner/sun",
                vec![Input::layer("planner/terrain")],
                json!({
                    "bounds": bounds,
                    "time_zone": region.time_zone,
                    "distance_m": config.terrain.margin_m as u32,
                    "horizon_samples": config.sun.horizon_samples,
                    "horizon_directions": config.sun.horizon_directions,
                }),
                ("tools.planner_sun", Some("planner-sun")),
                &[
                    "tools/planner_sun.py",
                    "tools/planner_sun_horizons.py",
                    "tools/planner_map_archive.py",
                    "tools/planner_geo.py",
                ],
                &["sun.pmtiles"],
            ));
        }
        match wanted.is_empty() {
            true => Ok(steps),
            false => Err(Unplanned::NeedsFetch(wanted)),
        }
    }
}

/// The input of the fetch of `source` with `params`, at its version. While the store lacks that
/// fetch, `wanted` names it, and the input has no version: the step list is not used then.
fn snapshot(
    env: &Env,
    store: &Store,
    source: &str,
    params: Vec<(String, String)>,
    wanted: &mut Vec<Wanted>,
) -> Result<Input, Unplanned> {
    let version = version(env, store, source, &params).map_err(Unplanned::Failed)?.unwrap_or_else(|fetch| {
        wanted.push(fetch);
        String::new()
    });
    Ok(Input::Snapshot { source: source.into(), version, params, files: Vec::new() })
}

/// The GLO-30 tiles of `bounds` that the tile list names, as an input. A square that it does not
/// name is sea, and `bounds` at sea reads no snapshot.
fn tiles(bounds: [f64; 4], land: &HashSet<&str>, glo30: &str) -> Vec<Input> {
    let tiles = obc_dem::planner::tiles(bounds).into_iter().map(|tile| tile.stem());
    let params: Vec<_> =
        tiles.filter(|tile| land.contains(tile.as_str())).map(|tile| ("tile".to_string(), tile)).collect();
    match params.is_empty() {
        true => Vec::new(),
        false => vec![Input::Snapshot { source: GLO30.into(), version: glo30.into(), params, files: Vec::new() }],
    }
}

/// The terrain of the maps around `bounds`: the zoom 10 tiles that it touches and their
/// neighbours, because the contours of a tile read a 3 × 3 tile neighbourhood; and at least
/// `margin_m` metres around `bounds`.
fn terrain_bounds([west, south, east, north]: [f64; 4], margin_m: f64) -> [f64; 4] {
    let latitude = margin_m / 110_000.0;
    let longitude = latitude / south.abs().max(north.abs()).to_radians().cos();
    let tiles = zoom_10_neighbourhood([west, south, east, north]);
    [
        tiles[0].min(west - longitude),
        tiles[1].min(south - latitude),
        tiles[2].max(east + longitude),
        tiles[3].max(north + latitude),
    ]
}

fn zoom_10_neighbourhood([west, south, east, north]: [f64; 4]) -> [f64; 4] {
    const ZOOM: u32 = 10;
    let last = (1 << ZOOM) - 1;
    let ((left, top), (right, bottom)) = (mercator(west, north, ZOOM), mercator(east, south, ZOOM));
    let (left, top) = ((left.floor() as u32).saturating_sub(1), (top.floor() as u32).saturating_sub(1));
    let (right, bottom) = ((right.floor() as u32 + 1).min(last), (bottom.floor() as u32 + 1).min(last));
    let [west, south, ..] = tile_bounds(ZOOM, left, bottom);
    let [.., east, north] = tile_bounds(ZOOM, right, top);
    [west, south, east, north]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use obc_data::engine::plan::plan;
    use obc_data::regions::parse_region;

    use super::*;
    use crate::maps::tests::{fetched, root, temp, Temp};
    use crate::maps::Maps;

    const AREA: &str = "europe/test";

    /// An environment with `layers` on that names a version of every source the planner reads but
    /// the extract: live reads the sources without params, and the captures move.
    fn env(region: &str, layers: &[&str]) -> Env {
        let read = [GLO30, TILE_LIST, "protomaps-assets", "tangrams-icons", "query-model", "nominatim-country-data"];
        let live = read.map(|source| ((source.into(), Vec::new()), BTreeSet::from(["1".into()])));
        let moves = ["era5-land", "hr-wsi"].map(|source| (source.into(), Some("2026-10-01".into())));
        let layers = layers.iter().map(|layer| layer.to_string()).collect();
        Env {
            name: "test".into(),
            region: region.into(),
            layers,
            live: live.into(),
            moves: moves.into(),
            ..Env::default()
        }
    }

    fn regions() -> Regions {
        let region = |id: &str, text: &str| parse_region(id, &format!("name = \"{id}\"\n{text}")).unwrap();
        Regions::new(vec![
            region(AREA, "kind = \"geofabrik\"\ncountries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n"),
            region("no-countries", "kind = \"geofabrik\"\ntime_zone = \"Europe/Berlin\"\n"),
            region("no-time-zone", "kind = \"geofabrik\"\ncountries = [\"DE\"]\n"),
            region("boxed", "kind = \"box\"\nbox = [7.79, 47.99, 7.82, 48.02]\ncountries = [\"DE\"]\n"),
        ])
        .unwrap()
    }

    /// A store with what the step list of `AREA` reads: its `.poly`, a box near Freiburg; the
    /// tile list, which names the two squares west of 8°; the land polygons; and the extract of
    /// each of `days`.
    fn store(temp: &Temp, days: &[&str]) -> Store {
        let store = Store::at(temp.0.join("store"));
        let area = [("area".to_string(), AREA.to_string())];
        let poly = "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";
        fetched(&store, "geofabrik-poly", "2026-10-01", &area, &[(format!("{AREA}.poly"), poly.into())]);
        let list = "Copernicus_DSM_COG_10_N47_00_E007_00_DEM\nCopernicus_DSM_COG_10_N48_00_E007_00_DEM\n";
        fetched(&store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list.into())]);
        fetched(&store, obc_pack::step::LAND, "1", &[], &[("land-polygons-split-3857.zip".into(), "land".into())]);
        for day in days {
            fetched(&store, EXTRACTS, day, &area, &[(format!("{AREA}-{day}.osm.pbf"), (*day).into())]);
        }
        store
    }

    fn tiles(input: &Input) -> Vec<&str> {
        let Input::Snapshot { params, .. } = input else { panic!("not a snapshot") };
        params.iter().map(|(_, tile)| &tile["Copernicus_DSM_COG_10_".len()..][..11]).collect()
    }

    #[test]
    fn a_geofabrik_region_reads_its_newest_extract_and_the_tiles_of_its_bounds() {
        let temp = temp("planner-steps");
        let Err(Unplanned::NeedsFetch(wanted)) = Planner.steps(&env(AREA, &[]), &regions(), &store(&temp, &[])) else {
            panic!("the store has no extract");
        };
        let area = vec![("area".to_string(), AREA.to_string())];
        assert_eq!(wanted, [Wanted { source: EXTRACTS.into(), version: None, params: area.clone() }]);

        let steps = Planner.steps(&env(AREA, &[]), &regions(), &store(&temp, &["2026-10-01", "2026-10-02"])).unwrap();
        let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
        let layers = [
            "osm",
            "terrain",
            "routing",
            "overlays",
            "assets",
            "model",
            "search/policy",
            "search/dump",
            "search/records",
            "search/pois",
            "search/addresses",
            "places",
        ];
        assert_eq!(names, layers.map(|layer| format!("planner/{layer}")), "no optional layer is on");
        let intermediate: Vec<&str> = steps.iter().filter(|step| !step.client).map(|step| step.name.as_str()).collect();
        assert_eq!(
            intermediate,
            ["planner/osm", "planner/search/policy", "planner/search/dump", "planner/search/records"]
        );
        let [osm, terrain, routing, .., pois, _, _] = &steps[..] else { unreachable!() };
        let Input::Snapshot { source, version, params, .. } = &osm.inputs[0] else { panic!("not a snapshot") };
        assert_eq!((source.as_str(), version.as_str(), params), (EXTRACTS, "2026-10-02", &area));
        // `terrain_coverage` of `tools/planner_bake.py` with a sun layer of 30 km.
        assert_eq!(terrain.options["bounds"], json!([7.382257389170511, 47.71727272727273, 8.4375, 48.45835188280866]));
        assert_eq!(
            tiles(&terrain.inputs[0]),
            ["N47_00_E007", "N48_00_E007"],
            "the tile list names no square east of 8°"
        );
        assert!(
            matches!(&routing.inputs[0], Input::Layer { name, files } if name == "planner/osm" && files.is_empty())
        );
        assert_eq!(tiles(&routing.inputs[1]), ["N47_00_E007", "N48_00_E007"]);
        assert_eq!(routing.options["bounds"], json!([7.79, 47.99, 7.82, 48.02]));
        assert_eq!((&routing.options["region"], &routing.options["countries"]), (&json!("test"), &json!(["DE"])));
        assert_eq!(
            (&pois.options["bounds"], &pois.options["time_zone"]),
            (&routing.options["bounds"], &json!("Europe/Berlin"))
        );
        // About Baden-Württemberg, and the box of the old planner recipe.
        let old = [7.03125, 47.04018214480666, 10.922533154247459, 50.07272727272727];
        assert_eq!(terrain_bounds([7.5, 47.5, 10.5, 49.8], 30_000.0), old);
        let old = [5.2734375, 45.33670190996811, 10.922970099182649, 50.28933925329178];
        assert_eq!(terrain_bounds([5.95, 45.8, 10.5, 49.85], 30_000.0), old);

        for region in ["boxed", "no-countries", "no-time-zone"] {
            let result = Planner.steps(&env(region, &[]), &regions(), &store(&temp, &["2026-10-01"]));
            assert!(matches!(result, Err(Unplanned::Invalid(_))), "{region}");
        }
    }

    #[test]
    fn the_code_of_route_build_is_the_code_of_the_routing_layer_only() {
        let temp = temp("planner-code");
        let store = store(&temp, &["2026-10-01"]);
        crate::maps::tests::without_models(&store, &env(AREA, &[]), &regions());
        let mut steps = Planner.steps(&env(AREA, &[]), &regions(), &store).unwrap();
        steps.extend(Maps.steps(&env(AREA, &[]), &regions(), &store).unwrap());
        for step in &steps {
            let files = step.code.files(&root()).unwrap();
            assert!(!files.contains_key("Cargo.lock"), "{} declares Cargo.lock", step.name);
            let route_build = files.keys().any(|path| path.starts_with("host/route-build/src/"));
            assert_eq!(route_build, step.name == "planner/routing", "{}", step.name);
            if step.name == "planner/terrain" {
                assert!(files.keys().any(|path| path.starts_with("host/obc-dem/src/")));
                assert!(!files.keys().any(|path| path.starts_with("host/route-build/")), "terrain reads route-build");
            }
        }
    }

    #[test]
    fn climate_adds_one_group_to_the_plan_and_changes_no_other() {
        let temp = temp("planner-climate");
        let store = store(&temp, &["2026-10-01"]);
        let plan =
            |layers: &[&str]| plan(&store, &root(), &Planner.steps(&env(AREA, layers), &regions(), &store).unwrap());
        let (without, with) = (plan(&[]).unwrap(), plan(&["climate"]).unwrap());
        let added: Vec<_> = with.groups.iter().filter(|group| !without.groups.contains(group)).collect();
        let [climate] = &added[..] else { panic!("{} groups are new", added.len()) };
        let builds: Vec<&str> = climate.builds.iter().map(|build| build.step.as_str()).collect();
        assert_eq!((climate.id.as_str(), builds), ("planner/climate", vec!["planner/climate"]));
        assert_eq!(with.groups.len(), without.groups.len() + 1);
    }

    /// The Python files of the repository that the Python file `file` imports, except in `main()`:
    /// the old command line, which no step runs. They are `tools/*.py` modules, and modules beside
    /// `file`, which a script imports by name, also in a function.
    fn imports(file: &str) -> Vec<String> {
        let text = std::fs::read_to_string(root().join(file)).unwrap();
        let dir = Path::new(file).parent().unwrap();
        let sibling = |name: &str| {
            let path = dir.join(format!("{name}.py"));
            root().join(&path).is_file().then(|| path.to_str().unwrap().to_string())
        };
        let mut found = Vec::new();
        let mut lines = text.lines();
        let mut in_main = false;
        while let Some(line) = lines.next() {
            if !line.is_empty() && !line.starts_with([' ', '#']) {
                in_main = line.starts_with("def main(");
            }
            if in_main {
                continue;
            }
            let mut line = line.trim().to_string();
            while line.contains('(') && !line.contains(')') {
                line += lines.next().unwrap_or(")");
            }
            let module = |name: &str| format!("tools/{}.py", name.trim().split(' ').next().unwrap_or_default());
            if let Some(names) = line.strip_prefix("from . import ").or(line.strip_prefix("from tools import ")) {
                found.extend(names.trim_matches(['(', ')']).split(',').map(module));
            } else if let Some(rest) = line.strip_prefix("from .").or(line.strip_prefix("from tools.")) {
                found.push(module(rest));
            } else if let Some(rest) = line.strip_prefix("import tools.") {
                found.push(module(rest));
            } else if let Some(rest) = line.strip_prefix("from ").or(line.strip_prefix("import ")) {
                found.extend(sibling(rest.split([' ', ',', '.']).next().unwrap_or_default()));
            }
        }
        found
    }

    #[test]
    fn a_python_step_declares_each_module_that_it_imports() {
        let temp = temp("planner-python");
        let store = store(&temp, &["2026-10-01"]);
        let steps = Planner.steps(&env(AREA, &["climate", "snow", "sun"]), &regions(), &store).unwrap();
        let mut python = 0;
        for step in &steps {
            let Run::Command(argv) = &step.run else { continue };
            python += 1;
            let entry = match &argv[argv.iter().position(|arg| arg == "python").unwrap() + 1..] {
                [flag, module, ..] if flag == "-m" => format!("{}.py", module.replace('.', "/")),
                [script, ..] => script.clone(),
                [] => panic!("{}: no entry", step.name),
            };
            let (mut pending, mut seen) = (vec![entry], BTreeSet::new());
            while let Some(file) = pending.pop() {
                if seen.insert(file.clone()) {
                    pending.extend(imports(&file));
                }
            }
            // `tools/planner_maps.py` runs the processes of the older planner bake. A step gets its
            // inputs in its request, so no step imports it.
            assert!(!seen.contains("tools/planner_maps.py"), "{} imports tools/planner_maps.py", step.name);
            for file in seen {
                assert!(step.code.paths.contains(&file), "{} runs {file}, which its code does not declare", step.name);
            }
        }
        assert_eq!(python, 11, "overlays, assets, model, four search layers, places, climate, snow and sun");
    }
}
