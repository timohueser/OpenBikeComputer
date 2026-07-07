//! Byte-pinned serializer tests. Polygon rings are pre-closed (`first == last`),
//! matching the closed geometry that reaches `pack_feature`.

use obc_pack::{pack_chunk, pack_feature, pack_style_dict, serialize_lods, Feature, Kind, LodLayer, Node, Style};

fn line(style_id: u8, pts: &[(f64, f64)]) -> Feature {
    Feature { style_id, kind: Kind::Line, rings: vec![pts.to_vec()] }
}

#[test]
fn pack_style_dict_one_style() {
    let data = pack_style_dict(&[Style { id: 10, z_index: 50, color: 0xF9A6, weight: 4, priority: 2 }]);
    assert_eq!(data.len(), 7); // count(1) + 6-byte record
    assert_eq!(data[0], 1); // count
    assert_eq!(data[1], 10); // id
    assert_eq!(data[2] as i8, 50); // z_index
    assert_eq!(u16::from_le_bytes([data[3], data[4]]), 0xF9A6); // color
    assert_eq!(data[5], 4); // weight
    assert_eq!(data[6], 1); // flags = priority(2) - 1
}

#[test]
fn pack_feature_8bit_line() {
    // A 2-point line, anchor at the node min corner.
    let f = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let data = pack_feature(&f, node_bbox);

    assert_eq!(data.len(), 14); // header(12) + one 8-bit delta pair(2)
    assert_eq!(data[0], 10); // style
    assert_eq!(u16::from_le_bytes([data[1], data[2]]), 2); // pt count
    assert_eq!(i32::from_le_bytes([data[3], data[4], data[5], data[6]]), 0); // anchor x
    assert_eq!(i32::from_le_bytes([data[7], data[8], data[9], data[10]]), 0); // anchor y
    assert_eq!(data[11], 0); // flags: line, 8-bit
    assert_eq!(data[12] as i8, 100); // dx = (1.0001 - 1.0) * 1e6
    assert_eq!(data[13] as i8, 100); // dy
}

#[test]
fn pack_chunk_pads_with_ff() {
    let f = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let (chunk, dropped) = pack_chunk(&[f], node_bbox, 32);
    assert_eq!(chunk.len(), 32);
    assert_eq!(dropped, 0);
    assert!(chunk[14..].iter().all(|&b| b == 0xFF)); // 18 bytes of padding
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
    assert_eq!(u16::from_le_bytes([data[1], data[2]]), 5); // exterior pt count (closed)
    assert_eq!(data[11], 0x06); // poly | has-holes, 8-bit
    assert_eq!(data[20], 1); // hole count (after 12 header + 8 exterior delta bytes)
    assert_eq!(u16::from_le_bytes([data[21], data[22]]), 5); // hole pt count
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
    );
    assert_eq!(dropped, 0);

    // v9 header(40) + style count(1) + 1 LOD entry(18) + index(4) = 63, then the empty POI directory
    // — count(1) + chunk_size(2) + 6 entries × 13 + the two v7 pool fields (offset u32 + count u16 =
    // 6) = 87 bytes — the empty hours pool (a bare `count u16` = 2 bytes), and the empty v9 nav
    // section (28-byte directory + the always-present profile table) at the tail.
    let poi_dir_len = 1 + 2 + 6 * 13 + 6;
    let hours_pool_len = 2; // an empty pool is just its count
                            // Empty graph: the 28-byte directory + the four default profiles (52 B each), always present.
    let profile_table_len = 4 * 52;
    let nav_section_len = 28 + profile_table_len;
    assert_eq!(bin.len(), 63 + poi_dir_len + hours_pool_len + nav_section_len);
    assert_eq!(&bin[0..4], b"OBCM");
    assert_eq!(bin[4], 9); // version
    assert_eq!(u32::from_le_bytes([bin[21], bin[22], bin[23], bin[24]]), 40); // style offset (40 since v8)
    assert_eq!(bin[25], 1); // lod count
    let lod_tbl = u32::from_le_bytes([bin[26], bin[27], bin[28], bin[29]]) as usize;
    assert_eq!(lod_tbl, 41); // 40 header + 1 style-count byte

    // The POI section offset (header byte 32) points just past the LOD payload: the
    // section is 63 bytes in (header 40 + style 1 + LOD entry 18 + index 4).
    let poi_off = u32::from_le_bytes([bin[32], bin[33], bin[34], bin[35]]) as usize;
    assert_eq!(poi_off, 63);
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
}

// === 16-bit delta path — byte-pinned ========================================

