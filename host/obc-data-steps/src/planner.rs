//! The planner. Its layers cover the region of the settings: `planner/osm` is the OSM of the
//! region, `planner/basemap` the offline Protomaps map, `planner/terrain` the terrain of the maps,
//! `planner/routing` the routing package with its grid, and `planner/search/dump` the search records of the OSM. `planner/overlays`,
//! `planner/assets`, `planner/model`, `planner/places`, the other `planner/search/*` layers and
//! the optional layers `planner/climate`, `planner/snow`, `planner/sun` and `planner/trailview-*` are
//! Python steps.
//! Each map producer is intermediate. Its `/grid` step partitions and packs the client tiles.
//! The release document, the public files and the grid indexes are the files that a client finds
//! by name; `planner/catalog` points at them. `data/planner.toml` holds the options that are the
//! same for each region.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

use obc_data::engine::{snapshot_files, Client, Input, Layer, LayerFile, Reader, Run, Shape, Step};
use obc_data::product::{version, Inputs, Pointer, Product, Steps, Unplanned, Wanted};
use obc_data::regions::{geofabrik, Bbox, Regions};
use obc_data::release::{Output, Release};
use obc_data::sources::{attribution, embedded};
use obc_data::store::Store;
use obc_dem::reference::{Window, TILE_LOG2};
use obc_dem::step::GLO30;
use obc_map_core::grid::CellId;
use planner_router_build::grid::{mercator, tile_bounds};
use serde::Deserialize;
use serde_json::json;

use crate::device::{invalid, text, Models, TILE_LIST};
use crate::readers::{self, DOWNLOADS_SERVICE, IOS, IOS_OFFLINE_ROUTER, ROUTER_SERVICE, SEARCH_SERVICE, WEB_PLANNER};
use crate::region_sources::OSM;
use crate::{functions, python};

/// The offline Protomaps map, with its fonts and sprites.
pub static PLANNER_BASEMAP: Layer = whole("planner/basemap", 1, &[&WEB_PLANNER, &IOS]);
/// The terrain of the maps and of TrailView: its format is that of the terrain tiles of
/// `specs/planner-release.md`.
pub static PLANNER_TERRAIN: Layer =
    whole("planner/terrain", 2, &[&WEB_PLANNER, &readers::PLANNER_SUN, &readers::PLANNER_TRAILVIEW_FEATURES]);
/// The routing package, its grid and the route network overlays.
pub static PLANNER_ROUTING: Layer =
    whole("planner/routing", planner_router::package::FORMAT, &[&ROUTER_SERVICE, &IOS_OFFLINE_ROUTER]);
/// The search databases and the query model. Its format is the search schema of
/// `specs/planner-release.md`.
pub static PLANNER_SEARCH: Layer = whole("planner/search", 6, &[&SEARCH_SERVICE]);
/// The rider places of the search.
pub static PLANNER_PLACES: Layer = whole("planner/places", 1, &[&WEB_PLANNER]);
pub static PLANNER_CLIMATE: Layer = whole("planner/climate", 1, &[&WEB_PLANNER]);
pub static PLANNER_SNOW: Layer = whole("planner/snow", 1, &[&WEB_PLANNER]);
/// Its format is `sun_format` of `specs/planner-sun-tiles.md`.
pub static PLANNER_SUN: Layer = whole("planner/sun", 3, &[&WEB_PLANNER]);
/// Its format is `features_format` of `specs/trailview-features.md`.
pub static PLANNER_TRAILVIEW_FEATURES: Layer =
    Layer { revision: 4, ..whole("planner/trailview-features", 1, &[&WEB_PLANNER]) };
/// The TrailView ruleset, `rules.json` of `specs/trailview-tiles.md`.
pub static PLANNER_TRAILVIEW_RULES: Layer =
    whole("planner/trailview-rules", 1, &[&WEB_PLANNER, &readers::PLANNER_TRAILVIEW_GROUND]);
/// The TrailView material, lithology and region tiles. Its format is `trailview_format` of
/// `specs/trailview-tiles.md`.
pub static PLANNER_TRAILVIEW_GROUND: Layer = whole("planner/trailview-ground", 1, &[&WEB_PLANNER]);
/// The TrailView colour tiles. Its format is `trailview_format` of `specs/trailview-tiles.md`.
pub static PLANNER_TRAILVIEW_COLOUR: Layer = whole("planner/trailview-colour", 1, &[&WEB_PLANNER]);
/// Its format is `canopy_format` of `specs/trailview-tiles.md`.
pub static PLANNER_TRAILVIEW_CANOPY: Layer = whole("planner/trailview-canopy", 1, &[&WEB_PLANNER]);
/// The offline catalog, the release document, the joined fonts of every planner layer and the
/// pointer. Its format is that of the offline catalog and of `search/<region>.grid.json`, which the
/// downloads and search services check: a change of either file format changes it, and
/// `INDEX_FORMAT` of `tools/planner_runtime.py` and `GRID` of `planner/search/installation.mjs`.
pub static PLANNER_INDEX: Layer = whole("planner/index", 3, &[&WEB_PLANNER, &IOS, &SEARCH_SERVICE, &DOWNLOADS_SERVICE]);

const WORLDCOVER_GRID: &str = "esa-worldcover-grid";

