//! The planner routing steps of `obc data`. Three work layers hold the bake up to the package, so
//! that a stop or a new profile list repeats only the steps after it: `planner/routing-source`
//! (the OSM objects with their facts), `planner/routing-roads` (the roads with their terrain and
//! their topology) and `planner/routing-costs` (the costs of each profile). `planner/routing`
//! writes the package of format 9 and of format 10, its overlay index, its route catalog, the routing
//! blocks and the routes of each grid cell. `planner/routing/cells/grid` publishes the files of
//! format 10.

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
use crate::tables::{self, Tables};
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

/// The option `cell_sizes`: the largest number of junctions in a cell of level 1, 2, ….
fn cell_sizes(options: &Value) -> Result<Vec<u32>, String> {
    let values = options["cell_sizes"].as_array().ok_or("option `cell_sizes` is not a list")?;
    let sizes = values
        .iter()
        .map(|value| value.as_u64().and_then(|size| u32::try_from(size).ok()))
        .collect::<Option<Vec<_>>>()
        .ok_or("option `cell_sizes` lists a value that is not a 32-bit count")?;
    crate::partition::check_sizes(&sizes)?;
    Ok(sizes)
}

/// The option `download_level`: the level of the download cells of format 10.
fn download_level(options: &Value) -> Result<u8, String> {
    crate::cells::download_level(options["download_level"].as_u64().ok_or("option `download_level` is not a number")?)
}

/// Starts the memory guard, and reads the page budget first, so that a bad value fails before the
/// work.
fn start() -> Result<Guard, String> {
    let guard = Guard::start(OWN_BYTES);
    let (budget, source) = crate::work::page_budget()?;
    eprintln!("Page budget: {} MiB, from {source}", budget >> 20);
    Ok(guard)
}

/// The folder of the layer of a work step in its output: the last part of the step name.
pub fn folder_name(step: &str) -> &str {
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
/// The options are `bounds`, `terrain_attribution`, the credit of GLO-30, and `cell_sizes`
/// (`partition::check_sizes`). The layer `roads/` holds the junctions and the roads in their final
/// order, the roads with their heights, the cells (`partition::Cells`), the forbidden turns, the
/// closure sets with those of the roads, the topology (`first`, `head`), and `roads.json`: the
/// warnings of the manifest and the identities of its inputs, the OSM files and then the terrain.
pub fn roads(request: &Request) -> Result<(), String> {
    let sizes = cell_sizes(&request.options)?;
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
    let mut graph = crate::osm::roads(source, &sizes, Some(&mut height))?;
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

/// The graph, the topology, the costs and the cost tables of a roads layer and a costs layer in the
/// folders `roads` and `costs`, without the source objects: the input of `cells::write` for a tool.
pub fn layers(roads: &Path, costs: &Path) -> Result<(Graph, Topology, Costs, Tables), String> {
    let read = |dir: &Path| {
        let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let paths = entries
            .map(|entry| entry.map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path())))
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        Layout::read(paths)
    };
    let roads = read(roads)?;
    let summary = read_json(&roads, "roads.json")?;
    let graph = Graph::open(&roads, Data::default(), strings(&summary["warnings"]))?;
    let topology = Topology { first: roads.open("first")?, head: roads.open("head")? };
    let layout = read(costs)?;
    let costs = Costs::open(&layout)?;
    let tables = Tables::open(&layout, graph.cells.levels(), costs.profiles.len())?;
    Ok((graph, topology, costs, tables))
}

/// The costs step. It reads the source and the roads layers and the options `bounds` and
/// `profiles`, the profile IDs or `all`. The layer `costs/` holds the costs of each profile
/// (`Costs::save`) and the cost tables of each level (`Tables::save`).
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
    Guard::stage("tables");
    eprintln!("Building the cost tables");
    let (first, head) = (topology.first.values(), topology.head.values());
    let tables = tables::build(&graph.cells, first, head, &costs.columns(), tables::COLUMNS_FROM)?;
    let mut layout = output(request, COSTS)?;
    costs.save(&mut layout)?;
    tables.save(&mut layout)?;
    layout.finish()
}

