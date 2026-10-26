//! `data/planner-runtime.toml`: what the services of a planner release run on, and the origins
//! that a planner release names.

use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct Runtime {
    pub target: Target,
    pub publication: Origins,
}

/// The machine of the follower: its target triple and the `major.minor` of Python and Node.
#[derive(Debug, Clone, Deserialize)]
pub struct Target {
    pub triple: String,
    pub python: String,
    pub node: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Origins {
    pub site_origin: String,
    /// The follower, `obc data serve` of the target.
    pub api_origin: String,
    pub objects_origin: String,
    /// The tile Worker, which reads `releases/<id>/public/` and `objects/` of the bucket.
    pub tiles_origin: String,
}

/// The file of the checkout at `root`. Each origin is `https://` and a host, without a path.
pub fn load(root: &Path) -> Result<Runtime, String> {
    let path = root.join("data/planner-runtime.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let runtime: Runtime = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let Origins { site_origin, api_origin, objects_origin, tiles_origin } = &runtime.publication;
    for origin in [site_origin, api_origin, objects_origin, tiles_origin] {
        let host = origin.strip_prefix("https://").ok_or("publication needs HTTPS origins")?;
        if host.is_empty() || !host.bytes().all(|byte| byte.is_ascii_alphanumeric() || b".-:".contains(&byte)) {
            return Err(format!("publication origin `{origin}` has a path, credentials or unsafe characters"));
        }
    }
    Ok(runtime)
}
