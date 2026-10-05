use super::*;
use nav::legacy::LegacyReader;
use obc_formats::io::{Error as IoError, SliceSource};
use std::cell::RefCell;
use std::vec::Vec as StdVec;

struct Traced<'a> {
    source: SliceSource<'a>,
    reads: RefCell<StdVec<(u64, usize)>>,
}
impl ByteSource for Traced<'_> {
    fn len(&self) -> u64 {
        self.source.len()
    }
    fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<(), IoError> {
        self.reads.borrow_mut().push((offset, bytes.len()));
        self.source.read_at(offset, bytes)
    }
}
struct Path {
    chunks: [[(i32, i32); 3]; 2],
    length: u32,
    reads: RefCell<StdVec<usize>>,
}
impl crate::RoutePath for Path {
    fn chunk_count(&self) -> usize {
        2
    }
    fn chunk_start_m(&self, k: usize) -> u32 {
        self.length * k as u32
    }
    fn chunk_bbox(&self, k: usize) -> BBox {
        let points = self.chunks[k];
        BBox {
            min_lon: points.iter().map(|p| p.0).min().unwrap(),
            max_lon: points.iter().map(|p| p.0).max().unwrap(),
            min_lat: points.iter().map(|p| p.1).min().unwrap(),
            max_lat: points.iter().map(|p| p.1).max().unwrap(),
        }
    }
    fn visit_chunk_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)])) {
        self.reads.borrow_mut().push(k);
        visit(&self.chunks[k]);
    }
}
fn next(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
    *seed
}
fn check(bytes: &[u8]) {
    let src = Traced { source: SliceSource(bytes), reads: RefCell::new(StdVec::new()) };
    let old_src = Traced { source: SliceSource(bytes), reads: RefCell::new(StdVec::new()) };
    let tables = MapTables::parse(&src).unwrap();
    src.reads.borrow_mut().clear();
    let cache = MapCache::new();
    let old_cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let old = LegacyReader(Reader::new(&old_src, &tables, &old_cache));
    let mut tiles = NavTileCache::new();
    let mut old_tiles = NavTileCache::new();
    let bounds = r.bbox;
    let lat = (bounds.min_lat as i64 + bounds.max_lat as i64).div_euclid(2) as i32;
    let lon = (bounds.min_lon as i64 + bounds.max_lon as i64).div_euclid(2) as i32;
    let points = [(bounds.min_lon, lat), (lon, lat), (bounds.max_lon, lat)];
    let length = points.windows(2).map(|p| obc_map_scene::ground_dist_m(p[0], p[1])).sum::<f32>() as u32;
    let path = Path { chunks: [points, [points[2], points[1], points[0]]], length, reads: RefCell::new(StdVec::new()) };
    let old_path = Path { chunks: path.chunks, length, reads: RefCell::new(StdVec::new()) };
    let mut seed = 23;
    for query in 0..33 {
        let view = if query == 0 {
            bounds
        } else {
            let lon = bounds.min_lon
                + (next(&mut seed) as u64 % (bounds.max_lon as i64 - bounds.min_lon as i64 + 1) as u64) as i32;
            let lat = bounds.min_lat
                + (next(&mut seed) as u64 % (bounds.max_lat as i64 - bounds.min_lat as i64 + 1) as u64) as i32;
            let width = (next(&mut seed) % 100_001) as i32;
            BBox { min_lon: lon - width, min_lat: lat - width, max_lon: lon + width, max_lat: lat + width }
        };
        for lod in 0..r.lods().len() {
            let mut a = StdVec::new();
            let mut b = StdVec::new();
            assert_eq!(
                r.for_each_chunk(lod, &view, |cid, bbox| a.push((cid, bbox))),
                old.for_each_chunk(lod, &view, |cid, bbox| b.push((cid, bbox)))
            );
            assert_eq!(a, b);
            assert_eq!(*src.reads.borrow(), *old_src.reads.borrow());
            assert_eq!(cache.stats(), old_cache.stats());
        }
        let mut a = StdVec::new();
        let mut b = StdVec::new();
        assert_eq!(r.for_each_nav_chunk(&view, |cid| a.push(cid)), old.for_each_nav_chunk(&view, |cid| b.push(cid)));
        assert_eq!(a, b);
        let mut a = StdVec::new();
        let mut b = StdVec::new();
        r.for_each_nav_node_cached(&view, &mut tiles, |n| {
            a.push((n.id, n.lat, n.lon, n.neighbors().collect::<StdVec<_>>()))
        })
        .unwrap();
        old.for_each_nav_node_cached(&view, &mut old_tiles, |n| {
            b.push((n.id, n.lat, n.lon, n.neighbors().collect::<StdVec<_>>()))
        })
        .unwrap();
        assert_eq!(a, b);
        assert_eq!(tiles.stats(), old_tiles.stats());
        assert_eq!(*src.reads.borrow(), *old_src.reads.borrow());
        let mut new = MapPointQuery::new(tables.generation, view, crate::PoiCategorySet::ALL, true);
        let mut legacy =
            proof_map_points::MapPointQuery::new(tables.generation, view, crate::PoiCategorySet::ALL, true);
        for steps in 0..100_000 {
            assert!(steps < 99_999);
            let mut a = StdVec::new();
            let mut b = StdVec::new();
            let a_done = new.step(&r, |p| a.push((p.position, p.source, p.subtype, p.elevation_m))).unwrap();
            let b_done = legacy.step(&old, |p| b.push((p.position, p.source, p.subtype, p.elevation_m))).unwrap();
            assert_eq!((a_done, a), (b_done, b));
            assert_eq!(*src.reads.borrow(), *old_src.reads.borrow());
            if a_done {
                break;
            }
        }
        let position = (
            (view.min_lon as i64 + view.max_lon as i64).div_euclid(2) as i32,
            (view.min_lat as i64 + view.max_lat as i64).div_euclid(2) as i32,
        );
        let radius_m = 2000;
        let mut new = places::PlaceQuery::new(
            7,
            crate::PoiCategorySet::ALL,
            places::PlaceWindow::Nearby { position, radius_m },
            None,
        );
        let mut legacy = proof_places::PlaceQuery::new(
            7,
            crate::PoiCategorySet::ALL,
            proof_places::PlaceWindow::Nearby { position, radius_m },
            None,
        );
        let mut a = Vec::<crate::CorridorPoi, 8>::new();
        let mut b = Vec::<crate::CorridorPoi, 8>::new();
        for steps in 0..100_000 {
            assert!(steps < 99_999);
            let status = new.step(&r, None, 7, &mut a);
            let old_status = legacy.step(&old, None, 7, &mut b);
            assert_eq!(format!("{status:?}"), format!("{old_status:?}"));
            assert_eq!(format!("{a:?}"), format!("{b:?}"));
            assert_eq!(*src.reads.borrow(), *old_src.reads.borrow());
            if status != places::QueryProgress::Pending {
                break;
            }
        }
        assert_eq!(cache.stats(), old_cache.stats());
        if query % 8 == 0 {
            let mut new = places::PlaceQuery::new(
                7,
                crate::PoiCategorySet::ALL,
                places::PlaceWindow::Corridor { from_m: 0, to_m: 2 * length, half_width_m: 800 },
                None,
            );
            let mut legacy = proof_places::PlaceQuery::new(
                7,
                crate::PoiCategorySet::ALL,
                proof_places::PlaceWindow::Corridor { from_m: 0, to_m: 2 * length, half_width_m: 800 },
                None,
            );
            let mut a = Vec::<crate::CorridorPoi, 8>::new();
            let mut b = Vec::<crate::CorridorPoi, 8>::new();
            for _page in 0..4 {
                for step in 0..100_000 {
                    assert!(step < 99_999);
                    let status = new.step(&r, Some(&path), 7, &mut a);
                    let old_status = legacy.step(&old, Some(&old_path), 7, &mut b);
                    assert_eq!(format!("{status:?}"), format!("{old_status:?}"));
                    assert_eq!(a, b);
                    assert_eq!(*src.reads.borrow(), *old_src.reads.borrow());
                    assert_eq!(*path.reads.borrow(), *old_path.reads.borrow());
                    if status != places::QueryProgress::Pending {
                        break;
                    }
                }
                let Some(last) = a.last() else { break };
                new.next_page(new.key(last));
                legacy.next_page(legacy.key(last));
                a.clear();
                b.clear();
            }
        }
        src.reads.borrow_mut().clear();
        old_src.reads.borrow_mut().clear();
    }
    println!(
        "walk differential: 33 views, {} LODs, both resumable queries, exact reads and cache counters",
        r.lods().len()
    );
}
#[test]
fn testkit_walks() {
    check(&obcm_testkit::build_bench_map());
    let pois = (0..80)
        .map(|n| obcm_testkit::PoiSpec {
            lat: n * 300 - 12_000,
            lon: n * 500 - 20_000,
            subtype: 0,
            name: "Water".into(),
            payload: 0xffff,
        })
        .collect::<StdVec<_>>();
    check(&obcm_testkit::build_poi_map((-25_000, -15_000, 25_000, 15_000), 512, &[(0, pois)]));
}
#[cfg(feature = "external-fixtures")]
#[cfg_attr(miri, ignore)]
#[test]
fn grimsel_walks() {
    check(&obc_fixtures::read("sim-grimsel", "grimsel.obcm"));
}
