//! **The fixture regenerator** — `#[ignore]`d, and the only thing allowed to write
//! `tests/fixture/`.
//!
//! `tests/determinism.rs` and `builder/app/src/lib/assemble/bridge.test.ts` both assert that an
//! assembly of the checked-in cell tree reproduces the checked-in output **byte for byte** — the
//! native driver on one side, the wasm build on the other. That only means anything if the fixture
//! has a stated provenance, so here it is, executable:
//!
//! ```text
//! # 1. cut the synthetic extract into cells, and write the terrain cells beside it
//! #    (writes tests/fixture/cells/, cells.json, skin.json, terrain/, terrain.json)
//! cargo test -p obc-web-assemble --test fixture regenerate -- --ignored --nocapture
//! # 2. assemble them with the NATIVE CLI — the bytes both sides are then held to
//! cargo run --release -p obcm-assemble -- \
//!     --cells   apps/obc-web-assemble/tests/fixture/cells.json \
//!     --terrain apps/obc-web-assemble/tests/fixture/terrain.json \
//!     --skin    apps/obc-web-assemble/tests/fixture/skin.json \
//!     --out     apps/obc-web-assemble/tests/fixture/expected/map.obcm \
//!     --accept-partial
//! # 3. …and again with no raster, which is `expected/flat.obcm`: the same selection with an empty
//! #    §1.3 region, and the file this crate's output was byte-identical to before the raster was
//! #    spliced in (it was a separate `.OBD` then, so the map itself is unchanged).
//! cargo run --release -p obcm-assemble -- \
//!     --cells   apps/obc-web-assemble/tests/fixture/cells.json \
//!     --skin    apps/obc-web-assemble/tests/fixture/skin.json \
//!     --out     apps/obc-web-assemble/tests/fixture/expected/flat.obcm \
//!     --accept-partial
//! ```
//!
//! Steps 2 and 3 are deliberately the real CLI rather than a library call from step 1: the pin's
//! claim is "the browser produces what the command line produces", and a fixture generated through
//! the same entry point the test uses would only prove the engine agrees with itself.
//!
//! **This crate does not depend on `obc-pack`** except here, as a dev-dependency, exactly as
//! `obcm-assemble`'s own oracle does: the cutter carries libGEOS and must never enter the bridge's
//! build graph, let alone the wasm one.
//!
//! The extract is a scaled-down cousin of that oracle's — small enough that the whole cell tree is a
//! few tens of KB in the repo, but still carrying every section the assembler has to *rebuild*
//! rather than copy: POIs (including two cells sharing one opening-hours schedule, so §4.5.3's pool
//! remap is live), a road network whose ways cross a cell seam (so §4.6.2's junction unification
//! runs), an interior islet below the prune threshold, and geometry both cut by a seam and wholly
//! inside one cell.

use std::path::{Path, PathBuf};

use obc_pack::config::Config;
use obc_pack::cut::{cut_ingested, CutOptions, SourceExtent};
use obc_pack::geom::Geom;
use obc_pack::grid::BandTable;
use obc_pack::ingest::{IngestFeature, Ingested};
use obc_pack::nav::RoutableWay;
use obc_pack::poi::Poi;
use obc_pack::progress::Progress;

/// The `2^18` lon line the fixture straddles — OBCA §7's worked-example seam.
const SEAM: i64 = 7_602_176;
/// A latitude comfortably inside `2^18` cell row 180 (`47 185 920 .. 47 448 064`).
const LAT: i64 = 47_300_000;

/// A two-level ladder with no simplification, so the cut vertices are exactly the crossing
/// coordinates and nothing depends on a tolerance. `chunk_size` is small on purpose: the quadtrees
/// must genuinely subdivide, or the graft has no subtrees to relocate and the pin proves nothing.
///
/// `highway.path` is dashed with a `color2` because those are the two OBCM style-record flag
/// bits (`0x04` / `0x08`) plus a trailing `uint16` — the part of OBCA §4.7's skin stamp a plain
/// style never exercises.
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

/// The v1 table's shape at a toy ladder: one coarse band, one geometry band, one core band, with the
/// geometry and core bands sharing `2^18` exactly as `fine` and `network` do.
const BANDS: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 20, "lods": [0], "role": "coarse"},
    {"id": "fine",    "cell_log2": 18, "lods": [1], "role": "geometry"},
    {"id": "network", "cell_log2": 18, "lods": [],  "sections": ["nav", "poi"], "role": "core"}
]}"#;

/// Where the checked-in fixture lives.
pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

// --- Terrain (EL4, #1072) ---------------------------------------------------------------------
//
// The fixture's terrain store, at a lattice chosen the way `obc-vectors`' own does: the **cell** is
// the v1 `2^19` (so the assembly rectangle is the real thing — 2 × 2 squares over the fixture's
// `2^20` assembly bbox) while the **posting** is coarsened to `2^14`, because the v1 `2^9` posting
// would make one cell 2 MiB of raster and the whole fixture 8 MiB in the repo. Both are header data
// precisely so this is legal (`OBCT_Spec.md` §1.3); at this pairing a cell is 32 × 32 samples =
// 2 × 2 tiles = 2048 bytes, so the tree is 6 KB and still exercises tile addressing, the cross-cell
// seam, and a hole.

