//! The OSM steps of `obc data`: for the device maps, the extract of the region cut into its source
//! leaves with native Osmium extraction ([`crate::split`]); for the planner, the extract cut to a box.

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
        std::fs::hard_link(path, &target)
            .or_else(|_| std::fs::copy(path, &target).map(drop))
            .map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(())
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
    let extract = one_file(request, &osmium)?;
    let osm = request.output.join("osm");
    std::fs::create_dir(&osm).map_err(|e| format!("{}: {e}", osm.display()))?;
    // The halves and the Osmium configs go beside the output: they are not part of the layer.
    crate::split(&osmium, extract, &leaves, &request.output.with_file_name("extract"), &osm)
}

/// The option `bbox` is west, south, east, north in degrees. The step reads the one file of its
/// inputs and writes `osm.pbf`, its cut by [`OsmiumRunner::cut`]. Its metrics name the Osmium version.
pub fn cut(request: &Request) -> Result<(), String> {
    let osmium = OsmiumRunner::default();
    let bbox = request.options["bbox"].as_array().and_then(|bbox| bbox.iter().map(|value| value.as_f64()).collect());
    let bbox: Vec<f64> = bbox.ok_or("option `bbox` is not a list of numbers")?;
    let bbox = bbox.try_into().map_err(|_| "option `bbox` does not have four numbers")?;
    osmium.cut(one_file(request, &osmium)?, &request.output.join("osm.pbf"), bbox)
}

/// The one file of the inputs of `request`. It writes the metrics: the Osmium version.
fn one_file<'a>(request: &'a Request, osmium: &OsmiumRunner) -> Result<&'a PathBuf, String> {
    let files: Vec<&PathBuf> =
        request.snapshots.values().chain(request.layers.values()).flat_map(|files| files.values()).collect();
    let [file] = files[..] else {
        return Err(format!("the step reads {} files, not one", files.len()));
    };
    let metrics = serde_json::json!({"osmium": osmium.version()?});
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| format!("{}: {e}", request.metrics.display()))?;
    Ok(file)
}
