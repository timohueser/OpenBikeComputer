//! The route-corridor query's OBCR half (epic #946, U2): `RouteReader` as an
//! [`obc_reader::RoutePath`], driven end to end over a real converted `.obcr` and a real packed POI
//! quadtree.
//!
//! `obc-reader` owns the query and its projection math (pinned in its own `poi_corridor.rs` against
//! a hand-built path); what only this crate can pin is that the **seam** hands the query the right
//! geometry — chunk order, seam-shared points, and above all the *same along-route axis* stored
//! waypoints are placed on. A corridor POI and a `<wpt>` at the same coordinate must report the same
//! distance, or the Up-ahead list would mix two rulers in one column.

use obc_formats::io::SliceSource;
use obc_reader::{
    CorridorPoi, MapCache, MapTables, PoiCategory, PoiCategorySet, Reader, RoutePath, MAX_CORRIDOR_RESULTS,
};
use obc_route::{RouteIndex, RouteReader};
use obcm_testkit::{build_poi_map, PoiSpec};

mod common;
use common::convert;

/// The map bbox the POI fixtures pack into, and the POI chunk size (the packer's §7.1 default).
const BBOX: (i32, i32, i32, i32) = (7_000_000, 47_000_000, 9_000_000, 49_000_000);
const CS: usize = 512;

