//! **v13 through the assemble path** (#1105) — the two facts a restyled cell tree must not lose.
//!
//! A contour says how high it is (a §5.2 `int16` behind its feature header) and its style says it is
//! an index contour (a §2 flag bit). The cell path touches both: the cutter clips the features into
//! cells, the assembler grafts the chunk bytes back and **rewrites the style table from the skin**.
//! So the level rides through as opaque geometry bytes — which is the easy half — while the flag bit
//! is *recomputed* on every assembly, and a skin document that does not mention it (every skin
//! written before v13, and every hand-rolled one after) would silently clear it.
//!
//! That is why bit 6 is derived from the **feature type** rather than read off the skin
//! (`Skin::resolve`), and this is the end-to-end proof: the shipped `default` skin — which says
//! nothing about contours beyond their colour — stamped onto a cell tree that has them.

use std::path::PathBuf;

use obc_formats::obcm::CONTOUR_INDEX_FEATURE_TYPE;
use obc_pack::catalog::feature_type_ids;
use obc_pack::config::{Config, ContourClass, LineStyle};
use obc_pack::cut::{cut_ingested, CutOptions, SourceExtent};
use obc_pack::geom::Geom;
use obc_pack::grid::BandTable;
use obc_pack::ingest::{IngestFeature, Ingested};
use obc_pack::progress::Progress;
use obc_reader::{MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};
use obcm_assemble::grid::CellId;
use obcm_assemble::schema::{LodEntry, Routing, Schema, Skin, StyleId};
use obcm_assemble::{assemble, CellInput, MemorySource, MemoryStore, NoClock, Options};

/// The shipped schema and the shipped look — the documents a real hosted assembly is driven by, so
/// the test cannot pass by inventing a skin that happens to carry what the engine needs.
const SCHEMA: &str = include_str!("../../../builder/presets/schema.json");
const DEFAULT_SKIN: &str = include_str!("../../../builder/presets/skins/default.json");

/// A band table over the shipped 7-tier ladder. Cell sizes follow the real one (coarse 2^20 for the
/// planning tiers, finer bands below); only the grouping is simplified, because what is under test
/// is the style table and the feature bytes, not the tiling.
const BANDS: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 20, "lods": [0, 1, 2],    "role": "coarse"},
    {"id": "mid",     "cell_log2": 19, "lods": [3, 4],       "role": "geometry"},
    {"id": "fine",    "cell_log2": 18, "lods": [5, 6],       "role": "geometry"},
    {"id": "network", "cell_log2": 18, "lods": [],           "sections": ["nav", "poi"], "role": "core"}
]}"#;

/// Elevations for the two traced classes. Both are multiples of the shipped 100 m interval, and the
/// index one is a multiple of 500 m (`index_every: 5`) — i.e. what a real trace would emit.
const INDEX_LEVEL: i16 = 2500;
const MAJOR_LEVEL: i16 = 2400;

fn deg(udeg: i64) -> f64 {
    udeg as f64 / 1e6
}

/// A contour: a line at `level`, long enough to survive the ladder's coarse simplify.
fn contour(style_id: u8, min_lod: usize, level: i16, lat: i64) -> IngestFeature {
    let pts: Vec<(f64, f64)> =
        (0..5).map(|k| (deg(7_600_000 + k * 3_000), deg(lat + if k % 2 == 0 { 0 } else { 400 }))).collect();
    IngestFeature { style_id, min_lod, level: Some(level), geom: Geom::Line(pts) }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obcm-assemble-contours-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// The engine's schema for the shipped config, **carrying the canonical id assignment** — which is
/// what a hosted catalog publishes, and what the derivation reads the feature type out of.
fn schema(cfg: &Config) -> Schema {
    let bands = serde_json::from_value(serde_json::from_str::<serde_json::Value>(BANDS).unwrap()["bands"].clone())
        .expect("bands parse into the engine's own table");
    let mut styles: Vec<StyleId> =
        feature_type_ids(cfg).into_iter().map(|(feature_type, id)| StyleId { id, feature_type }).collect();
    styles.sort_by_key(|s| s.id);
    Schema {
        id: "bikepacking".into(),
        revision: 1,
        obcm_version: obc_formats::obcm::VERSION,
        lods: cfg
            .lods
            .iter()
            .enumerate()
            .map(|(index, l)| LodEntry { index, max_mpp: l.max_mpp, band: String::new() })
            .collect(),
        bands,
        styles,
        routing: Routing { min_component_edges: cfg.routing.min_component_edges, profiles: Vec::new() },
        chunk_size: cfg.chunk_size,
    }
}

/// The shipped `default` skin as a skin **document**: feature types and presentation values, and
/// deliberately nothing about flag bit 6 — a skin has no way to state it.
fn shipped_default_skin(schema_cfg: &Config) -> Skin {
    let skin_cfg = Config::parse(DEFAULT_SKIN).expect("the shipped skin document parses");
    let ids = feature_type_ids(schema_cfg);
    let mut styles: Vec<serde_json::Value> = skin_cfg
        .features
        .iter()
        .flat_map(|(key, values)| {
            values.iter().map(move |(value, style)| {
                serde_json::json!({
                    "feature_type": format!("{key}.{value}"),
                    "color": style.color,
                    "weight": style.weight,
                    "z_index": style.z_index,
                    "priority": style.priority,
                    "dashed": style.line_style == LineStyle::Dashed,
                    "fixed_width": style.fixed_width,
                    "terrain_layer": style.terrain_layer,
                    "color2": style.color2,
                })
            })
        })
        .collect();
    styles.sort_by_key(|s| ids[s["feature_type"].as_str().unwrap()]);
    let text = serde_json::json!({"id": "default", "name": "Default", "marker_color": skin_cfg.marker_color, "styles": styles});
    let skin = Skin::parse(&text.to_string()).expect("the skin resolves into the engine's type");
    assert!(
        !text.to_string().contains("contour_index"),
        "the fixture must prove the bit survives a skin that never mentions it"
    );
    skin
}

/// Every feature in the assembled map as `(style_id, level)`, over every LOD.
fn features(bytes: &[u8]) -> Vec<(u8, Option<i16>)> {
    let cache = MapCache::new_boxed();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("the assembled map parses");
    let reader = Reader::new(&src, &tables, &cache);
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let mut out = Vec::new();
    for lod in 0..reader.lods().len() {
        let mut chunks = Vec::new();
        reader.for_each_chunk(lod, &reader.bbox, |cid, node| chunks.push((cid, node))).expect("walk");
        for (cid, node) in chunks {
            reader
                .for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| out.push((f.style_id, f.level)))
                .expect("decode");
        }
    }
    out
}

