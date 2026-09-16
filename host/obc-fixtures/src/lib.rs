//! Stable access to external developer fixtures from host-side tests.
//!
//! The registry owns acquisition and integrity; this crate owns the only path
//! convention Rust consumers need to know. It deliberately has no dependencies.

use std::path::{Path, PathBuf};

/// The logical package view populated by `obc fixtures sync`.
#[must_use]
pub fn root() -> PathBuf {
    if let Some(root) = std::env::var_os("OBC_FIXTURE_ROOT") {
        return PathBuf::from(root);
    }
    if let Some(cache) = std::env::var_os("OBC_FIXTURE_CACHE") {
        return PathBuf::from(cache).join("by-id");
    }
    if let Some(cache) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(cache).join("openbikecomputer/fixtures/by-id");
    }
    let home = std::env::var_os("HOME").unwrap_or_else(|| {
        panic!("OBC fixtures: HOME is unset; set OBC_FIXTURE_ROOT to the registry's by-id directory")
    });
    PathBuf::from(home).join(".cache/openbikecomputer/fixtures/by-id")
}

/// Resolve a file in a logical package, or fail with the command that syncs it.
///
/// Fixtures are never downloaded by a test. A package that is not synced is a
/// setup error, and the panic names the exact `obc fixtures sync` command.
#[must_use]
pub fn file(package: &str, relative: impl AsRef<Path>) -> PathBuf {
    file_in("test", package, relative)
}

/// [`file`] for a package outside the `test` profile; `profile` is the one that provides it.
#[must_use]
pub fn file_in(profile: &str, package: &str, relative: impl AsRef<Path>) -> PathBuf {
    assert_id(package);
    let relative = relative.as_ref();
    assert!(
        !relative.is_absolute() && relative.components().all(|part| matches!(part, std::path::Component::Normal(_))),
        "OBC fixtures: relative paths may not escape a package"
    );
    let path = root().join(package).join(relative);
    assert!(path.is_file(), "OBC fixture is missing: {}. Run `obc fixtures sync {profile}`.", path.display());
    path
}

/// Read a fixture file, with the same missing-package failure as [`file`].
#[must_use]
pub fn read(package: &str, relative: impl AsRef<Path>) -> Vec<u8> {
    let path = file(package, relative);
    std::fs::read(&path).unwrap_or_else(|error| panic!("OBC fixture became unreadable at {}: {error}", path.display()))
}

fn assert_id(package: &str) {
    assert!(
        !package.is_empty()
            && package
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')),
        "OBC fixtures: invalid package id {package:?}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_an_escaping_relative_path() {
        let result = std::panic::catch_unwind(|| file("sample", "../escape"));
        assert!(result.is_err());
    }
}
