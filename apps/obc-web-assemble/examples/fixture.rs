//! The fixture regenerator: the only thing allowed to write `tests/fixture/`.
//!
//! The determinism tests assert that an assembly of the checked-in cell tree reproduces the
//! checked-in output byte for byte, which needs the fixture to have a stated provenance:
//!
//! ```text
//! # 1. cut the synthetic extract into cells, and write the terrain cells beside it
//! #    (writes tests/fixture/cells/, cells.json, skin.json, terrain/, terrain.json)
//! cargo run -p obc-web-assemble --example fixture --locked
//! # 2. assemble them with the NATIVE CLI — the bytes both sides are then held to
//! cargo run --release -p obcm-assemble -- \
//!     --cells   apps/obc-web-assemble/tests/fixture/cells.json \
//!     --terrain apps/obc-web-assemble/tests/fixture/terrain.json \
//!     --light-skin apps/obc-web-assemble/tests/fixture/skin.json \
//!     --dark-skin apps/obc-web-assemble/tests/fixture/dark-skin.json \
//!     --out     apps/obc-web-assemble/tests/fixture/expected/map.obcm \
//!     --accept-partial
//! # 3. and again with no raster, which is `expected/flat.obcm`: the same selection with an empty
//! #    terrain region.
//! cargo run --release -p obcm-assemble -- \
//!     --cells   apps/obc-web-assemble/tests/fixture/cells.json \
//!     --light-skin apps/obc-web-assemble/tests/fixture/skin.json \
//!     --dark-skin apps/obc-web-assemble/tests/fixture/dark-skin.json \
//!     --out     apps/obc-web-assemble/tests/fixture/expected/flat.obcm \
//!     --accept-partial
//! ```
//!
//! Steps 2 and 3 are the real CLI and not a library call from step 1. The claim is that the browser
//! produces what the command line produces, and a fixture made through the entry point the test uses
//! would only prove the engine agrees with itself.
//!
//! This crate depends on `obc-pack` only here, as a dev-dependency: the cutter carries libGEOS and
//! must never enter the bridge's build graph.
//!
//! The extract is a few tens of KB but still carries every section the assembler has to rebuild
//! rather than copy: POIs, including two cells sharing one opening-hours schedule, a road network
//! whose ways cross a cell seam, an interior islet below the prune threshold, and geometry both cut
//! by a seam and wholly inside one cell.

use std::path::{Path, PathBuf};

use obc_pack::config::Config;
use obc_pack::cut::{cut_ingested, CutOptions, SourceExtent};
use obc_pack::geom::Geom;
use obc_pack::grid::BandTable;
use obc_pack::ingest::{IngestFeature, Ingested};
use obc_pack::nav::RoutableWay;
use obc_pack::poi::Poi;
use obc_pack::progress::Progress;

/// The `2^18` lon line the fixture straddles.
const SEAM: i64 = 7_602_176;
/// A latitude comfortably inside `2^18` cell row 180 (`47 185 920 .. 47 448 064`).
const LAT: i64 = 47_300_000;

/// A two-level ladder with no simplification, so the cut vertices are exactly the crossing
/// coordinates and nothing depends on a tolerance. `chunk_size` is small so the quadtrees
/// subdivide; without subtrees to relocate the graft proves nothing.
///
/// `highway.path` is dashed with a `color2` because those are the two style-record flag bits plus a
/// trailing `uint16`, which a plain style never exercises.
const CONFIG: &str = r#"{
    "lods": [
        {"max_mpp": null, "simplify": 0},
        {"max_mpp": 6, "simplify": 0}
    ],
    "features": {
        "natural": { "water": {"color": "0x001F", "weight": 1, "z_index": 1, "min_lod": 0} },
        "highway": {
            "primary":     {"color": "0xF800", "weight": 3, "z_index": 5, "min_lod": 0},
            "residential": {"color": "0xFFE0", "weight": 2, "z_index": 4, "min_lod": 1},
            "path":        {"color": "0x780F", "weight": 2, "z_index": 6, "min_lod": 1,
                            "line_style": "dashed", "color2": "0x07FF", "priority": 2}
        }
    },
    "marker": {"color": "0xF800"},
    "chunk_size": 512,
    "routing": {"min_component_edges": 4}
}"#;

/// The band table's shape at a toy ladder: one coarse band, one geometry band, one core band, with
/// the geometry and core bands sharing `2^18`.
const BANDS: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 20, "lods": [0], "role": "coarse"},
    {"id": "fine",    "cell_log2": 18, "lods": [1], "role": "geometry"},
    {"id": "network", "cell_log2": 18, "lods": [],  "sections": ["nav", "poi"], "role": "core"}
]}"#;

