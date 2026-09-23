//! Small independent terrain and routable-map fixtures for retained source tests.

use crate::*;

/// One 32×32 sample cell covering [0, 16384) µdeg on both axes. Height is
/// `offset + 2 * row + 3 * column`, so zero and non-flat samples need no external data.
pub fn plane(offset: i16) -> Vec<u8> {
    let mut bytes = vec![0; 36];
    bytes[..4].copy_from_slice(b"OBCT");
    bytes[4..7].copy_from_slice(&[1, 9, 14]);
    bytes[8..12].copy_from_slice(&16384u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&16384u32.to_le_bytes());
    bytes[16..18].copy_from_slice(&1u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&1u16.to_le_bytes());
    bytes[20..24].copy_from_slice(&32u32.to_le_bytes());
    bytes[32..36].copy_from_slice(&36u32.to_le_bytes());
    for ti in 0..2 {
        for tj in 0..2 {
            for row in 0..16 {
                for col in 0..16 {
                    let height = offset + 2 * (ti * 16 + row) + 3 * (tj * 16 + col);
                    bytes.extend_from_slice(&height.to_le_bytes());
                }
            }
        }
    }
    bytes
}

/// One two-node road from (lon,lat) (512,512) to (8192,8192), with embedded terrain.
pub fn map(offset: i16) -> Vec<u8> {
    let base = build_file(
        (0, 0, 16384, 16384),
        &[],
        &[LodSpec { max_mpp: f32::INFINITY, index: vec![], chunks: vec![], chunk_size: 4096 }],
    );
    // A water POI at the destination lets the real native UI start Route here.
    let poi = resolve_offset(&base, 32);
    let poi_index = align_up(poi + poi_dir_len());
    let poi_chunk = align_up(poi_index + 4);
    let pool = poi_chunk + 512;
    let cats: Vec<_> = (1..=6)
        .map(|id| PoiCat {
            category_id: id,
            index_offset: if id == 1 { poi_index } else { pool },
            node_count: u32::from(id == 1),
            chunk_count: u32::from(id == 1),
        })
        .collect();
    let mut bytes = base[..poi].to_vec();
    bytes.extend_from_slice(&poi_directory(512, &cats, pool, 0));
    bytes.resize(poi_index, FILLER);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.resize(poi_chunk, FILLER);
    bytes.extend_from_slice(&pack_poi_chunk(&[pack_poi_record(8192, 8192, 1, "Terrain goal", 0xffff)], 512));
    bytes.extend_from_slice(&hours_pool(&[]));
    bytes.resize(align_up(bytes.len()), FILLER);
    let nav = bytes.len();
    bytes[36..40].copy_from_slice(&scaled(nav).to_le_bytes());
    let profile = default_nav_profile_table();
    let profile_at = align_up(nav + NAV_DIR_LEN);
    let index_at = align_up(profile_at + profile.len());
    let nodes_at = align_up(index_at + 4);
    let edges_at = nodes_at + NAV_CHUNK_SIZE;
    bytes.extend_from_slice(&nav_directory(index_at, 1, 1, edges_at, 1, NAV_CHUNK_SIZE as u16, profile_at, 1));
    bytes.resize(profile_at, FILLER);
    bytes.extend_from_slice(&profile);
    bytes.resize(index_at, FILLER);
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.resize(nodes_at, FILLER);
    let a = pack_nav_record(512, 512, 0, &[(1, 8192, 8192, 0, 1200, 0, 75)]);
    let b = pack_nav_record(8192, 8192, 1, &[(0, 512, 512, 0, 1200, 0, 0)]);
    bytes.extend_from_slice(&pack_nav_chunk(&[a, b], NAV_CHUNK_SIZE));
    let mut edge = pack_nav_edge_record(1200, 0, &[(512, 512), (4096, 4096), (8192, 8192)]);
    edge[5] |= 0x80; // Every integration point lies inside the complete plane below.
    bytes.extend_from_slice(&pad(edge, NAV_CHUNK_SIZE));
    append_dark_styles(&mut bytes, &[], DARK_MARKER);
    splice_terrain(&bytes, &plane(offset))
}
