//! End-to-end round-trip: pack with the real `obc-pack` serializer, read back with
//! the real `obc-reader`.
//!
//! The sibling suites pin each half against separately hand-coded bytes, so a
//! writer/reader disagreement on the shared format (LOD-table field order, header
//! bbox lat/lon ordering, priority flags, the delta/hole encodings) keeps both
//! green while every real map is corrupt. This test closes that loop and asserts
//! styles, marker, LOD table, and per-feature geometry survive intact.
//!
//! Every coordinate is an exact integer microdegree fed in as `udeg / 1e6` (so
//! `to_udeg` recovers it exactly) and every segment stays under the 30 000-µdeg
//! densify threshold, so no midpoints are inserted and point counts are preserved.

use obc_pack::{serialize_lods, Feature, Kind as PackKind, LodLayer, Node, Style};
use obc_reader::{BBox, Kind as ReadKind, MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// Global bbox (min_lon, min_lat, max_lon, max_lat) in microdegrees. min corner
/// is (0,0) so each single-leaf node's anchor base is the origin and decoded
/// coordinates are absolute — directly comparable to the inputs. max_lon and
/// max_lat differ (2° vs 1°) so a lat/lon swap in the header would be caught.
const GLOBAL: (i64, i64, i64, i64) = (0, 0, 2_000_000, 1_000_000);
const MARKER: u16 = 0xBEEF;

// Small features (microdegrees). Kept under 30 000 µdeg per segment so nothing
// densifies, and polygon rings are pre-closed (first == last) like shapely's.
const LINE5: &[(i32, i32)] = &[(200_000, 50_000), (200_050, 50_050), (200_100, 50_000)];
const POLY12_EXT: &[(i32, i32)] =
    &[(100_000, 100_000), (120_000, 100_000), (120_000, 120_000), (100_000, 120_000), (100_000, 100_000)];
const POLY12_HOLE: &[(i32, i32)] =
    &[(105_000, 105_000), (115_000, 105_000), (115_000, 115_000), (105_000, 115_000), (105_000, 105_000)];
// Deltas of 500 µdeg exceed the int8 range, forcing the 16-bit delta path.
const LINE16: &[(i32, i32)] = &[(300_000, 300_000), (300_500, 300_500), (301_000, 300_500)];

fn styles() -> Vec<Style> {
    vec![
        // Lowest z_index → the backdrop. Negative z and priority 1 (flags 0).
        Style { id: 1, z_index: -2, color: 0x07E0, weight: 1, priority: 1, dashed: false, color2: None },
        // Priority 4 (flags 3, the top of the clamped range). Dashed + a secondary color exercises
        // the v10 flag bits (2 and 3) and the trailing color2 u16 through the whole pack→read path.
        Style { id: 5, z_index: 3, color: 0xF800, weight: 2, priority: 4, dashed: true, color2: Some(0x8410) },
        // Mid priority; non-contiguous id exercises the sparse style lookup.
        Style { id: 12, z_index: 0, color: 0x001F, weight: 3, priority: 2, dashed: false, color2: None },
    ]
}

/// A line that fills the reader's per-feature buffer to exactly `MAX_FEAT_PTS`.
/// Small alternating steps keep it on the 8-bit delta path with no densification,
/// so all `MAX_FEAT_PTS` vertices round-trip intact.
fn big_line_points() -> Vec<(i32, i32)> {
    (0..MAX_FEAT_PTS as i32).map(|i| (10_000 + i * 100, 500_000 + (i % 2) * 50)).collect()
}

fn deg(udeg: i32) -> f64 {
    udeg as f64 / 1e6
}

fn ring_deg(pts: &[(i32, i32)]) -> Vec<(f64, f64)> {
    pts.iter().map(|&(x, y)| (deg(x), deg(y))).collect()
}

fn line(style_id: u8, pts: &[(i32, i32)]) -> Feature {
    Feature { style_id, kind: PackKind::Line, rings: vec![ring_deg(pts)] }
}

fn polygon(style_id: u8, rings: &[&[(i32, i32)]]) -> Feature {
    Feature { style_id, kind: PackKind::Polygon, rings: rings.iter().map(|r| ring_deg(r)).collect() }
}

/// Build a two-LOD map and serialize it the way the packer really does:
/// `serialize_lods` over a `Node` pyramid. LOD0 is the coarse (+inf) layer with a
/// line and a polygon-with-hole; LOD1 (max_mpp 50) holds a 16-bit line and the
/// `MAX_FEAT_PTS` line. Both are single-leaf trees over the global bbox.
fn packed() -> Vec<u8> {
    let lod0 = LodLayer {
        max_mpp: None, // coarsest layer ⇒ +inf
        chunk_size: 512,
        root: Node::Leaf { bbox: GLOBAL, features: vec![line(5, LINE5), polygon(12, &[POLY12_EXT, POLY12_HOLE])] },
    };
    let lod1 = LodLayer {
        max_mpp: Some(50.0),
        chunk_size: 8192, // must hold the ~4 KiB MAX_FEAT_PTS line
        root: Node::Leaf { bbox: GLOBAL, features: vec![line(1, LINE16), line(5, &big_line_points())] },
    };
    let (bytes, dropped) = serialize_lods(
        &[lod0, lod1],
        &styles(),
        MARKER,
        GLOBAL,
        &[],
        &Default::default(),
        &obc_pack::config::default_profiles(),
    );
    assert_eq!(dropped, 0, "every fixture feature fits its chunk");
    bytes
}

/// A decoded feature in a comparable, owned form.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Decoded {
    style_id: u8,
    is_polygon: bool,
    exterior: Vec<(i32, i32)>,
    interiors: Vec<Vec<(i32, i32)>>,
}

