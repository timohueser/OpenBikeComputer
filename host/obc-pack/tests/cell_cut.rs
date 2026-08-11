//! End-to-end cell cutting: cut a synthetic extract into grid cells with the real cutter, then read
//! every artifact back with the real `obc-reader` — the acceptance suite of #1018 / `OBCA_Spec.md`
//! §3.
//!
//! The fixture is deliberately synthetic and deliberately *placed*: everything sits on the spec's own
//! worked-example seam (OBCA §7, the fine-band lon line at `7 602 176` µdeg above Basel) with a
//! second crossing at `7 864 320`, which is simultaneously a `2^18` and a `2^19` line. Simplify
//! tolerances are zero throughout, so a clipped vertex is *exactly* the interpolated crossing and the
//! adjacency assertions can be equalities rather than tolerances — which is the whole point of the
//! seam contract.
//!
//! What each test pins:
//!
//! - [`two_runs_are_byte_identical`] — determinism (§3.2), across the parallel per-cell path.
//! - [`adjacent_cells_meet_exactly_at_the_seam`] — the boundary junctions of two neighbours coincide
//!   to the microdegree and their clipped geometry endpoints meet (§3.3/§3.4).
//! - [`every_cell_round_trips_through_the_reader`] — a cell is a valid OBCM whose header bbox is its
//!   square, with the complete ladder and out-of-band levels genuinely empty (§3.1).
//! - [`sections_live_only_in_the_band_that_carries_them`] — nav/POI in the core band only (§3.1/§3.6).
//! - [`island_pruning_is_strictly_interior`] — §3.5, through the whole cut.
//! - [`partial_marking_follows_declared_coverage`] — §3.7.
//! - [`a_real_pbf_cuts_into_cells`] — the ingest→cut path over the committed corpus fixture.
//! - [`a_terrain_fed_cut_agrees_across_the_seam`] — OBCM v12's §8.3 ascent is integrated from the
//!   *global* OBCT lattice, so two neighbours' stubs agree and sum to the uncut way (#1073).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use obc_map_scene::BBox;
use obc_pack::config::Config;
use obc_pack::cut::{cut_ingested, CutOptions, CutSummary, SourceExtent};
use obc_pack::geom::Geom;
use obc_pack::grid::{BandTable, CellId, UBox};
use obc_pack::ingest::{IngestFeature, Ingested};
use obc_pack::nav::{integrate_edge_ascent, RoutableWay};
use obc_pack::poi::Poi;
use obc_pack::progress::Progress;
use obc_pack::terrain::TerrainSet;
use obc_reader::{MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// The band-`2^18` lon line between cells `j = 1052` and `j = 1053` — OBCA §7's worked-example seam.
const SEAM: i64 = 7_602_176;
/// The next lon line east, which is also a `2^19` line.
const SEAM_E: i64 = 7_864_320;
/// A latitude comfortably inside row 1204 of every band the fixture uses.
const LAT: i64 = 47_300_000;

/// A three-level ladder with **no simplification**: a clipped vertex is then exactly the crossing
/// coordinate, so seam equality is testable.
const CONFIG: &str = r#"{
    "lods": [
        {"max_mpp": null, "simplify": 0},
        {"max_mpp": 20, "simplify": 0},
        {"max_mpp": 4, "simplify": 0}
    ],
    "features": {
        "highway": { "residential": {"color": "0xF800", "weight": 2, "min_lod": 1} },
        "natural": { "water": {"color": "0x001F", "weight": 1, "min_lod": 0} }
    },
    "marker": {"color": "0xF800"},
    "chunk_size": 4096,
    "routing": {"min_component_edges": 4}
}"#;

/// The band table for that ladder: one coarse band, one geometry band, one core band. Two bands share
/// `2^18`, exactly as the recommended table's `fine` and `network` do.
const BANDS: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 20, "lods": [0],    "role": "coarse"},
    {"id": "fine",    "cell_log2": 18, "lods": [1, 2], "role": "geometry"},
    {"id": "network", "cell_log2": 18, "lods": [],     "sections": ["nav", "poi"], "role": "core"}
]}"#;

fn config() -> Config {
    Config::parse(CONFIG).expect("test config parses")
}

fn bands() -> BandTable {
    BandTable::parse(BANDS).expect("test band table parses")
}

fn style_id(cfg: &Config, key: &str, value: &str) -> u8 {
    cfg.get_style(&HashMap::from([(key, value)])).expect("styled feature type").id
}

fn deg(udeg: i64) -> f64 {
    udeg as f64 / 1e6
}

/// A line feature from `(lat, lon)` µdeg pairs.
fn line(style_id: u8, min_lod: usize, pts: &[(i64, i64)]) -> IngestFeature {
    IngestFeature { style_id, min_lod, geom: Geom::Line(pts.iter().map(|&(lat, lon)| (deg(lon), deg(lat))).collect()) }
}

/// A closed rectangle feature over `lat0..lat1 × lon0..lon1` µdeg.
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

