//! **The differential oracle** — #1024's acceptance bar, and the tripwire the epic asked for.
//!
//! One synthetic extract, packed two ways over the *same* snapped bbox:
//!
//! - `pack(X)` — the monolithic path, through the real packer: one quadtree per LOD over the whole
//!   box, one nav graph, one POI section.
//! - `assemble(cut(X))` — the cell path: the real cutter writes per-band cell artifacts, and this
//!   crate grafts them back together.
//!
//! The two files have *different bytes on purpose* (different quadtrees, different chunk layout,
//! different node ids), so the comparison is at the level a rider can see:
//!
//! 1. **Pixels.** The real renderer draws both into the real device framebuffer across a matrix of
//!    viewports — including ones straddling a cell seam and ones sitting exactly at a zoom-band
//!    transition — and the frames must agree.
//! 2. **Routes.** The real A\* plans between endpoint pairs whose straight line crosses a seam, and
//!    the two maps must agree on success and on length.
//!
//! What the oracle is *for*: a mis-relocated index node, a chunk base off by one, a seam junction
//! that failed to unify, an island pruned that should not have been — every one of those shows up
//! here as a pixel or a route that moved, in a test that says which.

use obc_elevation::NullElevation;
use std::path::{Path, PathBuf};

use embedded_graphics::pixelcolor::raw::RawU16;
use embedded_graphics::pixelcolor::Rgb565;
use obc_display::Framebuffer565;
use obc_formats::io::{ByteSink, SliceSource};
use obc_pack::config::Config;
use obc_pack::cut::{cut_ingested, CutOptions, CutSummary, SourceExtent};
use obc_pack::geom::Geom;
use obc_pack::grid::BandTable;
use obc_pack::ingest::{IngestFeature, Ingested};
use obc_pack::nav::RoutableWay;
use obc_pack::poi::Poi;
use obc_pack::progress::Progress;
use obc_pack::quadtree::build_lod_with;
use obc_pack::{serialize_lods, LodLayer};
use obc_reader::{MapCache, MapTables, NavTileCache, Reader};
use obc_render::{zoom_for_mpp, MapRenderer, Viewport};
use obc_route::nav::{plan_route, NavScratch};
use obcm_assemble::grid::{assembly_box, CellId};
use obcm_assemble::schema::{Schema, Skin, SkinStyle};
use obcm_assemble::{assemble, CellInput, MemorySource, MemoryStore, NoClock, Options};

// --- the fixture ------------------------------------------------------------------------------

/// The `2^18` lon line between cells `j = 1052` and `j = 1053` — OBCA §7's worked-example seam.
const SEAM: i64 = 7_602_176;
/// The next `2^18` lon line east, which is simultaneously a `2^19` line.
const SEAM_E: i64 = 7_864_320;
/// A `2^18` lat line inside the fixture's ground.
const SEAM_N: i64 = 47_448_064;
/// A latitude comfortably inside cell row 1204.
const LAT: i64 = 47_300_000;

/// A three-level ladder with **no simplification and no culling**: the cutter's clipped vertices are
/// then exactly the crossing coordinates, so any pixel difference is a real one rather than a
/// tolerance artefact. `chunk_size` is deliberately small so the quadtrees genuinely subdivide and
/// the graft has real subtrees to relocate.
/// `highway.path` is deliberately **dashed with a `color2`**: the two OBCM style-record flag bits
/// (`0x04` / `0x08`) plus the trailing `uint16` are the part of §4.7's stamp a plain style never
/// exercises, and a skin that lost them would ship a map whose lines are all solid. The feature that
/// uses it is placed strictly inside one cell, because dash **phase** is one of the two cosmetic
/// costs OBCA §2.4 books against cutting at a cell boundary — a dashed line across a seam would
/// legitimately differ between the two paths and turn the pixel oracle into a fuzz test.
const CONFIG: &str = r#"{
    "lods": [
        {"max_mpp": null, "simplify": 0},
        {"max_mpp": 20, "simplify": 0},
        {"max_mpp": 4, "simplify": 0}
    ],
    "features": {
        "natural": { "water": {"color": "0x001F", "weight": 1, "z_index": 1, "min_lod": 0} },
        "landuse": { "forest": {"color": "0x07E0", "weight": 1, "z_index": 2, "min_lod": 1} },
        "highway": {
            "primary":     {"color": "0xF800", "weight": 3, "z_index": 5, "min_lod": 0},
            "residential": {"color": "0xFFE0", "weight": 2, "z_index": 4, "min_lod": 1},
            "track":       {"color": "0x8410", "weight": 1, "z_index": 3, "min_lod": 2},
            "path":        {"color": "0x780F", "weight": 2, "z_index": 6, "min_lod": 1,
                            "line_style": "dashed", "color2": "0x07FF", "priority": 2}
        }
    },
    "marker": {"color": "0xF800"},
    "chunk_size": 1024,
    "routing": {"min_component_edges": 4}
}"#;

/// One coarse band, one geometry band, one core band — the v1 table's shape at a toy ladder, with
/// two bands sharing `2^18` exactly as `fine` and `network` do.
const BANDS: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 20, "lods": [0],    "role": "coarse"},
    {"id": "fine",    "cell_log2": 18, "lods": [1, 2], "role": "geometry"},
    {"id": "network", "cell_log2": 18, "lods": [],     "sections": ["nav", "poi"], "role": "core"}
]}"#;

/// The **v1 table's actual shape**: `mid` and `fine` are two bands at two cell sizes that share the
/// one `geometry` role, which is the configuration the single-geometry-band table above cannot
/// exercise. Kept permanently, because the set planner is defined by *role* and a per-band tiling
/// looks correct until a schema names two bands of one role — at which point it emits two
/// overlapping antichains of `Role == 1` shards and §5.3 refuses the set after every byte is
/// written.
const BANDS_TWO_GEOMETRY: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 20, "lods": [0], "role": "coarse"},
    {"id": "mid",     "cell_log2": 19, "lods": [1], "role": "geometry"},
    {"id": "fine",    "cell_log2": 18, "lods": [2], "role": "geometry"},
    {"id": "network", "cell_log2": 18, "lods": [],  "sections": ["nav", "poi"], "role": "core"}
]}"#;

fn config() -> Config {
    Config::parse(CONFIG).expect("test config parses")
}

fn style_id(cfg: &Config, key: &str, value: &str) -> u8 {
    cfg.get_style(&std::collections::HashMap::from([(key, value)])).expect("styled feature type").id
}

fn deg(udeg: i64) -> f64 {
    udeg as f64 / 1e6
}

fn line(style_id: u8, min_lod: usize, pts: &[(i64, i64)]) -> IngestFeature {
    IngestFeature { style_id, min_lod, geom: Geom::Line(pts.iter().map(|&(lat, lon)| (deg(lon), deg(lat))).collect()) }
}

fn ring(lat0: i64, lon0: i64, lat1: i64, lon1: i64) -> Vec<(f64, f64)> {
    vec![
        (deg(lon0), deg(lat0)),
        (deg(lon1), deg(lat0)),
        (deg(lon1), deg(lat1)),
        (deg(lon0), deg(lat1)),
        (deg(lon0), deg(lat0)),
    ]
}

fn rect(style_id: u8, min_lod: usize, lat0: i64, lon0: i64, lat1: i64, lon1: i64) -> IngestFeature {
    IngestFeature {
        style_id,
        min_lod,
        geom: Geom::Polygon { exterior: ring(lat0, lon0, lat1, lon1), interiors: vec![] },
    }
}

/// A polygon **with a hole** — the `FEATURE_FLAG_HOLES` path of `OBCM_Spec.md` §6, which is the one
/// feature shape whose chunk bytes carry a ring table. It is copied verbatim like everything else,
/// so what this proves is that the graft never has to understand it; but a hole is also the shape a
/// clip at a cell edge most easily gets wrong, so the fixture has to contain one for the "every
/// feature of every chunk decodes" half of §4.8 to mean anything.
fn rect_with_hole(
    style_id: u8,
    min_lod: usize,
    (lat0, lon0, lat1, lon1): (i64, i64, i64, i64),
    inset: i64,
) -> IngestFeature {
    IngestFeature {
        style_id,
        min_lod,
        geom: Geom::Polygon {
            exterior: ring(lat0, lon0, lat1, lon1),
            // Interior rings run the other way round; the packer normalises, but stating it here
            // keeps the fixture honest about what it is handing in.
            interiors: vec![ring(lat0 + inset, lon0 + inset, lat1 - inset, lon1 - inset).into_iter().rev().collect()],
        },
    }
}

/// A routable way from explicit `(osm node id, (lat, lon))` vertices. The ids are explicit because
/// the packer's graph builder identifies a junction by **OSM node id**: two ways meeting at one
/// coordinate but naming different ids are not connected, and a fixture that got that wrong would
/// test the router's failure mode rather than the assembler's seams.
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

/// A POI **with opening hours**, so the §4.5.3 pool rebuild has something to rebuild. `HoursRef` is
/// a file-local pool index — the one POI field that cannot travel verbatim — so a fixture with no
/// hours leaves the whole remap untested.
fn poi_with_hours(subtype: u8, lat: i64, lon: i64, name: &str, hours: &str) -> Poi {
    Poi {
        hours: Some(obc_pack::hours::parse(hours).expect("the fixture's opening_hours parses")),
        ..poi(subtype, lat, lon, name)
    }
}

