//! Format-contract tests for the OBCM v5 reader.
//!
//! Each test builds a synthetic `.obcm` byte buffer with a small handwritten
//! builder that mirrors `packer/obcm/serialize.py` exactly, then asserts the reader
//! parses it back. Building the bytes here (rather than checking in a binary
//! fixture) keeps the Rust and Python encoders pinned to the same layout: if
//! either drifts, these break.

use obc_reader::{BBox, Error, Kind, MapCache, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feature {
    pub style_id: u8,
    pub kind: Kind,
    pub exterior: Vec<(i32, i32)>,
    pub interiors: Vec<Vec<(i32, i32)>>,
}

fn decode_chunk(r: &Reader, lod: usize, chunk_id: u32, node: &BBox) -> Vec<Feature> {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    r.for_each_feature(lod, chunk_id, node, &mut points, &mut ring_lens, |f| {
        out.push(Feature {
            style_id: f.style_id,
            kind: f.kind,
            exterior: f.exterior().to_vec(),
            interiors: f.interiors().map(|h| h.to_vec()).collect(),
        });
    });
    out
}

/// Like [`decode_chunk`] but only the features for which `keep(style_id)` is
/// true are decoded and returned; the rest are skipped in the reader.
fn decode_filtered(r: &Reader, lod: usize, chunk_id: u32, node: &BBox, keep: impl Fn(u8) -> bool) -> Vec<Feature> {
    let mut out = Vec::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    r.for_each_feature_filtered(lod, chunk_id, node, &mut points, &mut ring_lens, keep, |f| {
        out.push(Feature {
            style_id: f.style_id,
            kind: f.kind,
            exterior: f.exterior().to_vec(),
            interiors: f.interiors().map(|h| h.to_vec()).collect(),
        });
    });
    out
}

const BRANCH_BIT: u32 = 0x8000_0000;
const EMPTY_LEAF: u32 = 0x7FFF_FFFF;
/// Distinctive (non-default) marker color so the round-trip test is meaningful.
const MARKER: u16 = 0xABCD;

// ---------------------------------------------------------------------------
// Byte builders (mirror serialize.py)
// ---------------------------------------------------------------------------

/// One LOD layer: its quadtree index (flat u32 nodes) and padded data chunks.
struct LodSpec {
    max_mpp: f32,
    index: Vec<u32>,
    chunks: Vec<Vec<u8>>,
    chunk_size: usize,
}

/// `bbox` is (min_lon, min_lat, max_lon, max_lat); `styles` are
/// (id, z_index, color_rgb565, weight).
fn build_file(bbox: (i32, i32, i32, i32), styles: &[(u8, i8, u16, u8, u8)], lods: &[LodSpec]) -> Vec<u8> {
    let style_off = 32usize;

    let mut style_bytes = vec![styles.len() as u8];
    for &(id, z, color, weight, priority) in styles {
        style_bytes.push(id);
        style_bytes.push(z as u8);
        style_bytes.extend_from_slice(&color.to_le_bytes());
        style_bytes.push(weight);
        style_bytes.push((priority - 1) & 0x03);
    }

    let lod_tab_off = style_off + style_bytes.len();
    let mut cursor = lod_tab_off + lods.len() * 18;
    let mut table = Vec::new();
    let mut payload = Vec::new();
    for lod in lods {
        let idx_off = cursor;
        let mut idx_bytes = Vec::new();
        for &node in &lod.index {
            idx_bytes.extend_from_slice(&node.to_le_bytes());
        }
        let mut chunk_bytes = Vec::new();
        for c in &lod.chunks {
            assert_eq!(c.len(), lod.chunk_size, "chunk must be padded to chunk_size");
            chunk_bytes.extend_from_slice(c);
        }
        table.extend_from_slice(&lod.max_mpp.to_le_bytes());
        table.extend_from_slice(&(idx_off as u32).to_le_bytes());
        table.extend_from_slice(&(lod.index.len() as u32).to_le_bytes());
        table.extend_from_slice(&(lod.chunk_size as u16).to_le_bytes());
        table.extend_from_slice(&(lod.chunks.len() as u32).to_le_bytes());
        cursor += idx_bytes.len() + chunk_bytes.len();
        payload.extend_from_slice(&idx_bytes);
        payload.extend_from_slice(&chunk_bytes);
    }

    // Header: <4sBiiiiIBIH  magic, ver, min_lat, min_lon, max_lat, max_lon,
    // style_off, lod_count, lod_table_off, marker_color.
    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(5);
    f.extend_from_slice(&bbox.1.to_le_bytes()); // min_lat
    f.extend_from_slice(&bbox.0.to_le_bytes()); // min_lon
    f.extend_from_slice(&bbox.3.to_le_bytes()); // max_lat
    f.extend_from_slice(&bbox.2.to_le_bytes()); // max_lon
    f.extend_from_slice(&(style_off as u32).to_le_bytes());
    f.push(lods.len() as u8);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    f.extend_from_slice(&MARKER.to_le_bytes());
    assert_eq!(f.len(), 32, "header must be 32 bytes");

    f.extend_from_slice(&style_bytes);
    f.extend_from_slice(&table);
    f.extend_from_slice(&payload);
    f
}

fn pad(mut chunk: Vec<u8>, size: usize) -> Vec<u8> {
    assert!(chunk.len() <= size);
    chunk.resize(size, 0xFF);
    chunk
}

/// A line feature with 8-bit deltas. Exterior point count = 1 + deltas.len().
fn pack_line(style_id: u8, ax: i32, ay: i32, deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x00); // flags: line, 8-bit deltas
    for &(dx, dy) in deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v
}

