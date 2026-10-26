//! The step `planner/trailview-features` of `obc data`: `trailview-features.pmtiles` of a region from
//! the OSM of `planner/osm` and the master terrain of the grid of `planner/terrain`, by the kind
//! table and the road classes of its options.

use obc_data::engine::Request;

use crate::bake::{bake, Options};
use crate::{kinds, terrain};

pub fn step(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let kinds = kinds::validate(&options["kinds"])?;
    let (class_names, classes) = kinds::classes(&options["roads"])?;
    let bounds_json = options["bounds"].as_array().filter(|b| b.len() == 4).ok_or("option `bounds` is not four numbers")?;
    let bounds: Vec<f64> = bounds_json.iter().filter_map(|v| v.as_f64()).collect();
    let bounds: [f64; 4] = bounds.try_into().map_err(|_| "option `bounds` is not four numbers")?;
    let attribution = options["attribution"].as_str().ok_or("option `attribution` is not a string")?.to_string();
    let options = Options { kinds, bounds, bounds_json: bounds_json.clone(), attribution, classes, class_names };
    let layer = |name: &str| request.layers.get(name).ok_or(format!("the step has no input `{name}`"));
    let [pbf] = layer("planner/osm")?.values().collect::<Vec<_>>()[..] else {
        return Err("`planner/osm` is not one file".into());
    };
    let terrain = terrain::Tiles::grid(layer("planner/terrain/grid")?)?;
    let parent = request.output.parent().ok_or("the output has no directory")?;
    let work = tempfile::Builder::new().prefix(".trailview-osm-").tempdir_in(parent).map_err(|e| e.to_string())?;
    // Osmium finds the format of a file by its name; a store object has none.
    let osm = work.path().join("input.osm.pbf");
    std::os::unix::fs::symlink(pbf, &osm).map_err(|e| format!("{}: {e}", osm.display()))?;
    let output = request.output.join("trailview-features.pmtiles");
    let metrics = bake(&options, &osm, &terrain, &output, work.path(), free_memory(request.memory_bytes))?;
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| e.to_string())
}

/// The bytes of memory that the step may take: what is free past a quarter of the machine's memory, as
/// other work shares the machine and its memory grows while the step runs, and no more than `obc
/// data` reserves for it. None free and nothing known are both 0: the road jobs then run one at a time.
fn free_memory(reserved: Option<u64>) -> usize {
    let meminfo = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |key: &str| -> Option<u64> {
        let line = meminfo.lines().find(|l| l.starts_with(key))?;
        Some(line.split_whitespace().nth(1)?.parse::<u64>().ok()? << 10)
    };
    let free = match (field("MemAvailable:"), field("MemTotal:")) {
        (Some(available), Some(total)) => available.saturating_sub(total / 4),
        _ => 0,
    };
    free.min(reserved.unwrap_or(u64::MAX)) as usize
}
