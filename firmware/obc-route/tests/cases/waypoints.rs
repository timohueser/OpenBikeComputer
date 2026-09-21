//! OBCR waypoint-section tests: the hand-built format contract from `OBCR_Spec.md`, the converter's
//! `<wpt>` emission (category from `<sym>`/`<type>`, signed lateral offset), and the storage-only
//! guarantee that a waypoint-bearing route rides identically to the same route without waypoints.

use obc_formats::io::SliceSource;
use obc_formats::obcr::{WAYPOINT_ELE_NONE, WAYPOINT_LEN};
use obc_reader::PoiCategory;
use obc_route::{for_each_waypoint, gpx_to_obcr, RouteIndex, RouteReader, Waypoint, Waypoints, MAX_WAYPOINTS};

use crate::common::{build_obcr, convert, decode, ChunkIn, IndexPlacement, RouteSpec, VecSink, WpRec};

/// Collect every stored waypoint of an `.obcr` byte buffer.
fn waypoints(bytes: &[u8]) -> Vec<Waypoint> {
    let src = SliceSource(bytes);
    let mut out = Vec::new();
    let count = for_each_waypoint(&src, |w| out.push(w.clone())).unwrap();
    assert_eq!(count as usize, out.len());
    out
}

/// Build a `.obcr` by hand, mirroring the spec's byte layout independently of the converter: a
/// 128-byte header, one 2-point chunk, the index, then the waypoint table.
fn v3_route(wps: &[WpRec]) -> Vec<u8> {
    build_obcr(&RouteSpec {
        name: "Alps",
        chunks: &[ChunkIn {
            points: vec![(1_000, 2_000, 100), (1_500, 2_500, 110)],
            cum_distance_m: 0,
            cum_ascent_m: 0,
        }],
        totals: (70, 10, 0),
        index: IndexPlacement::AfterData,
        waypoints: Some(wps),
        ..Default::default()
    })
    .0
}

/// A forged `Waypoint Offset` near `u32::MAX` must read as a truncated file, and must not wrap the
/// per-record offset arithmetic back inside the buffer. Browser-supplied bytes reach this walk, so
/// the header is untrusted.
#[test]
fn a_forged_waypoint_offset_near_u32_max_reads_as_truncation() {
    for forged in [u32::MAX, u32::MAX - WAYPOINT_LEN as u32 + 1, u32::MAX - 43] {
        let mut bytes =
            v3_route(&[(0, 1_000, 2_010, 213, 1, 8, 0, b"Fountain"), (70, 1_490, 2_500, 0, 0, 6, 0, b"Summit")]);
        bytes[112..116].copy_from_slice(&forged.to_le_bytes()); // the header's Waypoint Offset
        let err = for_each_waypoint(&SliceSource(&bytes), |_| panic!("no forged record may decode"))
            .expect_err("a wrapped offset must be an error");
        assert_eq!(err, obc_formats::io::Error::BadOffset, "offset {forged:#x}");
    }
}

#[test]
fn record_contract_reads_every_field() {
    let bytes = v3_route(&[
        (0, 1_000, 2_010, 213, 1, 8, -320, b"Fountain"),
        (70, 1_490, 2_500, WAYPOINT_ELE_NONE, 0, 6, 0, b"Summit"),
    ]);

    // The ride path parses the file without touching the waypoint section.
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert_eq!(r.name(), "Alps");
    assert_eq!(r.point_count, 2);
    assert_eq!(decode(&r, 0).len(), 2);

    let wps = waypoints(&bytes);
    assert_eq!(wps.len(), 2);
    assert_eq!(wps[0].name, "Fountain");
    assert_eq!(
        (wps[0].dist_along_m, wps[0].lon, wps[0].lat, wps[0].ele, wps[0].category_id, wps[0].lateral_offset_m),
        (0, 1_000, 2_010, 213, 1, -320)
    );
    assert_eq!(wps[0].category(), Some(PoiCategory::Water));
    assert_eq!(wps[1].name, "Summit");
    assert_eq!(wps[1].ele, WAYPOINT_ELE_NONE);
    assert_eq!((wps[1].category(), wps[1].lateral_offset_m), (None, 0));
}

