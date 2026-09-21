//! Query-contract tests for the nearest-16 POI scan (`Reader::nearest_pois`).
//!
//! Each test builds a synthetic `.obcm` whose POI section is a real per-category quadtree, then
//! asserts the reader's expanding-ring scan returns exactly the brute-force nearest-16: same set,
//! same order, same distances. The last test runs against the committed real Monaco map.

use obc_map_scene::{cos_lat, ground_dist_m_cl};
use obc_reader::{MapCache, MapTables, Poi, PoiCategory, Reader, SliceSource, MAX_POI_RESULTS};
use obcm_testkit::{align_up, build_poi_map, resolve_offset, PoiSpec};

/// Ground distance in metres from `pos` to a POI, in the same equirectangular metric the reader
/// uses, so the brute-force truth and the query agree before the `u32` round.
fn dist_m(pos: (i32, i32), lat: i32, lon: i32) -> f32 {
    ground_dist_m_cl(pos, (lon, lat), cos_lat(pos.1))
}

/// Brute-force truth: the nearest-16 POIs of `cat_id` to `pos` in canonical order. Selection uses
/// the exact float distance, then sorts the winners canonically so equal-distance members compare
/// as a set, because the query orders ties by scan order. The fixtures avoid a tie straddling the
/// 16/17 boundary, so the winner set is unambiguous.
fn brute_force(
    pois: &[PoiSpec],
    cat_id: u8,
    pos: (i32, i32),
    subtype_cat: impl Fn(u8) -> u8,
) -> Vec<(u32, i32, i32, u8)> {
    let mut by_dist: Vec<(f32, i32, i32, u8)> = pois
        .iter()
        .filter(|p| subtype_cat(p.subtype) == cat_id)
        .map(|p| (dist_m(pos, p.lat, p.lon), p.lat, p.lon, p.subtype))
        .collect();
    by_dist.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
    by_dist.truncate(MAX_POI_RESULTS);
    // Canonicalize so the set comparison ignores the query's tie order.
    let mut out: Vec<(u32, i32, i32, u8)> =
        by_dist.into_iter().map(|(d, lat, lon, s)| (d as u32, lat, lon, s)).collect();
    out.sort();
    out
}

/// Subtype to category id for the fixtures, so the tests do not depend on the reader's own table.
fn cat_of(subtype: u8) -> u8 {
    match subtype {
        1..=4 => 1,   // Water
        5..=6 => 2,   // Campsite
        7..=12 => 3,  // Accommodation
        13..=16 => 4, // Resupply
        17 => 5,      // Pharmacy
        18 => 6,      // Bike shop
        _ => 0,
    }
}

fn query(bytes: &[u8], cat: PoiCategory, pos: (i32, i32)) -> Vec<Poi> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out = heapless::Vec::<Poi, MAX_POI_RESULTS>::new();
    r.nearest_pois(cat, pos, &mut out).unwrap();
    out.into_iter().collect()
}

/// Assert the query returns exactly the brute-force nearest-16 as a set, with ascending distances
/// that equal the recomputed metric, and that a re-run gives the identical sequence.
fn assert_matches_brute_force(pois: &[PoiSpec], cat: PoiCategory, cat_id: u8, pos: (i32, i32), bytes: &[u8]) {
    let got = query(bytes, cat, pos);
    // Ascending, and each distance equals the truth.
    let mut prev = 0u32;
    for p in &got {
        assert!(p.distance_m >= prev, "results ascending by distance");
        prev = p.distance_m;
        assert_eq!(p.distance_m, dist_m(pos, p.lat, p.lon) as u32, "distance matches the metric");
    }
    // The returned set equals the brute-force winner set.
    let mut got_canon: Vec<(u32, i32, i32, u8)> = got.iter().map(|p| (p.distance_m, p.lat, p.lon, p.subtype)).collect();
    got_canon.sort();
    let want = brute_force(pois, cat_id, pos, cat_of);
    assert_eq!(got_canon, want, "query set must equal brute-force nearest-16");
    // Determinism: a second identical query yields the identical sequence.
    let again = query(bytes, cat, pos);
    let seq: Vec<(i32, i32, u8)> = got.iter().map(|p| (p.lat, p.lon, p.subtype)).collect();
    let seq2: Vec<(i32, i32, u8)> = again.iter().map(|p| (p.lat, p.lon, p.subtype)).collect();
    assert_eq!(seq, seq2, "query is deterministic (stable tie order)");
}

