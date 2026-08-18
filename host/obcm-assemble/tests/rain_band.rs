//! The rain band-gap contract (WX10, epic #1185): the packer schema and every shipped skin keep
//! the open z interval (`RAIN_BAND_GAP_LOW`, `RAIN_BAND_GAP_HIGH`) empty and never move a feature
//! type across the `RAIN_BELOW_Z` boundary — the invariant that makes "roads render above
//! precipitation" hold for every stamped map. Pins the shipped preset documents byte-for-byte and
//! the two enforcement points (skin resolve, image restamp).

use obc_map_scene::{RAIN_BAND_GAP_HIGH, RAIN_BAND_GAP_LOW, RAIN_BELOW_Z};
use obcm_assemble::schema::{Schema, Skin, SkinStyle, StyleId, StyleRecord};
use obcm_assemble::emit::{pack_style_table, restamp_style_table, RestampError};
use serde_json::Value;

const SCHEMA_JSON: &str = include_str!("../../../builder/presets/schema.json");
const DEFAULT_SKIN_JSON: &str = include_str!("../../../builder/presets/skins/default.json");
const DUSK_SKIN_JSON: &str = include_str!("../../../builder/presets/skins/dusk.json");

/// Every `(group.name, z_index)` pair in a preset document's `features` tree.
fn feature_z(doc: &str) -> Vec<(String, i64)> {
    let value: Value = serde_json::from_str(doc).expect("preset parses");
    let features = value.get("features").and_then(Value::as_object).expect("preset has features");
    let mut out = Vec::new();
    for (group, entries) in features {
        let entries = entries.as_object().expect("feature group is an object");
        for (name, style) in entries {
            let z = style.get("z_index").and_then(Value::as_i64).expect("style has z_index");
            out.push((format!("{group}.{name}"), z));
        }
    }
    assert!(!out.is_empty());
    out
}

/// The shipped presets keep the rain band gap empty — no style of the schema or either skin sits
/// strictly inside `(RAIN_BAND_GAP_LOW, RAIN_BAND_GAP_HIGH)`.
#[test]
fn shipped_presets_keep_the_band_gap_empty() {
    for (label, doc) in [("schema", SCHEMA_JSON), ("default", DEFAULT_SKIN_JSON), ("dusk", DUSK_SKIN_JSON)] {
        for (feature, z) in feature_z(doc) {
            assert!(
                z <= RAIN_BAND_GAP_LOW as i64 || z >= RAIN_BAND_GAP_HIGH as i64,
                "{label}: {feature} z_index {z} sits inside the reserved rain band gap ({}, {})",
                RAIN_BAND_GAP_LOW,
                RAIN_BAND_GAP_HIGH
            );
        }
    }
}

/// Neither shipped skin moves any feature type across the rain boundary relative to the schema:
/// what is ground in the schema stays below `RAIN_BELOW_Z` in every skin, and what is road band
/// stays at or above it.
#[test]
fn shipped_skins_agree_with_the_schema_on_boundary_sides() {
    let schema: std::collections::BTreeMap<String, i64> = feature_z(SCHEMA_JSON).into_iter().collect();
    for (label, doc) in [("default", DEFAULT_SKIN_JSON), ("dusk", DUSK_SKIN_JSON)] {
        for (feature, z) in feature_z(doc) {
            let Some(&schema_z) = schema.get(&feature) else {
                panic!("{label}: {feature} is not in the schema");
            };
            assert_eq!(
                z >= RAIN_BELOW_Z as i64,
                schema_z >= RAIN_BELOW_Z as i64,
                "{label}: {feature} crosses the rain boundary (schema z {schema_z}, skin z {z})"
            );
        }
    }
}

fn style(id: u8, z: i8) -> SkinStyle {
    SkinStyle {
        id: Some(id),
        feature_type: None,
        color: 0x1234,
        weight: 1,
        z_index: z,
        priority: 1,
        dashed: false,
        fixed_width: false,
        terrain_layer: false,
        color2: None,
    }
}