/// The extract both paths consume: geometry and roads crossing three grid lines (two lon, one lat),
/// features wholly inside single cells, and POIs spread across the network cells.
fn fixture(cfg: &Config) -> (Ingested, Vec<RoutableWay>) {
    let water = style_id(cfg, "natural", "water");
    let forest = style_id(cfg, "landuse", "forest");
    let primary = style_id(cfg, "highway", "primary");
    let residential = style_id(cfg, "highway", "residential");
    let track = style_id(cfg, "highway", "track");
    let path = style_id(cfg, "highway", "path");

    let mut features = vec![
        // A long primary road crossing both lon seams — the line whose clipped halves must meet.
        line(primary, 0, &[(LAT, SEAM - 60_000), (LAT, SEAM + 40_000), (LAT + 30_000, SEAM_E + 60_000)]),
        // A lake straddling the first seam: a *polygon* clip on a seam.
        rect(water, 0, LAT + 20_000, SEAM - 30_000, LAT + 60_000, SEAM + 30_000),
        // A forest straddling the lat seam, so a horizontal edge is exercised too.
        rect(forest, 1, SEAM_N - 40_000, SEAM + 60_000, SEAM_N + 40_000, SEAM + 160_000),
        // Features wholly inside one cell: they must be written untouched, in that cell only.
        rect(water, 0, LAT - 60_000, SEAM + 90_000, LAT - 20_000, SEAM + 150_000),
        line(residential, 1, &[(LAT - 10_000, SEAM + 20_000), (LAT + 10_000, SEAM + 70_000)]),
        // The **four-cell corner** at (SEAM_N, SEAM): a diagonal line through it and a pond
        // straddling it, so the graft's worst case has something to draw.
        line(residential, 1, &[(SEAM_N - 30_000, SEAM - 30_000), (SEAM_N + 30_000, SEAM + 30_000)]),
        rect(water, 0, SEAM_N - 12_000, SEAM - 12_000, SEAM_N + 12_000, SEAM + 12_000),
        // A ring-shaped lake (polygon **with a hole**) straddling the first seam, so the clip and
        // the verify pass both meet the `FEATURE_FLAG_HOLES` shape.
        rect_with_hole(water, 0, (LAT + 80_000, SEAM - 50_000, LAT + 140_000, SEAM + 50_000), 15_000),
        // …and one wholly inside the eastern cell, so a hole survives the graft uncut as well.
        rect_with_hole(forest, 1, (LAT - 140_000, SEAM + 40_000, LAT - 80_000, SEAM + 140_000), 12_000),
        // The dashed / `color2` style, strictly inside one cell (dash phase across a seam is a
        // documented §2.4 cosmetic difference and would make the pixel oracle a fuzz test).
        line(path, 1, &[(LAT + 100_000, SEAM + 90_000), (LAT + 130_000, SEAM + 150_000)]),
    ];
    // A grid of small tracks: enough features that the fine LOD's quadtree really subdivides.
    for k in 0..24i64 {
        let lat = LAT - 80_000 + k * 8_000;
        features.push(line(track, 2, &[(lat, SEAM - 70_000), (lat, SEAM + 90_000)]));
    }
    // The road network. Node 3 is the junction every branch shares, so the whole thing is one
    // component that survives `min_component_edges` — except the islet, which is meant to be pruned.
    let junction = (LAT, SEAM + 40_000);
    let ways = vec![
        // The through road: west of the first seam, across it, to the junction.
        way(7, &[(1, (LAT, SEAM - 60_000)), (2, (LAT, SEAM - 10_000)), (3, junction)]),
        // On east across the second seam — the route neither cell can carry alone.
        way(7, &[(3, junction), (4, (LAT + 20_000, SEAM_E - 20_000)), (5, (LAT + 20_000, SEAM_E + 40_000))]),
        // A T-spur north from the same junction…
        way(10, &[(3, junction), (6, (LAT + 60_000, SEAM + 40_000))]),
        // …continuing across the **latitude** seam, so a horizontal seam carries a route too.
        way(
            10,
            &[
                (6, (LAT + 60_000, SEAM + 40_000)),
                (7, (SEAM_N - 20_000, SEAM + 40_000)),
                (8, (SEAM_N + 60_000, SEAM + 40_000)),
            ],
        ),
        // A tiny interior islet: strictly inside one cell and below the threshold, so the **bake**
        // prunes it — §3.5 lets a cutter prune only what is strictly interior.
        way(7, &[(90, (LAT - 90_000, SEAM + 120_000)), (91, (LAT - 89_000, SEAM + 121_000))]),
        // …and one that **crosses a cell boundary**, which is exactly what §3.5 forbids a bake from
        // pruning (the piece on the other side might connect to the rest of the world). It reaches
        // the assembler alive, in two cells, and §4.6.4 is the pass that must drop it — over the
        // *merged* graph, where the threshold finally means what it says. Nothing else in this
        // fixture exercises that pass, because everything else is one big component.
        way(7, &[(92, (LAT - 60_000, SEAM - 20_000)), (93, (LAT - 60_000, SEAM + 20_000))]),
    ];
    let pois = vec![
        poi(1, LAT, SEAM - 20_000, "West water"),
        // Two POIs in **different cells sharing one schedule**, plus a third with its own: the
        // rebuilt pool must therefore hold exactly two blobs and remap three `HoursRef`s across a
        // seam (§4.5.3), which is the case a single-cell fixture cannot produce.
        poi_with_hours(5, LAT + 5_000, SEAM + 15_000, "East camp", "Mo-Fr 08:00-18:00"),
        poi_with_hours(5, LAT + 5_000, SEAM - 15_000, "West camp", "Mo-Fr 08:00-18:00"),
        poi_with_hours(13, LAT + 25_000, SEAM_E + 10_000, "Far shop", "Mo-Sa 09:00-12:00,14:00-19:00"),
        poi(1, SEAM_N + 10_000, SEAM + 100_000, "North water"),
    ];
    (Ingested { features, coastlines: Vec::new(), pois, nav_graph: Default::default() }, ways)
}

/// The **uncut** fixture: the same kinds of feature, placed so that nothing crosses a cell edge and
/// no quadtree in either file has to subdivide. It is the control for the pixel oracle — with no
/// geometry cut, the two files must agree bit for bit, so any difference there is the graft's fault
/// and not the rasteriser's.
fn uncut_fixture(cfg: &Config) -> (Ingested, Vec<RoutableWay>) {
    let water = style_id(cfg, "natural", "water");
    let primary = style_id(cfg, "highway", "primary");
    let residential = style_id(cfg, "highway", "residential");
    // Two `2^18` cells, each holding a lake and a road well clear of every edge (the nearest is
    // 40 000 µdeg ≈ 4.5 km away).
    let features = vec![
        rect(water, 0, LAT - 40_000, SEAM - 200_000, LAT + 40_000, SEAM - 120_000),
        line(primary, 0, &[(LAT - 30_000, SEAM - 190_000), (LAT + 30_000, SEAM - 130_000)]),
        rect(water, 0, LAT - 40_000, SEAM + 60_000, LAT + 40_000, SEAM + 140_000),
        line(residential, 1, &[(LAT - 30_000, SEAM + 70_000), (LAT + 30_000, SEAM + 130_000)]),
    ];
    let ways = vec![way(7, &[(1, (LAT, SEAM + 70_000)), (2, (LAT + 20_000, SEAM + 120_000))])];
    let pois = vec![poi(1, LAT, SEAM - 160_000, "West water"), poi(5, LAT, SEAM + 100_000, "East camp")];
    (Ingested { features, coastlines: Vec::new(), pois, nav_graph: Default::default() }, ways)
}

/// Viewports over the uncut fixture: both cells, north-up and rotated, at every ladder level.
fn uncut_scenes() -> Vec<(&'static str, (i32, i32), f32, f32)> {
    vec![
        ("west-street", (SEAM as i32 - 160_000, LAT as i32), 2.0, 0.0),
        ("west-rot", (SEAM as i32 - 160_000, LAT as i32), 2.0, 35.0),
        ("east-street", (SEAM as i32 + 100_000, LAT as i32), 2.0, 0.0),
        ("east-rot", (SEAM as i32 + 100_000, LAT as i32), 3.0, 35.0),
        ("mid", (SEAM as i32 - 160_000, LAT as i32), 12.0, 0.0),
        ("overview", (SEAM as i32 - 30_000, LAT as i32), 100.0, 20.0),
    ]
}

// --- the two paths ----------------------------------------------------------------------------

/// A scratch directory for one test's cell tree. The process id keeps concurrent workspace runs
/// from deleting each other's artifacts.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obcm-assemble-oracle-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Cut the fixture into cells with the **real cutter**.
fn cut(dir: &Path, cfg: &Config, ing: &Ingested, ways: &[RoutableWay]) -> CutSummary {
    cut_with(dir, cfg, ing, ways, BANDS)
}

/// …at an explicit band table, so the same fixture can be cut at both shapes of the v1 schema.
fn cut_with(dir: &Path, cfg: &Config, ing: &Ingested, ways: &[RoutableWay], bands: &str) -> CutSummary {
    let opts = CutOptions {
        bands: BandTable::parse(bands).expect("band table"),
        sources: vec![SourceExtent::parse("fixture=6.9,46.9,8.1,47.9").expect("source")],
        ..Default::default()
    };
    cut_ingested(ing, ways, cfg, dir, &opts, &Progress::silent()).expect("the cutter runs")
}

/// `pack(X)`: the monolithic path over an explicit global bbox — the same stages
/// [`obc_pack::pipeline`] runs, minus the `.pbf` ingest (the fixture is already ingested) and minus
/// land (the fixture has no coastline).
fn monolithic(cfg: &Config, ing: &Ingested, ways: &[RoutableWay], bbox: (i64, i64, i64, i64)) -> Vec<u8> {
    let (graph, _) = obc_pack::nav::build_graph_with(ways, cfg.routing.min_component_edges);
    let lods: Vec<LodLayer> = cfg
        .lods
        .iter()
        .enumerate()
        .map(|(i, lod)| {
            let level: Vec<(u8, Geom)> =
                ing.features.iter().filter(|f| f.min_lod <= i).map(|f| (f.style_id, f.geom.clone())).collect();
            LodLayer {
                max_mpp: lod.max_mpp,
                chunk_size: cfg.chunk_size,
                root: build_lod_with(level, bbox, cfg.chunk_size, &Progress::silent()),
            }
        })
        .collect();
    let (bytes, dropped) = serialize_lods(
        &lods,
        &cfg.styles(),
        cfg.marker_color,
        bbox,
        &ing.pois,
        &graph,
        &cfg.routing.profiles,
        &mut NullElevation,
    );
    assert_eq!(dropped, 0, "the fixture must not lose features to the chunk cap");
    bytes
}

