//! Integration: the coverage pass through the real quadtree + serializer + `obc-reader`
//! read-back, and through the real `pack` pipeline.
//!
//! The unit suite in `src/coverage.rs` pins the transform's semantics on bare geometry. What is
//! left to close here is the thing the transform exists for and the thing a bake-time knob must
//! promise:
//!
//! - [`a_shared_seam_reaches_the_device_glued`] — two abutting fills of *different* classes come
//!   out of a real pack with the identical seam vertices, and the control
//!   ([`the_same_seam_reaches_the_device_torn`]) shows the per-feature path putting different
//!   ones there. Everything in between — µdeg rounding, the quadtree, the chunk encoder, the
//!   reader — is the real code.
//! - the two byte-identity guarantees: a tier with no participating fills packs exactly as it
//!   did before the pass existed, and the flag's mere presence in a config changes nothing.

use obc_elevation::NullElevation;
use obc_pack::config::Config;
use obc_pack::coverage::coverage_simplify_fills;
use obc_pack::geom::{topology_preserve_simplify, Geom};
use obc_pack::merge::merge_classes;
use obc_pack::pipeline::{pack, PackOptions};
use obc_pack::progress::Progress;
use obc_pack::quadtree::build_lod;
use obc_pack::{serialize_lods, LodLayer, Style};
use obc_reader::{MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

use std::path::{Path, PathBuf};

const MARKER: u16 = 0xF800;
const CHUNK: usize = 4096;
/// A bbox around the fixture, wide enough that the whole block sits in the root leaf.
const GLOBAL: (i64, i64, i64, i64) = (7_700_000, 47_900_000, 7_900_000, 48_100_000);
/// Simplify tolerance in degrees — ~89 m, a coarse tier's setting.
const TOL: f64 = 0.2 * SCALE;
/// The fixture's unit, in degrees: the seam geometry below is written in units and scaled.
const SCALE: f64 = 0.004;

fn fill(id: u8, color: u16) -> Style {
    Style {
        id,
        z_index: 0,
        color,
        weight: 1,
        priority: 3,
        dashed: false,
        color2: None,
        fixed_width: false,
        terrain_layer: false,
    }
}

/// Fixture units → degrees, near Freiburg.
fn at(x: f64, y: f64) -> (f64, f64) {
    (7.8 + x * SCALE, 47.99 + y * SCALE)
}

/// The wiggly boundary the two fills share, in fixture units. Both carry these identical
/// vertices, as two abutting OSM ways reference the same boundary nodes.
const SEAM: [(f64, f64); 6] = [(1.12, 0.2), (0.95, 0.35), (1.18, 0.5), (0.9, 0.62), (1.14, 0.8), (1.0, 1.0)];

fn poly(ring: &[(f64, f64)]) -> Geom {
    let mut exterior: Vec<(f64, f64)> = ring.iter().map(|&(x, y)| at(x, y)).collect();
    if exterior.first() != exterior.last() {
        exterior.push(exterior[0]);
    }
    Geom::Polygon { exterior, interiors: vec![] }
}

/// A tall slab west of the seam — a big landuse block.
fn west() -> Geom {
    let mut ring = vec![(0.0, 0.0), (1.0, 0.0)];
    ring.extend(SEAM);
    ring.extend([(1.0, 4.0), (0.0, 4.0)]);
    poly(&ring)
}

/// A small parcel east of it, sharing the seam.
fn east() -> Geom {
    let mut ring = vec![(1.0, 0.0), (2.0, 0.0), (2.0, 1.0), (1.0, 1.0)];
    ring.extend(SEAM.iter().rev().skip(1).copied());
    poly(&ring)
}

/// Pack one LOD exactly as the pipeline does.
fn serialize(features: Vec<(u8, Geom)>, styles: &[Style]) -> Vec<u8> {
    let root = build_lod(features, GLOBAL, CHUNK);
    let lod = LodLayer { max_mpp: None, chunk_size: CHUNK, root };
    let (bytes, dropped) = serialize_lods(
        &[lod],
        styles,
        MARKER,
        GLOBAL,
        &[],
        &Default::default(),
        &obc_pack::config::default_profiles(),
        &mut NullElevation,
    );
    assert_eq!(dropped, 0, "the fixture must fit its chunks");
    bytes
}

/// Every decoded feature's `(style_id, vertices in µdeg)`, through the real reader path.
fn decode(bytes: &[u8]) -> Vec<(u8, Vec<(i32, i32)>)> {
    let cache = MapCache::new();
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("parse tables");
    let r = Reader::new(&src, &tables, &cache);
    let mut chunks = Vec::new();
    r.for_each_chunk(0, &r.bbox, |cid, node| chunks.push((cid, node))).unwrap();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let mut out = Vec::new();
    for (cid, node) in chunks {
        r.for_each_feature(0, cid, &node, &mut points, &mut ring_lens, |f| {
            let mut verts: Vec<(i32, i32)> = f.exterior().to_vec();
            for h in f.interiors() {
                verts.extend(h.iter().copied());
            }
            verts.sort_unstable();
            verts.dedup();
            out.push((f.style_id, verts));
        })
        .unwrap();
    }
    out.sort_by_key(|(sid, _)| *sid);
    out
}

/// A decoded feature's vertices inside the seam band — its copy of the shared boundary.
fn seam_band(verts: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let lon = |u: f64| (at(u, 0.0).0 * 1e6) as i32;
    let lat = |u: f64| (at(0.0, u).1 * 1e6) as i32;
    verts.iter().copied().filter(|(x, y)| *x > lon(0.5) && *x < lon(1.5) && *y < lat(1.01)).collect()
}

/// **The whole point, end to end.** Two abutting fills of different classes, packed through the
/// coverage pass, reach the reader with the *identical* seam — no sliver of backdrop between
/// them at any tolerance.
#[test]
fn a_shared_seam_reaches_the_device_glued() {
    let styles = [fill(1, 0x0001), fill(2, 0x0002)];
    let classes = merge_classes(&styles);
    let (covered, stats) = coverage_simplify_fills(vec![(1, west()), (2, east())], &classes, TOL, None);
    assert_eq!(stats.fallbacks, 0, "no GEOS failure: {stats:?}");
    let bytes = serialize(covered.into_iter().map(|(s, g, _)| (s, g)).collect(), &styles);

    let decoded = decode(&bytes);
    assert_eq!(decoded.len(), 2, "both fills round-trip: {decoded:?}");
    let a = seam_band(&decoded[0].1);
    let b = seam_band(&decoded[1].1);
    assert!(!a.is_empty(), "the seam did not vanish");
    assert_eq!(a, b, "the two sides must carry the SAME seam vertices out of the packer");
}

/// The control: the same fixture down the per-feature path really does tear, so the test above
/// is measuring something. Douglas–Peucker resolves the shared chain differently inside the
/// long ring and the short one, and the two copies of the boundary end up apart.
#[test]
fn the_same_seam_reaches_the_device_torn() {
    let styles = [fill(1, 0x0001), fill(2, 0x0002)];
    let feats = vec![(1u8, topology_preserve_simplify(&west(), TOL)), (2u8, topology_preserve_simplify(&east(), TOL))];
    let decoded = decode(&serialize(feats, &styles));
    assert_eq!(decoded.len(), 2);
    assert_ne!(
        seam_band(&decoded[0].1),
        seam_band(&decoded[1].1),
        "if this ever matches, the fixture stopped exercising the tear the coverage pass fixes"
    );
}

/// A tier whose fills have no participating candidates serializes byte-identically to one
/// packed without the pass at all — `merge_fills`' "flag on, nothing to do ⇒ empty diff"
/// guarantee, for the coverage flag.
#[test]
fn a_tier_with_no_candidates_is_byte_identical() {
    // One line and one outlined polygon: neither participates (kind, then `color2`).
    let styles = [fill(1, 0x0001), Style { color2: Some(0x1234), ..fill(2, 0x0002) }];
    let classes = merge_classes(&styles);
    let feats = vec![
        (1u8, Geom::Line(vec![at(0.0, 0.0), at(1.0, 0.5), at(2.0, 0.0)])),
        (2u8, poly(&[(0.0, 2.0), (1.0, 2.0), (1.0, 3.0), (0.0, 3.0)])),
    ];
    let off = serialize(feats.clone(), &styles);
    let (covered, stats) = coverage_simplify_fills(feats, &classes, TOL, None);
    assert_eq!(stats.inputs, 0, "nothing participated: {stats:?}");
    assert!(covered.iter().all(|(_, _, simplified)| !*simplified), "everything still needs the per-feature path");
    let on = serialize(covered.into_iter().map(|(s, g, _)| (s, g)).collect(), &styles);
    assert_eq!(on, off, "a no-candidate coverage pass serializes byte-identically");
}

// --- the pipeline, with a real `.osm.pbf` ------------------------------------------------

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn out_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-pack-coverage-{}-{}", name, std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

/// Pack the corpus fixture with `config` and hand back the bytes. Land is skipped: it is a
/// ~950 MB network download, and nothing here is about land.
fn pack_fixture(config: &str, name: &str) -> Vec<u8> {
    let cfg = Config::parse(config).expect("config parses");
    let out = out_dir(name).join(format!("{name}.obcm"));
    let pbf = repo("builder/tests/corpus/data/tiny.osm.pbf").to_string_lossy().into_owned();
    pack(&[pbf], &cfg, &out, &PackOptions { no_land: true, ..PackOptions::default() }, &Progress::silent())
        .expect("pack");
    std::fs::read(&out).expect("read output")
}

/// The default is off, and *writing* the default is the same as not writing it: a config that
/// spells out `coverage_simplify: false` packs the identical bytes to one that never heard of
/// the field. This is the guarantee that adding the knob cannot move a single baked map.
#[test]
fn spelling_out_the_default_changes_no_bytes() {
    const ABSENT: &str = r#"{
        "lods": [{"max_mpp": null, "simplify": 200, "min_area_px": 50}, {"max_mpp": 30, "simplify": 10}],
        "merge_fills": true, "merge_lines": true,
        "features": {"highway": {"residential": {"color": "0xF800", "weight": 2}},
                     "natural": {"water": {"color": "0x001F"}},
                     "landuse": {"forest": {"color": "0x07E0"}}}
    }"#;
    let present = ABSENT.replace("\"simplify\": 200", "\"simplify\": 200, \"coverage_simplify\": false");
    assert_ne!(present, ABSENT, "the test string really did gain the field");
    assert_eq!(pack_fixture(ABSENT, "absent"), pack_fixture(&present, "false"), "the default is inert");
}

