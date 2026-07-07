//! `config.rs` — parse the packer's `config.json`. Assigns style IDs 1-based in
//! document order, so `serde_json`'s `preserve_order` feature is mandatory (a
//! hash-ordered map would scramble the IDs). Exposes the ordered `tag_key → value →
//! style` map for first-match styling, the style table, LOD tiers, marker color,
//! chunk size, and the `routing` section (island-pruning threshold + the §8.6 bike
//! profiles the serializer bakes into the nav graph).

use std::collections::HashMap;

use serde_json::Value;

use crate::nav::{
    highway_class_index, surface_class_index, DEFAULT_MIN_COMPONENT_EDGES, HIGHWAY_CLASS_NAMES, SURFACE_CLASS_NAMES,
};
use crate::serialize::{NavProfile, Style, NAV_MAX_PROFILES, NAV_PROFILE_NAME_LEN};

/// 0xFF is the end-of-features sentinel in chunk payloads, so style IDs occupy
/// 1..=254 (ID 0 left unused).
const MAX_STYLE_ID: u32 = 254;

/// The config's JSON Schema, embedded verbatim and printed by `obc-pack schema`.
/// The web builder derives UI capability from it (a field present in the schema
/// ⇔ this binary parses it), so it must stay in lock-step with the parser below
/// — the `schema_*` tests walk this document against `parse_style` & friends.
pub const CONFIG_SCHEMA_JSON: &str = include_str!("../schema/config.schema.json");

/// Version of the `obc-pack schema` envelope itself; bump only on breaking
/// changes to the envelope shape, not on ordinary schema field additions.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// The `obc-pack schema` output: the embedded schema wrapped with the envelope
/// version and the OBCM format version this binary writes.
pub fn schema_envelope() -> String {
    let schema: Value = serde_json::from_str(CONFIG_SCHEMA_JSON).expect("embedded schema is valid JSON");
    let envelope = serde_json::json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "format_version": crate::serialize::OBCM_VERSION,
        "schema": schema,
    });
    serde_json::to_string_pretty(&envelope).expect("envelope serializes")
}

/// The Style Table fields plus the `min_lod` gate (filtered on, not serialized).
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

/// The parsed `routing` config section (N2): the island-pruning threshold plus the §8.6 bike
/// profiles baked into the nav graph. Absent ⇒ [`default_routing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Routing {
    /// Keep every connected graph component with ≥ this many edges (plus the largest, always). Wired
    /// into [`crate::nav::build_graph_with`]. Default [`DEFAULT_MIN_COMPONENT_EDGES`] (50).
    pub min_component_edges: usize,
    /// 1..=8 routing profiles, quantized to the §8.6 wire form. Never empty.
    pub profiles: Vec<NavProfile>,
}

/// The parsed `config.json`.
pub struct Config {
    /// `(tag_key, {value → style})` in document order. `get_style` walks the
    /// keys in this order and returns the first whose value matches.
    pub features: Vec<(String, HashMap<String, FeatureStyle>)>,
    pub lods: Vec<Lod>,
    pub marker_color: u16,
    pub chunk_size: usize,
    /// The `routing` section (island pruning + bike profiles).
    pub routing: Routing,
}

impl Config {
    /// Read + parse a `config.json` file.
    pub fn load(path: &str) -> Result<Config, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
        Self::parse(&text)
    }

    /// Parse `config.json` text.
    pub fn parse(text: &str) -> Result<Config, String> {
        let root: Value = serde_json::from_str(text).map_err(|e| format!("config json: {e}"))?;

        // Number every (tag_key, value) pair 1-based in document order; any `id`
        // present in the config is ignored.
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
                    max_mpp: l.get("max_mpp").and_then(Value::as_f64),
                    simplify_m: l.get("simplify").and_then(Value::as_f64).unwrap_or(0.0),
                })
                .collect(),
            _ => vec![Lod { max_mpp: None, simplify_m: 0.0 }],
        };

        // The reader parses the LOD table into a fixed `heapless::Vec<_, 16>` and the
        // header count is a `u8`, so cap here rather than let `lod_count as u8` wrap
        // or the reader silently drop layers.
        const MAX_LODS: usize = 16;
        if lods.len() > MAX_LODS {
            return Err(format!("too many LODs: {} configured, the reader supports at most {MAX_LODS}", lods.len()));
        }

        let marker_color =
            root.get("marker").and_then(|m| m.get("color")).map(parse_color).transpose()?.unwrap_or(0xF800);

        let chunk_size = root.get("chunk_size").and_then(Value::as_u64).map(|v| v as usize).unwrap_or(4096);

        let routing = parse_routing(root.get("routing"))?;

        Ok(Config { features, lods, marker_color, chunk_size, routing })
    }

    /// First matching `(tag_key, value)` in document order.
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

    /// The `natural.land` style, if the config requests land generation.
    pub fn land_style(&self) -> Option<&FeatureStyle> {
        self.features.iter().find(|(k, _)| k == "natural").and_then(|(_, m)| m.get("land"))
    }

    /// The full Style Table for the serializer (order is irrelevant; the
    /// serializer sorts by id).
    pub fn styles(&self) -> Vec<Style> {
        self.features.iter().flat_map(|(_, m)| m.values().map(FeatureStyle::to_style)).collect()
    }
}

