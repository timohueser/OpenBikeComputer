//! Route-matcher tests: convert a synthetic GPX, open the reader, then drive
//! [`RouteMatch`] with crafted fixes — on the line, beside it, rejoining, and around a
//! loop — checking progress, the off-route flag/freeze, and cross-track distance. Fixes
//! are built from the *decoded* geometry, so the assertions don't depend on exactly which
//! vertices survived decimation.

use heapless::Vec as HVec;
use obc_formats::io::SliceSource;
use obc_route::{RouteIndex, RouteMatch, RoutePoint, RouteReader, MAX_POINTS_PER_CHUNK};

mod common;
use common::convert;

/// Build GPX text from `(lat_deg, lon_deg, ele_m)` track points.
fn gpx_from(pts: &[(f64, f64, f64)]) -> String {
    let mut s = String::from("<?xml version=\"1.0\"?><gpx><trk><trkseg>");
    for &(lat, lon, ele) in pts {
        s.push_str(&format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{ele}</ele></trkpt>"));
    }
    s.push_str("</trkseg></trk></gpx>");
    s
}

/// Decode the whole route to a flat polyline (dropping each chunk's shared seam point).
fn decode_all(r: &RouteReader) -> Vec<RoutePoint> {
    let mut all = Vec::new();
    let mut buf: HVec<RoutePoint, MAX_POINTS_PER_CHUNK> = HVec::new();
    for k in 0..r.chunks().len() {
        r.decode_chunk(k, &mut buf).unwrap();
        let skip = if all.is_empty() { 0 } else { 1 };
        all.extend(buf.iter().skip(skip).copied());
    }
    all
}

/// Microdegrees of latitude for `d` meters north (latitude is unscaled).
fn north_ud(d_m: f64) -> i32 {
    (d_m / 0.111_320).round() as i32
}

// A single straight segment running due east at lat 48.0000, lon 7.800 → 7.810
// (~745 m). One segment keeps the on-line / off-route / cross-track assertions clean: a
// north-offset fix's nearest point is directly below it, so cross-track == the offset.
const EAST: &[(f64, f64, f64)] = &[(48.0000, 7.8000, 200.0), (48.0000, 7.8100, 210.0)];

/// Position on the straight EAST route at fraction `f` (0..1), offset `north_m` meters
/// north of the line. Returns `(lon, lat)` microdegrees for [`RouteMatch::update`].
fn east_fix(p0: RoutePoint, p1: RoutePoint, f: f64, north_m: f64) -> (i32, i32) {
    let lon = p0.lon + ((p1.lon - p0.lon) as f64 * f).round() as i32;
    let lat = p0.lat + north_ud(north_m);
    (lon, lat)
}

#[test]
fn on_line_fixes_advance_monotonically() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());
    let total = r.total_distance_m;

    let mut m = RouteMatch::new();
    let mut last = 0u32;
    for (i, f) in [0.0, 0.25, 0.5, 0.75, 1.0].iter().enumerate() {
        let (lon, lat) = east_fix(p0, p1, *f, 0.0);
        let res = m.update(lon, lat, &r);
        assert!(!res.off_route, "fix {i} on the line must read on-route");
        assert!(res.dist_m <= 2, "fix {i} on the line: cross-track {} m", res.dist_m);
        assert!(res.progress_m + 1 >= last, "progress went backwards at fix {i}");
        // Progress tracks the fraction along the route.
        let want = (*f * total as f64) as u32;
        assert!(res.progress_m.abs_diff(want) <= 5, "fix {i}: progress {} m, expected ~{want} m", res.progress_m);
        last = res.progress_m;
    }
}

#[test]
fn cross_track_distance_matches_a_known_offset() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());

    let mut m = RouteMatch::new();
    // 50 m north of the midpoint: the foot of the perpendicular is the midpoint, so the
    // reported cross-track distance should be ~50 m (and past the off-route threshold).
    let (lon, lat) = east_fix(p0, p1, 0.5, 50.0);
    let res = m.update(lon, lat, &r);
    assert!(res.off_route, "50 m off the line is past the 25 m threshold");
    assert!((res.dist_m as i32 - 50).abs() <= 3, "cross-track {} m, expected ~50", res.dist_m);
}

