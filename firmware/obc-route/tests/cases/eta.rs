//! Gradient-aware time model.
//!
//! Two halves: the pure model (`t = dist / v_flat + ascent × k_climb`) against a hand-computed
//! table, and the route-relative `time_to_go_s` against real converted geometry. The flat route
//! must degrade to `dist / v_flat` with no special case; the synthetic pass must count its
//! remaining time down to zero.

use obc_formats::io::SliceSource;
use obc_route::eta::{K_CLIMB_S_PER_M, PROFILE_COUNT, V_FLAT_KMH};
use obc_route::{ride_time_s, route_time_s, time_to_go_s, v_flat_mps, RouteIndex, RouteReader};

use crate::common::convert;

/// The four shipped profile indices, in table order: Road, Gravel, MTB, Touring.
const PROFILES: [u8; PROFILE_COUNT] = [0, 1, 2, 3];

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

/// With no ascent the climb term is zero, so the model returns the plain distance-over-speed
/// answer on every profile. An elevation-free route degrades naturally instead of needing a branch.
#[test]
fn flat_is_exactly_distance_over_v_flat() {
    for idx in PROFILES {
        for dist_m in [0u32, 1, 1_000, 42_195, 300_000] {
            let want = (dist_m as f32 / v_flat_mps(idx)) as u32;
            assert_eq!(ride_time_s(dist_m, 0, idx), want, "profile {idx}, {dist_m} m flat");
        }
    }
    // The readable version of the same fact: 22 km at Road's 22 km/h is one hour.
    let hour = ride_time_s(22_000, 0, 0);
    assert!((3599..=3600).contains(&hour), "22 km flat on Road should be ~1 h, got {hour} s");
}

/// The hand-computed table: `t = dist / v_flat + ascent × k_climb`, worked out per profile from
/// the two knob tables. The 1 s tolerance absorbs the `f32` division; more means a knob moved.
#[test]
fn hand_computed_table() {
    // (profile, dist_m, ascent_m, expected_s), worked out from the knob tables:
    //   Road    v=22.0 km/h = 6.1111 m/s, k=1.6 : 30000/6.1111 = 4909 + 1500×1.6 = 2400 → 7309
    //   Gravel  v=19.0 km/h = 5.2778 m/s, k=1.9 : 30000/5.2778 = 5684 + 1500×1.9 = 2850 → 8534
    //   MTB     v=16.0 km/h = 4.4444 m/s, k=2.3 : 30000/4.4444 = 6750 + 1500×2.3 = 3450 → 10200
    //   Touring v=17.0 km/h = 4.7222 m/s, k=2.2 : 30000/4.7222 = 6353 + 1500×2.2 = 3300 →  9653
    const TABLE: [(u8, u32, u32, u32); 6] = [
        (0, 30_000, 1_500, 7_309),
        (1, 30_000, 1_500, 8_534),
        (2, 30_000, 1_500, 10_200),
        (3, 30_000, 1_500, 9_653),
        // The same 30 km with no climbing: the climb term is the whole difference.
        (0, 30_000, 0, 4_909),
        // A short, steep alpine ramp: 5 km, 600 m up, on a gravel bike.
        //   5000/5.2778 = 947 + 600×1.9 = 1140 → 2087
        (1, 5_000, 600, 2_087),
    ];
    for (idx, dist, ascent, want) in TABLE {
        let got = ride_time_s(dist, ascent, idx);
        assert!(got.abs_diff(want) <= 1, "profile {idx}: {dist} m / {ascent} m → {got} s, hand-computed {want} s");
    }
}

