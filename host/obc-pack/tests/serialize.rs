//! Byte-pinned serializer tests. Polygon rings are pre-closed (`first == last`),
//! matching the closed geometry that reaches `pack_feature`.

use obc_elevation::NullElevation;
use obc_pack::{
    pack_chunk, pack_feature, pack_style_dict, serialize_lods, serialize_tree, Feature, Kind, LodLayer, Node, Style,
};

fn line(style_id: u8, pts: &[(f64, f64)]) -> Feature {
    Feature { style_id, kind: Kind::Line, rings: vec![pts.to_vec()] }
}

#[test]
fn pack_style_dict_one_style() {
    let data = pack_style_dict(&[Style {
        id: 10,
        z_index: 50,
        color: 0xF9A6,
        weight: 4,
        priority: 2,
        dashed: false,
        color2: None,
        fixed_width: false,
        terrain_layer: false,
    }]);
    assert_eq!(data.len(), 9); // count(1) + 8-byte v10 record
    assert_eq!(data[0], 1); // count
    assert_eq!(data[1], 10); // id
    assert_eq!(data[2] as i8, 50); // z_index
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 0xF9A6); // color
    assert_eq!(data[5], 4); // weight
    assert_eq!(data[6], 1); // flags = priority(2) - 1, no dashed/color2 bits
    assert_eq!(u16::from_le_bytes([data[7], data[8]]), 0x0000); // color2 absent ⇒ 0x0000 on the wire
}

#[test]
fn pack_style_dict_line_style_and_color2() {
    // Dashed + a secondary color: flag bit 2 (dashed) and bit 3 (color2-present) set over the
    // priority bits, and the color2 u16 trails the record.
    let data = pack_style_dict(&[Style {
        id: 3,
        z_index: 0,
        color: 0x001F,
        weight: 2,
        priority: 3,
        dashed: true,
        color2: Some(0x8410),
        fixed_width: false,
        terrain_layer: false,
    }]);
    assert_eq!(data.len(), 9);
    assert_eq!(data[6], (3 - 1) | 0x04 | 0x08, "priority 3 + dashed bit 2 + color2 bit 3");
    assert_eq!(u16::from_le_bytes([data[7], data[8]]), 0x8410, "color2");

    // `color2 == Some(0x0000)` still sets bit 3 — black is a real secondary color, not a sentinel.
    let data = pack_style_dict(&[Style {
        id: 4,
        z_index: 0,
        color: 0x001F,
        weight: 1,
        priority: 1,
        dashed: false,
        color2: Some(0x0000),
        fixed_width: false,
        terrain_layer: false,
    }]);
    assert_eq!(data[6] & 0x08, 0x08, "color2-present bit set even for black");
    assert_eq!(u16::from_le_bytes([data[7], data[8]]), 0x0000);
}

/// #1095: fixed width is bit 4 and terrain layer is bit 5, each independent of the other and of the
/// priority/dashed/color2 bits below them. The record stays 8 bytes — both bits live in the byte
/// that was already there, which is why defining them cost no format bump.
#[test]
fn pack_style_dict_fixed_width_and_terrain_layer_bits() {
    let contour = |fixed_width, terrain_layer| Style {
        id: 7,
        z_index: 8,
        color: 0xAD55,
        weight: 1,
        priority: 4,
        dashed: true,
        color2: None,
        fixed_width,
        terrain_layer,
    };
    let flags = |s: Style| {
        let data = pack_style_dict(&[s]);
        assert_eq!(data.len(), 9, "the record is still 8 bytes behind its count");
        data[6]
    };
    assert_eq!(flags(contour(false, false)), (4 - 1) | 0x04, "priority 4 + dashed, neither new bit");
    assert_eq!(flags(contour(true, false)), (4 - 1) | 0x04 | 0x10, "bit 4 alone");
    assert_eq!(flags(contour(false, true)), (4 - 1) | 0x04 | 0x20, "bit 5 alone");
    assert_eq!(flags(contour(true, true)), (4 - 1) | 0x04 | 0x10 | 0x20, "the shipped E3 contour style");
    assert_eq!(flags(contour(true, true)) & 0xC0, 0, "bits 6-7 stay reserved, written 0");
}

