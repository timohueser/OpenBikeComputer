//! `config.rs` — parse the packer's `config.json`. Assigns style IDs 1-based in
//! document order, so `serde_json`'s `preserve_order` feature is mandatory (a
//! hash-ordered map would scramble the IDs). Exposes the ordered `tag_key → value →
//! style` map for first-match styling, the style table, LOD tiers, marker color,
//! chunk size, and the `routing` section (island-pruning threshold + the §8.6 bike
//! profiles the serializer bakes into the nav graph).

use std::collections::{BTreeMap, HashMap};

use indexmap::IndexMap;
use schemars::{json_schema, JsonSchema, Schema, SchemaGenerator};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::nav::{
    highway_class_index, surface_class_index, DEFAULT_MIN_COMPONENT_EDGES, HIGHWAY_CLASS_NAMES, SURFACE_CLASS_NAMES,
};
use obc_formats::obcm::{NAV_MAX_PROFILES, NAV_PROFILE_NAME_LEN, VERSION as OBCM_VERSION};

use crate::serialize::{NavProfile, Style};

/// 0xFF is the end-of-features sentinel in chunk payloads, so style IDs occupy
/// 1..=254 (ID 0 left unused).
const MAX_STYLE_ID: u32 = 254;
const MAX_LODS: usize = 16;
const DEFAULT_CHUNK_SIZE: usize = 4096;
const DEFAULT_MARKER_COLOR: u16 = 0xF800;

/// Deterministically generated fallback schema served by the web builder when
/// an `obc-pack` binary is unavailable. Production `obc-pack schema` output is
/// generated directly from [`ConfigDocument`]; a stale-generation test requires
/// this checked-in artifact to be semantically identical.
pub const CONFIG_SCHEMA_JSON: &str = include_str!("../schema/config.schema.json");

/// Version of the `obc-pack schema` envelope itself; bump only on breaking
/// changes to the envelope shape, not on ordinary schema field additions.
pub const CONFIG_SCHEMA_VERSION: u32 = 1;

/// Generate the config JSON Schema from the typed serde input model, then add
/// the semantic constraints that cannot be represented by Rust field types
/// alone (serializer capacities and the canonical routing-class vocabulary).
pub fn config_schema() -> Value {
    let mut schema = serde_json::to_value(schemars::schema_for!(ConfigDocument)).expect("config schema serializes");
    let root = schema.as_object_mut().expect("root schema is an object");
    root.insert("$schema".into(), Value::String("https://json-schema.org/draft/2020-12/schema".into()));
    root.insert("title".into(), Value::String("obc-pack config".into()));
    root.insert("description".into(), Value::String(CONFIG_DESCRIPTION.into()));

    let properties = root.get_mut("properties").and_then(Value::as_object_mut).expect("config properties");
    annotate_property(properties, "features", FEATURES_DESCRIPTION);
    annotate_property(properties, "lods", LODS_DESCRIPTION);
    annotate_property(properties, "marker", MARKER_DESCRIPTION);
    annotate_property(properties, "chunk_size", CHUNK_SIZE_DESCRIPTION);
    annotate_property(properties, "merge_fills", MERGE_FILLS_DESCRIPTION);
    annotate_property(properties, "merge_lines", MERGE_LINES_DESCRIPTION);
    annotate_property(properties, "contours", CONTOURS_DESCRIPTION);
    annotate_property(properties, "routing", ROUTING_DESCRIPTION);
    let contour_props = properties["contours"]["properties"].as_object_mut().expect("contours properties");
    contour_props["enabled"]["description"] = Value::String(
        "Trace contours from the terrain given to `--terrain`. Without terrain, nothing is traced.".into(),
    );
    contour_props["interval"]["description"] =
        Value::String("Vertical spacing between contours, in metres. Every level is a `contour.major` feature.".into());
    contour_props["interval"]["minimum"] = Value::from(1);
    contour_props["index_every"]["description"] = Value::String(
        "Every Nth contour is a `contour.index` feature instead of `contour.major`; 1 makes every contour an index \
         contour."
            .into(),
    );
    contour_props["index_every"]["minimum"] = Value::from(1);
    contour_props["simplify"]["description"] = Value::String(
        "Simplify tolerance in metres applied to traced contours *before* the LOD ladder sees them. This is a clamp, \
         not a detail knob: the fine tiers simplify at 3 m / 0.5 m, one to two orders finer than a ~40 m-posting DEM \
         supports, so a smaller value only stores interpolation noise."
            .into(),
    );
    contour_props["simplify"]["minimum"] = Value::from(0.0);
    properties["lods"]["maxItems"] = Value::from(MAX_LODS);
    properties["chunk_size"]["minimum"] = Value::from(crate::serialize::MIN_CHUNK_SIZE);
    properties["chunk_size"]["maximum"] = Value::from(crate::serialize::MAX_SAFE_CHUNK_SIZE);
    let routing_props = properties["routing"]["properties"].as_object_mut().expect("routing properties");
    routing_props["min_component_edges"]["description"] =
        Value::String("Drop disconnected graph components below this many edges; the largest is always kept.".into());
    routing_props["profiles"]["description"] =
        Value::String("Bike profiles selectable by index; every non-forbidden multiplier is >= 1.0.".into());
    routing_props["profiles"].as_object_mut().expect("profiles schema").remove("default");
    routing_props["profiles"]["minItems"] = Value::from(1);
    routing_props["profiles"]["maxItems"] = Value::from(NAV_MAX_PROFILES);

    let defs = root.get_mut("$defs").and_then(Value::as_object_mut).expect("config definitions");
    annotate_definitions(defs);
    schema
}

/// Stable pretty representation used to regenerate the checked-in web-builder
/// fallback: `obc-pack schema --config > host/obc-pack/schema/config.schema.json`.
pub fn config_schema_json() -> String {
    let mut text = serde_json::to_string_pretty(&config_schema()).expect("config schema serializes");
    text.push('\n');
    text
}

/// The `obc-pack schema` output: the generated schema wrapped with the stable
/// envelope version and the OBCM format version this binary writes.
pub fn schema_envelope() -> String {
    let envelope = serde_json::json!({
        "schema_version": CONFIG_SCHEMA_VERSION,
        "format_version": OBCM_VERSION,
        "schema": config_schema(),
    });
    serde_json::to_string_pretty(&envelope).expect("envelope serializes")
}

const CONFIG_DESCRIPTION: &str = "Configuration for the obc-pack map packer (.osm.pbf -> .obcm). Feature styling is an ordered map: style IDs are assigned 1-based in document order (at most 254 styles), and a way is styled by the first (tag_key, value) match in that order. Unknown keys at the root and inside objects are ignored, so tooling metadata (`_meta`, `disabled`) can ride along and any exported file doubles as a CLI config.";
const FEATURES_DESCRIPTION: &str = "OSM tag_key -> value -> style. Document order is load-bearing: it assigns style IDs and first-match wins. Within a tag_key, the value \"*\" is a catch-all: an exact value match wins, otherwise a \"*\" entry styles every other value that key carries (e.g. building -> \"*\" paints all buildings without listing each OSM type).";
const LODS_DESCRIPTION: &str =
    "LOD pyramid, coarsest tier first. Absent, null, or empty means a single coarsest layer.";
const MARKER_DESCRIPTION: &str =
    "User-position marker; the shape is fixed in firmware, only the color is configurable.";
const CHUNK_SIZE_DESCRIPTION: &str = "Quadtree chunk payload target in bytes. The maximum is the reader's per-feature vertex cap; the minimum guards against chunks so small that features are dropped wholesale. Values outside the range are rejected at pack time. Governs the geometry sections (LODs) only; the nav graph's chunks are pinned to 512 bytes.";
const MERGE_FILLS_DESCRIPTION: &str = "Dissolve fill polygons that render pixel-identically - same z_index, color, and priority, with no color2 - into one union per LOD. A pure map-size/render-cost optimization with no intended visual change; false (the default) packs byte-identically to before.";
const MERGE_LINES_DESCRIPTION: &str = "Stitch same-styled connected line fragments into maximal polylines per LOD. No intended visual change for solid lines; a dashed or cased line's pattern runs continuously across a former join. false (the default) packs byte-identically to before.";
const CONTOURS_DESCRIPTION: &str = "Contour lines traced from the baked OBCT terrain passed to `--terrain` and packed as ordinary line features. Two classes are emitted: `contour.major` for every level and `contour.index` for every Nth one; each is styled by a `features.contour.<class>` rule, and a class with no rule is simply not packed. Absent, or `enabled: false`, packs byte-identically to a map with no contours at all.";
const ROUTING_DESCRIPTION: &str = "Nav-graph routing config (OBCM §8): the island-pruning threshold plus the bike profiles baked into the map's profile table. Absent means the four shipped profiles (Road / Gravel / MTB / Touring).";