fn way(id_base: i64, kind: u8, pts: &[(i64, i64)]) -> RoutableWay {
    RoutableWay {
        node_ids: (0..pts.len() as i64).map(|k| id_base + k).collect(),
        coords: pts.iter().map(|&(lat, lon)| (lon as i32, lat as i32)).collect(),
        kind,
    }
}

fn poi(subtype: u8, lat: i64, lon: i64, name: &str) -> Poi {
    Poi {
        subtype,
        lon_udeg: lon as i32,
        lat_udeg: lat as i32,
        name: Some(name.to_string()),
        from_node: true,
        hours: None,
    }
}

/// The extract every test cuts: geometry and roads crossing two `2^18` seams, plus one POI per
/// network cell and one interior islet.
fn fixture(cfg: &Config) -> (Ingested, Vec<RoutableWay>) {
    let road = style_id(cfg, "highway", "residential");
    let water = style_id(cfg, "natural", "water");
    let features = vec![
        // A road crossing both seams — the geometry whose clipped ends must meet.
        line(road, 1, &[(LAT, SEAM - 8_000), (LAT, SEAM + 8_000), (LAT, SEAM_E + 8_000)]),
        // A lake straddling the first seam, so a *polygon* clip is exercised too.
        rect(water, 0, LAT + 20_000, SEAM - 12_000, LAT + 40_000, SEAM + 12_000),
        // A feature wholly inside one cell: it must be written untouched, in that cell only.
        rect(water, 0, LAT - 40_000, SEAM + 30_000, LAT - 20_000, SEAM + 50_000),
    ];
    let ways = vec![
        // Crosses the first seam; short enough that the §8.3 `i16` bound does not split it.
        way(1_000, 7, &[(LAT, SEAM - 6_000), (LAT, SEAM + 6_000)]),
        // A T-junction inside cell j = 1053, sharing node 1_001 (the way above's second node) so the
        // whole-extract touch count makes it a junction.
        way(2_000, 7, &[(LAT, SEAM + 6_000), (LAT + 9_000, SEAM + 6_000)]),
        // Crosses the eastern seam.
        way(3_000, 7, &[(LAT, SEAM_E - 6_000), (LAT, SEAM_E + 6_000)]),
        // A tiny interior islet, far from any edge: prunable, and must be pruned.
        way(4_000, 7, &[(LAT + 60_000, SEAM + 60_000), (LAT + 60_500, SEAM + 60_500)]),
    ];
    // The T-junction way must literally share the OSM node id, not just the coordinate.
    let mut ways = ways;
    ways[1].node_ids[0] = 1_001;
    let pois = vec![
        poi(1, LAT + 1_000, SEAM - 1_000, "West"),
        poi(1, LAT + 1_000, SEAM + 1_000, "East"),
        poi(5, LAT + 2_000, SEAM_E + 1_000, "Far East"),
    ];
    (Ingested { features, coastlines: Vec::new(), pois, nav_graph: Default::default() }, ways)
}

/// A scratch directory that cleans up after itself.
struct Scratch(PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let base = std::env::temp_dir().join(format!("obc-cell-cut-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("scratch dir");
        Scratch(base)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn options() -> CutOptions {
    CutOptions { bands: bands(), ..Default::default() }
}

fn cut_fixture(out: &Path, opts: &CutOptions) -> CutSummary {
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    cut_ingested(&ing, &ways, &cfg, out, opts, &Progress::silent()).expect("cut succeeds")
}

/// Every file under `dir`, as `relative path → bytes`.
fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(dir).expect("read_dir") {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                walk(&p, base, out);
            } else {
                let rel = p.strip_prefix(base).expect("under base").to_string_lossy().into_owned();
                out.insert(rel, std::fs::read(&p).expect("read file"));
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

// --- determinism (§3.2) ------------------------------------------------------------------------

/// Same inputs ⇒ byte-identical cells, manifest included. This is what lets the catalog
/// content-address a cell and a re-bake be a no-op — and the per-cell work runs on a rayon pool, so
/// it is also the guard against a thread-order-dependent byte anywhere in the cut.
#[test]
fn two_runs_are_byte_identical() {
    let a = Scratch::new("det-a");
    let b = Scratch::new("det-b");
    let sa = cut_fixture(a.path(), &options());
    let sb = cut_fixture(b.path(), &options());
    assert!(!sa.cells.is_empty(), "the fixture produced cells");
    assert_eq!(sa.bytes, sb.bytes);
    let (ta, tb) = (tree(a.path()), tree(b.path()));
    assert_eq!(ta.keys().collect::<Vec<_>>(), tb.keys().collect::<Vec<_>>(), "same paths");
    for (path, bytes) in &ta {
        assert_eq!(bytes, &tb[path], "{path} differs between two runs of the same cut");
    }
    // The summary's own ordering is content-derived too.
    let ids = |s: &CutSummary| s.cells.iter().map(|c| (c.band.clone(), c.id.to_string())).collect::<Vec<_>>();
    assert_eq!(ids(&sa), ids(&sb));
    assert!(ta.contains_key("cells.json"), "the provenance sidecar is written");
}

// --- adjacency (§3.3 / §3.4) -------------------------------------------------------------------

fn open(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn cell_file(out: &Path, summary: &CutSummary, band: &str, i: i64, j: i64) -> Vec<u8> {
    let a = summary.cells.iter().find(|c| c.band == band && c.id.i == i && c.id.j == j).unwrap_or_else(|| {
        panic!(
            "no {band} cell {i}/{j} in {:?}",
            summary.cells.iter().map(|c| format!("{} {}", c.band, c.id)).collect::<Vec<_>>()
        )
    });
    open(&out.join(&a.path))
}

/// Every vertex of every decoded feature of every LOD, as µdeg `(lon, lat)`.
fn all_vertices(bytes: &[u8]) -> Vec<(i32, i32)> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a cell parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for lod in 0..r.lods().len() {
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node))).expect("walk");
        for (cid, node) in chunks {
            r.for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| {
                out.extend(f.exterior().iter().copied());
                for h in f.interiors() {
                    out.extend(h.iter().copied());
                }
            })
            .expect("decode");
        }
    }
    out
}