/// Where the checked-in fixture lives.
pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

// The fixture's terrain store keeps the real `2^19` cell, so the assembly rectangle is 2 x 2
// squares, but coarsens the posting to `2^14`: at the real `2^9` posting one cell would be 2 MiB of
// raster. Both are header data, so the pairing is legal, and at 2048 bytes a cell the tree still
// exercises tile addressing, the cross-cell seam and a hole.

/// The store's lattice, as the catalog's terrain block would state it.
pub const T_POSTING_LOG2: u8 = 14;
pub const T_CELL_LOG2: u8 = 19;
/// The fixture's assembly bbox is `2^20` at (47.185920 °N, 7.340032 °E), which is these four
/// `2^19` squares.
const T_MIN_I: u32 = 602;
const T_MIN_J: u32 = 526;
/// The square left unpublished: the fixture's known-empty terrain, which must reach the map's
/// terrain region as a `0` directory slot and cost four bytes.
const T_ABSENT: (u32, u32) = (603, 527);

/// The surface: a plane with different coefficients per axis, so a transposed latitude and longitude
/// produce different numbers rather than plausible ones. Indexed by lattice offsets from each cell's
/// own base sample, which makes each cell a pure function of its id.
fn t_height(ci: u32, cj: u32) -> impl Fn(u32, u32) -> i16 {
    let per_cell = 1u32 << (T_CELL_LOG2 - T_POSTING_LOG2);
    move |di, dj| {
        let i = (ci - T_MIN_I) * per_cell + di;
        let j = (cj - T_MIN_J) * per_cell + dj;
        (400 + 3 * i as i32 + 11 * j as i32) as i16
    }
}

/// Write `tests/fixture/terrain/<i>/<j>.obcd` plus the `terrain.json` sidecar the CLI's `--terrain`
/// takes.
fn regenerate_terrain(dir: &Path) {
    use sha2::{Digest, Sha256};

    let root = dir.join("terrain");
    let _ = std::fs::remove_dir_all(&root);
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for ci in T_MIN_I..T_MIN_I + 2 {
        for cj in T_MIN_J..T_MIN_J + 2 {
            if (ci, cj) == T_ABSENT {
                continue; // canonically void: no object at all
            }
            // A published cell is a 1 x 1 container at exactly its own square.
            let bytes = obc_vectors::terrain_container(
                T_POSTING_LOG2,
                T_CELL_LOG2,
                ci,
                cj,
                1,
                1,
                &|_, _| true,
                &t_height(ci, cj),
            );
            let rel = format!("terrain/{ci:04}/{cj:04}.obcd");
            let path = dir.join(&rel);
            std::fs::create_dir_all(path.parent().expect("has a parent")).expect("mkdir");
            std::fs::write(&path, &bytes).expect("write terrain cell");
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            entries.push(serde_json::json!({
                "id": format!("{T_CELL_LOG2}/{ci:04}/{cj:04}"),
                "path": rel,
                "bytes": bytes.len(),
                "sha256": digest.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            }));
            println!("  terrain {rel:28} {:>7} B", bytes.len());
        }
    }
    let mut json = serde_json::to_string_pretty(&serde_json::json!({
        "posting_log2": T_POSTING_LOG2,
        "cell_log2": T_CELL_LOG2,
        "cells": entries,
    }))
    .expect("the terrain sidecar serialises");
    json.push('\n');
    std::fs::write(dir.join("terrain.json"), json).expect("write terrain.json");
}

fn deg(udeg: i64) -> f64 {
    udeg as f64 / 1e6
}

fn line(style_id: u8, min_lod: usize, pts: &[(i64, i64)]) -> IngestFeature {
    IngestFeature { style_id, min_lod, geom: Geom::Line(pts.iter().map(|&(lat, lon)| (deg(lon), deg(lat))).collect()) }
}

fn rect(style_id: u8, min_lod: usize, lat0: i64, lon0: i64, lat1: i64, lon1: i64) -> IngestFeature {
    let ring = vec![
        (deg(lon0), deg(lat0)),
        (deg(lon1), deg(lat0)),
        (deg(lon1), deg(lat1)),
        (deg(lon0), deg(lat1)),
        (deg(lon0), deg(lat0)),
    ];
    IngestFeature { style_id, min_lod, geom: Geom::Polygon { exterior: ring, interiors: vec![] } }
}

