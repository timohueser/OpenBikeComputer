//! What the app has put on this disk, and how to get it back.
//!
//! This exists because the numbers are large enough to matter: a country `.pbf`
//! is hundreds of megabytes, the land-polygon dataset is ~2.3 GB once unpacked,
//! and both are invisible caches the user never chose. #906 asks for "a visible
//! cache location and a working clear-cache action" for exactly that reason, so
//! each place reports a real path (which the user can open) and a real size.
//!
//! The maps folder and the ride archive are listed alongside them and are
//! deliberately **not** clearable: one holds the artifacts the user asked for,
//! the other backs the ride library. Sizes are what makes the difference legible
//! — the caches are the big ones, and they are the ones with a button.
//!
//! Every `note` follows one pattern — *what this is → what deleting it costs* —
//! in plain words, because "cache", "extract" and "dataset" are this build
//! system's vocabulary, not the reader's.

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

pub fn places(maps_dir: &std::path::Path, ride_archive: &std::path::Path) -> Vec<Place> {
    let pbf = crate::paths::pbf_cache();
    let land = crate::paths::land_cache();
    let index = crate::paths::geofabrik_cache();
    vec![
        describe(
            "pbf",
            "OpenStreetMap data",
            "The raw OpenStreetMap data for regions you've built. Safe to delete — building that \
             region again will just download it again.",
            &pbf,
            true,
        ),
        describe(
            "land",
            "Coastline data",
            "Coastline data every map build needs. Safe to delete, but the next build re-downloads \
             all ~950 MB of it.",
            &land,
            true,
        ),
        describe(
            "index",
            "Region list",
            "The list of world regions you can pick from. Safe to delete — it is fetched again the \
             next time the app opens.",
            &index,
            true,
        ),
        describe(
            "maps",
            "Built maps",
            "Finished maps, ready to send to the device. Deleting one only removes it from this \
             computer.",
            maps_dir,
            false,
        ),
        describe(
            "rides",
            "Ride archive",
            "The device's own recordings of your pulled rides — what the GPX files and the ride \
             list are rebuilt from. Deleting one would make the app fetch that ride from the \
             device again.",
            ride_archive,
            false,
        ),
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
/// Refuses anything not in the table above, and refuses `maps` and `rides`
/// explicitly: this is a command reachable from a webview, and "delete a
/// directory the caller names" is not a thing it should ever be able to ask for.
pub fn clear(id: &str) -> Result<u64, String> {
    let dir = match id {
        "pbf" => crate::paths::pbf_cache(),
        "land" => crate::paths::land_cache(),
        "index" => crate::paths::geofabrik_cache(),
        "maps" => return Err("built maps are yours — delete them in the file manager".into()),
        "rides" => return Err("the ride archive backs the ride library — it is not a cache".into()),
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
        let places = places(&std::env::temp_dir(), &std::env::temp_dir());
        let ids: Vec<&str> = places.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["pbf", "land", "index", "maps", "rides"]);
        for p in &places {
            assert!(!p.path.is_empty(), "{} has no visible location", p.id);
            assert!(!p.note.is_empty(), "{} does not say what clearing it costs", p.id);
        }
        assert!(!places.iter().find(|p| p.id == "maps").expect("maps").clearable);
        assert!(!places.iter().find(|p| p.id == "rides").expect("rides").clearable);
    }

    #[test]
    fn notes_speak_the_reader_s_language_not_the_build_system_s() {
        for p in places(&std::env::temp_dir(), &std::env::temp_dir()) {
            for word in ["cache", "dataset", "extract", "index"] {
                assert!(!p.note.to_lowercase().contains(word), "{}'s note says \"{word}\": {}", p.id, p.note);
            }
        }
    }

    #[test]
    fn clearing_refuses_anything_that_is_not_a_named_cache() {
        assert!(clear("maps").is_err());
        assert!(clear("rides").is_err());
        assert!(clear("/etc").is_err());
        assert!(clear("").is_err());
    }
}
