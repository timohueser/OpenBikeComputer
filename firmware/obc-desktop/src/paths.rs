//! Where the app keeps things.
//!
//! Two decisions worth stating, because both are visible to the user:
//!
//! * **The cache is the *shared* one, `~/.cache/obcm`** (overridable with
//!   `OBCM_CACHE_DIR`, exactly as `packer/web_builder/paths.py` reads it) — not a
//!   per-app directory under `~/Library/Caches`. A `.pbf` is hundreds of megabytes
//!   and the land-polygon dataset is over two gigabytes; a developer who has
//!   already downloaded Switzerland from the CLI should not download it again
//!   because they opened the app. `obc-pack`'s own land cache is anchored here
//!   too, so a private app cache would have split the two halves of one dataset.
//! * **Built maps go somewhere the user can find them.** The whole reason the
//!   desktop tier exists is that it has a real filesystem (#894); a build that
//!   landed in an opaque app-support directory would be a download manager with
//!   extra steps. `Documents/OpenBikeComputer` is a folder a person can back up,
//!   point Finder at, and copy to an SD card.

use std::path::PathBuf;

/// Shared with the CLI and the dev server — see the module docs.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("OBCM_CACHE_DIR") {
        return PathBuf::from(dir);
    }
    home().join(".cache/obcm")
}

/// Downloaded Geofabrik `.pbf` extracts, keyed by region id.
pub fn pbf_cache() -> PathBuf {
    cache_dir().join("pbf")
}

/// The Geofabrik download index (raw + simplified).
pub fn geofabrik_cache() -> PathBuf {
    cache_dir().join("geofabrik")
}

/// `obc-pack`'s land-polygon dataset. Not written by this crate — reported and
/// cleared by it, because at ~2.3 GB unpacked it is by far the largest thing the
/// app puts on someone's disk and "where did my space go" must have an answer.
pub fn land_cache() -> PathBuf {
    cache_dir().join("land")
}

/// The visible output folder for built maps.
pub fn maps_dir(documents: Option<PathBuf>) -> PathBuf {
    documents.unwrap_or_else(home).join("OpenBikeComputer")
}

fn home() -> PathBuf {
    #[cfg(windows)]
    let var = "USERPROFILE";
    #[cfg(not(windows))]
    let var = "HOME";
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(std::env::temp_dir)
}

/// A filesystem-friendly `.obcm` basename, mirroring the dev server's
/// `_sanitize_output_name`: the name comes from a text field, and it becomes a
/// real path here rather than a URL.
pub fn sanitize_output_name(name: &str) -> String {
    let base = name.trim().rsplit(['/', '\\']).next().unwrap_or("");
    let mut cleaned: String =
        base.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')).collect();
    cleaned = cleaned.trim().to_string();
    if cleaned.is_empty() || cleaned == ".obcm" {
        cleaned = "output.obcm".into();
    }
    if !cleaned.ends_with(".obcm") {
        cleaned.push_str(".obcm");
    }
    cleaned
}

/// `dir/name`, or `dir/name-2`, `dir/name-3`… if that is taken.
///
/// Overwriting is the wrong default here and the dev server never had to decide:
/// its builds went into a per-job directory behind a download link. A desktop
/// build lands in the user's own folder, where the previous `mymap.obcm` may be
/// the one already copied onto a card.
pub fn unique_in(dir: &std::path::Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = name.rsplit_once('.').unwrap_or((name, "obcm"));
    for n in 2..10_000 {
        let candidate = dir.join(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
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
    fn output_names_become_safe_basenames() {
        assert_eq!(sanitize_output_name("alps"), "alps.obcm");
        assert_eq!(sanitize_output_name("  my map.obcm "), "my map.obcm");
        assert_eq!(sanitize_output_name("../../etc/passwd"), "passwd.obcm");
        assert_eq!(sanitize_output_name(""), "output.obcm");
        assert_eq!(sanitize_output_name("/"), "output.obcm");
        assert_eq!(sanitize_output_name("a/b\\c.obcm"), "c.obcm");
    }

    #[test]
    fn a_second_build_of_the_same_name_does_not_overwrite_the_first() {
        let dir = std::env::temp_dir().join(format!("obc-desktop-unique-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let first = unique_in(&dir, "map.obcm");
        assert_eq!(first.file_name().unwrap(), "map.obcm");
        std::fs::write(&first, b"x").expect("write");
        assert_eq!(unique_in(&dir, "map.obcm").file_name().unwrap(), "map-2.obcm");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
