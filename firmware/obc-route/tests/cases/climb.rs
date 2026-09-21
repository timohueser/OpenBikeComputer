//! Climb-detection tests.
//!
//! Two layers: the pure state machine ([`segment_climbs`]) runs directly over hand-built
//! `(distance, elevation)` streams, one synthetic profile per behaviour the knobs govern, so each
//! gate is pinned without any `.obcr` bytes. One test then drives the whole reader path over a
//! committed real-route fixture.
//!
//! The synthetic streams use round values that sit clearly on one side of each knob ([`MIN_GAIN`]
//! 80 m, [`MIN_AVG_GRADE`] 3 %, [`MAX_DROP`] 15 m, [`MAX_FLAT`] 300 m, [`MIN_LEN`] 400 m), so a
//! retune of the defaults moves them instead of silently flipping a result. Elevation steps are
//! larger than the 3 m dead-band, so the reader's smoothing would not change the outcome.

use obc_formats::io::SliceSource;
use obc_route::{
    segment_climbs, ClimbSeg, Climbs, ElePt, RouteIndex, RouteReader, MAX_CLIMBS, MAX_DROP, MAX_FLAT, MIN_AVG_GRADE,
    MIN_GAIN, MIN_LEN,
};

/// Connect successive `(dist, ele)` waypoints with ~10 m steps, so the state machine sees a dense
/// profile like a real route, not a few sparse points.
fn ramp(points: &[(f64, f32)]) -> Vec<ElePt> {
    let mut out = Vec::new();
    for w in points.windows(2) {
        let (d0, e0) = w[0];
        let (d1, e1) = w[1];
        let span = d1 - d0;
        let steps = ((span / 10.0).ceil() as usize).max(1);
        for i in 0..steps {
            let f = i as f64 / steps as f64;
            out.push(ElePt { dist_m: d0 + span * f, ele_m: e0 + (e1 - e0) * f as f32 });
        }
    }
    // The loop above emits each leg's start, but not the very last point.
    if let Some(&(d, e)) = points.last() {
        out.push(ElePt { dist_m: d, ele_m: e });
    }
    out
}

fn detect(points: &[(f64, f32)]) -> Vec<ClimbSeg> {
    segment_climbs(ramp(points)).as_slice().to_vec()
}

#[test]
fn clean_single_climb() {
    let climbs = detect(&[(0.0, 100.0), (2000.0, 400.0), (3000.0, 400.0)]);
    assert_eq!(climbs.len(), 1);
    let c = climbs[0];
    assert_eq!(c.base_ele_m, 100);
    assert_eq!(c.top_ele_m, 400);
    assert_eq!(c.gain_m, 300);
    assert_eq!(c.start_m, 0);
    assert_eq!(c.end_m, 2000);
    assert_eq!(c.len_m(), 2000);
    assert_eq!(c.avg_grade_pct, 15); // 300 m over 2000 m
    assert!(c.gain_m >= MIN_GAIN && c.avg_grade_pct >= MIN_AVG_GRADE && c.len_m() >= MIN_LEN);
}

/// A shallow internal dip (10 m, under [`MAX_DROP`]) is bridged: one climb spans the whole rise,
/// with the base at the start and the top at the final summit.
#[test]
fn bridged_false_flat_stays_one_climb() {
    let dip = (MAX_DROP - 5) as f32; // under the col tolerance, and tracks a retune of MAX_DROP
                                     // The recovery past the earlier summit is steep and short, so the no-new-max run stays well
                                     // under MAX_FLAT. This isolates the MAX_DROP gate from the plateau gate.
    let climbs = detect(&[
        (0.0, 100.0),
        (1000.0, 250.0),       // up 150
        (1100.0, 250.0 - dip), // shallow dip over a short span (bridged, not a col)
        (1300.0, 400.0),       // steep recovery past the earlier summit → new max
        (2100.0, 400.0),
    ]);
    assert_eq!(climbs.len(), 1, "a dip shallower than MAX_DROP must not split the climb");
    let c = climbs[0];
    assert_eq!(c.base_ele_m, 100);
    assert_eq!(c.top_ele_m, 400);
    assert_eq!(c.end_m, 1300);
}