/// A routable way from explicit `(osm node id, (lat, lon))` vertices. The packer identifies a
/// junction by OSM node id, so two ways that meet at one coordinate under different ids are not
/// connected.
fn way(kind: u8, pts: &[(i64, (i64, i64))]) -> RoutableWay {
    RoutableWay {
        node_ids: pts.iter().map(|(id, _)| *id).collect(),
        coords: pts.iter().map(|(_, (lat, lon))| (*lon as i32, *lat as i32)).collect(),
        kind,
    }
}

fn poi(subtype: u8, lat: i64, lon: i64, name: &str) -> Poi {
    Poi {
        metadata: obc_formats::obcm::PoiMetadata {
            source: obc_formats::obcm::SourceId::osm(1, ((lat as u64) << 26) ^ ((lon as u64) << 5) ^ subtype as u64),
            approach: None,
        },
        access_nodes: Vec::new(),
        wikidata: None,
        wikipedia: None,
        subtype,
        lon_udeg: lon as i32,
        lat_udeg: lat as i32,
        name: Some(name.into()),
        from_node: true,
        hours: None,
        elevation_m: None,
        population: None,
    }
}

fn poi_with_hours(subtype: u8, lat: i64, lon: i64, name: &str, hours: &str) -> Poi {
    Poi {
        hours: Some(obc_pack::hours::parse(hours).expect("the fixture's opening_hours parses")),
        ..poi(subtype, lat, lon, name)
    }
}

fn style_id(cfg: &Config, key: &str, value: &str) -> u8 {
    cfg.get_style(&std::collections::HashMap::from([(key, value)])).expect("styled feature type").id
}

fn extract(cfg: &Config) -> (Ingested, Vec<RoutableWay>) {
    let water = style_id(cfg, "natural", "water");
    let primary = style_id(cfg, "highway", "primary");
    let residential = style_id(cfg, "highway", "residential");
    let path = style_id(cfg, "highway", "path");

    let mut features = vec![
        // A primary road across the seam — the line whose clipped halves must meet again.
        line(primary, 0, &[(LAT, SEAM - 50_000), (LAT, SEAM + 50_000)]),
        // A lake straddling the seam: a *polygon* clip on a cell edge.
        rect(water, 0, LAT + 20_000, SEAM - 30_000, LAT + 50_000, SEAM + 30_000),
        // …and one wholly inside the eastern cell, which must be written untouched.
        rect(water, 0, LAT - 50_000, SEAM + 70_000, LAT - 20_000, SEAM + 120_000),
        // The dashed and `color2` style, strictly inside one cell: dash phase across a seam is a
        // documented cosmetic difference.
        line(path, 1, &[(LAT + 70_000, SEAM + 60_000), (LAT + 90_000, SEAM + 110_000)]),
    ];
    // A comb of roads, so the fine LOD's quadtree subdivides at a 512-byte chunk. Without several
    // chunks per cell the graft has no subtree to relocate.
    for k in 0..40i64 {
        let lat = LAT - 60_000 + k * 3_000;
        features.push(line(
            residential,
            1,
            &[
                (lat, SEAM - 40_000),
                (lat + 1_000, SEAM - 20_000),
                (lat, SEAM),
                (lat + 1_000, SEAM + 20_000),
                (lat, SEAM + 40_000),
            ],
        ));
    }

    // The road network. Node 3 is the junction the branches share, so this is one component that
    // survives `min_component_edges = 4`.
    let junction = (LAT, SEAM + 30_000);
    let ways = vec![
        // Across the seam to the junction — the route neither cell can carry alone.
        way(7, &[(1, (LAT, SEAM - 50_000)), (2, (LAT, SEAM - 10_000)), (3, junction)]),
        way(7, &[(3, junction), (4, (LAT + 20_000, SEAM + 90_000)), (5, (LAT + 40_000, SEAM + 120_000))]),
        way(10, &[(3, junction), (6, (LAT + 60_000, SEAM + 30_000))]),
        // An islet that crosses the seam, which a bake may not prune: it reaches the assembler
        // alive, in two cells, and the merge must drop it.
        way(7, &[(92, (LAT - 40_000, SEAM - 20_000)), (93, (LAT - 40_000, SEAM + 20_000))]),
    ];
    let mut pois = vec![
        poi(1, LAT, SEAM - 20_000, "West water"),
        // Two POIs in different cells sharing one schedule, plus a third with its own: the rebuilt
        // hours pool must hold two blobs and remap three `HoursRef`s across a seam.
        poi_with_hours(5, LAT + 5_000, SEAM + 15_000, "East camp", "Mo-Fr 08:00-18:00"),
        poi_with_hours(5, LAT + 5_000, SEAM - 15_000, "West camp", "Mo-Fr 08:00-18:00"),
        poi_with_hours(13, LAT + 25_000, SEAM + 60_000, "Shop", "Mo-Sa 09:00-12:00,14:00-19:00"),
    ];
    for (id, lon) in [(101, SEAM - 20_000), (102, SEAM + 20_000)] {
        let mut summit = poi(obc_formats::obcm::SUMMIT_SUBTYPE_ID, LAT + 15_000, lon, "Shared massif");
        summit.metadata.source = obc_formats::obcm::SourceId::osm(1, id);
        summit.elevation_m = Some(3000);
        pois.push(summit);
    }
    (
        Ingested { landmark_links: Vec::new(), features, coastlines: Vec::new(), pois, nav_graph: Default::default() },
        ways,
    )
}