/// Cut a tree holding one contour of each class, assemble it under the shipped default skin, and
/// read the result back through the device's own reader.
#[test]
fn the_shipped_skin_preserves_the_level_bytes_and_the_index_bit() {
    let cfg = Config::parse(SCHEMA).expect("the shipped schema parses");
    let index = cfg.contour_style(ContourClass::Index).expect("the shipped schema styles index contours");
    let major = cfg.contour_style(ContourClass::Major).expect("…and major ones");
    let (index_id, major_id) = (index.id, major.id);

    let features_in = vec![
        contour(index_id, index.min_lod, INDEX_LEVEL, 47_300_000),
        contour(major_id, major.min_lod, MAJOR_LEVEL, 47_320_000),
    ];
    let ing = Ingested {
        features: features_in,
        coastlines: Vec::new(),
        pois: Vec::new(),
        nav_graph: obc_pack::nav::build_graph_with(&[], cfg.routing.min_component_edges).0,
    };

    let dir = scratch("shipped-skin");
    let opts = CutOptions {
        bands: BandTable::parse(BANDS).expect("band table"),
        sources: vec![SourceExtent::parse("fixture=7.5,47.2,7.7,47.4").expect("source")],
        ..Default::default()
    };
    let summary = cut_ingested(&ing, &[], &cfg, &dir, &opts, &Progress::silent()).expect("the cutter runs");

    let sources: Vec<MemorySource> = summary
        .cells
        .iter()
        .map(|c| MemorySource(std::fs::read(dir.join(&c.path)).expect("a cell artifact")))
        .collect();
    let inputs: Vec<CellInput<'_>> = summary
        .cells
        .iter()
        .zip(&sources)
        .map(|(c, src)| CellInput {
            id: CellId::parse(&c.id.to_string()).expect("the two CellId spellings agree"),
            band: c.band.clone(),
            src,
            partial: c.partial,
        })
        .collect();
    let mut store = MemoryStore::default();
    let out = assemble(
        inputs,
        &schema(&cfg),
        &shipped_default_skin(&cfg),
        &Options { name: "Contours".into(), accept_partial: true, ..Default::default() },
        &mut store,
        &NoClock,
    )
    .expect("the assembly runs");
    assert_eq!(out.shards.len(), 1, "a fixture-scale map takes the single-file fast path");
    let bytes = &store.shards[0].0;

    // 1. The style table: bit 6 survives a skin that cannot state it, and lands on the index class
    //    alone — the derivation is from the feature type, so the major class must NOT pick it up.
    let cache = MapCache::new_boxed();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("the assembled map parses");
    let reader = Reader::new(&src, &tables, &cache);
    assert_eq!(reader.version, obc_formats::obcm::VERSION);
    let index_style = reader.style(index_id).expect("the index-contour style is in the table");
    assert!(index_style.flags.contour_index(), "bit 6 is derived from {CONTOUR_INDEX_FEATURE_TYPE}, not authored");
    assert!(index_style.flags.terrain_layer() && index_style.flags.fixed_width(), "…and #1095's two ride along");
    assert!(
        !reader.style(major_id).expect("the major-contour style").flags.contour_index(),
        "the major class is not an index contour, and shares every other flag with one"
    );
    for id in 0u16..=255 {
        if let Some(s) = reader.style(id as u8) {
            assert_eq!(
                s.flags.contour_index(),
                s.id == index_id,
                "style {} must not claim bit 6: exactly one feature type derives it",
                s.id
            );
        }
    }

    // 2. The features: the graft copies chunk bytes, so the §5.2 level must come back unchanged on
    //    every tier the contour reaches — and on nothing else.
    let decoded = features(bytes);
    let index_levels: Vec<Option<i16>> = decoded.iter().filter(|(id, _)| *id == index_id).map(|(_, l)| *l).collect();
    let major_levels: Vec<Option<i16>> = decoded.iter().filter(|(id, _)| *id == major_id).map(|(_, l)| *l).collect();
    assert!(!index_levels.is_empty(), "the index contour reached the assembled map");
    assert!(!major_levels.is_empty(), "so did the major one");
    assert!(index_levels.iter().all(|l| *l == Some(INDEX_LEVEL)), "index levels survive the graft: {index_levels:?}");
    assert!(major_levels.iter().all(|l| *l == Some(MAJOR_LEVEL)), "major levels survive the graft: {major_levels:?}");
    assert!(
        decoded.iter().filter(|(id, _)| *id != index_id && *id != major_id).all(|(_, l)| l.is_none()),
        "nothing but a contour carries a level"
    );
}
