//! Elevation-profile tests: convert a synthetic GPX, build the profile from the
//! reader, and check it captures the route's shape — the peak, the y-range, and a
//! gap-free band — independent of how sparsely the route samples the columns.

use obc_route::{RouteIndex, RouteReader, SliceSource, PROFILE_COLS};

mod common;
use common::convert;

/// Densely scan one pyramid `level` across `[lo, hi]` and return its `(min, max)`
/// elevation envelope — for asserting the downsample keeps extremes (it's min/max, not
/// averaging) using only the public [`Profile::sample`] API.
fn level_envelope(p: &obc_route::Profile, level: usize, lo: f32, hi: f32) -> (i16, i16) {
    let (mut mn, mut mx) = (i16::MAX, i16::MIN);
    for i in 0..=512 {
        let t = lo + (hi - lo) * (i as f32 / 512.0);
        let (a, b) = p.sample(level, t);
        mn = mn.min(a);
        mx = mx.max(b);
    }
    (mn, mx)
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
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    // Y-range mirrors the route header.
    assert_eq!((p.min_ele_m, p.max_ele_m), (r.min_ele_m, r.max_ele_m));
    assert_eq!((p.min_ele_m, p.max_ele_m), (200, 300));

    // The 300 m peak survives and lands near the middle (distance ~0.5). Expressed in
    // terms of PROFILE_COLS so it tracks the base-resolution knob.
    assert_eq!(p.peak_ele_m(), 300);
    assert!(
        (PROFILE_COLS * 3 / 8..=PROFILE_COLS * 5 / 8).contains(&p.peak_col),
        "peak_col {} not near the middle",
        p.peak_col
    );
    assert!((0.375..=0.625).contains(&p.peak_frac()), "peak_frac {} not near 0.5", p.peak_frac());

    // Scrubbing: the ends are below the peak, the peak fraction reads the peak, and the
    // midpoint sits high on the climb (the exact peak column drifts with base resolution,
    // so don't pin it to 0.5 — read it at peak_frac instead).
    assert!(p.at(0.0).1 < 300, "start should be below the peak");
    assert!(p.at(1.0).1 < 300, "end should be below the peak");
    assert_eq!(p.at(p.peak_frac()).1, 300, "the peak fraction should read the peak");
    assert!(p.at(0.5).1 >= 250, "midpoint should be high on the climb");
}

#[test]
fn profile_band_is_gap_free() {
    // Five points fill at most five columns directly; the other ~250 are gaps the
    // builder must carry-fill so the band has no sentinel (min > max) holes.
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    assert_eq!(p.cols().len(), PROFILE_COLS);
    for (i, &(mn, mx)) in p.cols().iter().enumerate() {
        assert!(mn <= mx, "column {i} left unfilled ({mn} > {mx})");
        assert!((200..=300).contains(&mn) && (200..=300).contains(&mx));
    }
}

#[test]
fn profile_ascent_to_tracks_where_the_climb_happens() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    // Endpoints pin to 0 and the exact route total (clamped past the end).
    assert_eq!(p.ascent_to(0.0), 0);
    assert_eq!(p.ascent_to(1.0), r.total_ascent_m);
    assert_eq!(p.ascent_to(1.5), r.total_ascent_m);

    // PEAKED climbs 200→300 then descends — so by the peak essentially all of the route's
    // ascent is already done. (The old per-chunk interpolation spread the climb uniformly
    // over distance and reported only ~half here, leaving a phantom "to climb" at the top.)
    let peak_frac = p.peak_col as f32 / (PROFILE_COLS - 1) as f32;
    assert!(
        p.ascent_to(peak_frac) as f32 > 0.9 * r.total_ascent_m as f32,
        "by the peak the climb should be ~done, got {} of {}",
        p.ascent_to(peak_frac),
        r.total_ascent_m
    );
    // Monotonic, and the descending tail past the peak adds nothing.
    assert!(p.ascent_to(0.25) <= p.ascent_to(0.5));
    assert_eq!(p.ascent_to(0.95), r.total_ascent_m);
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
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    assert_eq!((p.min_ele_m, p.max_ele_m), (150, 150));
    for &(mn, mx) in p.cols() {
        assert_eq!((mn, mx), (150, 150));
    }
}

