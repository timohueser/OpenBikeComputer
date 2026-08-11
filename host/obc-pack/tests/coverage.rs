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
//!
//! The last section is the branch's **adversarial probes**, adopted from the review round — the
//! degenerate-input and thread-safety sweeps over the raw GEOS wrapper, the elimination stress
//! fixture, and the two probes that found real defects, kept here inverted so the fixes stay fixed.

use obc_elevation::NullElevation;
use obc_map_scene::M_PER_DEG;
use obc_pack::config::Config;
use obc_pack::coverage::{coverage_simplify_fills, Eliminate};
use obc_pack::geom::{
    coverage_is_valid, coverage_simplify_vw, footprint_below, strip_small_holes, topology_preserve_simplify, Geom,
};
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

// --- adversarial probes -----------------------------------------------------------------
//
// Adopted from the branch's review round, where they were written to break the pass' claimed
// properties. Three of them held; two found real defects and are kept here inverted, pinning the
// fixed behaviour. They work on bare geometry through the public API rather than through a pack,
// because what they are about is the operator, not the file.

/// A plain-fill style at a chosen `z_index` — the paint-order key the probes need to vary.
fn probe_fill(id: u8, z_index: i8, color: u16) -> Style {
    Style { z_index, ..fill(id, color) }
}

fn probe_poly(ring: &[(f64, f64)]) -> Geom {
    let mut exterior = ring.to_vec();
    if exterior.first() != exterior.last() {
        exterior.push(exterior[0]);
    }
    Geom::Polygon { exterior, interiors: vec![] }
}

/// An axis-aligned box in raw degrees (these fixtures are not on the Freiburg grid the seam
/// fixtures above use — they are about areas and thresholds, not about a place).
fn probe_rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Geom {
    probe_poly(&[(x0, y0), (x1, y0), (x1, y1), (x0, y1)])
}

/// `deg2` square degrees expressed as the `(mpp, min_area_px)` pair the tier culls with.
fn threshold_for(deg2: f64, mpp: f64) -> Eliminate {
    Eliminate { mpp, min_area_px: deg2 * (M_PER_DEG / mpp) * (M_PER_DEG / mpp) }
}

/// **An isolated sub-threshold face escapes neither operator.** On a coverage tier `min_area_px`
/// is an elimination threshold and the caller's `footprint_below` drop is suppressed for
/// everything the pass produced — so a face with no *covered* neighbour to be absorbed into used
/// to satisfy neither and survive at any size. That is the mechanism behind the measured
/// `--no-land` blow-up: a 0.35° x 0.20° Freiburg crop packed without the `natural.land` base fill
/// emitted **1069** fill polygons at LOD 0, against **21** once the cull below runs.
///
/// The pass now applies the cull to exactly those faces itself: an island is part of no tiling, so
/// dropping it opens no hole.
#[test]
fn an_isolated_small_face_is_culled_by_the_pass_itself() {
    let classes = merge_classes(&[probe_fill(1, 0, 0x0001), probe_fill(2, 5, 0x0002)]);
    let mpp = 100.0;
    let e = threshold_for(0.5, mpp);

    // A speck far away from everything: its own bbox component, no neighbour at all.
    let speck = probe_rect(50.0, 50.0, 50.01, 50.01); // 1e-4 deg², 5000x under the threshold
    let anchor = probe_rect(0.0, 0.0, 1.0, 1.0);
    assert!(footprint_below(&speck, e.mpp, e.min_area_px), "the fixture really is under the tier's threshold");

    let (out, stats) = coverage_simplify_fills(vec![(1, anchor), (2, speck)], &classes, 0.0, Some(e));
    assert_eq!(stats.fallbacks, 0, "{stats:?}");
    assert_eq!(stats.eliminated, 0, "there was nothing to absorb it into: {stats:?}");
    assert_eq!(stats.uneliminable_culled, 1, "so the pass culled it: {stats:?}");
    assert!(
        !out.iter().any(|(sid, g, _)| *sid == 2 && !g.is_empty()),
        "the speck must not reach the tier at all — it would cost a span, a ring and its points \
         and never be culled downstream: {out:?}"
    );
    assert!(out.iter().any(|(sid, _, _)| *sid == 1), "and the anchor is untouched: {out:?}");
}