/// The v3 bump is breaking on purpose: the same bytes labelled v1 or v2 are rejected outright, so
/// a stored pre-v3 route re-imports from GPX instead of mis-decoding.
#[test]
fn pre_v3_routes_are_rejected() {
    let bytes = v3_route(&[(0, 1_000, 2_010, 213, 1, 8, 0, b"Fountain")]);
    for old in [1u8, 2] {
        let mut stale = bytes.clone();
        stale[4] = old;
        let src = SliceSource(&stale);
        assert!(RouteIndex::read(&src).is_err(), "v{old} must not load");
        assert!(for_each_waypoint(&src, |_| panic!("v{old} must not decode records")).is_err());
    }
}

/// A category byte outside the wire ids reads as generic, and the raw byte stays available for a
/// rewrite to carry through.
#[test]
fn unknown_category_byte_reads_as_generic() {
    let bytes = v3_route(&[(0, 1_000, 2_000, 100, 42, 2, 0, b"ok")]);
    let wps = waypoints(&bytes);
    assert_eq!((wps[0].category_id, wps[0].category()), (42, None));
}

#[test]
fn lying_name_len_is_clamped_not_overrun() {
    let bytes = v3_route(&[(0, 1_000, 2_000, 100, 0, 200, 0, b"ok")]);
    let wps = waypoints(&bytes);
    assert_eq!(wps.len(), 1);
    // Clamped to the 24-byte record capacity: the 2 real bytes + zero padding.
    assert!(wps[0].name.as_bytes().starts_with(b"ok"));
    assert!(wps[0].name.len() <= 24);
}

/// A straight eastward track at 48°N (~670 m), with two waypoints listed out of ride order, as
/// GPX carries them.
const WPT_GPX: &str = r#"<?xml version="1.0"?>
<gpx>
  <wpt lat="48.0000" lon="7.8090"><name>Summit Cafe</name></wpt>
  <wpt lat="48.0002" lon="7.8000"><ele>212.0</ele><name>Start Fountain</name></wpt>
  <trk><trkseg>
    <trkpt lat="48.0000" lon="7.8000"><ele>200.0</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8030"><ele>210.0</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8060"><ele>225.0</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8090"><ele>215.0</ele></trkpt>
  </trkseg></trk></gpx>"#;