#[test]
fn off_route_freezes_progress_then_resumes_on_rejoin() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());
    let total = r.total_distance_m;

    let mut m = RouteMatch::new();
    // Lock on-line at the midpoint.
    let (lon, lat) = east_fix(p0, p1, 0.5, 0.0);
    let mid = m.update(lon, lat, &r);
    assert!(!mid.off_route);
    let frozen = mid.progress_m;

    // Stray 60 m north further along (60 % of the way): off-route, progress must NOT
    // advance to ~60 % — it stays frozen at the last on-route value.
    let (lon, lat) = east_fix(p0, p1, 0.6, 60.0);
    let off = m.update(lon, lat, &r);
    assert!(off.off_route, "60 m away must read off-route");
    assert_eq!(off.progress_m, frozen, "progress must freeze while off-route");
    assert!((off.dist_m as i32 - 60).abs() <= 4, "live cross-track {} m ~ 60", off.dist_m);

    // Rejoin the line at 80 %: clears, and progress jumps forward to ~80 %.
    let (lon, lat) = east_fix(p0, p1, 0.8, 0.0);
    let back = m.update(lon, lat, &r);
    assert!(!back.off_route, "back on the line clears off-route");
    let want = (0.8 * total as f64) as u32;
    assert!(back.progress_m.abs_diff(want) <= 6, "resumed progress {} ~ {want}", back.progress_m);
}

#[test]
fn position_lookup_and_clipped_interval_are_exact_and_end_clamped() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let p0 = pts[0];
    let total = r.total_distance_m;

    let mid = r.position_at(total / 2).unwrap();
    assert_eq!(mid.progress_m, total / 2);
    let walked = obc_route::ground_dist_m((p0.lon, p0.lat), (mid.lon, mid.lat));
    assert!((walked - total as f32 / 2.0).abs() <= 2.0, "midpoint walked {walked} m of {total} m");
    assert_eq!(r.position_at(total + 10_000).unwrap().progress_m, total, "lookup clamps to route end");

    let lo = total / 4;
    let hi = total * 3 / 4;
    let want_lo = r.position_at(lo).unwrap();
    let want_hi = r.position_at(hi).unwrap();
    let mut first = None;
    let mut last = None;
    let mut visits = 0;
    r.visit_points_between(lo, hi, |part| {
        visits += 1;
        first.get_or_insert(part[0]);
        last = part.last().copied();
    });
    assert!(visits >= 1);
    assert_eq!(first, Some((want_lo.lon, want_lo.lat)), "highlight starts at the interpolated lower bound");
    assert_eq!(last, Some((want_hi.lon, want_hi.lat)), "highlight ends at the interpolated rejoin point");
}

#[test]
fn skip_floor_blocks_the_skipped_stretch_and_never_moves_backward() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());
    let total = r.total_distance_m;
    let mut m = RouteMatch::new();

    let (lon, lat) = east_fix(p0, p1, 0.2, 0.0);
    let before = m.update(lon, lat, &r);
    let floor = total * 7 / 10;
    assert_eq!(m.set_progress_floor(&r, floor).unwrap().progress_m, floor);

    // Still physically on the skipped part of the same long segment: measure to the floor point,
    // report off-route, and freeze at the new navigation anchor.
    let (lon, lat) = east_fix(p0, p1, 0.3, 0.0);
    let skipped = m.update(lon, lat, &r);
    assert!(skipped.off_route);
    assert_eq!(skipped.progress_m, floor);
    assert!(skipped.progress_m > before.progress_m);

    let (lon, lat) = east_fix(p0, p1, 0.8, 0.0);
    let rejoined = m.update(lon, lat, &r);
    assert!(!rejoined.off_route);
    assert!(rejoined.progress_m > floor);

    // A chooser opened earlier may commit after the rider has advanced. The matcher method itself
    // enforces forward-only semantics, independent of the caller's freshness.
    let stale = m.set_progress_floor(&r, total / 2).unwrap();
    assert_eq!(stale.progress_m, rejoined.progress_m, "a lower floor cannot move progress backward");

    m.reset();
    assert_eq!(m.set_progress_floor(&r, total / 2).unwrap().progress_m, total / 2, "route/session reset clears it");
}