/// **The hole trim may not paint out a kept face.** A face between the threshold and whatever
/// looser floor a hole trim used would survive elimination *deliberately* — and if it is a hole in
/// a higher-`z` neighbour's dissolved polygon, filling that hole makes the neighbour paint over it:
/// the kept face is in the file, costs bytes, and is invisible. The coverage tier's hole floor is
/// therefore exactly the elimination threshold and not a multiple of it.
#[test]
fn the_hole_trim_keeps_the_hole_of_a_kept_lower_z_face() {
    // z 2 inside, z 6 around it — the preset's own landuse.residential (z 2) inside a z-6 class.
    let classes = merge_classes(&[probe_fill(1, 2, 0x0001), probe_fill(2, 6, 0x0002)]);
    let mpp = 100.0;
    let e = threshold_for(0.02, mpp); // threshold 0.02 deg²

    let inner = probe_rect(4.0, 4.0, 4.25, 4.25); // 0.0625 deg² — 3.1x the threshold: kept, on purpose
    let around = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
        interiors: vec![vec![(4.0, 4.0), (4.25, 4.0), (4.25, 4.25), (4.0, 4.25), (4.0, 4.0)]],
    };
    let (out, stats) = coverage_simplify_fills(vec![(1, inner), (2, around)], &classes, 0.0, Some(e));
    assert_eq!(stats.fallbacks, 0, "{stats:?}");
    assert_eq!(stats.eliminated, 0, "the inner face is over the threshold and is kept: {stats:?}");

    let (_, mut enclosing, _) =
        out.iter().find(|(sid, _, _)| *sid == 2).cloned().expect("the enclosing class survives");
    let holes = |g: &Geom| match g {
        Geom::Polygon { interiors, .. } => interiors.len(),
        _ => 0,
    };
    assert_eq!(holes(&enclosing), 1, "it carries the inner face as a hole: {enclosing:?}");

    // This is exactly what `pipeline.rs` / `cut.rs` do to a `from_coverage` feature.
    let stripped = strip_small_holes(&mut enclosing, e.mpp, e.min_area_px);
    assert_eq!(stripped, 0, "the hole of a kept face survives, or z 6 would paint z 2 out of existence");
    assert_eq!(holes(&enclosing), 1, "and it is still there: {enclosing:?}");
    assert!(out.iter().any(|(sid, _, _)| *sid == 1), "with the face it belongs to: {out:?}");

    // The trim is not dead: a hole genuinely under the threshold — one no kept face can correspond
    // to, because such a face would have been absorbed into the class around it — still goes.
    let mut speck_hole = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0), (0.0, 0.0)],
        interiors: vec![vec![(4.0, 4.0), (4.05, 4.0), (4.05, 4.05), (4.0, 4.05), (4.0, 4.0)]],
    };
    assert_eq!(strip_small_holes(&mut speck_hole, e.mpp, e.min_area_px), 1, "a 0.0025 deg² hole is still trimmed");
}

