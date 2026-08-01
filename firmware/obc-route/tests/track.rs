//! Recorded-track tests: the fixed-record encode/decode roundtrip and the GPX export
//! (coordinate formatting, `<trkseg>` splitting on `segment_start`, well-formedness).

use obc_formats::io::SliceSource;
use obc_formats::track::{decode_record, encode_record};
use obc_formats::track::{
    CAD_NONE as TRACK_CAD_NONE, HR_NONE as TRACK_HR_NONE, PWR_NONE as TRACK_PWR_NONE, RECORD_LEN as TRACK_RECORD_LEN,
};
use obc_ports::TrackPoint;
use obc_route::track_to_gpx;

mod common;
use common::VecSink;

/// A `TrackPoint` with no sensor values — the pre-#707 shape, used where sensors are irrelevant.
fn pt(lon: i32, lat: i32, ele: i16, t_ms: u32, segment_start: bool) -> TrackPoint {
    TrackPoint { lon, lat, ele, t_ms, segment_start, hr: None, cadence: None, power: None }
}

#[test]
fn record_roundtrip() {
    // Mixed present/absent sensor fields, plus the plain no-sensor shape, all round-trip exactly.
    for p in [
        TrackPoint {
            lon: -7_654_321,
            lat: 47_123_456,
            ele: 812,
            t_ms: 0,
            segment_start: true,
            hr: Some(142),
            cadence: Some(85),
            power: Some(210),
        },
        TrackPoint {
            lon: 13_404_954,
            lat: -8_000_001,
            ele: -42,
            t_ms: 1_234_567,
            segment_start: false,
            hr: Some(60),
            cadence: None,
            power: Some(0),
        },
        TrackPoint {
            lon: 0,
            lat: 0,
            ele: 0,
            t_ms: u32::MAX,
            segment_start: true,
            hr: None,
            cadence: Some(0),
            power: None,
        },
        pt(1, 2, 3, 4, false),
    ] {
        assert_eq!(decode_record(&encode_record(&p)), p);
    }
}

/// The record is exactly 20 bytes and the sensor tail sits at the documented offsets, with the
/// sentinels encoding `None`. Pins the v2 wire layout the ride converter + iOS both mirror.
#[test]
fn record_is_20_bytes_with_sensor_tail() {
    assert_eq!(TRACK_RECORD_LEN, 20);
    let present = TrackPoint {
        lon: 0,
        lat: 0,
        ele: 0,
        t_ms: 0,
        segment_start: false,
        hr: Some(142),
        cadence: Some(85),
        power: Some(300),
    };
    let b = encode_record(&present);
    assert_eq!(b[16], 142, "hr u8 at offset 16");
    assert_eq!(b[17], 85, "cad u8 at offset 17");
    assert_eq!(u16::from_le_bytes([b[18], b[19]]), 300, "pwr u16 LE at 18..20");

    let absent = pt(0, 0, 0, 0, false);
    let b = encode_record(&absent);
    assert_eq!(b[16], TRACK_HR_NONE);
    assert_eq!(b[17], TRACK_CAD_NONE);
    assert_eq!(u16::from_le_bytes([b[18], b[19]]), TRACK_PWR_NONE);
}

/// Build a flat `.obct` log (concatenated records) from points.
fn log_of(pts: &[TrackPoint]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in pts {
        v.extend_from_slice(&encode_record(p));
    }
    v
}

fn to_gpx(log: &[u8], name: &str) -> String {
    let mut sink = VecSink::default();
    track_to_gpx(&SliceSource(log), name, &mut sink).unwrap();
    String::from_utf8(sink.buf).unwrap()
}

#[test]
fn gpx_coords_and_structure() {
    let pts = [pt(7_842_000, 47_995_000, 300, 0, true), pt(7_843_500, 47_996_000, 305, 1000, false)];
    let gpx = to_gpx(&log_of(&pts), "Kandel");

    assert!(gpx.starts_with("<?xml"), "xml prolog");
    assert!(gpx.trim_end().ends_with("</gpx>"), "closes gpx");
    assert!(gpx.contains("<trk><name>Kandel</name>"));
    // The root element declares the Garmin TrackPointExtension namespace (epic #707).
    assert!(gpx.contains("xmlns:gpxtpx=\"http://www.garmin.com/xmlschemas/TrackPointExtension/v1\""));
    // GPX attribute order is lat then lon, fixed 6-decimal degrees.
    assert!(gpx.contains("<trkpt lat=\"47.995000\" lon=\"7.842000\"><ele>300</ele></trkpt>"));
    assert!(gpx.contains("<trkpt lat=\"47.996000\" lon=\"7.843500\"><ele>305</ele></trkpt>"));
    // No sensor extensions when every point is sensor-free.
    assert!(!gpx.contains("<extensions>"));
    // No fabricated timestamps until a clock exists.
    assert!(!gpx.contains("<time>"));
    // One segment (only the first point starts one).
    assert_eq!(gpx.matches("<trkseg>").count(), 1);
}

