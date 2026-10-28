//! Recorded-track GPX export tests: coordinate formatting, sensor extensions, well-formedness.
//! The byte layout and the segment split are pinned by the shared track vectors.

use obc_formats::io::SliceSource;
use obc_formats::ride::{encode_footer, Footer};
use obc_formats::track::{encode_record, RECORD_LEN as TRACK_RECORD_LEN};
use obc_ports::TrackPoint;
use obc_route::track_to_gpx;

use crate::common::VecSink;

/// A `TrackPoint` with no sensor values, for the tests where sensors are irrelevant.
fn pt(lon: i32, lat: i32, ele: i16, t_ms: u32, segment_start: bool) -> TrackPoint {
    TrackPoint { lon, lat, ele, t_ms, segment_start, hr: None, cadence: None, power: None }
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
    assert_eq!(log.len() % TRACK_RECORD_LEN, 0);
    let mut ride = log.to_vec();
    ride.extend_from_slice(&encode_footer(&Footer::new(
        name,
        1_700_000_000,
        0,
        0,
        0,
        0,
        (log.len() / TRACK_RECORD_LEN) as u32,
        None,
        None,
        None,
        None,
        None,
    )));
    let mut sink = VecSink::default();
    track_to_gpx(&SliceSource(&ride), name, &mut sink).unwrap();
    String::from_utf8(sink.buf).unwrap()
}

#[test]
fn gpx_coords_and_structure() {
    let pts = [pt(7_842_000, 47_995_000, 300, 0, true), pt(7_843_500, 47_996_000, 305, 1000, false)];
    let gpx = to_gpx(&log_of(&pts), "Kandel");

    assert!(gpx.starts_with("<?xml"), "xml prolog");
    assert!(gpx.trim_end().ends_with("</gpx>"), "closes gpx");
    assert!(gpx.contains("<trk><name>Kandel</name>"));
    // The root element declares the Garmin TrackPointExtension namespace.
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
fn gpx_handles_negative_degrees_and_escapes_name() {
    let pts = [pt(-122_419_400, -37_774_900, 0, 0, true)];
    let gpx = to_gpx(&log_of(&pts), "a < b & c");
    assert!(gpx.contains("lat=\"-37.774900\" lon=\"-122.419400\""));
    assert!(gpx.contains("<name>a &lt; b &amp; c</name>"));
}

#[test]
fn gpx_rejects_the_retired_unfinished_sample_log() {
    let mut log = log_of(&[pt(5, 6, 7, 8, true)]);
    log.extend_from_slice(&[0xAB; TRACK_RECORD_LEN - 3]);
    let mut sink = VecSink::default();
    assert!(track_to_gpx(&SliceSource(&log), "partial", &mut sink).is_err());
}

#[test]
fn gpx_reads_finished_samples_and_skips_footer() {
    let points = [pt(5, 6, 7, 8, true), pt(15, 16, 17, 18, false)];
    let mut ride = log_of(&points);
    ride.extend_from_slice(&encode_footer(&Footer::new(
        "finished",
        1_700_000_000,
        1,
        2,
        3,
        4,
        2,
        None,
        None,
        None,
        None,
        None,
    )));
    let mut sink = VecSink::default();
    track_to_gpx(&SliceSource(&ride), "finished", &mut sink).unwrap();
    let gpx = String::from_utf8(sink.buf).unwrap();
    assert_eq!(gpx.matches("<trkpt").count(), 2, "the footer never becomes fake points");
    assert!(gpx.contains("lat=\"0.000006\" lon=\"0.000005\""));
    assert!(gpx.contains("lat=\"0.000016\" lon=\"0.000015\""));
}

/// Each element is omitted where its field is absent, but the wrapper still appears when any
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
