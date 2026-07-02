//! OBCR v2 waypoint-extension tests (issue #268): the hand-built format contract
//! (`OBCR_Spec.md` §1.1 / §4), the converter's `<wpt>` emission, and the storage-only
//! guarantee — a waypoint-bearing route loads and rides **identically** to the same
//! route without waypoints.

use obc_route::{
    for_each_waypoint, gpx_to_obcr, RouteIndex, RouteReader, SliceSource, Waypoint, HEADER_V2_LEN, MAX_WAYPOINTS,
    WAYPOINT_ELE_NONE, WAYPOINT_LEN,
};

mod common;
use common::{convert, decode, VecSink};

/// Collect every stored waypoint of an `.obcr` byte buffer.
fn waypoints(bytes: &[u8]) -> Vec<Waypoint> {
    let src = SliceSource(bytes);
    let mut out = Vec::new();
    let count = for_each_waypoint(&src, |w| out.push(w.clone())).unwrap();
    assert_eq!(count as usize, out.len());
    out
}

/// A waypoint record to hand-encode: `(dist_along_m, lon, lat, ele, kind, name_len,
/// name_bytes)` — `name_len` passed explicitly so a test can lie with it.
type WpRec<'a> = (u32, i32, i32, i16, u8, u8, &'a [u8]);

/// Build a v2 `.obcr` by hand, mirroring the spec's byte layout independently of the
/// converter (the format.rs philosophy): a 128-byte header, one 2-point chunk, the
/// index, then the waypoint table.
fn v2_route(wps: &[WpRec]) -> Vec<u8> {
    // Geometry: (1000, 2000, 100) → (1500, 2500, 110), one chunk.
    let data_offset = HEADER_V2_LEN as u32;
    let index_offset = data_offset + 6; // one 6-byte delta record
    let wpt_offset = index_offset + 44;

    let mut f: Vec<u8> = Vec::new();
    f.extend_from_slice(b"OBCR");
    f.push(2); // version
    f.push(0); // flags
    f.push(4); // name len
    f.push(0); // reserved
    for v in [1_000i32, 2_000, 1_500, 2_500, 1_000, 2_000] {
        f.extend_from_slice(&v.to_le_bytes()); // bbox min/max + start
    }
    f.extend_from_slice(&2u32.to_le_bytes()); // point count
    f.extend_from_slice(&70u32.to_le_bytes()); // total distance
    f.extend_from_slice(&10u32.to_le_bytes()); // ascent
    f.extend_from_slice(&0u32.to_le_bytes()); // descent
    f.extend_from_slice(&100i16.to_le_bytes()); // min ele
    f.extend_from_slice(&110i16.to_le_bytes()); // max ele
    f.extend_from_slice(&1u32.to_le_bytes()); // chunk count
    f.extend_from_slice(&index_offset.to_le_bytes());
    f.extend_from_slice(&data_offset.to_le_bytes());
    let mut name_field = [0u8; 48];
    name_field[..4].copy_from_slice(b"Alps");
    f.extend_from_slice(&name_field);
    // v2 extension (§1.1): waypoint table offset + count + 10 reserved bytes.
    f.extend_from_slice(&wpt_offset.to_le_bytes());
    f.extend_from_slice(&(wps.len() as u16).to_le_bytes());
    f.extend_from_slice(&[0u8; 10]);
    assert_eq!(f.len(), HEADER_V2_LEN, "v2 header must be 128 bytes");

    // Chunk 0 data: the anchor is implicit; one delta record to the second point.
    f.extend_from_slice(&500i16.to_le_bytes());
    f.extend_from_slice(&500i16.to_le_bytes());
    f.extend_from_slice(&110i16.to_le_bytes());

    // ChunkMeta.
    for v in [1_000i32, 2_000, 1_500, 2_500, 1_000, 2_000] {
        f.extend_from_slice(&v.to_le_bytes()); // bbox + anchor
    }
    f.extend_from_slice(&100i16.to_le_bytes()); // anchor ele
    f.extend_from_slice(&2u16.to_le_bytes()); // point count
    f.extend_from_slice(&0u32.to_le_bytes()); // cum distance
    f.extend_from_slice(&0u32.to_le_bytes()); // cum ascent
    f.extend_from_slice(&data_offset.to_le_bytes());
    f.extend_from_slice(&6u32.to_le_bytes());

    // Waypoint records (§4): 40 bytes each.
    for &(along, lon, lat, ele, kind, name_len, name) in wps {
        let mut rec = [0u8; WAYPOINT_LEN];
        rec[0..4].copy_from_slice(&along.to_le_bytes());
        rec[4..8].copy_from_slice(&lon.to_le_bytes());
        rec[8..12].copy_from_slice(&lat.to_le_bytes());
        rec[12..14].copy_from_slice(&ele.to_le_bytes());
        rec[14] = kind;
        rec[15] = name_len;
        rec[16..16 + name.len()].copy_from_slice(name);
        f.extend_from_slice(&rec);
    }
    f
}