/// Every nav junction coordinate in a cell, as µdeg `(lon, lat)`.
fn nav_coords(bytes: &[u8]) -> BTreeSet<(i32, i32)> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a cell parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = BTreeSet::new();
    let mut scratch = [0u8; 512];
    r.for_each_nav_node(&r.bbox, &mut scratch, |n| {
        out.insert((n.lon, n.lat));
    })
    .expect("nav walk");
    out
}

/// **The seam test.** Two adjacent cells cut from one extract must agree, to the microdegree, on
/// every junction and every clipped geometry endpoint that lands on their shared edge — that
/// agreement is what an assembler unifies, and it is why unification needs no tolerance (§3.4).
#[test]
fn adjacent_cells_meet_exactly_at_the_seam() {
    let out = Scratch::new("adjacency");
    let summary = cut_fixture(out.path(), &options());

    // --- nav: the boundary junctions of the two `network` neighbours coincide exactly.
    let west = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1052));
    let east = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1053));
    let on_seam = |set: &BTreeSet<(i32, i32)>| -> Vec<(i32, i32)> {
        set.iter().copied().filter(|(lon, _)| *lon as i64 == SEAM).collect()
    };
    let (ws, es) = (on_seam(&west), on_seam(&east));
    assert!(!ws.is_empty(), "the western cell materialised a boundary junction on the shared edge");
    assert_eq!(ws, es, "both neighbours put the SAME junction coordinate on the shared edge");
    assert_eq!(ws, vec![(SEAM as i32, LAT as i32)], "and it is the crossing coordinate, exactly");
    // Each side keeps its own stub inward and nothing of the neighbour's side.
    assert!(west.iter().all(|(lon, _)| (*lon as i64) <= SEAM), "no western node east of the seam");
    assert!(east.iter().all(|(lon, _)| (*lon as i64) >= SEAM), "no eastern node west of the seam");

    // --- geometry: the clipped pieces meet on the line, with no gap and no overlap.
    let wv = all_vertices(&cell_file(out.path(), &summary, "fine", 1204, 1052));
    let ev = all_vertices(&cell_file(out.path(), &summary, "fine", 1204, 1053));
    assert!(!wv.is_empty() && !ev.is_empty(), "both fine cells carry geometry");
    let seam_lats = |v: &[(i32, i32)]| -> BTreeSet<i32> {
        v.iter().filter(|(lon, _)| *lon as i64 == SEAM).map(|(_, lat)| *lat).collect()
    };
    let (wl, el) = (seam_lats(&wv), seam_lats(&ev));
    assert!(!wl.is_empty(), "the clip put vertices exactly on the edge line");
    assert_eq!(wl, el, "both neighbours' clipped edges land on the identical seam coordinates");
    // The road's clipped ends: LAT on the seam, from both sides.
    assert!(wl.contains(&(LAT as i32)), "the road's clip vertex is on the line: {wl:?}");
    // Neither cell reaches across the line.
    let (w_box, e_box) = (CellId::new(18, 1204, 1052).unwrap().square(), CellId::new(18, 1204, 1053).unwrap().square());
    assert!(wv.iter().all(|(lon, _)| (*lon as i64) <= w_box.2), "western geometry stays in its square");
    assert!(ev.iter().all(|(lon, _)| (*lon as i64) >= e_box.0), "eastern geometry stays in its square");
}

