//! Build and install services from the published commit, then probe their data.

use std::path::Path;
use std::process::Command;

use obc_data::engine::release::Release;
use obc_data::engine::{Profile, Rust, Step};
use obc_data::store::Store;
use serde::Deserialize;

#[derive(Deserialize)]
struct Config {
    publication: Origins,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Origins {
    pub site_origin: String,
    pub api_origin: String,
    pub objects_origin: String,
}

pub(super) fn publication(root: &Path) -> Result<Origins, String> {
    let path = root.join("data/planner-runtime.toml");
    let config: Config =
        toml::from_str(&std::fs::read_to_string(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    for origin in [&config.publication.site_origin, &config.publication.api_origin, &config.publication.objects_origin]
    {
        let host = origin.strip_prefix("https://").ok_or("publication needs HTTPS origins")?;
        if host.is_empty() || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-:".contains(&byte)) {
            return Err(format!("publication origin `{origin}` has a path, credentials or unsafe characters"));
        }
    }
    Ok(config.publication)
}

/// Service source and dependency changes give the release a new identity.
pub(super) fn bind(index: &mut Step) {
    index.code.paths.extend(
        [
            "data/planner-runtime.toml",
            "tools/planner_publish.py",
            "tools/planner_downloads.py",
            "tools/planner_maps.py",
            "tools/planner_map_archive.py",
            "planner/search",
        ]
        .map(str::to_string),
    );
    index.code.crates = vec!["planner-service".into()];
    index.code.target = Some("x86_64-unknown-linux-gnu".into());
    index.code.rust = Some(Rust::Prepared { profile: Profile::Release });
    index.code.python_packages = Some("search-runtime".into());
}

pub(super) fn activate(root: &Path, release: &Release, store: &Store, commit: &str) -> Result<(), String> {
    let host = std::env::var("OBC_PLANNER_HOST").map_err(|_| "set OBC_PLANNER_HOST to USER@HOST before Live apply")?;
    let scratch = tempfile::tempdir_in(store.root()).map_err(|e| e.to_string())?;
    let manifest = scratch.path().join("release.json");
    std::fs::write(&manifest, super::catalog::descriptor(release, store)?).map_err(|e| e.to_string())?;
    let status = Command::new(obc_data::engine::python_executable(root)?)
        .args([
            "-m",
            "tools.planner_publish",
            "--host",
            &host,
            "--commit",
            commit,
            "--release",
            &release.id(),
            "--manifest",
        ])
        .arg(manifest)
        .arg("--store")
        .arg(store.root())
        .current_dir(root)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("planner installation or probe failed; no catalog pointer switched".into());
    }
    Ok(())
}
