//! `config.rs` — parses the packer's `config.json`. Assigns style IDs 1-based in
//! document order (so `serde_json`'s `preserve_order` feature is mandatory — see
//! Cargo.toml; a hash-ordered map would scramble the IDs), and exposes everything
//! the pipeline needs: the ordered `tag_key → value → style` map for first-match
//! styling ([`Config::get_style`]), the style table for the serializer, the LOD
//! tiers, the marker color, and the chunk size.

use std::collections::HashMap;

use serde_json::Value;

use crate::serialize::Style;

/// 0xFF is the end-of-features sentinel in chunk payloads, so style IDs occupy
/// 1..=254 (ID 0 left unused).
const MAX_STYLE_ID: u32 = 254;

/// A configured feature style: its assigned id + the fields the Style Table
/// packs, plus the `min_lod` gate the pipeline filters on (not serialized).
#[derive(Debug, Clone)]
pub struct FeatureStyle {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    pub priority: u8,
    pub min_lod: usize,
}

impl FeatureStyle {
    /// The serializer's `Style` view (drops `min_lod`).
    pub fn to_style(&self) -> Style {
        Style { id: self.id, z_index: self.z_index, color: self.color, weight: self.weight, priority: self.priority }
    }
}

/// One LOD tier from `config["lods"]`.
#[derive(Debug, Clone, Copy)]
pub struct Lod {
    /// Meters-per-pixel upper bound; `None` ⇒ coarsest layer (`+inf`).
    pub max_mpp: Option<f64>,
    /// Simplify tolerance in **meters**; `0.0` ⇒ no simplify.
    pub simplify_m: f64,
}

/// The parsed `config.json`.
pub struct Config {
    /// `(tag_key, {value → style})` in document order. `get_style` walks the
    /// keys in this order and returns the first whose value matches.
    pub features: Vec<(String, HashMap<String, FeatureStyle>)>,
    pub lods: Vec<Lod>,
    pub marker_color: u16,
    pub chunk_size: usize,
}

impl Config {
    /// Read + parse a `config.json` file.
    pub fn load(path: &str) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
        Self::parse(&text)
    }

    /// Parse `config.json` text. `serde_json` must be built with `preserve_order`
    /// (it is — see Cargo.toml) so the `features` object keeps document order.
    pub fn parse(text: &str) -> Result<Config, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("config json: {e}"))?;

        // --- features + style-ID assignment ---
        // Number every (tag_key, value) pair 1-based in document order; any `id`
        // present in the config is deliberately ignored.
        let mut features: Vec<(String, HashMap<String, FeatureStyle>)> = Vec::new();
        let mut next_id: u32 = 1;
        if let Some(feature_map) = root.get("features").and_then(Value::as_object) {
            for (tag_key, values) in feature_map {
                let values = values.as_object().ok_or_else(|| format!("features.{tag_key} must be an object"))?;
                let mut by_value: HashMap<String, FeatureStyle> = HashMap::with_capacity(values.len());
                for (value, style) in values {
                    if next_id > MAX_STYLE_ID {
                        return Err(format!(
                            "too many feature types: the style table supports at most {MAX_STYLE_ID} entries"
                        ));
                    }
                    by_value.insert(value.clone(), parse_style(next_id as u8, style)?);
                    next_id += 1;
                }
                features.push((tag_key.clone(), by_value));
            }
        }

        // --- lods (absent/empty ⇒ a single coarsest layer) ---
        let lods = match root.get("lods").and_then(Value::as_array) {
            Some(arr) if !arr.is_empty() => arr
                .iter()
                .map(|l| Lod {
                    // Absent/null ⇒ None ⇒ +inf layer.
                    max_mpp: l.get("max_mpp").and_then(Value::as_f64),
                    // Absent/null ⇒ 0.0 ⇒ no simplify.
                    simplify_m: l.get("simplify").and_then(Value::as_f64).unwrap_or(0.0),
                })
                .collect(),
            // Missing or empty list ⇒ single coarsest layer, no simplify.
            _ => vec![Lod { max_mpp: None, simplify_m: 0.0 }],
        };

        // The device reader parses the LOD table into a fixed `heapless::Vec<_, 16>`, and the
        // header stores the count as a `u8` — so cap it here with a clear error rather than let
        // `lod_count as u8` wrap or the reader silently drop layers (issue #5).
        const MAX_LODS: usize = 16;
        if lods.len() > MAX_LODS {
            return Err(format!("too many LODs: {} configured, the reader supports at most {MAX_LODS}", lods.len()));
        }

        // --- marker color (default 0xF800) ---
        let marker_color =
            root.get("marker").and_then(|m| m.get("color")).map(parse_color).transpose()?.unwrap_or(0xF800);

        // --- chunk_size (default 4096) ---
        let chunk_size = root.get("chunk_size").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(4096);

        Ok(Config { features, lods, marker_color, chunk_size })
    }

    /// First matching `(tag_key, value)` in document order. `tags` is the way's
    /// tag map.
    pub fn get_style(&self, tags: &HashMap<&str, &str>) -> Option<&FeatureStyle> {
        for (tag_key, by_value) in &self.features {
            if let Some(val) = tags.get(tag_key.as_str()) {
                if let Some(style) = by_value.get(*val) {
                    return Some(style);
                }
            }
        }
        None
    }

    /// The `natural.land` style, if the config requests land generation. Its id
    /// + `min_lod` style the generated land polygons (see [`crate::land`]).
    pub fn land_style(&self) -> Option<&FeatureStyle> {
        self.features.iter().find(|(k, _)| k == "natural").and_then(|(_, m)| m.get("land"))
    }

    /// The full Style Table for the serializer (order is irrelevant; the
    /// serializer sorts by id).
    pub fn styles(&self) -> Vec<Style> {
        self.features.iter().flat_map(|(_, m)| m.values().map(FeatureStyle::to_style)).collect()
    }
}