/// A line feature with 16-bit deltas (flag bit 0).
fn pack_line16(style_id: u8, ax: i32, ay: i32, deltas: &[(i16, i16)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x01); // flags: line, 16-bit deltas
    for &(dx, dy) in deltas {
        v.extend_from_slice(&dx.to_le_bytes());
        v.extend_from_slice(&dy.to_le_bytes());
    }
    v
}

/// A polygon with one hole, 8-bit deltas. Hole vertices are all deltas (first
/// relative to the anchor), so its stored point count == hole_deltas.len().
fn pack_poly_hole(style_id: u8, ax: i32, ay: i32, ext_deltas: &[(i8, i8)], hole_deltas: &[(i8, i8)]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(style_id);
    v.extend_from_slice(&((1 + ext_deltas.len()) as u16).to_le_bytes());
    v.extend_from_slice(&ax.to_le_bytes());
    v.extend_from_slice(&ay.to_le_bytes());
    v.push(0x06); // flags: polygon | has-holes, 8-bit deltas
    for &(dx, dy) in ext_deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v.push(1u8); // hole count
    v.extend_from_slice(&(hole_deltas.len() as u16).to_le_bytes());
    for &(dx, dy) in hole_deltas {
        v.push(dx as u8);
        v.push(dy as u8);
    }
    v
}

// A two-LOD file used by several tests: LOD0 (coarse, +inf) holds one line,
// LOD1 (max_mpp 50) holds one polygon-with-hole. Both are single-leaf trees over
// the global bbox (0,0,1000,1000), so the leaf's node bbox is the global bbox
// and feature anchors are absolute.
const CS: usize = 64;
const GLOBAL: (i32, i32, i32, i32) = (0, 0, 1000, 1000);
const STYLES: &[(u8, i8, u16, u8, u8)] = &[(1, 3, 0xF800, 2, 3), (2, -1, 0x07E0, 1, 3)];

fn two_lod_file() -> Vec<u8> {
    let line = pad(pack_line(1, 100, 200, &[(10, 0), (0, 10)]), CS);
    let poly =
        pad(pack_poly_hole(2, 100, 100, &[(100, 0), (0, 100), (-100, 0)], &[(25, 25), (50, 0), (0, 50), (-50, 0)]), CS);
    build_file(
        GLOBAL,
        STYLES,
        &[
            LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![line], chunk_size: CS },
            LodSpec { max_mpp: 50.0, index: vec![0], chunks: vec![poly], chunk_size: CS },
        ],
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn header_and_lod_table() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    assert_eq!(r.version, 5);
    assert_eq!(r.marker_color, MARKER);
    assert_eq!(r.bbox.min_lon, 0);
    assert_eq!(r.bbox.min_lat, 0);
    assert_eq!(r.bbox.max_lon, 1000);
    assert_eq!(r.bbox.max_lat, 1000);

    let lods = r.lods();
    assert_eq!(lods.len(), 2);
    assert!(lods[0].max_mpp.is_infinite());
    assert_eq!(lods[1].max_mpp, 50.0);
    assert_eq!(lods[0].node_count, 1);
    assert_eq!(lods[0].chunk_size, CS);
    assert_eq!(lods[0].chunk_count, 1);
    assert_eq!(lods[1].chunk_count, 1);
}