/// A give-back deeper than [`MAX_DROP`] splits the route in two, and the second climb's base sits
/// at the col floor.
#[test]
fn deep_col_splits_into_two() {
    let climbs = detect(&[
        (0.0, 100.0),
        (1500.0, 300.0), // climb 1 summit (gain 200)
        (2000.0, 260.0), // col: 40 m give-back > MAX_DROP → closes climb 1
        (3500.0, 460.0), // climb 2 summit (gain 200 from the col)
        (4000.0, 460.0),
    ]);
    assert_eq!(climbs.len(), 2, "a col deeper than MAX_DROP must split into two climbs");
    assert_eq!(climbs[0].base_ele_m, 100);
    assert_eq!(climbs[0].top_ele_m, 300);
    assert_eq!(climbs[0].end_m, 1500);
    assert_eq!(climbs[1].base_ele_m, 260);
    assert_eq!(climbs[1].top_ele_m, 460);
    assert_eq!(climbs[1].start_m, 2000);
    assert!(climbs[0].end_m <= climbs[1].start_m);
}

#[test]
fn small_bump_rejected() {
    let bump = (MIN_GAIN - 30) as f32; // 50 m
    let climbs = detect(&[(0.0, 100.0), (600.0, 100.0 + bump), (1200.0, 100.0)]);
    assert!(climbs.is_empty(), "a rise under MIN_GAIN is a bump, not a climb");
}

#[test]
fn shallow_drag_rejected() {
    // Premise: 100 m over 5 km clears MIN_GAIN, but its 2 % grade is under MIN_AVG_GRADE.
    const _: () = assert!(100 >= MIN_GAIN as u32 && 100 * 100 / 5000 < MIN_AVG_GRADE as u32);
    let climbs = detect(&[(0.0, 100.0), (5000.0, 200.0), (6000.0, 200.0)]);
    assert!(climbs.is_empty(), "a rise under MIN_AVG_GRADE is a drag, not a climb");
}

/// A flat run longer than [`MAX_FLAT`] closes the candidate at the first summit, so the
/// pre-plateau climb is kept on its own.
#[test]
fn long_plateau_closes_climb() {
    let flat = (MAX_FLAT + 400) as f64; // 700 m of dead-flat, well past the 300 m tolerance
    let climbs = detect(&[
        (0.0, 100.0),
        (1000.0, 300.0),        // summit (gain 200) — a keeper on its own
        (1000.0 + flat, 300.0), // long flat: no new max for > MAX_FLAT → closes here
        (2000.0 + flat, 500.0), // a second, separate climb after the plateau
        (2500.0 + flat, 500.0),
    ]);
    assert_eq!(climbs.len(), 2, "a plateau longer than MAX_FLAT closes the climb");
    assert_eq!(climbs[0].top_ele_m, 300);
    assert_eq!(climbs[0].end_m, 1000);
    assert_eq!(climbs[1].top_ele_m, 500);
}

#[test]
fn flat_route_yields_none() {
    let climbs = detect(&[(0.0, 200.0), (1000.0, 200.0), (5000.0, 200.0)]);
    assert!(climbs.is_empty());
}

#[test]
fn descent_only_yields_none() {
    let climbs = detect(&[(0.0, 500.0), (2000.0, 100.0), (3000.0, 100.0)]);
    assert!(climbs.is_empty());
}

