//! The planner routing steps of `obc data`. Three work layers hold the bake up to the package, so
//! that a stop or a new profile list repeats only the steps after it: `planner/routing-source`
//! (the OSM objects with their facts), `planner/routing-roads` (the roads with their terrain and
//! their topology) and `planner/routing-costs` (the costs of each profile). `planner/routing`
//! writes the package, its overlay index, its route catalog, the routing blocks and the routes of
//! each grid cell.

use std::fs;
use std::path::{Path, PathBuf};

use obc_data::engine::Request;
use obc_dem::planner::{Recent, Terrain};
use planner_router::{directory::Writer, model::Point, package};
use serde_json::{json, Value};

use crate::base::Topology;
use crate::column::Layout;
use crate::osm::{Counts, Source};
use crate::source::{Data, Sets};
use crate::work::{Guard, OWN_BYTES};
use crate::{catalog, grid, overlays, Costs, Graph, Limits, SETS};

/// The step of the source layer. The last part of the name of a work layer names its folder.
pub const SOURCE: &str = "planner/routing-source";
pub const ROADS: &str = "planner/routing-roads";
pub const COSTS: &str = "planner/routing-costs";

fn text(options: &Value, name: &str) -> Result<String, String> {
    options[name].as_str().map(str::to_string).ok_or(format!("option `{name}` is not a string"))
}

fn list(options: &Value, name: &str) -> Result<Vec<String>, String> {
    let values = options[name].as_array().ok_or(format!("option `{name}` is not a list"))?;
    values
        .iter()
        .map(|value| value.as_str().map(str::to_string).ok_or(format!("option `{name}` lists a non-string")))
        .collect()
}

/// Starts the memory guard, and reads the page budget first, so that a bad value fails before the
/// work.
fn start() -> Result<Guard, String> {
    let guard = Guard::start(OWN_BYTES);
    let (budget, source) = crate::work::page_budget()?;
    eprintln!("Page budget: {} MiB, from {source}", budget >> 20);
    Ok(guard)
}

fn folder_name(step: &str) -> &str {
    step.rsplit('-').next().expect("a step name")
}

/// The folder of the input layer of `step`, by the names of its files in the folder.
fn folder(request: &Request, step: &str) -> Result<Layout, String> {
    let files = request.layers.get(step).ok_or(format!("the step reads no layer {step}"))?;
    let prefix = format!("{}/", folder_name(step));
    let paths = files.iter().filter_map(|(path, object)| Some((path.strip_prefix(&prefix)?.into(), object.clone())));
    Layout::read(paths.collect())
}

/// The new folder of the layer of `step` in the output.
fn output(request: &Request, step: &str) -> Result<Layout, String> {
    Layout::create(&request.output.join(folder_name(step)))
}

fn read_json(layout: &Layout, name: &str) -> Result<Value, String> {
    let path = layout.path(name)?;
    let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{name}: {e}"))
}