/// The engine's schema for this fixture: the cutter's band table plus the config's ladder.
fn schema(cfg: &Config) -> Schema {
    schema_with(cfg, BANDS)
}

fn schema_with(cfg: &Config, band_json: &str) -> Schema {
    let bands: Vec<obcm_assemble::Band> =
        serde_json::from_value(serde_json::from_str::<serde_json::Value>(band_json).unwrap()["bands"].clone())
            .expect("bands parse into the engine's own table");
    Schema {
        id: "fixture".into(),
        revision: 1,
        obcm_version: obc_formats::obcm::VERSION,
        lods: cfg
            .lods
            .iter()
            .enumerate()
            .map(|(index, l)| obcm_assemble::schema::LodEntry { index, max_mpp: l.max_mpp, band: String::new() })
            .collect(),
        bands,
        styles: Vec::new(), // no canonical assignment travels with a local cut ⇒ the id-keyed skin
        routing: obcm_assemble::schema::Routing {
            min_component_edges: cfg.routing.min_component_edges,
            profiles: Vec::new(),
        },
        chunk_size: cfg.chunk_size,
    }
}

/// A skin that reproduces the config's own styling **exactly**, so the two files differ in layout
/// and never in presentation — otherwise a pixel comparison would only be measuring the skin.
fn skin(cfg: &Config) -> Skin {
    // §4.7 makes the *order* the skin author's responsibility — the engine refuses a table whose ids
    // do not ascend rather than silently re-sorting one. `Config::styles()` walks a `HashMap`, so a
    // document generated from it has to be put in order here, exactly as a hand-written skin would
    // already be.
    let mut cfg_styles = cfg.styles();
    cfg_styles.sort_by_key(|s| s.id);
    let styles = cfg_styles
        .iter()
        .map(|s| SkinStyle {
            id: Some(s.id),
            feature_type: None,
            color: s.color,
            weight: s.weight,
            z_index: s.z_index,
            priority: s.priority,
            dashed: s.dashed,
            color2: s.color2,
        })
        .collect();
    Skin { id: "fixture".into(), name: "Fixture".into(), marker_color: cfg.marker_color, styles }
}

/// `assemble(cut(X))`: graft every cell the cutter wrote back into one map.
fn assembled(dir: &Path, cfg: &Config, summary: &CutSummary) -> (Vec<u8>, MemoryStore) {
    let opts = Options { name: "Oracle".into(), accept_partial: true, ..Default::default() };
    let (out, store) = assemble_with(dir, cfg, summary, &opts).expect("the assembly runs");
    assert_eq!(out.shards.len(), 1, "a fixture-scale map takes the single-file fast path (OBCA §5.5)");
    let bytes = store.shards[0].0.clone();
    (bytes, store)
}

/// Run the engine over a cut tree with explicit options — the shared driver behind the oracle, the
/// volume-set test, and the refusal tests.
fn assemble_with(
    dir: &Path,
    cfg: &Config,
    summary: &CutSummary,
    opts: &Options,
) -> Result<(obcm_assemble::Summary, MemoryStore), obcm_assemble::Error> {
    assemble_bands(dir, cfg, summary, opts, BANDS)
}

/// …against an explicit band table (the two-geometry-band variant needs one).
fn assemble_bands(
    dir: &Path,
    cfg: &Config,
    summary: &CutSummary,
    opts: &Options,
    band_json: &str,
) -> Result<(obcm_assemble::Summary, MemoryStore), obcm_assemble::Error> {
    let sources: Vec<MemorySource> = summary
        .cells
        .iter()
        .map(|c| MemorySource(std::fs::read(dir.join(&c.path)).expect("a cell artifact")))
        .collect();
    let inputs: Vec<CellInput<'_>> = summary
        .cells
        .iter()
        .zip(&sources)
        .map(|(c, src)| CellInput { id: to_engine_cell(c.id), band: c.band.clone(), src, partial: c.partial })
        .collect();
    let mut store = MemoryStore::default();
    let out = assemble(inputs, &schema_with(cfg, band_json), &skin(cfg), opts, &mut store, &NoClock)?;
    Ok((out, store))
}

/// The packer's `CellId` and the engine's are two spellings of one normative id (see the engine's
/// `grid` module docs). Converting through the canonical text is also the cheapest proof they agree.
fn to_engine_cell(id: obc_pack::grid::CellId) -> CellId {
    CellId::parse(&id.to_string()).expect("the two CellId spellings round-trip through the canonical id")
}

/// The assembly bbox both paths are built over: the minimal grid-aligned power-of-two box covering
/// every cell the cutter wrote (OBCA §4.2). The monolithic pack is handed the *same* box, so the two
/// maps cover identical ground and a pixel difference can only come from the graft.
fn snapped_box(summary: &CutSummary) -> (i64, i64, i64, i64) {
    let ids: Vec<CellId> = summary.cells.iter().map(|c| to_engine_cell(c.id)).collect();
    assembly_box(&ids, 20).expect("an aligned box").ubox()
}

/// Both maps plus the store and the box they were built over: what every comparison test needs.
type Both = (Vec<u8>, Vec<u8>, MemoryStore, (i64, i64, i64, i64));

/// Build both maps once for a test.
fn both(name: &str) -> Both {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch(name);
    let summary = cut(&dir, &cfg, &ing, &ways);
    let bbox = snapped_box(&summary);
    let packed = monolithic(&cfg, &ing, &ways, bbox);
    let (grafted, store) = assembled(&dir, &cfg, &summary);
    (packed, grafted, store, bbox)
}

// --- (a) the pixel oracle ---------------------------------------------------------------------

const WIDTH: u32 = obc_display::ls021::FRAME_W as u32;
const HEIGHT: u32 = obc_display::ls021::FRAME_H as u32;

/// Render one viewport of one map through the real reader → renderer → device framebuffer.
fn render(map: &[u8], center: (i32, i32), mpp: f32, heading_deg: f32) -> Vec<u16> {
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).expect("the map parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    let mut renderer = MapRenderer::new();
    let mut buf = vec![0u16; (WIDTH * HEIGHT) as usize];
    let bg = Rgb565::from(RawU16::new(reader.backdrop_style().map(|s| s.color).unwrap_or(0xFFFF)));
    let vp = Viewport::new_rotated(
        WIDTH as f32,
        HEIGHT as f32,
        center.0,
        center.1,
        zoom_for_mpp(mpp),
        heading_deg.to_radians(),
    );
    let mut fb = Framebuffer565::new(&mut buf, WIDTH, HEIGHT);
    renderer.render(&mut fb, &reader, &vp, bg, |c| Rgb565::from(RawU16::new(c)));
    buf
}

/// The map's backdrop colour — the style with the lowest z-index, which the renderer clears to.
fn backdrop_color(map: &[u8]) -> u16 {
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).expect("the map parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    reader.backdrop_style().map(|s| s.color).unwrap_or(0xFFFF)
}

/// The viewport matrix. Every scene names what it is there to catch.
fn scenes() -> Vec<(&'static str, (i32, i32), f32, f32)> {
    vec![
        // Straddling the worked-example lon seam, at three zooms — the cell boundary is vertically
        // through the middle of the frame.
        ("seam-lon-street", (SEAM as i32, LAT as i32), 2.0, 0.0),
        ("seam-lon-mid", (SEAM as i32, LAT as i32), 10.0, 0.0),
        ("seam-lon-rot", (SEAM as i32, LAT as i32), 6.0, 35.0),
        // Straddling the lat seam, where a *horizontal* clip edge meets the graft.
        ("seam-lat", (SEAM as i32 + 100_000, SEAM_N as i32), 4.0, 0.0),
        // The corner where four fine cells meet — the graft's worst case.
        ("seam-corner", (SEAM as i32, SEAM_N as i32), 8.0, 0.0),
        // Zoom-band transitions: exactly at, just under and just over each ladder threshold, so the
        // LOD the renderer selects flips between the coarse band (its own cell size!) and the fine
        // one across the pair.
        ("band-20-under", (SEAM as i32, LAT as i32), 19.9, 0.0),
        ("band-20-over", (SEAM as i32, LAT as i32), 20.1, 0.0),
        ("band-4-under", (SEAM as i32, LAT as i32), 3.9, 0.0),
        ("band-4-over", (SEAM as i32, LAT as i32), 4.1, 0.0),
        // Cell interiors, where nothing should be interesting — and a wide overview that pulls in
        // every cell of the coarse band at once.
        ("interior-west", (SEAM as i32 - 40_000, LAT as i32), 3.0, 0.0),
        ("interior-east", (SEAM as i32 + 60_000, LAT as i32), 3.0, 0.0),
        ("overview", (SEAM as i32, LAT as i32), 60.0, 0.0),
    ]
}

/// How two frames differ. The *shape* of the difference is the point: a comparison that only counts
/// pixels cannot tell a one-pixel boundary shift from a feature drawn in the wrong place, and those
/// are exactly the two outcomes this oracle has to keep apart.
struct Diff {
    count: usize,
    /// Every distinct `(packed colour, assembled colour)` pair, with its count.
    pairs: std::collections::BTreeMap<(u16, u16), usize>,
    /// Differing pixels where the other frame's colour is **not** present in the 8-neighbourhood —
    /// i.e. a boundary that did not merely move by a pixel. That is what a graft bug looks like.
    non_edge: usize,
}

