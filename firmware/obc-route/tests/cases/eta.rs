//! Time to go along real converted geometry. The estimate table itself is pinned by the shared
//! vector file (`specs/vectors/eta.csv`) in `obc-formats`.

use obc_formats::io::SliceSource;
use obc_route::{time_to_go_s, BikeType, RouteIndex, RouteReader};

use crate::common::convert;

/// A ~9 km pass, zigzagged so no point decimates away: 500 m to 800 m to 500 m, 300 m of ascent.
const PASS: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="47.0000" lon="8.0000"><ele>500.0</ele></trkpt>
  <trkpt lat="47.0020" lon="8.0200"><ele>600.0</ele></trkpt>
  <trkpt lat="47.0000" lon="8.0400"><ele>700.0</ele></trkpt>
  <trkpt lat="47.0020" lon="8.0600"><ele>800.0</ele></trkpt>
  <trkpt lat="47.0000" lon="8.0800"><ele>700.0</ele></trkpt>
  <trkpt lat="47.0020" lon="8.1000"><ele>600.0</ele></trkpt>
  <trkpt lat="47.0000" lon="8.1200"><ele>500.0</ele></trkpt>
</trkseg></trk></gpx>"#;

/// Convert `gpx`, then hand `(total_distance_m, total_ascent_m, Profile)` to `f`.
fn with_route<R>(name: &str, gpx: &str, f: impl FnOnce(u32, u32, &obc_route::Profile) -> R) -> R {
    let bytes = convert(name, gpx);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();
    f(r.total_distance_m, r.total_ascent_m, &p)
}

/// Time-to-go never increases as the rider advances, and it hits zero at and past the end. Both
/// terms are non-increasing in progress, so the readout can only count down. The pass fixture puts
/// all its climbing in the first half, which is where a non-monotonic implementation would show up.
#[test]
fn time_to_go_never_increases_along_the_route() {
    with_route("Pass", PASS, |total, ascent, p| {
        assert_eq!(ascent, 300);
        for bike in BikeType::ALL {
            let mut prev = u32::MAX;
            for step in 0..=200u32 {
                let progress = total * step / 200;
                let t = time_to_go_s(p, total, progress, bike);
                assert!(t <= prev, "{bike:?}: time-to-go rose from {prev} to {t} s at {progress} m");
                prev = t;
            }
            assert_eq!(time_to_go_s(p, total, total, bike), 0, "nothing left at the finish");
            assert_eq!(time_to_go_s(p, total, total + 5_000, bike), 0, "past the end clamps to 0");
        }
    });
}

/// At the start line, time-to-go is the whole-route estimate the EST TIME row shows: one model.
#[test]
fn time_to_go_at_the_start_is_the_route_estimate() {
    with_route("Pass", PASS, |total, ascent, p| {
        for bike in BikeType::ALL {
            assert_eq!(time_to_go_s(p, total, 0, bike), bike.ride_time_s(total, ascent), "{bike:?}");
        }
    });
}

/// `ascent_between_m` over `[progress, end]` equals the total minus what has been climbed, and the
/// fraction-indexed `ascent_to` it wraps agrees at the same point.
#[test]
fn ascent_between_m_is_the_cumulative_curve_in_metres() {
    with_route("Pass", PASS, |total, ascent, p| {
        assert_eq!(p.ascent_to_m(0, total), 0);
        assert_eq!(p.ascent_to_m(total, total), ascent);
        assert_eq!(p.ascent_to_m(total * 3, total), ascent, "past the end clamps to the total");
        assert_eq!(p.ascent_between_m(0, total, total), ascent);
        // Backwards pair saturates rather than wrapping.
        assert_eq!(p.ascent_between_m(total, 0, total), 0);
        let half = total / 2;
        assert_eq!(p.ascent_to_m(half, total), p.ascent_to(half as f32 / total as f32));
        // A zero-length route has no axis to place a distance on.
        assert_eq!(p.ascent_to_m(1_000, 0), 0);
        assert!(p.ascent_to_m(half, total) >= ascent - 20, "the pass tops out at the midpoint");
    });
}
