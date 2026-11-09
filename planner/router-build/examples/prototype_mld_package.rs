//! PROTOTYPE (never merges): builds a routing package from one PBF without terrain, for the
//! hierarchical search prototype. Usage: PBF WEST SOUTH EAST NORTH OUT PROFILE[,PROFILE...]
use planner_router::{directory::Writer, model::Profile};
use std::path::PathBuf;

#[cfg(any())]
#[global_allocator]
static MIMALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

extern "C" {
    fn malloc_trim(pad: usize) -> i32;
}

/// Releases freed heap pages, then returns the resident GB; `reset` restarts the peak.
fn live(reset: bool) -> f64 {
    unsafe { malloc_trim(0) };
    if reset {
        std::fs::write("/proc/self/clear_refs", "5").unwrap();
    }
    planner_router_build::osm::rss()
}

fn peak() -> f64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse::<f64>().unwrap() / 1e6
}

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
    eprintln!("MEM after import: rss {:.2} GB, live after trim {:.2} GB, import peak {:.2} GB", planner_router_build::osm::rss(), live(false), peak());
    {
        let g = &graph;
        let roads = g.roads.capacity() * std::mem::size_of::<planner_router::model::Road>();
        let shapes: usize = g.roads.iter().map(|r| r.shape.capacity() * 12).sum();
        let other = g.points.capacity() * 12 + g.node_ids.capacity() * 8 + g.node_access.capacity()
            + (g.forbidden.capacity() + g.forbidden_foot.capacity()) * 8;
        eprintln!("MEM graph analytic: road structs {:.2} GB ({} B each), shapes {:.2} GB, other {:.2} GB", roads as f64 / 1e9, std::mem::size_of::<planner_router::model::Road>(), shapes as f64 / 1e9, other as f64 / 1e9);
    }
    live(true);
    planner_router_build::layout::spatial_order(&mut graph)?;
    eprintln!("MEM after spatial order: rss {:.2} GB", planner_router_build::osm::rss());
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
    eprintln!("MEM after prepare: peak during spatial order and prepare {:.2} GB", peak());
    let osm = std::mem::take(&mut graph.osm);
    let with = live(false);
    drop(osm);
    eprintln!("MEM source data {:.2} GB, graph and the rest {:.2} GB", with - live(false), live(false));
    writer.finish().map_err(|e| e.to_string())?;
    std::fs::write(out.join("manifest.json"), serde_json::to_vec(&manifest).unwrap()).map_err(|e| e.to_string())?;
    eprintln!("done {:?}", t.elapsed());
    Ok(())
}