/// Every early-return / degenerate path of `geom::coverage_api`, back to back and repeatedly, so a
/// missing free or a double free shows as a crash or unbounded RSS growth under a leak checker.
/// `PROBE_ROUNDS` turns it up for a deliberate leak hunt.
#[test]
fn the_coverage_api_survives_degenerate_inputs() {
    let square = probe_rect(0.0, 0.0, 1.0, 1.0);
    let line = Geom::Line(vec![(0.0, 0.0), (1.0, 1.0)]);
    let empty = Geom::Empty;
    let stub = Geom::Polygon { exterior: vec![(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)], interiors: vec![] };
    let two_pt = Geom::Polygon { exterior: vec![(0.0, 0.0), (1.0, 0.0)], interiors: vec![] };
    let bowtie = probe_poly(&[(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)]);
    let nan = probe_poly(&[(0.0, 0.0), (f64::NAN, 0.0), (1.0, 1.0), (0.0, 1.0)]);
    let inf = probe_poly(&[(0.0, 0.0), (f64::INFINITY, 0.0), (1.0, 1.0), (0.0, 1.0)]);
    let holed = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0)],
        // an unclosed hole ring: `build_ring` closes it rather than refusing
        interiors: vec![vec![(1.0, 1.0), (2.0, 1.0), (2.0, 2.0), (1.0, 2.0)]],
    };
    let bad_hole = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (4.0, 0.0), (4.0, 4.0), (0.0, 4.0), (0.0, 0.0)],
        interiors: vec![vec![(1.0, 1.0), (2.0, 1.0)]], // too short: build_ring returns null
    };
    let multi = Geom::Multi(vec![probe_rect(0.0, 0.0, 1.0, 1.0), probe_rect(2.0, 0.0, 3.0, 1.0)]);
    let overlapping = [&square, &probe_rect(0.5, 0.5, 1.5, 1.5)];

    let rounds = std::env::var("PROBE_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(200u32);
    for _ in 0..rounds {
        assert!(coverage_simplify_vw(&[], 0.1, false).is_none(), "empty input");
        assert!(!coverage_is_valid(&[], 0.0), "empty input is not a valid coverage");

        for g in [&line, &empty, &stub, &two_pt, &nan, &inf, &bad_hole, &multi] {
            // Each of these makes `build_polygon` (or GEOS) refuse; the whole call must bail out
            // with everything freed rather than crash.
            let _ = coverage_simplify_vw(&[g], 0.1, false);
            let _ = coverage_is_valid(&[g], 0.0);
            let _ = coverage_simplify_vw(&[&square, g], 0.1, false);
            let _ = coverage_is_valid(&[&square, g], 0.0);
        }
        // Invalid-but-buildable geometry: GEOS accepts the polygon, then throws inside the
        // coverage algorithm.
        let _ = coverage_simplify_vw(&[&bowtie], 0.1, false);
        let _ = coverage_is_valid(&[&bowtie, &square], 0.0);
        // A ring GEOS has to close for us.
        assert!(coverage_simplify_vw(&[&holed], 0.1, false).is_some(), "an unclosed hole ring is closed, not lost");
        // Not a coverage at all: overlapping members.
        assert!(!coverage_is_valid(&overlapping, 0.0), "overlaps are not a valid coverage");
        let _ = coverage_simplify_vw(&overlapping, 0.1, false);
        // The happy path, so the loop also exercises the success free.
        let ok = coverage_simplify_vw(&[&square, &probe_rect(1.0, 0.0, 2.0, 1.0)], 0.01, false);
        assert_eq!(ok.expect("a real coverage simplifies").len(), 2, "element count and order are preserved");
        assert!(coverage_is_valid(&[&square, &probe_rect(1.0, 0.0, 2.0, 1.0)], 0.0));
    }
}

/// The same wrapper under rayon: many threads, each creating and destroying its own GEOS context.
#[test]
fn the_coverage_api_is_thread_safe_under_rayon() {
    use rayon::prelude::*;
    let out: Vec<usize> = (0..2000u32)
        .into_par_iter()
        .map(|i| {
            let x = i as f64;
            let a = probe_rect(x, 0.0, x + 1.0, 1.0);
            let b = probe_rect(x + 1.0, 0.0, x + 2.0, 1.0);
            let bad = Geom::Line(vec![(x, 0.0), (x + 1.0, 1.0)]);
            let _ = coverage_simplify_vw(&[&a, &bad], 0.01, false);
            let _ = coverage_is_valid(&[&a, &b], 0.0);
            coverage_simplify_vw(&[&a, &b], 0.01, false).map(|v| v.len()).unwrap_or(0)
        })
        .collect();
    assert!(out.iter().all(|&n| n == 2), "every parallel call returned both elements");
}

