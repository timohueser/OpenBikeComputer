use super::*;
use crate::{MapCache, MapTables};
use legacy::LegacyReader;
use obc_formats::io::SliceSource;
use obc_formats::obcm::nav_edge_id;
use std::vec::Vec as StdVec;

fn next(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    *seed
}

fn map() -> StdVec<u8> {
    use obcm_testkit::*;
    let mut bytes = build_file(
        (-1_000_000, -1_000_000, 1_000_000, 1_000_000),
        &[],
        &[LodSpec { max_mpp: 0.0, chunk_size: 512, index: vec![EMPTY_LEAF], chunks: vec![] }],
    );
    let nav = align_up(bytes.len());
    bytes.resize(nav, 0xff);
    bytes[36..40].copy_from_slice(&scaled(nav).to_le_bytes());
    let profile = align_up(nav + NAV_DIR_LEN);
    let index = align_up(profile + NAV_PROFILE_LEN);
    let nodes = align_up(index + 4);
    let pool = nodes + NAV_CHUNK_SIZE;
    let mut seed = 41;
    let mut edges = StdVec::new();
    let mut chunk = StdVec::new();
    for n in 2..=124 {
        let mut point = (10_000, -10_000);
        let mut pts = vec![point];
        for _ in 1..n {
            point.0 += (next(&mut seed) % 2001) as i32 - 1000;
            point.1 += (next(&mut seed) % 2001) as i32 - 1000;
            pts.push(point);
        }
        let rec = pack_nav_edge_record(n * 13, n as u8, &pts);
        if chunk.len() + rec.len() > NAV_CHUNK_SIZE {
            edges.extend(pad(core::mem::take(&mut chunk), NAV_CHUNK_SIZE));
        }
        chunk.extend(rec);
    }
    edges.extend(pad(chunk, NAV_CHUNK_SIZE));
    bytes.extend(nav_directory(
        index,
        1,
        1,
        pool,
        (edges.len() / NAV_CHUNK_SIZE) as u32,
        NAV_CHUNK_SIZE as u16,
        profile,
        1,
    ));
    bytes.resize(profile, 0xff);
    bytes.extend(default_nav_profile_table());
    bytes.resize(index, 0xff);
    bytes.extend(0u32.to_le_bytes());
    bytes.resize(nodes, 0xff);
    bytes.extend(pack_nav_chunk(
        &[
            pack_nav_record(10_000, -10_000, 0, &[]),
            pack_nav_record(10_001, -10_002, 1, &[(2, 11_000, -9_000, 0, 42, 17, 19)]),
            pack_nav_record(11_000, -9_000, 2, &[(1, 10_001, -10_002, 0, 42, 17, 31)]),
        ],
        NAV_CHUNK_SIZE,
    ));
    bytes.extend(edges);
    bytes
}