#[test]
fn pack_feature_8bit_line() {
    // A 2-point line, anchor at the node min corner.
    let f = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let data = pack_feature(&f, node_bbox);

    // v11 compact header: 2 vertices and a zero anchor both fit the narrow fields.
    assert_eq!(data.len(), 9); // compact header(7) + one 8-bit delta pair(2)
    assert_eq!(data[0], 10); // style
    assert_eq!(data[1], 0); // flags: line, 8-bit, compact (WIDE clear)
    assert_eq!(data[2], 2); // pt count (u8)
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 0); // anchor x
    assert_eq!(u16::from_le_bytes([data[5], data[6]]), 0); // anchor y
    assert_eq!(data[7] as i8, 100); // dx = (1.0001 - 1.0) * 1e6
    assert_eq!(data[8] as i8, 100); // dy
}

#[test]
fn pack_chunk_is_tight_and_ends_in_one_sentinel() {
    // v11: no padding to `chunk_size`. The chunk is the packed features plus exactly one 0xFF.
    let f = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let (chunk, dropped) = pack_chunk(&[f], node_bbox, 32);
    assert_eq!(chunk.len(), 10, "9-byte feature + 1 sentinel, not padded to 32");
    assert_eq!(dropped, 0);
    assert_eq!(chunk[9], 0xFF, "the one trailing sentinel");
}

#[test]
fn pack_polygon_with_hole() {
    // Rings pre-closed: 4 distinct pts -> 5 stored.
    let ext = vec![(0.0, 0.0), (0.0001, 0.0), (0.0001, 0.0001), (0.0, 0.0001), (0.0, 0.0)];
    let hole = vec![(0.00002, 0.00002), (0.00008, 0.00002), (0.00008, 0.00008), (0.00002, 0.00008), (0.00002, 0.00002)];
    let f = Feature { style_id: 20, kind: Kind::Polygon, rings: vec![ext, hole] };
    let node_bbox = (0, 0, 200, 200);
    let data = pack_feature(&f, node_bbox);

    assert_eq!(data[0], 20); // style
    assert_eq!(data[1], 0x06); // flags: poly | has-holes, 8-bit, compact
    assert_eq!(data[2], 5); // exterior pt count (closed)
    assert_eq!(data[15], 1); // hole count (after the 7-byte header + 8 exterior delta bytes)
    assert_eq!(u16::from_le_bytes([data[16], data[17]]), 5); // hole pt count (still u16)
}