fn expect_line(style_id: u8, pts: &[(i32, i32)]) -> Decoded {
    Decoded { style_id, is_polygon: false, exterior: pts.to_vec(), interiors: vec![] }
}

fn expect_poly(style_id: u8, ext: &[(i32, i32)], holes: &[&[(i32, i32)]]) -> Decoded {
    Decoded {
        style_id,
        is_polygon: true,
        exterior: ext.to_vec(),
        interiors: holes.iter().map(|h| h.to_vec()).collect(),
    }
}

/// Decode every feature in a LOD, in quadtree → chunk → feature order, through
/// the real allocation-free reader path (`for_each_chunk` + `for_each_feature`).
fn decode_lod(r: &Reader, lod: usize) -> Vec<Decoded> {
    let mut chunks: Vec<(u32, BBox)> = Vec::new();
    r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node)));

    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for (cid, node) in chunks {
        r.for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| {
            out.push(Decoded {
                style_id: f.style_id,
                is_polygon: f.kind == ReadKind::Polygon,
                exterior: f.exterior().to_vec(),
                interiors: f.interiors().map(|h| h.to_vec()).collect(),
            });
        });
    }
    out
}

/// Collect every leaf `for_each_chunk` yields — the uncapped replacement for the
/// removed `Reader::query` test convenience.
fn query_all(r: &Reader, lod: usize, view: &BBox) -> Vec<(u32, BBox)> {
    let mut out = Vec::new();
    r.for_each_chunk(lod, view, |cid, node| out.push((cid, node)));
    out
}

#[test]
fn header_round_trips() {
    let bytes = packed();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    assert_eq!(r.version, 10);
    assert_eq!(r.marker_color, MARKER);
    // bbox stored lat,lon,lat,lon in the header; the reader must hand it back
    // with lon and lat in the right fields (max_lon=2°, max_lat=1°).
    assert_eq!(r.bbox, BBox { min_lon: 0, min_lat: 0, max_lon: 2_000_000, max_lat: 1_000_000 });
}

#[test]
fn styles_round_trip() {
    let bytes = packed();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let s1 = r.style(1).expect("style 1");
    assert_eq!((s1.z_index, s1.color, s1.weight, s1.priority), (-2, 0x07E0, 1, 1));
    assert!(!s1.dashed);
    assert_eq!(s1.color2, None);

    let s5 = r.style(5).expect("style 5");
    assert_eq!((s5.z_index, s5.color, s5.weight, s5.priority), (3, 0xF800, 2, 4));
    assert!(s5.dashed, "line_style survives the pack → read round trip");
    assert_eq!(s5.color2, Some(0x8410), "color2 survives the pack → read round trip");

    let s12 = r.style(12).expect("style 12");
    assert_eq!((s12.z_index, s12.color, s12.weight, s12.priority), (0, 0x001F, 3, 2));
    assert!(!s12.dashed);
    assert_eq!(s12.color2, None);

    // Unused ids are absent.
    assert!(r.style(2).is_none());

    // Backdrop is the lowest z_index (id 1), independent of style id ordering.
    assert_eq!(r.backdrop_style().expect("backdrop").id, 1);
}

#[test]
fn lod_table_and_selection() {
    let bytes = packed();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    let lods = r.lods();
    assert_eq!(lods.len(), 2);
    assert!(lods[0].max_mpp.is_infinite()); // coarsest
    assert_eq!(lods[1].max_mpp, 50.0);
    assert_eq!(lods[0].chunk_size, 512);
    assert_eq!(lods[1].chunk_size, 8192);
    // One single-leaf tree per LOD ⇒ one node, one populated chunk each.
    assert_eq!(lods[0].node_count, 1);
    assert_eq!(lods[0].chunk_count, 1);
    assert_eq!(lods[1].chunk_count, 1);

    // Multi-LOD selection: coarse for far-out mpp, fine once we cross 50.
    assert_eq!(r.select_lod_for_mpp(1000.0), 0);
    assert_eq!(r.select_lod_for_mpp(51.0), 0);
    assert_eq!(r.select_lod_for_mpp(50.0), 1); // boundary: 50 >= 50 covers
    assert_eq!(r.select_lod_for_mpp(5.0), 1);
}

#[test]
fn features_round_trip() {
    let bytes = packed();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);

    // LOD0: an 8-bit line and a 16-bit polygon-with-hole.
    assert_eq!(decode_lod(&r, 0), vec![expect_line(5, LINE5), expect_poly(12, POLY12_EXT, &[POLY12_HOLE])],);

    // LOD1: a 16-bit line and the MAX_FEAT_PTS line.
    let big = big_line_points();
    assert_eq!(decode_lod(&r, 1), vec![expect_line(1, LINE16), expect_line(5, &big)]);
}

#[test]
fn max_feat_pts_boundary_survives() {
    let bytes = packed();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    let d1 = decode_lod(&r, 1);
    // The big line filled the reader's per-feature buffer to exactly its cap and
    // was decoded without truncation.
    assert_eq!(d1[1].exterior.len(), MAX_FEAT_PTS);
    assert_eq!(d1[1].exterior, big_line_points());
}

#[test]
fn query_finds_the_leaf() {
    let bytes = packed();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let r = Reader::new(&src, &tables, &cache);
    // A view overlapping the global bbox hits the single populated leaf; the
    // returned node bbox is the global bbox.
    let inside = BBox { min_lon: 90_000, min_lat: 90_000, max_lon: 130_000, max_lat: 130_000 };
    let hits = query_all(&r, 0, &inside);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, r.bbox);

    // A view fully outside the bbox hits nothing.
    let outside = BBox { min_lon: 9_000_000, min_lat: 9_000_000, max_lon: 9_001_000, max_lat: 9_001_000 };
    assert!(query_all(&r, 0, &outside).is_empty());
}