/// A high-frequency eastward sawtooth of `n` points. Every interior vertex is a
/// peak/valley deviating ~4 m from its neighbours' chord — well past the 1 m
/// decimation tolerance — so all survive and a few hundred span more than one
/// chunk. Walking it is the only matcher test that crosses a chunk seam, exercising
/// the cumulative segment index + cursor advance across chunks.
fn sawtooth_gpx(n: usize) -> String {
    let (lat0, lon0) = (48.0_f64, 7.8_f64);
    let cl = (lat0 * std::f64::consts::PI / 180.0).cos();
    let dlon = 8.0 / (111_320.0 * cl); // ~8 m east per step
    let dlat = 4.0 / 111_320.0; // ±4 m north sawtooth
    let pts: Vec<(f64, f64, f64)> = (0..n)
        .map(|i| {
            let lat = lat0 + if i % 2 == 0 { 0.0 } else { dlat };
            (lat, lon0 + dlon * i as f64, 200.0)
        })
        .collect();
    gpx_from(&pts)
}

#[test]
fn multi_chunk_route_matches_across_chunk_boundaries() {
    let bytes = convert("Sawtooth", &sawtooth_gpx(400));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert!(r.chunks().len() >= 2, "sawtooth should span >1 chunk, got {}", r.chunks().len());
    let pts = decode_all(&r);
    let total = r.total_distance_m;

    // Each decoded vertex sits exactly on the route, so a fix walked through them must
    // read on-route with monotonically advancing progress — across every chunk seam.
    let mut m = RouteMatch::new();
    let mut last = 0u32;
    for (i, p) in pts.iter().enumerate() {
        let res = m.update(p.lon, p.lat, &r);
        assert!(!res.off_route, "vertex {i} sits on the route");
        assert!(res.progress_m + 1 >= last, "vertex {i}: progress {} < {last}", res.progress_m);
        last = res.progress_m;
    }
    assert!(last as f64 > 0.9 * total as f64, "final progress {last} m should reach near the {total} m total");
}

#[test]
fn clipped_interval_streams_continuously_across_a_chunk_seam() {
    let bytes = convert("Sawtooth", &sawtooth_gpx(400));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert!(r.chunks().len() >= 2, "fixture must exercise the multi-chunk path");
    let seam = r.chunks()[1].cum_distance_m;
    let lo = seam.saturating_sub(37);
    let hi = (seam + 53).min(r.total_distance_m);
    let want_lo = r.position_at(lo).unwrap();
    let want_hi = r.position_at(hi).unwrap();

    let mut parts: Vec<Vec<(i32, i32)>> = Vec::new();
    r.visit_points_between(lo, hi, |part| parts.push(part.to_vec()));
    assert!(parts.len() >= 2, "an interval around seam {seam} must visit both adjacent chunks");
    assert_eq!(parts[0][0], (want_lo.lon, want_lo.lat), "the lower clip equals position_at(lo)");
    assert_eq!(parts.last().unwrap().last().copied(), Some((want_hi.lon, want_hi.lat)));
    for pair in parts.windows(2) {
        assert_eq!(pair[0].last(), pair[1].first(), "adjacent callbacks share the exact seam coordinate");
        assert!(pair[0][0].0 <= pair[1][0].0, "callbacks retain the eastward route order");
    }
}

/// A closed ~800 m-radius loop (meter-corrected so it's round on the ground) sampled at 20
/// vertices — enough curvature that every vertex survives decimation, giving ~20 segments.
/// The loop returns to its start, so spatial nearest-point is ambiguous there; only the
/// forward bias keeps progress from snapping back.
fn loop_gpx() -> String {
    let (clat, clon) = (48.0_f64, 7.8_f64);
    let r_deg = 800.0 / 111_320.0; // ~800 m in latitude degrees
    let cl = (clat * std::f64::consts::PI / 180.0).cos();
    let n = 20;
    let pts: Vec<(f64, f64, f64)> = (0..=n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            (clat + r_deg * a.sin(), clon + r_deg * a.cos() / cl, 200.0)
        })
        .collect();
    gpx_from(&pts)
}