#[test]
fn serialize_lods_header_single_empty_leaf() {
    // One LOD, one empty leaf, no styles.
    let lods = vec![LodLayer {
        max_mpp: None,
        chunk_size: 2048,
        root: Node::Leaf { bbox: (0, 0, 100, 100), features: vec![] },
    }];
    let (bin, dropped) = serialize_lods(
        &lods,
        &[],
        0xF800,
        (0, 0, 100, 100),
        &[],
        &Default::default(),
        &obc_pack::config::default_profiles(),
        &mut NullElevation,
    );
    assert_eq!(dropped, 0);

    // header(40) + style count(1) + 1 LOD entry(18) + index(4) + the chunkless LOD's one-entry
    // offset table(4) = 67, then the empty POI directory
    // — count(1) + chunk_size(2) + 6 entries × 13 + the two v7 pool fields (offset u32 + count u16 =
    // 6) = 87 bytes — the empty hours pool (a bare `count u16` = 2 bytes), and the empty nav
    // section (28-byte directory + the always-present profile table) at the tail.
    let poi_dir_len = 1 + 2 + 6 * 13 + 6;
    let hours_pool_len = 2; // an empty pool is just its count
                            // Empty graph: the 28-byte directory + the four default profiles (56 B
                            // each in v12), always present.
    let profile_table_len = 4 * obc_formats::obcm::NAV_PROFILE_LEN;
    let nav_section_len = 28 + profile_table_len;
    assert_eq!(bin.len(), 67 + poi_dir_len + hours_pool_len + nav_section_len);
    assert_eq!(&bin[0..4], b"OBCM");
    assert_eq!(bin[4], obc_formats::obcm::VERSION); // version
    assert_eq!(u32::from_le_bytes([bin[21], bin[22], bin[23], bin[24]]), 40); // style offset (40 since v8)
    assert_eq!(bin[25], 1); // lod count
    let lod_tbl = u32::from_le_bytes([bin[26], bin[27], bin[28], bin[29]]) as usize;
    assert_eq!(lod_tbl, 41); // 40 header + 1 style-count byte

    // The POI section offset (header byte 32) points just past the LOD payload: the
    // section is 67 bytes in (header 40 + style 1 + LOD entry 18 + index 4 + offset table 4).
    let poi_off = u32::from_le_bytes([bin[32], bin[33], bin[34], bin[35]]) as usize;
    assert_eq!(poi_off, 67);
    assert_eq!(bin[poi_off], 6, "empty POI directory still declares 6 categories");
    assert_eq!(u16::from_le_bytes([bin[poi_off + 1], bin[poi_off + 2]]), 512); // shared chunk_size

    // The v7 hours-pool fields trail the six 13-byte entries: offset u32 + count u16. Count is 0 (no
    // hours), and the pool region (its bare `count u16`) begins right after the directory.
    let pool_fields_off = poi_off + 3 + 6 * 13;
    let hours_pool_off = u32::from_le_bytes(bin[pool_fields_off..pool_fields_off + 4].try_into().unwrap()) as usize;
    let hours_pool_count = u16::from_le_bytes(bin[pool_fields_off + 4..pool_fields_off + 6].try_into().unwrap());
    assert_eq!(hours_pool_count, 0, "no hours in this map");
    assert_eq!(hours_pool_off, poi_off + poi_dir_len, "pool follows the directory (no categories)");
    assert_eq!(u16::from_le_bytes(bin[hours_pool_off..hours_pool_off + 2].try_into().unwrap()), 0, "empty pool count");

    // The nav section offset (header byte 36) points just past the hours pool; an empty graph is the
    // 28-byte directory followed by the always-present profile table — zero index nodes / chunks /
    // edges, chunk_size pinned to 512, profile_count 4, profile table right after the directory.
    let nav_off = u32::from_le_bytes(bin[36..40].try_into().unwrap()) as usize;
    assert_eq!(nav_off, hours_pool_off + hours_pool_len, "nav section at the file tail");
    assert_eq!(u32::from_le_bytes(bin[nav_off + 4..nav_off + 8].try_into().unwrap()), 0, "index_node_count 0");
    assert_eq!(u32::from_le_bytes(bin[nav_off + 8..nav_off + 12].try_into().unwrap()), 0, "node_chunk_count 0");
    assert_eq!(u32::from_le_bytes(bin[nav_off + 16..nav_off + 20].try_into().unwrap()), 0, "edge_chunk_count 0");
    assert_eq!(u16::from_le_bytes(bin[nav_off + 20..nav_off + 22].try_into().unwrap()), 512, "nav chunk_size pinned");
    assert_eq!(
        u32::from_le_bytes(bin[nav_off + 22..nav_off + 26].try_into().unwrap()) as usize,
        nav_off + 28,
        "profile table sits immediately after the 28-byte directory"
    );
    assert_eq!(bin[nav_off + 26], 4, "profile_count = the 4 default profiles");
    assert_eq!(bin[nav_off + 27], 0, "reserved byte is 0");
    // The empty nav section is exactly the directory + the 4-profile table.
    assert_eq!(nav_off + nav_section_len, bin.len(), "empty nav section is dir + profile table");

    let mpp = f32::from_le_bytes([bin[lod_tbl], bin[lod_tbl + 1], bin[lod_tbl + 2], bin[lod_tbl + 3]]);
    assert!(mpp.is_infinite()); // coarsest layer
    let idx_off = u32::from_le_bytes([bin[lod_tbl + 4], bin[lod_tbl + 5], bin[lod_tbl + 6], bin[lod_tbl + 7]]);
    let node_count = u32::from_le_bytes([bin[lod_tbl + 8], bin[lod_tbl + 9], bin[lod_tbl + 10], bin[lod_tbl + 11]]);
    let c_size = u16::from_le_bytes([bin[lod_tbl + 12], bin[lod_tbl + 13]]);
    let chunk_count = u32::from_le_bytes([bin[lod_tbl + 14], bin[lod_tbl + 15], bin[lod_tbl + 16], bin[lod_tbl + 17]]);
    assert_eq!(idx_off as usize, lod_tbl + 18);
    assert_eq!(node_count, 1);
    assert_eq!(c_size, 2048);
    assert_eq!(chunk_count, 0);
    // A chunkless LOD still writes its offset table: the single `0` entry, and nothing after it.
    let table_off = idx_off as usize + node_count as usize * 4;
    assert_eq!(u32::from_le_bytes(bin[table_off..table_off + 4].try_into().unwrap()), 0);
    assert_eq!(table_off + 4, poi_off, "table of one entry, then the POI section");
}

// === 16-bit delta path — byte-pinned ========================================

