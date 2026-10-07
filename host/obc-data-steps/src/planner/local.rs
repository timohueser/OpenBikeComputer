//! Matching-host execution is separate from portable published planner data.

use std::path::Path;

use obc_data::engine::{Code, Profile, Rust, Step};
use serde_json::json;

pub(super) fn routing(root: &Path) -> Result<Step, String> {
    let mut step = crate::python(
        "local/route-service",
        Vec::new(),
        json!({}),
        ("tools.planner_local_route", None),
        &["tools/planner_local_route.py"],
        &["route-server"],
    );
    step.code.crates = vec!["route-server".into()];
    step.code.rust = Some(Rust::Native { profile: Profile::Release });
    let files = step.code.files(root)?;
    step.options =
        json!({"code": obc_data::engine::digest(files.iter().map(|(key, value)| (key.as_str(), value.as_str())))});
    Ok(step)
}

pub(super) fn services() -> Code {
    Code {
        paths: [
            "tools/planner_local.py",
            "tools/planner_maps.py",
            "tools/planner_offline.py",
            "tools/planner_runtime.py",
            "apps/planner-search",
            "apps/planner-tiles",
            "builder/app/vite.config.ts",
            "builder/app/package.json",
            "builder/app/package-lock.json",
        ]
        .map(String::from)
        .into(),
        python: Some(obc_data::engine::Python { group: Some("planner-search".into()) }),
        ..Default::default()
    }
}