const BBOX: (i32, i32, i32, i32) = (7_000_000, 43_000_000, 8_000_000, 44_000_000);
const CS: usize = 512;

/// A grid of POIs across the map, two categories interleaved so the query must filter.
#[test]
fn grid_nearest_matches_brute_force() {
    let mut water = Vec::new();
    let mut bikes = Vec::new();
    // 10×10 grid at about 1 km spacing, two categories on alternating cells.
    for i in 0..10 {
        for j in 0..10 {
            let lat = 43_100_000 + i * 9_000;
            let lon = 7_100_000 + j * 9_000;
            let name = format!("P{i}{j}");
            if (i + j) % 2 == 0 {
                water.push(PoiSpec { lat, lon, subtype: 1, name, payload: 0xFFFF });
            } else {
                bikes.push(PoiSpec { lat, lon, subtype: 18, name, payload: 0xFFFF });
            }
        }
    }
    let mut all = water.clone();
    all.extend(bikes.clone());
    let bytes = build_poi_map(BBOX, CS, &[(1, water), (6, bikes)]);
    // A query point in the grid interior, so the set fills from the first ring.
    let pos = (7_150_000, 43_150_000);
    assert_matches_brute_force(&all, PoiCategory::Water, 1, pos, &bytes);
    assert_matches_brute_force(&all, PoiCategory::BikeShop, 6, pos, &bytes);
}

/// Tight clusters far apart: the ring must expand past the near-empty first ring to reach the
/// nearest cluster, over multiple chunks.
#[test]
fn clusters_across_multiple_chunks() {
    let mut pois = Vec::new();
    // Three clusters of 20 water POIs about 20 km apart. 20 is more than one chunk holds, so each
    // cluster subdivides into several leaves.
    for (ci, (clat, clon)) in
        [(43_200_000, 7_200_000), (43_500_000, 7_500_000), (43_800_000, 7_800_000)].iter().enumerate()
    {
        for k in 0..20 {
            pois.push(PoiSpec {
                lat: clat + (k % 5) * 300,
                lon: clon + (k / 5) * 300,
                subtype: 1,
                name: format!("C{ci}-{k}"),
                payload: 0xFFFF,
            });
        }
    }
    let bytes = build_poi_map(BBOX, CS, &[(1, pois.clone())]);
    // Query near the middle cluster: its 20 fill the set.
    let pos = (7_500_000, 43_500_000);
    assert_matches_brute_force(&pois, PoiCategory::Water, 1, pos, &bytes);
    // And a query between clusters, which forces ring expansion across more than one.
    let pos2 = (7_350_000, 43_350_000);
    assert_matches_brute_force(&pois, PoiCategory::Water, 1, pos2, &bytes);
}

/// Everything in one chunk (≤ 16 POIs, single leaf): the query still returns them ascending.
#[test]
fn all_in_one_chunk() {
    let mut pois = Vec::new();
    for k in 0..12 {
        pois.push(PoiSpec {
            lat: 43_500_000 + k * 200,
            lon: 7_500_000 + k * 150,
            subtype: 5,
            name: String::new(),
            payload: 0xFFFF,
        });
    }
    let bytes = build_poi_map(BBOX, CS, &[(2, pois.clone())]);
    let pos = (7_500_000, 43_500_000);
    assert_matches_brute_force(&pois, PoiCategory::Campsite, 2, pos, &bytes);
    // Fewer than 16 in the whole map ⇒ all returned.
    assert_eq!(query(&bytes, PoiCategory::Campsite, pos).len(), 12);
}

