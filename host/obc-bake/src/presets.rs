//! The shipped style presets, as the bakery sees them.
//!
//! A preset is a complete, CLI-usable packer config (`builder/presets/*.json`) with
//! a `_meta` block riding along. The bakery needs three things from it: the parsed
//! [`Config`] to pack with, the `_meta.version` to **record** in each sidecar it
//! produces, and the file's bytes — both to copy verbatim into the bake tree (where
//! `obc-pack catalog` reads its `_meta`) and to hash into the idempotency key, so a
//! restyle re-bakes exactly the artifacts it invalidates and nothing else.
//!
//! Note the asymmetry the catalog spec insists on (`OBCC_Spec.md` §3): the version
//! copied into the tree describes the preset *now*, the version written into a
//! sidecar describes the preset *that artifact was packed with*. They are the same
//! number only until the next restyle, and the difference is the only signal a
//! consumer has that a region has not been re-baked yet. Both come from here, at
//! different moments, and neither is ever re-derived later.

use std::path::{Path, PathBuf};

use obc_pack::config::Config;

/// One preset, loaded and ready to bake with.
pub struct Preset {
    /// `default`, `minimal`, … — the filename stem and `_meta.id`, which must agree.
    pub id: String,
    /// `_meta.version` at load time. Recorded into every sidecar this run writes.
    pub version: u32,
    pub path: PathBuf,
    /// The file's bytes, copied verbatim into `<tree>/presets/<id>.json`.
    pub json: String,
    /// SHA-256 of `json` — an ingredient of the bake key.
    pub sha256: String,
    pub config: Config,
}

/// Load every preset in `dir`, optionally restricted to `only`.
///
/// `only` names that match nothing are an error rather than an empty run: a typo in
/// `--presets high-detials` must not read as "no work to do".
pub fn load(dir: &Path, only: Option<&[String]>) -> Result<Vec<Preset>, String> {
    if !dir.is_dir() {
        return Err(format!(
            "{}: no presets directory — pass --presets-dir, or run from the repo root where `builder/presets/` is",
            dir.display()
        ));
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("{}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    paths.sort();

    let mut presets = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let stem = name.trim_end_matches(".json").to_string();
        if let Some(only) = only {
            if !only.iter().any(|p| p == &stem) {
                continue;
            }
        }
        let json = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = doc.get("_meta").ok_or_else(|| {
            format!("{}: no `_meta` block — the catalog cannot describe a preset without one", path.display())
        })?;
        let meta_id = meta.get("id").and_then(|v| v.as_str()).unwrap_or_default();
        if meta_id != stem {
            return Err(format!("{}: `_meta.id` is `{meta_id}` but the filename says `{stem}`", path.display()));
        }
        let version = meta
            .get("version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("{}: `_meta.version` is missing — a sidecar has to record it", path.display()))?
            as u32;
        // Parse with the packer's own loader: a preset that does not parse must fail
        // now, not per region, and not after the extract download.
        let config = Config::load(&path.to_string_lossy())?;
        let sha256 = crate::hash::text(&json);
        presets.push(Preset { id: stem, version, path, json, sha256, config });
    }

    if let Some(only) = only {
        for want in only {
            if !presets.iter().any(|p| &p.id == want) {
                return Err(format!("{}: no preset named `{want}`", dir.display()));
            }
        }
    }
    if presets.is_empty() {
        return Err(format!("{}: no preset configs found", dir.display()));
    }
    Ok(presets)
}