fn diff(a: &[u16], b: &[u16]) -> Diff {
    let at = |buf: &[u16], x: i64, y: i64| -> Option<u16> {
        if x < 0 || y < 0 || x >= WIDTH as i64 || y >= HEIGHT as i64 {
            None
        } else {
            Some(buf[y as usize * WIDTH as usize + x as usize])
        }
    };
    let mut out = Diff { count: 0, pairs: Default::default(), non_edge: 0 };
    for (i, (p, q)) in a.iter().zip(b).enumerate() {
        if p == q {
            continue;
        }
        out.count += 1;
        *out.pairs.entry((*p, *q)).or_default() += 1;
        let (x, y) = ((i % WIDTH as usize) as i64, (i / WIDTH as usize) as i64);
        let nearby =
            |buf: &[u16], want: u16| (-1..=1).any(|dy| (-1..=1).any(|dx| at(buf, x + dx, y + dy) == Some(want)));
        if !nearby(a, *q) || !nearby(b, *p) {
            out.non_edge += 1;
        }
    }
    out
}

/// (a) **Pixel equivalence, where it is achievable: exactly.**
///
/// This fixture is built so that **no feature crosses a cell edge** — every polygon and every line
/// sits inside one `2^18` cell — so the two maps carry the same geometry with the same vertices,
/// differently addressed, and the graft must be invisible: every frame is bit-identical, at every
/// zoom, north-up and rotated. A single differing pixel here is a graft bug, not a rasteriser
/// artefact.
#[test]
fn rendering_is_pixel_identical_where_no_feature_is_cut() {
    let cfg = config();
    let (ing, ways) = uncut_fixture(&cfg);
    let dir = scratch("uncut");
    let summary = cut(&dir, &cfg, &ing, &ways);
    let bbox = snapped_box(&summary);
    let packed = monolithic(&cfg, &ing, &ways, bbox);
    let (grafted, _) = assembled(&dir, &cfg, &summary);
    let backdrop = backdrop_color(&packed);
    for (name, center, mpp, heading) in uncut_scenes() {
        let a = render(&packed, center, mpp, heading);
        let b = render(&grafted, center, mpp, heading);
        assert!(a.iter().any(|&p| p != backdrop), "scene {name} drew only backdrop — the viewport tests nothing");
        let d = diff(&a, &b);
        assert_eq!(d.count, 0, "scene {name}: {} pixel(s) differ ({:?}) although no feature is cut", d.count, d.pairs);
    }
}

/// (a) **Pixel equivalence across seams: identical, or a one-pixel edge shift — and nothing else.**
///
/// Most of the matrix comes out bit-identical (the count at the bottom pins how much), and the rest
/// differs in a way this test *characterises* rather than tolerates. The mechanism is in the
/// **renderer**, not in the graft, and it is worth stating exactly, because "pixel-identical
/// everywhere" was the goal going in:
///
/// - Cutting a cell inserts a vertex on the cell edge and splits the feature there, so the assembled
///   map strokes two polylines where the monolithic map strokes one.
/// - `obc-render`'s stroker simplifies each polyline **in screen space** (`SIMPLIFY_EPS_PX`, ¾ px)
///   and lays a round join/cap disc at each run's two ends. Neither operation is distributive over
///   splitting a polyline: two halves simplified separately, each with its own end cap, can put a
///   stroke edge one pixel off where the joined line put it. Rotated views show it along the whole
///   stroke; north-up ones usually only at the seam pixel itself.
/// - The monolithic packer splits features at **its** quadtree-leaf boundaries for the same reason,
///   so this is a property of *where geometry was cut*, which OBCA §2.4 already books as a cosmetic
///   cost of cutting at cell boundaries — alongside dash phase, the other one, which this fixture
///   avoids by using no dashed style (a dashed line would legitimately differ along the seam).
///
/// So the assertion is the bound that matters rather than a fuzz factor: **every differing pixel is
/// a boundary that moved by at most one pixel** — the colour each frame draws there is present in the
/// other frame's 8-neighbourhood. A feature drawn in the wrong place, drawn twice, or not drawn at
/// all fails that immediately, which is precisely the class of bug a graft can have.
#[test]
fn rendering_across_seams_differs_only_by_a_one_pixel_edge_shift() {
    let (packed, grafted, _, _) = both("pixels");
    let backdrop = backdrop_color(&packed);
    let scenes = scenes();
    let mut exact = 0usize;
    let mut shifted = Vec::new();
    for (name, center, mpp, heading) in &scenes {
        let a = render(&packed, *center, *mpp, *heading);
        let b = render(&grafted, *center, *mpp, *heading);
        // A frame of nothing but backdrop compares two blank screens and proves nothing.
        assert!(a.iter().any(|&p| p != backdrop), "scene {name} drew only backdrop — the viewport tests nothing");
        let d = diff(&a, &b);
        if d.count == 0 {
            exact += 1;
            continue;
        }
        assert_eq!(
            d.non_edge, 0,
            "scene {name}: {} of {} differing pixels are not a one-pixel edge shift — a feature moved, doubled or \
             vanished. Colour pairs (packed, assembled): {:?}",
            d.non_edge, d.count, d.pairs
        );
        // A shifted stroke edge is one pixel wide along one feature; a whole-frame difference is not.
        let budget = a.len() / 100;
        assert!(
            d.count <= budget,
            "scene {name}: {} differing pixels exceed the {budget}-pixel edge-shift budget ({:?})",
            d.count,
            d.pairs
        );
        shifted.push(format!("{name}: {} px, pairs {:?}", d.count, d.pairs));
    }
    // Pin how much of the matrix is exact, so a change that blurs a currently-identical scene has to
    // be looked at rather than absorbed.
    assert!(
        exact >= 8,
        "only {exact} of {} scenes are pixel-identical (expected ≥ 8); the shifted ones were:\n  {}",
        scenes.len(),
        shifted.join("\n  ")
    );
}

// --- (b) the route oracle ---------------------------------------------------------------------