#[test]
fn v2_contract_reads_waypoints_and_rides_like_v1() {
    let bytes =
        v2_route(&[(0, 1_000, 2_010, 213, 1, 8, b"Fountain"), (70, 1_490, 2_500, WAYPOINT_ELE_NONE, 0, 6, b"Summit")]);

    // The ride path parses the v2 file through the untouched v1 code.
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert_eq!(r.name(), "Alps");
    assert_eq!(r.point_count, 2);
    assert_eq!(decode(&r, 0).len(), 2);

    let wps = waypoints(&bytes);
    assert_eq!(wps.len(), 2);
    assert_eq!(wps[0].name, "Fountain");
    assert_eq!((wps[0].dist_along_m, wps[0].lon, wps[0].lat, wps[0].ele, wps[0].kind), (0, 1_000, 2_010, 213, 1));
    assert_eq!(wps[1].name, "Summit");
    assert_eq!(wps[1].ele, WAYPOINT_ELE_NONE);

    // The identical bytes as version 1: loads the same, and the waypoint read
    // (correctly) sees nothing — v1 has no extension to trust.
    let mut v1 = bytes.clone();
    v1[4] = 1;
    let src = SliceSource(&v1);
    assert_eq!(RouteIndex::read(&src).unwrap().point_count, 2);
    assert_eq!(for_each_waypoint(&src, |_| panic!("v1 has no waypoints")).unwrap(), 0);
}

#[test]
fn lying_name_len_is_clamped_not_overrun() {
    let bytes = v2_route(&[(0, 1_000, 2_000, 100, 0, 200, b"ok")]);
    let wps = waypoints(&bytes);
    assert_eq!(wps.len(), 1);
    // Clamped to the 24-byte record capacity: the 2 real bytes + zero padding.
    assert!(wps[0].name.as_bytes().starts_with(b"ok"));
    assert!(wps[0].name.len() <= 24);
}

/// The convert-test STRAIGHT track (rolling eastward at 48°N, ~670 m), with two
/// out-of-ride-order waypoints ahead of the track — as GPX carries them.
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

/// The same GPX without its `<wpt>` elements.
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
    // Sorted into ride order (the GPX listed them reversed): the fountain sits by the
    // first track point (along 0), the cafe by the last (along = total distance).
    assert_eq!(wps[0].name, "Start Fountain");
    assert_eq!(wps[0].dist_along_m, 0);
    assert_eq!(wps[0].ele, 212);
    assert_eq!((wps[0].lon, wps[0].lat), (7_800_000, 48_000_200));
    assert_eq!(wps[1].name, "Summit Cafe");
    assert_eq!(wps[1].dist_along_m, stats.total_distance_m);
    assert_eq!(wps[1].ele, WAYPOINT_ELE_NONE);
    assert_eq!(wps[1].kind, 0);
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

    // Everything the ride path consumes is identical: summary fields, chunk metas,
    // and every decoded chunk.
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