/// `{z_index?, color, weight?, priority?, min_lod?}` → `FeatureStyle` with the
/// `id` chosen by the caller. Defaults: z_index 0, weight 1, priority 3, min_lod 0.
/// Each numeric field is range-checked against its on-wire width: an out-of-range
/// value is a hard error, not a silent wrap (issue #5 — `z_index: 200` used to pack
/// as `-56` and quietly reorder the paint stack).
fn parse_style(id: u8, v: &Value) -> Result<FeatureStyle, String> {
    let color = v.get("color").map(parse_color).transpose()?.ok_or("style missing `color`")?;
    Ok(FeatureStyle {
        id,
        z_index: int_field(v, "z_index", i8::MIN as i64, i8::MAX as i64, 0)? as i8,
        color,
        weight: int_field(v, "weight", 0, u8::MAX as i64, 1)? as u8,
        // Priority is a 2-bit on-wire field; the serializer only writes 1..=4.
        priority: int_field(v, "priority", 1, 4, 3)? as u8,
        min_lod: int_field(v, "min_lod", 0, u8::MAX as i64, 0)? as usize,
    })
}

/// Read an optional integer style field, validating it fits `lo..=hi`. Absent/null ⇒
/// `default`. A non-integer or out-of-range value is a descriptive error — the old `as`
/// casts silently wrapped both (issue #5).
fn int_field(v: &Value, key: &str, lo: i64, hi: i64, default: i64) -> Result<i64, String> {
    match v.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(val) => {
            let n = val.as_i64().ok_or_else(|| format!("style `{key}` must be an integer, got {val}"))?;
            if n < lo || n > hi {
                return Err(format!("style `{key}` {n} out of range {lo}..={hi}"));
            }
            Ok(n)
        }
    }
}