#[derive(Default)]
struct VecSink(Vec<u8>);

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), obc_formats::io::Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// Plan one route through the real A\*, returning its ground length in metres.
fn route(map: &[u8], from: (i32, i32), to: (i32, i32)) -> Option<u32> {
    let src = SliceSource(map);
    let tables = MapTables::parse(&src).expect("the map parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    let mut scratch = NavScratch::<4096>::new_boxed();
    let mut tiles = NavTileCache::new();
    let mut sink = VecSink::default();
    plan_route(&reader, from, to, "oracle", 0, &mut scratch, &mut tiles, &mut NullElevation, &mut sink)
        .ok()
        .map(|stats| stats.total_distance_m)
}

/// (b) **Route equivalence.** Endpoint pairs whose straight line crosses one or two cell seams must
/// route in both maps, and to the same length.
///
/// The tolerance is not a fudge factor, it is the splice: cutting a way at a cell edge splits an
/// edge into pieces whose `Length M` is re-measured over each sub-polyline (`OBCM_Spec.md` §8.4), so
/// a merged route's total can differ from the monolithic one by the rounding of one metre per split.
/// The bound below is that, generously: 1 % or 5 m, whichever is larger.
#[test]
fn routes_across_cell_seams_agree_with_the_monolithic_pack() {
    let (packed, grafted, _, _) = both("routes");
    type Pair = ((i32, i32), (i32, i32), &'static str);
    let pairs: [Pair; 5] = [
        // West of the seam → the junction east of it: one seam crossing.
        (((SEAM - 60_000) as i32, LAT as i32), ((SEAM + 40_000) as i32, LAT as i32), "one seam"),
        // Across both lon seams — the route neither cell can carry alone.
        (((SEAM - 60_000) as i32, LAT as i32), ((SEAM_E + 40_000) as i32, (LAT + 20_000) as i32), "two seams"),
        // Across the **latitude** seam, up the northern spur.
        (
            ((SEAM + 40_000) as i32, (SEAM_N - 20_000) as i32),
            ((SEAM + 40_000) as i32, (SEAM_N + 60_000) as i32),
            "lat seam",
        ),
        // West of the first seam all the way to the far north — two seams of different axes.
        (((SEAM - 60_000) as i32, LAT as i32), ((SEAM + 40_000) as i32, (SEAM_N + 60_000) as i32), "both axes"),
        // Onto the T-junction spur, so the merged graph's branching is exercised too.
        (((SEAM - 60_000) as i32, LAT as i32), ((SEAM + 40_000) as i32, (LAT + 60_000) as i32), "spur"),
    ];
    let mut routed = 0;
    for (from, to, what) in pairs {
        let a = route(&packed, from, to);
        let b = route(&grafted, from, to);
        match (a, b) {
            (Some(a), Some(b)) => {
                routed += 1;
                // The tolerance is the splice, not a fudge factor (see the doc comment).
                let tolerance = (a as f64 * 0.01).max(5.0);
                assert!(
                    (a as f64 - b as f64).abs() <= tolerance,
                    "{what}: pack(X) routes {a} m but assemble(cut(X)) routes {b} m (tolerance {tolerance:.1} m)"
                );
            }
            // Agreeing that there is no route is also equivalence — but a suite where nothing routes
            // proves nothing, which the count below guards.
            (None, None) => {}
            (a, b) => panic!("{what}: the two maps disagree on routability — pack {a:?}, assemble {b:?}"),
        }
    }
    assert!(routed >= 4, "only {routed} of the seam-crossing pairs routed at all — the fixture stopped testing seams");
}

/// The seam property itself, stated as a graph fact rather than as a route: the two cells either
/// side of a seam materialise the *same* junction coordinate, and after assembly that coordinate is
/// one node, not two (OBCA §3.4/§4.6.2).
#[test]
fn boundary_junctions_unify_into_one_node() {
    let (_, grafted, _, _) = both("junctions");
    let src = SliceSource(&grafted);
    let tables = MapTables::parse(&src).expect("parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    let mut scratch = vec![0u8; obc_reader::NAV_MAX_CHUNK_BYTES];
    let mut on_seam: Vec<(i32, i32, u32, usize)> = Vec::new();
    reader
        .for_each_nav_node(&tables.bbox, &mut scratch, |n| {
            if n.lon as i64 == SEAM {
                on_seam.push((n.lat, n.lon, n.id, n.degree()));
            }
        })
        .expect("the nav walk runs");
    on_seam.sort_unstable();
    on_seam.dedup();
    assert!(!on_seam.is_empty(), "the fixture's roads cross the seam, so a boundary junction must exist");
    // One node per coordinate — two would mean the stubs never unified and the road is severed.
    let mut coords: Vec<(i32, i32)> = on_seam.iter().map(|n| (n.0, n.1)).collect();
    coords.sort_unstable();
    let unique = {
        let mut c = coords.clone();
        c.dedup();
        c.len()
    };
    assert_eq!(unique, coords.len(), "a seam coordinate carries two node ids — unification did not happen");
    // …and it is an ordinary through junction, not a dead end.
    assert!(on_seam.iter().any(|n| n.3 >= 2), "the unified seam junction must join both sides: {on_seam:?}");
}

/// The set the assembler wrote is a legal OBCS set: one core shard spanning the assembly bbox, the
/// manifest last, and the digests matching the bytes (OBCA §5.2/§5.3).
#[test]
fn the_output_is_a_legal_single_file_set() {
    let (_, grafted, store, bbox) = both("set");
    let m = &store.manifest;
    assert_eq!(&m[0..4], b"OBCS");
    assert_eq!(m[4], 1, "manifest version");
    assert_eq!(m[5], obc_formats::obcm::VERSION);
    assert_eq!(m[6], 1, "the single-file fast path is a set of one (§5.5)");
    assert_eq!(m[7], 0, "…and that shard is the core");
    assert_eq!(m.len(), 72 + 56, "72 + 56 × shard count");
    assert_eq!(m[72], 0, "role core");
    assert_eq!(u32::from_le_bytes(m[92..96].try_into().unwrap()) as usize, grafted.len(), "recorded size");
    // The shard bbox in the manifest is the header bbox, verbatim, and both are the assembly bbox.
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    assert_eq!(i32::from_le_bytes(m[76..80].try_into().unwrap()) as i64, min_lat);
    assert_eq!(i32::from_le_bytes(m[80..84].try_into().unwrap()) as i64, min_lon);
    assert_eq!(i32::from_le_bytes(m[84..88].try_into().unwrap()) as i64, max_lat);
    assert_eq!(i32::from_le_bytes(m[88..92].try_into().unwrap()) as i64, max_lon);
    let digest: [u8; 32] = m[96..128].try_into().unwrap();
    assert_eq!(digest.to_vec(), sha256(&grafted), "the manifest's digest is the shard's own");
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().to_vec()
}

/// The POI and hours halves of §4.5: every POI of every cell survives the merge exactly once, the
/// assembly's POI set equals the monolithic pack's, and — the half the name promised — every
/// **schedule** does too.
///
/// `HoursRef` is a file-local pool index, so it is the one POI field that cannot travel verbatim:
/// §4.5.3 rebuilds the pool from the distinct 29-byte blobs and remaps every reference. The fixture
/// puts one schedule on two POIs in *different cells* and a second on a third, so the rebuilt pool
/// must hold exactly two blobs — a merge that kept per-cell pools, or that failed to deduplicate
/// them, produces the right POIs with the wrong hours.
#[test]
fn pois_and_hours_survive_the_merge() {
    let (packed, grafted, _, _) = both("pois");
    /// `(lat, lon, subtype, name, the resolved schedule's own bytes)`.
    type Row = (i32, i32, u8, String, Option<Vec<u8>>);
    let list = |map: &[u8]| -> Vec<Row> {
        let src = SliceSource(map);
        let tables = MapTables::parse(&src).expect("parses");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        let mut out: Vec<Row> = Vec::new();
        for cat in obc_formats::obcm::PoiCategory::ALL {
            let mut found: heapless::Vec<obc_reader::Poi, { obc_reader::MAX_POI_RESULTS }> = heapless::Vec::new();
            reader.nearest_pois(cat, (SEAM as i32, LAT as i32), &mut found).expect("the POI query runs");
            out.extend(found.iter().map(|p| {
                // The pool index itself is *expected* to differ between the two files; the schedule
                // it resolves to is not. Comparing the resolved value is the whole point.
                let hours = reader.poi_hours(p.hours_ref).map(|h| {
                    let mut v: Vec<u8> = vec![h.flags()];
                    for d in 0..7u8 {
                        v.extend(h.today_intervals(d).iter().flat_map(|iv| [iv.open_q, iv.close_q]));
                        v.push(0xFF); // day separator, so two days cannot alias into one sequence
                    }
                    v
                });
                (p.lat, p.lon, p.subtype, p.name.as_str().to_string(), hours)
            }));
        }
        out.sort_unstable();
        out
    };
    let (a, b) = (list(&grafted), list(&packed));
    assert_eq!(a, b, "the assembled POI set — schedules included — must equal the monolithic one");
    assert!(a.iter().filter(|r| r.4.is_some()).count() >= 3, "the fixture must carry POIs with hours: {a:?}");

    // The pool itself: distinct blobs only, and the two POIs that share a schedule share a slot.
    let src = SliceSource(&grafted);
    let tables = MapTables::parse(&src).expect("parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    assert_eq!(reader.poi_directory().hours_pool_count, 2, "two distinct schedules over three POIs (§4.5.3)");
}

/// The engine restates OBCA's grid arithmetic because it may not depend on the packer (libGEOS is a
/// native dependency and the engine compiles for wasm). This is the drift guard the restatement is
/// only acceptable with: both copies must agree, cell for cell.
#[test]
fn the_engine_and_the_packer_agree_on_the_grid() {
    use obcm_assemble::grid::{quad_children, quad_mid, GRID_ORIGIN, MAX_CELL_LOG2, MIN_CELL_LOG2};

    // **Every** permitted cell size, not the three the fixture happens to use: the drift this guards
    // against is a rounding step, and a rounding step is most likely to show up at the ends of the
    // range — the smallest size (where the indices are largest) and the largest (where a cell spans
    // an eighth of the world).
    assert_eq!((MIN_CELL_LOG2, MAX_CELL_LOG2), (obc_pack::grid::MIN_CELL_LOG2, obc_pack::grid::MAX_CELL_LOG2));
    assert_eq!(GRID_ORIGIN, obc_pack::grid::GRID_ORIGIN);
    // …and the third copy: the OBCT terrain raster sits on this same grid (`OBCT_Spec.md` §1.1) but
    // is read by a no_std crate that cannot depend on either host copy, so `obc-formats` restates
    // the origin and the cell-size range. Same drift guard, same reason.
    assert_eq!(GRID_ORIGIN, obc_formats::obct::GRID_ORIGIN as i64);
    assert_eq!(obcm_assemble::grid::WORLD_SIDE, obc_formats::obct::WORLD_SIDE as i64);
    assert_eq!(
        (MIN_CELL_LOG2, MAX_CELL_LOG2),
        (obc_formats::obct::MIN_CELL_LOG2 as u32, obc_formats::obct::MAX_CELL_LOG2 as u32)
    );
    for log2 in MIN_CELL_LOG2..=MAX_CELL_LOG2 {
        let last = obc_pack::grid::axis_cells(log2) - 1;
        assert_eq!(obc_pack::grid::axis_cells(log2), obcm_assemble::grid::axis_cells(log2));
        assert_eq!(obc_pack::grid::id_width(log2), obcm_assemble::grid::id_width(log2), "zero padding at 2^{log2}");
        // The corners, the neighbours of the corners, and the middle of the axis — the indices where
        // a `div_euclid` and a truncating `/` disagree, and the ones either side of them.
        for i in [0i64, 1, last / 2, last / 2 + 1, last - 1, last] {
            for j in [0i64, 1, last / 2, last / 2 + 1, last - 1, last] {
                let p = obc_pack::grid::CellId::new(log2, i, j).expect("valid");
                let e = CellId::new(log2, i, j).expect("valid");
                assert_eq!(p.square(), e.square(), "cell {p} squares differ");
                assert_eq!(p.to_string(), e.to_string(), "canonical ids differ");
                // …and the square's own corners round-trip through `containing` in both copies.
                let (min_lon, min_lat, max_lon, max_lat) = e.square();
                for (lat, lon) in [(min_lat, min_lon), (max_lat - 1, max_lon - 1), (min_lat, max_lon - 1)] {
                    let (pc, ec) =
                        (obc_pack::grid::CellId::containing(log2, lat, lon), CellId::containing(log2, lat, lon));
                    assert_eq!((pc.i, pc.j), (ec.i, ec.j), "containing({lat}, {lon}) at 2^{log2} differs");
                    assert_eq!((ec.i, ec.j), (i, j), "…and must be the cell the square came from");
                }
                // The boundary predicate, on and just off every edge of this square.
                for v in [min_lat, min_lat + 1, min_lat - 1, max_lat, min_lon, max_lon, max_lon - 1] {
                    assert_eq!(
                        obc_pack::grid::on_grid_line(v, log2),
                        obcm_assemble::grid::on_grid_line(v, log2),
                        "the boundary predicate differs at {v} (2^{log2})"
                    );
                }
            }
        }
    }

    // The **quadtree midpoint** is the other half of the alignment theorem: the engine's fresh upper
    // tree and the packer's own trees must split at the same integer, or a depth-`d` node stops
    // being a cell. Checked at the negative origin too, where a truncating division drifts.
    for (min, max) in [
        (0i64, 1i64),
        (0, 2),
        (-1, 1),
        (-3, 0),
        (GRID_ORIGIN, GRID_ORIGIN + (1 << 29)),
        (SEAM, SEAM_E),
        (SEAM - 1, SEAM_N),
        (-7, -2),
    ] {
        assert_eq!(obc_pack::grid::quad_mid(min, max), quad_mid(min, max), "quad_mid({min}, {max}) differs");
    }
    // …and the four child boxes the midpoint produces, in the format's NW/NE/SW/SE order, for a box
    // at the origin and one at the negative corner.
    for b in [
        (SEAM, LAT, SEAM_E, LAT + 262_144),
        (GRID_ORIGIN, GRID_ORIGIN, GRID_ORIGIN + (1 << 22), GRID_ORIGIN + (1 << 22)),
        (-5, -5, 6, 6),
    ] {
        let (min_lon, min_lat, max_lon, max_lat) = b;
        let (mid_lon, mid_lat) =
            (obc_pack::grid::quad_mid(min_lon, max_lon), obc_pack::grid::quad_mid(min_lat, max_lat));
        let want = [
            (min_lon, mid_lat, mid_lon, max_lat),
            (mid_lon, mid_lat, max_lon, max_lat),
            (min_lon, min_lat, mid_lon, mid_lat),
            (mid_lon, min_lat, max_lon, mid_lat),
        ];
        assert_eq!(quad_children(b), want, "the child boxes of {b:?} are not the packer's midpoints");
    }
}

// --- volume sets (OBCA §5) --------------------------------------------------------------------

/// Forcing the split path: with a one-byte shard target, the fixture becomes a **multi-file set**,
/// and every §5.1/§5.3 invariant has to hold — one core spanning the assembly bbox and carrying no
/// geometry, one coarse shard spanning it too, geometry shards tiling it without overlap, every file
/// listing the full ladder, and nav + POIs only in the core.
///
/// The same graft bytes go out either way, so the test also pins the property that makes sharding
/// safe: **the set's geometry is exactly the single file's**, chunk for chunk.
#[test]
fn a_forced_split_produces_a_legal_volume_set() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("set-split");
    let cut_summary = cut(&dir, &cfg, &ing, &ways);

    let single = Options { name: "Single".into(), accept_partial: true, ..Default::default() };
    let (one, _) = assemble_with(&dir, &cfg, &cut_summary, &single).expect("the single-file assembly runs");
    assert_eq!(one.shards.len(), 1);

    let split = Options { target_shard_bytes: 1, force_split: true, ..single.clone() };
    let (set, store) = assemble_with(&dir, &cfg, &cut_summary, &split).expect("the split assembly runs");
    assert!(set.shards.len() > 2, "a one-byte target must split into core + coarse + geometry shards");
    assert_eq!(store.shards.len(), set.shards.len());

    let cores: Vec<_> = set.shards.iter().filter(|s| s.role == obcm_assemble::BandRole::Core).collect();
    assert_eq!(cores.len(), 1, "exactly one core shard (§5.3)");
    assert_eq!(cores[0].bbox, set.assembly_box, "the core spans the assembly bbox");
    assert_eq!(cores[0].index, 0, "the core is shard 0 here, and the manifest says which");
    assert_eq!(store.manifest[7] as usize, cores[0].index);
    assert_eq!(store.manifest[6] as usize, set.shards.len(), "shard count");
    assert!(set.shards.iter().any(|s| s.role == obcm_assemble::BandRole::Coarse), "a coarse shard exists");
    assert!(set.shards.iter().any(|s| s.role == obcm_assemble::BandRole::Geometry), "geometry shards exist");

    // Every shard verified (the engine ran §4.8 on each) and lists the full ladder; sections live
    // only in the core.
    let mut totals = vec![0u64; cfg.lods.len()];
    for s in &set.shards {
        let report = s.verify.as_ref().expect("every shard is verified before the manifest");
        let src = SliceSource(&store.shards[s.index].0);
        let tables = MapTables::parse(&src).expect("each shard is a valid OBCM file on its own");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        assert_eq!(reader.lods().len(), cfg.lods.len(), "every shard lists the full ladder (§5.1)");
        assert!(!reader.nav_profiles().is_empty(), "every shard carries the profile table");
        let core = s.role == obcm_assemble::BandRole::Core;
        assert_eq!(reader.nav_directory().is_empty(), !core, "the nav graph lives only in the core (§5.1)");
        assert_eq!(report.nav_nodes > 0, core);
        for (i, l) in reader.lods().iter().enumerate() {
            totals[i] += l.chunk_count as u64;
        }
        // A geometry/coarse shard's bbox is a node of the assembly quadtree, inside it.
        assert!(s.bbox.span_log2 <= set.assembly_box.span_log2);
    }

    // The set carries exactly the geometry the single file does.
    let single_bytes = {
        let (bytes, _) = assembled(&dir, &cfg, &cut_summary);
        bytes
    };
    let src = SliceSource(&single_bytes);
    let tables = MapTables::parse(&src).expect("parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    let expected: Vec<u64> = reader.lods().iter().map(|l| l.chunk_count as u64).collect();
    assert_eq!(totals, expected, "the shards' chunks must add up to the single file's, level by level");
}

/// **Two geometry bands, one tiling** — the shape of the real v1 schema, and the case a per-band
/// split gets wrong.
///
/// §5.1 partitions a set by **role**, not by band: "geometry shards carry the `mid`- and
/// `fine`-band LODs and nothing else", and "the shards of one role tile the assembly bbox". At the
/// v1 table `mid` (`2^19`) and `fine` (`2^18`) are two bands of one role, so a planner that tiles
/// per band emits two overlapping antichains of `Role == 1` shards whose areas sum to twice the
/// assembly — which §5.3 rejects, *after* every shard has been written, leaving a directory of
/// orphans and no manifest.
///
/// This test therefore asserts the property directly: with a one-byte target and two geometry bands,
/// the `Role == 1` shards must be **one** antichain — pairwise disjoint, covering the assembly bbox
/// exactly once — and each of them must carry both bands' LODs, with the whole set's chunk counts
/// still adding up to the single file's level by level.
#[test]
fn two_geometry_bands_share_one_tiling() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("two-geometry-bands");
    let cut_summary = cut_with(&dir, &cfg, &ing, &ways, BANDS_TWO_GEOMETRY);
    assert!(
        cut_summary.cells.iter().any(|c| c.band == "mid") && cut_summary.cells.iter().any(|c| c.band == "fine"),
        "the cutter must have written both geometry bands"
    );

    let base = Options { name: "TwoBands".into(), accept_partial: true, ..Default::default() };
    let (one, _) = assemble_bands(&dir, &cfg, &cut_summary, &base, BANDS_TWO_GEOMETRY).expect("single file");
    assert_eq!(one.shards.len(), 1, "the fixture still fits one file");

    let split = Options { target_shard_bytes: 1, force_split: true, ..base };
    let (set, store) =
        assemble_bands(&dir, &cfg, &cut_summary, &split, BANDS_TWO_GEOMETRY).expect("the split assembly runs");

    let geometry: Vec<&obcm_assemble::ShardSummary> =
        set.shards.iter().filter(|s| s.role == obcm_assemble::BandRole::Geometry).collect();
    assert!(geometry.len() > 1, "a one-byte target must split the geometry role into several shards");
    // One antichain: the squares are pairwise disjoint and their areas add up to the assembly's
    // exactly once. Two tilings would double the sum — the bug this test exists for.
    let area = |b: obcm_assemble::grid::AlignedBox| 1u128 << (2 * b.span_log2);
    assert_eq!(
        geometry.iter().map(|s| area(s.bbox)).sum::<u128>(),
        area(set.assembly_box),
        "the geometry shards must tile the assembly bbox exactly once"
    );
    for (i, a) in geometry.iter().enumerate() {
        for b in &geometry[i + 1..] {
            let (a0, a1, a2, a3) = a.bbox.ubox();
            let (b0, b1, b2, b3) = b.bbox.ubox();
            assert!(!(a0 < b2 && b0 < a2 && a1 < b3 && b1 < a3), "geometry shards {} and {} overlap", a.index, b.index);
        }
    }

    // Each geometry shard carries the **union** of the two bands' LODs (§5.1), and no shard of any
    // other role carries either — checked through the reader, on the bytes.
    let lods_present = |index: usize| -> Vec<usize> {
        let src = SliceSource(&store.shards[index].0);
        let tables = MapTables::parse(&src).expect("a shard parses");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        assert_eq!(reader.lods().len(), cfg.lods.len(), "every shard lists the full ladder (§5.1)");
        reader.lods().iter().enumerate().filter(|(_, l)| l.node_count > 0).map(|(i, _)| i).collect()
    };
    let carried: Vec<usize> = geometry.iter().flat_map(|s| lods_present(s.index)).collect();
    assert!(carried.contains(&1) && carried.contains(&2), "the geometry shards carry both mid (LOD 1) and fine (2)");
    for s in set.shards.iter().filter(|s| s.role != obcm_assemble::BandRole::Geometry) {
        let other = lods_present(s.index);
        assert!(!other.contains(&1) && !other.contains(&2), "shard {} carries geometry-role LODs", s.index);
    }

    // …and the split moved bytes, it did not invent or lose them.
    let mut totals = vec![0u64; cfg.lods.len()];
    for s in &set.shards {
        s.verify.as_ref().expect("every shard is verified before the manifest");
        let src = SliceSource(&store.shards[s.index].0);
        let tables = MapTables::parse(&src).expect("parses");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        for (i, l) in reader.lods().iter().enumerate() {
            totals[i] += l.chunk_count as u64;
        }
    }
    let single = {
        let src = SliceSource(&store.shards[0].0);
        let _ = &src;
        let (single, single_store) = assemble_bands(
            &dir,
            &cfg,
            &cut_summary,
            &Options { accept_partial: true, ..Default::default() },
            BANDS_TWO_GEOMETRY,
        )
        .expect("single file");
        assert_eq!(single.shards.len(), 1);
        single_store.shards[0].0.clone()
    };
    let src = SliceSource(&single);
    let tables = MapTables::parse(&src).expect("parses");
    let cache = MapCache::new_boxed();
    let reader = Reader::new(&src, &tables, &cache);
    let expected: Vec<u64> = reader.lods().iter().map(|l| l.chunk_count as u64).collect();
    assert_eq!(totals, expected, "the set's chunks must add up to the single file's, level by level");
    assert_eq!(store.manifest[6] as usize, set.shards.len(), "the manifest was written, so the set validated");
}

// --- input refusals and the degenerate selections (§4.1) ---------------------------------------

/// Everything a `CellInput` needs, owned, so a test can reorder, duplicate or corrupt the list.
struct Loaded {
    cells: Vec<(CellId, String, bool)>,
    bytes: Vec<MemorySource>,
}

fn load(dir: &Path, summary: &CutSummary) -> Loaded {
    Loaded {
        cells: summary.cells.iter().map(|c| (to_engine_cell(c.id), c.band.clone(), c.partial)).collect(),
        bytes: summary.cells.iter().map(|c| MemorySource(std::fs::read(dir.join(&c.path)).expect("cell"))).collect(),
    }
}

impl Loaded {
    /// Run the engine over exactly the cells at `pick` (indices into the loaded list, repeats
    /// allowed), with `opts`.
    fn assemble(
        &self,
        cfg: &Config,
        pick: &[usize],
        opts: &Options,
    ) -> Result<(obcm_assemble::Summary, MemoryStore), obcm_assemble::Error> {
        let inputs: Vec<CellInput<'_>> = pick
            .iter()
            .map(|&k| CellInput {
                id: self.cells[k].0,
                band: self.cells[k].1.clone(),
                src: &self.bytes[k],
                partial: self.cells[k].2,
            })
            .collect();
        let mut store = MemoryStore::default();
        let out = assemble(inputs, &schema(cfg), &skin(cfg), opts, &mut store, &NoClock)?;
        Ok((out, store))
    }

    fn index_of(&self, band: &str) -> usize {
        self.cells.iter().position(|c| c.1 == band).unwrap_or_else(|| panic!("no {band} cell"))
    }
}

/// The degenerate and mis-stated selections. Each is a case a caller can actually produce — a
/// catalog listing a cell twice, a one-cell corridor, a `network` cell with no roads in it, a band
/// id that does not exist — and each has a specific right answer that is not "assemble something".
#[test]
fn the_degenerate_selections_behave() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("degenerate");
    let summary = cut(&dir, &cfg, &ing, &ways);
    let loaded = load(&dir, &summary);
    let all: Vec<usize> = (0..loaded.cells.len()).collect();
    let opts = Options { name: "Degenerate".into(), accept_partial: true, accept_holes: true, ..Default::default() };

    // A cell listed twice. Geometry would survive it (the graft keys cells by grid slot), but the
    // nav merge mints fresh ids per copy, so the interior graph would silently double — and §4.8
    // would verify the result as correct.
    let mut twice = all.clone();
    twice.push(loaded.index_of("network"));
    let err = format!("{}", loaded.assemble(&cfg, &twice, &opts).expect_err("a duplicate cell must be refused"));
    assert!(err.contains("listed twice"), "got: {err}");

    // One cell, on its own: the smallest legal assembly. The bbox snaps to `S_MAX` (2^20 here), so
    // the single `2^18` cell sits in one corner and every other leaf is empty.
    let one = loaded.index_of("network");
    let (single, _) = loaded.assemble(&cfg, &[one], &opts).expect("a one-cell assembly is legal");
    assert_eq!(single.shards.len(), 1);
    assert_eq!(single.assembly_box.span_log2, 20, "the box snaps to S_MAX even for one 2^18 cell (§4.2)");
    assert!(single.assembly_box.contains_cell(loaded.cells[one].0));

    // A `network` cell with no nav at all — the fixture's roads do not reach every cell. The merge
    // must skip it rather than trip over an empty directory, and the graph must still come out.
    let empty_nav = loaded
        .cells
        .iter()
        .enumerate()
        .filter(|(_, c)| c.1 == "network")
        .find(|(k, _)| {
            let src = SliceSource(&loaded.bytes[*k].0);
            let tables = MapTables::parse(&src).expect("a cell parses");
            let cache = MapCache::new_boxed();
            Reader::new(&src, &tables, &cache).nav_directory().is_empty()
        })
        .map(|(k, _)| k);
    if let Some(k) = empty_nav {
        let with_roads = loaded.index_of("network");
        let pick = if k == with_roads { vec![k] } else { vec![k, with_roads] };
        let (out, _) = loaded.assemble(&cfg, &pick, &opts).expect("an empty-nav cell assembles");
        assert!(out.shards[0].verify.is_some(), "…and verifies");
    }

    // A band the schema does not name: refused before a byte is read, because the band is what
    // decides which LODs and sections a cell contributes (§3.1 — the bytes cannot say).
    let mut store = MemoryStore::default();
    let bogus =
        vec![CellInput { id: loaded.cells[one].0, band: "not-a-band".into(), src: &loaded.bytes[one], partial: false }];
    let err = format!(
        "{}",
        assemble(bogus, &schema(&cfg), &skin(&cfg), &opts, &mut store, &NoClock)
            .expect_err("a band outside the schema must be refused")
    );
    assert!(err.contains("is not in the schema"), "got: {err}");
}

/// The malformed-cell paths. A cell is an **input**, not this crate's own output, so every one of
/// these is reachable from a corrupt download or a bad bake — and each must come out as
/// [`obcm_assemble::Error::Format`] naming the cell, never as a panic, a wrapped index, or a map
/// that quietly draws the wrong thing.
#[test]
fn a_malformed_cell_is_a_format_error() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("malformed");
    let summary = cut(&dir, &cfg, &ing, &ways);
    let opts = Options { accept_partial: true, accept_holes: true, ..Default::default() };

    // Corrupt the first cell whose bytes `patch` can find something to break in — it returns
    // `false` when this cell/LOD does not carry the shape it wants — then assemble everything.
    let broken = |patch: &dyn Fn(&mut Vec<u8>, &obc_reader::Lod) -> bool| -> String {
        let mut loaded = load(&dir, &summary);
        let mut hit = false;
        'outer: for k in 0..loaded.cells.len() {
            let lods = {
                let src = SliceSource(&loaded.bytes[k].0);
                let tables = MapTables::parse(&src).expect("parses");
                let cache = MapCache::new_boxed();
                let reader = Reader::new(&src, &tables, &cache);
                reader.lods().to_vec()
            };
            for lod in lods.iter().filter(|l| l.node_count > 0 && l.chunk_count > 0) {
                if patch(&mut loaded.bytes[k].0, lod) {
                    hit = true;
                    break 'outer;
                }
            }
        }
        assert!(hit, "the fixture must contain the shape this case corrupts");
        let all: Vec<usize> = (0..loaded.cells.len()).collect();
        let err = loaded.assemble(&cfg, &all, &opts).expect_err("a corrupt cell must be refused");
        assert!(matches!(err, obcm_assemble::Error::Format(_)), "expected Error::Format, got {err:?}");
        format!("{err}")
    };

    // 1. Not an OBCM file at all.
    let err = broken(&|b, _| {
        b[0] = b'X';
        true
    });
    assert!(err.contains("not a readable OBCM"), "got: {err}");

    // 2. A header bbox that is not the cell's grid square — the fact the whole graft rests on
    //    (§3.1), and the one the alignment theorem cannot survive being wrong about.
    //    `Max Lat` is bytes 13..17 (`OBCM_Spec.md` §1); nudging its low byte keeps the file
    //    parseable and every other invariant intact, so the grid-square check is the only thing
    //    standing between this cell and a mis-aligned graft.
    let err = broken(&|b, _| {
        b[13] = b[13].wrapping_add(1);
        true
    });
    assert!(err.contains("is not its grid square"), "got: {err}");

    // 3. A leaf index word naming a chunk the cell does not have. Relocated blindly it would point
    //    into the *next* cell's chunks — geometry from somewhere else, drawn without complaint.
    let err = broken(&|b, lod| {
        let Some(word) = (0..lod.node_count).map(|k| lod.index_offset + k * 4).find(|&at| {
            let v = u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
            v & obc_formats::obcm::BRANCH_BIT == 0 && v != obc_formats::obcm::EMPTY_LEAF
        }) else {
            return false;
        };
        b[word..word + 4].copy_from_slice(&(lod.chunk_count as u32 + 7).to_le_bytes());
        true
    });
    assert!(err.contains("names chunk"), "got: {err}");

    // 4. A branch whose children fall outside the cell's index.
    let err = broken(&|b, lod| {
        let Some(word) = (0..lod.node_count)
            .map(|k| lod.index_offset + k * 4)
            .find(|&at| u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) & obc_formats::obcm::BRANCH_BIT != 0)
        else {
            return false;
        };
        let v = obc_formats::obcm::BRANCH_BIT | (lod.node_count as u32 + 3);
        b[word..word + 4].copy_from_slice(&v.to_le_bytes());
        true
    });
    assert!(err.contains("children start at"), "got: {err}");

    // 5. An offset table pair that spans more than the chunk capacity — §5.1's own bound,
    //    re-checked on the way in because a copied violation would poison the assembly (§4.4.4).
    let err = broken(&|b, lod| {
        // The *last* entry is the region's byte total, which the reader itself validates, so break
        // an interior pair instead — one the reader accepts and only §4.4.4 catches.
        if lod.chunk_count < 2 {
            return false;
        }
        let table = lod.index_offset + lod.node_count * 4;
        b[table + 4..table + 8].copy_from_slice(&(lod.chunk_size as u32 + 1).to_le_bytes());
        true
    });
    assert!(err.contains("spans") || err.contains("runs backwards"), "got: {err}");

    // 6. …and `offsets[0]`, which the format fixes at 0.
    let err = broken(&|b, lod| {
        let table = lod.index_offset + lod.node_count * 4;
        b[table..table + 4].copy_from_slice(&9u32.to_le_bytes());
        true
    });
    assert!(err.contains("offsets[0]"), "got: {err}");
}