/// `{z_index?, color, weight?, priority?, min_lod?}` → `FeatureStyle`, `id` from the
/// caller. Defaults: z_index 0, weight 1, priority 3, min_lod 0. Each numeric field
/// is range-checked against its on-wire width: an out-of-range value is a hard error,
/// not a silent wrap (e.g. `z_index: 200` would pack as `-56` and reorder the paint
/// stack). Adding a field here (e.g. v6 `line_style`/`color2`)? Extend
/// `schema/config.schema.json` and the `schema_*` tests in the same change.
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
/// `default`; a non-integer or out-of-range value is a descriptive error, not a wrap.
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

/// Color is a JSON int or a hex string like `"0xFAA0"`. A value past the 16-bit
/// RGB565 range is an error, not a silent truncation.
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

// --- routing section (N2): island-pruning threshold + §8.6 bike profiles --------------------

/// The four shipped bike profiles, embedded so `default_profiles` and the parser can't drift. The
/// presets in `packer/presets/` carry the same numbers verbatim (each preset is a complete config).
/// Multipliers are "prefer lower": each profile makes its favored way/surface classes ~1.0× and
/// penalizes the rest; `default` covers unlisted classes; `"forbidden"` excludes a class.
const DEFAULT_PROFILES_JSON: &str = r#"[
  {
    "name": "Road",
    "default": 3.0,
    "highway": {
      "cycleway": 1.0, "living_street": 1.4, "residential": 1.4, "unclassified": 1.5,
      "tertiary": 1.3, "secondary": 1.5, "primary": 1.8, "service": 2.2, "trunk_cycl": 2.5,
      "track": 6.0, "path": 7.0, "footway": 5.0, "bridleway": 7.0, "steps": "forbidden"
    },
    "surface": {
      "paved": 1.0, "compacted": 2.5, "gravel": 5.0, "dirt": 7.0, "rough": 8.0,
      "cobbles": 3.0, "grass": 8.0, "unknown": 1.5
    }
  },
  {
    "name": "Gravel",
    "default": 2.0,
    "highway": {
      "cycleway": 1.1, "track": 1.2, "path": 1.5, "unclassified": 1.2, "residential": 1.3,
      "living_street": 1.3, "tertiary": 1.3, "secondary": 1.6, "primary": 2.2, "service": 1.6,
      "footway": 3.0, "bridleway": 2.2, "trunk_cycl": 3.0, "steps": "forbidden"
    },
    "surface": {
      "paved": 1.1, "compacted": 1.0, "gravel": 1.1, "dirt": 1.6, "rough": 3.0,
      "cobbles": 2.0, "grass": 3.0, "unknown": 1.2
    }
  },
  {
    "name": "MTB",
    "default": 2.0,
    "highway": {
      "path": 1.0, "track": 1.0, "bridleway": 1.1, "cycleway": 1.3, "footway": 1.8,
      "unclassified": 1.6, "residential": 1.6, "living_street": 1.5, "tertiary": 1.8,
      "secondary": 2.4, "primary": 3.5, "service": 1.6, "steps": 3.0, "trunk_cycl": 4.0
    },
    "surface": {
      "dirt": 1.0, "gravel": 1.0, "compacted": 1.1, "rough": 1.3, "grass": 1.4,
      "cobbles": 1.5, "paved": 1.3, "unknown": 1.1
    }
  },
  {
    "name": "Touring",
    "default": 2.0,
    "highway": {
      "cycleway": 1.0, "residential": 1.2, "living_street": 1.2, "unclassified": 1.2,
      "tertiary": 1.3, "track": 1.6, "path": 2.0, "secondary": 1.7, "primary": 2.6,
      "service": 1.5, "footway": 2.5, "bridleway": 2.5, "steps": 6.0, "trunk_cycl": 3.0
    },
    "surface": {
      "paved": 1.0, "compacted": 1.2, "gravel": 2.0, "dirt": 3.0, "rough": 5.0,
      "cobbles": 2.0, "grass": 5.0, "unknown": 1.3
    }
  }
]"#;