#[test]
fn forward_bias_does_not_snap_back_on_a_loop() {
    let bytes = convert("Loop", &loop_gpx());
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let total = r.total_distance_m;
    assert!(pts.len() >= 8, "loop should keep many vertices, got {}", pts.len());

    // Walk a fix through every decoded vertex in order.
    let mut m = RouteMatch::new();
    let mut last = 0u32;
    let mut saw_midway = false;
    for (i, p) in pts.iter().enumerate() {
        let res = m.update(p.lon, p.lat, &r);
        assert!(!res.off_route, "vertex {i} sits on the route");
        // Monotonic (allow a 1 m rounding wobble).
        assert!(res.progress_m + 1 >= last, "vertex {i}: progress {} < {last}", res.progress_m);
        if (0.4..0.6).contains(&(res.progress_m as f64 / total as f64)) {
            saw_midway = true;
        }
        last = res.progress_m;
    }
    assert!(saw_midway, "progress should pass through the route's midpoint");
    // The closing vertex coincides with the start; forward bias means progress is ~total,
    // not back near zero.
    assert!(
        last as f64 > 0.8 * total as f64,
        "final progress {last} m should be near the {total} m total, not snapped back"
    );
}

/// An out-and-back: out along a north-bowing arc A→M→B, back straight B→A′, where A′ ends
/// ~2 m north of the A start. Start and finish sit nearly on top of each other, so a small
/// north offset at the start makes the *finish* segment marginally the nearest — the case the
/// first-lock earliest-bias must resolve. The bowed outbound keeps the mid-route unambiguous,
/// isolating the first-lock behaviour.
fn out_and_back_gpx() -> String {
    gpx_from(&[
        (48.0000, 7.8000, 200.0),  // A  — start
        (48.0006, 7.8050, 240.0),  // M  — outbound apex, ~67 m north of the chord
        (48.0000, 7.8100, 210.0),  // B  — turnaround (~745 m east)
        (48.00002, 7.8000, 201.0), // A′ — finish, ~2 m north of A
    ])
}

#[test]
fn out_and_back_first_lock_biases_to_the_start() {
    let bytes = convert("OutBack", &out_and_back_gpx());
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let total = r.total_distance_m;
    let pts = decode_all(&r);
    assert!(pts.len() >= 4, "out-and-back should keep A, M, B, A′; got {}", pts.len());

    // Walk the outbound vertices A, M, B. The first fix is offset 12 m north — the offset
    // that used to latch the coincident finish; M/B get a small offset for realism.
    let mut m = RouteMatch::new();
    let mut last = 0u32;
    for (k, (idx, off)) in [(0usize, 12.0), (1, 3.0), (2, -3.0)].iter().enumerate() {
        let p = pts[*idx];
        let res = m.update(p.lon, p.lat + north_ud(*off), &r);
        assert!(!res.off_route, "outbound vertex {idx} (~{off} m off) stays on-route");
        if k == 0 {
            assert!(
                (res.progress_m as f64) < 0.25 * total as f64,
                "first lock must land near the START (progress {} m), not the finish (~{total} m)",
                res.progress_m
            );
        }
        assert!(res.progress_m + 1 >= last, "progress must not snap back at vertex {idx}");
        last = res.progress_m;
    }
    assert!(
        (last as f64) < 0.75 * total as f64,
        "outbound progress {last} m should be on the first half, not near the {total} m finish"
    );
}