/// A GPX track running due east along 48.0000° N from 7.8000° E, one point every 0.0020°
/// (≈149 m), plus whatever `<wpt>`s the caller wants. Long enough (30 points, ≈4.5 km) to span
/// several OBCR chunks.
fn gpx(wpts: &str) -> String {
    let mut s = String::from(r#"<?xml version="1.0"?><gpx version="1.1"><trk><trkseg>"#);
    for i in 0..30 {
        let lon = 7.8000 + 0.0020 * i as f64;
        s.push_str(&format!(r#"<trkpt lat="48.0000" lon="{lon:.4}"><ele>200.0</ele></trkpt>"#));
    }
    s.push_str("</trkseg></trk>");
    s.push_str(wpts);
    s.push_str("</gpx>");
    s
}

/// Run the corridor query over `map` and the converted route `obcr`.
fn query(map: &[u8], obcr: &[u8], cats: PoiCategorySet, progress_m: u32) -> Vec<CorridorPoi> {
    let route_src = SliceSource(obcr);
    let idx = RouteIndex::read(&route_src).expect("valid .obcr");
    let route = RouteReader::new(&idx, &route_src);

    let map_src = SliceSource(map);
    let tables = MapTables::parse(&map_src).expect("valid .obcm");
    let cache = MapCache::new();
    let reader = Reader::new(&map_src, &tables, &cache);

    let mut out = heapless::Vec::<CorridorPoi, MAX_CORRIDOR_RESULTS>::new();
    let path: &dyn RoutePath = &route;
    reader.corridor_pois(cats, path, progress_m, &mut out).expect("corridor query");
    out.into_iter().collect()
}

/// A named Water POI (subtype 1) at `(lon, lat)` in µdeg.
fn water(name: &str, lon: i32, lat: i32) -> PoiSpec {
    PoiSpec { lat, lon, subtype: 1, name: name.into(), hours_ref: 0xFFFF }
}

/// The seam reports the resident chunk index: chunk count, non-decreasing starts, and the last
/// chunk's start below the route total. (`chunk_start_m` past the end answers "the route end", the
/// contract the query's chunk-extent arithmetic leans on.)
#[test]
fn route_reader_exposes_its_chunk_index_as_a_path() {
    let obcr = convert("East", &gpx(""));
    let src = SliceSource(&obcr);
    let idx = RouteIndex::read(&src).unwrap();
    let route = RouteReader::new(&idx, &src);
    let path: &dyn RoutePath = &route;

    assert!(path.chunk_count() >= 1);
    assert_eq!(path.chunk_start_m(0), 0, "the first chunk starts at the route origin");
    for k in 1..path.chunk_count() {
        assert!(path.chunk_start_m(k) >= path.chunk_start_m(k - 1), "non-decreasing along the route");
    }
    assert_eq!(
        path.chunk_start_m(path.chunk_count()),
        route.total_distance_m,
        "past the last chunk the seam answers with the route end"
    );

    // Every chunk decodes to at least two points, all inside its advertised bbox.
    for k in 0..path.chunk_count() {
        let bbox = path.chunk_bbox(k);
        let mut seen = 0usize;
        path.visit_chunk_points(k, &mut |pts| {
            seen = pts.len();
            for p in pts {
                assert!(p.0 >= bbox.min_lon && p.0 <= bbox.max_lon, "point inside the chunk bbox");
                assert!(p.1 >= bbox.min_lat && p.1 <= bbox.max_lat);
            }
        });
        assert!(seen >= 2, "chunk {k} decoded {seen} points");
    }
}

/// **The axis pin.** A `<wpt>` and a map POI at the identical coordinate must land at the same
/// along-route distance: the corridor query projects onto the same ruler the converter placed the
/// waypoint on, so U3 can sort the two into one route-ordered timeline.
#[test]
fn a_corridor_poi_and_a_waypoint_at_the_same_spot_agree_on_distance() {
    // Sit both exactly on the 12th track point (7.8000 + 0.0020×11 = 7.8220° E).
    let lon_deg = 7.8000 + 0.0020 * 11.0;
    let wpt = format!(r#"<wpt lat="48.0000" lon="{lon_deg:.4}"><name>Spring</name></wpt>"#);
    let obcr = convert("East", &gpx(&wpt));
    let map = build_poi_map(BBOX, CS, &[(1, vec![water("Spring", (lon_deg * 1e6) as i32, 48_000_000)])]);

    let src = SliceSource(&obcr);
    let idx = RouteIndex::read(&src).unwrap();
    let route = RouteReader::new(&idx, &src);
    let stored = route.load_waypoints(0);
    assert_eq!(stored.entries.len(), 1, "the converter placed the waypoint");
    let wpt_along = stored.entries[0].dist_along_m;

    let got = query(&map, &obcr, PoiCategorySet::ALL, 0);
    assert_eq!(got.len(), 1, "one POI beside the route");
    assert!(
        got[0].dist_along_m.abs_diff(wpt_along) <= 2,
        "corridor {} vs waypoint {} — the two must share the route axis",
        got[0].dist_along_m,
        wpt_along
    );
    assert_eq!(got[0].offset_m, 0, "a POI on the line has no lateral offset");
}

/// Over a real route the side of travel still reads the same way: north of an eastbound route is
/// the rider's left (negative), south is the right (positive), and only what is inside the corridor
/// and ahead of progress comes back.
#[test]
fn sides_corridor_and_progress_hold_over_a_real_route() {
    let obcr = convert("East", &gpx(""));
    // 1000 µdeg of latitude ≈ 111 m; 4000 ≈ 445 m (outside the 300 m corridor).
    let map = build_poi_map(
        BBOX,
        CS,
        &[(
            1,
            vec![
                water("Left", 7_810_000, 48_001_000),
                water("Right", 7_830_000, 47_999_000),
                water("Too far", 7_845_000, 48_004_000),
            ],
        )],
    );

    let got = query(&map, &obcr, PoiCategorySet::ALL, 0);
    assert_eq!(got.iter().map(|c| c.poi.name.as_str()).collect::<Vec<_>>(), ["Left", "Right"]);
    assert!(got[0].offset_m < 0 && got[1].offset_m > 0, "left is negative, right positive");
    assert!(got.windows(2).all(|w| w[0].dist_along_m <= w[1].dist_along_m), "route-ordered");

    // Ride past the first: only the one still ahead survives.
    let ahead = query(&map, &obcr, PoiCategorySet::ALL, got[0].dist_along_m + 1);
    assert_eq!(ahead.iter().map(|c| c.poi.name.as_str()).collect::<Vec<_>>(), ["Right"]);

    // A category the map doesn't carry is a valid empty answer over a real route too.
    assert!(query(&map, &obcr, PoiCategorySet::only(PoiCategory::Pharmacy), 0).is_empty());
}
