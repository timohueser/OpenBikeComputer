//! The shipped style documents, as the bakery sees them: **one schema and its
//! skins**.
//!
//! `builder/presets/` used to hold a flat set of style *presets*, each a complete
//! packer config that produced its own whole store. Epic #1016 D2 split that in two
//! ([`OBCC_Spec.md` §11.3/§11.4](../../../specs/OBCC_Spec.md)):
//!
//! ```text
//! builder/presets/
//!   schema.json          the packer config every artifact is baked with
//!   skins/<id>.json      presentation over that schema — colors, weights, dashes
//! ```
//!
//! The **schema** is what packs. Its feature-type list fixes the style *ids* that
//! every feature header in every chunk references (`OBCM_Spec.md` §5.2), so changing
//! it is a re-bake of the whole store, not a restyle. A **skin** is stamped onto the
//! ≈ 2 KB style table at assembly time and may restate only the presentation values —
//! which is why a skin is not a `--config` you can hand the packer, and why nothing
//! here treats it as one.
//!
//! The bakery needs three things from either document: the parsed [`Config`] (the
//! schema's to pack with, a skin's to check that it fits the schema), the
//! `_meta.version` to **record**, and the file's bytes — both to copy verbatim into
//! the bake tree (where `obc-pack catalog` reads its `_meta`) and to hash into the
//! idempotency key, so a schema edit re-bakes exactly the artifacts it invalidates
//! and nothing else.
//!
//! Note the asymmetry the v1 catalog spec insists on (`OBCC_Spec.md` §3): the version
//! copied into the tree describes the document *now*, the version written into a
//! sidecar describes the document *that artifact was packed with*. They are the same
//! number only until the next restyle, and the difference is the only signal a v1
//! consumer has that a region has not been re-baked yet. Both come from here, at
//! different moments, and neither is ever re-derived later.

use std::path::{Path, PathBuf};

use obc_pack::config::Config;

/// The canonical name of the schema document inside a style directory — the same
/// name it takes in a bake tree, so the source and the published tree read alike.
pub const SCHEMA_DOC: &str = "schema.json";

/// The skins subdirectory, likewise named as it is in a bake tree.
pub const SKINS_DIR: &str = "skins";

/// One style document, loaded and ready to bake with (a schema) or to publish
/// beside the cells (a skin).
pub struct StyleDoc {
    /// `_meta.id`: `bikepacking` for the schema, `default` for a skin. A skin's must
    /// also be its filename stem — a skin is addressed by id at assembly time, and
    /// two names for one document is one name too many.
    pub id: String,
    /// `_meta.version` at load time. Recorded into every sidecar this run writes.
    pub version: u32,
    pub path: PathBuf,
    /// The file's bytes, copied verbatim into the bake tree.
    pub json: String,
    /// SHA-256 of `json` — an ingredient of the bake key.
    pub sha256: String,
    /// Parsed with the packer's own loader. For a skin this is *not* a bakeable
    /// config (it carries no ladder and no routing); it is the style values, and the
    /// only thing that reads it is the schema-fit check.
    pub config: Config,
}

/// Load `<dir>/schema.json` — the one config every artifact is baked with.
pub fn load_schema(dir: &Path) -> Result<StyleDoc, String> {
    let path = dir.join(SCHEMA_DOC);
    if !path.is_file() {
        return Err(format!(
            "{}: no schema document — pass --presets-dir, or run from the repo root where \
             `builder/presets/{SCHEMA_DOC}` is",
            path.display()
        ));
    }
    read(&path, None)
}

/// Load `<dir>/skins/*.json`, optionally restricted to `only`.
///
/// `only` names that match nothing are an error rather than an empty run: a typo in
/// `--skin defualt` must not read as "publish no skins".
pub fn load_skins(dir: &Path, only: Option<&[String]>) -> Result<Vec<StyleDoc>, String> {
    let dir = dir.join(SKINS_DIR);
    if !dir.is_dir() {
        return Err(format!("{}: no skins directory — a catalog offers at least one skin", dir.display()));
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|e| e.map(|e| e.path()).map_err(|e| format!("{}: {e}", dir.display())))
        .collect::<Result<_, _>>()?;
    paths.sort();

    let mut skins = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
        if name.starts_with('.') || !name.ends_with(".json") {
            continue;
        }
        let stem = name.trim_end_matches(".json").to_string();
        if only.is_some_and(|only| !only.iter().any(|s| s == &stem)) {
            continue;
        }
        skins.push(read(&path, Some(&stem))?);
    }

    if let Some(only) = only {
        for want in only {
            if !skins.iter().any(|s| &s.id == want) {
                return Err(format!("{}: no skin named `{want}`", dir.display()));
            }
        }
    }
    if skins.is_empty() {
        return Err(format!("{}: no skin configs found", dir.display()));
    }
    Ok(skins)
}

/// Read one style document. `stem`, when given, is the filename the `_meta.id` must
/// agree with.
fn read(path: &Path, stem: Option<&str>) -> Result<StyleDoc, String> {
    let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: serde_json::Value = serde_json::from_str(&json).map_err(|e| format!("{}: {e}", path.display()))?;
    let meta = doc.get("_meta").ok_or_else(|| {
        format!("{}: no `_meta` block — the catalog cannot describe a style document without one", path.display())
    })?;
    let id = meta.get("id").and_then(|v| v.as_str()).unwrap_or_default().to_string();
    if id.is_empty() {
        return Err(format!("{}: `_meta.id` is missing", path.display()));
    }
    if let Some(stem) = stem {
        if id != stem {
            return Err(format!("{}: `_meta.id` is `{id}` but the filename says `{stem}`", path.display()));
        }
    }
    let version = meta
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("{}: `_meta.version` is missing — a sidecar has to record it", path.display()))?
        as u32;
    // Parse with the packer's own loader: a document that does not parse must fail
    // now, not per region, and not after the extract download.
    let config = Config::load(&path.to_string_lossy())?;
    let sha256 = crate::hash::text(&json);
    Ok(StyleDoc { id, version, path: path.to_path_buf(), json, sha256, config })
}