fn write(layout: &Layout, name: &str, bytes: &[u8]) -> Result<(), String> {
    let path = layout.path(name)?;
    fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_sets(layout: &Layout) -> Result<Sets, String> {
    let path = layout.path(SETS)?;
    Sets::from_bytes(&fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect()
}

/// The source step. It reads the OSM files of its input layer and the option `bounds`. The layer
/// `source/` holds the ways with their node references and facts, the relations, the present node
/// members, the turn rules of the relations (`rules`), the closure sets, and `source.json`: the
/// counts of the import so far and the SHA-256 of each OSM file, which `layer_files` gives.
pub fn source(request: &Request) -> Result<(), String> {
    let _guard = start()?;
    Guard::stage("import");
    let bounds = obc_dem::step::bounds(&request.options)?;
    let (mut inputs, mut sha256) = (Vec::<PathBuf>::new(), Vec::new());
    for (step, files) in &request.layers {
        for (path, object) in files {
            let record = request.layer_files.get(step).into_iter().flatten().find(|file| &file.path == path);
            sha256.push(record.ok_or(format!("the request gives no SHA-256 of {step}/{path}"))?.sha256.clone());
            inputs.push(object.clone());
        }
    }
    eprintln!("Importing the source");
    let Source { osm, sets, rules, counts } = crate::osm::source(&inputs, bounds)?;
    let mut layout = output(request, SOURCE)?;
    osm.save(&mut layout)?;
    layout.save("rules", rules)?;
    write(&layout, SETS, &sets.to_bytes())?;
    write(&layout, "source.json", json!({"counts": counts, "sha256": sha256}).to_string().as_bytes())?;
    layout.finish()
}

/// The roads step. It reads the source layer, the GLO-30 tiles of its bounds and, when the request
/// has it, the layer `planner/reference`: the heights of the planner terrain without its shapes.
/// The options are `bounds` and `terrain_attribution`, the credit of GLO-30. The layer `roads/`
/// holds the junctions, the roads in their final order with their heights, the forbidden turns,
/// the closure sets with those of the roads, the topology (`first`, `head`), and `roads.json`: the
/// warnings of the manifest and the identities of its inputs, the OSM files and then the terrain.
pub fn roads(request: &Request) -> Result<(), String> {
    let _guard = start()?;
    Guard::stage("import");
    let bounds = obc_dem::step::bounds(&request.options)?;
    let credit = text(&request.options, "terrain_attribution")?;
    let source = folder(request, SOURCE)?;
    let summary = read_json(&source, "source.json")?;
    let counts: Counts = serde_json::from_value(summary["counts"].clone()).map_err(|e| e.to_string())?;
    let rules = source.open("rules")?;
    let source = Source { osm: Data::open(&source)?, sets: read_sets(&source)?, rules, counts };
    let reference = obc_dem::step::reference(request)?;
    let terrain = Terrain::open(&obc_dem::step::glo30(request, bounds), &credit, reference.as_deref(), bounds)?;
    eprintln!("Building the roads");
    let mut recent = Recent::default();
    let mut height = |point: Point| terrain.height(point.lat, point.lon, &mut recent);
    let mut graph = crate::osm::roads(source, Some(&mut height))?;
    crate::check_closures(&graph)?;
    let mut warnings = std::mem::take(&mut graph.warnings);
    warnings.retain(|w| !w.starts_with("No DEM applied"));
    warnings.push(crate::terrain::WARNING.into());
    warnings.extend(terrain.attribution());
    let mut identities = strings(&summary["sha256"]);
    identities.extend(terrain.identities.iter().cloned());
    // The decoded squares and tiles are not needed after the heights.
    drop((terrain, recent));
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    let mut layout = output(request, ROADS)?;
    layout.save("first", topology.first)?;
    layout.save("head", topology.head)?;
    write(&layout, "roads.json", json!({"warnings": warnings, "identities": identities}).to_string().as_bytes())?;
    graph.save(&mut layout)?;
    layout.finish()
}

/// The graph of the source and the roads layers, its topology, and the identities of its inputs.
fn graph(request: &Request) -> Result<(Graph, Topology, Vec<String>), String> {
    let osm = Data::open(&folder(request, SOURCE)?)?;
    let roads = folder(request, ROADS)?;
    let summary = read_json(&roads, "roads.json")?;
    let graph = Graph::open(&roads, osm, strings(&summary["warnings"]))?;
    let topology = Topology { first: roads.open("first")?, head: roads.open("head")? };
    if topology.first.len() != graph.roads.len() + 1 {
        return Err("the topology has another number of roads than the roads".into());
    }
    Ok((graph, topology, strings(&summary["identities"])))
}

/// The costs step. It reads the source and the roads layers and the options `bounds` and
/// `profiles`, the profile IDs or `all`. The layer `costs/` holds the costs of each profile
/// (`Costs::save`).
pub fn costs(request: &Request) -> Result<(), String> {
    let _guard = start()?;
    Guard::stage("open");
    let bounds = obc_dem::step::bounds(&request.options)?;
    let names = list(&request.options, "profiles")?;
    let profiles = crate::profiles(&names.iter().map(String::as_str).collect::<Vec<_>>())?;
    let (graph, topology, _) = graph(request)?;
    Limits { profiles: profiles.len(), bounds }.check(&graph)?;
    crate::check(&graph, &profiles)?;
    eprintln!("Preparing {} profiles for {} directed roads", profiles.len(), graph.roads.len());
    let costs = crate::costs(&graph, &topology, &profiles)?;
    let mut layout = output(request, COSTS)?;
    costs.save(&mut layout)?;
    layout.finish()
}

/// The package step. It reads the three work layers and the options `region`, `bounds`,
/// `countries`, the ISO codes of the countries of the region for the route catalog, and
/// `attribution`, the credit of the OSM data. The layer:
///
/// - `routing/`: the package, `overlays.sqlite` and `route-catalog.json`;
/// - `blocks/`: the routing blocks of the grid cells (`blocks::publish`);
/// - `routes/<cell>.json`: the routes of each grid cell.
///
/// Its metrics are the report of the route catalog.
pub fn step(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let _guard = start()?;
    Guard::stage("open");
    let bounds = obc_dem::step::bounds(options)?;
    let countries = list(options, "countries")?;
    if countries.is_empty() {
        return Err("option `countries` is empty: the routes of each cell come from the route catalog".into());
    }
    let (region, attribution) = (text(options, "region")?, text(options, "attribution")?);
    let costs = Costs::open(&folder(request, COSTS)?)?;
    let france = catalog::check(&countries, &costs.profiles)?;
    let (mut graph, topology, identities) = graph(request)?;
    let routing = request.output.join("routing");
    fs::create_dir(&routing).map_err(|e| format!("{}: {e}", routing.display()))?;
    eprintln!("Writing the package of {} profiles", costs.profiles.len());
    let manifest = write_package(&graph, topology, costs, (region, bounds), identities, attribution, &routing)?;
    // The overlay index and the route catalog read only the source objects. The road graph is
    // freed here, so the catalog routers get its memory.
    let osm = std::mem::take(&mut graph.osm);
    drop(graph);
    Guard::stage("overlays");
    eprintln!("Writing the overlay index");
    overlays::write(&routing.join(overlays::FILE), &package::digest(&manifest), bounds, &osm)?;
    Guard::exempt("catalog");
    eprintln!("Writing the route catalog");
    let report = catalog::write(&routing, osm, france)?;

    let cells = grid::cells(bounds);
    Guard::exempt("blocks");
    crate::blocks::publish(&routing, &cells, &request.output.join("blocks")).map_err(|e| e.to_string())?;
    let catalog = fs::read(routing.join(catalog::FILE)).map_err(|e| e.to_string())?;
    let catalog: Value = serde_json::from_slice(&catalog).map_err(|e| e.to_string())?;
    let ids: Vec<&str> = cells.iter().map(|(id, _)| id.as_str()).collect();
    let routes = request.output.join("routes");
    fs::create_dir(&routes).map_err(|e| format!("{}: {e}", routes.display()))?;
    for (cell, document) in grid::route_tiles(&catalog, &ids)? {
        let path = routes.join(format!("{cell}.json"));
        let mut bytes = serde_json::to_vec(&crate::sort_keys(document)).expect("JSON values serialize");
        bytes.push(b'\n');
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    fs::write(&request.metrics, report.to_json().to_string()).map_err(|e| e.to_string())
}

/// Writes the package objects and `manifest.json` into the empty directory `output`. Returns the
/// bytes of the manifest.
fn write_package(
    graph: &Graph,
    topology: Topology,
    costs: Costs,
    (region, bounds): (String, [f64; 4]),
    identities: Vec<String>,
    attribution: String,
    output: &Path,
) -> Result<Vec<u8>, String> {
    let mut writer = Writer::create(output).map_err(|e| e.to_string())?;
    let manifest =
        crate::package(graph, topology, costs, (region, bounds), identities, attribution, |bytes| writer.write(bytes))?;
    // Only the bytes outlive this statement, not the decoded manifest.
    let manifest = serde_json::to_vec(&manifest).map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    fs::write(output.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    Ok(manifest)
}