#[test]
fn gpx_splits_segments_on_pause() {
    // A pause/gap mid-ride sets the next point's segment_start → a fresh <trkseg>.
    let pts =
        [pt(0, 0, 0, 0, true), pt(1000, 1000, 1, 1, false), pt(2000, 2000, 2, 2, true), pt(3000, 3000, 3, 3, false)];
    let gpx = to_gpx(&log_of(&pts), "ride");
    assert_eq!(gpx.matches("<trkseg>").count(), 2);
    assert_eq!(gpx.matches("</trkseg>").count(), 2);
    assert_eq!(gpx.matches("<trkpt").count(), 4);
}

#[test]
fn gpx_handles_negative_degrees_and_escapes_name() {
    let pts = [pt(-122_419_400, -37_774_900, 0, 0, true)];
    let gpx = to_gpx(&log_of(&pts), "a < b & c");
    assert!(gpx.contains("lat=\"-37.774900\" lon=\"-122.419400\""));
    assert!(gpx.contains("<name>a &lt; b &amp; c</name>"));
}

#[test]
fn gpx_ignores_trailing_partial_record() {
    // A power-loss mid-write leaves a partial record; the log stays valid to the boundary.
    let mut log = log_of(&[pt(5, 6, 7, 8, true)]);
    log.extend_from_slice(&[0xAB; TRACK_RECORD_LEN - 3]); // a truncated trailing record
    let gpx = to_gpx(&log, "partial");
    assert_eq!(gpx.matches("<trkpt").count(), 1);
}

/// A point with all three sensor fields emits the full extensions block: `gpxtpx:hr`/`gpxtpx:cad`
/// inside a TrackPointExtension wrapper, plus a bare `<power>` (the de-facto Strava form).
#[test]
fn gpx_emits_full_sensor_extensions() {
    let p = TrackPoint {
        lon: 7_842_000,
        lat: 47_995_000,
        ele: 300,
        t_ms: 0,
        segment_start: true,
        hr: Some(142),
        cadence: Some(85),
        power: Some(210),
    };
    let gpx = to_gpx(&log_of(&[p]), "Sensors");
    assert!(
        gpx.contains(
            "<trkpt lat=\"47.995000\" lon=\"7.842000\"><ele>300</ele><extensions>\
<gpxtpx:TrackPointExtension><gpxtpx:hr>142</gpxtpx:hr><gpxtpx:cad>85</gpxtpx:cad></gpxtpx:TrackPointExtension>\
<power>210</power></extensions></trkpt>"
        ),
        "got: {gpx}"
    );
}

/// Each element is omitted where its field is absent, but the wrapper still appears when *any*
/// TrackPointExtension field is present. Here only cadence and power are set.
#[test]
fn gpx_omits_absent_sensor_elements() {
    let p = TrackPoint {
        lon: 0,
        lat: 0,
        ele: 0,
        t_ms: 0,
        segment_start: true,
        hr: None,
        cadence: Some(90),
        power: Some(180),
    };
    let gpx = to_gpx(&log_of(&[p]), "Partial");
    assert!(gpx.contains("<extensions><gpxtpx:TrackPointExtension><gpxtpx:cad>90</gpxtpx:cad></gpxtpx:TrackPointExtension><power>180</power></extensions>"), "got: {gpx}");
    assert!(!gpx.contains("<gpxtpx:hr>"), "an absent hr emits no element");
}

/// Power-only: no TrackPointExtension wrapper at all (hr and cad both absent), just the bare
/// `<power>` inside `<extensions>`.
#[test]
fn gpx_power_only_omits_the_trackpoint_extension_wrapper() {
    let p =
        TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: 0, segment_start: true, hr: None, cadence: None, power: Some(250) };
    let gpx = to_gpx(&log_of(&[p]), "Power");
    assert!(gpx.contains("<ele>0</ele><extensions><power>250</power></extensions></trkpt>"), "got: {gpx}");
    assert!(!gpx.contains("gpxtpx:TrackPointExtension"), "no wrapper when hr+cad both absent");
}
