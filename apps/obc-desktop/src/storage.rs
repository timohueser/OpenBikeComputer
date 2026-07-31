//! What the app has put on this disk, and how to get it back.
//!
//! What the desktop app owns on disk. Catalog cells are not retained after an
//! assembly today; user maps and the ride archive remain deliberately non-clearable.
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
    vec![
        describe(
            "maps",
            "Maps",
            "Assembled maps, ready to send to the device. Deleting one only removes it from this \
             computer.",
            maps_dir,
            false,
        ),
        describe(
            "rides",
            "Ride archive",
            "The device's own recordings of your pulled rides — what the GPX files and the ride \
             list are rebuilt from. Don't delete these: the app can only get a ride back if it is \
             still on the device, and pulling it here is what lets the device delete its copy.",
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

/// Delete a named cache. There are no clearable caches until the published-cell
/// cache lands, so only the explicit user-data refusals exist today.
pub fn clear(id: &str) -> Result<u64, String> {
    match id {
        "maps" => Err("assembled maps are yours — delete them in the file manager".into()),
        "rides" => Err("the ride archive backs the ride library — it is not a cache".into()),
        other => Err(format!("unknown storage location: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cache_is_named_and_only_caches_are_clearable() {
        let places = places(&std::env::temp_dir(), &std::env::temp_dir());
        let ids: Vec<&str> = places.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["maps", "rides"]);
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
