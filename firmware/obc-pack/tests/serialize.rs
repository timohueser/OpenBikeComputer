//! Port of `packer/tests/test_serialize.py`: the same inputs must produce the
//! same bytes. Rings are pre-closed here because shapely closes polygon rings
//! automatically, so the geometry that reaches `pack_feature` (in Python and in
//! the dump the harness feeds us) already has `first == last`.

use obc_pack::{pack_chunk, pack_feature, pack_style_dict, serialize_lods, Feature, Kind, LodLayer, Node, Style};

fn line(style_id: u8, pts: &[(f64, f64)]) -> Feature {
    Feature { style_id, kind: Kind::Line, rings: vec![pts.to_vec()] }
}

#[test]
fn pack_style_dict_one_style() {
    // test_pack_style_dict: id 10, z 50, color 0xF9A6, weight 4, priority 2.
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
    // test_pack_feature_8bit: a 2-point line, anchor at the node min corner.
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
    // test_pack_chunk_padding.
    let f = line(10, &[(1.0, 1.0), (1.0001, 1.0001)]);
    let node_bbox = (1_000_000, 1_000_000, 1_010_000, 1_010_000);
    let chunk = pack_chunk(&[f], node_bbox, 32);
    assert_eq!(chunk.len(), 32);
    assert!(chunk[14..].iter().all(|&b| b == 0xFF)); // 18 bytes of padding
}

#[test]
fn pack_polygon_with_hole() {
    // test_pack_polygon_small. shapely closes both rings (4 pts -> 5 stored).
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
    // test_serialize_lods_header: one LOD, one empty leaf, no styles.
    let lods = vec![LodLayer {
        max_mpp: None,
        chunk_size: 2048,
        root: Node::Leaf { bbox: (0, 0, 100, 100), features: vec![] },
    }];
    let bin = serialize_lods(&lods, &[], 0xF800, (0, 0, 100, 100));

    // v5 header(32) + style count(1) + 1 LOD entry(18) + index(4) = 55.
    assert_eq!(bin.len(), 55);
    assert_eq!(&bin[0..4], b"OBCM");
    assert_eq!(bin[4], 5); // version
    assert_eq!(u32::from_le_bytes([bin[21], bin[22], bin[23], bin[24]]), 32); // style offset
    assert_eq!(bin[25], 1); // lod count
    let lod_tbl = u32::from_le_bytes([bin[26], bin[27], bin[28], bin[29]]) as usize;
    assert_eq!(lod_tbl, 33); // 32 header + 1 style-count byte

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
