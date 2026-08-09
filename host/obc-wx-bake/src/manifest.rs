//! The mutable `wx/v1/manifest.json`: the one document a weather client reads first.
//!
//! Everything a client needs to select a product, plan corridor Range reads and verify what it
//! fetched is here — tier, bbox, per-frame geometry/paging, keys, byte lengths, object CRCs,
//! staleness and attribution. The JSON Schema is generated from these structs (schemars, the
//! obc-pack config-schema discipline) and pinned by a test against the checked-in
//! `schema/manifest.schema.json`.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const MANIFEST_KEY: &str = "wx/v1/manifest.json";
pub const MANIFEST_VERSION: u32 = 1;

/// Canonical UTC second formatting for every manifest timestamp.
pub fn rfc3339(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|time| time.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| format!("invalid-{unix}"))
}

pub fn parse_rfc3339(text: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp())
}

/// The `<generated-utc>` key segment: the upstream reference time, minute precision.
pub fn key_timestamp(unix: i64) -> String {
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|time| time.format("%Y%m%dT%H%MZ").to_string())
        .unwrap_or_else(|| format!("invalid-{unix}"))
}

pub fn frame_key(product_id: &str, reference_unix: i64, offset_min: u32) -> String {
    format!("wx/v1/{product_id}/{}/f{offset_min}.obcg", key_timestamp(reference_unix))
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Manifest document version; readers reject an unknown value.
    pub version: u32,
    /// Wall-clock UTC time this manifest was produced (RFC 3339 seconds).
    pub generated_at: String,
    pub products: Vec<Product>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Product {
    /// Stable product identifier (`dwd-rv`, `icon-eu`, ...). Selection data, not provider UI.
    pub id: String,
    /// 1 radar, 2 model, 3 floor — the client prefers the highest fresh tier covering a corridor.
    pub tier: u8,
    /// The region where the whole product timeline is answerable: the intersection of its
    /// frames' windows, integer microdegrees.
    pub bbox_udeg: Bbox,
    /// Nominal lattice of the product's frames (frames restate their own exact geometry).
    pub cell: Cell,
    /// Upstream run/reference time (RFC 3339); also the immutable key segment, so a repeated
    /// bake of the same run overwrites objects with identical bytes.
    pub reference_time: String,
    /// When this product entry was baked (RFC 3339).
    pub generated_at: String,
    /// The moment the product must stop being used if no fresh manifest replaced it (RFC 3339).
    /// Expiry never turns into a dry claim.
    pub staleness_deadline: String,
    pub attribution: AttributionEntry,
    /// Upstream HTTP validator of the source object this entry was baked from, when the source
    /// offers one; the next cycle short-circuits on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_etag: Option<String>,
    pub frames: Vec<Frame>,
}

impl Product {
    pub fn reference_unix(&self) -> Option<i64> {
        parse_rfc3339(&self.reference_time)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Bbox {
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub north_udeg: i64,
    pub east_udeg: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Cell {
    pub lat_udeg: u32,
    pub lon_udeg: u32,
    /// Nominal source ground resolution in metres, for truthful UI/selection.
    pub nominal_m: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AttributionEntry {
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    /// `(valid_at - reference_time)` in minutes; the `f<offset-min>` key segment.
    pub offset_min: u32,
    /// Real upstream UTC validity timestamp (RFC 3339) — never a re-stamped fetch time.
    pub valid_at: String,
    pub source_class: SourceClass,
    /// Immutable object key under the service origin.
    pub key: String,
    /// Exact object length; a client may use it to bound Range arithmetic.
    pub bytes: u64,
    /// The OBCG whole-object CRC-32 (`0x` + 8 uppercase hex digits).
    pub object_crc32: String,
    /// The frame's exact OBCG geometry and paging, restated so corridor page arithmetic is
    /// plannable from the manifest and verifiable against the fetched header.
    pub geometry: FrameGeometry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceClass {
    Observation,
    Forecast,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameGeometry {
    pub south_udeg: i32,
    pub west_udeg: i32,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub cell_size_m: u16,
    pub tile_edge: u16,
    pub entries_per_page: u16,
}

/// Stable pretty serialization: struct field order is declaration order, so the same manifest
/// content is always the same bytes (the byte-stable-cycle contract).
pub fn to_json(manifest: &Manifest) -> String {
    let mut text = serde_json::to_string_pretty(manifest).expect("manifest serializes");
    text.push('\n');
    text
}

pub fn from_json(bytes: &[u8]) -> Result<Manifest, String> {
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|error| format!("manifest parse: {error}"))?;
    if manifest.version != MANIFEST_VERSION {
        return Err(format!("manifest version {} is not {MANIFEST_VERSION}", manifest.version));
    }
    Ok(manifest)
}

/// The generated JSON Schema for the manifest document.
pub fn schema() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(Manifest)).expect("manifest schema serializes")
}

/// Stable pretty schema text used to regenerate the checked-in file:
/// `cargo run -p obc-wx-bake -- schema > host/obc-wx-bake/schema/manifest.schema.json`.
pub fn schema_json() -> String {
    let mut text = serde_json::to_string_pretty(&schema()).expect("manifest schema serializes");
    text.push('\n');
    text
}

pub const CHECKED_IN_SCHEMA: &str = include_str!("../schema/manifest.schema.json");

#[cfg(test)]
mod tests {
    use super::*;

    /// The obc-pack schema discipline: the checked-in schema must equal the generated one, or
    /// the builder/phone-facing document lies about what this binary writes.
    #[test]
    fn checked_in_schema_is_current() {
        let checked_in: serde_json::Value =
            serde_json::from_str(CHECKED_IN_SCHEMA).expect("schema/manifest.schema.json parses");
        assert_eq!(
            checked_in,
            schema(),
            "schema/manifest.schema.json is stale; regenerate with `cargo run -p obc-wx-bake -- schema > host/obc-wx-bake/schema/manifest.schema.json`"
        );
    }

    #[test]
    fn timestamps_round_trip_canonically() {
        assert_eq!(rfc3339(1_800_000_000), "2027-01-15T08:00:00Z");
        assert_eq!(parse_rfc3339("2027-01-15T08:00:00Z"), Some(1_800_000_000));
        assert_eq!(key_timestamp(1_800_000_000), "20270115T0800Z");
        assert_eq!(frame_key("dwd-rv", 1_800_000_000, 15), "wx/v1/dwd-rv/20270115T0800Z/f15.obcg");
    }
}