#[test]
fn marker_color_round_trips() {
    // The header's marker color (v4) parses back unchanged at its fixed offset.
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    assert_eq!(r.marker_color, MARKER);
}

#[test]
fn styles_parse() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    let s1 = r.style(1).expect("style 1");
    assert_eq!(s1.z_index, 3);
    assert_eq!(s1.color, 0xF800);
    assert_eq!(s1.weight, 2);
    assert_eq!(s1.priority, 3);

    let s2 = r.style(2).expect("style 2");
    assert_eq!(s2.z_index, -1);
    assert_eq!(s2.color, 0x07E0);

    assert!(r.style(200).is_none());
}

#[test]
fn backdrop_is_lowest_z_regardless_of_id() {
    // STYLES = [(id 1, z 3), (id 2, z -1)]. The backdrop is the bottom of the
    // paint order (lowest z), i.e. id 2 — not the lowest id. This guards the
    // sea/background lookup against style-ID reassignment.
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    let bg = r.backdrop_style().expect("a backdrop");
    assert_eq!(bg.id, 2);
    assert_eq!(bg.z_index, -1);
    assert_eq!(bg.color, 0x07E0);
}

#[test]
fn select_lod_for_mpp_picks_finest_covering() {
    let bytes = two_lod_file(); // max_mpp = [+inf, 50]
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    assert_eq!(r.select_lod_for_mpp(1000.0), 0); // only +inf covers
    assert_eq!(r.select_lod_for_mpp(51.0), 0); // 50 doesn't cover
    assert_eq!(r.select_lod_for_mpp(50.0), 1); // boundary: 50 >= 50 covers
    assert_eq!(r.select_lod_for_mpp(49.0), 1);
    assert_eq!(r.select_lod_for_mpp(0.0), 1); // finest
}

#[test]
fn query_single_leaf() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    // A view overlapping the global bbox hits the single leaf (chunk 0).
    let hits = r.query::<64>(0, &obc_reader::BBox { min_lon: 100, min_lat: 100, max_lon: 200, max_lat: 200 });
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 0);
    assert_eq!(hits[0].1, r.bbox); // leaf node bbox == global bbox

    // A view entirely outside the global bbox hits nothing.
    let miss = r.query::<64>(0, &obc_reader::BBox { min_lon: 5000, min_lat: 5000, max_lon: 6000, max_lat: 6000 });
    assert!(miss.is_empty());
}

#[test]
fn decode_line() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    let node = r.bbox;

    let feats = decode_chunk(&r, 0, 0, &node);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    assert_eq!(f.style_id, 1);
    assert_eq!(f.kind, Kind::Line);
    assert_eq!(f.exterior, vec![(100, 200), (110, 200), (110, 210)]);
    assert!(f.interiors.is_empty());
}

#[test]
fn decode_polygon_with_hole() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    let node = r.bbox;

    let feats = decode_chunk(&r, 1, 0, &node);
    assert_eq!(feats.len(), 1);
    let f = &feats[0];
    assert_eq!(f.style_id, 2);
    assert_eq!(f.kind, Kind::Polygon);
    assert_eq!(f.exterior, vec![(100, 100), (200, 100), (200, 200), (100, 200)]);
    assert_eq!(f.interiors.len(), 1);
    assert_eq!(f.interiors[0], vec![(125, 125), (175, 125), (175, 175), (125, 175)]);
}

#[test]
fn visitor_matches_owned_decode() {
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    let node = r.bbox;

    // The borrowing for_each_feature must yield exactly what decode_chunk does.
    let owned = decode_chunk(&r, 1, 0, &node);

    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let mut seen = 0;
    r.for_each_feature(1, 0, &node, &mut points, &mut ring_lens, |f| {
        let o = &owned[seen];
        assert_eq!(f.style_id, o.style_id);
        assert_eq!(f.kind, o.kind);
        assert_eq!(f.exterior(), o.exterior.as_slice());
        let holes: Vec<&[(i32, i32)]> = f.interiors().collect();
        assert_eq!(holes.len(), o.interiors.len());
        for (h, oh) in holes.iter().zip(&o.interiors) {
            assert_eq!(*h, oh.as_slice());
        }
        seen += 1;
    });
    assert_eq!(seen, owned.len());
}