fn bare_schema() -> Schema {
    // An id-less schema: the skin states its own ids, exactly the assembler's fallback path.
    let mut schema: Schema = serde_json::from_str(SCHEMA_TEMPLATE).expect("template parses");
    schema.styles = Vec::<StyleId>::new();
    schema
}

/// A minimal OBCC schema document for the resolve-path tests (bands/lods are irrelevant here).
const SCHEMA_TEMPLATE: &str = r#"{
    "obcm_version": 13,
    "lods": [],
    "bands": [],
    "styles": [],
    "routing": { "min_component_edges": 1, "profiles": ["Road"] },
    "chunk_size": 4096
}"#;

/// `Skin::resolve` refuses a style parked inside the reserved gap.
#[test]
fn resolve_refuses_a_style_inside_the_gap() {
    let schema = bare_schema();
    let ok = Skin { id: "t".into(), name: "t".into(), marker_color: 0, styles: vec![style(1, 10), style(2, 30)] };
    assert!(ok.resolve(&schema).is_ok());
    for z in (RAIN_BAND_GAP_LOW + 1)..RAIN_BAND_GAP_HIGH {
        let bad = Skin { id: "t".into(), name: "t".into(), marker_color: 0, styles: vec![style(1, z)] };
        let err = bad.resolve(&schema).unwrap_err();
        assert!(err.contains("rain band gap"), "z {z}: {err}");
    }
}

/// `restamp_style_table` refuses a skin that moves a style across the boundary relative to the
/// image it stamps onto — in either direction — and accepts same-side restyling.
#[test]
fn restamp_refuses_boundary_crossings() {
    let record = |id: u8, z: i8| StyleRecord {
        id,
        z_index: z,
        color: 0xAAAA,
        weight: 1,
        priority: 1,
        dashed: false,
        color2: None,
        fixed_width: false,
        terrain_layer: false,
    };
    // A minimal image: header up to the style offset, then a two-style table (ground z 10, road
    // z 30). Only the header fields restamp reads need to be real — and since v14 that includes the
    // `Offset Scale` byte, because `Style Offset` is a **unit** count and means nothing without it.
    let baked = [record(1, 10), record(2, 30)];
    let table = pack_style_table(&baked);
    let at = obcm_assemble::emit::STYLE_OFFSET as usize;
    let mut image = vec![0u8; at + table.len()];
    image[obcm_assemble::emit::HEADER_STYLE_OFFSET_AT..obcm_assemble::emit::HEADER_STYLE_OFFSET_AT + 4]
        .copy_from_slice(&obcm_assemble::emit::scaled(at as u64).expect("a unit boundary").to_le_bytes());
    image[obc_formats::obcm::HEADER_OFFSET_SCALE_OFF] = obcm_assemble::emit::SCALE.log2();
    image[at..].copy_from_slice(&table);

    // Same-side restyle (colors, weights, even z shifts inside each side): accepted.
    let restyle = [record(1, 4), record(2, 60)];
    assert!(restamp_style_table(&mut image.clone(), &restyle, 0xF800).is_ok());

    // Pulling the road under the boundary: refused.
    let road_under = [record(1, 10), record(2, 10)];
    match restamp_style_table(&mut image.clone(), &road_under, 0xF800) {
        Err(RestampError::RainBandMoved { id: 2, from: 30, to: 10 }) => {}
        other => panic!("road-under-rain restamp must be refused, got {other:?}"),
    }

    // Lifting a ground fill above the boundary: refused too.
    let ground_over = [record(1, 30), record(2, 30)];
    match restamp_style_table(&mut image.clone(), &ground_over, 0xF800) {
        Err(RestampError::RainBandMoved { id: 1, from: 10, to: 30 }) => {}
        other => panic!("ground-over-rain restamp must be refused, got {other:?}"),
    }
}