/// A category dense enough to span many leaves, all far from `pos`, so the ring must expand to a
/// map-covering pass to collect 16. This guards the streaming scan against silently dropping a
/// leaf on a wide pass, which a per-leaf buffer would have.
#[test]
fn dense_category_exhaustive_pass_drops_no_leaf() {
    let mut pois = Vec::new();
    // About 300 POIs on a coarse grid across the whole map, none near the query corner.
    let mut n = 0;
    for i in 0..18 {
        for j in 0..18 {
            pois.push(PoiSpec {
                lat: 43_300_000 + i * 35_000,
                lon: 7_300_000 + j * 35_000,
                subtype: 1,
                name: format!("G{n}"),
                payload: 0xFFFF,
            });
            n += 1;
        }
    }
    let bytes = build_poi_map(BBOX, CS, &[(1, pois.clone())]);
    // Query the far corner, so the first rings are empty and expansion is forced.
    let pos = (7_010_000, 43_010_000);
    let got = query(&bytes, PoiCategory::Water, pos);
    assert_eq!(got.len(), MAX_POI_RESULTS, "16 found despite all POIs being far + across many leaves");
    assert_matches_brute_force(&pois, PoiCategory::Water, 1, pos, &bytes);
}

/// More than 16 within the very first ring: the query must return the 16 nearest and drop the
/// farther ones.
#[test]
fn more_than_16_in_first_ring() {
    let mut pois = Vec::new();
    // 30 POIs within ~500 m of the query point — all inside the initial 2 km ring.
    for k in 0..30 {
        pois.push(PoiSpec {
            lat: 43_500_000 + (k % 6) * 800 - 2000,
            lon: 7_500_000 + (k / 6) * 800 - 2000,
            subtype: 17,
            name: format!("Ph{k}"),
            payload: 0xFFFF,
        });
    }
    let bytes = build_poi_map(BBOX, CS, &[(5, pois.clone())]);
    let pos = (7_500_000, 43_500_000);
    let got = query(&bytes, PoiCategory::Pharmacy, pos);
    assert_eq!(got.len(), MAX_POI_RESULTS, "must cap at 16 even with 30 in range");
    assert_matches_brute_force(&pois, PoiCategory::Pharmacy, 5, pos, &bytes);
}

/// The termination-guarantee edge: 15 POIs just inside the first ring plus one true 16th-nearest
/// just outside it. The scan must expand to find it rather than stop with an incomplete set.
#[test]
fn ring_expansion_finds_the_16th_just_outside() {
    let pos = (7_500_000, 43_500_000);
    let mut pois = Vec::new();
    // 15 POIs within ~1 km (well inside the ~2 km initial ring).
    for k in 0..15 {
        pois.push(PoiSpec {
            lat: 43_500_000 + (k % 4) * 600 - 900,
            lon: 7_500_000 + (k / 4) * 600 - 900,
            subtype: 7,
            name: format!("H{k}"),
            payload: 0xFFFF,
        });
    }
    // The 16th sits outside the initial ring, so the first pass finds only 15 and cannot prove
    // completeness.
    let far = PoiSpec { lat: 43_500_000 + 27_000, lon: 7_500_000, subtype: 7, name: "Far".into(), payload: 0xFFFF };
    pois.push(far.clone());
    let bytes = build_poi_map(BBOX, CS, &[(3, pois.clone())]);
    let got = query(&bytes, PoiCategory::Accommodation, pos);
    assert_eq!(got.len(), MAX_POI_RESULTS, "the 16th just outside the first ring must be found");
    assert!(got.iter().any(|p| p.name == "Far"), "the far POI is the 16th-nearest and must appear");
    assert_matches_brute_force(&pois, PoiCategory::Accommodation, 3, pos, &bytes);
}

/// POIs straddling a ring boundary must all be found once, deduped across the widened re-walk.
#[test]
fn straddling_ring_boundary_no_duplicates() {
    let pos = (7_500_000, 43_500_000);
    let mut pois = Vec::new();
    // A ring of POIs right at the initial half-extent plus a few farther, so the second pass
    // re-walks the inner ones, which must not be returned twice.
    for k in 0..24 {
        let r = 18_000 + (k % 3) * 4_000; // 18k, 22k, 26k µdeg from pos
        let ang = k as f32 * 0.26;
        pois.push(PoiSpec {
            lat: 43_500_000 + (r as f32 * ang.cos()) as i32,
            lon: 7_500_000 + (r as f32 * ang.sin() * 1.4) as i32,
            subtype: 13,
            name: format!("S{k}"),
            payload: 0xFFFF,
        });
    }
    let bytes = build_poi_map(BBOX, CS, &[(4, pois.clone())]);
    let got = query(&bytes, PoiCategory::Resupply, pos);
    // No duplicate (lat,lon,subtype).
    let mut keys: Vec<(i32, i32, u8)> = got.iter().map(|p| (p.lat, p.lon, p.subtype)).collect();
    let n = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), n, "no POI returned twice across ring passes");
    assert_matches_brute_force(&pois, PoiCategory::Resupply, 4, pos, &bytes);
}