/// The default `routing` section used when a config omits it: [`DEFAULT_MIN_COMPONENT_EDGES`] plus
/// [`default_profiles`].
pub fn default_routing() -> Routing {
    Routing { min_component_edges: DEFAULT_MIN_COMPONENT_EDGES, profiles: default_profiles() }
}

/// The four shipped bike profiles (Road / Gravel / MTB / Touring), quantized to §8.6's wire form.
/// Parsed from [`DEFAULT_PROFILES_JSON`] through the same path as user config, so the shipped
/// defaults and the parser can never disagree.
pub fn default_profiles() -> Vec<NavProfile> {
    let arr: Value = serde_json::from_str(DEFAULT_PROFILES_JSON).expect("embedded default profiles are valid JSON");
    parse_profiles(arr.as_array().expect("default profiles is a JSON array")).expect("embedded default profiles valid")
}

/// Parse the optional `routing` section. Absent ⇒ [`default_routing`]. A present-but-partial
/// section fills each missing field from the default (an omitted `profiles` still ships the four
/// defaults). `min_component_edges` is a non-negative integer; `profiles` is validated by
/// [`parse_profiles`].
fn parse_routing(v: Option<&Value>) -> Result<Routing, String> {
    let Some(v) = v else {
        return Ok(default_routing());
    };
    let obj = v.as_object().ok_or("`routing` must be an object")?;
    let min_component_edges = match obj.get("min_component_edges") {
        None | Some(Value::Null) => DEFAULT_MIN_COMPONENT_EDGES,
        Some(n) => n.as_u64().ok_or("routing.min_component_edges must be a non-negative integer")? as usize,
    };
    let profiles = match obj.get("profiles") {
        None | Some(Value::Null) => default_profiles(),
        Some(p) => parse_profiles(p.as_array().ok_or("routing.profiles must be an array")?)?,
    };
    Ok(Routing { min_component_edges, profiles })
}

/// Validate + quantize `routing.profiles`: 1..=[`NAV_MAX_PROFILES`] entries, each a [`parse_profile`].
fn parse_profiles(arr: &[Value]) -> Result<Vec<NavProfile>, String> {
    if arr.is_empty() {
        return Err("routing.profiles must list at least one profile".into());
    }
    if arr.len() > NAV_MAX_PROFILES {
        return Err(format!(
            "routing.profiles has {} entries; the OBCM profile table supports at most {NAV_MAX_PROFILES}",
            arr.len()
        ));
    }
    arr.iter().map(parse_profile).collect()
}