fn annotate_property(properties: &mut Map<String, Value>, name: &str, description: &str) {
    properties[name]["description"] = Value::String(description.into());
}

/// Typed input model. Dynamic OSM keys deliberately use insertion-ordered JSON
/// maps; every fixed config field is a serde type rather than an untyped tree.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
struct ConfigDocument {
    #[serde(default = "default_features_document")]
    #[schemars(default = "default_features_document")]
    #[schemars(with = "Option<BTreeMap<String, BTreeMap<String, StyleDocument>>>")]
    features: Option<IndexMap<String, IndexMap<String, StyleDocument>>>,
    #[serde(default = "default_lods_document")]
    #[schemars(default = "default_lods_document")]
    lods: Option<Vec<LodDocument>>,
    #[serde(default = "default_marker_document")]
    #[schemars(default = "default_marker_document")]
    marker: Option<MarkerDocument>,
    #[serde(default = "default_chunk_size_document")]
    #[schemars(default = "default_chunk_size_document")]
    chunk_size: Option<usize>,
    #[serde(default = "default_false_document")]
    #[schemars(default = "default_false_document")]
    merge_fills: Option<bool>,
    #[serde(default = "default_false_document")]
    #[schemars(default = "default_false_document")]
    merge_lines: Option<bool>,
    #[serde(default = "default_contours_document")]
    #[schemars(default = "default_contours_document")]
    contours: ContoursDocument,
    #[serde(default = "default_routing_document")]
    #[schemars(default = "default_routing_document")]
    routing: RoutingDocument,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(rename = "style")]
struct StyleDocument {
    color: ColorValue,
    #[serde(default = "default_zero_i8_document")]
    #[schemars(default = "default_zero_i8_document")]
    z_index: Option<i8>,
    #[serde(default = "default_weight_document")]
    #[schemars(default = "default_weight_document")]
    weight: Option<u8>,
    #[serde(default = "default_priority_document")]
    #[schemars(default = "default_priority_document")]
    priority: Option<u8>,
    #[serde(default = "default_zero_u8_document")]
    #[schemars(default = "default_zero_u8_document")]
    min_lod: Option<u8>,
    #[serde(default = "default_line_style_document")]
    #[schemars(default = "default_line_style_document")]
    line_style: Option<LineStyle>,
    #[serde(default = "default_false_document")]
    #[schemars(default = "default_false_document")]
    fixed_width: Option<bool>,
    #[serde(default = "default_false_document")]
    #[schemars(default = "default_false_document")]
    terrain_layer: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_optional_color", skip_serializing_if = "Option::is_none")]
    #[schemars(with = "ColorValue")]
    color2: Option<ColorValue>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(rename = "lod")]
struct LodDocument {
    #[serde(default)]
    max_mpp: Option<f64>,
    #[serde(default = "default_zero_f64_document")]
    #[schemars(default = "default_zero_f64_document")]
    simplify: Option<f64>,
    #[serde(default = "default_zero_f64_document")]
    #[schemars(default = "default_zero_f64_document")]
    min_area_px: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(inline)]
struct MarkerDocument {
    #[serde(default = "default_color_document")]
    #[schemars(default = "default_color_document")]
    color: ColorValue,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(inline)]
struct ContoursDocument {
    #[serde(default = "default_false_document")]
    #[schemars(default = "default_false_document")]
    enabled: Option<bool>,
    #[serde(default = "default_contour_interval_document")]
    #[schemars(default = "default_contour_interval_document")]
    interval: Option<u32>,
    #[serde(default = "default_contour_index_every_document")]
    #[schemars(default = "default_contour_index_every_document")]
    index_every: Option<u32>,
    #[serde(default = "default_contour_simplify_document")]
    #[schemars(default = "default_contour_simplify_document")]
    simplify: Option<f64>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(inline)]
struct RoutingDocument {
    #[serde(default = "default_min_component_edges_document")]
    #[schemars(default = "default_min_component_edges_document")]
    min_component_edges: Option<usize>,
    #[serde(default = "default_profiles_document")]
    #[schemars(default = "default_profiles_document")]
    profiles: Option<Vec<ProfileDocument>>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[schemars(rename = "profile")]
struct ProfileDocument {
    name: String,
    #[serde(default = "default_multiplier_document", rename = "default")]
    #[schemars(default = "default_multiplier_document")]
    default_multiplier: Option<MultiplierValue>,
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, MultiplierValue>")]
    highway: IndexMap<String, MultiplierValue>,
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, MultiplierValue>")]
    surface: IndexMap<String, MultiplierValue>,
    /// OBCM §8.6 `Climb Weight` (v12): flat metres charged per metre of ascent. Absent ⇒ `0`,
    /// climb-blind — a config that lists profiles but says nothing about climbing gets exactly
    /// v11's routing, which is the same "you did not ask for it" rule the packer's `--terrain`
    /// input follows.
    #[serde(default = "default_zero_u8_document")]
    #[schemars(default = "default_zero_u8_document")]
    climb_weight: Option<u8>,
}

fn default_features_document() -> Option<IndexMap<String, IndexMap<String, StyleDocument>>> {
    Some(IndexMap::new())
}

fn default_lods_document() -> Option<Vec<LodDocument>> {
    Some(vec![LodDocument { max_mpp: None, simplify: Some(0.0), min_area_px: Some(0.0) }])
}

fn default_marker_document() -> Option<MarkerDocument> {
    Some(MarkerDocument { color: default_color_document() })
}

const fn default_chunk_size_document() -> Option<usize> {
    Some(DEFAULT_CHUNK_SIZE)
}

const fn default_false_document() -> Option<bool> {
    Some(false)
}

const fn default_zero_i8_document() -> Option<i8> {
    Some(0)
}

const fn default_zero_u8_document() -> Option<u8> {
    Some(0)
}

const fn default_zero_f64_document() -> Option<f64> {
    Some(0.0)
}

const fn default_weight_document() -> Option<u8> {
    Some(1)
}

const fn default_priority_document() -> Option<u8> {
    Some(3)
}

const fn default_line_style_document() -> Option<LineStyle> {
    Some(LineStyle::Solid)
}

const fn default_color_document() -> ColorValue {
    ColorValue(DEFAULT_MARKER_COLOR)
}

const fn default_min_component_edges_document() -> Option<usize> {
    Some(DEFAULT_MIN_COMPONENT_EDGES)
}

fn default_profiles_document() -> Option<Vec<ProfileDocument>> {
    Some(serde_json::from_str(DEFAULT_PROFILES_JSON).expect("embedded default profiles are valid typed config"))
}

/// 100 m, the one ladder EL10 traces: at the finest tier the screen is ~290 m wide against a
/// ~40 × 57 m posting, so a 50 m tier draws interpolation segments rather than terrain.
const DEFAULT_CONTOUR_INTERVAL_M: u32 = 100;
/// Every 5th contour (500 m at the default interval) is an index contour.
const DEFAULT_CONTOUR_INDEX_EVERY: u32 = 5;
/// The traced-geometry clamp in metres (#1094): see [`CONTOURS_DESCRIPTION`].
const DEFAULT_CONTOUR_SIMPLIFY_M: f64 = 15.0;

const fn default_contour_interval_document() -> Option<u32> {
    Some(DEFAULT_CONTOUR_INTERVAL_M)
}

const fn default_contour_index_every_document() -> Option<u32> {
    Some(DEFAULT_CONTOUR_INDEX_EVERY)
}

const fn default_contour_simplify_document() -> Option<f64> {
    Some(DEFAULT_CONTOUR_SIMPLIFY_M)
}

fn default_contours_document() -> ContoursDocument {
    ContoursDocument {
        enabled: default_false_document(),
        interval: default_contour_interval_document(),
        index_every: default_contour_index_every_document(),
        simplify: default_contour_simplify_document(),
    }
}

fn default_routing_document() -> RoutingDocument {
    RoutingDocument {
        min_component_edges: default_min_component_edges_document(),
        profiles: default_profiles_document(),
    }
}

const fn default_multiplier_document() -> Option<MultiplierValue> {
    Some(MultiplierValue::Number(2.0))
}

#[derive(Debug, Clone, Copy)]
struct ColorValue(u16);

impl<'de> Deserialize<'de> for ColorValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = ColorValue;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("an RGB565 integer 0..=65535 or 1..=4 digit hexadecimal string")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u16::try_from(value)
                    .map(ColorValue)
                    .map_err(|_| E::custom(format_args!("color {value} out of range 0..=65535")))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                u16::try_from(value)
                    .map(ColorValue)
                    .map_err(|_| E::custom(format_args!("color {value} out of range 0..=65535")))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let hex = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")).unwrap_or(value);
                u16::from_str_radix(hex, 16)
                    .map(ColorValue)
                    .map_err(|e| E::custom(format_args!("bad color {value:?}: {e}")))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

fn deserialize_optional_color<'de, D>(deserializer: D) -> Result<Option<ColorValue>, D::Error>
where
    D: Deserializer<'de>,
{
    ColorValue::deserialize(deserializer).map(Some)
}

impl Serialize for ColorValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("0x{:04X}", self.0))
    }
}