#[test]
fn pyramid_downsample_keeps_extremes() {
    // The coarse levels are min/max merges, not averages — so a coarser level still spans
    // the route's full 200..300 m envelope, with the peak's max and the valley's min intact.
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    // The full-route window lands on a coarse level; its envelope must still be 200..300.
    let full = p.window(0.5, 1.0, 216);
    assert!(full.level > 0, "full route should read a coarse level, got {}", full.level);
    assert_eq!(level_envelope(&p, full.level, 0.0, 1.0), (200, 300));
    // The base level too, naturally.
    assert_eq!(level_envelope(&p, 0, 0.0, 1.0), (200, 300));
}

#[test]
fn window_full_route_spans_everything() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    let w = p.window(0.5, 1.0, 216);
    assert_eq!((w.lo_frac, w.hi_frac), (0.0, 1.0));
    // A zoom below 1 is clamped to the whole route too.
    assert_eq!(p.window(0.5, 0.5, 216), w);
}

#[test]
fn window_zoom_narrows_span_and_chooses_finer_levels() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    let full = p.window(0.5, 1.0, 216);
    let z2 = p.window(0.5, 2.0, 216);
    let z4 = p.window(0.5, 4.0, 216);

    // Span is 1/zoom, centred.
    assert!((z2.hi_frac - z2.lo_frac - 0.5).abs() < 1e-4, "zoom 2 should show half the route");
    assert!((z4.hi_frac - z4.lo_frac - 0.25).abs() < 1e-4, "zoom 4 should show a quarter");
    assert!((z2.lo_frac - 0.25).abs() < 1e-4 && (z2.hi_frac - 0.75).abs() < 1e-4);

    // Zooming in reads finer (lower-index) levels, never coarser.
    assert!(z4.level <= z2.level && z2.level <= full.level);
    // Past what the base resolves (zoom 8 → 128 cols in a 216 px chart), it falls to finest.
    assert_eq!(p.window(0.5, 8.0, 216).level, 0);
}

/// A route with **no `<ele>` anywhere** — a planner GPX export. The converter stores a flat
/// 0 m elevation and a 0..0 header range, so the profile's `(min, max)` is `(0, 0)` and the
/// band must still be gap-free (the `fill_gaps` header fallback in `profile.rs`, ~line 341 —
/// the only path that uses the fallback). Item 10 calls this out: every other fixture has
/// `<ele>` on every point, so the no-elevation fallback was untested though planner GPX
/// frequently lacks elevation. Build the GPX by hand here (the `convert` fixtures all carry
/// `<ele>`).
const NO_ELE: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"/>
  <trkpt lat="48.0050" lon="7.8000"/>
  <trkpt lat="48.0100" lon="7.8000"/>
</trkseg></trk></gpx>"#;

#[test]
fn no_elevation_route_has_flat_zero_gap_free_band() {
    let bytes = convert("Unmeasured", NO_ELE);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert_eq!((r.min_ele_m, r.max_ele_m), (0, 0), "no <ele> → 0..0 header range");
    let p = r.elevation_profile();

    // The whole band is the flat 0 m fallback, with no sentinel (min > max) holes.
    assert_eq!((p.min_ele_m, p.max_ele_m), (0, 0));
    assert_eq!(p.peak_ele_m(), 0);
    for (i, &(mn, mx)) in p.cols().iter().enumerate() {
        assert_eq!((mn, mx), (0, 0), "column {i} should be the flat 0 m fallback");
    }
    // No climb anywhere, so "to climb" is 0 across the whole route.
    assert_eq!(p.ascent_to(0.0), 0);
    assert_eq!(p.ascent_to(1.0), 0);
}

#[test]
fn window_clamps_to_route_ends() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    // Centre at the very start/end: the fixed-width span slides flush against the edge
    // instead of running off it.
    let start = p.window(0.0, 4.0, 216);
    assert_eq!(start.lo_frac, 0.0);
    assert!((start.hi_frac - 0.25).abs() < 1e-4);
    let end = p.window(1.0, 4.0, 216);
    assert_eq!(end.hi_frac, 1.0);
    assert!((end.lo_frac - 0.75).abs() < 1e-4);
}