/// One profile object → the §8.6 wire form. `name` (required, ≤ 12 bytes) + a `default` multiplier
/// (config field, default 2.0) that fills every class not listed in the `highway`/`surface` maps.
/// Class keys are the canonical [`HIGHWAY_CLASS_NAMES`] / [`SURFACE_CLASS_NAMES`] (an unknown key is
/// a typo error). Every multiplier is a float ≥ 1.0 or `"forbidden"` ([`quantize_multiplier`]).
fn parse_profile(v: &Value) -> Result<NavProfile, String> {
    let obj = v.as_object().ok_or("routing.profiles[*] must be an object")?;
    let name = obj.get("name").and_then(Value::as_str).ok_or("routing profile missing string `name`")?.to_string();
    if name.len() > NAV_PROFILE_NAME_LEN {
        return Err(format!("routing profile name {name:?} exceeds {NAV_PROFILE_NAME_LEN} bytes on the wire"));
    }
    // Unlisted classes get the per-profile `default` (default 2.0×).
    let default_q = match obj.get("default") {
        None | Some(Value::Null) => 32u8, // 2.0× in 1/16 fixed-point
        Some(d) => quantize_multiplier(d, &name, "default", "(unlisted)")?,
    };
    let mut highway = [default_q; 32];
    let mut surface = [default_q; 8];
    if let Some(hw) = obj.get("highway") {
        let hw = hw.as_object().ok_or_else(|| format!("routing profile {name:?}: `highway` must be an object"))?;
        for (class, val) in hw {
            let idx = highway_class_index(class).ok_or_else(|| {
                format!(
                    "routing profile {name:?}: unknown highway class {class:?}; valid: {}",
                    HIGHWAY_CLASS_NAMES.join(", ")
                )
            })?;
            highway[idx as usize] = quantize_multiplier(val, &name, "highway", class)?;
        }
    }
    if let Some(sf) = obj.get("surface") {
        let sf = sf.as_object().ok_or_else(|| format!("routing profile {name:?}: `surface` must be an object"))?;
        for (class, val) in sf {
            let idx = surface_class_index(class).ok_or_else(|| {
                format!(
                    "routing profile {name:?}: unknown surface class {class:?}; valid: {}",
                    SURFACE_CLASS_NAMES.join(", ")
                )
            })?;
            surface[idx as usize] = quantize_multiplier(val, &name, "surface", class)?;
        }
    }
    // The admissibility invariant holds by construction (every value is 0 or ≥ 16); assert it so a
    // future quantization change can't silently break the A* heuristic bound.
    debug_assert!(
        highway.iter().chain(&surface).all(|&m| m == 0 || m >= 16),
        "every non-zero multiplier must be ≥ 16 (admissible)"
    );
    Ok(NavProfile { name, highway, surface })
}

