//! Elevation-profile tests: convert a synthetic GPX, build the profile from the
//! reader, and check it captures the route's shape — the peak, the y-range, and a
//! gap-free band — independent of how sparsely the route samples the columns.

use obcm_route::{gpx_to_obcr, ByteSink, Error, RouteReader, SliceSource, PROFILE_COLS};

/// A `ByteSink` over a growable `Vec` (the host's in-RAM file backing).
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

fn convert(name: &str, gpx: &str) -> Vec<u8> {
    let src = SliceSource(gpx.as_bytes());
    let mut sink = VecSink::default();
    gpx_to_obcr(&src, name, &mut sink).unwrap();
    sink.buf
}

/// A zigzag (so no point decimates away) that climbs 200→300 m then falls back to
/// 200 m — a single clean peak at the route's midpoint by distance.
const PEAKED: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"><ele>200.0</ele></trkpt>
  <trkpt lat="48.0020" lon="7.8030"><ele>250.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8060"><ele>300.0</ele></trkpt>
  <trkpt lat="48.0020" lon="7.8090"><ele>250.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8120"><ele>200.0</ele></trkpt>
</trkseg></trk></gpx>"#;

#[test]
fn profile_captures_peak_and_range() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let r = RouteReader::open(&src).unwrap();
    let p = r.elevation_profile();

    // Y-range mirrors the route header.
    assert_eq!((p.min_ele_m, p.max_ele_m), (r.min_ele_m, r.max_ele_m));
    assert_eq!((p.min_ele_m, p.max_ele_m), (200, 300));

    // The 300 m peak survives and lands near the middle (distance ~0.5).
    assert_eq!(p.peak_ele_m(), 300);
    assert!(
        (96..=160).contains(&p.peak_col),
        "peak_col {} not near the middle",
        p.peak_col
    );

    // Scrubbing: the ends are below the peak, the middle is the peak.
    assert!(p.at(0.0).1 < 300, "start should be below the peak");
    assert!(p.at(1.0).1 < 300, "end should be below the peak");
    assert_eq!(p.at(0.5).1, 300, "midpoint should be the peak");
}

#[test]
fn profile_band_is_gap_free() {
    // Five points fill at most five columns directly; the other ~250 are gaps the
    // builder must carry-fill so the band has no sentinel (min > max) holes.
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let r = RouteReader::open(&src).unwrap();
    let p = r.elevation_profile();

    assert_eq!(p.cols().len(), PROFILE_COLS);
    for (i, &(mn, mx)) in p.cols().iter().enumerate() {
        assert!(mn <= mx, "column {i} left unfilled ({mn} > {mx})");
        assert!((200..=300).contains(&mn) && (200..=300).contains(&mx));
    }
}

#[test]
fn ascent_to_interpolates_climb() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let r = RouteReader::open(&src).unwrap();

    // Endpoints pin to 0 and the route total; the middle lands strictly between.
    assert_eq!(r.ascent_to(0), 0);
    assert_eq!(r.ascent_to(r.total_distance_m), r.total_ascent_m);
    assert_eq!(r.ascent_to(r.total_distance_m * 10), r.total_ascent_m); // clamped past the end
    let mid = r.ascent_to(r.total_distance_m / 2);
    assert!(mid > 0 && mid < r.total_ascent_m, "mid ascent {mid} not between 0 and total");
}

/// A flat route: constant elevation, collinear-in-plan, so it decimates hard. The
/// profile must still produce a gap-free band with a sane (zero-height) range.
const FLAT: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"><ele>150.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8050"><ele>150.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8100"><ele>150.0</ele></trkpt>
</trkseg></trk></gpx>"#;

#[test]
fn flat_route_has_flat_gap_free_band() {
    let bytes = convert("Towpath", FLAT);
    let src = SliceSource(&bytes);
    let r = RouteReader::open(&src).unwrap();
    let p = r.elevation_profile();

    assert_eq!((p.min_ele_m, p.max_ele_m), (150, 150));
    for &(mn, mx) in p.cols() {
        assert_eq!((mn, mx), (150, 150));
    }
}
