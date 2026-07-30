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
            "track":       {"color": "0x8410", "weight": 1, "z_index": 3, "min_lod": 2}
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

fn rect(style_id: u8, min_lod: usize, lat0: i64, lon0: i64, lat1: i64, lon1: i64) -> IngestFeature {
    let ring = vec![
        (deg(lon0), deg(lat0)),
        (deg(lon1), deg(lat0)),
        (deg(lon1), deg(lat1)),
        (deg(lon0), deg(lat1)),
        (deg(lon0), deg(lat0)),
    ];
    IngestFeature { style_id, min_lod, geom: Geom::Polygon { exterior: ring, interiors: Vec::new() } }
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

/// The extract both paths consume: geometry and roads crossing three grid lines (two lon, one lat),
/// features wholly inside single cells, and POIs spread across the network cells.
fn fixture(cfg: &Config) -> (Ingested, Vec<RoutableWay>) {
    let water = style_id(cfg, "natural", "water");
    let forest = style_id(cfg, "landuse", "forest");
    let primary = style_id(cfg, "highway", "primary");
    let residential = style_id(cfg, "highway", "residential");
    let track = style_id(cfg, "highway", "track");

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
        // A tiny interior islet: strictly inside one cell and below the threshold, so both paths
        // prune it — the bake at cut time, the assembler at merge time.
        way(7, &[(90, (LAT - 90_000, SEAM + 120_000)), (91, (LAT - 89_000, SEAM + 121_000))]),
    ];
    let pois = vec![
        poi(1, LAT, SEAM - 20_000, "West water"),
        poi(5, LAT + 5_000, SEAM + 15_000, "East camp"),
        poi(13, LAT + 25_000, SEAM_E + 10_000, "Far shop"),
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

/// A scratch directory for one test's cell tree. No `tempfile` dependency: the path is derived from
/// the test's own name, and the directory is recreated per run.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obcm-assemble-oracle-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// Cut the fixture into cells with the **real cutter**.
fn cut(dir: &Path, cfg: &Config, ing: &Ingested, ways: &[RoutableWay]) -> CutSummary {
    let opts = CutOptions {
        bands: BandTable::parse(BANDS).expect("band table"),
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
    let (bytes, dropped) =
        serialize_lods(&lods, &cfg.styles(), cfg.marker_color, bbox, &ing.pois, &graph, &cfg.routing.profiles);
    assert_eq!(dropped, 0, "the fixture must not lose features to the chunk cap");
    bytes
}

/// The engine's schema for this fixture: the cutter's band table plus the config's ladder.
fn schema(cfg: &Config) -> Schema {
    let bands: Vec<obcm_assemble::Band> =
        serde_json::from_value(serde_json::from_str::<serde_json::Value>(BANDS).unwrap()["bands"].clone())
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
    let styles = cfg
        .styles()
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
    let out = assemble(inputs, &schema(cfg), &skin(cfg), opts, &mut store, &NoClock)?;
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
    plan_route(&reader, from, to, "oracle", 0, &mut scratch, &mut tiles, &mut sink)
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

/// The POI and hours halves of §4.5: every POI of every cell survives the merge exactly once, and
/// the assembly's POI set equals the monolithic pack's.
#[test]
fn pois_and_hours_survive_the_merge() {
    let (packed, grafted, _, _) = both("pois");
    let list = |map: &[u8]| -> Vec<(i32, i32, u8)> {
        let src = SliceSource(map);
        let tables = MapTables::parse(&src).expect("parses");
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&src, &tables, &cache);
        let mut out = Vec::new();
        for cat in obc_formats::obcm::PoiCategory::ALL {
            let mut found: heapless::Vec<obc_reader::Poi, { obc_reader::MAX_POI_RESULTS }> = heapless::Vec::new();
            reader.nearest_pois(cat, (SEAM as i32, LAT as i32), &mut found).expect("the POI query runs");
            out.extend(found.iter().map(|p| (p.lat, p.lon, p.subtype)));
        }
        out.sort_unstable();
        out
    };
    assert_eq!(list(&grafted), list(&packed), "the assembled POI set must equal the monolithic one");
}

/// The engine restates OBCA's grid arithmetic because it may not depend on the packer (libGEOS is a
/// native dependency and the engine compiles for wasm). This is the drift guard the restatement is
/// only acceptable with: both copies must agree, cell for cell.
#[test]
fn the_engine_and_the_packer_agree_on_the_grid() {
    for log2 in [18u32, 19, 20] {
        let last = obc_pack::grid::axis_cells(log2) - 1;
        for i in [0i64, 1, 1204.min(last), last] {
            for j in [0i64, 1052.min(last), 1053.min(last), last] {
                let p = obc_pack::grid::CellId::new(log2, i, j).expect("valid");
                let e = CellId::new(log2, i, j).expect("valid");
                assert_eq!(p.square(), e.square(), "cell {p} squares differ");
                assert_eq!(p.to_string(), e.to_string(), "canonical ids differ");
            }
        }
    }
    for v in [SEAM, SEAM_E, SEAM_N, LAT, SEAM + 1, obcm_assemble::grid::GRID_ORIGIN] {
        for log2 in [18u32, 20] {
            assert_eq!(
                obc_pack::grid::on_grid_line(v, log2),
                obcm_assemble::grid::on_grid_line(v, log2),
                "the boundary predicate differs at {v} (2^{log2})"
            );
        }
    }
    // The packer's `containing` and the engine's must place a coordinate in the same cell.
    for lat in [LAT, SEAM_N, SEAM_N - 1, -33_900_000] {
        for lon in [SEAM, SEAM - 1, SEAM_E, 18_400_000] {
            let p = obc_pack::grid::CellId::containing(18, lat, lon);
            let e = CellId::containing(18, lat, lon);
            assert_eq!((p.i, p.j), (e.i, e.j), "containing({lat}, {lon}) differs");
        }
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