/// Color is either a JSON int or a hex string like `"0xFAA0"`. A numeric value past the
/// 16-bit RGB565 range is an error rather than a silent truncation (issue #5).
fn parse_color(v: &Value) -> Result<u16, String> {
    match v {
        Value::String(s) => {
            let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
            u16::from_str_radix(hex, 16).map_err(|e| format!("bad color {s:?}: {e}"))
        }
        Value::Number(n) => {
            n.as_u64().and_then(|v| u16::try_from(v).ok()).ok_or_else(|| format!("color {n} out of range 0..=65535"))
        }
        other => Err(format!("color must be int or hex string, got {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus_config() -> Config {
        // The same config.json the corpus + web builder use.
        Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/config.json")).expect("load corpus config")
    }

    #[test]
    fn style_ids_are_1_based_document_order() {
        // 42 styles, numbered in the order the feature types appear in config.json.
        let cfg = corpus_config();

        // The first feature type's first value is id 1, and a few landmarks down
        // the document confirm the running counter never resets per tag_key.
        let id = |key: &str, val: &str| {
            cfg.features.iter().find(|(k, _)| k == key).and_then(|(_, m)| m.get(val)).map(|s| s.id)
        };
        assert_eq!(id("highway", "motorway"), Some(1));
        assert_eq!(id("highway", "cycleway"), Some(19)); // last highway value
        assert_eq!(id("railway", "rail"), Some(20)); // counter carries across keys
        assert_eq!(id("building", "yes"), Some(29));
        assert_eq!(id("natural", "land"), Some(31));
        assert_eq!(id("admin_level", "2"), Some(42)); // last value in the document

        // Every id is unique and within 1..=254.
        let mut ids: Vec<u8> = cfg.styles().iter().map(|s| s.id).collect();
        ids.sort_unstable();
        assert_eq!(ids.first(), Some(&1));
        assert_eq!(ids.last(), Some(&42));
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "style ids must be unique");
    }

    #[test]
    fn lods_marker_chunk_parsed() {
        let cfg = corpus_config();
        // `"lods": [{max_mpp:null, simplify:50}, {max_mpp:120, simplify:12},
        //           {max_mpp:18, simplify:0}]`
        assert_eq!(cfg.lods.len(), 3);
        assert_eq!(cfg.lods[0].max_mpp, None);
        assert_eq!(cfg.lods[0].simplify_m, 50.0);
        assert_eq!(cfg.lods[1].max_mpp, Some(120.0));
        assert_eq!(cfg.lods[2].simplify_m, 0.0);
        // `"marker": {"color": "0xF800"}`
        assert_eq!(cfg.marker_color, 0xF800);
        // No chunk_size key ⇒ default.
        assert_eq!(cfg.chunk_size, 4096);
    }

    #[test]
    fn get_style_is_first_key_in_document_order() {
        let cfg = corpus_config();
        // A way tagged both highway=primary and building=yes: `highway` comes
        // first in the document, so primary (a line style) wins.
        let mut tags = HashMap::new();
        tags.insert("highway", "primary");
        tags.insert("building", "yes");
        let s = cfg.get_style(&tags).expect("matched");
        assert_eq!(s.id, 5); // highway=primary
                             // Unmatched tags ⇒ None.
        let mut other = HashMap::new();
        other.insert("barrier", "fence");
        assert!(cfg.get_style(&other).is_none());
    }

    #[test]
    fn color_parses_hex_and_int() {
        assert_eq!(parse_color(&Value::String("0xFAA0".into())).unwrap(), 0xFAA0);
        assert_eq!(parse_color(&Value::String("FAA0".into())).unwrap(), 0xFAA0);
        assert_eq!(parse_color(&serde_json::json!(64160)).unwrap(), 64160);
    }

    #[test]
    fn style_numeric_fields_are_range_checked() {
        // In-range values (and the i8 max) parse fine.
        let ok = serde_json::json!({"color": "0x1234", "z_index": 100, "weight": 3, "priority": 4});
        assert!(parse_style(1, &ok).is_ok());

        // Each out-of-range field is a hard error, not a silent wrap (issue #5).
        let bad_z = serde_json::json!({"color": "0x1234", "z_index": 200}); // would wrap to -56 under `as i8`
        assert!(parse_style(1, &bad_z).is_err(), "z_index 200 must error");
        let bad_weight = serde_json::json!({"color": "0x1234", "weight": 300});
        assert!(parse_style(1, &bad_weight).is_err(), "weight 300 must error");
        let bad_priority_hi = serde_json::json!({"color": "0x1234", "priority": 5});
        let bad_priority_lo = serde_json::json!({"color": "0x1234", "priority": 0});
        assert!(parse_style(1, &bad_priority_hi).is_err(), "priority 5 must error");
        assert!(parse_style(1, &bad_priority_lo).is_err(), "priority 0 must error");
    }

    #[test]
    fn numeric_color_out_of_range_is_rejected() {
        assert_eq!(parse_color(&serde_json::json!(65535)).unwrap(), 0xFFFF);
        assert!(parse_color(&serde_json::json!(70000)).is_err(), "a color past u16 must error, not truncate (#5)");
        assert!(parse_color(&serde_json::json!(-1)).is_err(), "a negative color must error");
    }

    #[test]
    fn too_many_lods_is_rejected() {
        // 17 LODs > the reader's 16-slot LOD table.
        let entries: Vec<String> = (0..17).map(|i| format!("{{\"max_mpp\": {}, \"simplify\": 0}}", i + 1)).collect();
        let text = format!("{{\"features\": {{}}, \"lods\": [{}]}}", entries.join(","));
        assert!(Config::parse(&text).is_err(), "more LODs than the reader supports must error (#5)");
    }

    /// Style IDs are a `u8` capped at `MAX_STYLE_ID` (254): id 0 is unused and 0xFF
    /// is the end-of-features chunk sentinel. A config defining >254 styles must error
    /// (config.rs ~78), not wrap the 255th id past the `u8` and collide with a
    /// sentinel/lower style. Untested before (issue #95, item 10). The boundary: 254
    /// styles is the last legal count, 255 is one too many.
    #[test]
    fn too_many_styles_is_rejected() {
        // Build a `features` object with `n` distinct (key, value) pairs, each a
        // single tag_key with one value, so the 1-based counter reaches `n`.
        let make = |n: usize| {
            let pairs: Vec<String> =
                (0..n).map(|i| format!("\"k{i}\": {{\"v\": {{\"color\": \"0x0001\"}}}}")).collect();
            format!("{{\"features\": {{{}}}}}", pairs.join(","))
        };

        // 254 styles is exactly the cap ⇒ accepted.
        let ok = Config::parse(&make(254)).expect("254 styles is the legal maximum");
        assert_eq!(ok.styles().len(), 254, "all 254 styles parsed");

        // 255 styles overflows the table ⇒ hard error, not a silent u8 wrap.
        assert!(Config::parse(&make(255)).is_err(), "a 255th style must error (config.rs ~78), not wrap past u8");
    }
}