/// §4.7's stamp, end to end: the skin's values — including the two style-record flag bits and the trailing
/// `color2` — are what the assembled file's style table carries, at the schema's own ids.
#[test]
fn the_skin_is_stamped_onto_the_output() {
    let (packed, grafted, _, _) = both("skin");
    let table = |map: &[u8]| -> Vec<u8> {
        let style_offset = u32::from_le_bytes(map[21..25].try_into().unwrap()) as usize;
        let count = map[style_offset] as usize;
        map[style_offset..style_offset + 1 + count * obc_formats::obcm::STYLE_RECORD_LEN].to_vec()
    };
    // The skin reproduces the config's styling exactly, so the two tables must be byte-identical —
    // which is also what makes the pixel oracle a comparison of *layout* and nothing else.
    assert_eq!(table(&grafted), table(&packed), "the stamped style table must equal the packer's");
    // The dashed / `color2` record specifically: both flag bits set and the second colour present.
    let cfg = config();
    let dashed = style_id(&cfg, "highway", "path");
    let t = table(&grafted);
    let at = 1 + t[1..]
        .chunks_exact(obc_formats::obcm::STYLE_RECORD_LEN)
        .position(|r| r[0] == dashed)
        .expect("the dashed style is in the table")
        * obc_formats::obcm::STYLE_RECORD_LEN;
    let flags = t[at + 5];
    assert_ne!(flags & obc_formats::obcm::STYLE_DASHED_BIT, 0, "the dash bit survived the stamp");
    assert_ne!(flags & obc_formats::obcm::STYLE_HAS_COLOR2_BIT, 0, "…and so did the color2 bit");
    assert_eq!(u16::from_le_bytes([t[at + 6], t[at + 7]]), 0x07FF, "…with the skin's own second colour");
    assert_eq!(flags & obc_formats::obcm::STYLE_PRIORITY_MASK, 1, "priority 2 ⇒ bits 0-1 = 1");
}