/// Determinism through the whole pipeline with the pass on: the components run in parallel, and
/// two runs must still write the same file.
#[test]
fn a_coverage_pack_is_deterministic() {
    const ON: &str = r#"{
        "lods": [{"max_mpp": null, "simplify": 200, "min_area_px": 50, "coverage_simplify": true},
                 {"max_mpp": 30, "simplify": 10, "coverage_simplify": true}],
        "merge_lines": true,
        "features": {"highway": {"residential": {"color": "0xF800", "weight": 2}},
                     "natural": {"water": {"color": "0x001F"}},
                     "landuse": {"forest": {"color": "0x07E0"}}}
    }"#;
    assert_eq!(pack_fixture(ON, "det-a"), pack_fixture(ON, "det-b"), "two coverage packs are byte-identical");
}

// --- the cell cutter --------------------------------------------------------------------

/// The cutter runs the pass **once over the whole extract**, before cutting, for the same
/// reason it merges once (`cut.rs`'s module docs): every cell then clips the identical glued
/// geometry, and a feature the pass already simplified must not be simplified again per cell.
/// This drives that branch end to end: two cut runs are byte-identical, and the seam between
/// two classes inside a cell is the same on both sides.
#[test]
fn a_coverage_cut_is_deterministic_and_stays_glued() {
    use obc_pack::cut::{cut_ingested, CutOptions};
    use obc_pack::grid::BandTable;
    use obc_pack::ingest::{IngestFeature, Ingested};

    // One tier, coverage on, at the fixture's own tolerance (metres = TOL degrees).
    let config = Config::parse(&format!(
        r#"{{
            "lods": [{{"max_mpp": null, "simplify": {}, "coverage_simplify": true}}],
            "chunk_size": 4096,
            "features": {{"landuse": {{"forest": {{"color": "0x0001"}}, "meadow": {{"color": "0x0002"}}}}}}
        }}"#,
        TOL * 111_320.0
    ))
    .expect("cut config parses");
    let (forest, meadow) = (1u8, 2u8);
    let ing = Ingested {
        features: vec![
            IngestFeature { style_id: forest, min_lod: 0, geom: west() },
            IngestFeature { style_id: meadow, min_lod: 0, geom: east() },
        ],
        coastlines: Vec::new(),
        pois: Vec::new(),
        nav_graph: Default::default(),
    };
    // A single-band table: the whole fixture sits inside one 2^18 cell (lon 7.602..7.864,
    // lat 47.972..48.234), so this is about the pass, not about seams between cells.
    let bands = BandTable::parse(
        r#"{"bands": [
            {"id": "fine",    "cell_log2": 18, "lods": [0], "role": "geometry"},
            {"id": "network", "cell_log2": 18, "lods": [],  "sections": ["nav", "poi"], "role": "core"}
        ]}"#,
    )
    .expect("band table parses");
    let opts = CutOptions { bands, no_land: true, ..CutOptions::default() };

    let run = |name: &str| -> Vec<(String, Vec<u8>)> {
        let dir = out_dir(name);
        let summary = cut_ingested(&ing, &[], &config, &dir, &opts, &Progress::silent()).expect("cut");
        assert_eq!(summary.dropped, 0, "nothing outgrew its chunk");
        summary.cells.iter().map(|c| (c.path.clone(), std::fs::read(dir.join(&c.path)).expect("read cell"))).collect()
    };
    let first = run("cut-a");
    assert_eq!(first, run("cut-b"), "two coverage cuts write the same cells, byte for byte");

    let (_, bytes) = first.iter().find(|(_, b)| decode(b).len() == 2).expect("one cell holds both fills");
    let decoded = decode(bytes);
    assert_eq!(
        seam_band(&decoded[0].1),
        seam_band(&decoded[1].1),
        "the two classes leave the cutter with the same seam"
    );
}