/// The second seam is a `2^18` **and** a `2^19` line, so the same road is cut at it in one band and
/// runs straight through the middle of a cell in another. Both must be true at once.
#[test]
fn a_seam_of_one_band_is_interior_to_another() {
    let out = Scratch::new("bands");
    let summary = cut_fixture(out.path(), &options());
    // `2^18`: cut at SEAM_E, so the two neighbours share a junction there.
    let a = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1053));
    let b = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1054));
    let seam_e = |s: &BTreeSet<(i32, i32)>| s.iter().any(|(lon, _)| *lon as i64 == SEAM_E);
    assert!(seam_e(&a) && seam_e(&b), "both `2^18` neighbours carry the eastern boundary junction");
    // `2^20` (the coarse band): one cell holds the whole fixture, and SEAM is interior to it — no
    // clip vertex sits on it beyond the ones the source geometry itself has.
    let coarse: Vec<_> = summary.cells.iter().filter(|c| c.band == "coarse").collect();
    assert_eq!(coarse.len(), 1, "the fixture fits one `2^20` cell");
    let cv = all_vertices(&open(&out.path().join(&coarse[0].path)));
    assert!(!cv.is_empty(), "the coarse cell carries its LOD 0 geometry");
    let square = coarse[0].id.square();
    assert!(
        cv.iter().all(|(lon, lat)| (*lon as i64) >= square.0
            && (*lon as i64) <= square.2
            && (*lat as i64) >= square.1
            && (*lat as i64) <= square.3),
        "coarse geometry stays inside the coarse square"
    );
}

/// The same seam property under the **shipped preset** — 9 LODs, real simplify tolerances from
/// 2200 m down to 0.5 m, `merge_fills` and `merge_lines` both on, the recommended band table.
///
/// This is the test that would fail if the cutter ever simplified *after* clipping, or merged
/// per-cell: either would let two neighbours move their own copy of a seam vertex independently, and
/// the equality below would become a near-miss — the crack OBCA §3.3 exists to rule out.
#[test]
fn the_shipped_preset_still_meets_at_the_seam() {
    const PRESET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/presets/schema.json");
    let cfg = Config::load(PRESET).expect("the shipped preset loads");
    assert!(cfg.merge_fills && cfg.merge_lines, "the preset really does run the merge passes");
    let (ing, ways) = fixture(&cfg);
    let out = Scratch::new("preset");
    let opts = CutOptions { bands: BandTable::recommended(), ..Default::default() };
    let summary = cut_ingested(&ing, &ways, &cfg, out.path(), &opts, &Progress::silent()).expect("cut");

    // Nav (the `network` band): identical boundary junction on the shared edge.
    let west = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1052));
    let east = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1053));
    let seam = |s: &BTreeSet<(i32, i32)>| -> Vec<(i32, i32)> {
        s.iter().copied().filter(|(lon, _)| *lon as i64 == SEAM).collect()
    };
    assert_eq!(seam(&west), seam(&east), "the preset's cells agree on the seam junction");
    assert!(!seam(&west).is_empty());

    // Geometry, per band and per LOD: whatever each neighbour puts on the shared line, the other puts
    // there too. Checked per LOD because the tolerances differ by level, and on each band's own seam —
    // `SEAM` is a `2^18` line that runs through the middle of a `2^19` cell, while `SEAM_E` is both.
    for (band, i, j, seam) in [("mid", 602, 526, SEAM_E), ("fine", 1204, 1052, SEAM), ("fine", 1204, 1053, SEAM_E)] {
        let a = cell_file(out.path(), &summary, band, i, j);
        let b = cell_file(out.path(), &summary, band, i, j + 1);
        let (sa, sb) = (seam_coords_per_lod(&a, seam), seam_coords_per_lod(&b, seam));
        assert!(sa.values().any(|v| !v.is_empty()), "{band} {i}/{j}: something clips on the seam: {sa:?}");
        assert_eq!(sa, sb, "{band} {i}/{j}: the two neighbours' seam vertices differ");
    }
}

/// Per LOD, the latitudes at which a cell's decoded geometry touches the line `lon == seam`.
fn seam_coords_per_lod(bytes: &[u8], seam: i64) -> BTreeMap<usize, BTreeSet<i32>> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a cell parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out: BTreeMap<usize, BTreeSet<i32>> = BTreeMap::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for lod in 0..r.lods().len() {
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node))).expect("walk");
        let slot = out.entry(lod).or_default();
        for (cid, node) in chunks {
            r.for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| {
                let mut take = |pts: &[(i32, i32)]| {
                    for (lon, lat) in pts {
                        if *lon as i64 == seam {
                            slot.insert(*lat);
                        }
                    }
                };
                take(f.exterior());
                for h in f.interiors() {
                    take(h);
                }
            })
            .expect("decode");
        }
    }
    out
}

// --- the artifact contract (§3.1) --------------------------------------------------------------

