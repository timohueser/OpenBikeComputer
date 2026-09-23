//! Time to go along real converted geometry. The estimate table itself is pinned by the shared
//! vector file (`specs/vectors/eta.csv`) in `obc-formats`.

use obc_formats::io::SliceSource;
use obc_route::{time_to_go_s, BikeType, RouteIndex, RouteReader};

use crate::common::convert;

/// A dead-flat route: a zigzag (so no point decimates away) at a constant 200 m over ~9 km.
const FLAT: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="47.0000" lon="8.0000"><ele>200.0</ele></trkpt>
  <trkpt lat="47.0020" lon="8.0200"><ele>200.0</ele></trkpt>
  <trkpt lat="47.0000" lon="8.0400"><ele>200.0</ele></trkpt>
  <trkpt lat="47.0020" lon="8.0600"><ele>200.0</ele></trkpt>
  <trkpt lat="47.0000" lon="8.0800"><ele>200.0</ele></trkpt>
  <trkpt lat="47.0020" lon="8.1000"><ele>200.0</ele></trkpt>
  <trkpt lat="47.0000" lon="8.1200"><ele>200.0</ele></trkpt>
</trkseg></trk></gpx>"#;

/// The same ~9 km of ground as a pass: 500 m to 800 m to 500 m. Same length, same start and end
/// height, 300 m of ascent. This is the A/B that isolates the climb term.
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

/// The model reads ascent only, so descent credits nothing: the up-and-over pass costs the same as
/// a pure climb of the same gain, and strictly more than the flat twin.
#[test]
fn descent_is_never_a_credit() {
    // The two fixtures cover the same ground, so the whole difference is ascent.
    let flat = with_route("Flat", FLAT, |d, a, _| (d, a));
    let pass = with_route("Pass", PASS, |d, a, _| (d, a));
    assert_eq!(pass.1, 300, "the fixture climbs 300 m");
    assert_eq!(flat.1, 0, "the flat fixture climbs nothing");
    assert!(pass.0.abs_diff(flat.0) < 50, "the two fixtures cover the same ground ({} vs {} m)", pass.0, flat.0);
    let (t_flat, t_pass) = (BikeType::Road.ride_time_s(flat.0, flat.1), BikeType::Road.ride_time_s(pass.0, pass.1));
    assert!(t_pass > t_flat, "the pass must estimate longer than the flat twin ({t_pass} vs {t_flat} s)");
    // 300 m × 1.6 s/m = 480 s, modulo the few metres of length difference between the fixtures.
    assert!((t_pass - t_flat).abs_diff(480) <= 5, "the delta is the climb term: {} s", t_pass - t_flat);
}

#[test]
fn flat_route_time_to_go_is_distance_only() {
    with_route("Flat", FLAT, |total, ascent, p| {
        assert_eq!(ascent, 0);
        for bike in BikeType::ALL {
            for progress in [0, total / 4, total / 2, total - 1, total] {
                let want = bike.ride_time_s(total - progress, 0);
                assert_eq!(time_to_go_s(p, total, progress, bike), want, "{bike:?} at {progress} m");
            }
        }
    });
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