/// The store's lattice — what the catalog's §13.1 terrain block would state.
pub const T_POSTING_LOG2: u8 = 14;
pub const T_CELL_LOG2: u8 = 19;
/// The fixture's assembly bbox is `2^20` at (47.185920 °N, 7.340032 °E), which is these four
/// `2^19` squares.
const T_MIN_I: u32 = 602;
const T_MIN_J: u32 = 526;
/// The square left **unpublished**: the fixture's known-empty terrain, which must reach the map's
/// §1.3 region as a `0` directory slot and cost four bytes (`OBCC_Spec.md` §13.6, `OBCT_Spec.md`
/// §4.3).
const T_ABSENT: (u32, u32) = (603, 527);

/// The surface: a plane with **different** coefficients per axis, so a transposed latitude/longitude
/// produces different numbers rather than a plausible-looking one. Indexed by lattice offsets from
/// each cell's own base sample, which is what makes each cell a pure function of its id — the same
/// property the real bakery's `bake_cell` has, and the reason a cell inside the assembled region is
/// byte-for-byte the cell published on its own.
fn t_height(ci: u32, cj: u32) -> impl Fn(u32, u32) -> i16 {
    let per_cell = 1u32 << (T_CELL_LOG2 - T_POSTING_LOG2);
    move |di, dj| {
        let i = (ci - T_MIN_I) * per_cell + di;
        let j = (cj - T_MIN_J) * per_cell + dj;
        (400 + 3 * i as i32 + 11 * j as i32) as i16
    }
}

/// Write `tests/fixture/terrain/<i>/<j>.obcd` plus the `terrain.json` sidecar the CLI's `--terrain`
/// takes — the terrain half of step 1 in the module header.
fn regenerate_terrain(dir: &Path) {
    use sha2::{Digest, Sha256};

    let root = dir.join("terrain");
    let _ = std::fs::remove_dir_all(&root);
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for ci in T_MIN_I..T_MIN_I + 2 {
        for cj in T_MIN_J..T_MIN_J + 2 {
            if (ci, cj) == T_ABSENT {
                continue; // canonically void: no object at all (OBCC §13.6)
            }
            // A published cell is a 1 × 1 container at exactly its own square (OBCC §13.1).
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

/// A routable way from explicit `(osm node id, (lat, lon))` vertices. The ids are explicit because
/// the packer identifies a junction by **OSM node id**: two ways meeting at one coordinate but
/// naming different ids are not connected.
fn way(kind: u8, pts: &[(i64, (i64, i64))]) -> RoutableWay {
    RoutableWay {
        node_ids: pts.iter().map(|(id, _)| *id).collect(),
        coords: pts.iter().map(|(_, (lat, lon))| (*lon as i32, *lat as i32)).collect(),
        kind,
    }
}

fn poi(subtype: u8, lat: i64, lon: i64, name: &str) -> Poi {
    Poi { subtype, lon_udeg: lon as i32, lat_udeg: lat as i32, name: Some(name.into()), from_node: true, hours: None }
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
        // The dashed / `color2` style, strictly inside one cell (dash phase across a seam is a
        // documented OBCA §2.4 cosmetic difference, so it does not belong on the seam).
        line(path, 1, &[(LAT + 70_000, SEAM + 60_000), (LAT + 90_000, SEAM + 110_000)]),
    ];
    // A comb of roads, so the fine LOD's quadtree really subdivides at a 512-byte chunk: without
    // several chunks per cell the graft has no subtree to relocate and the pin proves nothing.
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
        // An islet that **crosses the seam**, which OBCA §3.5 forbids a bake from pruning: it
        // reaches the assembler alive, in two cells, and §4.6.4 is the pass that must drop it.
        way(7, &[(92, (LAT - 40_000, SEAM - 20_000)), (93, (LAT - 40_000, SEAM + 20_000))]),
    ];
    let pois = vec![
        poi(1, LAT, SEAM - 20_000, "West water"),
        // Two POIs in **different cells sharing one schedule**, plus a third with its own: the
        // rebuilt hours pool must hold exactly two blobs and remap three `HoursRef`s across a seam.
        poi_with_hours(5, LAT + 5_000, SEAM + 15_000, "East camp", "Mo-Fr 08:00-18:00"),
        poi_with_hours(5, LAT + 5_000, SEAM - 15_000, "West camp", "Mo-Fr 08:00-18:00"),
        poi_with_hours(13, LAT + 25_000, SEAM + 60_000, "Shop", "Mo-Sa 09:00-12:00,14:00-19:00"),
    ];
    (Ingested { features, coastlines: Vec::new(), pois, nav_graph: Default::default() }, ways)
}

/// A skin reproducing the config's own styling exactly, in ascending id order (OBCA §4.7 makes the
/// order the skin author's problem — the engine refuses an unsorted table rather than re-sorting).
/// The ids are named directly because no canonical style assignment travels with a local cut.
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
                "dashed": s.dashed,
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

/// Regenerate `tests/fixture/cells/`, `cells.json` and `skin.json`. See the module header for the
/// second half (the native CLI run that produces `expected/`).
#[test]
#[ignore = "regenerates the checked-in fixture; run deliberately, see the module header"]
fn regenerate() {
    let dir = fixture_dir();
    let cfg = Config::parse(CONFIG).expect("the fixture config parses");
    let (ing, ways) = extract(&cfg);
    let _ = std::fs::remove_dir_all(dir.join("cells"));
    let opts = CutOptions {
        bands: BandTable::parse(BANDS).expect("band table"),
        // A coverage claim wide enough that the two `2^18` cells are whole; the `2^20` coarse cell
        // is necessarily not, which is why the assembly runs with `--accept-partial`.
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