/// An empty category yields an empty result, not an error.
#[test]
fn empty_category_returns_empty_ok() {
    // Only Water populated; query Campsite (empty).
    let water = vec![PoiSpec { lat: 43_500_000, lon: 7_500_000, subtype: 1, name: "W".into(), payload: 0xFFFF }];
    let bytes = build_poi_map(BBOX, CS, &[(1, water)]);
    let got = query(&bytes, PoiCategory::Campsite, (7_500_000, 43_500_000));
    assert!(got.is_empty(), "an empty category returns no POIs");
}

/// The name field round-trips (named and unnamed), and coordinates/subtypes are exact.
#[test]
fn names_and_fields_round_trip() {
    let pois = vec![
        PoiSpec { lat: 43_500_100, lon: 7_500_200, subtype: 15, name: "Backerei Mueller".into(), payload: 0xFFFF },
        PoiSpec { lat: 43_500_300, lon: 7_500_400, subtype: 13, name: String::new(), payload: 0xFFFF },
    ];
    let bytes = build_poi_map(BBOX, CS, &[(4, pois)]);
    let got = query(&bytes, PoiCategory::Resupply, (7_500_000, 43_500_000));
    assert_eq!(got.len(), 2);
    let bakery = got.iter().find(|p| p.subtype == 15).unwrap();
    assert_eq!(bakery.name.as_str(), "Backerei Mueller");
    assert_eq!((bakery.lat, bakery.lon), (43_500_100, 7_500_200));
    let supermarket = got.iter().find(|p| p.subtype == 13).unwrap();
    assert_eq!(supermarket.name.as_str(), "", "unnamed POI ⇒ empty name (app shows the label)");
}

/// A directory advertising `chunk_size == 0` must not divide by zero or loop forever: the query
/// reports the unwalkable section as an error.
#[test]
fn corrupt_zero_chunk_size_is_safe() {
    let pois = vec![PoiSpec { lat: 43_500_000, lon: 7_500_000, subtype: 1, name: "W".into(), payload: 0xFFFF }];
    let mut bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let poi_off = resolve_offset(&bytes, 32);
    // Forge the shared chunk_size to 0. `MapTables::parse` accepts it, so the query must handle
    // it without panicking.
    bytes[poi_off + 1..poi_off + 3].copy_from_slice(&0u16.to_le_bytes());
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let mut out = heapless::Vec::new();
    assert!(Reader::new(&src, &tables, &cache)
        .nearest_pois(PoiCategory::Water, (7_500_000, 43_500_000), &mut out)
        .is_err());
}

/// A malformed record after a valid candidate fails the shared query and clears its partial page.
#[test]
fn corrupt_service_subtypes_fail_without_partial_results() {
    use obc_reader::reader::places::{PlaceQuery, PlaceWindow, QueryProgress, PLACE_PAGE_SIZE};
    let pois = vec![
        PoiSpec { lat: 43_500_000, lon: 7_500_000, subtype: 1, name: "Good".into(), payload: 0xFFFF },
        PoiSpec { lat: 43_500_100, lon: 7_500_100, subtype: 1, name: "Bad".into(), payload: 0xFFFF },
    ];
    let base = build_poi_map(BBOX, CS, &[(1, pois)]);
    let entry = resolve_offset(&base, 32) + 3;
    let index = resolve_offset(&base, entry + 1);
    let nodes = u32::from_le_bytes(base[entry + 5..entry + 9].try_into().unwrap()) as usize;
    let data = align_up(index + nodes * 4);
    for invalid in [0, 99, 20] {
        let mut bytes = base.clone();
        bytes[data + 64 + 8] = invalid;
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut query = PlaceQuery::new(
            1,
            obc_reader::PoiCategorySet::ALL,
            PlaceWindow::Nearby { position: (7_500_000, 43_500_000), radius_m: 5_000 },
            None,
        );
        let mut page = heapless::Vec::<_, PLACE_PAGE_SIZE>::new();
        while query.step(&reader, None, 1, &mut page) == QueryProgress::Pending {}
        assert!(matches!(query.progress(), QueryProgress::Failed(_)), "subtype {invalid}");
        assert!(page.is_empty());
    }
}