/// What the §4.6 merge reports about itself, asserted rather than printed. The seam count and the
/// island prune are the two numbers that say the merge did its job: a graph that unified nothing
/// has severed every road at a cell edge, and one that pruned nothing never ran §4.6.4 at all.
#[test]
fn the_merge_reports_the_seams_it_unified_and_the_islands_it_pruned() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("nav-stats");
    let summary = cut(&dir, &cfg, &ing, &ways);
    let loaded = load(&dir, &summary);
    let all: Vec<usize> = (0..loaded.cells.len()).collect();
    let opts = Options { accept_partial: true, ..Default::default() };
    let (out, _) = loaded.assemble(&cfg, &all, &opts).expect("the assembly runs");
    let nav = &out.stats.nav;

    assert!(nav.unified > 0, "the fixture's roads cross three grid lines, so stubs must have unified: {nav:?}");
    assert_eq!(
        nav.cell_nodes - nav.unified,
        nav.nodes + nav.pruned_nodes,
        "every cell record is unified, kept or pruned"
    );
    // The islet way is strictly interior and below `min_component_edges`, so §4.6.4 must drop it —
    // at merge time, over the *merged* graph, which is the only place the threshold means what it
    // says (§3.5 defers it from the bake).
    assert!(nav.pruned_nodes > 0, "the seam-crossing islet must be pruned at merge time: {nav:?}");
    assert_eq!((nav.components_found, nav.components_kept), (2, 1), "the islet is the second component");
    // The §4.8.5 report is taken *before* the prune, so the islet still counts against it here…
    assert!(nav.largest_component_permille > 850, "the through network dominates the merged graph: {nav:?}");
    // …and the shard that was actually written is one connected component, which is what a rider
    // gets. A broken seam would show up as a much smaller number in exactly this field.
    assert_eq!(out.shards[0].verify.as_ref().expect("verified").largest_component_permille, 1000);
    assert_eq!((nav.degree_truncated, nav.dropped_nodes), (0, 0), "nothing hit a cap in a fixture this small");
    assert_eq!(out.warnings, Vec::<String>::new(), "…so there is nothing to warn about either");
}