#[test]
fn decode_16bit_deltas() {
    // Deltas beyond the int8 range force the 16-bit path (flag bit 0).
    let bbox = (0, 0, 1_000_000, 1_000_000);
    let line = pad(pack_line16(1, 0, 0, &[(300, 400), (-200, 0)]), CS);
    let bytes = build_file(
        bbox,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![line], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    let feats = decode_chunk(&r, 0, 0, &r.bbox);
    assert_eq!(feats.len(), 1);
    assert_eq!(feats[0].exterior, vec![(0, 0), (300, 400), (100, 400)]);
}

#[test]
fn quadtree_subdivision_and_node_bbox() {
    // Root branch → 4 children (NW, NE, SW, SE); only NW is a non-empty leaf.
    // Exercises query_rec subdivision and the NW node-bbox math.
    // Global bbox (0,0,1000,1000); midpoints (500,500); NW = (0,500,500,1000).
    // Anchor is relative to the NW node's min corner (0,500): ax=10, ay=10.
    let line = pad(pack_line(1, 10, 10, &[(5, 5)]), CS);
    let index = vec![
        BRANCH_BIT | 1, // root: branch, children start at idx 1
        0,              // NW: leaf -> chunk 0
        EMPTY_LEAF,     // NE
        EMPTY_LEAF,     // SW
        EMPTY_LEAF,     // SE
    ];
    let bytes =
        build_file(GLOBAL, STYLES, &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![line], chunk_size: CS }]);
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    let nw = obc_reader::BBox { min_lon: 0, min_lat: 500, max_lon: 500, max_lat: 1000 };

    // View inside the NW quadrant hits the leaf, with the NW node bbox.
    let hits = r.query::<64>(0, &obc_reader::BBox { min_lon: 50, min_lat: 600, max_lon: 150, max_lat: 700 });
    assert_eq!(hits.as_slice(), &[(0, nw)]);

    // View inside the (empty) SE quadrant hits nothing.
    let se = r.query::<64>(0, &obc_reader::BBox { min_lon: 600, min_lat: 100, max_lon: 700, max_lat: 200 });
    assert!(se.is_empty());

    // The feature's anchor is computed from the NW node's min corner (0,500):
    // ax=10, ay=10 → absolute (10, 510), then +(5,5).
    let feats = decode_chunk(&r, 0, 0, &nw);
    assert_eq!(feats[0].exterior, vec![(10, 510), (15, 515)]);
}

#[test]
fn empty_leaf_yields_nothing() {
    let empty = pad(vec![], CS); // all 0xFF
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![EMPTY_LEAF], chunks: vec![empty], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    assert!(r.query::<64>(0, &r.bbox).is_empty());
}

#[test]
fn rejects_bad_input() {
    // Reader isn't Debug, so match the Err arm rather than using unwrap_err.
    let cache = MapCache::new();
    let err = |b: &[u8]| match Reader::new(&SliceSource(b), &cache) {
        Ok(_) => panic!("expected Err"),
        Err(e) => e,
    };

    assert_eq!(err(&[0u8; 10]), Error::TooShort);

    let mut bytes = two_lod_file();
    bytes[0] = b'X';
    assert_eq!(err(&bytes), Error::BadMagic);

    let mut bytes = two_lod_file();
    bytes[4] = 4; // v4 (and earlier) no longer supported — only v5 is read
    assert_eq!(err(&bytes), Error::BadVersion);
}

#[test]
fn out_of_range_chunk_id_decodes_nothing() {
    // `chunk_id` comes from a quadtree leaf and is never otherwise constrained to
    // `chunk_count`. LOD0 here holds a single chunk, so id 1 already points one
    // past it — straight into LOD1's bytes. The reader must decode nothing rather
    // than silently decode the adjacent layer (visible even on the 64-bit host)
    // or, on the 32-bit device, wrap the offset and panic. `u32::MAX` is the
    // arithmetic-overflow edge.
    let bytes = two_lod_file();
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    let node = r.bbox;
    assert!(decode_chunk(&r, 0, 1, &node).is_empty());
    assert!(decode_chunk(&r, 0, u32::MAX, &node).is_empty());
    // Filtered path shares the same guard.
    assert!(decode_filtered(&r, 0, 1, &node, |_| true).is_empty());
    // The in-range chunk still decodes, so the guard isn't over-broad.
    assert_eq!(decode_chunk(&r, 0, 0, &node).len(), 1);
}