/// A short eastward zigzag of `n` segments, ~30 m each, every vertex kept (the ±5 m zig is
/// well past the 1 m decimation tolerance). Used by the going-backwards / `BACK_SEGS` tests:
/// they need a route with *many distinct segments* so a fix can move onto an earlier one,
/// which the single-segment `EAST` route can't express.
fn zigzag_gpx(n: usize) -> String {
    let (lat0, lon0) = (48.0_f64, 7.8_f64);
    let cl = (lat0 * std::f64::consts::PI / 180.0).cos();
    let dlon = 30.0 / (111_320.0 * cl); // ~30 m east per step
    let dlat = 5.0 / 111_320.0; // ±5 m north zig — survives decimation
    let pts: Vec<(f64, f64, f64)> = (0..=n)
        .map(|i| {
            let lat = lat0 + if i % 2 == 0 { 0.0 } else { dlat };
            (lat, lon0 + dlon * i as f64, 100.0)
        })
        .collect();
    gpx_from(&pts)
}

/// Powering on off-route: the first fix scans the whole route and always locks onto a segment,
/// so only the off-route flag + frozen progress keep a rider 500 m away from reading "on route
/// at 0 %". A far first fix must read `off_route` with progress pinned at 0.
#[test]
fn first_fix_far_off_route_reports_off_and_frozen() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());

    let mut m = RouteMatch::new();
    // Power on 500 m north of the route's midpoint.
    let (lon, lat) = east_fix(p0, p1, 0.5, 500.0);
    let res = m.update(lon, lat, &r);
    assert!(res.off_route, "a first fix 500 m off the route must read off-route");
    assert_eq!(res.progress_m, 0, "an off-route first fix must not advance progress past 0");
    assert!((res.dist_m as i32 - 500).abs() <= 5, "live cross-track {} m ~ 500", res.dist_m);
}

/// The hysteresis-hold band (`ON_M`=15..`OFF_M`=25 m): a fix inside it must keep whichever
/// state the rider was already in, so the "off route" banner doesn't flap as GPS noise hovers
/// around the boundary.
#[test]
fn hysteresis_band_holds_previous_state() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());

    // Coming from on-route, a 20 m fix (inside the 15–25 m band) HOLDS on-route — and, being
    // on-route, progress keeps advancing.
    let mut on = RouteMatch::new();
    let (lon, lat) = east_fix(p0, p1, 0.5, 0.0);
    let locked = on.update(lon, lat, &r);
    assert!(!locked.off_route);
    let (lon, lat) = east_fix(p0, p1, 0.55, 20.0);
    let held_on = on.update(lon, lat, &r);
    assert!(!held_on.off_route, "20 m in the band, coming from on-route, must stay on-route");
    assert!(held_on.progress_m > locked.progress_m, "an on-route band fix still advances progress");
    assert!((held_on.dist_m as i32 - 20).abs() <= 3, "live cross-track {} m ~ 20", held_on.dist_m);

    // Coming from off-route, the same 20 m band fix HOLDS off-route — progress stays frozen.
    let mut off = RouteMatch::new();
    let (lon, lat) = east_fix(p0, p1, 0.5, 0.0);
    let frozen = off.update(lon, lat, &r).progress_m;
    let (lon, lat) = east_fix(p0, p1, 0.6, 60.0);
    assert!(off.update(lon, lat, &r).off_route, "60 m must trip off-route first");
    let (lon, lat) = east_fix(p0, p1, 0.62, 20.0);
    let held_off = off.update(lon, lat, &r);
    assert!(held_off.off_route, "20 m in the band, coming from off-route, must stay off-route");
    assert_eq!(held_off.progress_m, frozen, "an off-route band fix keeps progress frozen");
}