const fn whole(name: &'static str, format: u32, readers: &'static [&'static Reader]) -> Layer {
    Layer { name, shape: Shape::Whole, format, revision: 1, readers }
}
const BASEMAP_SOURCES: [&str; 7] = [
    "protomaps-basemaps",
    "natural-earth",
    "water-polygons",
    "land-polygons",
    "daylight-landcover",
    "qrank",
    "pgf-encoding",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    terrain: Terrain,
    routing: Routing,
    climate: Climate,
    snow: Snow,
    sun: Sun,
    trailview: TrailView,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TrailView {
    colour_year: u16,
    colour: String,
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

pub(crate) mod catalog;

pub struct Planner;

impl Product for Planner {
    fn name(&self) -> &'static str {
        "planner"
    }

    fn status_steps(&self, root: &Path, inputs: &Inputs, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        self.steps_with_tool(root, inputs, regions, store, Ok(()))
    }

    fn pointer(&self) -> Pointer {
        Pointer { key: "planner/catalog.json", step: catalog::STEP, path: "catalog.json" }
    }

    fn named(&self, outputs: &[Output]) -> Result<Vec<(String, LayerFile)>, String> {
        catalog::named(outputs.iter().map(|output| (output.step.as_str(), output.files.as_slice())))
    }

    fn verify(&self, root: &Path, previous: Option<&Release>, release: &Release, store: &Store) -> Result<(), String> {
        catalog::verify(root, previous, release, store)
    }

    fn optional(&self) -> &'static [&'static str] {
        &[
            "map",
            "routing",
            "search",
            "climate",
            "snow",
            "sun",
            "trailview-features",
            "trailview-rules",
            "trailview-ground",
            "trailview-colour",
            "trailview-canopy",
        ]
    }

    /// The planner bakes while one of its parts is on.
    fn on(&self, settings: &obc_data::settings::Settings) -> bool {
        settings.layers.iter().any(|layer| self.optional().contains(&layer.as_str()))
    }

    fn steps(&self, root: &Path, inputs: &Inputs, regions: &Regions, store: &Store) -> Result<Steps, Unplanned> {
        let osmium = obc_osm::ShardRunner::check(&obc_osm::OsmiumRunner::default());
        self.steps_with_tool(root, inputs, regions, store, osmium)
    }
}

impl Planner {
    /// Its steps. `osmium` says whether Osmium runs here: more than one source area needs it.
    pub fn steps_with_tool(
        &self,
        root: &Path,
        inputs: &Inputs,
        regions: &Regions,
        store: &Store,
        osmium: Result<(), String>,
    ) -> Result<Steps, Unplanned> {
        let config: Config = toml::from_str(include_str!("../../../data/planner.toml"))
            .map_err(|e| Unplanned::Failed(format!("data/planner.toml: {e}")))?;
        let origins = catalog::publication(root).map_err(Unplanned::Failed)?;
        let id = &inputs.settings.region;
        let region = regions.get(id).ok_or_else(|| Unplanned::Failed(format!("no region `{id}`")))?;
        // A region without a time zone, such as a box, gives the times of the search and the sun in UTC.
        let time_zone = region.time_zone.as_deref().unwrap_or("UTC");
        let on = |layer: &str| inputs.settings.layers.iter().any(|name| name == layer);
        let mut wanted = Vec::new();
        let glo30 = version(inputs, store, GLO30, &[]).map_err(Unplanned::Failed)?.map_err(|fetch| wanted.push(fetch));
        let selection = crate::region_sources::resolve(inputs, regions, store, &mut wanted)?;
        let tile_list = text(inputs, store, TILE_LIST, &[], &mut wanted)?;
        let assets = vec![
            snapshot(inputs, store, "protomaps-assets", Vec::new(), &mut wanted)?,
            snapshot(inputs, store, "tangrams-icons", Vec::new(), &mut wanted)?,
        ];
        let model = snapshot(inputs, store, "query-model", Vec::new(), &mut wanted)?;
        let country_data = snapshot(inputs, store, "nominatim-country-data", Vec::new(), &mut wanted)?;
        let (Some(selection), Some(tile_list), Ok(glo30)) = (selection, tile_list, glo30) else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
        let countries = match &region.countries[..] {
            [] => {
                let Some(body) = text(inputs, store, crate::device::catalog::INDEX, &[], &mut wanted)? else {
                    return Err(Unplanned::NeedsFetch(wanted));
                };
                let index = geofabrik::parse(body.as_bytes()).map_err(Unplanned::Failed)?;
                let mut countries: Vec<String> = selection
                    .sources
                    .iter()
                    .filter_map(|source| index.get(&source.id))
                    .flat_map(|area| area.countries.clone())
                    .collect();
                countries.sort();
                countries.dedup();
                countries
            }
            countries => countries.to_vec(),
        };
        if countries.is_empty() {
            return Err(invalid(format!(
                "the Geofabrik areas of region `{id}` name no country, which the route catalog needs"
            )));
        }
        let land: HashSet<&str> = tile_list.lines().map(str::trim).collect();
        let requested = obc_bake::coverage::Coverage::union(&selection.outlines.iter().collect::<Vec<_>>())
            .ok_or_else(|| Unplanned::Failed("cannot union planner region coverage".into()))?;
        let (west, south, east, north) = requested.bbox();
        let bounds = [west, south, east, north].map(|udeg| udeg as f64 / 1e6);
        let bbox = ("bbox".to_string(), bounds.map(|degrees| degrees.to_string()).join(","));
        let coverage = terrain_bounds(bounds, config.terrain.margin_m);
        let source_steps = crate::region_sources::inputs("planner", &selection);
        let osm = if selection.sources.len() == 1 {
            Step {
                layer: &OSM,
                name: "planner/osm".into(),
                inputs: vec![Input::Step { name: source_steps[0].name.clone(), files: vec!["source.osm.pbf".into()] }],
                options: json!({"path": "osm.pbf"}),
                outputs: vec!["osm.pbf".into()],
                run: Run::Rust(&obc_data::engine::PASS),
                client: Client::None,
            }
        } else {
            osmium.clone().map_err(Unplanned::Invalid)?;
            crate::region_sources::combined("planner/osm", &source_steps)
        };
        let mut reads = vec![Input::step(osm.name.clone())];
        for source in BASEMAP_SOURCES {
            let mut input = snapshot(inputs, store, source, Vec::new(), &mut wanted)?;
            if let Input::Snapshot { files, .. } = &mut input {
                if source == "protomaps-basemaps" {
                    *files = vec![obc_data::fetch::basemap_jar()];
                }
            }
            reads.push(input);
        }
        let credit = ["osm-planet", "natural-earth", "daylight-landcover"].map(attribution).join("; ");
        let basemap = python(
            &PLANNER_BASEMAP,
            "planner/basemap",
            reads,
            json!({"bounds": bounds, "attribution": credit}),
            ("tools.planner_basemap", None),
            &["basemap.pmtiles"],
        );
        let reference = reference(inputs, store, coverage, &mut wanted)?;
        let terrain = Step {
            layer: &PLANNER_TERRAIN,
            name: "planner/terrain".into(),
            inputs: tiles(coverage, &land, &glo30)
                .into_iter()
                .chain(reference.iter().map(|step| Input::step(&step.name)))
                .collect(),
            options: json!({"bounds": coverage, "attribution": attribution(GLO30)}),
            outputs: vec!["terrain.mbtiles".into()],
            run: Run::Rust(&functions::PLANNER_TERRAIN),
            client: Client::All,
        };
        // The last part of the id: the old planner names its files after it.
        let name = region.id.rsplit('/').next();
        let routing = Step {
            layer: &PLANNER_ROUTING,
            name: "planner/routing".into(),
            inputs: std::iter::once(Input::step(osm.name.clone())).chain(tiles(bounds, &land, &glo30)).collect(),
            options: json!({
                "region": name,
                "bounds": bounds,
                "profiles": config.routing.profiles,
                "countries": countries,
                "attribution": attribution("osm-planet"),
                "terrain_attribution": attribution(GLO30),
            }),
            outputs: vec!["routing".into(), "blocks".into(), "routes".into()],
            run: Run::Rust(&functions::ROUTING),
            client: Client::All,
        };
        let overlays = python(
            &PLANNER_ROUTING,
            "planner/overlays",
            vec![Input::step(routing.name.clone())],
            json!({"attribution": attribution("osm-planet")}),
            ("tools.planner_overlays", Some("planner-maps")),
            &["overlays.pmtiles"],
        );
        let assets =
            python(&PLANNER_BASEMAP, "planner/assets", assets, json!({}), ("tools.planner_assets", None), &["assets"]);
        let model = python(
            &PLANNER_SEARCH,
            "planner/model",
            vec![model],
            json!({}),
            ("planner/search/setup.py", None),
            &["model"],
        );
        let policy = Step {
            client: Client::None,
            ..python(
                &PLANNER_SEARCH,
                "planner/search/policy",
                vec![country_data],
                json!({}),
                ("host/obc-search-bake/policy.py", Some("planner-search")),
                &["policy.json", "country_osm_grid.sql.gz"],
            )
        };
        let dump = Step {
            layer: &PLANNER_SEARCH,
            name: "planner/search/dump".into(),
            inputs: vec![Input::step(osm.name.clone()), Input::step(policy.name.clone())],
            options: json!({"country": countries[0].to_lowercase()}),
            outputs: vec!["search.jsonl.zst".into()],
            run: Run::Rust(&functions::SEARCH),
            client: Client::None,
        };
        let records = Step {
            client: Client::None,
            ..python(
                &PLANNER_SEARCH,
                "planner/search/records",
                vec![Input::step(dump.name.clone())],
                json!({}),
                ("planner/search/split.py", Some("planner-search")),
                &["pois.jsonl.zst", "addresses.jsonl.zst"],
            )
        };
        let content = crate::device::content_declarations(inputs, store, &selection)?;
        let content_inputs = || {
            content
                .steps
                .iter()
                .map(|step| step.name.clone())
                .chain(content.blocked.iter().map(|blocked| blocked.step.clone()))
                .map(|name| Input::Step {
                    files: vec![if name.contains("landmark-content") {
                        "landmarks/content.json".into()
                    } else {
                        "peaks/peaks.json".into()
                    }],
                    name,
                })
                .collect::<Vec<_>>()
        };
        // One search database per component: the POIs and the addresses.
        let search = |component: &str| {
            python(
                &PLANNER_SEARCH,
                &format!("planner/search/{component}"),
                std::iter::once(layer_files(&records.name, &[&format!("{component}.jsonl.zst")]))
                    .chain((component == "pois").then(content_inputs).into_iter().flatten())
                    .collect(),
                json!({
                    "component": component,
                    "region": name,
                    "bounds": bounds,
                    "countries": countries,
                    "time_zone": time_zone,
                    "attribution": attribution("osm-planet"),
                }),
                ("planner/search/build.py", Some("planner-search")),
                &[component],
            )
        };
        let (pois, addresses) = (search("pois"), search("addresses"));
        let places = python(
            &PLANNER_PLACES,
            "planner/places",
            vec![Input::step(pois.name.clone())],
            json!({}),
            ("tools.planner_places", Some("planner-maps")),
            &["places.pmtiles"],
        );
        let mut steps = source_steps;
        steps.extend(content.steps);
        steps.extend(reference);
        steps.extend([
            osm, terrain, routing, overlays, assets, model, policy, dump, records, pois, addresses, places, basemap,
        ]);
        if on("climate") {
            let first_year = config.climate.first_year;
            let params = vec![bbox, ("first-year".to_string(), first_year.to_string())];
            steps.push(python(
                &PLANNER_CLIMATE,
                "planner/climate",
                vec![snapshot(inputs, store, "era5-land", params, &mut wanted)?],
                // `{year}` is the year after the ten years.
                json!({"bounds": bounds, "first_year": first_year, "attribution": attribution("era5-land")}),
                ("tools.planner_climate", Some("planner-climate")),
                &["climate.pmtiles"],
            ));
        }
        if on("snow") {
            steps.push(snow(inputs, store, bounds, config.snow.seasons, &mut wanted)?);
        }
        if on("sun") {
            steps.push(python(
                &PLANNER_SUN,
                "planner/sun",
                vec![Input::step("planner/terrain/grid")],
                json!({
                    "bounds": bounds,
                    "time_zone": time_zone,
                    "distance_m": config.terrain.margin_m as u32,
                    "horizon_samples": config.sun.horizon_samples,
                    "horizon_directions": config.sun.horizon_directions,
                }),
                ("tools.planner_sun", Some("planner-sun")),
                &["sun"],
            ));
        }
        if on("trailview-features") {
            osmium.map_err(Unplanned::Invalid)?;
            let kinds: serde_json::Value = serde_json::from_str(include_str!("../../../data/trailview/kinds.json"))
                .map_err(|e| Unplanned::Failed(format!("data/trailview/kinds.json: {e}")))?;
            let roads: serde_json::Value = serde_json::from_str(include_str!("../../../data/trailview/roads.json"))
                .map_err(|e| Unplanned::Failed(format!("data/trailview/roads.json: {e}")))?;
            // The heights of roads, rivers and lakes come from the master terrain, by the class rules of the roads.
            steps.push(python(
                &PLANNER_TRAILVIEW_FEATURES,
                "planner/trailview-features",
                vec![Input::step("planner/osm"), Input::step("planner/terrain/grid")],
                json!({"bounds": bounds, "kinds": kinds, "roads": roads, "attribution": attribution("osm-planet")}),
                ("tools.planner_trailview_features", Some("trailview")),
                &["trailview-features.pmtiles"],
            ));
        }
        steps.extend(trailview(root, inputs, store, &on, bounds, &config.trailview, &mut wanted)?);
        if on("trailview-canopy") {
            steps.extend(trailview_canopy(inputs, store, (bounds, &countries), &mut wanted)?);
        }
        let maps = [
            "basemap",
            "places",
            "terrain",
            "overlays",
            "climate",
            "snow",
            "sun",
            "trailview-features",
            "trailview-material",
            "trailview-lithology",
            "trailview-region",
            "trailview-canopy",
        ];
        let grids = steps
            .iter_mut()
            .filter_map(|step| {
                let kind = step.name.strip_prefix("planner/")?;
                if !maps.contains(&kind) {
                    return None;
                }
                step.client = Client::None;
                Some(map_grid(step, kind, bounds))
            })
            .collect::<Vec<_>>();
        steps.extend(grids);
        for kind in ["routing", "assets", "model", "search/pois", "search/addresses"] {
            let source = steps.iter_mut().find(|step| step.name == format!("planner/{kind}")).unwrap();
            source.client = Client::None;
            let (source, layer) = (source.name.clone(), source.layer);
            let search = kind.strip_prefix("search/");
            let (module, group) = match search {
                Some(_) => ("tools.planner_grid_search", Some("planner-search")),
                None => ("tools.planner_grid_pack", None),
            };
            let options = match search {
                Some(component) => json!({"kind": component, "component": component, "bounds": bounds}),
                None => json!({"kind": kind}),
            };
            steps.push(Step {
                client: Client::Paths(vec!["objects".into(), "index.json".into()]),
                ..python(
                    layer,
                    &format!("{source}/grid"),
                    vec![Input::step(source)],
                    options,
                    (module, group),
                    &["objects", "index.json"],
                )
            });
        }
        steps.push(Step {
            client: Client::Paths(vec!["objects".into(), "index.json".into()]),
            ..python(
                &PLANNER_INDEX,
                "planner/fonts/grid",
                ["basemap", "places", "overlays", "assets"]
                    .map(|kind| Input::step(format!("planner/{kind}/grid")))
                    .into(),
                json!({"kind": "fonts"}),
                ("tools.planner_grid_fonts", Some("planner-maps")),
                &["objects", "index.json"],
            )
        });
        let overlays = steps.iter_mut().find(|step| step.name == "planner/overlays/grid").unwrap();
        overlays.inputs.push(layer_files("planner/routing/grid", &["index.json"]));
        let grids: Vec<String> =
            steps.iter().filter(|step| step.name.ends_with("/grid")).map(|step| step.name.clone()).collect();
        let indexes = || grids.iter().map(|grid| layer_files(grid, &["index.json"]));
        steps.push(Step {
            client: Client::Paths(["objects", "release.json", "public"].map(String::from).into()),
            ..python(
                &PLANNER_INDEX,
                "planner/index",
                indexes().collect(),
                json!({
                    "region": name, "name": region.name, "bounds": bounds,
                    "attribution": attribution("osm-planet"),
                    "landcover_attribution": attribution("daylight-landcover"),
                }),
                ("tools.planner_grid_index", None),
                &["objects", "release.json", "public", "index.json"],
            )
        });
        steps.push(Step {
            layer: &PLANNER_INDEX,
            name: catalog::STEP.into(),
            inputs: std::iter::once(Input::step("planner/index")).chain(indexes()).collect(),
            options: json!({"origins": origins}),
            outputs: vec!["catalog.json".into()],
            run: Run::Rust(&functions::PLANNER_CATALOG),
            client: Client::All,
        });
        prune(&mut steps, &on);
        match wanted.is_empty() {
            true => {
                let mut listed: Steps = steps.into();
                listed.blocked.extend(content.blocked);
                listed.block_dependents();
                Ok(listed)
            }
            false => Err(Unplanned::NeedsFetch(wanted)),
        }
    }
}

/// The steps that make each core part of the planner. The other parts are optional layers whose
/// steps are listed only while they are on.
const PARTS: [(&str, &[&str]); 3] = [
    ("map", &["planner/basemap/grid", "planner/places/grid", "planner/assets/grid", "planner/terrain/grid"]),
    ("routing", &["planner/routing/grid", "planner/overlays/grid"]),
    ("search", &["planner/model/grid", "planner/search/pois/grid", "planner/search/addresses/grid"]),
];

/// The steps that gather what the other steps make. They read only the steps that stay.
const GATHERERS: [&str; 3] = ["planner/fonts/grid", "planner/index", catalog::STEP];

/// Remove the steps of each core part that is off, and then each step that no step reads any more.
/// A step that a part that is on reads stays, such as the terrain that the sun reads or the POIs
/// that the places of the map read.
fn prune(steps: &mut Vec<Step>, on: &dyn Fn(&str) -> bool) {
    let reads = |steps: &[Step]| -> HashSet<String> {
        let names = steps.iter().flat_map(|step| &step.inputs);
        names.filter_map(|input| if let Input::Step { name, .. } = input { Some(name.clone()) } else { None }).collect()
    };
    let gatherer = |step: &Step| GATHERERS.contains(&step.name.as_str());
    let mut gone: HashSet<String> = PARTS
        .iter()
        .filter(|(part, _)| !on(part))
        .flat_map(|(_, roots)| roots.iter().map(|root| root.to_string()))
        .collect();
    // Only a step that lost its readers goes, never one that no step read before.
    let mut collectable = reads(steps);
    collectable.extend(gone.iter().cloned());
    while !gone.is_empty() {
        for step in steps.iter_mut().filter(|step| gatherer(step)) {
            step.inputs.retain(|input| !matches!(input, Input::Step { name, .. } if gone.contains(name)));
        }
        let read = reads(steps);
        steps.retain(|step| !gone.contains(&step.name) || read.contains(&step.name));
        let read = reads(steps);
        let unread = |step: &Step| collectable.contains(&step.name) && !read.contains(&step.name);
        let empty = |step: &Step| gatherer(step) && step.inputs.is_empty();
        gone = steps
            .iter()
            .filter(|step| step.name != catalog::STEP && (unread(step) || empty(step)))
            .map(|step| step.name.clone())
            .collect();
    }
}

fn layer_files(name: &str, files: &[&str]) -> Input {
    Input::Step { name: name.into(), files: files.iter().map(|file| (*file).into()).collect() }
}

/// A map's transport objects and its small index ship; the index is input to the offline catalog too.
fn map_grid(source: &Step, kind: &str, bounds: [f64; 4]) -> Step {
    let bounds = source.options.get("bounds").cloned().unwrap_or_else(|| json!(bounds));
    Step {
        client: Client::Paths(vec!["objects".into(), "index.json".into()]),
        ..python(
            source.layer,
            &format!("{}/grid", source.name),
            vec![Input::step(source.name.clone())],
            json!({"kind": kind, "bounds": bounds}),
            ("tools.planner_grid_maps", Some("planner-maps")),
            &["objects", "index.json"],
        )
    }
}

/// The input of the fetch of `source` with `params`, at its version. While the store lacks that
/// fetch, `wanted` names it, and the input has no version: the step list is not used then.
fn snapshot(
    inputs: &Inputs,
    store: &Store,
    source: &str,
    params: Vec<(String, String)>,
    wanted: &mut Vec<Wanted>,
) -> Result<Input, Unplanned> {
    let version = version(inputs, store, source, &params).map_err(Unplanned::Failed)?.unwrap_or_else(|fetch| {
        wanted.push(fetch);
        String::new()
    });
    Ok(Input::Snapshot { source: source.into(), version, params, files: Vec::new() })
}

/// The mean tiles of `terrain` that meet `bounds`, as one archive: the step `planner/reference`.
/// `None` when the version has no tile there.
fn reference(
    inputs: &Inputs,
    store: &Store,
    [west, south, east, north]: [f64; 4],
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Step>, Unplanned> {
    let udeg = |degrees: f64| (degrees * 1e6) as i64;
    let window = Window { lat_lo: udeg(south), lat_hi: udeg(north) + 1, lon_lo: udeg(west), lon_hi: udeg(east) + 1 };
    let tile = |(ti, tj): (u32, u32)| CellId::new(TILE_LOG2.into(), ti.into(), tj.into()).expect("a tile is a cell");
    let tiles: BTreeSet<CellId> = window.tiles().map(tile).collect();
    // Without a layer, the read writes no note and reads no polygon.
    // Without a layer, the read writes no note and reads no polygon.
    let models = Models { tiles: &tiles, pool: "mean/", layer: None };
    let Some(read) = models.read(inputs, store, &mut BTreeMap::new(), wanted, &mut Vec::new())? else {
        return Ok(None);
    };
    let tiles: Vec<String> = tiles.iter().map(|tile| format!("{:04}/{:04}", tile.i, tile.j)).collect();
    Ok(Some(Step {
        client: Client::None,
        ..python(
            &PLANNER_TERRAIN,
            "planner/reference",
            vec![read],
            json!({"source": crate::terrain_reference::ID, "tiles": tiles, "pool": "mean"}),
            ("host/obc-dem/reference/merge.py", Some("terrain-reference")),
            &["reference"],
        )
    }))
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

/// The snow layer of `bounds`: HR-WSI where it has data, and MODIS elsewhere. Outside the extent
/// of HR-WSI, or where its fetch has no files, it reads MODIS only.
fn snow(
    inputs: &Inputs,
    store: &Store,
    bounds: [f64; 4],
    [first, last]: [u16; 2],
    wanted: &mut Vec<Wanted>,
) -> Result<Step, Unplanned> {
    let bbox = ("bbox".to_string(), bounds.map(|degrees| degrees.to_string()).join(","));
    let params = vec![bbox, ("seasons".to_string(), format!("{first}-{last}"))];
    let tiles = canopy_tiles(bounds).into_iter().map(|tile| ("tile".to_string(), tile)).collect();
    let mut reads = vec![
        snapshot(inputs, store, "modis-snow", params.clone(), wanted)?,
        snapshot(inputs, store, "hansen-gfc", tiles, wanted)?,
    ];
    let mut credit = format!("{}; tree canopy: {}", attribution("modis-snow"), attribution("hansen-gfc"));
    let mut year = None;
    if embedded("hr-wsi").meets(bounds) {
        let hr_wsi = snapshot(inputs, store, "hr-wsi", params, wanted)?;
        let Input::Snapshot { version, params, .. } = &hr_wsi else { unreachable!("a fetch is a snapshot input") };
        let files = snapshot_files(store, "hr-wsi", version, params, &[]).map_err(Unplanned::Failed)?;
        if !files.is_some_and(|files| files.is_empty()) {
            // The credit of HR-WSI names a year: the year of the capture.
            year = version.get(..4).and_then(|year| year.parse::<u16>().ok());
            credit = format!("{}; {credit}", attribution("hr-wsi"));
            reads.insert(0, hr_wsi);
        }
    }
    Ok(python(
        &PLANNER_SNOW,
        "planner/snow",
        reads,
        json!({"bounds": bounds, "seasons": [first, last], "year": year, "attribution": credit}),
        ("tools.planner_snow", Some("planner-snow")),
        &["snow.pmtiles"],
    ))
}

/// The canopy layer of `bounds`: the zoom 12 tiles of `forest` that the canopy models of
/// `countries` meet, with the credits of those models and of the leaf type. None while the store
/// lacks a manifest; then `wanted` has its fetch. A version whose scopes lack one of those tiles
/// refuses.
fn trailview_canopy(
    inputs: &Inputs,
    store: &Store,
    (bounds, countries): ([f64; 4], &[String]),
    wanted: &mut Vec<Wanted>,
) -> Result<Option<Step>, Unplanned> {
    use crate::forest_reference::{model_tiles, models, ID, LEAF_TYPE};
    let sources = obc_data::sources::all();
    let path = |(x, y): (u32, u32)| format!("12/{x}/{y}.tif");
    let tiles = |bbox: Option<&Bbox>, countries: &[String]| -> Result<BTreeMap<String, String>, Unplanned> {
        let mut tiles = BTreeMap::new();
        for (_, model) in models(sources, bbox, countries) {
            for tile in model_tiles(model, bbox).map_err(Unplanned::Failed)? {
                tiles.entry(path(tile)).or_insert_with(|| model.id.clone());
            }
        }
        Ok(tiles)
    };
    let [west, south, east, north] = bounds;
    let needed = tiles(Some(&Bbox { west, south, east, north }), countries)?;
    if needed.is_empty() {
        return Err(invalid("the layer trailview-canopy needs a canopy model: none covers the region".into()));
    }
    let pinned = match version(inputs, store, ID, &[]).map_err(Unplanned::Failed)? {
        Ok(pinned) => pinned,
        Err(fetch) => {
            wanted.push(fetch);
            return Ok(None);
        }
    };
    let Some(manifest) = crate::device::manifest(store, ID, &pinned, wanted)? else { return Ok(None) };
    // A version holds each tile with a pixel in its scope and in the scopes that it extends.
    let mut covered = BTreeSet::new();
    for scope in manifest.extends.iter().chain([&manifest.scope]) {
        let (bbox, countries) = crate::terrain_reference::area(scope.as_ref()).map_err(Unplanned::Failed)?;
        covered.extend(tiles(bbox.as_ref(), &countries)?.into_keys());
    }
    if needed.keys().any(|tile| !covered.contains(tile)) {
        let region = &inputs.settings.region;
        return Err(invalid(format!(
            "{ID}@{} does not cover all of the region `{region}`. Fix: obc data source prepare {ID} --region {region}",
            manifest.version
        )));
    }
    let files: Vec<String> =
        manifest.files.iter().map(|file| file.path.clone()).filter(|path| needed.contains_key(path)).collect();
    if files.is_empty() {
        return Err(invalid("the region has no canopy: switch the layer trailview-canopy off".into()));
    }
    let mut credits: Vec<&str> = Vec::new();
    for model in files.iter().map(|path| needed[path].as_str()).chain([LEAF_TYPE]) {
        let credit = attribution(model);
        if !credits.contains(&credit) {
            credits.push(credit);
        }
    }
    Ok(Some(python(
        &PLANNER_TRAILVIEW_CANOPY,
        "planner/trailview-canopy",
        vec![Input::Snapshot { source: ID.into(), version: pinned, params: Vec::new(), files }],
        json!({"bounds": bounds, "attribution": credits.join("; ")}),
        ("tools.planner_trailview_canopy", Some("planner-snow")),
        &["trailview-canopy.pmtiles"],
    )))
}

/// The TrailView layers of `bounds`, as `specs/trailview-tiles.md` defines them. The ruleset step is the
/// one reader of `data/trailview/`: its option `ruleset` is the digest of the files, and the ground
/// steps read its outputs, so the region ids of the tiles are those of `rules.json`.
fn trailview(
    root: &Path,
    inputs: &Inputs,
    store: &Store,
    on: &dyn Fn(&str) -> bool,
    bounds: [f64; 4],
    config: &TrailView,
    wanted: &mut Vec<Wanted>,
) -> Result<Vec<Step>, Unplanned> {
    let mut steps = Vec::new();
    let tool = |module| (module, Some("trailview"));
    if on("trailview-ground") && !on("trailview-rules") {
        return Err(invalid("the layer trailview-ground reads trailview-rules: name both".into()));
    }
    if on("trailview-rules") {
        let ruleset = json!({"ruleset": ruleset(root).map_err(Unplanned::Failed)?});
        let outputs = ["rules.json", "regions.json", "materials.json"];
        let rules = python(
            &PLANNER_TRAILVIEW_RULES,
            "planner/trailview-rules",
            Vec::new(),
            ruleset,
            tool("tools.trailview.rules"),
            &outputs,
        );
        steps.push(Step { client: Client::None, ..rules });
        steps.push(Step {
            client: Client::Paths(vec!["objects".into(), "index.json".into()]),
            ..python(
                &PLANNER_TRAILVIEW_RULES,
                "planner/trailview-rules/grid",
                vec![layer_files("planner/trailview-rules", &["rules.json"])],
                json!({"kind": "trailview-rules"}),
                ("tools.planner_grid_pack", None),
                &["objects", "index.json"],
            )
        });
    }
    let bbox = ("bbox".to_string(), bounds.map(|degrees| degrees.to_string()).join(","));
    let year = config.colour_year.to_string();
    let mosaics = vec![bbox.clone(), ("year".to_string(), year.clone())];
    let credit = |sources: &[&str]| {
        sources.iter().map(|id| attribution(id).replace("{year}", &year)).collect::<Vec<_>>().join("; ")
    };
    let grid = match on("trailview-ground") {
        true => crate::device::text(inputs, store, WORLDCOVER_GRID, &[], wanted)?,
        false => None,
    };
    if let Some(grid) = grid {
        let rules = |file: &str| layer_files("planner/trailview-rules", &[file]);
        let tiles = worldcover_tiles(bounds, &grid)?.into_iter().map(|tile| ("tile".to_string(), tile)).collect();
        let ground = |layer: &str, reads: Vec<Input>, text: String| {
            let options = json!({"layer": layer, "bounds": bounds, "attribution": text});
            let output = format!("{layer}.pmtiles");
            python(
                &PLANNER_TRAILVIEW_GROUND,
                &format!("planner/trailview-{layer}"),
                reads,
                options,
                tool("tools.trailview.ground"),
                &[&output],
            )
        };
        let material = vec![
            rules("materials.json"),
            layer_files("planner/osm", &["osm.pbf"]),
            snapshot(inputs, store, "esa-worldcover", tiles, wanted)?,
            snapshot(inputs, store, "corine-land-cover", vec![bbox.clone()], wanted)?,
            snapshot(inputs, store, "sentinel-2-mosaics", mosaics.clone(), wanted)?,
        ];
        let sources = ["esa-worldcover", "sentinel-2-mosaics", "corine-land-cover", "osm-planet"];
        steps.push(ground("material", material, credit(&sources)));
        let maps = ["egdi-geology", "igme5000"];
        let lithology = std::iter::once(Ok(rules("materials.json")))
            .chain(maps.map(|source| snapshot(inputs, store, source, Vec::new(), wanted)))
            .collect::<Result<_, _>>()?;
        steps.push(ground("lithology", lithology, credit(&maps)));
        steps.push(ground("region", vec![rules("regions.json")], credit(&["resolve-ecoregions", "natural-earth"])));
    }
    if on("trailview-colour") {
        let archives = ["q1", "q2", "q3", "q4", "ndvimax"];
        let outputs = archives.map(|archive| format!("{archive}.pmtiles"));
        let colour = python(
            &PLANNER_TRAILVIEW_COLOUR,
            "planner/trailview-colour",
            vec![snapshot(inputs, store, "sentinel-2-mosaics", mosaics, wanted)?],
            json!({"bounds": bounds, "year": config.colour_year, "variant": config.colour,
                   "attribution": credit(&["sentinel-2-mosaics"])}),
            tool("tools.trailview.colour"),
            &outputs.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        steps.push(Step { client: Client::None, ..colour });
        for (archive, output) in archives.iter().zip(&outputs) {
            steps.push(Step {
                client: Client::Paths(vec!["objects".into(), "index.json".into()]),
                ..python(
                    &PLANNER_TRAILVIEW_COLOUR,
                    &format!("planner/trailview-colour/{archive}/grid"),
                    vec![layer_files("planner/trailview-colour", &[output])],
                    json!({"kind": format!("trailview-colour-{archive}"), "bounds": bounds, "step": "planner/trailview-colour"}),
                    ("tools.planner_grid_maps", Some("planner-maps")),
                    &["objects", "index.json"],
                )
            });
        }
    }
    Ok(steps)
}

/// The digest of the JSON files of `data/trailview/`, by their paths in it, as `digest` of
/// `tools/trailview/rules.py` gives it.
fn ruleset(root: &Path) -> Result<String, String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                walk(&path, base, out)?;
            } else if path.extension().is_some_and(|extension| extension == "json") {
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let name = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                out.push((name, obc_data::store::sha256_hex(&bytes)));
            }
        }
        Ok(())
    }
    let base = root.join("data/trailview");
    let mut files = Vec::new();
    walk(&base, &base, &mut files)?;
    Ok(obc_data::engine::digest(files.iter().map(|(name, sha256)| (name.as_str(), sha256.as_str()))))
}

