//! The shipped style documents, as the bakery sees them: **one schema and its
//! skins**.
//!
//! ```text
//! builder/presets/
//!   schema.json          the packer config every cell is baked with
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
//! idempotency key, so a schema edit re-bakes exactly the cells it invalidates
//! and nothing else.
//!

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
    /// `_meta.version` at load time. The schema version becomes the cell-store revision;
    /// a skin version is published in that skin's catalog entry.
    pub version: u32,
    pub path: PathBuf,
    /// The file's bytes, copied verbatim into the bake tree.
    pub json: String,
    /// SHA-256 of the document's **body** — everything except `_meta` — and an
    /// ingredient of the bake key. See [`body_sha256`] for why it is not the file's.
    pub body_sha256: String,
    /// Parsed with the packer's own loader. For a skin this is *not* a bakeable
    /// config (it carries no ladder and no routing); it is the style values, and the
    /// only thing that reads it is the schema-fit check.
    pub config: Config,
}

/// Load `<dir>/schema.json` — the one config every cell is baked with.
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
    let body_sha256 = body_sha256(&doc, path)?;
    Ok(StyleDoc { id, version, path: path.to_path_buf(), json, body_sha256, config })
}

/// SHA-256 of a style document with `_meta` **stripped** — its packer-visible body.
///
/// The bake key exists to answer one question: *would re-packing produce different
/// bytes?* `_meta` cannot change that answer. The config loader treats it as an
/// unknown field and ignores it, so a document's id, display name, description,
/// swatch or `version` moving is, to the packer, no change at all — and hashing the
/// file's *text* said otherwise, which made a one-word description fix cost a full
/// re-pack of every region. #1036 is the case that made this concrete: the rename
/// `default` → `bikepacking` is a `_meta.id` edit and nothing else, and it must not
/// be an excuse to re-cut the store.
///
/// Everything *outside* `_meta` still counts, key order included: `obc-pack` numbers
/// feature types in document order and those ids are baked into every feature header
/// (`OBCM_Spec.md` §5.2), so a reordering really is a different bake. (`serde_json`
/// is built here with `preserve_order`, so the round-trip below keeps that order.)
///
/// The metadata is not thereby allowed to go stale: `_meta.version` is published in
/// every sidecar, and the bakery records it in the bake state and rewrites the
/// sidecar alone when it drifts — four lines of JSON instead of twenty hours.
fn body_sha256(doc: &serde_json::Value, path: &Path) -> Result<String, String> {
    let mut body = doc.clone();
    body.as_object_mut()
        .ok_or_else(|| format!("{}: a style document is a JSON object", path.display()))?
        .shift_remove("_meta");
    let text = serde_json::to_string(&body).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(crate::hash::text(&text))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal but real style document: `_meta` plus a body the packer would see.
    fn doc(meta: &str, features: &str) -> serde_json::Value {
        serde_json::from_str(&format!(
            r#"{{"_meta": {meta}, "chunk_size": 4096, "features": {{"highway": {features}}}}}"#
        ))
        .expect("valid JSON")
    }

    fn key(meta: &str, features: &str) -> String {
        body_sha256(&doc(meta, features), Path::new("doc.json")).expect("hashes")
    }

    const PRIMARY: &str = r#"{"primary": {"color": "0xFD40", "z_index": 50, "weight": 3}}"#;

    /// The property the schema/skin split rests on: `_meta` is not packer input,
    /// so every edit confined to it leaves the bake key alone. The id rename that
    /// turned `default` into `bikepacking` is the first case, and the one that would
    /// otherwise have cost a re-pack of the live shelf for no change in bytes.
    #[test]
    fn a_metadata_only_edit_does_not_move_the_bake_key() {
        let baseline = key(r#"{"id": "default", "name": "Default", "version": 1}"#, PRIMARY);
        for edited in [
            // the rename itself
            r#"{"id": "bikepacking", "name": "Default", "version": 1}"#,
            // a display-name fix
            r#"{"id": "default", "name": "Bikepacking", "version": 1}"#,
            // a version bump (published in the sidecar, not baked into a byte)
            r#"{"id": "default", "name": "Default", "version": 9}"#,
            // a whole new metadata field
            r##"{"id": "default", "name": "Default", "version": 1, "swatch": ["#FF5500"]}"##,
            // and `_meta` gone entirely
            r#"{}"#,
        ] {
            assert_eq!(baseline, key(edited, PRIMARY), "`_meta` reached the bake key: {edited}");
        }
    }

    /// The other half, or the first half would be a way to publish stale maps: a
    /// change to anything the packer reads must change the key.
    #[test]
    fn a_body_edit_moves_the_bake_key() {
        const META: &str = r#"{"id": "default", "name": "Default", "version": 1}"#;
        let baseline = key(META, PRIMARY);
        for edited in [
            // a recolor
            r#"{"primary": {"color": "0x0000", "z_index": 50, "weight": 3}}"#,
            // a weight
            r#"{"primary": {"color": "0xFD40", "z_index": 50, "weight": 1}}"#,
            // a new feature type — which also renumbers style ids
            r#"{"primary": {"color": "0xFD40", "z_index": 50, "weight": 3}, "track": {"color": "0xAAA0", "z_index": 24, "weight": 1}}"#,
        ] {
            assert_ne!(baseline, key(META, edited), "a body edit left the bake key alone: {edited}");
        }
    }

    /// Document order outside `_meta` still counts. `obc-pack` numbers feature types
    /// in the order it reads them and those ids are referenced by every feature header
    /// in every baked chunk (`OBCM_Spec.md` §5.2), so two documents that differ only
    /// in the order of their feature types genuinely bake to different bytes.
    #[test]
    fn reordering_feature_types_moves_the_bake_key() {
        const META: &str = r#"{"id": "default", "name": "Default", "version": 1}"#;
        let a = r#"{"primary": {"color": "0xFD40", "z_index": 50}, "track": {"color": "0xAAA0", "z_index": 24}}"#;
        let b = r#"{"track": {"color": "0xAAA0", "z_index": 24}, "primary": {"color": "0xFD40", "z_index": 50}}"#;
        assert_ne!(key(META, a), key(META, b));
    }

    /// The shipped schema is the document this all has to hold for, and it is the one
    /// that just had its `_meta` rewritten.
    #[test]
    fn the_shipped_schema_hashes_its_body_not_its_metadata() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../builder/presets");
        let schema = load_schema(&dir).expect("the shipped schema loads");
        let mut doc: serde_json::Value = serde_json::from_str(&schema.json).expect("valid JSON");
        assert_ne!(schema.body_sha256, crate::hash::text(&schema.json), "the key is not the file's text");
        doc["_meta"]["id"] = "something-else".into();
        doc["_meta"]["version"] = 999.into();
        assert_eq!(
            schema.body_sha256,
            body_sha256(&doc, &schema.path).expect("hashes"),
            "renaming the shipped schema must not invalidate a single baked artifact"
        );
    }
}