/// Every cell is a valid OBCM whose header bbox **is** its grid square, carrying the complete ladder
/// with every out-of-band level written genuinely empty (`Index Node Count == 0`) — so nothing in the
/// bytes says which band the cell belongs to.
#[test]
fn every_cell_round_trips_through_the_reader() {
    let out = Scratch::new("roundtrip");
    let summary = cut_fixture(out.path(), &options());
    let table = bands();
    let cfg = config();
    assert!(summary.cells.len() >= 7, "3 fine + 3 network + 1 coarse, got {}", summary.cells.len());
    assert_eq!(summary.dropped, 0, "nothing overflowed a chunk");

    for artifact in &summary.cells {
        let bytes = open(&out.path().join(&artifact.path));
        assert_eq!(bytes.len() as u64, artifact.bytes, "{}: manifest size matches the file", artifact.path);
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap_or_else(|e| panic!("{}: parse: {e:?}", artifact.path));
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);
        assert_eq!(r.version, obc_formats::obcm::VERSION);
        assert_eq!(r.marker_color, cfg.marker_color);

        // The header bbox is the cell square — the inverted rule of §3.1.
        let (min_lon, min_lat, max_lon, max_lat) = artifact.id.square();
        assert_eq!(
            r.bbox,
            BBox { min_lon: min_lon as i32, min_lat: min_lat as i32, max_lon: max_lon as i32, max_lat: max_lat as i32 },
            "{}: the header bbox must be exactly the grid square",
            artifact.path
        );

        // The complete ladder, with the band's own levels the only non-empty ones.
        let band = table.band(&artifact.band).expect("band in the table");
        assert_eq!(r.lods().len(), cfg.lods.len(), "{}: the whole ladder is written", artifact.path);
        for (i, lod) in r.lods().iter().enumerate() {
            let expected_mpp = cfg.lods[i].max_mpp.map_or(f32::INFINITY, |v| v as f32);
            assert_eq!(lod.max_mpp, expected_mpp, "{}: LOD {i} keeps the schema's max_mpp", artifact.path);
            if !band.lods.contains(&i) {
                assert_eq!(lod.node_count, 0, "{}: out-of-band LOD {i} has no index", artifact.path);
                assert_eq!(lod.chunk_count, 0, "{}: out-of-band LOD {i} has no chunk", artifact.path);
                assert_eq!(lod.chunk_bytes_total, 0, "{}: …and no chunk bytes", artifact.path);
                let mut seen = 0;
                r.for_each_chunk(i, &r.bbox, |_, _| seen += 1).expect("an empty LOD walks");
                assert_eq!(seen, 0, "{}: walking an empty LOD yields nothing", artifact.path);
            }
        }
        // Whatever is non-empty decodes cleanly, every feature of every chunk (§4.8's verify, in
        // miniature).
        let _ = all_vertices(&bytes);
    }
}

/// Sections live in exactly the band that carries them: nav + POIs in the `network` cells, and
/// nowhere else. A `fine` cell has no roads to route on and a `network` cell nothing to draw.
#[test]
fn sections_live_only_in_the_band_that_carries_them() {
    let out = Scratch::new("sections");
    let summary = cut_fixture(out.path(), &options());
    let mut network_pois = 0usize;
    for artifact in &summary.cells {
        let bytes = open(&out.path().join(&artifact.path));
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).expect("parse");
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);
        let nav = r.nav_directory();
        let poi = r.poi_directory();
        let poi_records: usize = poi.entries.iter().map(|e| e.chunk_count).sum();
        if artifact.band == "network" {
            assert!(!nav.is_empty(), "{}: a network cell carries the graph", artifact.path);
            assert!(artifact.nav_nodes > 0, "{}: …and reports it", artifact.path);
            network_pois += artifact.pois;
        } else {
            assert!(nav.is_empty(), "{}: a geometry cell carries no graph", artifact.path);
            assert_eq!(poi_records, 0, "{}: …and no POIs", artifact.path);
            assert_eq!((artifact.nav_nodes, artifact.nav_edges, artifact.pois), (0, 0, 0));
        }
        // The POI directory and the profile table are present either way (§3.1: the sections exist,
        // they are merely empty), which is what keeps every cell an openable map.
        assert_eq!(poi.entries.len(), 6, "{}: all six POI categories have a directory entry", artifact.path);
        assert!(nav.profile_count >= 1, "{}: the schema's profile table travels with every cell", artifact.path);
    }
    assert_eq!(network_pois, 3, "every POI landed in exactly one network cell");
    // Each POI is in the one cell whose half-open square contains it (§3.6).
    let west = summary.cells.iter().find(|c| c.band == "network" && c.id.j == 1052).expect("west cell");
    let east = summary.cells.iter().find(|c| c.band == "network" && c.id.j == 1053).expect("east cell");
    assert_eq!((west.pois, east.pois), (1, 1));
}

/// §3.5 through the whole cut: the interior islet is gone, the boundary-crossing stub is not.
#[test]
fn island_pruning_is_strictly_interior() {
    let out = Scratch::new("prune");
    let summary = cut_fixture(out.path(), &options());
    let east = nav_coords(&cell_file(out.path(), &summary, "network", 1204, 1053));
    assert!(east.iter().any(|(lon, _)| *lon as i64 == SEAM), "the seam stub survived (it touches the edge)");
    assert!(
        !east.iter().any(|(lon, lat)| *lon as i64 == SEAM + 60_000 && *lat as i64 == LAT + 60_000),
        "the strictly interior islet was pruned: {east:?}"
    );
}