/// The package step. It reads the three work layers and the options `region`, `bounds`,
/// `countries`, the ISO codes of the countries of the region for the route catalog,
/// `attribution`, the credit of the OSM data, `cell_sizes` of the roads layer and `download_level`.
/// The layer:
///
/// - `routing/`: the package, `overlays.sqlite` and `route-catalog.json`;
/// - `cells/`: the objects, `package.json` and `directory.bin` of format 10 (`cells::write`);
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
    let (sizes, level) = (cell_sizes(options)?, download_level(options)?);
    let costs = Costs::open(&folder(request, COSTS)?)?;
    let france = catalog::check(&countries, &costs.profiles)?;
    let (mut graph, topology, identities) = graph(request)?;
    let routing = request.output.join("routing");
    fs::create_dir(&routing).map_err(|e| format!("{}: {e}", routing.display()))?;
    eprintln!("Writing the package of {} profiles", costs.profiles.len());
    let info = crate::cells::Info {
        region: region.clone(),
        bounds,
        source_sha256: identities.clone(),
        attribution: attribution.clone(),
    };
    let manifest = write_package(&graph, topology, costs, (region, bounds), identities, attribution, &routing)?;
    Guard::stage("cells");
    eprintln!("Writing the package of format 10");
    let costs = Costs::open(&folder(request, COSTS)?)?;
    let tables = Tables::open(&folder(request, COSTS)?, graph.cells.levels(), costs.profiles.len())?;
    let roads = folder(request, ROADS)?;
    let topology = Topology { first: roads.open("first")?, head: roads.open("head")? };
    let input = crate::cells::Input {
        graph: &graph,
        topology: &topology,
        costs: &costs,
        tables: &tables,
        sizes: &sizes,
        download_level: level,
        info,
        level: crate::cells::LEVEL,
    };
    let summary = crate::cells::write(&input, &request.output.join("cells"))?;
    eprintln!("Cells: {}", summary.to_json());
    drop((costs, tables, topology));
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

/// The grid step of format 10. It reads `cells/` of `planner/routing` and writes `objects/`, each
/// file under its SHA-256, and `index.json` with the files `routing/package.json`,
/// `routing/directory.bin` and `routing/objects/SHA256`, each with the transport `identity`.
pub fn cells_step(request: &Request) -> Result<(), String> {
    const STEP: &str = "planner/routing";
    let files = request.layers.get(STEP).ok_or("the step reads no planner/routing")?;
    let records = request.layer_files.get(STEP).ok_or("the request gives no receipts of planner/routing")?;
    let records: std::collections::HashMap<&str, _> = records.iter().map(|file| (file.path.as_str(), file)).collect();
    let objects = request.output.join("objects");
    fs::create_dir_all(&objects).map_err(|e| format!("{}: {e}", objects.display()))?;
    let mut index = serde_json::Map::new();
    for (path, source) in files {
        let Some(name) = path.strip_prefix("cells/") else { continue };
        let record = records.get(path.as_str()).ok_or(format!("no receipt of {path}"))?;
        let logical = match name {
            crate::cells::PACKAGE | crate::cells::DIRECTORY => format!("routing/{name}"),
            object if object == record.sha256 => format!("routing/objects/{object}"),
            _ => return Err(format!("{path} is not named by its SHA-256")),
        };
        let target = objects.join(&record.sha256);
        if !target.exists() && fs::hard_link(source, &target).is_err() {
            fs::copy(source, &target).map_err(|e| format!("{}: {e}", target.display()))?;
        }
        let transport = json!({"bytes": record.size, "sha256": record.sha256, "encoding": "identity"});
        index.insert(logical, json!({"bytes": record.size, "sha256": record.sha256, "transport": transport}));
    }
    for name in [crate::cells::PACKAGE, crate::cells::DIRECTORY] {
        if !index.contains_key(&format!("routing/{name}")) {
            return Err(format!("planner/routing has no cells/{name}"));
        }
    }
    let count = index.len();
    let document = crate::sort_keys(json!({"format": 1, "kind": "routing-cells", "files": index}));
    let mut bytes = serde_json::to_vec(&document).map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    fs::write(request.output.join("index.json"), bytes).map_err(|e| e.to_string())?;
    fs::write(&request.metrics, json!({"files": count}).to_string()).map_err(|e| e.to_string())
}
