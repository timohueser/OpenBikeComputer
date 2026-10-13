//! The OSM step of the device maps in `obc data`: the extract of the region, cut into its source
//! leaves with native Osmium extraction ([`crate::split`]).

use std::collections::BTreeSet;
use std::path::PathBuf;

use obc_data::engine::Request;

use crate::{LeafId, OsmiumRunner};

/// The path in the layer of the OSM of a leaf.
pub fn leaf_pbf(leaf: LeafId) -> String {
    format!("osm/{}", leaf.file_name())
}

/// A single area's raw inputs, kept together so repeated sources retain separate request receipts.
pub fn area(request: &Request) -> Result<(), String> {
    for (source, output) in [("geofabrik-extracts", "source.osm.pbf"), ("geofabrik-poly", "source.poly")] {
        let files = request.snapshots.get(source).ok_or_else(|| format!("missing {source} input"))?;
        let [path] = files.values().collect::<Vec<_>>()[..] else {
            return Err(format!("{source} input has {} files, not one", files.len()));
        };
        let target = request.output.join(output);
        obc_data::target::link(path, &target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(())
}

/// The OSM of the planet on the day of its newest diff: the one file of `osm-planet` with the
/// daily diffs of `osm-replication` applied, and the outline of the option `poly`. The step writes
/// what [`area`] writes. Its metrics name the Osmium version.
pub fn planet(request: &Request) -> Result<(), String> {
    let osmium = OsmiumRunner::default();
    let files = |source: &str| request.snapshots.get(source).ok_or_else(|| format!("missing {source} input"));
    let planet: Vec<&PathBuf> = files("osm-planet")?.values().collect();
    let [base] = planet[..] else { return Err(format!("the osm-planet input has {} files, not one", planet.len())) };
    // A diff is named by its zero-padded sequence, so the names are in the order of the days.
    let diffs = files("osm-replication")?.iter().filter(|(name, _)| name.ends_with(".osc.gz"));
    let diffs: Vec<&PathBuf> = diffs.map(|(_, path)| path).collect();
    let metrics = serde_json::json!({"osmium": osmium.version()?});
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| format!("{}: {e}", request.metrics.display()))?;
    let output = request.output.join("source.osm.pbf");
    match diffs.is_empty() {
        true => obc_data::target::link(base, &output).map_err(|e| format!("{}: {e}", output.display()))?,
        false => osmium.apply_changes(base, &diffs, &output)?,
    }
    let poly = request.options["poly"].as_str().ok_or("option `poly` is not text")?;
    std::fs::write(request.output.join("source.poly"), poly).map_err(|e| format!("{}: {e}", request.output.display()))
}

/// The option `leaves` names each leaf as `[i, j]`. The step reads the one file of its snapshots,
/// and writes [`leaf_pbf`] for each leaf. Its metrics name the Osmium version.
pub fn osm(request: &Request) -> Result<(), String> {
    let osmium = OsmiumRunner::default();
    let leaves = request.options["leaves"].as_array().ok_or("option `leaves` is not a list")?;
    let leaves = leaves.iter().map(|leaf| match leaf.as_array().map(Vec::as_slice) {
        Some([i, j]) => Some(LeafId { i: i.as_i64()?, j: j.as_i64()? }),
        _ => None,
    });
    let leaves = leaves.collect::<Option<BTreeSet<_>>>().ok_or("a leaf is not [i, j]")?;
    let files: Vec<&PathBuf> =
        request.snapshots.values().chain(request.layers.values()).flat_map(|files| files.values()).collect();
    let [extract] = files[..] else {
        return Err(format!("the step reads {} files, not one", files.len()));
    };
    let metrics = serde_json::json!({"osmium": osmium.version()?});
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| format!("{}: {e}", request.metrics.display()))?;
    let osm = request.output.join("osm");
    std::fs::create_dir(&osm).map_err(|e| format!("{}: {e}", osm.display()))?;
    // The halves and the Osmium configs go beside the output: they are not part of the layer.
    crate::split(&osmium, extract, &leaves, &request.output.with_file_name("extract"), &osm)
}
