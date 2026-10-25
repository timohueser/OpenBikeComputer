//! Decode OSM objects without retaining nodes or applying search rules.

use osmpbfreader::{blobs::result_blob_into_iter, OsmObj, OsmPbfReader};
use rayon::prelude::*;
use std::{fs::File, path::Path};

/// Map each object of `path` with `map` on every core, and hand the results to `accept` in file
/// order. A chunk of blobs decodes at a time, so memory stays bounded by the chunk.
pub fn read<T: Send>(
    path: &Path,
    map: impl Fn(OsmObj) -> Option<T> + Sync,
    mut accept: impl FnMut(T),
) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = OsmPbfReader::new(File::open(path)?);
    let mut blobs = reader.blobs();
    let chunk = 2 * rayon::current_num_threads();
    loop {
        let raw: Vec<_> = blobs.by_ref().take(chunk).collect();
        if raw.is_empty() {
            return Ok(());
        }
        let decoded: Vec<osmpbfreader::Result<Vec<T>>> = raw
            .into_par_iter()
            .map(|blob| result_blob_into_iter(blob).filter_map(|object| object.map(&map).transpose()).collect())
            .collect();
        for objects in decoded {
            objects?.into_iter().for_each(&mut accept);
        }
    }
}
