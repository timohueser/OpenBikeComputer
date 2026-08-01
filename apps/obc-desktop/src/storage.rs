//! Storage owned by the desktop app and shown to the user.

use serde::Serialize;

#[derive(Serialize)]
pub struct Place {
    pub id: &'static str,
    pub label: &'static str,
    pub note: &'static str,
    pub path: String,
    pub bytes: u64,
    pub files: usize,
}

pub fn places(maps_dir: &std::path::Path, ride_archive: &std::path::Path) -> Vec<Place> {
    vec![
        describe(
            "maps",
            "Maps",
            "Assembled maps, ready to send to the device. Deleting one only removes it from this \
             computer.",
            maps_dir,
        ),
        describe(
            "rides",
            "Ride archive",
            "The device's own recordings of your pulled rides — what the GPX files and the ride \
             list are rebuilt from. Don't delete these: the app can only get a ride back if it is \
             still on the device, and pulling it here is what lets the device delete its copy.",
            ride_archive,
        ),
    ]
}

fn describe(id: &'static str, label: &'static str, note: &'static str, path: &std::path::Path) -> Place {
    let (bytes, files) = crate::paths::dir_size(path);
    Place { id, label, note, path: path.display().to_string(), bytes, files }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_storage_location_is_named_and_visible() {
        let places = places(&std::env::temp_dir(), &std::env::temp_dir());
        let ids: Vec<&str> = places.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["maps", "rides"]);
        for p in &places {
            assert!(!p.path.is_empty(), "{} has no visible location", p.id);
            assert!(!p.note.is_empty(), "{} does not say what clearing it costs", p.id);
        }
    }

    #[test]
    fn notes_speak_the_reader_s_language_not_the_build_system_s() {
        for p in places(&std::env::temp_dir(), &std::env::temp_dir()) {
            for word in ["cache", "dataset", "extract", "index"] {
                assert!(!p.note.to_lowercase().contains(word), "{}'s note says \"{word}\": {}", p.id, p.note);
            }
        }
    }
}