/// The graft's own arithmetic, seen from outside: a hole in the middle of a band's coverage is an
/// **empty leaf**, not a missing subtree — the map still parses, still verifies, and the surviving
/// cells' chunks are all still there.
#[test]
fn a_hole_becomes_an_empty_leaf_and_the_rest_still_grafts() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("hole");
    let summary = cut(&dir, &cfg, &ing, &ways);
    let loaded = load(&dir, &summary);
    let all: Vec<usize> = (0..loaded.cells.len()).collect();
    let opts = Options { accept_partial: true, accept_holes: true, ..Default::default() };
    let (whole, whole_store) = loaded.assemble(&cfg, &all, &opts).expect("the full assembly runs");

    // The cell to drop has to be one that actually carries geometry, or "the hole is visible" is a
    // statement about nothing.
    let victim = all
        .iter()
        .copied()
        .filter(|&k| loaded.cells[k].1 == "fine")
        .find(|&k| {
            let src = SliceSource(&loaded.bytes[k].0);
            let tables = MapTables::parse(&src).expect("parses");
            let cache = MapCache::new_boxed();
            Reader::new(&src, &tables, &cache).lods().iter().any(|l| l.chunk_count > 0)
        })
        .expect("a fine cell with geometry");
    let holed: Vec<usize> = all.iter().copied().filter(|&k| k != victim).collect();
    let (out, store) = loaded.assemble(&cfg, &holed, &opts).expect("an accepted hole assembles");
    assert_eq!(out.assembly_box, whole.assembly_box, "dropping a cell must not move the bbox (§4.2)");

    let counts = |bytes: &[u8]| -> Vec<usize> {
        let src = SliceSource(bytes);
        let tables = MapTables::parse(&src).expect("parses");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        reader.lods().iter().map(|l| l.chunk_count).collect()
    };
    let (with, without) = (counts(&whole_store.shards[0].0), counts(&store.shards[0].0));
    assert_eq!(with.len(), without.len(), "the ladder is unchanged");
    assert!(with.iter().zip(&without).any(|(a, b)| a > b), "the dropped cell's chunks are gone: {with:?} {without:?}");
    assert!(with.iter().zip(&without).all(|(a, b)| a >= b), "…and nothing else was invented: {with:?} {without:?}");
    // The §4.8 verify ran over the holed map and decoded every remaining feature.
    assert!(out.shards[0].verify.as_ref().expect("verified").features > 0);
}

/// The §4.1 refusals. Each one is a case where proceeding quietly would ship a map that looks fine
/// and is not: a hole in the coverage, an under-covered border cell presented as canonical, or a
/// skin that does not belong to the schema the cells were baked at.
#[test]
fn the_assembler_refuses_what_the_spec_says_it_must() {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let dir = scratch("refusals");
    let full = cut(&dir, &cfg, &ing, &ways);

    // `partial`: the fixture's declared source box does not cover every cell square, so the cutter
    // marked some cells partial — and the assembler must not accept them silently.
    assert!(full.cells.iter().any(|c| c.partial), "the fixture must produce at least one partial cell");
    let strict = Options { name: "Strict".into(), accept_partial: false, ..Default::default() };
    let err = assemble_with(&dir, &cfg, &full, &strict).expect_err("a partial cell must be refused");
    assert!(format!("{err}").contains("partial"), "got: {err}");

    // A hole: drop one fine cell from the selection. The assembly is still *legal* (empty leaves),
    // but only if the caller said so.
    let mut holed = full.clone();
    let victim = holed.cells.iter().position(|c| c.band == "fine").expect("a fine cell");
    holed.cells.remove(victim);
    let accept_partial = Options { accept_partial: true, ..strict.clone() };
    let err = assemble_with(&dir, &cfg, &holed, &accept_partial).expect_err("an unaccepted hole must be refused");
    assert!(format!("{err}").contains("hole"), "got: {err}");
    let accept_both = Options { accept_holes: true, ..accept_partial.clone() };
    assemble_with(&dir, &cfg, &holed, &accept_both).expect("an accepted hole assembles");

    // A skin from another schema revision: its ids are not the ids in the cells' chunk bytes.
    let sources: Vec<MemorySource> =
        full.cells.iter().map(|c| MemorySource(std::fs::read(dir.join(&c.path)).expect("cell"))).collect();
    let inputs: Vec<CellInput<'_>> = full
        .cells
        .iter()
        .zip(&sources)
        .map(|(c, src)| CellInput { id: to_engine_cell(c.id), band: c.band.clone(), src, partial: c.partial })
        .collect();
    let mut wrong = skin(&cfg);
    wrong.styles.truncate(2);
    let mut store = MemoryStore::default();
    let err = assemble(inputs, &schema(&cfg), &wrong, &accept_partial, &mut store, &NoClock)
        .expect_err("a skin that does not cover the cells' style ids must be refused");
    assert!(format!("{err}").contains("style table"), "got: {err}");
}
