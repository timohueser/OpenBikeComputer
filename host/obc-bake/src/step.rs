//! The OSM step of the device maps in `obc data`: the extract of the region, cut into its source
//! leaves with the Osmium extract of the planet bake.

use std::path::PathBuf;

use obc_data::engine::Request;
use obc_pack::grid::id_width;
use obc_pack::progress::Progress;

use crate::planet::{ExtractRequest, LeafId, OsmiumRunner, ShardRunner, SOURCE_LEAF_LOG2};

/// The path in the layer of the OSM of a leaf.
pub fn leaf_pbf(leaf: LeafId) -> String {
    format!("osm/{}", name(leaf))
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

fn name(leaf: LeafId) -> String {
    let width = id_width(SOURCE_LEAF_LOG2);
    format!("{:0width$}-{:0width$}.osm.pbf", leaf.i, leaf.j)
}

/// The option `leaves` names each leaf as `[i, j]`. The step reads the one file of its snapshots,
/// and writes [`leaf_pbf`] for each leaf. Its metrics name the Osmium version.
pub fn osm(request: &Request) -> Result<(), String> {
    let osmium = OsmiumRunner::default();
    if osmium.identity()? != request.options["osmium"] {
        return Err("prepared Osmium changed; prepare a new plan".into());
    }
    let leaves = request.options["leaves"].as_array().ok_or("option `leaves` is not a list")?;
    let leaves = leaves.iter().map(|leaf| match leaf.as_array().map(Vec::as_slice) {
        Some([i, j]) => Some(LeafId { i: i.as_i64()?, j: j.as_i64()? }),
        _ => None,
    });
    let leaves = leaves.collect::<Option<Vec<_>>>().ok_or("a leaf is not [i, j]")?;
    let files: Vec<&PathBuf> =
        request.snapshots.values().chain(request.layers.values()).flat_map(|files| files.values()).collect();
    let [extract] = files[..] else {
        return Err(format!("the step reads {} files, not one", files.len()));
    };
    let requests: Vec<ExtractRequest> =
        leaves.iter().map(|&leaf| ExtractRequest { output: name(leaf), bbox: leaf.extract_bbox() }).collect();
    // Osmium writes its config beside the extracts, which is not part of the layer.
    let dir = request.output.with_file_name("extract");
    let metrics = serde_json::json!({"osmium": osmium.version()?});
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| format!("{}: {e}", request.metrics.display()))?;
    osmium.split(extract, &dir, &requests, &Progress::silent())?;
    let osm = request.output.join("osm");
    std::fs::create_dir(&osm).map_err(|e| format!("{}: {e}", osm.display()))?;
    for leaf in leaves {
        let path = request.output.join(leaf_pbf(leaf));
        std::fs::rename(dir.join(name(leaf)), &path).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if osmium.identity()? != request.options["osmium"] {
        return Err("prepared Osmium changed during extraction".into());
    }
    Ok(())
}
