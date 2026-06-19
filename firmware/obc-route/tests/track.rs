//! Recorded-track tests: the fixed-record encode/decode roundtrip and the GPX export
//! (coordinate formatting, `<trkseg>` splitting on `segment_start`, well-formedness).

use obc_route::{
    decode_record, encode_record, track_to_gpx, ByteSink, Error, SliceSource, TrackPoint,
    TRACK_RECORD_LEN,
};

/// A `ByteSink` over a growable `Vec` (mirrors the matcher/profile test backings).
#[derive(Default)]
struct VecSink {
    buf: Vec<u8>,
}
impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

#[test]
fn record_roundtrip() {
    for p in [
        TrackPoint { lon: -7_654_321, lat: 47_123_456, ele: 812, t_ms: 0, segment_start: true },
        TrackPoint { lon: 13_404_954, lat: -8_000_001, ele: -42, t_ms: 1_234_567, segment_start: false },
        TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: u32::MAX, segment_start: true },
    ] {
        assert_eq!(decode_record(&encode_record(&p)), p);
    }
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
    let pts = [
        TrackPoint { lon: 7_842_000, lat: 47_995_000, ele: 300, t_ms: 0, segment_start: true },
        TrackPoint { lon: 7_843_500, lat: 47_996_000, ele: 305, t_ms: 1000, segment_start: false },
    ];
    let gpx = to_gpx(&log_of(&pts), "Kandel");

    assert!(gpx.starts_with("<?xml"), "xml prolog");
    assert!(gpx.trim_end().ends_with("</gpx>"), "closes gpx");
    assert!(gpx.contains("<trk><name>Kandel</name>"));
    // GPX attribute order is lat then lon, fixed 6-decimal degrees.
    assert!(gpx.contains("<trkpt lat=\"47.995000\" lon=\"7.842000\"><ele>300</ele></trkpt>"));
    assert!(gpx.contains("<trkpt lat=\"47.996000\" lon=\"7.843500\"><ele>305</ele></trkpt>"));
    // No fabricated timestamps until a clock exists.
    assert!(!gpx.contains("<time>"));
    // One segment (only the first point starts one).
    assert_eq!(gpx.matches("<trkseg>").count(), 1);
}

#[test]
fn gpx_splits_segments_on_pause() {
    // A pause/gap mid-ride sets the next point's segment_start → a fresh <trkseg>.
    let pts = [
        TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: 0, segment_start: true },
        TrackPoint { lon: 1000, lat: 1000, ele: 1, t_ms: 1, segment_start: false },
        TrackPoint { lon: 2000, lat: 2000, ele: 2, t_ms: 2, segment_start: true },
        TrackPoint { lon: 3000, lat: 3000, ele: 3, t_ms: 3, segment_start: false },
    ];
    let gpx = to_gpx(&log_of(&pts), "ride");
    assert_eq!(gpx.matches("<trkseg>").count(), 2);
    assert_eq!(gpx.matches("</trkseg>").count(), 2);
    assert_eq!(gpx.matches("<trkpt").count(), 4);
}

#[test]
fn gpx_handles_negative_degrees_and_escapes_name() {
    let pts = [TrackPoint { lon: -122_419_400, lat: -37_774_900, ele: 0, t_ms: 0, segment_start: true }];
    let gpx = to_gpx(&log_of(&pts), "a < b & c");
    assert!(gpx.contains("lat=\"-37.774900\" lon=\"-122.419400\""));
    assert!(gpx.contains("<name>a &lt; b &amp; c</name>"));
}

#[test]
fn gpx_ignores_trailing_partial_record() {
    // A power-loss mid-write leaves a partial record; the log stays valid to the boundary.
    let mut log = log_of(&[TrackPoint { lon: 5, lat: 6, ele: 7, t_ms: 8, segment_start: true }]);
    log.extend_from_slice(&[0xAB; TRACK_RECORD_LEN - 3]); // a truncated trailing record
    let gpx = to_gpx(&log, "partial");
    assert_eq!(gpx.matches("<trkpt").count(), 1);
}