#[test]
fn rejects_overflowing_lod_table_offset() {
    // A `lod_table_offset` near the top of the u32 range wraps `usize` once the
    // table length is added on the 32-bit device, passing the file-length guard
    // and then indexing far out of the file. Checked arithmetic rejects it up
    // front; on the 64-bit host the same value simply exceeds data.len().
    let mut bytes = two_lod_file();
    bytes[26..30].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes()); // header lod_table_offset
    assert!(matches!(Reader::new(&SliceSource(&bytes), &MapCache::new()), Err(Error::BadOffset)));
}

#[test]
fn rejects_overflowing_chunk_region() {
    // A corrupt LOD entry advertising a huge chunk_count × chunk_size must be
    // rejected, not trusted: on the 32-bit device the product wraps `usize` and
    // the computed chunks-end can land below data.len(), admitting a layer that
    // indexes out of the file. Checked arithmetic turns it into BadOffset.
    let mut bytes = two_lod_file();
    let lod_tab_off = u32::from_le_bytes(bytes[26..30].try_into().unwrap()) as usize;
    // Entry layout: max_mpp(4) index_off(4) node_count(4) chunk_size(2) chunk_count(4).
    let chunk_size_at = lod_tab_off + 12;
    let chunk_count_at = lod_tab_off + 14;
    bytes[chunk_size_at..chunk_size_at + 2].copy_from_slice(&u16::MAX.to_le_bytes());
    bytes[chunk_count_at..chunk_count_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(Reader::new(&SliceSource(&bytes), &MapCache::new()), Err(Error::BadOffset)));
}

#[test]
fn filtered_decode_skips_without_drifting() {
    // Three heterogeneous features packed back-to-back in one chunk: an 8-bit
    // line, a polygon-with-hole, and a 16-bit line. Skipping any of them must
    // leave the reader's byte offset exactly where a full decode would, so the
    // features *after* a skipped one decode byte-identically. This pins
    // `skip_ring` to `read_ring`: if they ever drift, a trailing feature would
    // decode garbage and these assertions break.
    let mut chunk = Vec::new();
    chunk.extend_from_slice(&pack_line(1, 100, 200, &[(10, 0), (0, 10)]));
    chunk.extend_from_slice(&pack_poly_hole(
        2,
        300,
        300,
        &[(50, 0), (0, 50), (-50, 0)],
        &[(10, 10), (20, 0), (0, 20), (-20, 0)],
    ));
    chunk.extend_from_slice(&pack_line16(3, 0, 0, &[(300, 400), (-200, 0)]));
    let chunk = pad(chunk, 128);

    let styles: &[(u8, i8, u16, u8, u8)] = &[(1, 3, 0xF800, 2, 3), (2, -1, 0x07E0, 1, 3), (3, 0, 0x001F, 1, 3)];
    let bytes = build_file(
        GLOBAL,
        styles,
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![0], chunks: vec![chunk], chunk_size: 128 }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();
    let node = r.bbox;

    let all = decode_chunk(&r, 0, 0, &node);
    assert_eq!(all.len(), 3);

    // Keeping everything is identical to the unfiltered decode path.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |_| true), all);
    // Keeping nothing visits nothing.
    assert!(decode_filtered(&r, 0, 0, &node, |_| false).is_empty());

    // Skip the middle polygon: the trailing 16-bit line must still be exact.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |sid| sid != 2), vec![all[0].clone(), all[2].clone()]);
    // Skip the leading line: both following features must be exact.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |sid| sid != 1), vec![all[1].clone(), all[2].clone()]);
    // Skip the trailing line: the leading two are unaffected.
    assert_eq!(decode_filtered(&r, 0, 0, &node, |sid| sid != 3), vec![all[0].clone(), all[1].clone()]);
}

