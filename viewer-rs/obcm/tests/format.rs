//! Format-contract tests for the OBCM v3 reader.
//!
//! Each test builds a synthetic `.obcm` byte buffer with a small handwritten
//! builder that mirrors `obcm/serialize.py` exactly, then asserts the reader
//! parses it back. Building the bytes here (rather than checking in a binary
//! fixture) keeps the Rust and Python encoders pinned to the same layout: if
//! either drifts, these break. Runs without the `render` feature (no SDL).

use obcm::{Error, Kind, Reader};

const BRANCH_BIT: u32 = 0x8000_0000;
const EMPTY_LEAF: u32 = 0x7FFF_FFFF;

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
fn build_file(
    bbox: (i32, i32, i32, i32),
    styles: &[(u8, i8, u16, u8)],
    lods: &[LodSpec],
) -> Vec<u8> {
    let style_off = 30usize;

    let mut style_bytes = vec![styles.len() as u8];
    for &(id, z, color, weight) in styles {
        style_bytes.push(id);
        style_bytes.push(z as u8);
        style_bytes.extend_from_slice(&color.to_le_bytes());
        style_bytes.push(weight);
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

    // Header: <4sBiiiiIBI  magic, ver, min_lat, min_lon, max_lat, max_lon,
    // style_off, lod_count, lod_table_off.
    let mut f = Vec::new();
    f.extend_from_slice(b"OBCM");
    f.push(3);
    f.extend_from_slice(&bbox.1.to_le_bytes()); // min_lat
    f.extend_from_slice(&bbox.0.to_le_bytes()); // min_lon
    f.extend_from_slice(&bbox.3.to_le_bytes()); // max_lat
    f.extend_from_slice(&bbox.2.to_le_bytes()); // max_lon
    f.extend_from_slice(&(style_off as u32).to_le_bytes());
    f.push(lods.len() as u8);
    f.extend_from_slice(&(lod_tab_off as u32).to_le_bytes());
    assert_eq!(f.len(), 30, "header must be 30 bytes");

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
fn pack_poly_hole(
    style_id: u8,
    ax: i32,
    ay: i32,
    ext_deltas: &[(i8, i8)],
    hole_deltas: &[(i8, i8)],
) -> Vec<u8> {
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
const STYLES: &[(u8, i8, u16, u8)] = &[(1, 3, 0xF800, 2), (2, -1, 0x07E0, 1)];

fn two_lod_file() -> Vec<u8> {
    let line = pad(pack_line(1, 100, 200, &[(10, 0), (0, 10)]), CS);
    let poly = pad(
        pack_poly_hole(
            2,
            100,
            100,
            &[(100, 0), (0, 100), (-100, 0)],
            &[(25, 25), (50, 0), (0, 50), (-50, 0)],
        ),
        CS,
    );
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
    let r = Reader::new(&bytes).unwrap();

    assert_eq!(r.version, 3);
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
fn styles_parse() {
    let bytes = two_lod_file();
    let r = Reader::new(&bytes).unwrap();

    let s1 = r.style(1).expect("style 1");
    assert_eq!(s1.z_index, 3);
    assert_eq!(s1.color, 0xF800);
    assert_eq!(s1.weight, 2);

    let s2 = r.style(2).expect("style 2");
    assert_eq!(s2.z_index, -1);
    assert_eq!(s2.color, 0x07E0);

    assert!(r.style(200).is_none());
}

#[test]
fn select_lod_for_mpp_picks_finest_covering() {
    let bytes = two_lod_file(); // max_mpp = [+inf, 50]
    let r = Reader::new(&bytes).unwrap();

    assert_eq!(r.select_lod_for_mpp(1000.0), 0); // only +inf covers
    assert_eq!(r.select_lod_for_mpp(51.0), 0); // 50 doesn't cover
    assert_eq!(r.select_lod_for_mpp(50.0), 1); // boundary: 50 >= 50 covers
    assert_eq!(r.select_lod_for_mpp(49.0), 1);
    assert_eq!(r.select_lod_for_mpp(0.0), 1); // finest
}

#[test]
fn query_single_leaf() {
    let bytes = two_lod_file();
    let r = Reader::new(&bytes).unwrap();

    // A view overlapping the global bbox hits the single leaf (chunk 0).
    let hits = r.query(0, &obcm::BBox { min_lon: 100, min_lat: 100, max_lon: 200, max_lat: 200 });
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0, 0);
    assert_eq!(hits[0].1, r.bbox); // leaf node bbox == global bbox

    // A view entirely outside the global bbox hits nothing.
    let miss =
        r.query(0, &obcm::BBox { min_lon: 5000, min_lat: 5000, max_lon: 6000, max_lat: 6000 });
    assert!(miss.is_empty());
}

#[test]
fn decode_line() {
    let bytes = two_lod_file();
    let r = Reader::new(&bytes).unwrap();
    let node = r.bbox;

    let feats = r.decode_chunk(0, 0, &node);
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
    let r = Reader::new(&bytes).unwrap();
    let node = r.bbox;

    let feats = r.decode_chunk(1, 0, &node);
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
    let r = Reader::new(&bytes).unwrap();
    let node = r.bbox;

    // The borrowing for_each_feature must yield exactly what decode_chunk does.
    let owned = r.decode_chunk(1, 0, &node);

    let mut points = Vec::new();
    let mut ring_lens = Vec::new();
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
    let r = Reader::new(&bytes).unwrap();
    let feats = r.decode_chunk(0, 0, &r.bbox);
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
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec { max_mpp: f32::INFINITY, index, chunks: vec![line], chunk_size: CS }],
    );
    let r = Reader::new(&bytes).unwrap();

    let nw = obcm::BBox { min_lon: 0, min_lat: 500, max_lon: 500, max_lat: 1000 };

    // View inside the NW quadrant hits the leaf, with the NW node bbox.
    let hits = r.query(0, &obcm::BBox { min_lon: 50, min_lat: 600, max_lon: 150, max_lat: 700 });
    assert_eq!(hits, vec![(0, nw)]);

    // View inside the (empty) SE quadrant hits nothing.
    let se = r.query(0, &obcm::BBox { min_lon: 600, min_lat: 100, max_lon: 700, max_lat: 200 });
    assert!(se.is_empty());

    // The feature's anchor is computed from the NW node's min corner (0,500):
    // ax=10, ay=10 → absolute (10, 510), then +(5,5).
    let feats = r.decode_chunk(0, 0, &nw);
    assert_eq!(feats[0].exterior, vec![(10, 510), (15, 515)]);
}

#[test]
fn empty_leaf_yields_nothing() {
    let empty = pad(vec![], CS); // all 0xFF
    let bytes = build_file(
        GLOBAL,
        STYLES,
        &[LodSpec {
            max_mpp: f32::INFINITY,
            index: vec![EMPTY_LEAF],
            chunks: vec![empty],
            chunk_size: CS,
        }],
    );
    let r = Reader::new(&bytes).unwrap();
    assert!(r.query(0, &r.bbox).is_empty());
}

#[test]
fn rejects_bad_input() {
    // Reader isn't Debug, so match the Err arm rather than using unwrap_err.
    let err = |b: &[u8]| match Reader::new(b) {
        Ok(_) => panic!("expected Err"),
        Err(e) => e,
    };

    assert_eq!(err(&[0u8; 10]), Error::TooShort);

    let mut bytes = two_lod_file();
    bytes[0] = b'X';
    assert_eq!(err(&bytes), Error::BadMagic);

    let mut bytes = two_lod_file();
    bytes[4] = 2; // version 2 no longer supported
    assert_eq!(err(&bytes), Error::BadVersion);
}