// --- provenance (§3.7) ------------------------------------------------------------------------

/// A cell is canonical only when the declared sources demonstrably cover its whole square; anything
/// less is `partial`, including "no coverage declared at all".
#[test]
fn partial_marking_follows_declared_coverage() {
    let out = Scratch::new("partial-none");
    let summary = cut_fixture(out.path(), &options());
    assert!(summary.cells.iter().all(|c| c.partial), "without declared coverage nothing is canonical");
    assert_eq!(summary.partial, summary.cells.len());

    // A source that covers the fine cells but not the coarse one: the `2^20` cell is 4× wider, so it
    // pokes out of the declared box.
    let covering: UBox = (7_000_000, 47_000_000, 8_000_000, 47_500_000);
    let opts = CutOptions {
        sources: vec![SourceExtent {
            id: "test/extract".into(),
            snapshot: Some("2026-07-30".into()),
            coverage: Some(covering),
        }],
        ..options()
    };
    let out2 = Scratch::new("partial-some");
    let summary = cut_fixture(out2.path(), &opts);
    for c in &summary.cells {
        let (min_lon, min_lat, max_lon, max_lat) = c.id.square();
        let covered = covering.0 <= min_lon && covering.1 <= min_lat && covering.2 >= max_lon && covering.3 >= max_lat;
        assert_eq!(!c.partial, covered, "{}: partial must follow real coverage", c.path);
    }
    assert!(summary.cells.iter().any(|c| !c.partial), "the covered cells are canonical");
    assert!(summary.cells.iter().any(|c| c.partial), "the coarse cell is not");
    // The sidecar states the provenance the artifacts cannot.
    let manifest = std::fs::read_to_string(out2.path().join("cells.json")).expect("manifest");
    assert!(manifest.contains("\"test/extract\""), "the source id is recorded");
    assert!(manifest.contains("\"2026-07-30\""), "…and its snapshot date");
    assert!(manifest.contains("\"partial\": true") && manifest.contains("\"partial\": false"));
    assert!(manifest.contains("\"band\": \"network\""), "band membership lives here, not in the bytes");
}

/// A band or cell selection restricts the run without changing what a cell contains.
#[test]
fn explicit_selection_cuts_exactly_those_cells() {
    let full = Scratch::new("select-full");
    let all = cut_fixture(full.path(), &options());
    let picked = Scratch::new("select-one");
    let opts =
        CutOptions { select: vec![CellId::new(18, 1204, 1053).unwrap()], only_bands: vec!["fine".into()], ..options() };
    let one = cut_fixture(picked.path(), &opts);
    assert_eq!(one.cells.len(), 1, "one band × one cell");
    assert_eq!(one.cells[0].band, "fine");
    assert_eq!(one.cells[0].id.to_string(), "18/1204/1053");
    // Selecting a cell must not change its bytes — the cell is a function of the source, not of the
    // run that asked for it.
    let same = all.cells.iter().find(|c| c.band == "fine" && c.id.j == 1053).expect("in the full run");
    assert_eq!(one.cells[0].sha256, same.sha256, "the same cell, however it was selected");
    assert_eq!(open(&picked.path().join(&one.cells[0].path)), open(&full.path().join(&same.path)));

    // A cell size no band uses is a hard error rather than a silent no-op.
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    let bad = CutOptions { select: vec![CellId::new(16, 1, 1).unwrap()], ..options() };
    assert!(cut_ingested(&ing, &ways, &cfg, picked.path(), &bad, &Progress::silent()).is_err());
    let bad_band = CutOptions { only_bands: vec!["nope".into()], ..options() };
    assert!(cut_ingested(&ing, &ways, &cfg, picked.path(), &bad_band, &Progress::silent()).is_err());
}

/// A band table that does not partition the ladder is refused before anything is written (OBCA §1.2).
#[test]
fn a_broken_band_table_is_refused() {
    let out = Scratch::new("bad-bands");
    let cfg = config();
    let (ing, ways) = fixture(&cfg);
    // The recommended table is a 9-LOD table; this config's ladder has three levels.
    let opts = CutOptions { bands: BandTable::recommended(), ..Default::default() };
    let err = cut_ingested(&ing, &ways, &cfg, out.path(), &opts, &Progress::silent()).expect_err("must refuse");
    assert!(err.contains("LOD"), "the error names the partition problem: {err}");
    assert!(!out.path().join("cells.json").exists(), "and nothing was published");
}

// --- the real ingest path ---------------------------------------------------------------------

