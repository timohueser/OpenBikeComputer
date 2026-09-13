//! Saved folder rides are immutable startup inputs for a new card, never runtime stores.

use obc_host_core::FlatRideStore;
use std::path::Path;

pub fn import(directory: &Path, rides: &mut FlatRideStore) -> Result<(), String> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("ride inputs {}: {error}", directory.display())),
    };
    let mut files = entries
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    files.sort();
    for path in files {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
        if !name
            .strip_prefix("ride-")
            .and_then(|name| name.strip_suffix(".obcr"))
            .is_some_and(|digits| digits.parse::<u64>().is_ok())
        {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        rides.import(&bytes).map_err(|error| format!("import {}: {error}", path.display()))?;
    }
    Ok(())
}
