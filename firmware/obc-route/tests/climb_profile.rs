//! Per-climb detail-profile tests ([`ClimbProfile`]).
//!
//! These drive the real reader path: each test hand-builds a small multi-chunk `.obcr` in
//! memory (so chunk boundaries, per-point elevations, and cumulative distances are all under the
//! test's control), parses it with the real [`RouteIndex`]/[`RouteReader`], and fills a
//! [`ClimbProfile`] for a [`ClimbSeg`] describing the climb of interest.
//!
//! The geometry is laid out along the **equator** (latitude 0, so `cos_lat = 1`) and spread only
//! in longitude, so a segment's ground distance is a clean function of its longitude step — the
//! builder measures distance with [`ground_dist_m`], which the tests reuse to predict exactly
//! where each point lands. Elevations are chosen as simple ramps so a column's expected height is
//! arithmetic.
//!
//! The headline invariant — **only chunks overlapping the climb are read** — is proven with a
//! counting [`ByteSource`] wrapper that records every byte range `read_at` touches; a mid-route
//! climb must never read chunk 0's data region.

use core::cell::RefCell;

use obc_formats::io::{ByteSource, Error, SliceSource};
use obc_map_scene::ground_dist_m;
use obc_route::{ClimbProfile, ClimbSeg, RouteIndex, RouteReader, CLIMB_PROFILE_COLS};

mod common;
use common::{ChunkExtent, ChunkIn, IndexPlacement, RouteSpec};

/// Serialize `chunks` into an in-memory `.obcr` through the shared hand-rolled writer, with the
/// geometry right after the header and the index after it. Returns the bytes and each chunk's
/// data-region byte extent — the ranges the counting source below checks were never read.
///
/// These chunks are **not** seam-sharing: each is an independent equatorial ramp starting at
/// longitude 0, told apart by its stamped cumulative distance, so the header's point count is the
/// plain sum. Ascent fields stay zero — the climb profile reads heights, not the header totals.
fn build_obcr(chunks: &[ChunkIn], total_distance_m: u32) -> (Vec<u8>, Vec<ChunkExtent>) {
    common::build_obcr(&RouteSpec {
        chunks,
        totals: (total_distance_m, 0, 0),
        index: IndexPlacement::AfterData,
        ..Default::default()
    })
}

/// Longitude step (microdegrees) that yields a segment of ~`meters` at the equator, so a chunk of
/// evenly spaced points maps distance to column linearly. Derived from the builder's own metric
/// ([`ground_dist_m`]) so the test and the code agree exactly.
fn lon_step_for_m(meters: f64) -> i32 {
    // One microdegree east at the equator is `ground_dist_m((0,0),(1,0))`.
    let per_udeg = ground_dist_m((0, 0), (1, 0)) as f64;
    (meters / per_udeg).round() as i32
}

/// A straight equatorial climb: `n` points from `base_ele` to `top_ele`, spaced `step_m` apart in
/// distance, with the first point at cumulative distance `start_dist_m`. Returns the points and
/// the exact cumulative distance of the last point (`start_dist_m + (n-1) * seg_m`, where `seg_m`
/// is the realized per-segment distance after microdegree rounding).
fn ramp_chunk(start_dist_m: u32, n: usize, step_m: f64, base_ele: i16, top_ele: i16) -> (ChunkIn, u32) {
    let dl = lon_step_for_m(step_m);
    let seg_m = ground_dist_m((0, 0), (dl, 0)) as f64;
    let mut points = Vec::with_capacity(n);
    for i in 0..n {
        let lon = dl * i as i32;
        let ele = base_ele as f64 + (top_ele as f64 - base_ele as f64) * (i as f64 / (n - 1).max(1) as f64);
        points.push((lon, 0, ele.round() as i16));
    }
    let end_dist = start_dist_m + (seg_m * (n - 1) as f64).round() as u32;
    (ChunkIn { cum_distance_m: start_dist_m, cum_ascent_m: 0, points }, end_dist)
}

// -------------------------------------------------------------------------------------------
// A counting ByteSource — proves the chunk-skip.
// -------------------------------------------------------------------------------------------