fn strip_wpts(gpx: &str) -> String {
    let mut out = String::new();
    for line in gpx.lines() {
        if !line.trim_start().starts_with("<wpt ") {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

#[test]
fn converter_places_and_sorts_waypoints() {
    let src = SliceSource(WPT_GPX.as_bytes());
    let mut sink = VecSink::default();
    let stats = gpx_to_obcr(&src, "Rhine Path", &mut sink).unwrap();
    assert_eq!(stats.waypoint_count, 2);

    let wps = waypoints(&sink.buf);
    assert_eq!(wps.len(), 2);
    // Sorted into ride order; the GPX listed them reversed.
    assert_eq!(wps[0].name, "Start Fountain");
    assert_eq!(wps[0].dist_along_m, 0);
    assert_eq!(wps[0].ele, 212);
    assert_eq!((wps[0].lon, wps[0].lat), (7_800_000, 48_000_200));
    assert_eq!(wps[1].name, "Summit Cafe");
    assert_eq!(wps[1].dist_along_m, stats.total_distance_m);
    assert_eq!(wps[1].ele, WAYPOINT_ELE_NONE);
    assert_eq!(wps[1].category_id, 0); // neither <wpt> carries a symbol
}

/// One `<wpt>` with the given inner XML, on a straight eastward track, converted and read back.
fn convert_one_wpt(inner: &str) -> Waypoint {
    let gpx = format!(
        r#"<gpx>
  <wpt lat="48.0000" lon="7.8030">{inner}</wpt>
  <trk><trkseg>
    <trkpt lat="48.0000" lon="7.8000"><ele>200</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8030"><ele>210</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8060"><ele>220</ele></trkpt>
  </trkseg></trk></gpx>"#
    );
    let bytes = convert("Symbols", &gpx);
    waypoints(&bytes).pop().expect("one waypoint")
}

#[test]
fn converter_maps_sym_and_type_onto_categories() {
    // `<sym>`: the Garmin-style spelling most planners copy.
    assert_eq!(convert_one_wpt("<name>Brunnen</name><sym>Water</sym>").category(), Some(PoiCategory::Water));
    // `<type>`: RideWithGPS and Komoot write the class here instead.
    assert_eq!(convert_one_wpt("<name>Camp</name><type>Campground</type>").category(), Some(PoiCategory::Campsite));
    assert_eq!(
        convert_one_wpt("<name>Shop</name><sym>BICYCLE_SHOP</sym>").category(),
        Some(PoiCategory::BikeShop),
        "matching is case- and separator-insensitive"
    );
    assert_eq!(convert_one_wpt("<name>Turn</name><sym>Flag, Blue</sym>").category(), None);
    assert_eq!(convert_one_wpt("<name>Turn</name>").category(), None);
    assert_eq!(convert_one_wpt("<name>Turn</name><sym>Flag, Blue</sym>").name, "Turn", "never dropped for a symbol");
}

/// `<sym>` wins over `<type>` when it says something; an empty `<sym>` falls through to `<type>`.
#[test]
fn sym_takes_precedence_over_type() {
    let both = convert_one_wpt("<name>Both</name><sym>Water</sym><type>Campground</type>");
    assert_eq!(both.category(), Some(PoiCategory::Water));
    let empty_sym = convert_one_wpt("<name>Empty</name><sym></sym><type>Campground</type>");
    assert_eq!(empty_sym.category(), Some(PoiCategory::Campsite));
}

/// A GPX with one `<wpt>` at `(lat, lon)` beside a straight eastward track at 48°N.
fn offset_of(lat: f64, lon: f64) -> i16 {
    let gpx = format!(
        r#"<gpx>
  <wpt lat="{lat:.6}" lon="{lon:.6}"><name>W</name></wpt>
  <trk><trkseg>
    <trkpt lat="48.0000" lon="7.8000"><ele>200</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8030"><ele>200</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8060"><ele>200</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8090"><ele>200</ele></trkpt>
  </trkseg></trk></gpx>"#
    );
    let bytes = convert("Offsets", &gpx);
    waypoints(&bytes).pop().expect("one waypoint").lateral_offset_m
}

/// Riding east, north is left (negative) and south is right (positive). One µdeg of latitude is
/// ~0.111 m, so 0.0002° is ~22 m.
#[test]
fn converter_signs_the_lateral_offset_by_side_of_travel() {
    let north = offset_of(48.0002, 7.8060);
    let south = offset_of(47.9998, 7.8060);
    assert_eq!(north, -22, "north of an eastward track is to the left");
    assert_eq!(south, 22, "south of an eastward track is to the right");
    assert_eq!(offset_of(48.0000, 7.8060), 0, "a waypoint on the line is on-route");
}

/// The track's first point has no incoming segment, so the sign resolves from the outgoing one.
#[test]
fn offset_at_the_first_track_point_still_takes_a_side() {
    assert_eq!(offset_of(48.0002, 7.8000), -22, "beside the start, north = left");
    assert_eq!(offset_of(47.9998, 7.8000), 22, "beside the start, south = right");
}

/// The magnitude saturates instead of wrapping, so a waypoint far off route reads as very far,
/// never as the opposite side.
#[test]
fn far_offsets_saturate() {
    let far = offset_of(48.5000, 7.8060); // ~55 km north of the track
    assert_eq!(far, i16::MIN + 1, "clamped to the i16 range, still on the left");
}

#[test]
fn converter_truncates_names_on_char_boundaries() {
    let gpx = r#"<gpx>
  <wpt lat="48.0" lon="7.8"><name>A Very Long Waypoint Name Here</name></wpt>
  <wpt lat="48.0" lon="7.8"><name>12345678901234567890123ä</name></wpt>
  <trk><trkseg>
    <trkpt lat="48.0000" lon="7.8000"><ele>200</ele></trkpt>
    <trkpt lat="48.0000" lon="7.8090"><ele>200</ele></trkpt>
  </trkseg></trk></gpx>"#;
    let bytes = convert("Names", gpx);
    let wps = waypoints(&bytes);
    // 24-byte cap; the split "ä" (byte 24 would be mid-char) is dropped whole.
    assert_eq!(wps[0].name, "A Very Long Waypoint Nam");
    assert_eq!(wps[1].name, "12345678901234567890123");
}

#[test]
fn converter_caps_waypoint_count() {
    let mut gpx = String::from("<gpx>");
    for k in 0..MAX_WAYPOINTS + 8 {
        gpx.push_str(&format!(r#"<wpt lat="48.0" lon="7.8"><name>W{k}</name></wpt>"#));
    }
    gpx.push_str(
        r#"<trk><trkseg>
        <trkpt lat="48.0000" lon="7.8000"><ele>200</ele></trkpt>
        <trkpt lat="48.0000" lon="7.8090"><ele>200</ele></trkpt>
    </trkseg></trk></gpx>"#,
    );
    let bytes = convert("Cap", &gpx);
    assert_eq!(waypoints(&bytes).len(), MAX_WAYPOINTS);
}

#[test]
fn waypoint_bearing_route_rides_identically() {
    let with = convert("Rhine Path", WPT_GPX);
    let without = convert("Rhine Path", &strip_wpts(WPT_GPX));

    assert_eq!(waypoints(&with).len(), 2);
    assert_eq!(waypoints(&without).len(), 0);
    // No waypoints ⇒ the extension is all zeros.
    assert_eq!(&without[112..118], &[0u8; 6]);

    let src_w = SliceSource(&with);
    let src_o = SliceSource(&without);
    let idx_w = RouteIndex::read(&src_w).unwrap();
    let idx_o = RouteIndex::read(&src_o).unwrap();

    assert_eq!(idx_w.name(), idx_o.name());
    assert_eq!(
        (idx_w.point_count, idx_w.total_distance_m, idx_w.total_ascent_m),
        (idx_o.point_count, idx_o.total_distance_m, idx_o.total_ascent_m,)
    );
    assert_eq!(idx_w.chunks().len(), idx_o.chunks().len());
    let (r_w, r_o) = (RouteReader::new(&idx_w, &src_w), RouteReader::new(&idx_o, &src_o));
    for k in 0..idx_w.chunks().len() {
        assert_eq!(decode(&r_w, k), decode(&r_o, k), "chunk {k} diverged");
    }
}

// The resident `Waypoints` table (`RouteReader::load_waypoints`) is the waypoint UI's data layer:
// it distils the stored section into named, windowed, capped entries.

fn load(bytes: &[u8], min_dist_m: u32) -> Waypoints {
    let src = SliceSource(bytes);
    let idx = RouteIndex::read(&src).unwrap();
    RouteReader::new(&idx, &src).load_waypoints(min_dist_m)
}

fn names(w: &Waypoints) -> Vec<&str> {
    w.as_slice().iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn load_waypoints_drops_unnamed_and_whitespace_only() {
    // A real name, an empty name, an all-spaces name, a tab+space name, another real name.
    let bytes = v3_route(&[
        (0, 1_000, 2_000, 100, 2, 8, -60, b"Fountain"),
        (10, 1_000, 2_000, 100, 0, 0, 0, b""),
        (20, 1_000, 2_000, 100, 0, 3, 0, b"   "),
        (30, 1_000, 2_000, 100, 0, 2, 0, b"\t "),
        (40, 1_000, 2_000, 100, 0, 6, 0, b"Summit"),
    ]);
    let w = load(&bytes, 0);
    assert_eq!(names(&w), ["Fountain", "Summit"]);
    assert!(!w.truncated);
    // The compact entry mirrors the record, except that `ele` is dropped.
    assert_eq!(w.as_slice()[0].dist_along_m, 0);
    assert_eq!((w.as_slice()[0].lon, w.as_slice()[0].lat), (1_000, 2_000));
    assert_eq!(w.as_slice()[0].category, Some(PoiCategory::Campsite));
    assert_eq!(w.as_slice()[0].lateral_offset_m, -60);
    assert_eq!((w.as_slice()[1].category, w.as_slice()[1].lateral_offset_m), (None, 0));
}

#[test]
fn load_waypoints_windows_by_min_dist() {
    let bytes = v3_route(&[
        (0, 1_000, 2_000, 100, 0, 1, 0, b"A"),
        (500, 1_000, 2_000, 100, 0, 1, 0, b"B"),
        (1_000, 1_000, 2_000, 100, 0, 1, 0, b"C"),
    ]);
    assert_eq!(names(&load(&bytes, 0)), ["A", "B", "C"]);
    // The boundary is inclusive: a window at 500 keeps B and C.
    assert_eq!(names(&load(&bytes, 500)), ["B", "C"]);
    let past = load(&bytes, 1_001);
    assert!(past.is_empty() && !past.truncated);
}

#[test]
fn load_waypoints_caps_first_by_distance_and_flags_truncation() {
    // MAX_WAYPOINTS + 5 named waypoints at strictly increasing distance. Only a non-empty name
    // matters to the cap.
    let recs: Vec<WpRec> =
        (0..(MAX_WAYPOINTS + 5) as u32).map(|k| (k * 100, 1_000, 2_000, 100, 0, 1, 0, b"w".as_slice())).collect();
    let bytes = v3_route(&recs);

    let w = load(&bytes, 0);
    assert_eq!(w.len(), MAX_WAYPOINTS);
    assert!(w.truncated, "an over-cap file must flag truncation");
    assert_eq!(w.as_slice().first().unwrap().dist_along_m, 0);
    assert_eq!(w.as_slice().last().unwrap().dist_along_m, (MAX_WAYPOINTS as u32 - 1) * 100);

    // Sliding the window forward re-captures the truncated remainder, which is what the app does
    // on exhaustion. Starting just past entry 4 drops 5, and the rest then fits.
    let slid = load(&bytes, 5 * 100);
    assert_eq!(slid.as_slice().first().unwrap().dist_along_m, 5 * 100);
    assert!(!slid.truncated, "the remaining tail now fits under the cap");
}

#[test]
fn load_waypoints_of_a_waypoint_free_route_is_empty() {
    let w = load(&v3_route(&[]), 0);
    assert!(w.is_empty() && !w.truncated);
}

/// Read the committed vector the way the shared vector tests reach it. It soft-skips when the
/// fixture is not reachable from the crate, so the unit contract above stays authoritative.
#[test]
fn load_waypoints_reads_the_committed_vector() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/route-waypoints.obcr");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {} not reachable", path.display());
        return;
    };
    let w = load(&bytes, 0);
    assert_eq!(names(&w), ["Brunnen", "Pass Summit"]);
    assert_eq!(w.as_slice()[0].dist_along_m, 0);
    assert!(w.as_slice()[1].dist_along_m > 0, "the summit sits mid-route");
    // The vector pins both halves of the symbol mapping: the fountain's `<sym>Drinking Water</sym>`
    // is Water, 13 m left of travel; the summit's `<type>Viewpoint</type>` is unmapped.
    assert_eq!((w.as_slice()[0].category, w.as_slice()[0].lateral_offset_m), (Some(PoiCategory::Water), -13));
    assert_eq!((w.as_slice()[1].category, w.as_slice()[1].lateral_offset_m), (None, 0));
    assert!(!w.truncated);
    assert_eq!(names(&load(&bytes, 1)), ["Pass Summit"]);
}