/// The 3° WorldCover tiles that `bounds` overlaps and that its grid lists, named by their south-west
/// corner. WorldCover has no tile over open sea.
fn worldcover_tiles(bounds: [f64; 4], grid: &str) -> Result<Vec<String>, Unplanned> {
    let grid: serde_json::Value =
        serde_json::from_str(grid).map_err(|e| Unplanned::Failed(format!("{WORLDCOVER_GRID}: {e}")))?;
    let land: HashSet<&str> = grid["features"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|feature| feature["properties"]["ll_tile"].as_str())
        .collect();
    Ok(worldcover_squares(bounds).into_iter().filter(|tile| land.contains(tile.as_str())).collect())
}

fn worldcover_squares([west, south, east, north]: [f64; 4]) -> Vec<String> {
    let corner = |degrees: f64| (degrees / 3.0).floor() as i32 * 3;
    let name = |lat: i32, lon: i32| {
        let (ns, ew) = (if lat >= 0 { 'N' } else { 'S' }, if lon >= 0 { 'E' } else { 'W' });
        format!("{ns}{:02}{ew}{:03}", lat.abs(), lon.abs())
    };
    let lats: Vec<i32> = (corner(south)..=corner(north - 1e-9)).step_by(3).collect();
    let lons: Vec<i32> = (corner(west)..=corner(east - 1e-9)).step_by(3).collect();
    lats.iter().flat_map(|&lat| lons.iter().map(move |&lon| name(lat, lon))).collect()
}

