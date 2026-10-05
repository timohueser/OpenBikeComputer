//! The planner. Its layers cover the region of the environment: `planner/osm` is the OSM of the
//! region, `planner/terrain` the terrain of the maps, and `planner/routing` the routing package
//! with its grid. `data/planner.toml` holds the options that are the same for each region.

use std::collections::HashSet;

use obc_data::engine::{Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{version, Product, Unplanned};
use obc_data::regions::{Area, Regions};
use obc_data::store::Store;
use obc_dem::step::GLO30;
use route_build::grid::{mercator, tile_bounds};
use serde::Deserialize;
use serde_json::json;

use crate::maps::{invalid, outlines, text, TILE_LIST};

const EXTRACTS: &str = "geofabrik-extracts";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    terrain: Terrain,
    routing: Routing,
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

    fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
        let config: Config = toml::from_str(include_str!("../../../data/planner.toml"))
            .map_err(|e| invalid(format!("data/planner.toml: {e}")))?;
        let region = regions.get(&env.region).ok_or_else(|| invalid(format!("no region `{}`", env.region)))?;
        if region.area != Area::Geofabrik {
            return Err(invalid(format!(
                "region `{}`: the planner reads the OSM of one Geofabrik area only",
                region.id
            )));
        }
        if region.countries.is_empty() {
            return Err(invalid(format!("region `{}` names no `countries`, which the route catalog needs", region.id)));
        }
        let glo30 =
            env.version(GLO30).ok_or_else(|| invalid(format!("data/env/{}.toml pins no `{GLO30}`", env.name)))?;
        let mut wanted = Vec::new();
        let outlines = outlines(env, regions, store, &mut wanted)?;
        let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
        let area = vec![("area".to_string(), region.id.clone())];
        let extract =
            version(env, store, EXTRACTS, &area).map_err(Unplanned::Invalid)?.map_err(|fetch| wanted.push(fetch));
        let (Some(outlines), Some(tile_list), Ok(extract)) = (outlines, tile_list, extract) else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let (west, south, east, north) = outlines[0].bbox();
        let bounds = [west, south, east, north].map(|udeg| udeg as f64 / 1e6);
        let coverage = terrain_bounds(bounds, config.terrain.margin_m);
        let osm = Step {
            name: "planner/osm".into(),
            inputs: vec![Input::Snapshot {
                source: EXTRACTS.into(),
                version: extract,
                params: area,
                files: Vec::new(),
            }],
            options: json!({"path": "osm.pbf"}),
            code: Code { paths: Vec::new(), crates: vec!["obc-data".into()] },
            outputs: vec!["osm.pbf".into()],
            run: Run::Rust(obc_data::engine::pass),
        };
        let terrain = Step {
            name: "planner/terrain".into(),
            inputs: tiles(coverage, &land, glo30),
            options: json!({"bounds": coverage}),
            code: Code { paths: vec!["data/sources.toml".into()], crates: vec!["obc-dem".into()] },
            outputs: vec!["terrain.mbtiles".into()],
            run: Run::Rust(obc_dem::step::planner_terrain),
        };
        let routing = Step {
            name: "planner/routing".into(),
            inputs: std::iter::once(Input::Layer(osm.name.clone())).chain(tiles(bounds, &land, glo30)).collect(),
            options: json!({
                // The last part of the id: the old planner names its files after it.
                "region": region.id.rsplit('/').next(),
                "bounds": bounds,
                "profiles": config.routing.profiles,
                "countries": region.countries,
            }),
            code: Code { paths: vec!["data/sources.toml".into()], crates: vec!["route-build".into()] },
            outputs: vec!["routing".into(), "blocks".into(), "routes".into()],
            run: Run::Rust(route_build::step::step),
        };
        Ok(vec![osm, terrain, routing])
    }
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
    use std::collections::BTreeMap;

    use obc_data::product::Wanted;
    use obc_data::regions::parse_region;

    use super::*;
    use crate::maps::tests::{fetched, root, temp, Temp};
    use crate::maps::Maps;

    const AREA: &str = "europe/test";

    fn env(region: &str) -> Env {
        let pins = BTreeMap::from([(GLO30.to_string(), "1".to_string()), (TILE_LIST.to_string(), "1".to_string())]);
        Env { name: "test".into(), region: region.into(), layers: Vec::new(), pins }
    }

    fn regions() -> Regions {
        let region = |id: &str, text: &str| parse_region(id, &format!("name = \"{id}\"\n{text}")).unwrap();
        Regions::new(vec![
            region(AREA, "kind = \"geofabrik\"\ncountries = [\"DE\"]\n"),
            region("no-countries", "kind = \"geofabrik\"\n"),
            region("boxed", "kind = \"box\"\nbox = [7.79, 47.99, 7.82, 48.02]\ncountries = [\"DE\"]\n"),
        ])
        .unwrap()
    }

    /// A store with what the step list of `AREA` reads: its `.poly`, a box near Freiburg; the
    /// tile list, which names the two squares west of 8°; and the extract of each of `days`.
    fn store(temp: &Temp, days: &[&str]) -> Store {
        let store = Store::at(temp.0.join("store"));
        let area = [("area".to_string(), AREA.to_string())];
        let poly = "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";
        fetched(&store, "geofabrik-poly", "2026-10-01", &area, &[(format!("{AREA}.poly"), poly.into())]);
        let list = "Copernicus_DSM_COG_10_N47_00_E007_00_DEM\nCopernicus_DSM_COG_10_N48_00_E007_00_DEM\n";
        fetched(&store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list.into())]);
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
        let Err(Unplanned::NeedsFetch(wanted)) = Planner.steps(&env(AREA), &regions(), &store(&temp, &[])) else {
            panic!("the store has no extract");
        };
        let area = vec![("area".to_string(), AREA.to_string())];
        assert_eq!(wanted, [Wanted { source: EXTRACTS.into(), version: None, params: area.clone() }]);

        let steps = Planner.steps(&env(AREA), &regions(), &store(&temp, &["2026-10-01", "2026-10-02"])).unwrap();
        let [osm, terrain, routing] = &steps[..] else { panic!("three steps") };
        let Input::Snapshot { source, version, params, .. } = &osm.inputs[0] else { panic!("not a snapshot") };
        assert_eq!((source.as_str(), version.as_str(), params), (EXTRACTS, "2026-10-02", &area));
        // `terrain_coverage` of `tools/planner_bake.py` with a sun layer of 30 km.
        assert_eq!(terrain.options["bounds"], json!([7.382257389170511, 47.71727272727273, 8.4375, 48.45835188280866]));
        assert_eq!(
            tiles(&terrain.inputs[0]),
            ["N47_00_E007", "N48_00_E007"],
            "the tile list names no square east of 8°"
        );
        assert!(matches!(&routing.inputs[0], Input::Layer(name) if name == "planner/osm"));
        assert_eq!(tiles(&routing.inputs[1]), ["N47_00_E007", "N48_00_E007"]);
        assert_eq!(routing.options["bounds"], json!([7.79, 47.99, 7.82, 48.02]));
        assert_eq!((&routing.options["region"], &routing.options["countries"]), (&json!("test"), &json!(["DE"])));
        // About Baden-Württemberg, and the box of the old planner recipe.
        let old = [7.03125, 47.04018214480666, 10.922533154247459, 50.07272727272727];
        assert_eq!(terrain_bounds([7.5, 47.5, 10.5, 49.8], 30_000.0), old);
        let old = [5.2734375, 45.33670190996811, 10.922970099182649, 50.28933925329178];
        assert_eq!(terrain_bounds([5.95, 45.8, 10.5, 49.85], 30_000.0), old);

        for region in ["boxed", "no-countries"] {
            let result = Planner.steps(&env(region), &regions(), &store(&temp, &["2026-10-01"]));
            assert!(matches!(result, Err(Unplanned::Invalid(_))), "{region}");
        }
    }

    #[test]
    fn the_code_of_route_build_is_the_code_of_the_routing_layer_only() {
        let temp = temp("planner-code");
        let store = store(&temp, &["2026-10-01"]);
        let mut steps = Planner.steps(&env(AREA), &regions(), &store).unwrap();
        steps.extend(Maps.steps(&env(AREA), &regions(), &store).unwrap());
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
}