/// A single GPS teleport / jitter spike: a lone wild outlier between two good fixes must not
/// lurch the route position — the off-route freeze holds progress at the last good value while
/// the spike reads off-route, and the next good fix resumes cleanly.
#[test]
fn single_teleport_spike_does_not_lurch_progress() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    let (p0, p1) = (pts[0], *pts.last().unwrap());
    let total = r.total_distance_m;

    let mut m = RouteMatch::new();
    let (lon, lat) = east_fix(p0, p1, 0.4, 0.0);
    let good = m.update(lon, lat, &r);
    assert!(!good.off_route);
    let before = good.progress_m;

    // A single fix teleported 2 km north: off-route, progress frozen at the last good value.
    let (lon, lat) = east_fix(p0, p1, 0.45, 2000.0);
    let spike = m.update(lon, lat, &r);
    assert!(spike.off_route, "a 2 km outlier must read off-route");
    assert_eq!(spike.progress_m, before, "the spike must not move progress");
    assert!(spike.dist_m > 1000, "live cross-track {} m should report the spike's distance", spike.dist_m);

    // The very next on-line fix recovers — back on-route, progress at ~50 %.
    let (lon, lat) = east_fix(p0, p1, 0.5, 0.0);
    let after = m.update(lon, lat, &r);
    assert!(!after.off_route, "the fix after the spike recovers on-route");
    let want = (0.5 * total as f64) as u32;
    assert!(after.progress_m.abs_diff(want) <= 5, "recovered progress {} ~ {want}", after.progress_m);
}

/// Going backwards within the `BACK_SEGS`=3 slack: a small backward step must follow onto the
/// earlier segment with progress *descending* (not frozen, not snapped forward), so "distance
/// ridden" tracks a rider who briefly reverses.
#[test]
fn small_backward_step_descends_progress() {
    let bytes = convert("Zig", &zigzag_gpx(6));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    assert!(pts.len() >= 6, "zigzag should keep its vertices, got {}", pts.len());

    let mut m = RouteMatch::new();
    // Advance forward through every vertex.
    let mut fwd = Vec::new();
    for p in &pts {
        fwd.push(m.update(p.lon, p.lat, &r).progress_m);
    }
    assert!(fwd.windows(2).all(|w| w[1] >= w[0]), "forward walk must be monotonic: {fwd:?}");

    // Now step back one vertex at a time: each fix stays on-route and progress strictly
    // descends, landing back at 0 — the BACK_SEGS slack keeps the lock the whole way.
    let mut prev = *fwd.last().unwrap();
    for i in (0..pts.len() - 1).rev() {
        let res = m.update(pts[i].lon, pts[i].lat, &r);
        assert!(!res.off_route, "a one-segment backward step stays on-route at vertex {i}");
        assert!(
            res.progress_m < prev,
            "progress must descend stepping back to vertex {i}: {} !< {prev}",
            res.progress_m
        );
        prev = res.progress_m;
    }
    assert_eq!(prev, 0, "walking all the way back reaches the route start (0 m)");
}

/// A backward jump *past* `BACK_SEGS` falls outside the backward window, so the nearest
/// in-window segment is far away → the matcher reports off-route and **freezes** progress
/// rather than teleporting the cursor backwards.
#[test]
fn backward_jump_beyond_back_segs_freezes() {
    let bytes = convert("Zig", &zigzag_gpx(12));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let pts = decode_all(&r);
    assert!(pts.len() >= 12, "need a long zigzag, got {}", pts.len());

    let mut m = RouteMatch::new();
    for p in &pts[..=8] {
        m.update(p.lon, p.lat, &r);
    }
    let at8 = m.update(pts[8].lon, pts[8].lat, &r);
    assert!(!at8.off_route);
    let frozen = at8.progress_m;

    // Jump back to vertex 2 — 6 segments behind, well past BACK_SEGS=3.
    let jumped = m.update(pts[2].lon, pts[2].lat, &r);
    assert!(jumped.off_route, "a 6-segment backward jump is outside the slack → off-route");
    assert_eq!(jumped.progress_m, frozen, "progress freezes; the cursor must not snap backwards");
}