#[test]
fn pack_feature_16bit_line() {
    // Deltas of 500 µdeg exceed int8 (>127), so the feature flips to the 16-bit
    // path: flags 0x01, and each delta is an int16 LE pair.
    let f = line(10, &[(1.0, 1.0), (1.0005, 1.0005)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let data = pack_feature(&f, node_bbox);

    // header(12) + one int16 delta pair(4) = 16.
    assert_eq!(data.len(), 16, "12-byte header + one int16 (dx,dy) pair");
    assert_eq!(data[0], 10); // style
    assert_eq!(u16::from_le_bytes([data[1], data[2]]), 2); // exterior pt count
    assert_eq!(i32::from_le_bytes([data[3], data[4], data[5], data[6]]), 0); // anchor x (at node min)
    assert_eq!(i32::from_le_bytes([data[7], data[8], data[9], data[10]]), 0); // anchor y
    assert_eq!(data[11], 0x01, "flags: line, 16-bit deltas");
    // dx = dy = 500 µdeg, little-endian int16.
    assert_eq!(i16::from_le_bytes([data[12], data[13]]), 500, "dx as int16");
    assert_eq!(i16::from_le_bytes([data[14], data[15]]), 500, "dy as int16");
}

#[test]
fn pack_feature_16bit_negative_delta() {
    // A negative delta past -128 must encode as a two's-complement int16 LE, not be
    // clamped or wrapped to a byte. -500 = 0xFE0C LE.
    let f = line(7, &[(2.0, 2.0), (1.9995, 1.9995)]);
    let node_bbox = (1_999_500, 1_999_500, 2_001_000, 2_001_000);
    let data = pack_feature(&f, node_bbox);
    assert_eq!(data[11], 0x01, "16-bit flag set for a -500 µdeg delta");
    // Anchor is the first vertex (2.0,2.0) relative to the node min corner.
    assert_eq!(i32::from_le_bytes([data[3], data[4], data[5], data[6]]), 2_000_000 - 1_999_500); // = 500
    assert_eq!(i16::from_le_bytes([data[12], data[13]]), -500, "dx = -500 as signed int16");
    assert_eq!(i16::from_le_bytes([data[14], data[15]]), -500, "dy = -500 as signed int16");
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

    assert_eq!(u16::from_le_bytes([data[1], data[2]]), 3, "densify bumped exterior Pt Count 2 → 3");
    assert_eq!(data[11], 0x01, "27 500-µdeg deltas force the 16-bit path");
    // header(12) + two int16 (dx,dy) pairs(8) = 20.
    assert_eq!(data.len(), 20, "header + two densified int16 delta pairs");
    // Both deltas are (0, +27500): the inserted midpoint and the real endpoint.
    assert_eq!(i16::from_le_bytes([data[12], data[13]]), 0); // dx to midpoint
    assert_eq!(i16::from_le_bytes([data[14], data[15]]), 27_500); // dy to midpoint
    assert_eq!(i16::from_le_bytes([data[16], data[17]]), 0); // dx to endpoint
    assert_eq!(i16::from_le_bytes([data[18], data[19]]), 27_500); // dy to endpoint
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

    // Anchor = first vertex (179_999_000, 85_000_000) − node min (179_998_000, 84_999_000).
    assert_eq!(i32::from_le_bytes([data[3], data[4], data[5], data[6]]), 1_000, "anchor x offset survives i32");
    assert_eq!(i32::from_le_bytes([data[7], data[8], data[9], data[10]]), 1_000, "anchor y offset survives i32");
    // Small 100-µdeg step stays on the 8-bit path even at extreme absolute coords.
    assert_eq!(data[11], 0x00, "small deltas ⇒ 8-bit even with a huge anchor");
    assert_eq!(data[12] as i8, 100);
    assert_eq!(data[13] as i8, 100);
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
    assert_eq!(i32::from_le_bytes([data[3], data[4], data[5], data[6]]), 1_000);
    assert_eq!(i32::from_le_bytes([data[7], data[8], data[9], data[10]]), 1_000);
    assert_eq!(data[12] as i8, 100, "dx = +100 µdeg");
    assert_eq!(data[13] as i8, 100, "dy = +100 µdeg");
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
    let a = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]); // 14 bytes (8-bit, 1 delta pair)
    let b = line(11, &[(1.0, 1.0), (1.0001, 1.0001)]); // also 14 bytes
    let c = line(12, &[(1.0, 1.0), (1.0001, 1.0001)]); // also 14 bytes
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);

    // Chunk of 20: holds `a` (14) but not `a`+`b` (28) ⇒ b and c dropped.
    let (chunk, dropped) = pack_chunk(&[a, b, c], node_bbox, 20);
    assert_eq!(chunk.len(), 20, "chunk is exactly chunk_size");
    assert_eq!(dropped, 2, "b and c are reported as dropped, not lost silently");
    // First 14 bytes are feature `a` (style 10); the rest is 0xFF padding — no
    // partial second feature, and `c` is gone too (break, not continue).
    assert_eq!(chunk[0], 10, "the one feature that fit is `a`");
    assert!(
        chunk[14..].iter().all(|&byte| byte == 0xFF),
        "everything after the kept feature is padding, no partial b/c"
    );
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
        chunk_size: 20,
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
    assert_eq!(entry & obc_reader::format::BRANCH_BIT, 0, "index entry is a leaf, not a branch");
    assert_eq!(entry & !obc_reader::format::BRANCH_BIT, 0, "leaf maps to chunk id 0");

    // The single chunk holds only feature `a` (style 10); style 11 was dropped.
    let chunk_off = idx_off + node_count as usize * 4;
    assert_eq!(bin[chunk_off], 10, "chunk starts with the kept feature");
    assert!(!bin[chunk_off..chunk_off + 20].contains(&11), "the overflowing feature is absent from the chunk");
}
