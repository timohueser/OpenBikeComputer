//! PROTOTYPE for GitHub issue 3836. It never merges, and it has no tests.
//!
//! The question: where the `touring` router leaves a signed cycle route between two points that
//! lie on that route, what differs between the stretch the router took and the stretch of the
//! signed route, and which term of the cost paid for the difference?
//!
//! `tools/prototype_route_census/run.sh` runs it. It bakes one box in memory with the real import
//! and the real costs, writes the price list, and runs the census.
mod census;
mod grid;
mod panel;
mod prices;
mod report;
mod terms;
pub(crate) mod variant;

use crate::base::Topology;
use obc_dem::planner::{Recent, Terrain};
use planner_router::model::Point;
use std::path::PathBuf;
use std::time::Instant;

/// West, south, east, north of the Freiburg extract.
const BOUNDS: [f64; 4] = [7.50, 47.78, 8.24, 48.31];

struct Options {
    pbf: PathBuf,
    dem: Option<PathBuf>,
    out: PathBuf,
    step_m: f64,
    profiles: String,
    package: bool,
    grid: bool,
}

fn options() -> Result<Options, String> {
    let usage =
        "Usage: prototype_census --pbf FILE --out DIR [--dem DIR] [--step-km 5] [--profiles all] [--package] [--grid]";
    let mut options = Options {
        pbf: PathBuf::new(),
        dem: None,
        out: PathBuf::new(),
        step_m: 5000.0,
        profiles: "all".into(),
        package: false,
        grid: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(usage);
        match arg.as_str() {
            "--pbf" => options.pbf = value()?.into(),
            "--dem" => options.dem = Some(value()?.into()),
            "--out" => options.out = value()?.into(),
            "--step-km" => options.step_m = 1000.0 * value()?.parse::<f64>().map_err(|e| e.to_string())?,
            "--profiles" => options.profiles = value()?,
            "--package" => options.package = true,
            "--grid" => options.grid = true,
            _ => return Err(usage.into()),
        }
    }
    if options.pbf.as_os_str().is_empty() || options.out.as_os_str().is_empty() {
        return Err(usage.into());
    }
    Ok(options)
}

pub fn main() {
    if let Err(error) = run() {
        eprintln!("prototype_census: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let options = options()?;
    let write = |name: &str, text: String| {
        let path = options.out.join(name);
        std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!("Wrote {}", path.display());
        Ok::<_, String>(())
    };
    std::fs::create_dir_all(&options.out).map_err(|e| e.to_string())?;
    write("price_list.md", prices::table()?)?;

    let mut stages = Vec::new();
    let mut clock = Instant::now();
    let mut lap = |name: &str| {
        stages.push((name.to_string(), clock.elapsed().as_secs_f64()));
        eprintln!("Stage {name}: {:.1} s", clock.elapsed().as_secs_f64());
        clock = Instant::now();
    };
    let source = crate::osm::source(std::slice::from_ref(&options.pbf), BOUNDS)?;
    lap("source: the OSM objects and the facts of each way");
    let mut vias: Vec<i64> = source.rules.values().iter().map(|rule| rule.via).collect();
    vias.sort_unstable();
    vias.dedup();
    let graph = match &options.dem {
        Some(dem) => {
            let tiles = obc_dem::planner::tiles_in(dem, BOUNDS);
            if tiles.len() != obc_dem::planner::tiles(BOUNDS).len() {
                return Err(format!("{}: {} GLO-30 tiles, the box needs 4", dem.display(), tiles.len()));
            }
            let terrain = Terrain::open(&tiles, "Copernicus GLO-30", None, BOUNDS)?;
            let mut recent = Recent::default();
            let mut height = |point: Point| terrain.height(point.lat, point.lon, &mut recent);
            crate::osm::roads(source, Some(&mut height))?
        }
        None => crate::osm::roads(source, None)?,
    };
    crate::check_closures(&graph)?;
    let topology = Topology::new(&graph.roads, graph.points.len())?;
    variant::prepare(&graph);
    lap(if options.dem.is_some() {
        "roads: junctions, roads with GLO-30 heights, turn rules, topology"
    } else {
        "roads: junctions, roads WITHOUT terrain, turn rules, topology"
    });
    if options.grid {
        return grid::run(&graph, &vias, options.step_m, &options.out);
    }
    let names: Vec<&str> = options.profiles.split(',').collect();
    let profiles = crate::profiles(&names)?;
    crate::check(&graph, &profiles)?;
    let costs = crate::costs(&graph, &topology, &profiles)?;
    lap(&format!("costs: way bases, road pass and snap bits of {} profiles", profiles.len()));

    let touring = profiles.iter().position(|p| p.name == "touring").ok_or("the profiles lack touring")?;
    let (first, head) = (topology.first.values(), topology.head.values());
    let (totals, turns) = (costs.roads[touring].totals.values(), costs.roads[touring].turns.values());
    // The census of the question, and the same with the short relations, to show what the limit of 5 km leaves out.
    let variants = [(options.step_m, ""), (1000.0, "_from_1km")];
    let mut censuses = Vec::new();
    for (min_line_m, _) in variants {
        let mut net = census::Net::new(&graph, &profiles[touring], (first, head), (totals, turns));
        censuses.push(census::run(&mut net, &vias, (options.step_m, min_line_m))?);
    }
    lap("census, twice (not a stage of the bake)");

    let counts = vec![
        ("ways of the import".to_string(), graph.osm.ways.len().to_string()),
        ("junctions".to_string(), graph.points.len().to_string()),
        ("directed roads".to_string(), graph.roads.len().to_string()),
        ("turns (arcs road → road)".to_string(), head.len().to_string()),
        ("forbidden turns".to_string(), graph.forbidden.len().to_string()),
        (
            "roads that `touring` may use".to_string(),
            totals.iter().filter(|&&cost| cost != u32::MAX).count().to_string(),
        ),
    ];
    let profile_count = profiles.len();
    if options.package {
        let digest = |bytes: &[u8]| Ok(planner_router::package::digest(bytes));
        crate::package(&graph, topology, costs, ("prototype".into(), BOUNDS), vec![], String::new(), digest)?;
        lap(&format!("package: landmarks and pages of {profile_count} profiles, hashed and not written"));
    }
    for ((min_line_m, suffix), census) in variants.into_iter().zip(&censuses) {
        let facts = report::Facts {
            inputs: vec![
                format!("OSM: `{}`, box {BOUNDS:?}", options.pbf.display()),
                match &options.dem {
                    Some(dem) => format!("Terrain: Copernicus GLO-30 tiles in `{}`", dem.display()),
                    None => "Terrain: none".into(),
                },
                format!("Points about every {} km", options.step_m / 1000.0),
            ],
            stages: &stages,
            counts: counts.clone(),
            step_m: options.step_m,
            min_line_m,
            terrain: options.dem.is_some(),
        };
        write(&format!("census{suffix}.md"), report::census_md(census, &facts))?;
        write(&format!("splits{suffix}.csv"), report::splits_csv(census))?;
        write(&format!("relations{suffix}.csv"), report::relations_csv(census))?;
    }
    Ok(())
}
