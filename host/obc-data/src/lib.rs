//! The data registry: every external source a bake uses, every region, and the pins of an
//! environment; and the store and the fetchers that fill it. `specs/obc-data.md` is the contract
//! for the files this crate reads and writes.

pub mod date;
pub mod fetch;
pub mod regions;
pub mod sources;
pub mod store;

use std::path::{Path, PathBuf};

/// The repository root above `start`: the first directory that holds `data/sources.toml`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join("data/sources.toml").is_file()).map(Path::to_path_buf)
}

/// Lowercase kebab-case: the form of a source id, an environment name and each segment of a
/// region id.
pub fn is_kebab(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && !text.ends_with('-')
        && !text.contains("--")
        && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
