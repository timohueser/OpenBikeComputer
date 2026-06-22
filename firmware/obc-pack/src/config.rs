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
fn parse_style(id: u8, v: &Value) -> Result<FeatureStyle, String> {
    let color = v.get("color").map(parse_color).transpose()?.ok_or("style missing `color`")?;
    Ok(FeatureStyle {
        id,
        z_index: v.get("z_index").and_then(Value::as_i64).unwrap_or(0) as i8,
        color,
        weight: v.get("weight").and_then(Value::as_u64).unwrap_or(1) as u8,
        priority: v.get("priority").and_then(Value::as_u64).unwrap_or(3) as u8,
        min_lod: v.get("min_lod").and_then(Value::as_u64).unwrap_or(0) as usize,
    })
}

/// Color is either a JSON int or a hex string like `"0xFAA0"`.
fn parse_color(v: &Value) -> Result<u16, String> {
    match v {
        Value::String(s) => {
            let hex = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
            u16::from_str_radix(hex, 16).map_err(|e| format!("bad color {s:?}: {e}"))
        }
        Value::Number(n) => n.as_u64().map(|v| v as u16).ok_or_else(|| format!("bad numeric color {n}")),
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
}