/// A chunk whose 0xFF end-of-records sentinel is overwritten with a valid-looking record byte must
/// still terminate at the chunk boundary, never reading past it.
#[test]
fn corrupt_missing_sentinel_stops_at_chunk_end() {
    // One POI, then a 0xFF sentinel, then padding. Overwriting the sentinel's subtype byte with a
    // valid subtype must still stop the record loop at the record cap, and the forged record has
    // invalid metadata and is reported as an error.
    let pois = vec![PoiSpec { lat: 43_500_000, lon: 7_500_000, subtype: 1, name: "One".into(), payload: 0xFFFF }];
    let mut bytes = build_poi_map(BBOX, CS, &[(1, pois)]);
    let poi_off = resolve_offset(&bytes, 32);
    let e1 = poi_off + 3;
    let idx_off = resolve_offset(&bytes, e1 + 1);
    let node_count = u32::from_le_bytes(bytes[e1 + 5..e1 + 9].try_into().unwrap()) as usize;
    // A category's chunks begin one rounding step past its index, not flush behind it.
    let data_start = align_up(idx_off + node_count * 4);
    // The sentinel subtype byte is at the second record slot; forge it to 1.
    bytes[data_start + 64 + 8] = 1;
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let mut out = heapless::Vec::new();
    assert!(Reader::new(&src, &tables, &cache)
        .nearest_pois(PoiCategory::Water, (7_500_000, 43_500_000), &mut out)
        .is_err());
}