/// The committed corpus fixture through the whole `cut()` path: ingest a `.pbf` **once**, emit the
/// cells it touches.
///
/// `tiny.osm` covers ~1 × 1.5 km of styled content, which fits well inside a single `2^18` cell, so
/// this run uses a **finer band table** (`2^12`, ≈ 460 × 310 m) to get a genuine multi-cell cut out of
/// it. That is not a workaround but the point: cell sizes are schema data ([`BandTable`]), not format
/// constants, and a cutter that only worked at the recommended sizes would hide an assumption.
///
/// Land generation is off: it fetches a dataset over the network, which a test must never do. A
/// Geofabrik-scale run stays a manual exercise (`obc-pack cells …`) rather than a CI download.
#[test]
fn a_real_pbf_cuts_into_cells() {
    const TINY: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../builder/tests/corpus/data/tiny.osm.pbf");
    const FINE_BANDS: &str = r#"{"bands": [
        {"id": "coarse",  "cell_log2": 14, "lods": [0],    "role": "coarse"},
        {"id": "fine",    "cell_log2": 12, "lods": [1, 2], "role": "geometry"},
        {"id": "network", "cell_log2": 12, "lods": [],     "sections": ["nav", "poi"], "role": "core"}
    ]}"#;
    assert!(
        Path::new(TINY).exists(),
        "corpus fixture missing: {TINY} — it is committed; rebuild via builder/tests/corpus/build_corpus.sh"
    );
    let out = Scratch::new("pbf");
    let cfg = Config::parse(CONFIG).expect("config");
    let opts =
        CutOptions { bands: BandTable::parse(FINE_BANDS).expect("band table"), no_land: true, ..Default::default() };
    let summary = obc_pack::cut::cut(&[TINY.to_string()], &cfg, out.path(), &opts, &Progress::silent())
        .expect("cutting the corpus fixture");
    assert_eq!(summary.dropped, 0);
    let fine: Vec<_> = summary.cells.iter().filter(|c| c.band == "fine").collect();
    assert!(fine.len() >= 8, "the fixture spans several 2^12 cells, got {}", fine.len());
    assert!(fine.iter().any(|c| c.bytes > 0), "and they were written");
    // Ids are padded to the width the smaller size needs (OBCA §1.3), not to four digits.
    assert!(fine[0].id.to_string().starts_with("12/"), "{}", fine[0].id);
    assert_eq!(fine[0].id.to_string().split('/').nth(1).unwrap().len(), 6, "2^12 needs six digits");
    // The nav graph really did come out of the pbf and land in the core band's cells.
    let network_nodes: usize = summary.cells.iter().filter(|c| c.band == "network").map(|c| c.nav_nodes).sum();
    assert!(network_nodes > 0, "the corpus fixture's highways produced junctions");
    // Every artifact opens, and its bbox is its square.
    for a in &summary.cells {
        let bytes = open(&out.path().join(&a.path));
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap_or_else(|e| panic!("{}: parse {e:?}", a.path));
        let cache = MapCache::new();
        let r = Reader::new(&src, &tables, &cache);
        let (min_lon, min_lat, ..) = a.id.square();
        assert_eq!((r.bbox.min_lon as i64, r.bbox.min_lat as i64), (min_lon, min_lat), "{}", a.path);
    }
}

// === Terrain-fed cuts: the v12 §8.3 ascent at a seam (epic #1068 EL5) ==========================

/// The synthetic terrain's posting and cell size — both legal OBCT v1 header values, both small so
/// the container covering the fixture is ~100 KB rather than ~100 MB (`OBCT_Spec.md` §1.3: posting
/// and cell size are data, and the sampler cannot tell a small one from a production one).
const T_POSTING_LOG2: u8 = 9;
const T_CELL_LOG2: u8 = 14;

/// The OBCA cell index of a µdeg coordinate at [`T_CELL_LOG2`].
fn t_cell(udeg: i64) -> u32 {
    ((udeg + (1 << 28)) >> T_CELL_LOG2) as u32
}

/// Write a `.obcd` container covering the fixture's whole neighbourhood — both seams, both cell
/// rows — whose height rises **with longitude** at ~4 m per lattice column. Eastbound roads
/// therefore climb monotonically, which is what makes the seam arithmetic below checkable by hand.
///
/// Latitude is deliberately absent from the surface: a transposed lat/lon would book a flat road and
/// be caught, rather than sampling a plausible-looking plane and passing.
fn write_seam_terrain(dir: &Path) -> PathBuf {
    let (lat0, lat1) = (LAT - 80_000, LAT + 80_000);
    let (lon0, lon1) = (SEAM - 40_000, SEAM_E + 40_000);
    let (min_i, min_j) = (t_cell(lat0), t_cell(lon0));
    let rows = (t_cell(lat1) - min_i + 1) as u16;
    let cols = (t_cell(lon1) - min_j + 1) as u16;
    let bytes = obc_vectors::terrain_container(
        T_POSTING_LOG2,
        T_CELL_LOG2,
        min_i,
        min_j,
        rows,
        cols,
        &|_, _| true,
        &|_di, dj| (200 + 4 * dj as i32) as i16,
    );
    let path = dir.join("seam.obcd");
    std::fs::write(&path, bytes).expect("write terrain");
    path
}

/// One adjacency arc: `(from coord, to coord, ascent_m)`, coords µdeg `(lon, lat)`.
type Arc = ((i32, i32), (i32, i32), u16);

