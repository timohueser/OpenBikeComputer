//! What the app has put on this disk, and how to get it back.
//!
//! This exists because the numbers are large enough to matter: a country `.pbf`
//! is hundreds of megabytes, the land-polygon dataset is ~2.3 GB once unpacked,
//! and both are invisible caches the user never chose. #906 asks for "a visible
//! cache location and a working clear-cache action" for exactly that reason, so
//! each place reports a real path (which the user can open) and a real size.
//!
//! The maps folder is listed alongside them and is deliberately **not**
//! clearable: it holds the artifacts the user asked for. Sizes are what makes the
//! difference legible — the caches are the big ones, and they are the ones with
//! a button.

use serde::Serialize;

#[derive(Serialize)]
pub struct Place {
    /// Stable id — what `storage_clear` takes.
    pub id: &'static str,
    pub label: &'static str,
    /// One line on what it is and what deleting it costs.
    pub note: &'static str,
    pub path: String,
    pub bytes: u64,
    pub files: usize,
    pub clearable: bool,
}

pub fn places(maps_dir: &std::path::Path) -> Vec<Place> {
    let pbf = crate::paths::pbf_cache();
    let land = crate::paths::land_cache();
    let index = crate::paths::geofabrik_cache();
    vec![
        describe("pbf", "OpenStreetMap extracts", "Re-downloaded per region on the next build.", &pbf, true),
        describe("land", "Land-polygon dataset", "Shared by every build; ~950 MB to download again.", &land, true),
        describe("index", "Region index", "The Geofabrik download index; refetched on the next launch.", &index, true),
        describe("maps", "Built maps", "Your own output — never cleared from here.", maps_dir, false),
    ]
}

fn describe(
    id: &'static str,
    label: &'static str,
    note: &'static str,
    path: &std::path::Path,
    clearable: bool,
) -> Place {
    let (bytes, files) = crate::paths::dir_size(path);
    Place { id, label, note, path: path.display().to_string(), bytes, files, clearable }
}

/// Delete a cache. Returns the freed byte count.
///
/// Refuses anything not in the table above, and refuses `maps` explicitly: this
/// is a command reachable from a webview, and "delete a directory the caller
/// names" is not a thing it should ever be able to ask for.
pub fn clear(id: &str) -> Result<u64, String> {
    let dir = match id {
        "pbf" => crate::paths::pbf_cache(),
        "land" => crate::paths::land_cache(),
        "index" => crate::paths::geofabrik_cache(),
        "maps" => return Err("built maps are yours — delete them in the file manager".into()),
        other => return Err(format!("unknown storage location: {other}")),
    };
    let (bytes, _) = crate::paths::dir_size(&dir);
    if !dir.exists() {
        return Ok(0);
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("clear {}: {e}", dir.display()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cache_is_named_and_only_caches_are_clearable() {
        let places = places(&std::env::temp_dir());
        let ids: Vec<&str> = places.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["pbf", "land", "index", "maps"]);
        for p in &places {
            assert!(!p.path.is_empty(), "{} has no visible location", p.id);
            assert!(!p.note.is_empty(), "{} does not say what clearing it costs", p.id);
        }
        assert!(!places.iter().find(|p| p.id == "maps").expect("maps").clearable);
    }

    #[test]
    fn clearing_refuses_anything_that_is_not_a_named_cache() {
        assert!(clear("maps").is_err());
        assert!(clear("/etc").is_err());
        assert!(clear("").is_err());
    }
}