#[test]
fn pack_feature_16bit_line() {
    // Deltas of 500 µdeg exceed int8 (>127), so the feature flips to the 16-bit
    // path: flags 0x01, and each delta is an int16 LE pair.
    let f = line(10, &[(1.0, 1.0), (1.0005, 1.0005)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let data = pack_feature(&f, node_bbox);

    // compact header(7) + one int16 delta pair(4) = 11.
    assert_eq!(data.len(), 11, "7-byte compact header + one int16 (dx,dy) pair");
    assert_eq!(data[0], 10); // style
    assert_eq!(data[1], 0x01, "flags: line, 16-bit deltas, compact");
    assert_eq!(data[2], 2); // exterior pt count
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 0); // anchor x (at node min)
    assert_eq!(u16::from_le_bytes([data[5], data[6]]), 0); // anchor y
                                                           // dx = dy = 500 µdeg, little-endian int16.
    assert_eq!(i16::from_le_bytes([data[7], data[8]]), 500, "dx as int16");
    assert_eq!(i16::from_le_bytes([data[9], data[10]]), 500, "dy as int16");
}

#[test]
fn pack_feature_16bit_negative_delta() {
    // A negative delta past -128 must encode as a two's-complement int16 LE, not be
    // clamped or wrapped to a byte. -500 = 0xFE0C LE.
    let f = line(7, &[(2.0, 2.0), (1.9995, 1.9995)]);
    let node_bbox = (1_999_500, 1_999_500, 2_001_000, 2_001_000);
    let data = pack_feature(&f, node_bbox);
    assert_eq!(data[1], 0x01, "16-bit flag set for a -500 µdeg delta, header still compact");
    // Anchor is the first vertex (2.0,2.0) relative to the node min corner.
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 500, "= 2_000_000 − 1_999_500");
    assert_eq!(i16::from_le_bytes([data[7], data[8]]), -500, "dx = -500 as signed int16");
    assert_eq!(i16::from_le_bytes([data[9], data[10]]), -500, "dy = -500 as signed int16");
}

// === Densify byte-pinning inside pack_feature ===============================

#[test]
fn pack_feature_densifies_long_segment_bytes() {
    // A 55 000-µdeg vertical segment exceeds MAX_SEGMENT (30 000): densify inserts
    // exactly one midpoint (steps = 55000/30000 + 1 = 2), so the stored exterior is
    // 3 points, not 2, and there are two int16 delta pairs of +27 500 each.
    let f = line(3, &[(1.0, 1.0), (1.0, 1.055)]);
    let node_bbox = (1_000_000, 1_000_000, 1_100_000, 1_100_000);
    let data = pack_feature(&f, node_bbox);

    assert_eq!(data[2], 3, "densify bumped exterior Pt Count 2 → 3");
    assert_eq!(data[1], 0x01, "27 500-µdeg deltas force the 16-bit path");
    // compact header(7) + two int16 (dx,dy) pairs(8) = 15.
    assert_eq!(data.len(), 15, "header + two densified int16 delta pairs");
    // Both deltas are (0, +27500): the inserted midpoint and the real endpoint.
    assert_eq!(i16::from_le_bytes([data[7], data[8]]), 0); // dx to midpoint
    assert_eq!(i16::from_le_bytes([data[9], data[10]]), 27_500); // dy to midpoint
    assert_eq!(i16::from_le_bytes([data[11], data[12]]), 0); // dx to endpoint
    assert_eq!(i16::from_le_bytes([data[13], data[14]]), 27_500); // dy to endpoint
}

// === Numeric extremes =======================================================
// `pack_feature` casts the anchor `as i32` and deltas `as i16`. A continent-
// spanning anchor must survive the i32 cast, and the anchor-relative deltas (kept
// small by the node frame) stay correct even for huge absolute coordinates.

#[test]
fn pack_feature_extreme_anchor_survives_i32() {
    // Anchor near +180° lon (180e6 µdeg) — comfortably inside i32 (max ~2.147e9) but
    // far past any earlier test. The node min corner is the anchor, so the stored
    // anchor is the offset of the first vertex from it.
    let lon = 179.999_000; // 179_999_000 µdeg
    let lat = 85.000_000; //  85_000_000 µdeg
    let f = line(5, &[(lon, lat), (lon + 0.0001, lat + 0.0001)]);
    // Node min slightly below the anchor so the stored anchor is a small positive.
    let node_bbox = (179_998_000, 84_999_000, 180_000_000, 85_001_000);
    let data = pack_feature(&f, node_bbox);

    // Anchor = first vertex (179_999_000, 85_000_000) − node min (179_998_000, 84_999_000). Extreme
    // *absolute* coords, but the leaf-relative offset is small — so the header stays compact.
    assert_eq!(data[1], 0x00, "small deltas ⇒ 8-bit; small offset ⇒ compact, even at 180° lon");
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 1_000, "anchor x offset");
    assert_eq!(u16::from_le_bytes([data[5], data[6]]), 1_000, "anchor y offset");
    assert_eq!(data[7] as i8, 100);
    assert_eq!(data[8] as i8, 100);
}

