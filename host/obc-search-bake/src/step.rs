//! The planner step `planner/search/dump` of `obc data`.

use obc_data::engine::Request;

/// The search dump of the OSM of `planner/osm`, with the country policy and grid of
/// `planner/search/policy`. The option `country` is the country of a place outside every
/// country polygon. The layer is `search.jsonl.zst`; its metrics are the report of [`crate::bake`].
pub fn step(request: &Request) -> Result<(), String> {
    let country = request.options["country"].as_str().ok_or("option `country` is not a string")?;
    let file = |layer: &str, path: &str| {
        request.layers.get(layer).and_then(|files| files.get(path)).ok_or(format!("layer `{layer}` has no {path}"))
    };
    let policy = file("planner/search/policy", "policy.json")?;
    let grid = file("planner/search/policy", "country_osm_grid.sql.gz")?;
    let osm = file("planner/osm", "osm.pbf")?;
    let report = crate::bake(osm, &request.output.join("search.jsonl.zst"), country, Some(grid), policy)
        .map_err(|e| e.to_string())?;
    std::fs::write(&request.metrics, report.to_string()).map_err(|e| e.to_string())
}
