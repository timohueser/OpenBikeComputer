//! The three static documents the app serves to its own frontend: the shipped
//! packer config(s), the device palette, and `obc-pack`'s config schema.
//!
//! All three are *baked in*, and that is the substantive difference from the dev
//! server. It reads presets off disk, the palette off disk, and shells out to
//! `obc-pack schema` (falling back to a checked-in copy when the binary isn't
//! built). None of those survive shipping: there is no repo next to an installed
//! app, and there is no `obc-pack` on `PATH` — there is one binary that *is* the
//! packer. So the schema comes from the linked-in library, which makes the
//! "editor's capability always matches the binary that packs" promise structural
//! rather than a cache key on an mtime.

use serde::Serialize;
use serde_json::{json, Value};

include!(concat!(env!("OUT_DIR"), "/presets.rs"));

/// `builder/palette.json` — the LS021B7DD02's 64-color gamut, laid out for the
/// picker grid.
const PALETTE_JSON: &str = include_str!("../../../builder/palette.json");

#[derive(Serialize)]
pub struct Preset {
    id: String,
    name: String,
    description: String,
    version: u32,
    swatch: Vec<String>,
    /// The bare packer config, `_meta` removed — directly submittable, and
    /// directly usable as a CLI config file.
    config: Value,
}

/// The shipped presets — since #1036 the one `schema.json`, the config every hosted
/// artifact is baked with. Sorted by name, so the order depends on the documents
/// rather than on the filesystem; the first card is the one a new user gets applied
/// for them.
pub fn presets() -> Vec<Preset> {
    let mut out: Vec<Preset> = PRESETS
        .iter()
        .filter_map(|(stem, body)| {
            let mut config: Value = serde_json::from_str(body).ok()?;
            let meta = config.as_object_mut()?.remove("_meta").unwrap_or(Value::Null);
            let s = |key: &str| meta.get(key).and_then(Value::as_str).map(str::to_string);
            Some(Preset {
                id: s("id").unwrap_or_else(|| (*stem).to_string()),
                name: s("name").unwrap_or_else(|| (*stem).to_string()),
                description: s("description").unwrap_or_default(),
                version: meta.get("version").and_then(Value::as_u64).unwrap_or(1) as u32,
                swatch: meta
                    .get("swatch")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default(),
                config,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The device's color gamut. `palette.json` is committed, so unlike the dev
/// server there is no "file missing" branch to fall back from — but a malformed
/// one would take the color picker down, so the generated gamut stays as the
/// backstop.
pub fn palette() -> Value {
    serde_json::from_str(PALETTE_JSON).unwrap_or_else(|_| generated_palette())
}

/// The LS021B7DD02's 64-color RGB222 gamut, laid out like `obc-sim --palette`
/// (8 columns, 2×2 of 4×4 red blocks).
fn generated_palette() -> Value {
    const LEVELS: [u32; 4] = [0, 85, 170, 255];
    let mut colors = Vec::with_capacity(64);
    for row in 0..8usize {
        for col in 0..8usize {
            let r = LEVELS[(row / 4) * 2 + (col / 4)];
            let g = LEVELS[row % 4];
            let b = LEVELS[col % 4];
            colors.push(format!("#{r:02X}{g:02X}{b:02X}"));
        }
    }
    json!({ "columns": 8, "colors": colors })
}

/// The config JSON Schema envelope, from the packer this binary links — not from
/// a subprocess and not from a checked-in copy.
pub fn schema() -> Result<Value, String> {
    serde_json::from_str::<Value>(&obc_pack::config::schema_envelope())
        .map(|mut envelope| {
            // The dev server tags where the schema came from ("binary" /
            // "repo-file") so a stale editor is diagnosable. Here there is only
            // one possible source, and saying so is the diagnosis.
            if let Some(obj) = envelope.as_object_mut() {
                obj.insert("source".into(), json!("linked"));
            }
            envelope
        })
        .map_err(|e| format!("obc-pack's schema envelope did not parse: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shipped_preset_is_a_packer_config() {
        let presets = presets();
        assert!(!presets.is_empty(), "expected the shipped schema to be embedded");
        for p in &presets {
            // The claim that matters: what the app hands the build command parses
            // as a config for the packer that is linked into this same binary.
            obc_pack::config::Config::parse(&serde_json::to_string(&p.config).expect("re-serialize"))
                .unwrap_or_else(|e| panic!("preset {} is not a valid packer config: {e}", p.id));
            assert!(!p.name.is_empty(), "preset {} has no name", p.id);
        }
    }

    /// The one bakeable style document is the bikepacking schema, and the skins beside
    /// it are **not** embedded: a skin carries no ladder and no routing table, so
    /// handing one to the build command would pack a one-level map (#1036).
    #[test]
    fn the_offered_config_is_the_bikepacking_schema_and_no_skin() {
        let presets = presets();
        assert_eq!(presets.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["bikepacking"]);
        assert_eq!(presets[0].name, "Bikepacking");
    }

    #[test]
    fn the_palette_is_the_committed_one_and_has_a_grid() {
        let p = palette();
        assert_eq!(p["columns"].as_u64(), Some(8));
        assert_eq!(p["colors"].as_array().map(Vec::len), Some(64));
    }

    #[test]
    fn the_schema_comes_from_the_linked_packer() {
        let s = schema().expect("schema");
        assert_eq!(s["source"], "linked");
        assert!(s.get("schema").is_some(), "envelope must carry the schema itself");
    }
}
