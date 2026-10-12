//! The data registry: every external source a bake uses, every region, and the targets; the
//! store and the fetchers that fill it; the engine that builds layers and the releases of the
//! targets; and the `obc data` commands. `specs/obc-data.md` is the contract for the files this
//! crate reads and writes.

pub mod cli;
pub mod date;
pub mod engine;
pub mod fetch;
pub mod machines;
pub mod product;
pub mod r2;
pub mod regions;
pub mod release;
pub mod serve;
pub mod settings;
pub mod sources;
pub mod store;
pub mod target;

use std::path::{Path, PathBuf};

/// The repository root above `start`: the first directory that holds `data/sources.toml`.
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start.ancestors().find(|dir| dir.join("data/sources.toml").is_file()).map(Path::to_path_buf)
}

/// Lowercase kebab-case: the form of a source id, a target name and each segment of a region id.
pub fn is_kebab(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('-')
        && !text.ends_with('-')
        && !text.contains("--")
        && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The name of this machine in a release and a lock: `OBC_DATA_MACHINE`, or else its host name up
/// to the first dot.
pub fn machine() -> String {
    if let Some(name) = std::env::var("OBC_DATA_MACHINE").ok().filter(|name| !name.is_empty()) {
        return name;
    }
    let name = host_name().unwrap_or_default();
    let name = name.split('.').next().unwrap_or_default();
    if name.is_empty() {
        "unknown".into()
    } else {
        name.into()
    }
}

#[cfg(unix)]
fn host_name() -> Option<String> {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is writable for its length, and gethostname writes at most that many bytes.
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } != 0 {
        return None;
    }
    let end = buffer.iter().position(|&byte| byte == 0).unwrap_or(buffer.len());
    String::from_utf8(buffer[..end].to_vec()).ok()
}

#[cfg(not(unix))]
fn host_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}
