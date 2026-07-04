//! Breadcrumb bounds + decimation: the two tiers stay within fixed caps however long the ride, the
//! start stays visible, `clear` empties it — and the coarse spine keeps the ride's shape without
//! straight-lining a long stretch under budget pressure (issue #22). It simplifies by Visvalingam
//! effective area, so the kept points spread along the whole ride instead of collapsing to a chord.

use obc_app::Breadcrumb;

/// At lat 0, 1 µdeg ≈ 0.111 m on both axes (`cos 0 = 1`), so Euclidean µdeg distances scale
/// uniformly to metres — these tests ride near the equator to keep the geometry honest.
const LAT0: i32 = 0;
const M_PER_UD: f64 = 0.111_320;
/// Shape-error budget. The spine keeps the ride's bends within a few metres while it fits the
/// point budget — far inside the chord-cuts the old designs produced.
const SHAPE_TOL_M: f64 = 4.0;

#[test]
fn empty_then_cleared() {
    let mut bc = Breadcrumb::new();
    assert!(bc.is_empty());
    bc.push(0, LAT0);
    bc.push(1000, LAT0);
    assert!(!bc.is_empty());
    bc.clear();
    assert!(bc.is_empty());
    assert_eq!(bc.spine_iter().count(), 0);
    assert_eq!(bc.recent_iter().count(), 0);
}

#[test]
fn short_ride_is_all_recent_one_line() {
    let mut bc = Breadcrumb::new();
    // Five points ~22 m apart: the ring isn't full, so nothing has aged into the spine — the whole
    // short trail is full-resolution `recent`.
    for i in 0..5 {
        bc.push(i * 200, LAT0);
    }
    assert_eq!(bc.recent_iter().count(), 5);
    assert_eq!(bc.spine_iter().count(), 0, "nothing has aged out of the ring yet");
    assert_eq!(bc.points().count(), 5);
    assert_eq!(bc.points().next(), Some((0, LAT0)), "the ride start is the first point");
    assert_eq!(bc.points().last(), Some((800, LAT0)), "…and the latest fix is the last");
}

#[test]
fn straight_ride_is_drawn_exactly() {
    let mut bc = Breadcrumb::new();
    // ~150 km dead straight. Colinear points carry zero area, so the spine never bends.
    let mut input = std::vec::Vec::new();
    for i in 0..30_000 {
        bc.push(i * 50, LAT0);
        input.push((i * 50, LAT0));
    }
    let out: std::vec::Vec<(i32, i32)> = bc.points().collect();
    assert!(max_deviation_m(&input, &out) <= 0.001, "a straight ride must draw straight");
    assert_eq!(bc.points().next(), Some((0, LAT0)));
    assert_eq!(bc.points().last(), Some((29_999 * 50, LAT0)));
}

#[test]
fn long_curvy_ride_distributes_and_stays_bounded() {
    let mut bc = Breadcrumb::new();
    // ~200 km continuously weaving — far past both caps, so the spine is budget-pressured the whole
    // way. It must stay bounded, keep its start, follow the ride — and must NOT collapse a long
    // stretch into one straight chord (the regression this guards).
    let mut input = std::vec::Vec::new();
    let mut last = (0, 0);
    for i in 0..30_000i64 {
        let lon = (i * 60) as i32;
        let lat = LAT0 + (900.0 * (i as f64 / 80.0).sin()) as i32;
        bc.push(lon, lat);
        input.push((lon, lat));
        last = (lon, lat);
    }
    let spine: std::vec::Vec<(i32, i32)> = bc.spine_iter().collect();
    // Bounded regardless of the cap: a fixed budget, not ride-length, yet well populated.
    assert!(spine.len() < input.len() / 4, "spine bounded well below ride length: {}", spine.len());
    assert!(spine.len() > 100, "spine stays well populated: {}", spine.len());
    assert_eq!(bc.points().next(), Some((0, LAT0)), "start preserved");
    assert_eq!(bc.points().last(), Some(last), "ends at the latest fix");

    // No single spine segment may dominate: the bug drew one chord spanning most of the ride.
    // A well-spread spine has every segment a small fraction of the whole.
    let (total, longest) = span_stats(&spine);
    assert!(longest <= total / 20.0, "one segment spans too much: {longest:.0} m of {total:.0} m");

    // And the line still tracks the ride, bounded, never the straight-line collapse. The deviation
    // budget scales with spine density: the `nrf-mem` profile quarters SPINE_CAP (256 vs 1024), so
    // the same weave is drawn with a quarter the points and strays several times further — still
    // sub-pixel on a whole-ride view (~833 m/px for a 200 km ride on the 240 px panel).
    let out: std::vec::Vec<(i32, i32)> = bc.points().collect();
    let dev = max_deviation_m(&input, &out);
    let dev_budget = if cfg!(feature = "nrf-mem") { 200.0 } else { 40.0 };
    assert!(dev <= dev_budget, "trail strays {dev:.1} m from a 200 km weave on a fixed point budget");
}

