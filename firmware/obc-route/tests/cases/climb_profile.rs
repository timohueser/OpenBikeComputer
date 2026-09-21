//! Per-climb detail-profile tests ([`ClimbProfile`]).
//!
//! Each test hand-builds a small multi-chunk `.obcr` in memory, so chunk boundaries, per-point
//! elevations and cumulative distances are all under the test's control, and parses it with the
//! real reader. The geometry lies along the equator (latitude 0, so cos_lat = 1) and spreads only
//! in longitude, so a segment's ground distance is a clean function of its longitude step.
//! Elevations are simple ramps, so a column's expected height is arithmetic.

use core::cell::RefCell;

use obc_formats::io::{ByteSource, Error, SliceSource};
use obc_map_scene::ground_dist_m;
use obc_route::{ClimbProfile, ClimbSeg, RouteIndex, RouteReader, CLIMB_PROFILE_COLS};

use crate::common::{ChunkExtent, ChunkIn, IndexPlacement, RouteSpec};

/// Serialize `chunks` into an in-memory `.obcr`. Returns the bytes and each chunk's data-region
/// extent, the ranges the counting source below checks were never read. The chunks do not share
/// seams: each is an independent equatorial ramp from longitude 0, told apart by its stamped
/// cumulative distance. Ascent fields stay zero, because the profile reads heights, not the totals.
fn build_obcr(chunks: &[ChunkIn], total_distance_m: u32) -> (Vec<u8>, Vec<ChunkExtent>) {
    crate::common::build_obcr(&RouteSpec {
        chunks,
        totals: (total_distance_m, 0, 0),
        index: IndexPlacement::AfterData,
        ..Default::default()
    })
}

/// Longitude step (µdeg) for a segment of ~`meters` at the equator, so evenly spaced points map
/// distance to column linearly. Derived from the builder's own metric, so test and code agree.
fn lon_step_for_m(meters: f64) -> i32 {
    let per_udeg = ground_dist_m((0, 0), (1, 0)) as f64;
    (meters / per_udeg).round() as i32
}

/// A straight equatorial climb: `n` points from `base_ele` to `top_ele`, `step_m` apart, with the
/// first point at cumulative distance `start_dist_m`. Returns the points and the exact cumulative
/// distance of the last point, after microdegree rounding.
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

/// Column placement matches within-climb distance: a constant-grade ramp puts a linearly rising
/// elevation in each column, column `i` ≈ base + gain · i/(COLS-1).
#[test]
fn column_placement_matches_within_climb_distance() {
    // A 4 km climb from 100 m to 500 m. A single chunk caps at MAX_POINTS_PER_CHUNK (256), so this
    // is two ~2 km chunks of 128 points.
    let (c0, e0) = ramp_chunk(0, 128, 16.0, 100, 300);
    let (c1, end) = ramp_chunk(e0, 128, 16.0, 300, 500);
    let (bytes, _) = build_obcr(&[c0, c1], end);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    let seg = ClimbSeg { start_m: 0, end_m: end, base_ele_m: 100, top_ele_m: 500, gain_m: 400, avg_grade_pct: 10 };
    let prof = ClimbProfile::build(&r, &seg);

    // Tolerance: one column's worth of gain (400 / (COLS-1) ≈ 2 m) plus rounding.
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

    // Tolerance: one column's worth of gain (400 / (COLS-1) ≈ 2 m) plus rounding.
    let last = CLIMB_PROFILE_COLS - 1;
    let tol = 400 / last as i32 + 3;
    for i in 0..CLIMB_PROFILE_COLS {
        let frac = i as f32 / last as f32;
        let ideal = 100 + (400.0 * frac).round() as i32;
        let got = prof.col(i) as i32;
        assert!((got - ideal).abs() <= tol, "col {i}: got {got} m, ideal {ideal} m (tol {tol})",);
    }
}

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

    // A chunk that rises then falls, with the ClimbSeg still spanning all of it.
    let dl = lon_step_for_m(20.0);
    let seg_m = ground_dist_m((0, 0), (dl, 0)) as f64;
    let n = 200usize;
    let mut points = Vec::with_capacity(n);
    for i in 0..n {
        let lon = dl * i as i32;
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

/// First and last columns equal the seg's `base_ele_m` / `top_ele_m` exactly, even though no point
/// landed on the endpoints: the builder pins them.
#[test]
fn endpoints_equal_seg_base_and_top() {
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

/// Gap-fill leaves no empty column on a sparse climb: most columns have no point of their own and
/// must inherit a neighbour, never the EMPTY sentinel (i16::MIN).
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
    let mut prev = i16::MIN;
    for i in 0..CLIMB_PROFILE_COLS {
        let v = prof.col(i);
        assert_ne!(v, i16::MIN, "col {i} was left empty (not gap-filled)");
        assert!(v >= prev, "gap-filled ramp should be non-decreasing: col {i} = {v} < prev {prev}");
        prev = v;
    }
}

/// A 3-chunk route with the climb spanning only chunk 1's distance span. Filling that climb must
/// read chunk 1's bytes and must not read chunk 0's.
#[test]
fn only_overlapping_chunks_are_read() {
    // Three ~2 km chunks laid end to end: [0,2k], [2k,4k], [4k,6k].
    let (c0, e0) = ramp_chunk(0, 200, 10.0, 100, 200);
    let (c1, e1) = ramp_chunk(e0, 200, 10.0, 200, 500); // the climb: rises 200→500 over chunk 1
    let (c2, e2) = ramp_chunk(e1, 200, 10.0, 500, 480); // gentle descent after
    let (bytes, extents) = build_obcr(&[c0, c1, c2], e2);

    let src = CountingSource::new(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    // Clear the reads made while parsing the header and index, so only the fill's reads are tested.
    src.reads.borrow_mut().clear();

    let r = RouteReader::new(&ridx, &src);
    let seg = ClimbSeg { start_m: e0, end_m: e1, base_ele_m: 200, top_ele_m: 500, gain_m: 300, avg_grade_pct: 15 };
    let mut prof = ClimbProfile::new();
    prof.fill(&r, &seg);

    // Non-vacuity: chunk 1 really was decoded.
    assert_eq!(prof.col(0), 200);
    assert_eq!(prof.col(CLIMB_PROFILE_COLS - 1), 500);
    assert!(!src.touched(extents[0]), "chunk 0 must NOT be read for a mid-route climb");
    assert!(src.touched(extents[1]), "chunk 1 (the climb) must be read");
    // Chunk 2 borders the climb at e1, so whether it is read is not part of the guarantee.
}

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

    assert!((prof.cursor_frac(start) - 0.0).abs() < 1e-6, "base maps to 0");
    let mid = start + len / 2;
    assert!((prof.cursor_frac(mid) - 0.5).abs() < 0.01, "middle maps to ~0.5");
    assert!((prof.cursor_frac(start + len) - 1.0).abs() < 1e-6, "top maps to 1");

    assert_eq!(prof.cursor_frac(start - 500), 0.0, "before the climb clamps to 0");
    assert_eq!(prof.cursor_frac(start + len + 500), 1.0, "past the summit clamps to 1");
}

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
