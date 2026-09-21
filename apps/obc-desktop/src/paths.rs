//! Where the app keeps things.
//!
//! Two locations are visible to the user. Assembled maps go somewhere the user can find them: the
//! desktop tier exists because it has a real filesystem, and a build that landed in an opaque
//! app-support directory would be a download manager with extra steps. Pulled rides live beside
//! them in a relocatable GPX folder, with their durable archive in app data.

use std::path::PathBuf;

/// The visible output folder for built maps.
pub fn maps_dir(documents: Option<PathBuf>) -> PathBuf {
    documents.unwrap_or_else(home).join("OpenBikeComputer")
}

/// The default home of the managed ride library, beside the maps, so one folder answers where this
/// app puts things and `reveal_file`'s rule covers it without widening.
///
/// It is only the default. A rider whose rides belong on an external drive relocates it, and the
/// choice is remembered in the app's config directory: a folder that named itself could not be
/// found once it moved.
pub fn rides_dir(documents: Option<PathBuf>) -> PathBuf {
    maps_dir(documents).join("rides")
}

/// The internal ride archive: the index plus the ride objects the library keeps behind the visible
/// GPX folder. It is app data and not user files, and it does not follow the GPX folder when the
/// rider moves it, because a store that follows another folder around is two ways to lose it.
///
/// `app_data` is Tauri's per-app data directory. The fallback exists so a platform that cannot name
/// one still gets a deterministic, private location rather than a panic.
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

/// The same rule for any suffix: strip every path separator, keep only characters that mean nothing
/// to a shell or a filesystem, and force the extension. It is the reason a name typed into the
/// window cannot name a place. `fallback` is the stem used when nothing survives the filter.
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
