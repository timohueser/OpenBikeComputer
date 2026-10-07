//! The planner. Its layers cover the region of the environment: `planner/osm` is the OSM of the
//! region, `planner/basemap` the offline Protomaps map, `planner/terrain` the terrain of the maps,
//! `planner/routing` the routing package with its grid, and `planner/search/dump` the search records of the OSM. `planner/overlays`,
//! `planner/assets`, `planner/model`, `planner/places`, the other `planner/search/*` layers and
//! the optional layers `planner/climate`, `planner/snow` and `planner/sun` are Python steps.
//! Each map producer is intermediate. Its `/grid` step partitions and packs the client tiles.
//! Grid indexes are local inputs for the offline catalog. `data/planner.toml` holds the options
//! that are the same for each region.

use std::collections::HashSet;
use std::path::Path;

use obc_data::engine::{snapshot_files, Client, Code, Input, Run, Step};
use obc_data::env::Env;
use obc_data::product::{version, Product, Unplanned, Wanted};
use obc_data::regions::Regions;
use obc_data::sources::{attribution, embedded};
use obc_data::store::Store;
use obc_dem::step::GLO30;
use planner_router_build::grid::{mercator, tile_bounds};
use serde::Deserialize;
use serde_json::json;

use crate::maps::{invalid, text, TILE_LIST};
use crate::python;

const SEARCH: &str = "planner/search";
/// `planner/search/records.py` and the files that it reads: the data kinds of the query
/// contract, and the POI kinds of the web planner, which the places also read.
const RECORDS: [&str; 3] = ["planner/search/records.py", "planner/search/query/contract.json", POI_KINDS];
const POI_KINDS: &str = "builder/web/src/lib/planner/poi-kinds.json";
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

mod catalog;
pub mod install;
mod local;
mod runtime;

pub struct Planner;