#[test]
fn pack_feature_antimeridian_negative_anchor() {
    // A vertex just west of the antimeridian: lon ≈ -179.999° (anchor near the i32
    // negative extreme when measured from a node at -180°). Confirms the `as i32`
    // anchor cast handles large-magnitude negatives, and a wide (continent-spanning)
    // node frame still yields a correct small anchor offset.
    let f = line(5, &[(-179.999, -85.0), (-179.9989, -84.9999)]);
    let node_bbox = (-180_000_000, -85_001_000, -179_000_000, -84_000_000);
    let data = pack_feature(&f, node_bbox);
    // Anchor = (-179_999_000) − (-180_000_000) = 1_000; (-85_000_000) − (-85_001_000) = 1_000.
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 1_000);
    assert_eq!(u16::from_le_bytes([data[5], data[6]]), 1_000);
    assert_eq!(data[7] as i8, 100, "dx = +100 µdeg");
    assert_eq!(data[8] as i8, 100, "dy = +100 µdeg");
}

// === Quadtree budget vs real packed bytes ===================================
// The quadtree splits on `geom::packed_size_budget`; if that ever under-counts
// what `pack_feature` really emits, a leaf survives splitting and `pack_chunk`
// silently drops the overflow. Pin `budget >= packed.len()` for the cases that
// used to be under-counted: densified long segments, densified anchor→hole
// jumps, and hole bookkeeping bytes.

#[test]
fn budget_covers_packed_bytes_for_densify_and_holes() {
    use obc_pack::geom::{packed_size_budget, Geom};
    let node_bbox = (0, 0, 10_000_000, 10_000_000);

    let check = |name: &str, geom: &Geom, feature: &Feature| {
        let budget = packed_size_budget(geom);
        let packed = pack_feature(feature, node_bbox).len();
        assert!(budget >= packed, "{name}: budget {budget} must cover packed {packed} bytes");
    };

    // A 2-point line spanning 3° of longitude densifies to ~100 midpoints.
    let long = vec![(0.1, 0.5), (3.1, 0.5)];
    check("densified line", &Geom::Line(long.clone()), &Feature { style_id: 1, kind: Kind::Line, rings: vec![long] });

    // A polygon with two holes, each ~1° from the anchor: the anchor→hole jumps
    // densify too, and the hole count/pt_count bookkeeping bytes must be counted.
    let ext = vec![(0.0, 0.0), (2.0, 0.0), (2.0, 2.0), (0.0, 2.0), (0.0, 0.0)];
    let h1 = vec![(1.0, 1.0), (1.01, 1.0), (1.01, 1.01), (1.0, 1.01), (1.0, 1.0)];
    let h2 = vec![(1.5, 1.5), (1.51, 1.5), (1.51, 1.51), (1.5, 1.51), (1.5, 1.5)];
    check(
        "polygon with far holes",
        &Geom::Polygon { exterior: ext.clone(), interiors: vec![h1.clone(), h2.clone()] },
        &Feature { style_id: 2, kind: Kind::Polygon, rings: vec![ext, h1, h2] },
    );

    // A small 8-bit line: the budget may be loose (16-bit worst case) but never under.
    let small = vec![(1.0, 1.0), (1.0001, 1.0001), (1.0002, 1.0)];
    check(
        "small 8-bit line",
        &Geom::Line(small.clone()),
        &Feature { style_id: 3, kind: Kind::Line, rings: vec![small] },
    );
}

// === Chunk-size overflow drop ===============================================
// `pack_chunk` drops a feature (and every feature after it) that would overflow
// the chunk; pin that the padding/contents stay consistent.