impl JsonSchema for ColorValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "color".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "RGB565 as a `0x`-prefixed hex string (1-4 digits) or an integer 0..=65535. Pick values on the panel's RGB222 grid (4 levels per channel) so the editor and the glass agree.",
            "oneOf": [
                { "type": "string", "pattern": "^(0[xX])?[0-9A-Fa-f]{1,4}$" },
                { "type": "integer", "minimum": 0, "maximum": 65535 }
            ]
        })
    }
}

#[derive(Debug, Clone)]
enum MultiplierValue {
    Number(f64),
    Forbidden,
}

impl<'de> Deserialize<'de> for MultiplierValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct Visitor;
        impl de::Visitor<'_> for Visitor {
            type Value = MultiplierValue;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a number >= 1.0 or the string \"forbidden\"")
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MultiplierValue::Number(value))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MultiplierValue::Number(value as f64))
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(MultiplierValue::Number(value as f64))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value == "forbidden" {
                    Ok(MultiplierValue::Forbidden)
                } else {
                    Err(E::custom(format_args!("expected \"forbidden\", got {value:?}")))
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

impl Serialize for MultiplierValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            MultiplierValue::Number(value) => serializer.serialize_f64(*value),
            MultiplierValue::Forbidden => serializer.serialize_str("forbidden"),
        }
    }
}

impl JsonSchema for MultiplierValue {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "multiplier".into()
    }

    fn json_schema(_generator: &mut SchemaGenerator) -> Schema {
        json_schema!({
            "description": "One routing edge-weight multiplier: a number >= 1.0 or the string \"forbidden\". Values below 1.0 are rejected because the great-circle A* heuristic must stay admissible.",
            "oneOf": [
                { "type": "number", "minimum": 1.0 },
                { "type": "string", "const": "forbidden" }
            ]
        })
    }
}

/// A line's stroke style (OBCM §2 style-record flag bit 2). The config value is `"solid"` (the
/// default) or `"dashed"`; the renderer draws dashes for `Dashed` lines and ignores it for polygons
/// (#557 only carries the bit end to end — later sub-issues render it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(rename = "line_style", rename_all = "lowercase")]
pub enum LineStyle {
    #[default]
    Solid,
    Dashed,
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
    /// v10: solid or dashed stroke (style-record flag bit 2).
    pub line_style: LineStyle,
    /// #1095: **fixed width** (flag bit 4) — `weight` is the on-screen stroke in device pixels and
    /// the renderer's zoom→width ramp is bypassed for this style. For a *mark on the map* rather
    /// than a thing with width on the ground; contours are the first shipped style that is one.
    pub fixed_width: bool,
    /// #1095: **terrain layer** (flag bit 5) — this style belongs to the suppressible terrain group.
    /// The packer writes the bit and the reader carries it; the consumer is the device's Settings
    /// toggle (#1096). Nothing renders differently because of it today.
    pub terrain_layer: bool,
    /// v10: optional RGB565 secondary color (flag bit 3 + the trailing u16), parsed like `color`.
    pub color2: Option<u16>,
}

impl FeatureStyle {
    /// The serializer's `Style` view (drops `min_lod`).
    pub fn to_style(&self) -> Style {
        Style {
            id: self.id,
            z_index: self.z_index,
            color: self.color,
            weight: self.weight,
            priority: self.priority,
            dashed: self.line_style == LineStyle::Dashed,
            color2: self.color2,
            fixed_width: self.fixed_width,
            terrain_layer: self.terrain_layer,
        }
    }
}

/// One LOD tier from `config["lods"]`.
#[derive(Debug, Clone, Copy)]
pub struct Lod {
    /// Meters-per-pixel upper bound; `None` ⇒ coarsest layer (`+inf`).
    pub max_mpp: Option<f64>,
    /// Simplify tolerance in **meters**; `0.0` ⇒ no simplify.
    pub simplify_m: f64,
    /// Coarse-LOD minimum-area cull threshold in **square pixels**; `0.0` ⇒ off.
    /// A **polygon** whose projected area is below this at the tier's finest
    /// on-screen scale — the next-finer tier's `max_mpp` — is dropped from this
    /// tier. Lines are never culled (fragmented ways ⇒ road holes). The finest
    /// tier is never culled (no finer fallback), so its value is ignored. See
    /// [`crate::geom::footprint_below`].
    pub min_area_px: f64,
}

/// The parsed `contours` config section (EL10a, #1094): what [`crate::contour`] traces out of the
/// OBCT terrain a run was given. Absent ⇒ [`Contours::default`], which is off.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contours {
    /// Trace at all. `false` ⇒ the tracer is never entered and the pack is byte-identical to one
    /// built before contours existed.
    pub enabled: bool,
    /// Vertical spacing between contours in **metres**, ≥ 1.
    pub interval_m: i32,
    /// Every Nth level is a [`ContourClass::Index`] contour instead of [`ContourClass::Major`], ≥ 1.
    pub index_every: u32,
    /// The pre-ladder simplify clamp in **metres**; `0.0` ⇒ no clamp (see [`CONTOURS_DESCRIPTION`]).
    pub simplify_m: f64,
}

impl Default for Contours {
    fn default() -> Self {
        Contours {
            enabled: false,
            interval_m: DEFAULT_CONTOUR_INTERVAL_M as i32,
            index_every: DEFAULT_CONTOUR_INDEX_EVERY,
            simplify_m: DEFAULT_CONTOUR_SIMPLIFY_M,
        }
    }
}

/// The two feature classes a contour trace emits. They are ordinary style rules under the synthetic
/// `contour` tag key — `features.contour.major` / `features.contour.index` — so the config styles
/// them independently, and a class with no rule is simply not traced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContourClass {
    /// Every level of the ladder.
    Major,
    /// Every `index_every`th level.
    Index,
}