/// Elimination under stress: a deterministic pseudo-random cluster of overlapping parcels across
/// several classes, with a threshold that binds on almost everything. The fixed point must
/// terminate, two runs must agree coordinate for coordinate, and the ground must be conserved —
/// elimination is a relabelling, and the base fill under everything means nothing is uneliminable.
#[test]
fn elimination_terminates_conserves_and_is_deterministic() {
    let styles: Vec<Style> = (1..=6).map(|i| probe_fill(i, i as i8, 0x0100 + i as u16)).collect();
    let classes = merge_classes(&styles);

    let build = || {
        // xorshift, so the fixture is identical every run and every process.
        let mut s: u64 = 0x2545F491_4F6CDD1D;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut v: Vec<(u8, Geom)> = vec![(1u8, probe_rect(0.0, 0.0, 20.0, 20.0))]; // a base "land" fill
        for i in 0..600 {
            let x = next() * 19.0;
            let y = next() * 19.0;
            let w = 0.02 + next() * 0.6;
            let h = 0.02 + next() * 0.6;
            v.push(((i % 5 + 2) as u8, probe_rect(x, y, x + w, y + h)));
        }
        v
    };

    let e = Some(threshold_for(0.35, 100.0)); // binds on most parcels, not on the base
    let (a, sa) = coverage_simplify_fills(build(), &classes, 0.02, e);
    let (b, sb) = coverage_simplify_fills(build(), &classes, 0.02, e);
    assert_eq!(sa, sb, "two runs, same counters: {sa:?} vs {sb:?}");
    assert_eq!(sa.fallbacks, 0, "no GEOS failure: {sa:?}");
    assert_eq!(sa.uneliminable_culled, 0, "the base fill is a neighbour to everything: {sa:?}");

    let key = |v: &[(u8, Geom, bool)]| {
        v.iter()
            .map(|(s, g, d)| {
                let mut pts = Vec::new();
                fn walk(g: &Geom, out: &mut Vec<(u64, u64)>) {
                    match g {
                        Geom::Polygon { exterior, interiors } => {
                            for r in std::iter::once(exterior).chain(interiors.iter()) {
                                out.extend(r.iter().map(|&(x, y)| (x.to_bits(), y.to_bits())));
                            }
                        }
                        Geom::Line(c) => out.extend(c.iter().map(|&(x, y)| (x.to_bits(), y.to_bits()))),
                        Geom::Multi(p) => p.iter().for_each(|p| walk(p, out)),
                        Geom::Empty => {}
                    }
                }
                walk(g, &mut pts);
                (*s, pts, *d)
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&a), key(&b), "two runs, same bytes");

    // Ground conservation: the union of the inputs is 20x20 (the base covers everything), and the
    // pass is a relabelling, so the output must still cover 400 deg².
    fn area_of(v: &[(u8, Geom, bool)]) -> f64 {
        fn ring(r: &[(f64, f64)]) -> f64 {
            let mut s = 0.0;
            for i in 0..r.len() {
                let (x1, y1) = r[i];
                let (x2, y2) = r[(i + 1) % r.len()];
                s += x1 * y2 - x2 * y1;
            }
            (s * 0.5).abs()
        }
        let mut t = 0.0;
        for (_, g, _) in v {
            if let Geom::Polygon { exterior, interiors } = g {
                t += ring(exterior);
                for h in interiors {
                    t -= ring(h);
                }
            }
        }
        t
    }
    let got = area_of(&a);
    assert!((got - 400.0).abs() < 0.5, "the ground is conserved through elimination: {got}");
}

/// **How wide an uncovered face the healing gate really admits.** The derived worst case is one
/// decimation tolerance of *mean half-width* (two neighbours each moving a full tolerance, in
/// opposite directions, opens a crack `2 x dec_tol` wide, and a ribbon's mean half-width is half
/// its width). `HEAL_WIDTH_TOLERANCES = 2.0` doubles that as margin, so the admitted ribbon is up
/// to `4 x dec_tol` wide — at the shipped preset's coarse tier (2200 m simplify, 275 m decimation)
/// that is a kilometre. This pins the number, because it is the one the docs have to state
/// honestly.
#[test]
fn healing_admits_a_ribbon_twice_the_widest_crack() {
    let classes = merge_classes(&[probe_fill(1, 0, 0x0001), probe_fill(2, 5, 0x0002)]);
    // The shipped LOD 0: 2200 m simplify.
    let tol = 2200.0 / M_PER_DEG;
    let dec_tol = tol / 8.0; // 275 m
    let widest_real_crack_deg = 2.0 * dec_tol; // both sides move a full tolerance, opposite ways

    // An uncovered ribbon of *twice* that width still heals.
    let w = 2.0 * widest_real_crack_deg * 0.98;
    let host = Geom::Polygon {
        exterior: vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)],
        interiors: vec![vec![(0.5, 1.0), (1.5, 1.0), (1.5, 1.0 + w), (0.5, 1.0 + w), (0.5, 1.0)]],
    };
    let feats = vec![(1u8, host), (2u8, probe_rect(2.0, 0.0, 3.0, 1.0))];
    let e = Some(threshold_for(0.5, 100.0));
    let (_out, stats) = coverage_simplify_fills(feats, &classes, tol, e);
    assert_eq!(stats.fallbacks, 0, "{stats:?}");
    let width_m = w * M_PER_DEG;
    assert_eq!(
        stats.healed, 1,
        "an uncovered ribbon {width_m:.0} m wide — twice the widest gap decimation can open — is \
         still filled in with a neighbour's class: {stats:?}"
    );
    assert!(width_m > 1000.0, "and that is over a kilometre: {width_m}");
}