/// Every adjacency arc of a cell.
fn nav_arcs(bytes: &[u8]) -> BTreeSet<Arc> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a cell parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = BTreeSet::new();
    let mut scratch = [0u8; 512];
    r.for_each_nav_node(&r.bbox, &mut scratch, |n| {
        for nb in n.neighbors() {
            out.insert(((n.lon, n.lat), (nb.lon, nb.lat), nb.ascent_m));
        }
    })
    .expect("nav walk");
    out
}

/// The one arc leaving `from` toward `to`, or `None`.
fn arc_ascent(arcs: &BTreeSet<Arc>, from: (i32, i32), to: (i32, i32)) -> Option<u16> {
    arcs.iter().find(|(f, t, _)| *f == from && *t == to).map(|(_, _, a)| *a)
}

/// **The seam-determinism pin for v12.** The cutter slices the road crossing the `2^18` line exactly
/// on it, and each neighbour bakes the ascent of *its own* stub — but both integrate the same global
/// OBCT lattice, so:
///
/// 1. each side's booked climb equals what integrating that stub through the shared sampler gives,
///    which is what would break the moment anything sampled a cell-local raster or a per-cell origin;
/// 2. the two stubs' eastbound climbs **sum to the uncut way's**, so cutting an edge at a border
///    does not create or destroy metres of climbing (exactly, on this monotone ramp — the dead-band
///    re-anchors at the cut, which costs at most its own threshold);
/// 3. and cutting twice produces byte-identical cells, terrain and all.
#[test]
fn a_terrain_fed_cut_agrees_across_the_seam() {
    let out = Scratch::new("terrain-seam");
    let terrain_path = write_seam_terrain(out.path());
    let opts = CutOptions { terrain: Some(terrain_path.clone()), ..options() };
    let summary = cut_fixture(out.path(), &opts);

    let west = nav_arcs(&cell_file(out.path(), &summary, "network", 1204, 1052));
    let east = nav_arcs(&cell_file(out.path(), &summary, "network", 1204, 1053));

    // The road of `way(1_000)`: (SEAM − 6 000) → SEAM → (SEAM + 6 000), all at LAT.
    let w_end = ((SEAM - 6_000) as i32, LAT as i32);
    let boundary = (SEAM as i32, LAT as i32);
    let e_end = ((SEAM + 6_000) as i32, LAT as i32);

    let up_west = arc_ascent(&west, w_end, boundary).expect("the western stub is in the western cell");
    let up_east = arc_ascent(&east, boundary, e_end).expect("the eastern stub is in the eastern cell");
    let down_west = arc_ascent(&west, boundary, w_end).expect("…and its reverse");
    assert!(up_west > 20 && up_east > 20, "both stubs climb: {up_west} m / {up_east} m");
    assert_eq!(down_west, 0, "riding the ramp westward books no climb");

    // (1) Each side's number is the shared sampler's, over the global lattice.
    let set = TerrainSet::open(&terrain_path).expect("the container opens");
    let mut sampler = set.sampler_for(None).expect("sampler");
    let (expect_west, _) = integrate_edge_ascent(&[w_end, boundary], &mut sampler);
    let (expect_east, _) = integrate_edge_ascent(&[boundary, e_end], &mut sampler);
    assert_eq!(up_west, expect_west, "the western cell sampled the global lattice, not something local");
    assert_eq!(up_east, expect_east, "and so did the eastern one");

    // (2) The cut neither created nor destroyed climbing.
    let (whole, _) = integrate_edge_ascent(&[w_end, e_end], &mut sampler);
    let summed = i32::from(up_west) + i32::from(up_east);
    assert!(
        (summed - i32::from(whole)).abs() <= 3,
        "the two stubs sum to the uncut way's climb: {summed} m vs {whole} m"
    );

    // (3) Same inputs, same bytes — terrain does not make a cut order- or cache-dependent.
    let again = Scratch::new("terrain-seam-again");
    let again_terrain = write_seam_terrain(again.path());
    let again_opts = CutOptions { terrain: Some(again_terrain), ..options() };
    let again_summary = cut_fixture(again.path(), &again_opts);
    for (band, i, j) in [("network", 1204, 1052), ("network", 1204, 1053)] {
        assert_eq!(
            cell_file(out.path(), &summary, band, i, j),
            cell_file(again.path(), &again_summary, band, i, j),
            "{band}/{i}/{j} must be byte-identical across runs"
        );
    }
}

/// A cut with **no** `--terrain` writes `Ascent M = 0` everywhere: the degrade path, and what every
/// other cut test in this file (and every bake until the terrain track is wired in) produces.
#[test]
fn a_cut_without_terrain_books_no_ascent() {
    let out = Scratch::new("no-terrain");
    let summary = cut_fixture(out.path(), &options());
    let arcs = nav_arcs(&cell_file(out.path(), &summary, "network", 1204, 1052));
    assert!(!arcs.is_empty(), "the western cell has a graph to be flat about");
    assert!(arcs.iter().all(|(_, _, ascent)| *ascent == 0), "no terrain in ⇒ no climb out");
}