#[test]
fn curve_within_budget_is_faithful() {
    let mut bc = Breadcrumb::new();
    // A weave whose aged points overflow the spine (so it simplifies) but only modestly, so the kept
    // budget tracks it within a few metres.
    let mut input = std::vec::Vec::new();
    for i in 0..3_000i64 {
        let lon = (i * 60) as i32;
        let lat = LAT0 + (900.0 * (i as f64 / 110.0 * std::f64::consts::TAU).sin()) as i32;
        bc.push(lon, lat);
        input.push((lon, lat));
    }
    assert!(bc.spine_iter().count() > 50, "spine exercised");
    let out: std::vec::Vec<(i32, i32)> = bc.points().collect();
    let dev = max_deviation_m(&input, &out);
    // The aged points overflow the spine and simplify; the constrained `nrf-mem` profile keeps a
    // quarter the points (SPINE_CAP 256 vs 1024), so the same weave strays several times further.
    let tol = if cfg!(feature = "nrf-mem") { 20.0 } else { SHAPE_TOL_M };
    assert!(dev <= tol, "breadcrumb strays {dev:.2} m from the weave");
}

#[test]
fn switchbacks_keep_their_corners() {
    let mut bc = Breadcrumb::new();
    // Straight legs joined by sharp turns — the corners carry large area, so Visvalingam spends the
    // budget on them. The turns must survive; the old along-track gate cut them once its spacing
    // relaxed (issue #22).
    let mut input = std::vec::Vec::new();
    for i in 0..6_000i64 {
        let lon = (i * 60) as i32;
        let lat = LAT0 + triangle(i, 30, 540);
        bc.push(lon, lat);
        input.push((lon, lat));
    }
    assert!(bc.spine_iter().count() > 50, "corners aged into the spine");
    let out: std::vec::Vec<(i32, i32)> = bc.points().collect();
    let dev = max_deviation_m(&input, &out);
    assert!(dev <= SHAPE_TOL_M, "switchbacks cut by {dev:.2} m — corners should survive");
}

/// Triangle wave: ramp `0 → amp` over `half` steps then `amp → 0`, repeating — sharp corners at
/// the turns, straight legs between.
fn triangle(i: i64, half: i64, amp: i64) -> i32 {
    let phase = i.rem_euclid(2 * half);
    let v = if phase <= half { phase } else { 2 * half - phase };
    (v * amp / half) as i32
}

/// `(total length, longest segment)` of a polyline in metres (near lat 0, see [`M_PER_UD`]).
fn span_stats(pts: &[(i32, i32)]) -> (f64, f64) {
    let mut total = 0.0;
    let mut longest = 0.0f64;
    for w in pts.windows(2) {
        let d = seg_len(w[0], w[1]);
        total += d;
        longest = longest.max(d);
    }
    (total, longest)
}

fn seg_len(a: (i32, i32), b: (i32, i32)) -> f64 {
    let (dx, dy) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
    dx.hypot(dy) * M_PER_UD
}

/// Worst perpendicular distance (metres) from any `input` point to the `polyline` it should lie
/// on — the breadcrumb's shape error against the ridden path.
fn max_deviation_m(input: &[(i32, i32)], polyline: &[(i32, i32)]) -> f64 {
    let mut worst = 0.0f64;
    for &(plon, plat) in input {
        let p = (plon as f64, plat as f64);
        let mut best = f64::MAX;
        for w in polyline.windows(2) {
            let a = (w[0].0 as f64, w[0].1 as f64);
            let b = (w[1].0 as f64, w[1].1 as f64);
            best = best.min(point_segment_dist(p, a, b));
        }
        worst = worst.max(best);
    }
    worst * M_PER_UD
}

/// Distance from point `p` to segment `a→b`, all in microdegrees (clamped to the segment).
fn point_segment_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= 1e-9 {
        return (p.0 - a.0).hypot(p.1 - a.1);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    (p.0 - (a.0 + dx * t)).hypot(p.1 - (a.1 + dy * t))
}