/// A skin reproducing the config's own styling exactly, in ascending id order: the engine refuses
/// an unsorted table rather than re-sorting it.
fn skin_json(cfg: &Config) -> String {
    let mut styles = cfg.styles();
    styles.sort_by_key(|s| s.id);
    let entries: Vec<serde_json::Value> = styles
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.id,
                "color": s.color,
                "weight": s.weight,
                "z_index": s.z_index,
                "priority": s.priority,
                "dashed": s.line_style,
                "fixed_width": s.fixed_width,
                "terrain_layer": s.terrain_layer,
                "color2": s.color2,
            })
        })
        .collect();
    let mut json = serde_json::to_string_pretty(&serde_json::json!({
        "id": "fixture",
        "name": "Bridge Fixture",
        "marker_color": cfg.marker_color,
        "styles": entries,
    }))
    .expect("the skin serialises");
    json.push('\n');
    json
}

fn peak_catalogue(dir: &Path) -> PathBuf {
    use serde_json::json;
    let credit = json!({"source_url":"https://en.wikipedia.org/w/index.php?title=Massif&oldid=1","revision":"1","license_url":"https://creativecommons.org/licenses/by-sa/4.0/","original_notices":"Authors","display_pages":["Source: Authors"]});
    let source = json!({"schema":1,"collection":"peaks","input_sha256":"authored","policy_sha256":"authored","languages":["en","de","fr","es"],"source_coverage":{},"counts":obc_pack::landmarks::Counts::default(),"omissions":[],
        "records":[{"id":"Q7","name":"Shared massif","default_language":"en","fallback_sources":[],"variants":[{"language":"en","text_pages":["A shared mountain."],"attribution":credit},{"language":"de","text_pages":["Ein gemeinsamer Berg."],"attribution":credit}],"photo":null}],
        "associations":[{"node_id":101,"article_id":"Q7","latitude":0,"longitude":0},{"node_id":102,"article_id":"Q7","latitude":0,"longitude":0}]});
    let path = dir.join("peaks.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&source).unwrap()).expect("write peak catalogue");
    path
}

fn main() {
    let dir = fixture_dir();
    let cfg = Config::parse(CONFIG).expect("the fixture config parses");
    let (ing, ways) = extract(&cfg);
    let _ = std::fs::remove_dir_all(dir.join("cells"));
    let opts = CutOptions {
        peaks: vec![peak_catalogue(&dir)],
        bands: BandTable::parse(BANDS).expect("band table"),
        // A coverage claim wide enough that the two `2^18` cells are whole. The `2^20` coarse cell
        // cannot be, which is why the assembly runs with `--accept-partial`.
        sources: vec![SourceExtent::parse("fixture=7.34,47.18,7.87,47.45").expect("source")],
        ..Default::default()
    };
    let summary = cut_ingested(&ing, &ways, &cfg, &dir, &opts, &Progress::silent()).expect("the cutter runs");
    std::fs::write(dir.join("skin.json"), skin_json(&cfg)).expect("write skin.json");

    println!("wrote {} cell(s), {} bytes, {} partial", summary.cells.len(), summary.bytes, summary.partial);
    for c in &summary.cells {
        println!("  {:8} {:20} {:>7} B", c.band, c.path, c.bytes);
    }
    regenerate_terrain(&dir);
    println!("\nnow run the native CLI twice to write tests/fixture/expected/ — see the module header.");
}
