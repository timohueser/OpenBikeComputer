//! A routing package from OSM files: the build of the `route-build` command, and the planner
//! routing step of `obc data`, which adds the routing blocks and the routes of each grid cell.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use obc_data::engine::Request;
use obc_dem::planner::Terrain;
use route_engine::{directory::Writer, model::Profile, package};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{catalog, grid, overlays};

pub struct Build {
    /// OSM PBF files.
    pub inputs: Vec<PathBuf>,
    pub region: String,
    /// The country whose access defaults the import applies.
    pub country: String,
    /// West, south, east, north in degrees. Routes are exact within this clipped graph.
    pub bounds: [f64; 4],
    /// Profile ids, or `all`.
    pub profiles: Vec<String>,
    /// The ISO codes of the countries of the region. With them, the build also writes the route
    /// catalog.
    pub countries: Vec<String>,
}

/// Write the package, its overlay index and, with countries, its route catalog into the empty
/// directory `output`. `terrain` gives the heights of the roads. Returns the report of the catalog.
pub fn build(args: Build, terrain: Option<Terrain>, output: &Path) -> Result<Option<catalog::Report>, String> {
    let profiles: Vec<_> = Profile::presets()
        .into_iter()
        .filter(|p| args.profiles.iter().any(|name| name == "all" || name == &p.name))
        .collect();
    if profiles.is_empty()
        || args.profiles.iter().any(|name| name != "all" && !profiles.iter().any(|p| &p.name == name))
    {
        return Err("Unknown profile ID".into());
    }
    let france = (!args.countries.is_empty()).then(|| catalog::check(&args.countries, &profiles)).transpose()?;
    let mut identities = Vec::new();
    for path in &args.inputs {
        let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
        let mut hash = Sha256::new();
        let mut buffer = vec![0u8; 1024 * 1024];
        loop {
            let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        identities.push(format!("{:x}", hash.finalize()));
    }
    eprintln!("Importing {}", args.region);
    let mut graph = crate::osm::import(&args.inputs, args.bounds, &args.country)?;
    if let Some(mut terrain) = terrain {
        eprintln!("Sampling terrain");
        crate::terrain::apply(&mut graph, |point| terrain.height(point.lat, point.lon))?;
        identities.extend(terrain.identities.iter().cloned());
        graph.warnings.extend(terrain.attribution());
    }
    eprintln!("Preparing {} profiles for {} directed roads", profiles.len(), graph.roads.len());
    crate::layout::spatial_order(&mut graph)?;
    let mut writer = Writer::create(output).map_err(|e| e.to_string())?;
    // Only the bytes outlive this statement, not the decoded manifest.
    let manifest =
        serde_json::to_vec(&crate::prepare(&graph, args.region, args.bounds, &profiles, identities, |bytes| {
            writer.write(bytes)
        })?)
        .map_err(|e| e.to_string())?;
    writer.finish().map_err(|e| e.to_string())?;
    fs::write(output.join("manifest.json"), &manifest).map_err(|e| e.to_string())?;
    // The overlay index and the route catalog read only the source objects. The road graph is
    // freed here, so the catalog routers get its memory.
    let osm = std::mem::take(&mut graph.osm);
    drop(graph);
    eprintln!("Writing the overlay index");
    overlays::write(&output.join(overlays::FILE), &package::digest(&manifest), args.bounds, &osm)?;
    let Some(france) = france else { return Ok(None) };
    eprintln!("Writing the route catalog");
    catalog::write(output, osm, france).map(Some)
}

/// The planner routing step. It reads the OSM files of its input layers and the GLO-30 tiles of
/// its bounds. The options are `region`, `access` (the country of the access defaults), `bounds`,
/// `profiles` and `countries`, as in [`Build`]. The layer:
///
/// - `routing/`: the package, `overlays.sqlite` and `route-catalog.json`;
/// - `blocks/`: the routing blocks of the grid cells (`blocks::publish`);
/// - `routes/<cell>.json`: the routes of each grid cell.
///
/// Its metrics are the report of the route catalog.
pub fn step(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let text =
        |name: &str| options[name].as_str().map(str::to_string).ok_or(format!("option `{name}` is not a string"));
    let list = |name: &str| -> Result<Vec<String>, String> {
        let values = options[name].as_array().ok_or(format!("option `{name}` is not a list"))?;
        values
            .iter()
            .map(|value| value.as_str().map(str::to_string).ok_or(format!("option `{name}` lists a non-string")))
            .collect()
    };
    let bounds = obc_dem::step::bounds(options)?;
    let countries = list("countries")?;
    if countries.is_empty() {
        return Err("option `countries` is empty: the routes of each cell come from the route catalog".into());
    }
    let inputs = request.layers.values().flat_map(|files| files.values().cloned()).collect();
    let args = Build {
        inputs,
        region: text("region")?,
        country: text("access")?,
        bounds,
        profiles: list("profiles")?,
        countries,
    };
    let terrain = Terrain::open(&obc_dem::step::glo30(request, bounds), None, bounds)?;
    let routing = request.output.join("routing");
    fs::create_dir(&routing).map_err(|e| format!("{}: {e}", routing.display()))?;
    let report = build(args, Some(terrain), &routing)?.expect("a build with countries writes the route catalog");

    let cells = grid::cells(bounds);
    crate::blocks::publish(&routing, &cells, &request.output.join("blocks")).map_err(|e| e.to_string())?;
    let catalog = fs::read(routing.join(catalog::FILE)).map_err(|e| e.to_string())?;
    let catalog: Value = serde_json::from_slice(&catalog).map_err(|e| e.to_string())?;
    let ids: Vec<&str> = cells.iter().map(|(id, _)| id.as_str()).collect();
    let routes = request.output.join("routes");
    fs::create_dir(&routes).map_err(|e| format!("{}: {e}", routes.display()))?;
    for (cell, document) in grid::route_tiles(&catalog, &ids)? {
        let path = routes.join(format!("{cell}.json"));
        let bytes = serde_json::to_vec(&obc_data::engine::sorted(document)).expect("JSON values serialize");
        fs::write(&path, bytes).map_err(|e| e.to_string())?;
    }
    fs::write(&request.metrics, report.to_json().to_string()).map_err(|e| e.to_string())
}