#[test]
fn pack_chunk_drops_overflowing_feature_and_the_rest() {
    // Two small features that each fit, then a wide one that doesn't. Size a chunk to
    // hold the first feature but not the first + second, so the second (and the third
    // after it) are dropped.
    let a = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]); // 9 bytes (compact, 1 8-bit delta pair)
    let b = line(11, &[(1.0, 1.0), (1.0001, 1.0001)]); // also 9 bytes
    let c = line(12, &[(1.0, 1.0), (1.0001, 1.0001)]); // also 9 bytes
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);

    // Chunk of 12: holds `a` + its sentinel (10) but not `a`+`b` (19) ⇒ b and c dropped.
    let (chunk, dropped) = pack_chunk(&[a, b, c], node_bbox, 12);
    assert_eq!(chunk.len(), 10, "the kept feature (9) + its sentinel — tight, not 12");
    assert_eq!(dropped, 2, "b and c are reported as dropped, not lost silently");
    // The chunk is feature `a` (style 10) and nothing else: no partial second feature, and `c` is
    // gone too (break, not continue).
    assert_eq!(chunk[0], 10, "the one feature that fit is `a`");
    assert_eq!(chunk[9], 0xFF, "the sentinel closes the stream right after `a`");
    // Specifically, style ids 11 and 12 never appear.
    assert!(!chunk.contains(&11) && !chunk.contains(&12), "overflowing features b/c were dropped, not packed");
}

