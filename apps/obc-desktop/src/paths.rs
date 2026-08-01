//! Where the app keeps things.
//!
//! Two locations are visible to the user. **Assembled maps go somewhere the user
//! can find them.** The whole reason the desktop tier exists is that it has a real
//! filesystem (#894); a build that landed in an opaque app-support directory would
//! be a download manager with extra steps. `Documents/OpenBikeComputer` is a folder
//! a person can back up, point Finder at, and copy to an SD card. Pulled rides live
//! beside them in a relocatable GPX folder, with their durable archive in app data.

use std::path::PathBuf;

/// The visible output folder for built maps.
pub fn maps_dir(documents: Option<PathBuf>) -> PathBuf {
    documents.unwrap_or_else(home).join("OpenBikeComputer")
}

/// The **default** home of the managed ride library (E2 #912) — beside the maps, because one
/// folder is the whole answer to "where does this app put my things",
/// and `reveal_file`'s "under the maps folder" rule covers it without widening.
///
/// Only the default. A rider whose rides belong on an external drive relocates it, and the choice
/// is remembered in the app's config directory rather than here (`rides::configured`) — a folder
/// that named itself could not be found once it moved.
pub fn rides_dir(documents: Option<PathBuf>) -> PathBuf {
    maps_dir(documents).join("rides")
}

/// The internal ride **archive**: `index.json` plus the `.obcride` objects the ride library keeps
/// behind the visible GPX folder (`rides.rs`'s module docs own the why). App data, not user files —
/// and deliberately **not** relocatable: it does not follow the GPX folder when the rider moves it,
/// because a store that follows another folder around is two ways to lose it.
///
/// `app_data` is Tauri's per-app data directory; the fallback only exists so a platform that cannot
/// name one still gets a deterministic, private location rather than a panic.
pub fn ride_archive_dir(app_data: Option<PathBuf>) -> PathBuf {
    app_data.unwrap_or_else(|| home().join(".openbikecomputer")).join("ride-archive")
}

fn home() -> PathBuf {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// The same rule for any suffix: strip every path separator, keep only characters
/// that mean nothing to a shell or a filesystem, and force the extension.
///
/// Shared rather than duplicated because it is a *policy*, not a formatting
/// helper — it is the reason a name typed into the window cannot name a place.
/// `fallback` is the stem used when nothing survives the filter.
pub fn sanitize_basename(name: &str, ext: &str, fallback: &str) -> String {
    let base = name.trim().rsplit(['/', '\\']).next().unwrap_or("");
    let mut cleaned: String =
        base.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')).collect();
    cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned == ext {
        cleaned = fallback.to_string();
    }
    if !cleaned.ends_with(ext) {
        cleaned.push_str(ext);
    }
    cleaned
}

/// Total bytes and file count under `dir`, or `(0, 0)` if it isn't there.
pub fn dir_size(dir: &std::path::Path) -> (u64, usize) {
    let mut bytes = 0;
    let mut files = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let (b, f) = dir_size(&entry.path());
            bytes += b;
            files += f;
        } else {
            bytes += meta.len();
            files += 1;
        }
    }
    (bytes, files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_rule_forces_any_other_suffix() {
        assert_eq!(sanitize_basename("bikepacking", ".json", "style"), "bikepacking.json");
        assert_eq!(sanitize_basename("../../.ssh/config", ".json", "style"), "config.json");
        assert_eq!(sanitize_basename("", ".json", "style"), "style.json");
        assert_eq!(sanitize_basename(".json", ".json", "style"), "style.json");
        assert_eq!(sanitize_basename("obcm-style-default.json", ".json", "style"), "obcm-style-default.json");
    }
}