/// A regional summit tree must return every in-radius candidate for the app's angular selection,
/// even past 32, without mixing service POIs into it.
#[test]
fn summit_spatial_query_preserves_metadata_and_clamps_the_search_radius() {
    let pos = (7_500_000, 43_500_000);
    let peaks: Vec<PoiSpec> = (0..80)
        .map(|i| PoiSpec {
            lat: pos.1 + (i % 8 - 4) * 140_000,
            lon: pos.0 + (i / 8 - 5) * 140_000,
            subtype: 19,
            name: format!("Mönch {i}"),
            payload: if i % 3 == 0 { i16::MIN as u16 } else { (i as i16 - 25) as u16 },
        })
        .collect();
    let mut records = peaks.clone();
    records.push(PoiSpec { lat: pos.1, lon: pos.0, subtype: 19, name: String::new(), payload: 10 });
    records.push(PoiSpec { lat: pos.1, lon: pos.0, subtype: 1, name: "Wrong category".into(), payload: 0xFFFF });
    let bytes = build_poi_map((6_000_000, 42_000_000, 9_000_000, 45_000_000), CS, &[(7, records)]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = Box::new(MapCache::new());
    let reader = Reader::new(&src, &tables, &cache);
    for radius in [0, 20_000, 70_000, u32::MAX] {
        let mut found = Vec::new();
        reader.visit_summits_within(pos, radius, |p| found.push(p)).unwrap();
        let mut expected: Vec<_> = peaks
            .iter()
            .filter(|p| dist_m(pos, p.lat, p.lon) <= radius.min(100_000) as f32)
            .map(|p| (p.lat, p.lon, p.name.clone(), (p.payload != i16::MIN as u16).then_some(p.payload as i16)))
            .collect();
        let mut actual: Vec<_> = found.iter().map(|p| (p.lat, p.lon, p.name.to_string(), p.elevation_m)).collect();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
        if radius == u32::MAX {
            assert!(actual.len() > 32, "caller, not reader, selects the 32 labels");
        }
    }
    assert!(query(&bytes, PoiCategory::Water, pos).is_empty());
    let empty = build_poi_map(BBOX, CS, &[]);
    let src = SliceSource(&empty);
    let tables = MapTables::parse(&src).unwrap();
    Reader::new(&src, &tables, &cache).visit_summits_within(pos, 100_000, |_| panic!("absent category")).unwrap();
}

#[test]
fn complete_pages_filter_hours_before_capacity_and_cancel_old_generations() {
    use obc_reader::reader::places::{HoursFilter, PlaceQuery, PlaceWindow, QueryProgress, PLACE_PAGE_SIZE};
    use obc_reader::PoiCategorySet;
    let pois: Vec<_> = (0..55)
        .map(|i| PoiSpec {
            // Sub-metre spacing puts distinct identities at tied integer distances across pages.
            lat: 43_500_000 + i,
            lon: 7_500_000,
            subtype: 1,
            name: format!("P{i:02}"),
            payload: if i < 20 { 0 } else { 1 },
        })
        .collect();
    let closed = [0; 29];
    let mut open = [0; 29];
    for day in 0..7 {
        open[2 + day * 4] = 96;
    }
    let bytes = obcm_testkit::build_poi_map_with_hours(BBOX, CS, &[(1, pois)], &[closed, open]);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    for (filter, local, start) in [
        (HoursFilter::HideClosed, Some((0, 600)), 20),
        (HoursFilter::All, Some((0, 600)), 0),
        (HoursFilter::HideClosed, None, 0),
    ] {
        let mut query = PlaceQuery::new(
            7,
            PoiCategorySet::ALL,
            PlaceWindow::Nearby { position: (7_500_000, 43_500_000), radius_m: 5_000 },
            local,
        )
        .with_hours_filter(filter);
        let mut page = heapless::Vec::<_, PLACE_PAGE_SIZE>::new();
        let mut names = Vec::new();
        let mut first_page = Vec::new();
        let mut second_start = None;
        loop {
            let status = loop {
                let status = query.step(&reader, None, 7, &mut page);
                if status != QueryProgress::Pending {
                    break status;
                }
            };
            if first_page.is_empty() {
                first_page = page.iter().map(|p| p.poi.name.clone()).collect();
            } else if second_start.is_none() {
                second_start = Some(query.key(&page[0]));
            }
            names.extend(page.iter().map(|p| p.poi.name.clone()));
            let QueryProgress::Ready { more, coverage_complete: true } = status else { panic!("{status:?}") };
            if !more {
                break;
            }
            query.next_page(query.key(page.last().unwrap()));
            page.clear();
        }
        assert_eq!(first_page.len(), PLACE_PAGE_SIZE);
        assert_eq!(
            names.iter().map(|name| name.as_str()).collect::<Vec<_>>(),
            (start..55).map(|i| format!("P{i:02}")).collect::<Vec<_>>()
        );
        query.previous_page(second_start.unwrap());
        page.clear();
        while query.step(&reader, None, 7, &mut page) == QueryProgress::Pending {}
        assert_eq!(page.iter().map(|p| p.poi.name.clone()).collect::<Vec<_>>(), first_page);
        query.next_page(query.key(page.last().unwrap()));
        assert_eq!(query.step(&reader, None, 8, &mut page), QueryProgress::Unavailable);
        assert!(page.is_empty());
        query.next_page(second_start.unwrap());
        query.previous_page(second_start.unwrap());
        assert_eq!(query.progress(), QueryProgress::Unavailable, "cancelled work cannot revive through paging");
    }
}

#[test]
fn coverage_and_train_are_explicit() {
    use obc_reader::reader::places::{PlaceQuery, PlaceWindow, QueryProgress, PLACE_PAGE_SIZE};
    use obc_reader::PoiCategorySet;
    let bytes = build_poi_map(
        BBOX,
        CS,
        &[(8, vec![PoiSpec { lat: 43_000_001, lon: 7_000_001, subtype: 20, name: "Station".into(), payload: 0xffff }])],
    );
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    let mut query = PlaceQuery::new(
        1,
        PoiCategorySet::ALL,
        PlaceWindow::Nearby { position: (7_000_001, 43_000_001), radius_m: 1000 },
        None,
    );
    let mut page = heapless::Vec::<_, PLACE_PAGE_SIZE>::new();
    while query.step(&reader, None, 1, &mut page) == QueryProgress::Pending {}
    assert_eq!(query.progress(), QueryProgress::Ready { more: false, coverage_complete: false });
    assert_eq!(page[0].poi.subtype, 20);
    assert_eq!(PoiCategorySet::ALL.len(), 7);
    assert!(PoiCategorySet::ALL.contains(PoiCategory::Train));
    assert_eq!(PoiCategorySet::ALL.bits() & (1 << 6), 0, "summits are not services");
}