#[test]
fn serialize_keeps_chunk_index_consistent_when_a_feature_overflows() {
    // End-to-end: a single leaf whose features overflow the chunk must still produce
    // one chunk and a populated (non-sentinel) index entry — the drop happens inside
    // the chunk, the leaf's chunk-id/index stays valid. Two 14-byte features in a
    // 20-byte chunk ⇒ the second is dropped, but the leaf is still chunk 0.
    let a = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let b = line(11, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let lods = vec![LodLayer {
        max_mpp: None,
        chunk_size: 12,
        root: Node::Leaf { bbox: (1_000_000, 1_000_000, 1_010_000, 1_010_000), features: vec![a, b] },
    }];
    let (bin, dropped) = serialize_lods(
        &lods,
        &[],
        0xF800,
        (1_000_000, 1_000_000, 1_010_000, 1_010_000),
        &[],
        &Default::default(),
        &obc_pack::config::default_profiles(),
        &mut NullElevation,
    );
    assert_eq!(dropped, 1, "the overflowing feature is reported dropped");

    // Locate the LOD table and read its node/chunk counts + index offset.
    let lod_tbl = u32::from_le_bytes([bin[26], bin[27], bin[28], bin[29]]) as usize;
    let idx_off = u32::from_le_bytes([bin[lod_tbl + 4], bin[lod_tbl + 5], bin[lod_tbl + 6], bin[lod_tbl + 7]]) as usize;
    let node_count = u32::from_le_bytes([bin[lod_tbl + 8], bin[lod_tbl + 9], bin[lod_tbl + 10], bin[lod_tbl + 11]]);
    let chunk_count = u32::from_le_bytes([bin[lod_tbl + 14], bin[lod_tbl + 15], bin[lod_tbl + 16], bin[lod_tbl + 17]]);
    assert_eq!(node_count, 1, "single leaf ⇒ one node");
    assert_eq!(chunk_count, 1, "the leaf still produces exactly one chunk despite the dropped feature");

    // The leaf's index entry is chunk 0 (high bit clear ⇒ a leaf, not a branch).
    let entry = u32::from_le_bytes([bin[idx_off], bin[idx_off + 1], bin[idx_off + 2], bin[idx_off + 3]]);
    assert_eq!(entry & obc_formats::obcm::BRANCH_BIT, 0, "index entry is a leaf, not a branch");
    assert_eq!(entry & !obc_formats::obcm::BRANCH_BIT, 0, "leaf maps to chunk id 0");

    // The single chunk holds only feature `a` (style 10); style 11 was dropped. Chunk data starts
    // after the index *and* the (chunk_count + 1)-entry offset table, and its extent is the table's
    // first two entries.
    let table_off = idx_off + node_count as usize * 4;
    let chunk_off = table_off + (chunk_count as usize + 1) * 4;
    let end = u32::from_le_bytes(bin[table_off + 4..table_off + 8].try_into().unwrap()) as usize;
    assert_eq!(u32::from_le_bytes(bin[table_off..table_off + 4].try_into().unwrap()), 0, "offsets[0] is 0");
    assert_eq!(end, 10, "the tight chunk is the 9-byte feature plus its sentinel");
    assert_eq!(bin[chunk_off], 10, "chunk starts with the kept feature");
    assert!(!bin[chunk_off..chunk_off + end].contains(&11), "the overflowing feature is absent from the chunk");
}

// === v11 chunk offset table (§5, issue #1009) ================================

/// `serialize_tree`'s data region: a `chunk_count + 1` entry `uint32` table, then the tight chunks.
/// Every property the reader's arithmetic depends on is pinned here — zero-based, monotonic, the last
/// entry the region total, and the slice each pair delimits actually being that chunk's bytes.
#[test]
fn serialize_tree_writes_a_monotonic_offset_table() {
    // Two leaves under a branch: NW and NE hold one feature each, SW/SE are empty.
    let leaf = |lon: f64, style: u8, bbox| Node::Leaf {
        bbox,
        features: vec![line(style, &[(lon, 0.1), (lon + 0.0001, 0.1)])],
    };
    let empty = |bbox| Node::Leaf { bbox, features: vec![] };
    let root = Node::Branch(Box::new([
        leaf(0.1, 10, (0, 500_000, 500_000, 1_000_000)),
        leaf(0.6, 11, (500_000, 500_000, 1_000_000, 1_000_000)),
        empty((0, 0, 500_000, 500_000)),
        empty((500_000, 0, 1_000_000, 500_000)),
    ]));
    let (index, node_count, data, chunk_count, dropped) = serialize_tree(&root, 4096);
    assert_eq!((node_count, chunk_count, dropped), (5, 2, 0));
    assert_eq!(index.len(), 5 * 4);

    let table_len = (chunk_count as usize + 1) * 4;
    let offsets: Vec<u32> =
        (0..=chunk_count as usize).map(|k| u32::from_le_bytes(data[k * 4..k * 4 + 4].try_into().unwrap())).collect();
    assert_eq!(offsets[0], 0, "offsets are relative to the first chunk byte");
    assert!(offsets.windows(2).all(|w| w[0] <= w[1]), "monotonic: {offsets:?}");
    assert_eq!(offsets[chunk_count as usize] as usize, data.len() - table_len, "last entry is the region total");

    // Each pair delimits exactly that chunk: the style byte it starts with, and its one sentinel.
    for (k, style) in [(0usize, 10u8), (1, 11)] {
        let chunk = &data[table_len + offsets[k] as usize..table_len + offsets[k + 1] as usize];
        assert_eq!(chunk[0], style, "chunk {k} starts with its feature");
        assert_eq!(*chunk.last().unwrap(), 0xFF, "and ends on exactly one sentinel");
        assert!(!chunk[..chunk.len() - 1].ends_with(&[0xFF]), "no padding before it");
    }
}

/// The chunkless tree: one empty leaf, no chunks, and the table is still written as the single `0`
/// entry (the reader reads that entry unconditionally as its region bound).
#[test]
fn serialize_tree_writes_the_table_even_with_no_chunks() {
    let root = Node::Leaf { bbox: (0, 0, 1_000_000, 1_000_000), features: vec![] };
    let (_, node_count, data, chunk_count, dropped) = serialize_tree(&root, 4096);
    assert_eq!((node_count, chunk_count, dropped), (1, 0, 0));
    assert_eq!(data, vec![0, 0, 0, 0], "one zero entry, nothing after it");
}

// === v11 compact-vs-wide header selection (§5, issue #1009) ==================

/// Read a packed feature's `(wide, ext_pt_count, ax, ay)` the way the reader does.
fn header_of(packed: &[u8]) -> (bool, usize, i32, i32) {
    let wide = packed[1] & obc_formats::obcm::FEATURE_FLAG_WIDE != 0;
    if wide {
        (
            true,
            u16::from_le_bytes([packed[2], packed[3]]) as usize,
            i32::from_le_bytes(packed[4..8].try_into().unwrap()),
            i32::from_le_bytes(packed[8..12].try_into().unwrap()),
        )
    } else {
        (
            false,
            packed[2] as usize,
            u16::from_le_bytes([packed[3], packed[4]]) as i32,
            u16::from_le_bytes([packed[5], packed[6]]) as i32,
        )
    }
}

/// The `pt_count` boundary: 255 vertices still fit the compact `u8`, 256 do not. Both are 8-bit-delta
/// lines anchored at the leaf min corner, so nothing but the count moves the decision.
#[test]
fn compact_header_holds_up_to_255_vertices() {
    let node_bbox = (0, 0, 1_000_000, 1_000_000);
    let ramp = |n: usize| line(1, &(0..n).map(|i| (i as f64 * 1e-6, 0.0)).collect::<Vec<_>>());

    let packed = pack_feature(&ramp(255), node_bbox);
    assert_eq!(header_of(&packed), (false, 255, 0, 0), "255 is the last compact count");
    assert_eq!(packed.len(), 7 + 254 * 2);

    let packed = pack_feature(&ramp(256), node_bbox);
    assert_eq!(header_of(&packed), (true, 256, 0, 0), "256 escapes to the wide header");
    assert_eq!(packed.len(), 12 + 255 * 2);
}

/// The anchor boundary: `65535` is the last compact anchor, `65536` escapes. Independently on each
/// axis, since either one out of range forces the wide form.
#[test]
fn compact_header_holds_anchors_up_to_65535() {
    // The anchor is the first vertex minus the leaf's min corner, so a leaf at the origin makes the
    // anchor the coordinate itself.
    let node_bbox = (0, 0, 1_000_000, 1_000_000);
    let at = |lon_udeg: i64, lat_udeg: i64| {
        let (lon, lat) = (lon_udeg as f64 * 1e-6, lat_udeg as f64 * 1e-6);
        line(1, &[(lon, lat), (lon + 1e-6, lat)])
    };

    assert_eq!(header_of(&pack_feature(&at(65_535, 65_535), node_bbox)), (false, 2, 65_535, 65_535));
    assert_eq!(header_of(&pack_feature(&at(65_536, 0), node_bbox)), (true, 2, 65_536, 0), "x one past the u16");
    assert_eq!(header_of(&pack_feature(&at(0, 65_536), node_bbox)), (true, 2, 0, 65_536), "y one past the u16");
}

/// A negative anchor — the leaf's min corner *above* the feature's first vertex. The packer should
/// never emit one (clipping keeps a feature inside its leaf), but the encoding must not silently
/// wrap it into a positive `u16`, so it takes the wide escape and round-trips as the negative it is.
#[test]
fn a_negative_anchor_takes_the_wide_escape() {
    let node_bbox = (1_000, 1_000, 1_000_000, 1_000_000);
    let f = line(1, &[(0.0, 0.0), (0.000_001, 0.0)]); // first vertex below the leaf min corner
    let packed = pack_feature(&f, node_bbox);
    assert_eq!(header_of(&packed), (true, 2, -1_000, -1_000));
}

/// End-to-end: a **coarse leaf spanning more than 65 535 µdeg** — the case the escape exists for.
/// Features near the leaf's min corner stay compact; one far enough in to pass the `u16` needs the
/// wide header, and the reader must hand back the same absolute coordinates for both.
#[test]
fn a_leaf_wider_than_the_u16_anchor_round_trips_through_both_forms() {
    use obc_reader::{MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

    // A single leaf spanning 1° (1 000 000 µdeg) — 15× the u16 anchor range.
    const BBOX: (i64, i64, i64, i64) = (0, 0, 1_000_000, 1_000_000);
    let near = line(1, &[(0.01, 0.01), (0.010_001, 0.01)]); // anchor 10 000 → compact
    let far = line(1, &[(0.9, 0.9), (0.900_001, 0.9)]); // anchor 900 000 → wide
    let lods =
        vec![LodLayer { max_mpp: None, chunk_size: 4096, root: Node::Leaf { bbox: BBOX, features: vec![near, far] } }];
    let styles = vec![Style {
        id: 1,
        z_index: 0,
        color: 0x1234,
        weight: 1,
        priority: 1,
        dashed: false,
        color2: None,
        fixed_width: false,
        terrain_layer: false,
    }];
    let (bin, dropped) = serialize_lods(
        &lods,
        &styles,
        0xF800,
        BBOX,
        &[],
        &Default::default(),
        &obc_pack::config::default_profiles(),
        &mut NullElevation,
    );
    assert_eq!(dropped, 0);

    let cache = MapCache::new();
    let src = SliceSource(&bin);
    let tables = MapTables::parse(&src).expect("v11 map parses");
    let r = Reader::new(&src, &tables, &cache);
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    let mut got: Vec<Vec<(i32, i32)>> = Vec::new();
    r.for_each_feature(0, 0, &r.bbox, &mut points, &mut ring_lens, |f| got.push(f.exterior().to_vec())).unwrap();
    assert_eq!(
        got,
        vec![vec![(10_000, 10_000), (10_001, 10_000)], vec![(900_000, 900_000), (900_001, 900_000)]],
        "both header forms decode to their absolute microdegrees"
    );
}
