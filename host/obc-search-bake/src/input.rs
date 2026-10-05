//! Stream decoded OSM objects without retaining nodes or applying search rules.

use osmpbfreader::{OsmObj, OsmPbfReader};
use std::{fs::File, path::Path};

pub fn read(path: &Path, mut accept: impl FnMut(OsmObj)) -> Result<(), Box<dyn std::error::Error>> {
    let mut reader = OsmPbfReader::new(File::open(path)?);
    for object in reader.iter() {
        accept(object?);
    }
    Ok(())
}