impl ContourClass {
    /// The config value this class is styled under.
    pub fn as_str(self) -> &'static str {
        match self {
            ContourClass::Major => "major",
            ContourClass::Index => "index",
        }
    }
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
    /// Dissolve fill polygons that render pixel-identically (same `(z_index, color,
    /// priority)`, no `color2`) into one union per LOD — a pure size/render-cost win
    /// with no intended visual change ([`crate::merge`]). Default `false` ⇒ absent
    /// flag packs byte-identically to before.
    pub merge_fills: bool,
    /// Stitch same-styled connected line fragments (an OSM way split into many
    /// segments) into maximal polylines per LOD, reclaiming a span + a ring per join
    /// ([`crate::merge`]). No intended visual change for solid lines; dash/casing
    /// phase runs continuously across a former join. Default `false` ⇒ byte-identical.
    pub merge_lines: bool,
    /// The `contours` section (EL10a): the ladder [`crate::contour`] traces out of `--terrain`.
    /// Default off ⇒ absent block packs byte-identically to before.
    pub contours: Contours,
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
        let mut deserializer = serde_json::Deserializer::from_str(text);
        let document: ConfigDocument = serde_path_to_error::deserialize(&mut deserializer).map_err(|error| {
            let path = error.path().to_string();
            if path == "." {
                format!("config json: {}", error.inner())
            } else {
                format!("config {path}: {}", error.inner())
            }
        })?;
        Self::from_document(document)
    }

    fn from_document(document: ConfigDocument) -> Result<Config, String> {
        // Number every (tag_key, value) pair 1-based in document order. Unknown
        // object fields were ignored by serde; a configured `id` never enters
        // the typed style model and therefore remains intentionally ignored.
        let mut features: Vec<(String, HashMap<String, FeatureStyle>)> = Vec::new();
        let mut next_id: u32 = 1;
        if let Some(feature_map) = document.features {
            for (tag_key, values) in feature_map {
                let mut by_value: HashMap<String, FeatureStyle> = HashMap::with_capacity(values.len());
                for (value, style) in values {
                    if next_id > MAX_STYLE_ID {
                        return Err(format!(
                            "too many feature types: the style table supports at most {MAX_STYLE_ID} entries"
                        ));
                    }
                    by_value.insert(value.clone(), style.normalize(next_id as u8, &tag_key, &value)?);
                    next_id += 1;
                }
                features.push((tag_key, by_value));
            }
        }

        // --- lods (absent/empty ⇒ a single coarsest layer) ---
        let lods = match document.lods {
            Some(entries) if !entries.is_empty() => entries
                .into_iter()
                .enumerate()
                .map(|(index, lod)| lod.normalize(index))
                .collect::<Result<Vec<_>, _>>()?,
            _ => vec![Lod { max_mpp: None, simplify_m: 0.0, min_area_px: 0.0 }],
        };

        // The reader parses the LOD table into a fixed `heapless::Vec<_, 16>` and the
        // header count is a `u8`, so cap here rather than let `lod_count as u8` wrap
        // or the reader silently drop layers.
        if lods.len() > MAX_LODS {
            return Err(format!("too many LODs: {} configured, the reader supports at most {MAX_LODS}", lods.len()));
        }

        let marker_color = document.marker.map_or(DEFAULT_MARKER_COLOR, |marker| marker.color.0);
        let chunk_size = document.chunk_size.unwrap_or(DEFAULT_CHUNK_SIZE);
        let merge_fills = document.merge_fills.unwrap_or(false);
        let merge_lines = document.merge_lines.unwrap_or(false);
        let contours = document.contours.normalize()?;
        let routing = document.routing.normalize()?;

        Ok(Config { features, lods, marker_color, chunk_size, merge_fills, merge_lines, contours, routing })
    }

    /// First matching `(tag_key, value)` in document order. Within a matched
    /// `tag_key`, an exact value match wins; failing that, a `"*"` catch-all
    /// entry (if the category defines one) styles every other value the key
    /// carries. So `building: {"warehouse": …, "*": …}` gives warehouses their
    /// own style and paints every other `building=*` with the catch-all — no
    /// need to enumerate OSM's ~50 building values by hand.
    pub fn get_style(&self, tags: &HashMap<&str, &str>) -> Option<&FeatureStyle> {
        for (tag_key, by_value) in &self.features {
            if let Some(val) = tags.get(tag_key.as_str()) {
                if let Some(style) = by_value.get(*val).or_else(|| by_value.get("*")) {
                    return Some(style);
                }
            }
        }
        None
    }

    /// The style rule at `features.<tag_key>.<value>`, exactly — no `"*"` fallback.
    ///
    /// This is the lookup the packer's **generated** features use (land, contours): they carry no
    /// OSM tags, so [`get_style`](Self::get_style)'s first-match walk has nothing to match on, and a
    /// catch-all written for some real OSM key must never end up styling them.
    pub fn feature_style(&self, tag_key: &str, value: &str) -> Option<&FeatureStyle> {
        self.features.iter().find(|(k, _)| k == tag_key).and_then(|(_, m)| m.get(value))
    }

    /// The `natural.land` style, if the config requests land generation.
    pub fn land_style(&self) -> Option<&FeatureStyle> {
        self.feature_style("natural", "land")
    }

    /// The style for one traced contour class, if the config asks for it. `None` ⇒ that class is
    /// not packed (and not even traced).
    pub fn contour_style(&self, class: ContourClass) -> Option<&FeatureStyle> {
        self.feature_style("contour", class.as_str())
    }

    /// The full Style Table for the serializer (order is irrelevant; the
    /// serializer sorts by id).
    pub fn styles(&self) -> Vec<Style> {
        self.features.iter().flat_map(|(_, m)| m.values().map(FeatureStyle::to_style)).collect()
    }
}

impl StyleDocument {
    /// Normalize a typed style into the serializer-facing representation. Rust
    /// integer widths own the wire-sized ranges; priority's narrower 1..=4
    /// policy remains an explicit semantic check.
    fn normalize(self, id: u8, tag_key: &str, tag_value: &str) -> Result<FeatureStyle, String> {
        let priority = self.priority.unwrap_or(3);
        if !(1..=4).contains(&priority) {
            return Err(format!("config features.{tag_key}.{tag_value}.priority: {priority} out of range 1..=4"));
        }
        Ok(FeatureStyle {
            id,
            z_index: self.z_index.unwrap_or(0),
            color: self.color.0,
            weight: self.weight.unwrap_or(1),
            priority,
            min_lod: self.min_lod.unwrap_or(0) as usize,
            line_style: self.line_style.unwrap_or_default(),
            fixed_width: self.fixed_width.unwrap_or(false),
            terrain_layer: self.terrain_layer.unwrap_or(false),
            color2: self.color2.map(|color| color.0),
        })
    }
}

impl LodDocument {
    fn normalize(self, index: usize) -> Result<Lod, String> {
        let simplify_m = self.simplify.unwrap_or(0.0);
        if simplify_m < 0.0 {
            return Err(format!("config lods[{index}].simplify: {simplify_m} must be >= 0"));
        }
        let min_area_px = self.min_area_px.unwrap_or(0.0);
        if min_area_px < 0.0 {
            return Err(format!("config lods[{index}].min_area_px: {min_area_px} must be >= 0"));
        }
        Ok(Lod { max_mpp: self.max_mpp, simplify_m, min_area_px })
    }
}

impl ContoursDocument {
    fn normalize(self) -> Result<Contours, String> {
        let default = Contours::default();
        let interval = self.interval.unwrap_or(DEFAULT_CONTOUR_INTERVAL_M);
        if interval == 0 {
            return Err("config contours.interval: must be >= 1 metre".into());
        }
        let interval_m = i32::try_from(interval)
            .map_err(|_| format!("config contours.interval: {interval} m is not a plausible contour spacing"))?;
        let index_every = self.index_every.unwrap_or(DEFAULT_CONTOUR_INDEX_EVERY);
        if index_every == 0 {
            return Err("config contours.index_every: must be >= 1 (1 makes every contour an index contour)".into());
        }
        // The product is what a level is tested against; it must stay in `i32` for that test.
        interval_m
            .checked_mul(index_every as i32)
            .ok_or_else(|| format!("config contours.index_every: {interval} m x {index_every} overflows"))?;
        let simplify_m = self.simplify.unwrap_or(DEFAULT_CONTOUR_SIMPLIFY_M);
        if !(simplify_m.is_finite() && simplify_m >= 0.0) {
            return Err(format!("config contours.simplify: {simplify_m} must be a finite value >= 0"));
        }
        Ok(Contours { enabled: self.enabled.unwrap_or(default.enabled), interval_m, index_every, simplify_m })
    }
}

// --- routing section (N2): island-pruning threshold + §8.6 bike profiles --------------------