#[test]
fn for_each_chunk_has_no_cap() {
    // Root branch with four non-empty leaf quadrants. `for_each_chunk` streams
    // every overlapping leaf through its callback with no upper bound, whereas a
    // capacity-bounded `query` silently truncates — the exact behaviour the
    // renderer depends on so a wide viewport never silently loses whole chunks.
    let mk = || pad(pack_line(1, 1, 1, &[(1, 1)]), CS);
    let index = vec![
        BRANCH_BIT | 1, // root branch, children start at idx 1
        0,              // NW -> chunk 0
        1,              // NE -> chunk 1
        2,              // SW -> chunk 2
        3,              // SE -> chunk 3
    ];
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![mk(), mk(), mk(), mk()], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    // A view over the whole bbox overlaps all four leaves.
    let mut seen = 0;
    r.for_each_chunk(0, &r.bbox, |_cid, _node| seen += 1);
    assert_eq!(seen, 4);

    // The same query into a 2-slot buffer keeps only the first two it reaches.
    let capped = r.query::<2>(0, &r.bbox);
    assert_eq!(capped.len(), 2);
}

/// Build a forward-only quadtree index that is a single NW-chain `levels` branches deep, ending
/// in a non-empty leaf (chunk 0). Each branch's four children are contiguous (NW, NE, SW, SE):
/// NE/SW/SE are empty leaves and NW continues the chain, so every child index is strictly greater
/// than its parent's — the `child > idx` invariant of a well-formed map holds, isolating the
/// **depth** cap as the only thing that can stop the descent.
fn nw_chain_index(levels: usize) -> Vec<u32> {
    let mut index: Vec<u32> = vec![0]; // slot 0 is the root branch, filled below
    let mut cur = 0usize;
    for _ in 0..levels {
        let base = index.len(); // the four children are appended here, after `cur`
        index[cur] = BRANCH_BIT | base as u32;
        index.push(0); // NW: next chain node (overwritten next iteration, or the final leaf)
        index.push(EMPTY_LEAF); // NE
        index.push(EMPTY_LEAF); // SW
        index.push(EMPTY_LEAF); // SE
        cur = base;
    }
    index[cur] = 0; // deepest NW is a non-empty leaf -> chunk 0
    index
}

#[test]
fn walk_terminates_on_back_referencing_branch() {
    // A corrupt map whose root branch points its first child back at itself (`child == idx`).
    // The node bbox would shrink toward the NW corner and then stay put, so `intersects(view)`
    // never goes false — with no guard the walk recurses forever and stack-overflows (a HardFault
    // on the MCU, which has no MMU guard page; issue #65). The `child > idx` guard rejects the
    // back-edge, so the walk must simply return, reporting no chunks.
    let chunk = pad(pack_line(1, 0, 0, &[(1, 1)]), CS);
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![BRANCH_BIT], // root branch, child base 0 == its own index
            chunks: vec![chunk],
            chunk_size: CS,
        }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    // A viewport over the whole bbox keeps intersecting the (degenerate) node every level — the
    // condition under which the unguarded walk would never terminate.
    let mut seen = 0;
    r.for_each_chunk(0, &r.bbox, |_cid, _node| seen += 1);
    assert_eq!(seen, 0);
    // `query` walks the same path; it too must return rather than overflow.
    assert!(r.query::<64>(0, &r.bbox).is_empty());
}

#[test]
fn walk_caps_depth_on_forward_chain() {
    // A forward-only NW-chain (every `child > idx`, so the back-reference guard never fires) far
    // deeper than the depth cap (~32). The node bbox degenerates to the NW corner after ~10
    // levels but keeps intersecting a whole-bbox viewport forever, so without the depth cap the
    // walk would descend all `LEVELS` levels and report the leaf's chunk. With the cap it stops
    // first, pruning the over-cap leaf — so no chunk is reported. This pins the depth cap
    // independently of the `child > idx` guard (issue #65).
    const LEVELS: usize = 50; // comfortably past the ~32 cap
    let chunk = pad(pack_line(1, 0, 0, &[(1, 1)]), CS);
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index: nw_chain_index(LEVELS), chunks: vec![chunk], chunk_size: CS }],
    );
    let cache = MapCache::new();
    let src = SliceSource(&bytes);
    let r = Reader::new(&src, &cache).unwrap();

    let mut seen = 0;
    r.for_each_chunk(0, &r.bbox, |_cid, _node| seen += 1);
    assert_eq!(seen, 0, "the depth cap must prune the over-cap leaf before it is reached");
}