/// Quantize one profile multiplier to §8.6's `u8` 1/16 fixed-point. `"forbidden"` ⇒ `0`; a number
/// ≥ 1.0 ⇒ `round(v × 16)` clamped to `16..=255` (≈ 1.0×..16×). A number **below 1.0 is rejected**
/// — the admissibility invariant (every non-zero weight ≥ 16) is what keeps the great-circle A*
/// heuristic admissible, so the ε-optimality bound survives profile weighting.
fn quantize_multiplier(v: &Value, profile: &str, kind: &str, class: &str) -> Result<u8, String> {
    match v {
        Value::String(s) if s == "forbidden" => Ok(0),
        Value::Number(_) => {
            let f = v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| {
                format!("routing profile {profile:?}: {kind} {class:?} multiplier {v} is not a finite number")
            })?;
            if f < 1.0 {
                return Err(format!(
                    "routing profile {profile:?}: {kind} {class:?} multiplier {f} is below 1.0 — every non-zero \
                     weight must be ≥ 1.0 (≥ 16 in 1/16 fixed-point) so the great-circle A* heuristic stays \
                     admissible; a weight < 1.0 lets the search underweight an edge and breaks the ε-optimality \
                     bound. Use \"forbidden\" to exclude a class."
                ));
            }
            Ok((f * 16.0).round().clamp(16.0, u8::MAX as f64) as u8)
        }
        other => Err(format!(
            "routing profile {profile:?}: {kind} {class:?} multiplier must be a number ≥ 1.0 or \"forbidden\", got {other}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus_config() -> Config {
        Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../packer/presets/default.json"))
            .expect("load corpus config")
    }

    #[test]
    fn style_ids_are_1_based_document_order() {
        let cfg = corpus_config();

        // Landmarks confirm the running counter never resets per tag_key.
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
        // `"lods": [{max_mpp:null, simplify:120}, {max_mpp:120, simplify:18},
        //           {max_mpp:18, simplify:0}]`
        assert_eq!(cfg.lods.len(), 3);
        assert_eq!(cfg.lods[0].max_mpp, None);
        assert_eq!(cfg.lods[0].simplify_m, 120.0);
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
        let ok = serde_json::json!({"color": "0x1234", "z_index": 100, "weight": 3, "priority": 4});
        assert!(parse_style(1, &ok).is_ok());

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

    /// Style IDs are a `u8` capped at 254 (id 0 unused, 0xFF is the chunk sentinel);
    /// a config with >254 styles must error, not wrap the 255th id and collide.
    #[test]
    fn too_many_styles_is_rejected() {
        let make = |n: usize| {
            let pairs: Vec<String> =
                (0..n).map(|i| format!("\"k{i}\": {{\"v\": {{\"color\": \"0x0001\"}}}}")).collect();
            format!("{{\"features\": {{{}}}}}", pairs.join(","))
        };

        let ok = Config::parse(&make(254)).expect("254 styles is the legal maximum");
        assert_eq!(ok.styles().len(), 254, "all 254 styles parsed");

        assert!(Config::parse(&make(255)).is_err(), "a 255th style must error (config.rs ~78), not wrap past u8");
    }

    // --- schema pinning: the embedded JSON Schema (served via `obc-pack schema`)
    // must describe exactly what this parser accepts. The web builder derives its
    // editor capability from the schema, so drift here means a UI that lies. ---

    fn embedded_schema() -> Value {
        serde_json::from_str(CONFIG_SCHEMA_JSON).expect("embedded schema is valid JSON")
    }

    fn style_with(field: &str, v: i64) -> Value {
        let mut m = serde_json::Map::new();
        m.insert("color".into(), Value::String("0x0001".into()));
        m.insert(field.into(), Value::from(v));
        Value::Object(m)
    }

    #[test]
    fn schema_style_bounds_match_parser() {
        let schema = embedded_schema();
        for field in ["z_index", "weight", "priority", "min_lod"] {
            let prop = &schema["$defs"]["style"]["properties"][field];
            let lo = prop["minimum"].as_i64().expect("schema minimum");
            let hi = prop["maximum"].as_i64().expect("schema maximum");
            assert!(parse_style(1, &style_with(field, lo)).is_ok(), "{field}: schema minimum {lo} must parse");
            assert!(parse_style(1, &style_with(field, hi)).is_ok(), "{field}: schema maximum {hi} must parse");
            assert!(parse_style(1, &style_with(field, lo - 1)).is_err(), "{field}: below schema minimum must error");
            assert!(parse_style(1, &style_with(field, hi + 1)).is_err(), "{field}: above schema maximum must error");
        }
    }

    #[test]
    fn schema_defaults_match_parser() {
        let schema = embedded_schema();
        let props = &schema["$defs"]["style"]["properties"];

        // Per-style defaults: a color-only style must come out as the schema says.
        let parsed = parse_style(1, &serde_json::json!({"color": "0x0001"})).expect("minimal style");
        assert_eq!(props["z_index"]["default"].as_i64(), Some(parsed.z_index as i64));
        assert_eq!(props["weight"]["default"].as_i64(), Some(parsed.weight as i64));
        assert_eq!(props["priority"]["default"].as_i64(), Some(parsed.priority as i64));
        assert_eq!(props["min_lod"]["default"].as_i64(), Some(parsed.min_lod as i64));

        // Global defaults: an empty config must come out as the schema says.
        let cfg = Config::parse("{}").expect("empty config parses");
        let marker_default = &schema["properties"]["marker"]["properties"]["color"]["default"];
        assert_eq!(parse_color(marker_default).unwrap(), cfg.marker_color);
        assert_eq!(schema["properties"]["chunk_size"]["default"].as_u64(), Some(cfg.chunk_size as u64));
        let lods_default = schema["properties"]["lods"]["default"].as_array().expect("lods default");
        assert_eq!(lods_default.len(), cfg.lods.len());
        assert!(lods_default[0]["max_mpp"].is_null() && cfg.lods[0].max_mpp.is_none());
        assert_eq!(lods_default[0]["simplify"].as_f64(), Some(cfg.lods[0].simplify_m));
    }

    #[test]
    fn schema_caps_match_parser_and_serializer() {
        let schema = embedded_schema();

        // LOD cap: 16 parses, 17 errors (see `too_many_lods_is_rejected`).
        assert_eq!(schema["properties"]["lods"]["maxItems"].as_u64(), Some(16));
        let entries: Vec<String> = (0..16).map(|i| format!("{{\"max_mpp\": {}, \"simplify\": 0}}", i + 1)).collect();
        let text = format!("{{\"features\": {{}}, \"lods\": [{}]}}", entries.join(","));
        assert!(Config::parse(&text).is_ok(), "exactly maxItems LODs must parse");

        // chunk_size bounds are the serializer's safe range, enforced at pack time.
        let max = schema["properties"]["chunk_size"]["maximum"].as_u64().expect("chunk_size maximum") as usize;
        assert_eq!(max, crate::serialize::MAX_SAFE_CHUNK_SIZE);
        assert!(crate::serialize::validate_chunk_size(max).is_ok());
        assert!(crate::serialize::validate_chunk_size(max + 1).is_err());
        let min = schema["properties"]["chunk_size"]["minimum"].as_u64().expect("chunk_size minimum") as usize;
        assert_eq!(min, crate::serialize::MIN_CHUNK_SIZE);
        assert!(crate::serialize::validate_chunk_size(min).is_ok());
        assert!(crate::serialize::validate_chunk_size(min - 1).is_err());
    }

    #[test]
    fn schema_color_def_matches_parser() {
        let schema = embedded_schema();
        let color = &schema["$defs"]["color"]["oneOf"];
        // Pin the hex-string pattern; `parse_color` accepts optional 0x/0X + 1..=4
        // hex digits (5 digits overflow the u16 and error — consistent both sides).
        assert_eq!(color[0]["pattern"].as_str(), Some("^(0[xX])?[0-9A-Fa-f]{1,4}$"));
        assert!(parse_color(&Value::String("0x12345".into())).is_err(), "5 hex digits must overflow u16");
        assert_eq!(color[1]["minimum"].as_u64(), Some(0));
        assert_eq!(color[1]["maximum"].as_u64(), Some(65535));
    }

    #[test]
    fn schema_declares_exactly_the_parsed_style_fields() {
        let schema = embedded_schema();
        let props = schema["$defs"]["style"]["properties"].as_object().expect("style properties");
        let mut keys: Vec<&str> = props.keys().map(String::as_str).collect();
        keys.sort_unstable();
        // The exact field set `parse_style` reads. Extending the parser (v6
        // `line_style`/`color2`) must extend the schema in the same change.
        assert_eq!(keys, ["color", "min_lod", "priority", "weight", "z_index"]);
        assert_eq!(schema["$defs"]["style"]["required"], serde_json::json!(["color"]));
    }

    #[test]
    fn schema_envelope_shape() {
        let env: Value = serde_json::from_str(&schema_envelope()).expect("envelope is valid JSON");
        assert_eq!(env["schema_version"].as_u64(), Some(CONFIG_SCHEMA_VERSION as u64));
        assert_eq!(env["format_version"].as_u64(), Some(crate::serialize::OBCM_VERSION as u64));
        assert_eq!(env["format_version"].as_u64(), Some(9), "N2 bumps the OBCM format to v9");
        assert!(env["schema"]["$defs"]["style"].is_object(), "envelope embeds the schema");
    }

    // --- routing section (N2) ---------------------------------------------------------------

    /// An omitted `routing` section yields the four shipped profiles + the default threshold.
    #[test]
    fn routing_defaults_when_absent() {
        let cfg = Config::parse("{}").expect("empty config parses");
        assert_eq!(cfg.routing.min_component_edges, DEFAULT_MIN_COMPONENT_EDGES);
        let names: Vec<&str> = cfg.routing.profiles.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Road", "Gravel", "MTB", "Touring"]);
        // Spot-check the quantization on the Road profile: cycleway 1.0 → 16, steps forbidden → 0,
        // primary 1.8 → 29, gravel 5.0 → 80.
        let road = &cfg.routing.profiles[0];
        assert_eq!(road.highway[0], 16, "cycleway 1.0× = 16");
        assert_eq!(road.highway[4], 0, "steps forbidden = 0");
        assert_eq!(road.highway[12], 29, "primary 1.8× ≈ 29");
        assert_eq!(road.surface[1], 16, "paved 1.0× = 16");
        assert_eq!(road.surface[3], 80, "gravel 5.0× = 80");
    }

    /// The admissibility invariant holds on every shipped profile: no non-zero multiplier below 16.
    #[test]
    fn default_profiles_are_admissible() {
        for p in default_profiles() {
            assert!(
                p.highway.iter().chain(&p.surface).all(|&m| m == 0 || m >= 16),
                "profile {:?} has a non-zero multiplier < 16 (inadmissible)",
                p.name
            );
        }
    }

    /// A custom profile quantizes correctly; unlisted classes take the per-profile `default`.
    #[test]
    fn routing_parses_and_quantizes_custom_profile() {
        let text = r#"{"routing":{"min_component_edges":12,"profiles":[
            {"name":"Test","default":2.0,"highway":{"cycleway":1.0,"primary":2.5,"steps":"forbidden"},
             "surface":{"paved":1.0,"gravel":4.0}}]}}"#;
        let cfg = Config::parse(text).expect("custom routing parses");
        assert_eq!(cfg.routing.min_component_edges, 12);
        assert_eq!(cfg.routing.profiles.len(), 1);
        let p = &cfg.routing.profiles[0];
        assert_eq!(p.name, "Test");
        assert_eq!(p.highway[0], 16, "cycleway 1.0×");
        assert_eq!(p.highway[12], 40, "primary 2.5× = 40");
        assert_eq!(p.highway[4], 0, "steps forbidden");
        assert_eq!(p.highway[7], 32, "residential unlisted → default 2.0× = 32");
        assert_eq!(p.surface[1], 16, "paved 1.0×");
        assert_eq!(p.surface[3], 64, "gravel 4.0× = 64");
        assert_eq!(p.surface[4], 32, "dirt unlisted → default 2.0×");
    }

    /// A multiplier below 1.0 is rejected and the message names the A* heuristic bound.
    #[test]
    fn routing_rejects_sub_unit_multiplier() {
        let text = r#"{"routing":{"profiles":[{"name":"Bad","highway":{"cycleway":0.5}}]}}"#;
        // `Config` isn't `Debug`, so match the Err arm rather than `expect_err`.
        let err = match Config::parse(text) {
            Ok(_) => panic!("a <1.0 multiplier must error"),
            Err(e) => e,
        };
        assert!(err.contains("admissible"), "the error must name the A* admissibility bound: {err}");
    }

    /// Boundary + rejection cases: empty list, >8, and unknown class names.
    #[test]
    fn routing_rejects_malformed_profiles() {
        assert!(Config::parse(r#"{"routing":{"profiles":[]}}"#).is_err(), "empty profiles must error");
        let nine: Vec<String> = (0..9).map(|i| format!("{{\"name\":\"P{i}\"}}")).collect();
        let text = format!("{{\"routing\":{{\"profiles\":[{}]}}}}", nine.join(","));
        assert!(Config::parse(&text).is_err(), "9 profiles must exceed the 8-cap");
        assert!(
            Config::parse(r#"{"routing":{"profiles":[{"name":"X","highway":{"autobahn":2.0}}]}}"#).is_err(),
            "an unknown highway class must error (typo protection)"
        );
    }

    // --- schema pinning for the routing section ---------------------------------------------

    /// The schema's routing default parses back to `default_routing()` — pins every shipped profile's
    /// quantized bytes AND the threshold, so the web builder's starting config matches the packer.
    #[test]
    fn schema_routing_default_matches_parser() {
        let schema = embedded_schema();
        let default = &schema["properties"]["routing"]["default"];
        let parsed = parse_routing(Some(default)).expect("schema routing default parses");
        assert_eq!(parsed, default_routing(), "schema routing default must equal the code default");
        assert_eq!(schema["properties"]["routing"]["properties"]["min_component_edges"]["default"].as_u64(), Some(50));
    }

    /// The schema's multiplier bound, profile caps, and class-name enums match the parser and the
    /// canonical class tables — so the editor accepts exactly what `parse_profile` does.
    #[test]
    fn schema_routing_bounds_match_parser() {
        let schema = embedded_schema();
        // Multiplier: number branch minimum 1.0, string branch const "forbidden".
        let mult = &schema["$defs"]["multiplier"]["oneOf"];
        assert_eq!(mult[0]["minimum"].as_f64(), Some(1.0));
        assert_eq!(mult[1]["const"].as_str(), Some("forbidden"));
        // 1.0 quantizes to 16, just under 1.0 is rejected — consistent both sides.
        assert_eq!(quantize_multiplier(&serde_json::json!(1.0), "p", "highway", "cycleway").unwrap(), 16);
        assert!(quantize_multiplier(&serde_json::json!(0.99), "p", "highway", "cycleway").is_err());
        // Profile-count caps.
        let profiles = &schema["properties"]["routing"]["properties"]["profiles"];
        assert_eq!(profiles["minItems"].as_u64(), Some(1));
        assert_eq!(profiles["maxItems"].as_u64(), Some(NAV_MAX_PROFILES as u64));
        // The class-name enums are exactly the canonical tables (the config vocabulary).
        let hw_enum: Vec<&str> = schema["$defs"]["profile"]["properties"]["highway"]["propertyNames"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(hw_enum, HIGHWAY_CLASS_NAMES, "highway class enum must mirror the canonical table");
        let sf_enum: Vec<&str> = schema["$defs"]["profile"]["properties"]["surface"]["propertyNames"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(sf_enum, SURFACE_CLASS_NAMES, "surface class enum must mirror the canonical table");
    }
}
