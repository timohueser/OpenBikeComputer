//! Route-matcher tests: convert a synthetic GPX, open the reader, then drive
//! [`RouteMatch`] with crafted fixes — on the line, beside it, rejoining, and around a
//! loop — checking progress, the off-route flag/freeze, and cross-track distance. Fixes
//! are built from the *decoded* geometry, so the assertions don't depend on exactly which
//! vertices survived decimation.

use heapless::Vec as HVec;
use obc_route::{
    gpx_to_obcr, ByteSink, Error, RouteMatch, RoutePoint, RouteReader, SliceSource,
    MAX_POINTS_PER_CHUNK,
};

/// A `ByteSink` over a growable `Vec` (mirrors the profile test's backing).
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

/// Build GPX text from `(lat_deg, lon_deg, ele_m)` track points.
fn gpx_from(pts: &[(f64, f64, f64)]) -> String {
    let mut s = String::from("<?xml version=\"1.0\"?><gpx><trk><trkseg>");
    for &(lat, lon, ele) in pts {
        s.push_str(&format!(
            "<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{ele}</ele></trkpt>"
        ));
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
    let r = RouteReader::open(&src).unwrap();
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
        assert!(
            res.progress_m.abs_diff(want) <= 5,
            "fix {i}: progress {} m, expected ~{want} m",
            res.progress_m
        );
        last = res.progress_m;
    }
}

#[test]
fn cross_track_distance_matches_a_known_offset() {
    let bytes = convert("East", &gpx_from(EAST));
    let src = SliceSource(&bytes);
    let r = RouteReader::open(&src).unwrap();
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
    let r = RouteReader::open(&src).unwrap();
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

/// A closed ~800 m-radius loop (meter-corrected so it's round on the ground) sampled at 20
/// vertices — enough curvature that every vertex survives decimation, giving ~20 segments.
/// The loop returns to its start, so spatial nearest-point is ambiguous there; only the
/// forward bias keeps progress from snapping back.
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
    let r = RouteReader::open(&src).unwrap();
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
    assert!(
        last as f64 > 0.9 * total as f64,
        "final progress {last} m should reach near the {total} m total"
    );
}

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
    let r = RouteReader::open(&src).unwrap();
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
/// ~2 m north of the A start (a real GPS retrace never lands exactly on its start). Start
/// and finish sit nearly on top of each other, so a small north offset at the start makes
/// the *finish* segment marginally the nearest — which latched the cursor onto progress ≈
/// total before the first-lock earliest-bias fix, tripping spurious off-route. The bowed
/// outbound keeps the mid-route unambiguous (the legs are tens of metres apart there), so
/// the test isolates the genuine failure: the first lock.
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
    let r = RouteReader::open(&src).unwrap();
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