/// The knob tables stay well formed: one entry per shipped profile, positive everywhere. Speed and
/// climb penalty are deliberately not ordered across profiles, because Touring is faster than MTB
/// on the flat while paying nearly as much to climb.
#[test]
fn knob_tables_are_well_formed() {
    assert_eq!(V_FLAT_KMH.len(), PROFILE_COUNT);
    assert_eq!(K_CLIMB_S_PER_M.len(), PROFILE_COUNT);
    for idx in PROFILES {
        assert!(V_FLAT_KMH[idx as usize] > 0.0, "a zero flat speed would divide by zero");
        assert!(K_CLIMB_S_PER_M[idx as usize] > 0.0, "climbing must never be free");
        // The road bike is the fastest and the cheapest climber of the four.
        assert!(V_FLAT_KMH[0] >= V_FLAT_KMH[idx as usize]);
        assert!(K_CLIMB_S_PER_M[0] <= K_CLIMB_S_PER_M[idx as usize]);
    }
}

/// An out-of-range profile index resolves to profile 0, the same fallback the router and the
/// Bike-type label use, so a stale setting cannot make the clock describe a different bike.
#[test]
fn out_of_range_profile_falls_back_to_road() {
    for idx in [PROFILE_COUNT as u8, 7, 200, u8::MAX] {
        assert_eq!(ride_time_s(30_000, 1_500, idx), ride_time_s(30_000, 1_500, 0), "index {idx}");
        assert_eq!(v_flat_mps(idx), v_flat_mps(0));
    }
}

/// The model reads ascent only, so descent credits nothing: the up-and-over pass costs the same as
/// a pure climb of the same gain, and strictly more than the flat twin.
#[test]
fn descent_is_never_a_credit() {
    // 9 km, 300 m up then 300 m down, vs 9 km with the same 300 m up and no descent: same answer.
    assert_eq!(ride_time_s(9_000, 300, 0), ride_time_s(9_000, 300, 0));
    assert!(ride_time_s(9_000, 300, 0) > ride_time_s(9_000, 0, 0), "the climb must cost time");

    // On real geometry: the two fixtures cover the same ground, so the whole difference is ascent.
    let flat = with_route("Flat", FLAT, |d, a, _| (d, a));
    let pass = with_route("Pass", PASS, |d, a, _| (d, a));
    assert_eq!(pass.1, 300, "the fixture climbs 300 m");
    assert_eq!(flat.1, 0, "the flat fixture climbs nothing");
    assert!(pass.0.abs_diff(flat.0) < 50, "the two fixtures cover the same ground ({} vs {} m)", pass.0, flat.0);
    let (t_flat, t_pass) = (route_time_s(flat.0, flat.1, 0), route_time_s(pass.0, pass.1, 0));
    assert!(t_pass > t_flat, "the pass must estimate longer than the flat twin ({t_pass} vs {t_flat} s)");
    // 300 m × 1.6 s/m = 480 s, modulo the few metres of length difference between the fixtures.
    assert!((t_pass - t_flat).abs_diff(480) <= 5, "the delta is the climb term: {} s", t_pass - t_flat);
}

#[test]
fn flat_route_time_to_go_is_distance_only() {
    with_route("Flat", FLAT, |total, ascent, p| {
        assert_eq!(ascent, 0);
        for idx in PROFILES {
            for progress in [0, total / 4, total / 2, total - 1, total] {
                let want = ride_time_s(total - progress, 0, idx);
                assert_eq!(time_to_go_s(p, total, progress, idx), want, "profile {idx} at {progress} m");
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
        for idx in PROFILES {
            let mut prev = u32::MAX;
            for step in 0..=200u32 {
                let progress = total * step / 200;
                let t = time_to_go_s(p, total, progress, idx);
                assert!(t <= prev, "profile {idx}: time-to-go rose from {prev} to {t} s at {progress} m");
                prev = t;
            }
            assert_eq!(time_to_go_s(p, total, total, idx), 0, "nothing left at the finish");
            assert_eq!(time_to_go_s(p, total, total + 5_000, idx), 0, "past the end clamps to 0");
        }
    });
}

/// At the start line, time-to-go is the whole-route estimate the EST TIME row shows: one model.
#[test]
fn time_to_go_at_the_start_is_the_route_estimate() {
    with_route("Pass", PASS, |total, ascent, p| {
        for idx in PROFILES {
            assert_eq!(time_to_go_s(p, total, 0, idx), route_time_s(total, ascent, idx), "profile {idx}");
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