/// A hairpin whose out and back legs are metres apart, so at the apex two segments are almost
/// equidistant. Walking through the turn, the forward bias must keep progress monotonic — not
/// snap from the inbound leg back onto the just-as-near outbound leg.
#[test]
fn hairpin_close_legs_progress_stays_monotonic() {
    // Out east along lat 48.0000, hairpin at the far end, back west along lat 48.00005 (~5.5 m
    // north — the two legs run nearly on top of each other, unlike the bowed out-and-back).
    let mut pts: Vec<(f64, f64, f64)> = Vec::new();
    let cl = (48.0_f64 * std::f64::consts::PI / 180.0).cos();
    let dlon = 30.0 / (111_320.0 * cl);
    let n = 8;
    for i in 0..=n {
        pts.push((48.0, 7.8 + dlon * i as f64, 100.0)); // outbound
    }
    for i in (0..n).rev() {
        pts.push((48.000_05, 7.8 + dlon * i as f64, 100.0)); // inbound, ~5.5 m north
    }
    let bytes = convert("Hairpin", &gpx_from(&pts));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let decoded = decode_all(&r);
    let total = r.total_distance_m;

    let mut m = RouteMatch::new();
    let mut last = 0u32;
    let mut saw_apex = false;
    for (i, p) in decoded.iter().enumerate() {
        let res = m.update(p.lon, p.lat, &r);
        assert!(!res.off_route, "hairpin vertex {i} sits on the route");
        // Monotonic across the apex — the inbound leg never snaps back onto the close outbound.
        assert!(res.progress_m + 1 >= last, "progress snapped back at vertex {i}: {} < {last}", res.progress_m);
        if (0.45..0.55).contains(&(res.progress_m as f64 / total as f64)) {
            saw_apex = true;
        }
        last = res.progress_m;
    }
    assert!(saw_apex, "progress should pass through the hairpin apex (~50 %)");
    assert!(last as f64 > 0.9 * total as f64, "the inbound leg should finish near the {total} m total, got {last}");
}

/// The 1-point / segment-less route branch: with no segment to project onto, `best` stays
/// `None`, so every fix — even one right on the point — must read off-route, frozen at 0, and
/// `u32::MAX` far. The rider is told "off route", never falsely "arrived".
#[test]
fn one_point_route_reports_off_and_far() {
    let bytes = convert("One", &gpx_from(&[(48.0, 7.8, 100.0)]));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert_eq!(r.point_count, 1);
    assert_eq!(r.chunks().len(), 1);
    assert_eq!(r.chunks()[0].point_count, 1);

    let mut m = RouteMatch::new();
    let on_point = m.update(7_800_000, 48_000_000, &r);
    assert!(on_point.off_route, "a 1-point route can't be 'on route'");
    assert_eq!(on_point.progress_m, 0);
    assert_eq!(on_point.dist_m, u32::MAX, "no segment → reported maximally far");

    let far = m.update(7_900_000, 48_100_000, &r);
    assert!(far.off_route);
    assert_eq!(far.progress_m, 0, "progress stays frozen on a segment-less route");
    assert_eq!(far.dist_m, u32::MAX);
}

/// A fix sitting exactly on chunk 1's anchor must report progress == `chunks[1].cum_distance_m`
/// — the O(1) remaining-distance join the Statistics "to go" readout depends on. Catches
/// per-chunk cumulative-bookkeeping drift mid-route, not just at the ends.
#[test]
fn progress_at_a_chunk_seam_equals_cum_distance() {
    let bytes = convert("Sawtooth", &sawtooth_gpx(400));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert!(r.chunks().len() >= 2, "need a multi-chunk route, got {}", r.chunks().len());
    let seam = r.chunks()[1];
    let seam_dist = seam.cum_distance_m;
    let (lon, lat) = (seam.anchor_lon, seam.anchor_lat);

    // Walk up to the seam so the cursor is tracking when it arrives there, then land on it.
    let pts = decode_all(&r);
    let mut m = RouteMatch::new();
    for p in &pts {
        if p.lon >= lon {
            break;
        }
        m.update(p.lon, p.lat, &r);
    }
    let at_seam = m.update(lon, lat, &r);
    assert!(!at_seam.off_route, "the seam point sits on the route");
    // Progress at the seam is exactly the stored chunk-1 cumulative distance (±1 m rounding
    // from the f32 segment metric).
    assert!(
        at_seam.progress_m.abs_diff(seam_dist) <= 1,
        "progress at the seam {} m must equal chunks[1].cum_distance_m {} m",
        at_seam.progress_m,
        seam_dist
    );
}