#[test]
fn overflow_keeps_largest_never_panics() {
    // 40 climbs (> MAX_CLIMBS = 24) on a strictly increasing gain ladder, so "keep the largest"
    // has one right answer. Each drops > MAX_DROP into a col, so they stay distinct.
    let n = 40usize;
    let mut pts = vec![(0.0f64, 0.0f32)];
    let mut d = 0.0;
    let mut floor = 0.0f32;
    let mut gains = Vec::new();
    for i in 0..n {
        let gain = 100.0 + 5.0 * i as f32; // 100, 105, ... 295
        gains.push(gain as u16);
        d += 1000.0;
        pts.push((d, floor + gain)); // up to the summit
        d += 300.0;
        floor = floor + gain - 40.0; // 40 m col (> MAX_DROP) down to the next base
        pts.push((d, floor));
    }
    let climbs = segment_climbs(ramp(&pts));
    assert_eq!(climbs.len(), MAX_CLIMBS, "must cap at MAX_CLIMBS, not overflow");

    // The kept set is the MAX_CLIMBS largest gains.
    gains.sort_unstable();
    let smallest_kept = gains[n - MAX_CLIMBS];
    for c in climbs.as_slice() {
        assert!(c.gain_m >= smallest_kept, "kept a climb smaller than a dropped one");
    }
    for w in climbs.as_slice().windows(2) {
        assert!(w[0].start_m <= w[1].start_m, "climbs must stay in route order after capping");
        assert!(w[0].end_m <= w[1].start_m, "climbs must not overlap");
    }
}

#[test]
fn active_at_is_raw_interval_lookup() {
    let climbs = segment_climbs(ramp(&[
        (0.0, 100.0),
        (1500.0, 300.0), // climb 0: [0, 1500]
        (2500.0, 260.0), // col > MAX_DROP
        (4000.0, 460.0), // climb 1: [2500, 4000]
        (4500.0, 460.0),
    ]));
    assert_eq!(climbs.len(), 2);
    let (c0, c1) = (climbs.as_slice()[0], climbs.as_slice()[1]);

    assert_eq!(climbs.active_at(c0.start_m), Some(0)); // inclusive lower bound
    assert_eq!(climbs.active_at((c0.start_m + c0.end_m) / 2), Some(0)); // mid climb 0
    assert_eq!(climbs.active_at(c0.end_m), Some(0)); // inclusive upper bound
    assert_eq!(climbs.active_at((c0.end_m + c1.start_m) / 2), None); // in the col, no climb
    assert_eq!(climbs.active_at(c1.start_m + 10), Some(1)); // into climb 1
    assert_eq!(climbs.active_at(c1.end_m + 1000), None); // past the last climb
}

/// Detect climbs on the committed `grimsel-climb.obcr` fixture, a real ~18.7 km Alpine pass
/// (1063 to 2151 m, header ascent 1085 m), through the full reader path: chunk decode, cumulative
/// distance and dead-band smoothing. The detector runs on the exact decimated points on the card,
/// so the assertions pin the shape, not exact metres.
#[test]
fn grimsel_fixture_detects_the_pass() {
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/sources/sim-grimsel/routes/grimsel-climb.obcr"
    ))
    .expect("grimsel-climb.obcr fixture present");
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    let climbs = r.detect_climbs();
    assert!(!climbs.is_empty(), "the Grimsel pass must yield at least one climb");

    let header_ascent = r.total_ascent_m;
    let total_gain: u32 = climbs.as_slice().iter().map(|c| c.gain_m as u32).sum();

    // Detection drops sub-threshold wiggle and the dips between climbs, so the total stays under
    // the header ascent. A sustained pass should still recover most of it.
    assert!(
        total_gain >= header_ascent * 60 / 100 && total_gain <= header_ascent,
        "total detected gain {total_gain} m out of sane range vs header ascent {header_ascent} m",
    );

    let route_len = r.total_distance_m;
    let mut prev_end = 0u32;
    for c in climbs.as_slice() {
        assert!(c.start_m >= prev_end, "climbs overlap or are out of order");
        assert!(c.end_m > c.start_m, "a climb must have positive length");
        assert!(c.end_m <= route_len, "climb runs past the route end");
        assert!(c.top_ele_m > c.base_ele_m, "top must be above base");
        prev_end = c.end_m;
    }
}

#[test]
fn empty_climbs_helpers() {
    let c = Climbs::new();
    assert!(c.is_empty());
    assert_eq!(c.len(), 0);
    assert_eq!(c.active_at(0), None);
    assert!(c.as_slice().is_empty());
}