impl Product for Planner {
    fn name(&self) -> &'static str {
        "planner"
    }

    fn approval_config(&self, root: &std::path::Path) -> Result<serde_json::Value, String> {
        runtime::approval_config(root)
    }

    fn runtime_binding(&self, step: &Step) -> Result<Option<obc_data::approval::RuntimeBinding>, String> {
        runtime::binding(step)
    }

    fn planning_code(&self, _env: &Env) -> Result<Option<obc_data::engine::OwnerCode>, String> {
        Ok(Some(obc_data::engine::OwnerCode {
            crate_name: "obc-data-steps".into(),
            code: Code {
                paths: [
                    "host/obc-data-steps/src/planner.rs",
                    "host/obc-data-steps/src/planner",
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
        step.name.starts_with("planner/") && !step.name.starts_with("planner/runtime/") && !step.client.is_none()
    }

    fn local_plan(
        &self,
        root: &std::path::Path,
        env: &Env,
        regions: &Regions,
        store: &Store,
        release: &obc_data::engine::release::Release,
        required: &std::collections::BTreeMap<String, Vec<String>>,
    ) -> Result<obc_data::local::Plan, Unplanned> {
        let declarations = self.declarations(root, env, regions, store, Ok(None), false)?;
        obc_data::local::plan(root, store, self, release, &declarations.steps, required).map_err(Unplanned::Failed)
    }

    fn dev_check(
        &self,
        root: &Path,
        store: &Store,
        request: &obc_data::dev::Request,
    ) -> Result<obc_data::cli::EnvPlan, String> {
        local::check(root, store, request)
    }

    fn dev_inputs(
        &self,
        root: &Path,
        store: &Store,
        request: &obc_data::dev::Request,
        run: &mut obc_data::engine::runs::Run,
    ) -> Result<obc_data::cli::EnvPlan, String> {
        local::inputs(root, store, request, run)
    }

    fn dev_prepare(
        &self,
        root: &std::path::Path,
        store: &Store,
        request: &obc_data::dev::Request,
        run: &mut obc_data::engine::runs::Run,
    ) -> Result<obc_data::dev::Prepared, String> {
        local::prepare(root, store, request, run)
    }

    fn pointer(&self) -> Option<obc_data::product::PointerFn> {
        Some(catalog::pointer)
    }

    fn services(
        &self,
        root: &std::path::Path,
        release: &obc_data::engine::release::Release,
        store: &Store,
        destination: &std::path::Path,
    ) -> Result<Vec<obc_data::vps::Candidate>, String> {
        let origins = runtime::publication(root)?;
        install::prepare_into(release, store, &origins, destination)
    }

    fn named(&self, release: &obc_data::engine::release::Release) -> Result<Vec<obc_data::engine::LayerFile>, String> {
        let mut files = catalog::named(release)?;
        files.extend(runtime::named(release)?);
        Ok(files)
    }

    fn verify(
        &self,
        root: &std::path::Path,
        previous: Option<&obc_data::engine::release::Release>,
        release: &obc_data::engine::release::Release,
        store: &Store,
    ) -> Result<(), String> {
        catalog::verify(root, previous, release, store)
    }

    fn optional(&self) -> &'static [&'static str] {
        &["climate", "snow", "sun"]
    }

    fn steps(
        &self,
        root: &std::path::Path,
        env: &Env,
        regions: &Regions,
        store: &Store,
    ) -> Result<obc_data::product::Steps, Unplanned> {
        self.steps_with_tool(root, env, regions, store, obc_osm::OsmiumRunner::default().binding())
    }
}

impl Planner {
    /// Plan with an explicit prepared-tool observation. Multiple area inputs need the merge tool.
    pub fn steps_with_tool(
        &self,
        root: &std::path::Path,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<obc_data::engine::Library, String>,
    ) -> Result<obc_data::product::Steps, Unplanned> {
        self.declarations(root, env, regions, store, tool.map(Some), true)
    }

    fn declarations(
        &self,
        root: &std::path::Path,
        env: &Env,
        regions: &Regions,
        store: &Store,
        tool: Result<Option<obc_data::engine::Library>, String>,
        execution: bool,
    ) -> Result<obc_data::product::Steps, Unplanned> {
        let config: Config = toml::from_str(include_str!("../../../data/planner.toml"))
            .map_err(|e| Unplanned::Failed(format!("data/planner.toml: {e}")))?;
        let region =
            regions.get(&env.region).ok_or_else(|| Unplanned::Failed(format!("no region `{}`", env.region)))?;
        if region.countries.is_empty() {
            return Err(invalid(format!("region `{}` names no `countries`, which the route catalog needs", region.id)));
        }
        let Some(time_zone) = &region.time_zone else {
            return Err(invalid(format!("region `{}` names no `time_zone`, which the search needs", region.id)));
        };
        let on = |layer: &str| env.layers.iter().any(|name| name == layer);
        let mut wanted = Vec::new();
        let glo30 = version(env, store, GLO30, &[]).map_err(Unplanned::Failed)?.map_err(|fetch| wanted.push(fetch));
        let selection = crate::region_sources::resolve(env, regions, store, &mut wanted)?;
        let tile_list = text(env, store, TILE_LIST, &[], &mut wanted)?;
        let assets = vec![
            snapshot(env, store, "protomaps-assets", Vec::new(), &mut wanted)?,
            snapshot(env, store, "tangrams-icons", Vec::new(), &mut wanted)?,
        ];
        let model = snapshot(env, store, "query-model", Vec::new(), &mut wanted)?;
        let country_data = snapshot(env, store, "nominatim-country-data", Vec::new(), &mut wanted)?;
        let (Some(selection), Some(tile_list), Ok(glo30)) = (selection, tile_list, glo30) else {
            return Err(Unplanned::NeedsFetch(wanted));
        };
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
                name: "planner/osm".into(),
                inputs: vec![Input::Layer { name: source_steps[0].name.clone(), files: vec!["source.osm.pbf".into()] }],
                options: json!({"path": "osm.pbf"}),
                code: Code { crates: vec!["obc-data".into()], ..Default::default() },
                outputs: vec!["osm.pbf".into()],
                run: Run::Rust(obc_data::engine::pass),
                client: Client::None,
            }
        } else {
            crate::region_sources::combined("planner/osm", &source_steps, tool.map_err(Unplanned::Invalid)?.as_ref())?
        };
        let mut inputs = vec![Input::layer(osm.name.clone())];
        for source in BASEMAP_SOURCES {
            let mut input = snapshot(env, store, source, Vec::new(), &mut wanted)?;
            if let Input::Snapshot { files, .. } = &mut input {
                if source == "protomaps-basemaps" {
                    *files = vec![obc_data::fetch::basemap_jar()];
                }
            }
            inputs.push(input);
        }
        let credit = ["osm-planet", "natural-earth", "daylight-landcover"].map(attribution).join("; ");
        let basemap = python(
            "planner/basemap",
            inputs,
            json!({"bounds": bounds, "attribution": credit}),
            ("tools.planner_basemap", None),
            &["tools/planner_basemap.py"],
            &["basemap.pmtiles"],
        );
        let terrain = Step {
            name: "planner/terrain".into(),
            inputs: tiles(coverage, &land, &glo30),
            options: json!({"bounds": coverage}),
            code: Code {
                paths: Vec::new(),
                crates: vec!["obc-dem".into()],
                sources: vec!["copernicus-glo-30".into()],
                ..Default::default()
            },
            outputs: vec!["terrain.mbtiles".into()],
            run: Run::Rust(obc_dem::step::planner_terrain),
            client: Client::All,
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
            code: Code {
                paths: Vec::new(),
                crates: vec!["planner-router-build".into()],
                sources: vec!["osm-planet".into(), "copernicus-glo-30".into()],
                ..Default::default()
            },
            outputs: vec!["routing".into(), "blocks".into(), "routes".into()],
            run: Run::Rust(planner_router_build::step::step),
            client: Client::All,
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
            ("planner/search/setup.py", None),
            &[
                "planner/search/setup.py",
                "planner/search/query/artifacts.py",
                "planner/search/query/schema.py",
                "planner/search/query/contract.json",
            ],
            &["model"],
        );
        let policy = Step {
            client: Client::None,
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
            code: Code { paths: Vec::new(), crates: vec!["obc-search-bake".into()], ..Default::default() },
            outputs: vec!["search.jsonl.zst".into()],
            run: Run::Rust(obc_search_bake::step::step),
            client: Client::None,
        };
        let records = Step {
            client: Client::None,
            ..python(
                "planner/search/records",
                vec![Input::layer(dump.name.clone())],
                json!({}),
                ("planner/search/split.py", Some("planner-search")),
                &[["planner/search/split.py"].as_slice(), &RECORDS].concat(),
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
        let mut steps = source_steps;
        steps.extend([
            osm, terrain, routing, overlays, assets, model, policy, dump, records, pois, addresses, places, basemap,
        ]);
        if on("climate") {
            let first_year = config.climate.first_year;
            let params = vec![bbox, ("first-year".to_string(), first_year.to_string())];
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
            steps.push(snow(env, store, bounds, config.snow.seasons, &mut wanted)?);
        }
        if on("sun") {
            steps.push(python(
                "planner/sun",
                vec![Input::layer("planner/terrain/grid")],
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
                    "tools/planner_offline.py",
                    "tools/planner_runtime.py",
                    "tools/planner_map_archive.py",
                    "tools/planner_geo.py",
                ],
                &["sun"],
            ));
        }
        let maps = ["basemap", "places", "terrain", "overlays", "climate", "snow", "sun"];
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
            let source = source.name.clone();
            let search = kind.strip_prefix("search/");
            let (module, group, code) = match search {
                Some(_) => (
                    "tools.planner_grid_search",
                    Some("planner-search"),
                    vec![
                        "tools/planner_grid_search.py",
                        "tools/planner_grid.py",
                        "tools/planner_geo.py",
                        "tools/planner_offline.py",
                        "tools/planner_runtime.py",
                        "planner/search/storage.py",
                        "planner/search/index.py",
                        "planner/search/schema.sql",
                        "planner/search/indexes.sql",
                        "planner/search/web/address-terms.json",
                    ],
                ),
                None => ("tools.planner_grid_pack", None, PACK.to_vec()),
            };
            let options = match search {
                Some(component) => json!({"kind": component, "component": component, "bounds": bounds}),
                None => json!({"kind": kind}),
            };
            steps.push(Step {
                client: Client::Paths(vec!["objects".into()]),
                ..python(
                    &format!("{source}/grid"),
                    vec![Input::layer(source)],
                    options,
                    (module, group),
                    &code,
                    &["objects", "index.json"],
                )
            });
        }
        steps.push(Step {
            client: Client::Paths(vec!["objects".into()]),
            ..python(
                "planner/fonts/grid",
                vec![
                    layer_files("planner/basemap", &["basemap.pmtiles"]),
                    layer_files("planner/places", &["places.pmtiles"]),
                    layer_files("planner/routing", &["routing/overlays.sqlite"]),
                    Input::layer("planner/assets"),
                ],
                json!({"kind": "fonts"}),
                ("tools.planner_grid_fonts", Some("planner-maps")),
                &PACK
                    .iter()
                    .copied()
                    .chain(["tools/planner_grid_fonts.py", "tools/planner_mvt.py"])
                    .collect::<Vec<_>>(),
                &["objects", "index.json"],
            )
        });
        let overlays = steps.iter_mut().find(|step| step.name == "planner/overlays/grid").unwrap();
        overlays.inputs.push(layer_files("planner/routing/grid", &["index.json"]));
        let indexes = steps
            .iter()
            .filter(|step| step.name.ends_with("/grid"))
            .map(|step| layer_files(&step.name, &["index.json"]))
            .collect();
        steps.push(Step {
            client: Client::Paths(vec!["objects".into()]),
            ..python(
                "planner/index",
                indexes,
                json!({
                    "region": name, "name": region.name, "bounds": bounds,
                    "attribution": attribution("osm-planet"),
                    "landcover_attribution": attribution("daylight-landcover"),
                }),
                ("tools.planner_grid_index", None),
                &[
                    "tools/planner_grid_index.py",
                    "tools/planner_grid.py",
                    "tools/planner_geo.py",
                    "tools/planner_offline.py",
                    "tools/planner_runtime.py",
                ],
                &["objects", "release.json", "public", "index.json"],
            )
        });
        match wanted.is_empty() {
            true => {
                let mut runtime = if execution { runtime::steps(root) } else { Default::default() };
                steps.append(&mut runtime.steps);
                Ok(obc_data::product::Steps { steps, blocked: runtime.blocked })
            }
            false => Err(Unplanned::NeedsFetch(wanted)),
        }
    }
}

const PACK: [&str; 3] = ["tools/planner_grid_pack.py", "tools/planner_offline.py", "tools/planner_runtime.py"];

fn layer_files(name: &str, files: &[&str]) -> Input {
    Input::Layer { name: name.into(), files: files.iter().map(|file| (*file).into()).collect() }
}

/// A map's transport objects ship; the small index is input to the offline catalog only.
fn map_grid(source: &Step, kind: &str, bounds: [f64; 4]) -> Step {
    let bounds = source.options.get("bounds").cloned().unwrap_or_else(|| json!(bounds));
    Step {
        client: Client::Paths(vec!["objects".into()]),
        ..python(
            &format!("{}/grid", source.name),
            vec![Input::layer(source.name.clone())],
            json!({"kind": kind, "bounds": bounds}),
            ("tools.planner_grid_maps", Some("planner-maps")),
            &[
                "tools/planner_grid_maps.py",
                "tools/planner_map_archive.py",
                "tools/planner_geo.py",
                "tools/planner_offline.py",
                "tools/planner_runtime.py",
            ],
            &["objects", "index.json"],
        )
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

/// The snow layer of `bounds`: HR-WSI where it has data, and MODIS elsewhere. Outside the extent
/// of HR-WSI, or where its fetch has no files, it reads MODIS only.
fn snow(
    env: &Env,
    store: &Store,
    bounds: [f64; 4],
    [first, last]: [u16; 2],
    wanted: &mut Vec<Wanted>,
) -> Result<Step, Unplanned> {
    let bbox = ("bbox".to_string(), bounds.map(|degrees| degrees.to_string()).join(","));
    let params = vec![bbox, ("seasons".to_string(), format!("{first}-{last}"))];
    let tiles = canopy_tiles(bounds).into_iter().map(|tile| ("tile".to_string(), tile)).collect();
    let mut inputs = vec![
        snapshot(env, store, "modis-snow", params.clone(), wanted)?,
        snapshot(env, store, "hansen-gfc", tiles, wanted)?,
    ];
    let mut credit = format!("{}; tree canopy: {}", attribution("modis-snow"), attribution("hansen-gfc"));
    let mut year = None;
    if embedded("hr-wsi").meets(bounds) {
        let hr_wsi = snapshot(env, store, "hr-wsi", params, wanted)?;
        let Input::Snapshot { version, params, .. } = &hr_wsi else { unreachable!("a fetch is a snapshot input") };
        let files = snapshot_files(store, "hr-wsi", version, params, &[]).map_err(Unplanned::Failed)?;
        if !files.is_some_and(|files| files.is_empty()) {
            // The credit of HR-WSI names a year: the year of the capture.
            year = version.get(..4).and_then(|year| year.parse::<u16>().ok());
            credit = format!("{}; {credit}", attribution("hr-wsi"));
            inputs.insert(0, hr_wsi);
        }
    }
    Ok(python(
        "planner/snow",
        inputs,
        json!({"bounds": bounds, "seasons": [first, last], "year": year, "attribution": credit}),
        ("tools.planner_snow", Some("planner-snow")),
        &["tools/planner_snow.py", "tools/planner_geo.py"],
        &["snow.pmtiles"],
    ))
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

/// Resolve only the native routing providers for the checked runtime adapter.
pub fn native_routing(args: &[String]) -> Option<Result<serde_json::Value, String>> {
    runtime::native_routing(args)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::Path;

    use obc_data::regions::parse_region;

    use super::*;
    use crate::maps::tests::{fetched, root, temp, Temp};
    use crate::maps::{Maps, EXTRACTS};

    const AREA: &str = "europe/test";

    /// An environment with `layers` on that names a version of every source the planner reads but
    /// the extract: live reads the sources without params, and the captures move.
    fn env(region: &str, layers: &[&str]) -> Env {
        let read = [GLO30, TILE_LIST, "protomaps-assets", "tangrams-icons", "query-model", "nominatim-country-data"];
        let live = read
            .into_iter()
            .chain(BASEMAP_SOURCES)
            .map(|source| ((source.into(), Vec::new()), BTreeSet::from(["1".into()])));
        let moves = ["era5-land", "hr-wsi", "modis-snow"].map(|source| (source.into(), Some("2026-10-01".into())));
        let moves = moves.into_iter().chain([("hansen-gfc".into(), Some("v1.11".into()))]).collect();
        let layers = layers.iter().map(|layer| layer.to_string()).collect();
        Env { name: "test".into(), region: region.into(), layers, live: live.collect(), moves, ..Env::default() }
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
        crate::maps::tests::with_index(&store);
        let area = [("area".to_string(), AREA.to_string())];
        let poly = "test\n1\n   7.79 47.99\n   7.82 47.99\n   7.82 48.02\n   7.79 48.02\n   7.79 47.99\nEND\nEND\n";
        fetched(&store, "geofabrik-poly", "2026-10-01", &area, &[(format!("{AREA}.poly"), poly.into())]);
        let list = "Copernicus_DSM_COG_10_N47_00_E007_00_DEM\nCopernicus_DSM_COG_10_N48_00_E007_00_DEM\n";
        fetched(&store, TILE_LIST, "1", &[], &[("tileList.txt".into(), list.into())]);
        fetched(&store, obc_draw::step::LAND, "1", &[], &[("land-polygons-split-3857.zip".into(), "land".into())]);
        fetched(&store, "protomaps-basemaps", "1", &[], &[(obc_data::fetch::basemap_jar(), "jar".into())]);
        for day in days {
            fetched(&store, EXTRACTS, day, &area, &[(format!("{AREA}-{day}.osm.pbf"), (*day).into())]);
            crate::maps::tests::captured(&store, "1", AREA, day, poly);
        }
        store
    }

    fn tiles(input: &Input) -> Vec<&str> {
        let Input::Snapshot { params, .. } = input else { panic!("not a snapshot") };
        params.iter().map(|(_, tile)| &tile["Copernicus_DSM_COG_10_".len()..][..11]).collect()
    }

    #[test]
    fn local_metadata_pins_selected_versions_and_derives_the_current_saved_box() {
        let temp = temp("planner-local-metadata");
        let store = store(&temp, &["2026-10-01", "2026-10-02"]);
        let mut env = env("ride", &[]);
        env.live.insert((EXTRACTS.into(), vec![("area".into(), AREA.into())]), ["2026-10-01".into()].into());
        let region = |bbox: &str| {
            Regions::new(vec![parse_region(
                "ride",
                &format!(
            "name = \"Ride\"\nkind = \"box\"\nbox = [{bbox}]\ncountries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n"
        ),
            )
            .unwrap()])
            .unwrap()
        };
        std::fs::create_dir_all(temp.0.join("data/env")).unwrap();
        std::fs::write(Env::path(&temp.0, "live"), "region = \"ride\"\nlayers = []\n").unwrap();
        let definitions = region("7.79, 47.99, 7.82, 48.02");
        let request = obc_data::dev::Request {
            region: Some("ride".into()),
            refresh_live: false,
            app: obc_data::dev::App::WebPlanner,
            inputs_only: false,
            reviewed: None,
        };
        let configured = local::environment(&temp.0, &definitions, &request).unwrap();
        assert_eq!(configured, Env::load(&temp.0, "local", &definitions).unwrap());
        std::fs::write(Env::path(&temp.0, "local"), "region = \"ride\"\nlayers = [\"sun\"]\n").unwrap();
        let configured = local::environment(
            &temp.0,
            &definitions,
            &obc_data::dev::Request {
                region: None,
                refresh_live: true,
                app: obc_data::dev::App::WebPlanner,
                inputs_only: false,
                reviewed: None,
            },
        )
        .unwrap();
        assert_eq!(configured.layers, ["sun"]);
        assert_eq!(configured, Env::load(&temp.0, "local", &definitions).unwrap());
        assert!(!std::fs::read_to_string(Env::path(&temp.0, "local")).unwrap().contains("pins"));
        let first = Planner
            .declarations(&root(), &env, &region("7.79, 47.99, 7.82, 48.02"), &store, Ok(None), false)
            .unwrap()
            .steps;
        let changed = Planner
            .declarations(&root(), &env, &region("7.79, 47.99, 7.81, 48.01"), &store, Ok(None), false)
            .unwrap()
            .steps;
        for steps in [&first, &changed] {
            assert!(steps.iter().all(|step| !step.name.starts_with("planner/runtime/")));
            let extract = steps.iter().find(|step| step.name == "planner/source/europe/test").unwrap();
            assert!(
                matches!(&extract.inputs[0], Input::Snapshot { source, version, .. } if source == EXTRACTS && version == "2026-10-01")
            );
            assert!(extract.code.libraries.is_empty(), "comparison does not need the original Osmium executable");
        }
        let routing = |steps: &[Step]| {
            steps.iter().find(|step| step.name == "planner/routing").unwrap().options["bounds"].clone()
        };
        assert_ne!(routing(&first), routing(&changed), "the same saved ID does not hide a changed box");
    }

    #[test]
    fn a_saved_single_area_id_reads_the_selected_upstream_path() {
        let temp = temp("planner-saved-source");
        let store = store(&temp, &["2026-10-01"]);
        let region = parse_region("ride/freiburg", "name = \"My ride\"\nkind = \"geofabrik\"\nareas = [\"europe/test\"]\ncountries = [\"DE\"]\ntime_zone = \"Europe/Berlin\"\n").unwrap();
        let regions = Regions::new(vec![region]).unwrap();
        let listed = Planner.steps(&root(), &env("ride/freiburg", &[]), &regions, &store).unwrap();
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
        fetched(&store, EXTRACTS, "2026-10-02", &area, &[("europe/second.osm.pbf".into(), "second".into())]);
        let region = parse_region("ride", "name='Ride'\nkind='geofabrik'\nareas=['europe/test','europe/second']\ncountries=['DE']\ntime_zone='Europe/Berlin'\n").unwrap();
        let env = env("ride", &[]);
        let steps = Planner
            .steps_with_tool(
                &root(),
                &env,
                &Regions::new(vec![region]).unwrap(),
                &store,
                Ok(obc_data::engine::Library {
                    name: "osmium".into(),
                    path: std::path::PathBuf::from("/authored-copy-osmium"),
                    sha256: "0".repeat(64),
                }),
            )
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
        assert_eq!(env.read.borrow().iter().filter(|((source, _), _)| source == EXTRACTS).count(), 2);
    }

    #[test]
    fn a_geofabrik_region_reads_its_newest_extract_and_the_tiles_of_its_bounds() {
        let temp = temp("planner-steps");
        let Err(Unplanned::NeedsFetch(wanted)) =
            Planner.steps(&root(), &env(AREA, &[]), &regions(), &store(&temp, &[]))
        else {
            panic!("the store has no extract");
        };
        let area = vec![("area".to_string(), AREA.to_string())];
        assert_eq!(wanted, [Wanted { source: EXTRACTS.into(), version: None, params: area.clone() }]);

        let steps = Planner
            .steps(&root(), &env(AREA, &[]), &regions(), &store(&temp, &["2026-10-01", "2026-10-02"]))
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
        ];
        assert_eq!(names, layers.map(|layer| format!("planner/{layer}")), "no optional layer is on");
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
        let [source, osm, terrain, routing, ..] = &steps[..] else { unreachable!() };
        assert!(
            matches!(&osm.inputs[0], Input::Layer { name, files } if name == "planner/source/europe/test" && files == &["source.osm.pbf"])
        );
        let pois = steps.iter().find(|step| step.name == "planner/search/pois").unwrap();
        let Input::Snapshot { source, version, params, .. } = &source.inputs[0] else { panic!("not a snapshot") };
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
        let basemap = steps.iter().find(|step| step.name == "planner/basemap").unwrap();
        let sources: Vec<&str> = basemap
            .inputs
            .iter()
            .filter_map(|input| match input {
                Input::Snapshot { source, .. } => Some(source.as_str()),
                Input::Layer { .. } => None,
            })
            .collect();
        assert_eq!(sources, BASEMAP_SOURCES);
        assert!(
            matches!(&basemap.inputs[1], Input::Snapshot { files, .. } if files == &[obc_data::fetch::basemap_jar()])
        );
        assert_eq!(basemap.options["bounds"], routing.options["bounds"]);
        assert!(basemap.options["attribution"].as_str().unwrap().contains("Natural Earth"));
        assert!(!basemap.code.paths.iter().any(|path| path == "data/sources.toml"));
        // About Baden-Württemberg, and the box of the old planner recipe.
        let old = [7.03125, 47.04018214480666, 10.922533154247459, 50.07272727272727];
        assert!(terrain_bounds([7.5, 47.5, 10.5, 49.8], 30_000.0)
            .into_iter()
            .zip(old)
            .all(|(got, expected)| (got - expected).abs() < 1e-12));
        let old = [5.2734375, 45.33670190996811, 10.922970099182649, 50.28933925329178];
        assert!(terrain_bounds([5.95, 45.8, 10.5, 49.85], 30_000.0)
            .into_iter()
            .zip(old)
            .all(|(got, expected)| (got - expected).abs() < 1e-12));

        for region in ["boxed", "no-countries", "no-time-zone"] {
            let result = Planner.steps(&root(), &env(region, &[]), &regions(), &store(&temp, &["2026-10-01"]));
            assert!(matches!(result, Err(Unplanned::Invalid(_))), "{region}");
        }
    }

    #[test]
    fn the_code_of_route_build_is_the_code_of_the_routing_layer_only() {
        let temp = temp("planner-code");
        let store = store(&temp, &["2026-10-01"]);
        crate::maps::tests::without_models(&store, &env(AREA, &[]), &regions());
        let mut steps = Planner.steps(&root(), &env(AREA, &[]), &regions(), &store).unwrap().steps;
        steps.extend(
            Maps.steps_with_tool(
                &root(),
                &env(AREA, &[]),
                &regions(),
                &store,
                Ok(crate::maps::tests::authored_tool(store.root())),
            )
            .unwrap()
            .steps,
        );
        for step in &steps {
            let files = step.code.files(&root()).unwrap();
            assert!(!files.contains_key("Cargo.lock"), "{} declares Cargo.lock", step.name);
            let planner_router_build = files.keys().any(|path| path.starts_with("planner/router-build/src/"));
            assert_eq!(planner_router_build, step.name == "planner/routing", "{}", step.name);
            if step.name == "planner/terrain" {
                assert!(files.keys().any(|path| path.starts_with("host/obc-dem/src/")));
                assert!(
                    !files.keys().any(|path| path.starts_with("planner/router-build/")),
                    "terrain reads planner-router-build"
                );
            }
        }
    }

    #[test]
    fn snow_reads_hr_wsi_where_it_has_data_and_modis_and_the_canopy_everywhere() {
        let temp = temp("planner-snow");
        let store = store(&temp, &[]);
        let sources = |bounds: [f64; 4]| {
            let step = snow(&env(AREA, &["snow"]), &store, bounds, [2016, 2024], &mut Vec::new()).unwrap();
            let inputs = step.inputs.iter().map(|input| match input {
                Input::Snapshot { source, params, .. } => (source.clone(), params.clone()),
                Input::Layer { .. } => panic!("a layer input"),
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
    fn climate_changes_only_its_steps_and_the_final_index() {
        let temp = temp("planner-climate");
        let store = store(&temp, &["2026-10-01"]);
        let steps = |layers: &[&str]| Planner.steps(&root(), &env(AREA, layers), &regions(), &store).unwrap().steps;
        let (without, with) = (steps(&[]), steps(&["climate"]));
        let changed: Vec<&str> = with
            .iter()
            .filter(|step| {
                without.iter().find(|before| before.name == step.name).is_none_or(|before| {
                    obc_data::engine::recipe(before, "code") != obc_data::engine::recipe(step, "code")
                })
            })
            .map(|step| step.name.as_str())
            .collect();
        assert_eq!(changed, ["planner/climate", "planner/climate/grid", "planner/index"]);
        let index = with.iter().find(|step| step.name == "planner/index").unwrap();
        assert!(index
            .inputs
            .iter()
            .all(|input| matches!(input, Input::Layer { files, .. } if files == &["index.json"])));
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
        let steps = Planner.steps(&root(), &env(AREA, &["climate", "snow", "sun"]), &regions(), &store).unwrap().steps;
        let sun = steps.iter().find(|step| step.name == "planner/sun").unwrap();
        assert!(
            matches!(sun.inputs.as_slice(), [Input::Layer { name, files }]
            if name == "planner/terrain/grid" && files.is_empty()),
            "sun consumes portable terrain without its private bake input"
        );
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
        assert_eq!(python, 26, "twelve producers, thirteen grid steps and the final index");
    }
}