fn check(bytes: &[u8]) {
    let source = SliceSource(bytes);
    let tables = MapTables::parse(&source).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&source, &tables, &cache);
    let old = LegacyReader(Reader::new(&source, &tables, &cache));
    let mut new_tiles = NavTileCache::new();
    let mut old_tiles = NavTileCache::new();
    let mut seed = 239;
    let mut count = 0;
    let mut queries = StdVec::new();
    for chunk in 0..tables.nav.edge_chunk_count as u32 {
        for ordinal in 0..32 {
            let id = nav_edge_id(chunk, ordinal).unwrap();
            let mut new_points = Vec::<_, 128>::new();
            let mut old_points = Vec::<_, 128>::new();
            assert_eq!(reader.nav_edge(id, &mut new_points), old.nav_edge(id, &mut old_points));
            assert_eq!(new_points, old_points);
            assert_eq!(reader.nav_edge_facts(id), old.nav_edge_facts(id));
            if new_points.is_empty() {
                continue;
            }
            count += 1;
            if count % 7 == 1 && queries.len() < 128 {
                queries.push(new_points[new_points.len() / 2]);
            }
            for start in [new_points[0], *new_points.last().unwrap(), (i32::MIN, i32::MAX)] {
                let mut a = StdVec::new();
                let mut b = StdVec::new();
                assert_eq!(
                    reader.nav_edge_oriented(&mut new_tiles, id, start, |p| a.push(p)),
                    old.nav_edge_oriented(&mut old_tiles, id, start, |p| b.push(p))
                );
                assert_eq!(a, b);
            }
            for _ in 0..8 {
                let point = new_points[next(&mut seed) as usize % new_points.len()];
                let p = (
                    point.0 + (next(&mut seed) % 2001) as i32 - 1000,
                    point.1 + (next(&mut seed) % 2001) as i32 - 1000,
                );
                let new = reader.project_nav_edge_cached(&mut new_tiles, id, p);
                let legacy = old.project_nav_edge_cached(&mut old_tiles, id, p);
                assert_eq!(new, legacy);
                assert_eq!(new.map(|v| v.distance_m.to_bits()), legacy.map(|v| v.distance_m.to_bits()));
                let mut position = || {
                    let segment = next(&mut seed) as usize % (new_points.len() - 1);
                    let fraction = next(&mut seed) as u16;
                    let (a, b) = (new_points[segment], new_points[segment + 1]);
                    let lerp = |a: i32, b: i32| a + ((b as i64 - a as i64) * fraction as i64 / 65535) as i32;
                    NavEdgePosition { segment: segment as u16, fraction, coord: (lerp(a.0, b.0), lerp(a.1, b.1)) }
                };
                let (from, to) = (position(), position());
                let mut a = StdVec::new();
                let mut b = StdVec::new();
                assert_eq!(
                    reader.nav_edge_slice_oriented(&mut new_tiles, id, from, to, |p| a.push(p)),
                    old.nav_edge_slice_oriented(&mut old_tiles, id, from, to, |p| b.push(p))
                );
                assert_eq!(a, b);
            }
            assert_eq!(new_tiles.stats(), old_tiles.stats());
        }
    }
    let mut a = StdVec::new();
    let mut b = StdVec::new();
    reader
        .for_each_nav_node_cached(&tables.bbox, &mut new_tiles, |n| {
            a.push((n.id, n.lat, n.lon, n.neighbors().collect::<StdVec<_>>()))
        })
        .unwrap();
    old.for_each_nav_node_cached(&tables.bbox, &mut old_tiles, |n| {
        b.push((n.id, n.lat, n.lon, n.neighbors().collect::<StdVec<_>>()))
    })
    .unwrap();
    assert_eq!(a, b);
    assert_eq!(new_tiles.stats(), old_tiles.stats());
    for p in &queries {
        let view = BBox { min_lon: p.0 - 2000, max_lon: p.0 + 2000, min_lat: p.1 - 2000, max_lat: p.1 + 2000 };
        let a = reader.nearest_nav_edge_candidate_cached(&view, &mut new_tiles, *p, 200.0).unwrap();
        let b = old.nearest_nav_edge_candidate_cached(&view, &mut old_tiles, *p, 200.0).unwrap();
        assert_eq!(a, b);
        if let (Some(a), Some(b)) = (a, b) {
            assert_eq!(a.distance_m.to_bits(), b.distance_m.to_bits());
            assert_eq!(
                reader.resolve_nav_edge_candidate_cached(a, &mut new_tiles).unwrap(),
                old.resolve_nav_edge_candidate_cached(b, &mut old_tiles).unwrap()
            );
        }
        assert_eq!(
            reader.unique_nav_edge_candidate_cached(&view, &mut new_tiles, *p, 200.0).unwrap(),
            old.unique_nav_edge_candidate_cached(&view, &mut old_tiles, *p, 200.0).unwrap()
        );
        assert_eq!(new_tiles.stats(), old_tiles.stats());
    }
    println!(
        "differential: {count} edges, 8 projections/slices each, {} candidate/resolution queries, exact counters {:?}",
        queries.len(),
        new_tiles.stats()
    );
}

#[test]
fn testkit() {
    check(&map());
}

#[cfg(feature = "external-fixtures")]
#[cfg_attr(miri, ignore)]
#[test]
fn grimsel() {
    check(&obc_fixtures::read("sim-grimsel", "grimsel.obcm"));
}
