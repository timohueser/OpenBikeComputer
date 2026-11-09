//! PROTOTYPE (never merges): builds a routing package from one PBF without terrain, for the
//! hierarchical search prototype. Usage: PBF WEST SOUTH EAST NORTH OUT PROFILE[,PROFILE...]
use planner_router::{directory::Writer, model::Profile};
use std::path::PathBuf;

fn main() -> Result<(), String> {
    let a: Vec<String> = std::env::args().collect();
    let bounds = [a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap(), a[5].parse().unwrap()];
    let out = PathBuf::from(&a[6]);
    let names: Vec<&str> = a[7].split(',').collect();
    let profiles: Vec<Profile> =
        Profile::presets().into_iter().filter(|p| names.contains(&"all") || names.contains(&p.name.as_str())).collect();
    let t = std::time::Instant::now();
    let mut graph = planner_router_build::osm::import(&[PathBuf::from(&a[1])], bounds)?;
    eprintln!("import {:?}: {} roads, {} points", t.elapsed(), graph.roads.len(), graph.points.len());
    planner_router_build::layout::spatial_order(&mut graph)?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut writer = Writer::create(&out).map_err(|e| e.to_string())?;
    let manifest = planner_router_build::prepare(
        &graph,
        "prototype".into(),
        bounds,
        &profiles,
        vec![],
        "OpenStreetMap".into(),
        |b| writer.write(b),
    )?;
    writer.finish().map_err(|e| e.to_string())?;
    std::fs::write(out.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).map_err(|e| e.to_string())?;
    eprintln!("done {:?}", t.elapsed());
    Ok(())
}