/// The four shipped bike profiles, embedded so `default_profiles` and the parser can't drift. The
/// presets in `builder/presets/` carry the same numbers verbatim (each preset is a complete config).
/// Multipliers are "prefer lower": each profile makes its favored way/surface classes ~1.0× and
/// penalizes the rest; `default` covers unlisted classes; `"forbidden"` excludes a class.
const DEFAULT_PROFILES_JSON: &str = r#"[
  {
    "name": "Road",
    "default": 3.0,
    "climb_weight": 10,
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
    "climb_weight": 8,
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
    "climb_weight": 6,
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
    "climb_weight": 8,
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

fn annotate_definitions(defs: &mut Map<String, Value>) {
    let style = defs.get_mut("style").expect("style schema");
    style["description"] =
        Value::String("One feature style. `id` is never configured - it is assigned from document order.".into());
    let style_props = style["properties"].as_object_mut().expect("style properties");
    style_props["z_index"]["description"] = Value::String("Painter's order; lower is drawn first.".into());
    style_props["weight"]["description"] = Value::String("Stroke width in pixels (lines only).".into());
    style_props["priority"]["description"] =
        Value::String("Chunk-overflow drop order: 1 is kept longest, 4 dropped first.".into());
    style_props["priority"]["minimum"] = Value::from(1);
    style_props["priority"]["maximum"] = Value::from(4);
    style_props["min_lod"]["description"] = Value::String(
        "Index of the coarsest LOD tier that includes this feature; it appears in every tier >= min_lod.".into(),
    );
    style_props["line_style"]["description"] =
        Value::String("Line stroke style: `solid` (the default) or `dashed`. Ignored for polygons.".into());
    style_props["fixed_width"]["description"] = Value::String(
        "Use `weight` as the on-screen stroke in device pixels, bypassing the renderer's zoom width ramp (OBCM \
         style-record flag bit 4). For a mark on the map rather than a thing with width on the ground - a contour \
         line has no ground width, so ramping it draws it thickest exactly where it does the most damage."
            .into(),
    );
    style_props["terrain_layer"]["description"] = Value::String(
        "Mark this style as part of the suppressible terrain layer (OBCM style-record flag bit 5). Written into the \
         map; the device's terrain toggle is what reads it."
            .into(),
    );
    style_props["color2"]["description"] = Value::String("Optional secondary RGB565 color; absent means none.".into());

    let lod = defs.get_mut("lod").expect("LOD schema");
    let lod_props = lod["properties"].as_object_mut().expect("LOD properties");
    lod_props["max_mpp"]["description"] =
        Value::String("Meters-per-pixel upper bound for this tier; null means the coarsest tier.".into());
    lod_props["max_mpp"]["default"] = Value::Null;
    lod_props["simplify"]["description"] =
        Value::String("Topology-preserving simplify tolerance in meters; 0 means no simplification.".into());
    lod_props["simplify"]["minimum"] = Value::from(0.0);
    lod_props["min_area_px"]["description"] = Value::String(
        "Drop polygons below this many square pixels; 0 means no culling. Ignored on the finest tier.".into(),
    );
    lod_props["min_area_px"]["minimum"] = Value::from(0.0);

    let profile = defs.get_mut("profile").expect("profile schema");
    profile["description"] = Value::String(
        "One bike profile: a display name plus per-class edge-weight multipliers. Unlisted classes use `default`."
            .into(),
    );
    let props = profile["properties"].as_object_mut().expect("profile properties");
    props["name"]["description"] =
        Value::String("Display name shown on the device (UTF-8, at most 12 bytes on the wire).".into());
    props["name"]["maxLength"] = Value::from(NAV_PROFILE_NAME_LEN);
    props["name"]["x-maxUtf8Bytes"] = Value::from(NAV_PROFILE_NAME_LEN);
    props["default"]["description"] =
        Value::String("Multiplier applied to any highway/surface class not listed below.".into());
    props["climb_weight"]["description"] = Value::String(
        "Flat metres charged per metre of ascent (OBCM v12 §8.6). 0 is climb-blind; the shipped \
         profiles use Road 10 / Gravel 8 / MTB 6 / Touring 8. Ignored unless the map was packed \
         with terrain."
            .into(),
    );
    props["climb_weight"]["minimum"] = Value::from(0);
    props["climb_weight"]["maximum"] = Value::from(u8::MAX);
    annotate_class_map(&mut props["highway"], &HIGHWAY_CLASS_NAMES, "highway");
    annotate_class_map(&mut props["surface"], &SURFACE_CLASS_NAMES, "surface");
}

fn annotate_class_map(schema: &mut Value, classes: &[&str], kind: &str) {
    schema["description"] = Value::String(format!("Per-{kind}-class multipliers, keyed by canonical class name."));
    schema["propertyNames"] = serde_json::json!({ "enum": classes });
}

/// The default `routing` section used when a config omits it: [`DEFAULT_MIN_COMPONENT_EDGES`] plus
/// [`default_profiles`].
pub fn default_routing() -> Routing {
    Routing { min_component_edges: DEFAULT_MIN_COMPONENT_EDGES, profiles: default_profiles() }
}

/// The four shipped bike profiles (Road / Gravel / MTB / Touring), quantized to §8.6's wire form.
/// Parsed from [`DEFAULT_PROFILES_JSON`] through the same path as user config, so the shipped
/// defaults and the parser can never disagree.
pub fn default_profiles() -> Vec<NavProfile> {
    let profiles: Vec<ProfileDocument> =
        serde_json::from_str(DEFAULT_PROFILES_JSON).expect("embedded default profiles are valid typed config");
    normalize_profiles(profiles).expect("embedded default profiles pass semantic validation")
}

impl RoutingDocument {
    fn normalize(self) -> Result<Routing, String> {
        let profiles = match self.profiles {
            Some(profiles) => normalize_profiles(profiles)?,
            None => default_profiles(),
        };
        Ok(Routing { min_component_edges: self.min_component_edges.unwrap_or(DEFAULT_MIN_COMPONENT_EDGES), profiles })
    }
}

/// Validate + quantize `routing.profiles`: 1..=[`NAV_MAX_PROFILES`] entries.
fn normalize_profiles(profiles: Vec<ProfileDocument>) -> Result<Vec<NavProfile>, String> {
    if profiles.is_empty() {
        return Err("routing.profiles must list at least one profile".into());
    }
    if profiles.len() > NAV_MAX_PROFILES {
        return Err(format!(
            "routing.profiles has {} entries; the OBCM profile table supports at most {NAV_MAX_PROFILES}",
            profiles.len()
        ));
    }
    profiles.into_iter().enumerate().map(|(index, profile)| profile.normalize(index)).collect()
}

impl ProfileDocument {
    /// One profile object → the §8.6 wire form. Class maps remain dynamic but
    /// typed; canonical-name validation and admissible quantization are semantic.
    fn normalize(self, index: usize) -> Result<NavProfile, String> {
        let name = self.name;
        if name.len() > NAV_PROFILE_NAME_LEN {
            return Err(format!(
                "config routing.profiles[{index}].name: {name:?} exceeds {NAV_PROFILE_NAME_LEN} UTF-8 bytes on the wire"
            ));
        }
        let default_q = match self.default_multiplier {
            None => 32u8,
            Some(multiplier) => quantize_multiplier_value(&multiplier, &name, "default", "(unlisted)")?,
        };
        let mut highway = [default_q; 32];
        let mut surface = [default_q; 8];
        for (class, val) in self.highway {
            let idx = highway_class_index(&class).ok_or_else(|| {
                format!(
                    "config routing.profiles[{index}].highway.{class}: routing profile {name:?} has unknown highway class {class:?}; valid: {}",
                    HIGHWAY_CLASS_NAMES.join(", ")
                )
            })?;
            highway[idx as usize] = quantize_multiplier_value(&val, &name, "highway", &class)?;
        }
        for (class, val) in self.surface {
            let idx = surface_class_index(&class).ok_or_else(|| {
                format!(
                    "config routing.profiles[{index}].surface.{class}: routing profile {name:?} has unknown surface class {class:?}; valid: {}",
                    SURFACE_CLASS_NAMES.join(", ")
                )
            })?;
            surface[idx as usize] = quantize_multiplier_value(&val, &name, "surface", &class)?;
        }
        debug_assert!(
            highway.iter().chain(&surface).all(|&m| m == 0 || m >= 16),
            "every non-zero multiplier must be ≥ 16 (admissible)"
        );
        // `climb_weight` needs no admissibility check: §8.6's climb term is additive and
        // non-negative, so every `u8` — including 0 — leaves the great-circle heuristic admissible.
        // `serde` has already rejected anything outside `0..=255`.
        Ok(NavProfile { name, highway, surface, climb_weight: self.climb_weight.unwrap_or(0) })
    }
}

/// Quantize one profile multiplier to §8.6's `u8` 1/16 fixed-point. `"forbidden"` ⇒ `0`; a number
/// ≥ 1.0 ⇒ `round(v × 16)` clamped to `16..=255` (≈ 1.0×..16×). A number **below 1.0 is rejected**
/// — the admissibility invariant (every non-zero weight ≥ 16) is what keeps the great-circle A*
/// heuristic admissible, so the ε-optimality bound survives profile weighting.
fn quantize_multiplier_value(v: &MultiplierValue, profile: &str, kind: &str, class: &str) -> Result<u8, String> {
    match v {
        MultiplierValue::Forbidden => Ok(0),
        MultiplierValue::Number(f) => {
            if !f.is_finite() {
                return Err(format!("routing profile {profile:?}: {kind} {class:?} multiplier {f} is not finite"));
            }
            if *f < 1.0 {
                return Err(format!(
                    "routing profile {profile:?}: {kind} {class:?} multiplier {f} is below 1.0 — every non-zero \
                     weight must be ≥ 1.0 (≥ 16 in 1/16 fixed-point) so the great-circle A* heuristic stays \
                     admissible; a weight < 1.0 lets the search underweight an edge and breaks the ε-optimality \
                    bound. Use \"forbidden\" to exclude a class."
                ));
            }
            Ok((f * 16.0).round().clamp(16.0, u8::MAX as f64) as u8)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_style(id: u8, value: &Value) -> Result<FeatureStyle, String> {
        let document: StyleDocument = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        document.normalize(id, "test", "style")
    }

    fn parse_color(value: &Value) -> Result<u16, String> {
        serde_json::from_value::<ColorValue>(value.clone()).map(|color| color.0).map_err(|e| e.to_string())
    }

    fn parse_routing(value: Option<&Value>) -> Result<Routing, String> {
        match value {
            None => Ok(default_routing()),
            Some(value) => {
                serde_json::from_value::<RoutingDocument>(value.clone()).map_err(|e| e.to_string())?.normalize()
            }
        }
    }

    fn quantize_multiplier(value: &Value, profile: &str, kind: &str, class: &str) -> Result<u8, String> {
        let value: MultiplierValue = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        quantize_multiplier_value(&value, profile, kind, class)
    }

    fn corpus_config() -> Config {
        Config::load(concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json"))
            .expect("load corpus config")
    }

    /// The shipped **schema** is a complete, CLI-usable packer config.
    ///
    /// It is the one document in `builder/presets/` that has to be: the skins beside
    /// it are presentation over already-baked bytes (`OBCC_Spec.md` §5) and carry
    /// no ladder and no routing table on purpose, so "every file in the directory is
    /// a bakeable config" stopped being true with #1036 and this checks the file that
    /// still is.
    #[test]
    fn the_shipped_schema_is_a_complete_cli_config() {
        let path = format!("{}/../../builder/presets/schema.json", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load(&path).unwrap_or_else(|error| panic!("the schema must parse: {error}"));
        assert!(!config.features.is_empty(), "the schema must carry feature styles");
        assert!(!config.lods.is_empty(), "the schema must carry an LOD pyramid");
        assert!(!config.routing.profiles.is_empty(), "the schema must carry routing profiles");
    }

    #[test]
    fn typed_errors_retain_the_failing_config_path() {
        let color = Config::parse(r#"{"features":{"highway":{"primary":{"color":70000}}}}"#)
            .err()
            .expect("out-of-range color must fail");
        assert!(color.contains("features.highway.primary.color"), "path missing from: {color}");
        assert!(color.contains("0..=65535"), "range missing from: {color}");

        let profile = Config::parse(r#"{"routing":{"profiles":[{"name":"Bad","highway":[]}]}}"#)
            .err()
            .expect("non-object class map must fail");
        assert!(profile.contains("routing.profiles[0].highway"), "path missing from: {profile}");
        assert!(profile.contains("map"), "expected-type detail missing from: {profile}");
    }

    #[test]
    fn unknown_tooling_metadata_remains_compatible() {
        let config = Config::parse(
            r#"{
                "_meta":{"id":"custom"}, "disabled":["highway/path"], "future_root":true,
                "features":{"highway":{"path":{"color":"0x1234","id":99,"editor_note":"kept outside packer"}}}
            }"#,
        )
        .expect("unknown root/style metadata remains ignored");
        assert_eq!(config.styles().len(), 1);
        assert_eq!(config.styles()[0].id, 1, "configured id remains ignored in favor of document order");
    }

    #[test]
    fn null_defaults_and_required_colors_match_legacy_behavior() {
        let config = Config::parse(
            r#"{
                "lods": null, "marker": null, "chunk_size": null,
                "features":{"highway":{"path":{"color":"0x1234","weight":null,"line_style":null}}},
                "routing":{"min_component_edges":null,"profiles":null}
            }"#,
        )
        .expect("legacy null-as-default fields remain accepted");
        assert_eq!(config.lods.len(), 1);
        assert_eq!(config.features[0].1["path"].weight, 1);
        assert_eq!(config.routing, default_routing());

        assert!(Config::parse(r#"{"marker":{"color":null}}"#).is_err(), "a present marker color is not nullable");
        assert!(
            Config::parse(r#"{"features":{"highway":{"path":{"color":"0x1","color2":null}}}}"#).is_err(),
            "a present secondary color is not nullable"
        );
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
        assert_eq!(id("building", "*"), Some(29)); // the default preset's building catch-all
        assert_eq!(id("natural", "land"), Some(31));
        assert_eq!(id("admin_level", "2"), Some(50));
        // The synthetic contour classes are appended last on purpose: every OSM style above keeps
        // the id it had before contours existed.
        assert_eq!(id("contour", "major"), Some(51));
        assert_eq!(id("contour", "index"), Some(52)); // last value in the document

        // Every id is unique and within 1..=254.
        let mut ids: Vec<u8> = cfg.styles().iter().map(|s| s.id).collect();
        ids.sort_unstable();
        assert_eq!(ids.first(), Some(&1));
        assert_eq!(ids.last(), Some(&52));
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "style ids must be unique");
    }

    #[test]
    fn lods_marker_chunk_parsed() {
        let cfg = corpus_config();
        // The default preset's 7-tier pyramid (coarsest first, max_mpp
        // 30/16/10/5/3/1.2): the coarse tiers carry a footprint cull
        // (min_area_px) and the finest tier a small sub-pixel simplify (0.5 m)
        // that trims road vertices with no visible change.
        assert_eq!(cfg.lods.len(), 7);
        assert_eq!(cfg.lods[0].max_mpp, None);
        assert_eq!(cfg.lods[0].simplify_m, 200.0);
        assert_eq!(cfg.lods[0].min_area_px, 50.0);
        assert_eq!(cfg.lods[1].max_mpp, Some(30.0));
        assert_eq!(cfg.lods[6].simplify_m, 0.5);
        assert_eq!(cfg.lods[6].min_area_px, 0.0);
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

    /// A `"*"` value entry is a per-category catch-all: an exact value match still
    /// wins, but any other value the key carries falls back to `*`. A key with no
    /// `*` and no exact match stays unstyled (unchanged behaviour).
    #[test]
    fn wildcard_value_is_a_catch_all() {
        let text = r#"{"features":{"building":{
            "warehouse":{"color":"0x0001"},
            "*":{"color":"0x0002"}
        }}}"#;
        let cfg = Config::parse(text).expect("wildcard config parses");

        let style_for = |val: &str| {
            let mut tags = HashMap::new();
            tags.insert("building", val);
            cfg.get_style(&tags).map(|s| s.color)
        };
        assert_eq!(style_for("warehouse"), Some(0x0001), "exact match beats the catch-all");
        assert_eq!(style_for("house"), Some(0x0002), "an unlisted value falls back to *");
        assert_eq!(style_for("yes"), Some(0x0002), "so does every other building value");

        // No `*` in a category ⇒ an unlisted value is still unstyled.
        let no_star = Config::parse(r#"{"features":{"building":{"yes":{"color":"0x0001"}}}}"#).unwrap();
        let mut tags = HashMap::new();
        tags.insert("building", "house");
        assert!(no_star.get_style(&tags).is_none(), "without a catch-all, an unlisted value is dropped");
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

    // --- schema pinning: `obc-pack schema` derives structure/defaults from the
    // typed parser model. The checked-in web fallback must never drift from it. ---

    fn embedded_schema() -> Value {
        config_schema()
    }

    #[test]
    fn checked_in_schema_is_current_generated_schema() {
        let checked_in: Value = serde_json::from_str(CONFIG_SCHEMA_JSON).expect("checked-in schema is valid JSON");
        assert_eq!(checked_in, config_schema(), "schema/config.schema.json is stale; regenerate with `cargo run -p obc-pack --bin obc-pack -- schema --config > host/obc-pack/schema/config.schema.json`");
    }

    #[test]
    fn typed_defaults_normalize_to_the_public_config() {
        let parsed = parse_style(1, &serde_json::json!({"color": "0x0001"})).expect("minimal style");
        assert_eq!((parsed.z_index, parsed.weight, parsed.priority, parsed.min_lod), (0, 1, 3, 0));
        assert_eq!(parsed.line_style, LineStyle::Solid);
        assert!(parsed.color2.is_none());

        let cfg = Config::parse("{}").expect("empty config parses");
        assert_eq!((cfg.marker_color, cfg.chunk_size), (DEFAULT_MARKER_COLOR, DEFAULT_CHUNK_SIZE));
        assert_eq!(cfg.lods.len(), 1);
        assert_eq!((cfg.lods[0].max_mpp, cfg.lods[0].simplify_m, cfg.lods[0].min_area_px), (None, 0.0, 0.0));
        assert!(!cfg.merge_fills && !cfg.merge_lines);
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

    /// `color2` is the optional secondary color: parsed exactly like `color` (hex string or int),
    /// referencing the same schema `$def`; absent ⇒ `None`, over-range ⇒ error.
    #[test]
    fn schema_color2_parses_like_color() {
        let schema = embedded_schema();
        assert_eq!(
            schema["$defs"]["style"]["properties"]["color2"]["$ref"].as_str(),
            Some("#/$defs/color"),
            "color2 reuses the color $def"
        );
        assert_eq!(
            parse_style(1, &serde_json::json!({"color": "0x0001", "color2": "0x8410"})).unwrap().color2,
            Some(0x8410)
        );
        assert_eq!(parse_style(1, &serde_json::json!({"color": "0x0001", "color2": 31})).unwrap().color2, Some(31));
        assert_eq!(parse_style(1, &serde_json::json!({"color": "0x0001"})).unwrap().color2, None, "absent ⇒ None");
        assert!(
            parse_style(1, &serde_json::json!({"color": "0x0001", "color2": "0x12345"})).is_err(),
            "5 hex digits overflow u16, exactly like color"
        );
    }

    /// `line_style` accepts only `"solid"` (default) / `"dashed"`.
    #[test]
    fn line_style_enum_is_typed() {
        assert_eq!(parse_style(1, &serde_json::json!({"color": "0x1"})).unwrap().line_style, LineStyle::Solid);
        assert_eq!(
            parse_style(1, &serde_json::json!({"color": "0x1", "line_style": "solid"})).unwrap().line_style,
            LineStyle::Solid
        );
        assert_eq!(
            parse_style(1, &serde_json::json!({"color": "0x1", "line_style": "dashed"})).unwrap().line_style,
            LineStyle::Dashed
        );
        assert!(parse_style(1, &serde_json::json!({"color": "0x1", "line_style": "dotted"})).is_err());
    }

    /// #1095's two style-record flag bits are typed optional booleans that default **off**, are
    /// declared in the schema (so the builder's editor offers them rather than lying), and land on
    /// the wire as bits 4 and 5 of the style record's `Flags` byte. Bits 6-7 stay written `0`.
    #[test]
    fn schema_style_flag_bits_match_parser_and_serializer() {
        let schema = embedded_schema();
        for key in ["fixed_width", "terrain_layer"] {
            let prop = &schema["$defs"]["style"]["properties"][key];
            assert_eq!(prop["default"], Value::Bool(false), "{key} defaults off in the schema");
            assert!(prop["description"].as_str().is_some_and(|d| d.contains("flag bit")), "{key} names its wire bit");
        }

        // Parser: absent/null ⇒ off, `true` ⇒ on.
        let plain = parse_style(1, &serde_json::json!({"color": "0x1"})).unwrap();
        assert!(!plain.fixed_width && !plain.terrain_layer, "absent ⇒ both off");
        let null = parse_style(1, &serde_json::json!({"color": "0x1", "fixed_width": null, "terrain_layer": null}));
        let null = null.unwrap();
        assert!(!null.fixed_width && !null.terrain_layer, "null ⇒ both off");
        let both = parse_style(1, &serde_json::json!({"color": "0x1", "fixed_width": true, "terrain_layer": true}))
            .expect("both flags parse");
        assert!(both.fixed_width && both.terrain_layer);
        assert!(parse_style(1, &serde_json::json!({"color": "0x1", "fixed_width": "yes"})).is_err());

        // Serializer: bit 4 and bit 5 of the flags byte, nothing else disturbed (record 0's flags
        // sit at offset 5 behind the one-byte count).
        let flags = |style: &FeatureStyle| crate::serialize::pack_style_dict(&[style.to_style()])[1 + 5];
        assert_eq!(flags(&plain) & 0x30, 0x00, "neither bit set by default");
        assert_eq!(flags(&both) & 0x30, 0x30, "both bits set");
        let fixed = parse_style(1, &serde_json::json!({"color": "0x1", "fixed_width": true})).unwrap();
        assert_eq!(flags(&fixed) & 0x30, obc_formats::obcm::STYLE_FIXED_WIDTH_BIT);
        let terrain = parse_style(1, &serde_json::json!({"color": "0x1", "terrain_layer": true})).unwrap();
        assert_eq!(flags(&terrain) & 0x30, obc_formats::obcm::STYLE_TERRAIN_LAYER_BIT);
        assert_eq!(flags(&both) & obc_formats::obcm::STYLE_RESERVED_MASK, 0, "bits 6-7 stay written 0");
    }

    /// The schema advertises the `"*"` catch-all in the `features` description, and the parser
    /// actually honours it — so the web builder's editor can offer a catch-all row without lying.
    #[test]
    fn schema_documents_wildcard_catch_all() {
        let schema = embedded_schema();
        let desc = schema["properties"]["features"]["description"].as_str().expect("features description");
        assert!(desc.contains("\"*\""), "the features description must document the * catch-all: {desc}");

        // And the behaviour the description promises holds.
        let cfg = Config::parse(r#"{"features":{"building":{"*":{"color":"0x0002"}}}}"#).unwrap();
        let mut tags = HashMap::new();
        tags.insert("building", "anything");
        assert_eq!(cfg.get_style(&tags).map(|s| s.color), Some(0x0002));
    }

    /// Merge switches are typed optional booleans; absent/null/false are off.
    #[test]
    fn merge_switches_are_typed_and_default_off() {
        let cfg = Config::parse(r#"{"merge_fills": true, "merge_lines": null}"#).unwrap();
        assert!(cfg.merge_fills);
        assert!(!cfg.merge_lines);
        assert!(Config::parse(r#"{"merge_fills": "yes"}"#).is_err());
    }

    #[test]
    fn schema_envelope_shape() {
        let env: Value = serde_json::from_str(&schema_envelope()).expect("envelope is valid JSON");
        assert_eq!(env["schema_version"].as_u64(), Some(CONFIG_SCHEMA_VERSION as u64));
        assert_eq!(env["format_version"].as_u64(), Some(OBCM_VERSION as u64));
        assert_eq!(env["format_version"].as_u64(), Some(13), "exact-edge anchors bump OBCM to v13");
        assert!(env["schema"]["$defs"]["style"].is_object(), "envelope embeds the schema");
    }

    // --- contours section (EL10a, #1094) -------------------------------------------------------

    fn parse_contours(text: &str) -> Result<Contours, String> {
        Config::parse(text).map(|c| c.contours)
    }

    /// An omitted (or null-valued) `contours` section is off, and carries the shipped ladder so
    /// flipping `enabled` alone is a complete request.
    #[test]
    fn contours_default_to_off_with_the_shipped_ladder() {
        let cfg = Config::parse("{}").expect("empty config parses");
        assert_eq!(cfg.contours, Contours::default());
        assert!(!cfg.contours.enabled, "a config that says nothing about contours packs none");
        assert_eq!((cfg.contours.interval_m, cfg.contours.index_every), (100, 5));
        assert_eq!(cfg.contours.simplify_m, 15.0, "the 15 m clamp is the default, not a knob to discover");

        let explicit = parse_contours(r#"{"contours":{"enabled":true,"interval":null,"simplify":null}}"#)
            .expect("null fields fall back to the defaults");
        assert_eq!(explicit, Contours { enabled: true, ..Contours::default() });
    }

    /// The values that would make a trace meaningless are rejected at parse time, not survived into
    /// a division or an infinite level ladder.
    #[test]
    fn contours_reject_degenerate_ladders() {
        assert_eq!(parse_contours(r#"{"contours":{"interval":1}}"#).map(|c| c.interval_m), Ok(1));
        assert!(parse_contours(r#"{"contours":{"interval":0}}"#).is_err(), "a 0 m interval must error");
        assert!(parse_contours(r#"{"contours":{"index_every":0}}"#).is_err(), "index_every 0 must error");
        assert_eq!(parse_contours(r#"{"contours":{"index_every":1}}"#).map(|c| c.index_every), Ok(1));
        assert!(parse_contours(r#"{"contours":{"simplify":-1}}"#).is_err(), "a negative clamp must error");
        assert_eq!(parse_contours(r#"{"contours":{"simplify":0}}"#).map(|c| c.simplify_m), Ok(0.0));
        assert!(parse_contours(r#"{"contours":{"interval":"100"}}"#).is_err(), "the interval is a number");
    }

    /// The two classes are ordinary style rules under a synthetic tag key, looked up **exactly** —
    /// a `"*"` catch-all written for a real OSM key must never end up styling terrain.
    #[test]
    fn contour_classes_are_styled_independently() {
        let cfg = Config::parse(
            r#"{"features":{"contour":{"index":{"color":"0xAD55","min_lod":3}},
                            "building":{"*":{"color":"0x0001"}}}}"#,
        )
        .expect("a config styling one contour class parses");
        assert!(cfg.contour_style(ContourClass::Major).is_none(), "an unstyled class is not packed");
        let index = cfg.contour_style(ContourClass::Index).expect("index is styled");
        assert_eq!((index.color, index.min_lod), (0xAD55, 3));
        assert!(
            cfg.feature_style("building", "yes").is_none(),
            "the generated-feature lookup is exact — no catch-all fallback"
        );
    }

    /// The shipped preset ships E3 (#1095): both classes styled, all weight 1, `major` dashed and
    /// `index` solid — the emphasis is continuity, not mass — both off the width ramp and both
    /// tagged terrain, and the block itself **on**. Every one of those was argued from a rendered
    /// frame, so each is pinned rather than left to a re-read of the JSON.
    ///
    /// Both classes reach **LOD 2** (#1104), one tier above the planning tier (LOD 3) where #1095
    /// first put them; LODs 0–1 stay contour-free. The reach is the same number for both on purpose:
    /// index-only at LOD 2 was tried and rejected — solid grey lines with no dashes around them read
    /// as paths, because emphasis-by-continuity only means anything while the dashes are present
    /// (Timo's on-glass pick, 2026-08-03).
    #[test]
    fn the_shipped_schema_carries_both_contour_classes() {
        let cfg = corpus_config();
        for class in [ContourClass::Major, ContourClass::Index] {
            let style = cfg.contour_style(class).unwrap_or_else(|| panic!("{class:?} must be styled"));
            assert_eq!(style.weight, 1, "every contour is authored weight 1");
            assert_eq!(style.color, 0xAD55, "one grey, never a second colour");
            assert!(style.fixed_width, "a contour has no width on the ground — it is off the ramp");
            assert!(style.terrain_layer, "and it is what the device's terrain toggle suppresses");
            assert_eq!(
                style.min_lod, 2,
                "#1104: {class:?} reaches LOD 2 — index-only at LOD 2 read as paths, so both classes \
                 travel together (Timo's on-glass pick 2026-08-03)"
            );
        }
        assert_eq!(cfg.contour_style(ContourClass::Major).unwrap().line_style, LineStyle::Dashed);
        assert_eq!(
            cfg.contour_style(ContourClass::Index).unwrap().line_style,
            LineStyle::Solid,
            "index is the solid one"
        );
        assert_eq!(cfg.contours, Contours { enabled: true, interval_m: 100, index_every: 5, simplify_m: 15.0 });
    }

    /// The schema's `contours` default parses back to the code default, and its bounds are the
    /// parser's — the editor must not offer a ladder the packer rejects.
    #[test]
    fn schema_contours_default_matches_parser() {
        let schema = embedded_schema();
        let contours = &schema["properties"]["contours"];
        let default = serde_json::json!({ "contours": contours["default"].clone() });
        assert_eq!(
            parse_contours(&default.to_string()).expect("schema contours default parses"),
            Contours::default(),
            "the schema default must equal the code default"
        );
        let props = &contours["properties"];
        assert_eq!(props["enabled"]["default"], Value::Bool(false));
        assert_eq!(props["interval"]["minimum"].as_u64(), Some(1));
        assert_eq!(props["index_every"]["minimum"].as_u64(), Some(1));
        assert_eq!(props["simplify"]["minimum"].as_f64(), Some(0.0));
        assert_eq!(props["simplify"]["default"].as_f64(), Some(15.0));
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
        // v12 climb weight: a plain u8, bounds stated so the editor offers the same range the
        // packer accepts, and absent ⇒ 0 (climb-blind).
        let climb = &schema["$defs"]["profile"]["properties"]["climb_weight"];
        assert_eq!(climb["minimum"].as_u64(), Some(0));
        assert_eq!(climb["maximum"].as_u64(), Some(u8::MAX as u64));
        assert_eq!(climb["default"].as_u64(), Some(0));
    }

    /// The v12 §8.6 climb weight, end to end through the config: the four shipped profiles carry
    /// the seeded values, an explicit one is taken verbatim, and an omitted one is climb-blind
    /// rather than inherited — a config that says nothing about climbing must route exactly as it
    /// did before terrain existed.
    #[test]
    fn climb_weight_is_seeded_taken_verbatim_and_zero_when_unstated() {
        let shipped = default_profiles();
        let weights: Vec<u8> = shipped.iter().map(|p| p.climb_weight).collect();
        assert_eq!(weights, vec![10, 8, 6, 8], "Road / Gravel / MTB / Touring");
        assert_eq!(shipped.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Road", "Gravel", "MTB", "Touring"]);

        let cfg = Config::parse(r#"{"routing":{"profiles":[{"name":"Steep","climb_weight":255},{"name":"Blind"}]}}"#)
            .expect("both profiles parse");
        assert_eq!(cfg.routing.profiles[0].climb_weight, 255, "the maximum weight is legal — the term is additive");
        assert_eq!(cfg.routing.profiles[1].climb_weight, 0, "unstated is climb-blind, not inherited");

        // Unlike a multiplier there is no admissibility floor to fall below, so nothing here can be
        // rejected for being too small; a value outside u8 is a *type* error from serde.
        assert!(
            Config::parse(r#"{"routing":{"profiles":[{"name":"X","climb_weight":256}]}}"#).is_err(),
            "256 does not fit the u8 wire field"
        );
    }
}