/// The 10° Hansen GFC tiles that `bounds` overlaps with one MODIS pixel, 1/240° of latitude, of
/// margin: the snow layer samples the MODIS pixels around its edge. Named by their north-west corner.
fn canopy_tiles([west, south, east, north]: [f64; 4]) -> Vec<String> {
    let pad = 1.0 / 240.0;
    let widen = pad / south.abs().max(north.abs()).to_radians().cos();
    let [west, south, east, north] = [west - widen, south - pad, east + widen, north + pad];
    let lats = (south / 10.0).floor() as i32 + 1..=(north / 10.0).ceil() as i32;
    let lons = (west / 10.0).floor() as i32..(east / 10.0).ceil() as i32;
    let name = |lat: i32, lon: i32| {
        let (ns, ew) = (if lat >= 0 { 'N' } else { 'S' }, if lon >= 0 { 'E' } else { 'W' });
        format!("{:02}{ns}_{:03}{ew}", (lat * 10).abs(), (lon * 10).abs())
    };
    lats.flat_map(|lat| lons.clone().map(move |lon| name(lat, lon))).collect()
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

    use obc_data::regions::parse_region;

    use super::*;
    use crate::device::tests::{fetched, root, temp, Temp};
    use crate::device::{Device, EXTRACTS};
    use obc_data::settings::Settings;

    const AREA: &str = "europe/test";
    const OPTIONAL_SOURCES: [&str; 8] = [
        "era5-land",
        "hr-wsi",
        "modis-snow",
        "esa-worldcover",
        "corine-land-cover",
        "sentinel-2-mosaics",
        "egdi-geology",
        "igme5000",
    ];
    const OPTIONAL: [&str; 7] =
        ["climate", "snow", "sun", "trailview-features", "trailview-rules", "trailview-ground", "trailview-colour"];

    /// The inputs of `region` with `layers` on, whose serving release reads each source without
    /// params of the planner at `1`, and the canopy at `v1.11`.
    fn inputs(region: &str, layers: &[&str]) -> Inputs {
        let read = [GLO30, TILE_LIST, "protomaps-assets", "tangrams-icons", "query-model", "nominatim-country-data"];
        let read = read.into_iter().chain(BASEMAP_SOURCES).map(|source| (source, "1")).chain([("hansen-gfc", "v1.11")]);
        let serving = read.map(|(source, version)| ((source.to_string(), Vec::new()), version.to_string())).collect();
        // The core parts are on unless a test names its parts.
        let layers = ["map", "routing", "search"].iter().chain(layers).map(|layer| layer.to_string()).collect();
        Inputs {
            settings: Settings { region: region.into(), layers, ..Settings::default() },
            serving,
            ..Inputs::default()
        }
    }

    /// `inputs` with the version `2026-10-01` for each fetch of the optional layers that the steps
    /// ask for.
    fn resolved(mut inputs: Inputs, regions: &Regions, store: &Store) -> Inputs {
        for _ in 0..3 {
            let Err(Unplanned::NeedsFetch(wanted)) = Planner.steps_with_tool(&root(), &inputs, regions, store, Ok(()))
            else {
                return inputs;
            };
            for wanted in &wanted {
                assert!(OPTIONAL_SOURCES.contains(&wanted.source.as_str()), "{wanted:?}");
                inputs.resolve(wanted, "2026-10-01".into());
            }
        }
        panic!("the steps still ask for a fetch");
    }

    fn regions() -> Regions {
        let region = |id: &str, text: &str| parse_region(id, &format!("name = \"{id}\"\n{text}")).unwrap();
        Regions::new(vec![
            region(AREA, "kind = \"geofabrik\"\nareas = [\"europe/test\"]\ncountries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n"),
            region("no-countries", "kind = \"geofabrik\"\nareas = [\"europe/test\"]\ntime_zone = \"Europe/Berlin\"\n"),
            region("no-time-zone", "kind = \"geofabrik\"\nareas = [\"europe/test\"]\ncountries = [\"DE\"]\n"),
            region("boxed", "kind = \"box\"\nbox = [7.79, 47.99, 7.82, 48.02]\ncountries = [\"DE\"]\n"),
        ])
        .unwrap()
    }

    /// A store with what the step list of `AREA` reads: its `.poly`, a box near Freiburg; the
    /// tile list, which names the two squares west of 8°; the land polygons; and the extract of
    /// each of `days`.
    fn store(temp: &Temp, days: &[&str]) -> Store {
        let store = Store::at(temp.0.join("store"));
        crate::device::tests::with_index(&store);
        let area = [("area".to_string(), AREA.to_string())];
        let poly = "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";
        fetched(&store, "geofabrik-poly", "2026-10-01", &area, &[(format!("{AREA}.poly"), poly.into())]);
        let list = "Copernicus_DSM_COG_10_N47_00_E007_00_DEM\nCopernicus_DSM_COG_10_N48_00_E007_00_DEM\n";
        fetched(&store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list.into())]);
        fetched(&store, obc_bake::step::LAND, "1", &[], &[("land-polygons-split-3857.zip".into(), "land".into())]);
        fetched(&store, "protomaps-basemaps", "1", &[], &[(obc_data::fetch::basemap_jar(), "jar".into())]);
        // The box of the region touches the WorldCover squares N45E006 and N48E006; the second is sea here.
        let grid = r#"{"features": [{"properties": {"ll_tile": "N45E006"}}, {"properties": {"ll_tile": "N45E003"}}]}"#;
        fetched(&store, WORLDCOVER_GRID, "v200", &[], &[("esa_worldcover_grid.geojson".into(), grid.into())]);
        for day in days {
            crate::device::tests::extracted(&store, day, &area, &format!("{AREA}-{day}.osm.pbf"));
        }
        store
    }

    fn tiles(input: &Input) -> Vec<&str> {
        let Input::Snapshot { params, .. } = input else { panic!("not a snapshot") };
        params.iter().map(|(_, tile)| &tile["Copernicus_DSM_COG_10_".len()..][..11]).collect()
    }

    #[test]
    fn a_saved_single_area_id_reads_the_selected_upstream_path() {
        let temp = temp("planner-saved-source");
        let store = store(&temp, &["2026-10-01"]);
        let region = parse_region("ride/freiburg", "name = \"My ride\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\ncountries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n").unwrap();
        let regions = Regions::new(vec![region]).unwrap();
        let listed = Planner.steps(&root(), &inputs("ride/freiburg", &[]), &regions, &store).unwrap();
        let osm = listed.steps.iter().find(|step| step.name == "planner/source/europe/test").unwrap();
        let Input::Snapshot { params, .. } = &osm.inputs[0] else {
            panic!("OSM snapshot");
        };
        assert_eq!(params, &[("area".into(), AREA.into())]);
    }

    #[test]
    fn multiple_areas_keep_each_request_and_use_the_union_bounds() {
        let temporary = temp("planner-multi-source");
        let store = store(&temporary, &["2026-10-01"]);
        let area = [("area".into(), "europe/second".into())];
        let shape = "second\n1\n 7.82 47.99\n 7.85 47.99\n 7.85 48.02\n 7.82 48.02\n 7.82 47.99\nEND\nEND\n";
        fetched(&store, "geofabrik-poly", "2026-10-02", &area, &[("europe/second.poly".into(), shape.into())]);
        let bytes = include_bytes!("../tests/data/planner.osm.pbf");
        let sha256 = obc_data::store::sha256_hex(bytes);
        let file = store.partial("file");
        obc_data::store::write_atomic(&file, bytes).unwrap();
        store.insert(&file, &sha256).unwrap();
        let name = "europe/second.osm.pbf";
        let files = vec![obc_data::store::FileRecord {
            name: name.into(),
            url: format!("https://example.org/{name}"),
            size: bytes.len() as u64,
            sha256,
            retrieved: String::new(),
        }];
        store
            .put_snapshot(&obc_data::store::Snapshot { source: EXTRACTS.into(), version: "2026-10-02".into(), files })
            .unwrap();
        store
            .put_requested(
                EXTRACTS,
                &obc_data::store::Requested {
                    version: "2026-10-02".into(),
                    params: area.to_vec(),
                    files: vec![name.into()],
                },
            )
            .unwrap();
        let region = parse_region("ride", "name='Ride'\nkind='geofabrik'\nareas=['europe/test','europe/second']\ncountries=['DE']\ntime_zone='Europe/Berlin'\n").unwrap();
        let inputs = inputs("ride", &[]);
        let steps = Planner
            .steps_with_tool(&root(), &inputs, &Regions::new(vec![region]).unwrap(), &store, Ok(()))
            .unwrap()
            .steps;
        let versions = steps
            .iter()
            .filter(|step| step.name.starts_with("planner/source/"))
            .map(|step| {
                let Input::Snapshot { version, params, .. } = &step.inputs[0] else { panic!("area request") };
                (params[0].1.as_str(), version.as_str())
            })
            .collect::<Vec<_>>();
        assert_eq!(versions, [("europe/second", "2026-10-02"), ("europe/test", "2026-10-01")]);
        let osm = steps.iter().find(|step| step.name == "planner/osm").unwrap();
        assert_eq!(osm.inputs.len(), 2);
        let routing = steps.iter().find(|step| step.name == "planner/routing").unwrap();
        assert_eq!(routing.options["bounds"], json!([7.79, 47.99, 7.85, 48.02]));
        assert_eq!(inputs.requests.borrow().iter().filter(|(source, _)| source == EXTRACTS).count(), 2);
    }

    #[test]
    fn the_terrain_reads_the_mean_tiles_of_terrain_in_its_bounds() {
        let temp = temp("planner-reference");
        let (store, mut inputs) = (store(&temp, &["2026-10-01"]), inputs(AREA, &[]));
        let paths = ["16/4825/4218.tif", "mean/16/4825/4218.tif", "mean/16/4000/4000.tif"];
        let (pinned, manifest) = crate::device::tests::terrain_version(&mut inputs, &paths);
        let manifest = [(obc_data::prepared::MANIFEST.to_string(), manifest)];
        crate::device::tests::stored(&store, "terrain", &pinned, &[], &manifest);
        let steps = Planner.steps(&root(), &inputs, &regions(), &store).unwrap().steps;
        let reference = steps.iter().find(|step| step.name == "planner/reference").unwrap();
        let [Input::Snapshot { files, .. }] = &reference.inputs[..] else { panic!("one model") };
        assert_eq!(
            (&files[..], &reference.options["pool"]),
            (&["mean/16/4825/4218.tif".to_string(), "mean/index.json".to_string()][..], &json!("mean"))
        );
        let terrain = steps.iter().find(|step| step.name == "planner/terrain").unwrap();
        assert!(terrain
            .inputs
            .iter()
            .any(|input| matches!(input, Input::Step { name, .. } if name == "planner/reference")));
    }

    #[test]
    fn a_part_that_is_off_keeps_only_the_steps_that_a_part_that_is_on_reads() {
        let temp = temp("planner-parts");
        let store = store(&temp, &["2026-10-01"]);
        let only = |parts: &[&str]| {
            let mut inputs = inputs(AREA, &[]);
            inputs.settings.layers = parts.iter().map(|part| part.to_string()).collect();
            let steps = Planner.steps(&root(), &inputs, &regions(), &store).unwrap().steps;
            steps.into_iter().map(|step| step.name).collect::<HashSet<_>>()
        };
        let sun = only(&["sun"]);
        assert!(["planner/terrain/grid", "planner/sun/grid", "planner/index", catalog::STEP]
            .iter()
            .all(|step| sun.contains(*step)));
        let core = |step: &String| {
            ["osm", "routing", "search", "basemap", "places", "fonts"].iter().any(|part| step.contains(part))
        };
        assert!(!sun.iter().any(core), "{sun:?}");
        let map = only(&["map"]);
        assert!(
            map.contains("planner/search/pois") && map.contains("planner/fonts/grid"),
            "the places of the map read the POIs"
        );
        let gone = ["planner/search/addresses", "planner/search/pois/grid", "planner/routing", "planner/overlays/grid"];
        assert!(!gone.iter().any(|step| map.contains(*step)), "{map:?}");
    }

    #[test]
    fn a_geofabrik_region_reads_its_newest_extract_and_the_tiles_of_its_bounds() {
        let temp = temp("planner-steps");
        let Err(Unplanned::NeedsFetch(wanted)) =
            Planner.steps(&root(), &inputs(AREA, &[]), &regions(), &store(&temp, &[]))
        else {
            panic!("the store has no extract");
        };
        let area = vec![("area".to_string(), AREA.to_string())];
        assert_eq!(wanted, [Wanted { source: EXTRACTS.into(), version: None, params: area.clone() }]);

        let steps = Planner
            .steps(&root(), &inputs(AREA, &[]), &regions(), &store(&temp, &["2026-10-01", "2026-10-02"]))
            .unwrap()
            .steps;
        let names: Vec<&str> = steps.iter().map(|step| step.name.as_str()).collect();
        let layers = [
            "source/europe/test",
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
            "basemap",
            "terrain/grid",
            "overlays/grid",
            "places/grid",
            "basemap/grid",
            "routing/grid",
            "assets/grid",
            "model/grid",
            "search/pois/grid",
            "search/addresses/grid",
            "fonts/grid",
            "index",
            "catalog",
        ];
        let mut expected = layers.map(|layer| format!("planner/{layer}")).to_vec();
        expected.splice(1..1, ["device/landmark-content".into(), "device/peak-content".into()]);
        assert_eq!(names, expected, "content uses the same producers as device maps");
        let intermediate: Vec<&str> =
            steps.iter().filter(|step| step.client.is_none()).map(|step| step.name.as_str()).collect();
        assert_eq!(
            intermediate,
            [
                "planner/source/europe/test",
                "planner/osm",
                "planner/terrain",
                "planner/routing",
                "planner/overlays",
                "planner/assets",
                "planner/model",
                "planner/search/policy",
                "planner/search/dump",
                "planner/search/records",
                "planner/search/pois",
                "planner/search/addresses",
                "planner/places",
                "planner/basemap"
            ]
        );
        let get = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let (source, osm, terrain, routing) =
            (get("planner/source/europe/test"), get("planner/osm"), get("planner/terrain"), get("planner/routing"));
        assert!(
            matches!(&osm.inputs[0], Input::Step { name, files } if name == "planner/source/europe/test" && files == &["source.osm.pbf"])
        );
        let pois = steps.iter().find(|step| step.name == "planner/search/pois").unwrap();
        for name in ["device/landmark-content", "device/peak-content"] {
            assert!(pois.inputs.iter().any(|input| matches!(input,Input::Step {name:found,..} if found == name)));
        }
        let Input::Snapshot { source, version, params, .. } = &source.inputs[0] else { panic!("not a snapshot") };
        assert_eq!((source.as_str(), version.as_str(), params), (EXTRACTS, "2026-10-02", &area));
        // The terrain covers the region with a margin of 30 km for the sun layer.
        assert_eq!(terrain.options["bounds"], json!([7.382257389170511, 47.71727272727273, 8.4375, 48.45835188280866]));
        assert_eq!(
            tiles(&terrain.inputs[0]),
            ["N47_00_E007", "N48_00_E007"],
            "the tile list names no square east of 8°"
        );
        assert!(matches!(&routing.inputs[0], Input::Step { name, files } if name == "planner/osm" && files.is_empty()));
        assert_eq!(tiles(&routing.inputs[1]), ["N47_00_E007", "N48_00_E007"]);
        assert_eq!(routing.options["bounds"], json!([7.79, 47.99, 7.82, 48.02]));
        assert_eq!((&routing.options["region"], &routing.options["countries"]), (&json!("test"), &json!(["DE"])));
        assert_eq!(
            (&pois.options["bounds"], &pois.options["time_zone"]),
            (&routing.options["bounds"], &json!("Europe/Berlin"))
        );
        let basemap = steps.iter().find(|step| step.name == "planner/basemap").unwrap();
        let sources: Vec<&str> = basemap
            .inputs
            .iter()
            .filter_map(|input| match input {
                Input::Snapshot { source, .. } => Some(source.as_str()),
                Input::Step { .. } => None,
            })
            .collect();
        assert_eq!(sources, BASEMAP_SOURCES);
        assert!(
            matches!(&basemap.inputs[1], Input::Snapshot { files, .. } if files == &[obc_data::fetch::basemap_jar()])
        );
        assert_eq!(basemap.options["bounds"], routing.options["bounds"]);
        assert!(basemap.options["attribution"].as_str().unwrap().contains("Natural Earth"));
        // Pinned bounds: about Baden-Württemberg, and Baden-Württemberg with Switzerland.
        let pinned = [7.03125, 47.04018214480666, 10.922533154247459, 50.07272727272727];
        assert!(terrain_bounds([7.5, 47.5, 10.5, 49.8], 30_000.0)
            .into_iter()
            .zip(pinned)
            .all(|(got, expected)| (got - expected).abs() < 1e-12));
        let pinned = [5.2734375, 45.33670190996811, 10.922970099182649, 50.28933925329178];
        assert!(terrain_bounds([5.95, 45.8, 10.5, 49.85], 30_000.0)
            .into_iter()
            .zip(pinned)
            .all(|(got, expected)| (got - expected).abs() < 1e-12));

        let store = store(&temp, &["2026-10-01"]);
        let result = Planner.steps(&root(), &inputs("no-countries", &[]), &regions(), &store);
        assert!(matches!(result, Err(Unplanned::Invalid(reason)) if reason.contains("no country")));
        // A newer index names the country of the area.
        let index = serde_json::json!({"type":"FeatureCollection", "features":[{"type":"Feature",
            "properties":{"id":"test","name":"Test","parent":null,"iso3166-1:alpha2":["DE"],
                "urls":{"pbf":"https://download.geofabrik.de/europe/test-latest.osm.pbf"}},
            "geometry":{"type":"Polygon","coordinates":[[[7.79,47.99],[7.82,47.99],[7.82,48.02],[7.79,48.02],[7.79,47.99]]]}}]});
        fetched(&store, crate::device::catalog::INDEX, "2", &[], &[("index-v1.json".into(), index.to_string())]);
        let steps = Planner.steps(&root(), &inputs("no-countries", &[]), &regions(), &store).unwrap().steps;
        let routing = steps.iter().find(|step| step.name == "planner/routing").unwrap();
        assert_eq!(routing.options["countries"], json!(["DE"]));
        let steps = Planner.steps(&root(), &inputs("no-time-zone", &[]), &regions(), &store).unwrap().steps;
        let pois = steps.iter().find(|step| step.name == "planner/search/pois").unwrap();
        assert_eq!(pois.options["time_zone"], "UTC");
    }

    #[test]
    fn the_canopy_reads_the_tiles_of_the_region_and_refuses_a_version_whose_scopes_lack_one() {
        use obc_data::prepared::{Cost, Manifest, Read, Scope, MANIFEST};
        use obc_data::store::sha256_hex;

        let temp = temp("planner-canopy");
        let store = Store::at(temp.0.join("store"));
        let id = crate::forest_reference::ID;
        let file = |path: &str| LayerFile { path: path.into(), size: 1, sha256: sha256_hex(path.as_bytes()) };
        let manifest = |scope: Option<Scope>, extends: Vec<Option<Scope>>| Manifest {
            source: id.into(),
            version: "2026-10-20".into(),
            format: 2,
            revision: 1,
            inputs: vec![Read { source: "canopy-ch".into(), version: "2026-10-20".into(), digest: "d".into() }],
            files: ["12/2100/1400.tif", "12/2143/1443.tif", "12/2144/1443.tif"].map(file).into(),
            reused: BTreeMap::new(),
            scope,
            extends,
            languages: Vec::new(),
            licence: None,
            attribution: None,
            machine: "test".into(),
            time: "2026-10-20T00:00:00Z".into(),
            cost: Cost { wall_ms: 0, peak_rss_bytes: None },
        };
        let pin = |inputs: &mut Inputs, manifest: &Manifest| {
            let bytes = manifest.canonical();
            let pinned = format!("2026-10-20/{}", sha256_hex(&bytes));
            inputs.resolved.insert((id.into(), Vec::new()), pinned.clone());
            fetched(&store, id, &pinned, &[], &[(MANIFEST.into(), String::from_utf8(bytes).unwrap())]);
            pinned
        };
        let region = |name: &str, west: f64, east: f64| {
            let text = format!("name = \"{name}\"\nkind = \"box\"\nbox = [{west}, 46.81, {east}, 46.83]\n");
            Some(Scope { region: name.into(), definitions: [(name.into(), text)].into() })
        };
        // Engelberg: the box meets the zoom 12 tiles 2143/1443 and 2144/1443.
        let swiss = ([8.40, 46.81, 8.45, 46.83], &["CH".to_string()][..]);
        let mut inputs = Inputs::default();
        let mut wanted = Vec::new();
        let first = manifest(None, Vec::new());
        let pinned = format!("2026-10-20/{}", sha256_hex(&first.canonical()));
        inputs.resolved.insert((id.into(), Vec::new()), pinned.clone());
        assert!(trailview_canopy(&inputs, &store, swiss, &mut wanted).unwrap().is_none());
        assert_eq!(wanted, [Wanted { source: id.into(), version: Some(pinned), params: Vec::new() }]);

        let pinned = pin(&mut inputs, &first);
        let step = trailview_canopy(&inputs, &store, swiss, &mut Vec::new()).unwrap().unwrap();
        let [Input::Snapshot { source, version, files, .. }] = &step.inputs[..] else { panic!("one snapshot") };
        assert_eq!((source.as_str(), version), (id, &pinned));
        assert_eq!(files, &["12/2143/1443.tif", "12/2144/1443.tif"]);
        let credits = format!("{}; {}", attribution("canopy-ch"), attribution("hrl-dominant-leaf-type"));
        assert_eq!(step.options["attribution"], credits);

        // A version of a box that holds the tile 2143/1443 only, then one that extends another box.
        pin(&mut inputs, &manifest(region("small", 8.40, 8.42), Vec::new()));
        let Err(Unplanned::Invalid(message)) = trailview_canopy(&inputs, &store, swiss, &mut Vec::new()) else {
            panic!("a version of a smaller region refuses")
        };
        assert!(message.contains("obc data source prepare forest --region"), "{message}");
        pin(&mut inputs, &manifest(region("small", 8.40, 8.42), vec![region("east", 8.42, 8.45)]));
        assert!(trailview_canopy(&inputs, &store, swiss, &mut Vec::new()).unwrap().is_some());

        // A region of a country without a canopy model has none.
        let german = (swiss.0, &["DE".to_string()][..]);
        assert!(matches!(trailview_canopy(&inputs, &store, german, &mut Vec::new()), Err(Unplanned::Invalid(_))));
    }

    #[test]
    fn each_step_belongs_to_the_layer_of_its_producer() {
        let temp = temp("planner-layers");
        let store = store(&temp, &["2026-10-01"]);
        let inputs = resolved(inputs(AREA, &OPTIONAL), &regions(), &store);
        crate::device::tests::with_model_areas(&store);
        let mut steps = Planner.steps(&root(), &inputs, &regions(), &store).unwrap().steps;
        steps.extend(Device.steps_with_tool(&root(), &inputs, &regions(), &store, Ok(())).unwrap().steps);
        let layer = |name: &str| {
            let parts: Vec<&str> = name.split('/').collect();
            match parts[..] {
                ["device" | "planner", "source", ..] | ["device", "region-osm" | "osm"] | ["planner", "osm"] => "osm",
                ["device", "reference" | "terrain", _] => "device/terrain",
                ["device", "map", _] => "device/map",
                ["device", "landmark-content" | "peak-content", ..] | ["device", "landmarks" | "peaks", _] => {
                    "device/landmarks"
                }
                ["device", "catalog"] => "device/catalog",
                ["planner", "basemap" | "assets", ..] => "planner/basemap",
                ["planner", "terrain", ..] => "planner/terrain",
                ["planner", "routing" | "overlays", ..] => "planner/routing",
                ["planner", "search" | "model", ..] => "planner/search",
                ["planner", "places", ..] => "planner/places",
                ["planner", "climate", ..] => "planner/climate",
                ["planner", "snow", ..] => "planner/snow",
                ["planner", "sun", ..] => "planner/sun",
                ["planner", "trailview-features", ..] => "planner/trailview-features",
                ["planner", "trailview-rules", ..] => "planner/trailview-rules",
                ["planner", "trailview-material" | "trailview-lithology" | "trailview-region", ..] => {
                    "planner/trailview-ground"
                }
                ["planner", "trailview-colour", ..] => "planner/trailview-colour",
                ["planner", "fonts" | "index" | "catalog", ..] => "planner/index",
                _ => panic!("{name} has no layer"),
            }
        };
        for step in &steps {
            assert_eq!(step.layer.name, layer(&step.name), "{}", step.name);
        }
        let named: BTreeSet<&str> = steps.iter().map(|step| step.layer.name).collect();
        assert_eq!(named.len(), 18, "seventeen operator layers and the OSM of the region: {named:?}");

        // A target holds only client files: a step reads only those of a step of another layer
        // that a program reads, so another machine can bake it without baking that step again. A
        // read of an internal layer is exempt: a machine without it makes it from its sources, as
        // the map leaves split `device/osm` again. A read of the own layer is exempt: a new version
        // of the layer bakes both steps. The reads below bake their step again on such a machine:
        // the POI database is large, and its grid holds it in another form; the landmark and
        // peak content stays private until the wiki is a prepared source; the TrailView ruleset
        // step reads only the repository.
        let mut unheld = Vec::new();
        for step in &steps {
            for input in &step.inputs {
                let Input::Step { name, files } = input else { continue };
                let read = steps.iter().find(|other| &other.name == name).unwrap();
                if read.layer.readers.is_empty() || read.layer.name == step.layer.name {
                    continue;
                }
                let paths = if files.is_empty() { &read.outputs } else { files };
                if !paths.iter().all(|path| read.client.includes(path)) {
                    unheld.push((step.name.as_str(), name.as_str()));
                }
            }
        }
        let exempt = [
            ("planner/search/pois", "device/landmark-content"),
            ("planner/search/pois", "device/peak-content"),
            ("planner/places", "planner/search/pois"),
            ("planner/trailview-material", "planner/trailview-rules"),
            ("planner/trailview-lithology", "planner/trailview-rules"),
            ("planner/trailview-region", "planner/trailview-rules"),
        ];
        assert_eq!(unheld, exempt, "a step reads files of another layer that no client reads");
    }

    #[test]
    fn trailview_features_need_osmium_in_the_plan() {
        let temp = temp("planner-trailview-features");
        let store = store(&temp, &["2026-10-01"]);
        let plan = |layers: &[&str]| {
            Planner.steps_with_tool(&root(), &inputs(AREA, layers), &regions(), &store, Err("no osmium".into()))
        };
        assert!(plan(&[]).is_ok(), "one source area needs no Osmium");
        assert!(matches!(plan(&["trailview-features"]), Err(Unplanned::Invalid(reason)) if reason == "no osmium"));
    }

    #[test]
    fn snow_reads_hr_wsi_where_it_has_data_and_modis_and_the_canopy_everywhere() {
        let temp = temp("planner-snow");
        let store = store(&temp, &[]);
        let sources = |bounds: [f64; 4]| {
            let (mut inputs, mut wanted) = (inputs(AREA, &["snow"]), Vec::new());
            snow(&inputs, &store, bounds, [2016, 2024], &mut wanted).unwrap();
            wanted.iter().for_each(|wanted| inputs.resolve(wanted, "2026-10-01".into()));
            let step = snow(&inputs, &store, bounds, [2016, 2024], &mut Vec::new()).unwrap();
            let inputs = step.inputs.iter().map(|input| match input {
                Input::Snapshot { source, params, .. } => (source.clone(), params.clone()),
                Input::Step { .. } => panic!("a step input"),
            });
            (inputs.collect::<Vec<_>>(), step.options)
        };
        let (inputs, options) = sources([7.5, 47.5, 10.5, 49.8]);
        let names: Vec<&str> = inputs.iter().map(|(source, _)| source.as_str()).collect();
        assert_eq!(names, ["hr-wsi", "modis-snow", "hansen-gfc"]);
        assert_eq!(inputs[2].1, [("tile".to_string(), "50N_000E".to_string()), ("tile".into(), "50N_010E".into())]);
        assert_eq!(options["year"], 2026);
        assert!(options["attribution"].as_str().unwrap().starts_with("© European Union"));

        let (inputs, options) = sources([-106.0, 39.5, -105.0, 40.5]);
        let names: Vec<&str> = inputs.iter().map(|(source, _)| source.as_str()).collect();
        assert_eq!(names, ["modis-snow", "hansen-gfc"], "Colorado is outside HR-WSI");
        assert_eq!(inputs[1].1, [("tile".to_string(), "40N_110W".to_string()), ("tile".into(), "50N_110W".into())]);
        assert_eq!(options["year"], serde_json::Value::Null);
        assert!(options["attribution"].as_str().unwrap().starts_with("MODIS"));

        // Morocco is inside the extent of HR-WSI, and its fetch has no files. The canopy reaches one
        // MODIS pixel west of 10° W.
        let params = [("bbox".to_string(), "-10,31,-7,32".to_string()), ("seasons".into(), "2016-2024".into())];
        fetched(&store, "hr-wsi", "2026-10-01", &params, &[]);
        let (inputs, options) = sources([-10.0, 31.0, -7.0, 32.0]);
        let names: Vec<&str> = inputs.iter().map(|(source, _)| source.as_str()).collect();
        assert_eq!(names, ["modis-snow", "hansen-gfc"]);
        assert_eq!(inputs[1].1, [("tile".to_string(), "40N_020W".to_string()), ("tile".into(), "40N_010W".into())]);
        assert_eq!(options["year"], serde_json::Value::Null);
    }

    #[test]
    fn trailview_ground_reads_the_ruleset_step_and_each_archive_has_a_grid() {
        let temp = temp("planner-trailview");
        let store = store(&temp, &["2026-10-01"]);
        let steps = |layers: &[&str]| {
            let inputs = resolved(inputs(AREA, layers), &regions(), &store);
            Planner.steps(&root(), &inputs, &regions(), &store).map(|listed| listed.steps)
        };
        let result = steps(&["trailview-ground"]);
        assert!(matches!(result, Err(Unplanned::Invalid(reason)) if reason.contains("reads trailview-rules")));
        let steps = steps(&["trailview-rules", "trailview-ground", "trailview-colour"]).unwrap();
        let get = |name: &str| steps.iter().find(|step| step.name == name).unwrap();
        let rules = get("planner/trailview-rules");
        assert_eq!(rules.options["ruleset"], json!(ruleset(&root()).unwrap()));
        let read = |step: &Step| {
            step.inputs
                .iter()
                .map(|input| match input {
                    Input::Snapshot { source, params, .. } => format!("{source}{params:?}"),
                    Input::Step { name, files } => format!("{name}{files:?}"),
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            read(get("planner/trailview-material")),
            [
                r#"planner/trailview-rules["materials.json"]"#,
                r#"planner/osm["osm.pbf"]"#,
                r#"esa-worldcover[("tile", "N45E006")]"#,
                r#"corine-land-cover[("bbox", "7.79,47.99,7.82,48.02")]"#,
                r#"sentinel-2-mosaics[("bbox", "7.79,47.99,7.82,48.02"), ("year", "2025")]"#,
            ]
        );
        assert_eq!(read(get("planner/trailview-region")), [r#"planner/trailview-rules["regions.json"]"#]);
        assert_eq!(worldcover_squares([7.79, 47.99, 7.82, 48.02]), ["N45E006", "N48E006"]);
        let grids: Vec<(&str, &serde_json::Value)> = steps
            .iter()
            .filter(|step| step.name.starts_with("planner/trailview") && step.name.ends_with("/grid"))
            .map(|step| (step.name.as_str(), &step.options["kind"]))
            .collect();
        assert_eq!(grids.len(), 9, "the ruleset, three ground archives and five colour archives: {grids:?}");
        let q1 = get("planner/trailview-colour/q1/grid");
        assert_eq!(read(q1), [r#"planner/trailview-colour["q1.pmtiles"]"#]);
        let index = get("planner/index");
        assert!(index
            .inputs
            .iter()
            .any(|input| matches!(input, Input::Step { name, .. } if name == "planner/trailview-colour/ndvimax/grid")));
    }

    #[test]
    fn climate_changes_only_its_steps_and_the_final_index() {
        let temp = temp("planner-climate");
        let store = store(&temp, &["2026-10-01"]);
        let steps = |layers: &[&str]| {
            let inputs = resolved(inputs(AREA, layers), &regions(), &store);
            Planner.steps(&root(), &inputs, &regions(), &store).unwrap().steps
        };
        let (without, with) = (steps(&[]), steps(&["climate"]));
        let changed: Vec<&str> = with
            .iter()
            .filter(|step| {
                without
                    .iter()
                    .find(|before| before.name == step.name)
                    .is_none_or(|before| obc_data::engine::recipe(before) != obc_data::engine::recipe(step))
            })
            .map(|step| step.name.as_str())
            .collect();
        assert_eq!(changed, ["planner/climate", "planner/climate/grid", "planner/index", "planner/catalog"]);
        let index = with.iter().find(|step| step.name == "planner/index").unwrap();
        assert!(index
            .inputs
            .iter()
            .all(|input| matches!(input, Input::Step { files, .. } if files == &["index.json"])));
    }
}