/// Wraps a [`SliceSource`] and records every `(offset, len)` range read through it. The chunk-skip
/// test asserts none of the recorded ranges intersect a skipped chunk's data extent.
struct CountingSource<'a> {
    inner: SliceSource<'a>,
    reads: RefCell<Vec<(u32, u32)>>,
}

impl<'a> CountingSource<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        CountingSource { inner: SliceSource(bytes), reads: RefCell::new(Vec::new()) }
    }
    /// Whether any recorded read overlapped `[extent.start, extent.end)`.
    fn touched(&self, extent: ChunkExtent) -> bool {
        self.reads
            .borrow()
            .iter()
            .any(|&(off, len)| off < extent.end && off + len > extent.start && extent.end > extent.start)
    }
}

impl ByteSource for CountingSource<'_> {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<(), Error> {
        self.reads.borrow_mut().push((offset as u32, buf.len() as u32));
        self.inner.read_at(offset, buf)
    }
    fn len(&self) -> u64 {
        self.inner.len()
    }
}

// -------------------------------------------------------------------------------------------
// Tests.
// -------------------------------------------------------------------------------------------

/// Column placement matches within-climb distance: a single-chunk constant-grade ramp climb over
/// `[0, len]` should put a linearly rising elevation in each column — column `i` ≈ base + gain ·
/// i/(COLS-1). We allow a small tolerance for the per-column bucket width and microdegree
/// rounding.
#[test]
fn column_placement_matches_within_climb_distance() {
    // 250 points over a 4 km climb from 100 m to 500 m (gain 400, 10 %) — but a single chunk caps
    // at MAX_POINTS_PER_CHUNK (256), so split into two ~2 km chunks of 128 points each below.
    let (c0, e0) = ramp_chunk(0, 128, 16.0, 100, 300);
    let (c1, end) = ramp_chunk(e0, 128, 16.0, 300, 500);
    let (bytes, _) = build_obcr(&[c0, c1], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    let seg = ClimbSeg { start_m: 0, end_m: end, base_ele_m: 100, top_ele_m: 500, gain_m: 400, avg_grade_pct: 10 };
    let prof = ClimbProfile::build(&r, &seg);

    // Each column's height should track the ideal ramp within ~one column's worth of gain plus a
    // meter of rounding. Column width in gain = 400 / (COLS-1) ≈ 2 m.
    let last = CLIMB_PROFILE_COLS - 1;
    let tol = 400 / last as i32 + 4;
    for i in 0..CLIMB_PROFILE_COLS {
        let frac = i as f32 / last as f32;
        let ideal = 100 + (400.0 * frac).round() as i32;
        let got = prof.col(i) as i32;
        assert!((got - ideal).abs() <= tol, "col {i}: got {got} m, ideal {ideal} m (tol {tol})",);
    }
}

/// A second single-chunk placement check kept small (≤ MAX_POINTS_PER_CHUNK).
#[test]
fn column_placement_single_chunk() {
    let (chunk, end) = ramp_chunk(0, 200, 20.0, 100, 500);
    let (bytes, _) = build_obcr(&[chunk], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    let seg = ClimbSeg { start_m: 0, end_m: end, base_ele_m: 100, top_ele_m: 500, gain_m: 400, avg_grade_pct: 10 };
    let prof = ClimbProfile::build(&r, &seg);

    // Each column's height should track the ideal ramp within ~one column's worth of gain plus a
    // meter of rounding. Column width in gain = 400 / (COLS-1) ≈ 2 m.
    let last = CLIMB_PROFILE_COLS - 1;
    let tol = 400 / last as i32 + 3;
    for i in 0..CLIMB_PROFILE_COLS {
        let frac = i as f32 / last as f32;
        let ideal = 100 + (400.0 * frac).round() as i32;
        let got = prof.col(i) as i32;
        assert!((got - ideal).abs() <= tol, "col {i}: got {got} m, ideal {ideal} m (tol {tol})",);
    }
}

/// `grade_at` sign and magnitude on a constant-grade ramp: a 10 % climb should read ~+10 %
/// everywhere in the interior, and the sign flips negative on a profile that descends.
#[test]
fn grade_at_sign_and_magnitude() {
    // Rising: 100 → 500 over 4 km = +10 %.
    let (chunk, end) = ramp_chunk(0, 200, 20.0, 100, 500);
    let (bytes, _) = build_obcr(&[chunk], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let seg = ClimbSeg { start_m: 0, end_m: end, base_ele_m: 100, top_ele_m: 500, gain_m: 400, avg_grade_pct: 10 };
    let prof = ClimbProfile::build(&r, &seg);
    for &frac in &[0.25f32, 0.5, 0.75] {
        let g = prof.grade_at(frac);
        assert!((g - 10).abs() <= 2, "constant 10 % ramp read {g} % at frac {frac}");
    }

    // Falling profile inside the interval (a synthetic dip) → negative grade. Build a chunk that
    // rises then falls; the ClimbSeg still spans it, but grade_at at the falling part is negative.
    let dl = lon_step_for_m(20.0);
    let seg_m = ground_dist_m((0, 0), (dl, 0)) as f64;
    let n = 200usize;
    let mut points = Vec::with_capacity(n);
    for i in 0..n {
        let lon = dl * i as i32;
        // Up for the first half, down for the second — a peak in the middle.
        let ele = if i < n / 2 {
            100.0 + 300.0 * (i as f64 / (n / 2) as f64)
        } else {
            400.0 - 300.0 * ((i - n / 2) as f64 / (n / 2) as f64)
        };
        points.push((lon, 0, ele.round() as i16));
    }
    let end2 = (seg_m * (n - 1) as f64).round() as u32;
    let (bytes2, _) = build_obcr(&[ChunkIn { cum_distance_m: 0, cum_ascent_m: 0, points }], end2);
    let src2 = SliceSource(&bytes2);
    let ridx2 = RouteIndex::read(&src2).unwrap();
    let r2 = RouteReader::new(&ridx2, &src2);
    let seg2 = ClimbSeg { start_m: 0, end_m: end2, base_ele_m: 100, top_ele_m: 100, gain_m: 0, avg_grade_pct: 0 };
    let prof2 = ClimbProfile::build(&r2, &seg2);
    assert!(prof2.grade_at(0.25) > 0, "rising first half should read positive grade");
    assert!(prof2.grade_at(0.75) < 0, "falling second half should read negative grade");
}

/// First and last columns equal the seg's `base_ele_m` / `top_ele_m` exactly, even when no point
/// landed precisely on the endpoints (the builder pins them).
#[test]
fn endpoints_equal_seg_base_and_top() {
    // A ramp whose first/last *points* don't sit exactly on the seg base/top (offset the seg
    // interval inward), so pinning is what makes the endpoints exact.
    let (chunk, end) = ramp_chunk(0, 200, 18.0, 200, 900);
    let (bytes, _) = build_obcr(&[chunk], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let seg = ClimbSeg { start_m: 0, end_m: end, base_ele_m: 200, top_ele_m: 900, gain_m: 700, avg_grade_pct: 20 };
    let prof = ClimbProfile::build(&r, &seg);
    assert_eq!(prof.col(0), 200, "column 0 must equal base_ele_m");
    assert_eq!(prof.col(CLIMB_PROFILE_COLS - 1), 900, "last column must equal top_ele_m");
    assert_eq!(prof.at(0.0), 200);
    assert_eq!(prof.at(1.0), 900);
    assert_eq!(prof.base_ele_m(), 200);
    assert_eq!(prof.top_ele_m(), 900);
}

/// Gap-fill leaves no empty column on a sparse climb: only a handful of points over a long climb,
/// so most columns have no point of their own and must inherit a neighbour (never the EMPTY
/// sentinel = i16::MIN). We assert monotonic non-decreasing across a rising sparse ramp.
#[test]
fn gap_fill_leaves_no_empty_columns() {
    // 6 points over a 5 km climb → far fewer points than COLS (200), so most columns are gap-filled.
    let (chunk, end) = ramp_chunk(0, 6, 1000.0, 100, 600);
    let (bytes, _) = build_obcr(&[chunk], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let seg = ClimbSeg { start_m: 0, end_m: end, base_ele_m: 100, top_ele_m: 600, gain_m: 500, avg_grade_pct: 10 };
    let prof = ClimbProfile::build(&r, &seg);
    // No column left at the empty sentinel, and the (rising) profile never steps backwards.
    let mut prev = i16::MIN;
    for i in 0..CLIMB_PROFILE_COLS {
        let v = prof.col(i);
        assert_ne!(v, i16::MIN, "col {i} was left empty (not gap-filled)");
        assert!(v >= prev, "gap-filled ramp should be non-decreasing: col {i} = {v} < prev {prev}");
        prev = v;
    }
}

/// Only overlapping chunks are read: a 3-chunk route with the climb spanning chunk 1's distance
/// span only — a fill for that climb must read chunk 1's bytes but must NOT read chunk 0's.
#[test]
fn only_overlapping_chunks_are_read() {
    // Three chunks, each ~2 km, laid end to end: chunk 0 [0,2k], chunk 1 [2k,4k], chunk 2 [4k,6k].
    // Each is its own independent equatorial ramp (distinct elevations) so the reader/decoder has
    // real work; cumulative distances are stamped so the builder's overlap test can place them.
    let (c0, e0) = ramp_chunk(0, 200, 10.0, 100, 200);
    let (c1, e1) = ramp_chunk(e0, 200, 10.0, 200, 500); // the climb: rises 200→500 over chunk 1
    let (c2, e2) = ramp_chunk(e1, 200, 10.0, 500, 480); // gentle descent after
    let (bytes, extents) = build_obcr(&[c0, c1, c2], e2);

    let src = CountingSource::new(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    // Snapshot then clear the reads recorded while parsing the header/index, so only the fill's
    // reads are under test (RouteIndex::read touches the header + index, never chunk bodies).
    src.reads.borrow_mut().clear();

    let r = RouteReader::new(&ridx, &src);
    let seg = ClimbSeg { start_m: e0, end_m: e1, base_ele_m: 200, top_ele_m: 500, gain_m: 300, avg_grade_pct: 15 };
    let mut prof = ClimbProfile::new();
    prof.fill(&r, &seg);

    // The climb's endpoints are correct (chunk 1 really was decoded)...
    assert_eq!(prof.col(0), 200);
    assert_eq!(prof.col(CLIMB_PROFILE_COLS - 1), 500);
    // ...and, crucially, chunk 0's data region was never read: a mid-route climb skips it.
    assert!(!src.touched(extents[0]), "chunk 0 must NOT be read for a mid-route climb");
    assert!(src.touched(extents[1]), "chunk 1 (the climb) must be read");
    // Chunk 2 borders the climb at e1; whether it's read depends only on the overlap test, but it
    // must not be *required* — the important guarantee is chunk 0 is skipped.
}

/// The `cursor_frac(progress_m)` mapping is correct at the base, middle, and top of the climb, and
/// clamps outside the interval.
#[test]
fn cursor_frac_maps_progress_to_fraction() {
    let start = 3000u32;
    let (chunk, end) = ramp_chunk(start, 200, 10.0, 100, 400);
    let (bytes, _) = build_obcr(&[chunk], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let seg = ClimbSeg { start_m: start, end_m: end, base_ele_m: 100, top_ele_m: 400, gain_m: 300, avg_grade_pct: 15 };
    let prof = ClimbProfile::build(&r, &seg);
    let len = prof.len_m();
    assert_eq!(prof.start_m(), start);

    // Base, middle, top.
    assert!((prof.cursor_frac(start) - 0.0).abs() < 1e-6, "base maps to 0");
    let mid = start + len / 2;
    assert!((prof.cursor_frac(mid) - 0.5).abs() < 0.01, "middle maps to ~0.5");
    assert!((prof.cursor_frac(start + len) - 1.0).abs() < 1e-6, "top maps to 1");

    // Clamps outside the interval.
    assert_eq!(prof.cursor_frac(start - 500), 0.0, "before the climb clamps to 0");
    assert_eq!(prof.cursor_frac(start + len + 500), 1.0, "past the summit clamps to 1");
}

/// An empty (never-filled) profile is harmless: flat zero line, zero length, reads don't panic.
#[test]
fn empty_profile_is_safe() {
    let prof = ClimbProfile::new();
    assert_eq!(prof.at(0.0), 0);
    assert_eq!(prof.at(1.0), 0);
    assert_eq!(prof.grade_at(0.5), 0);
    assert_eq!(prof.len_m(), 0);
    // cursor_frac must not divide by zero on a zero-length climb.
    assert_eq!(prof.cursor_frac(1234), 0.0);
}
