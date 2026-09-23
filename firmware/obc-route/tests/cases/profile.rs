//! Elevation-profile tests: convert a synthetic GPX, build the profile from the reader, and check
//! that it captures the route's shape (the peak, the y-range, a gap-free band) whatever the
//! sampling density of the route.

use obc_formats::io::SliceSource;
use obc_route::{RouteIndex, RouteReader, PROFILE_COLS};

use crate::common::convert;

/// Densely scan one pyramid `level` across `[lo, hi]` and return its `(min, max)` envelope, using
/// only the public [`Profile::sample`] API.
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
/// 200 m, a single clean peak at the route's midpoint by distance.
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

    assert_eq!((p.min_ele_m, p.max_ele_m), (r.min_ele_m, r.max_ele_m));
    assert_eq!((p.min_ele_m, p.max_ele_m), (200, 300));

    // Expressed in PROFILE_COLS, so the bound tracks the base-resolution knob.
    assert_eq!(p.peak_ele_m(), 300);
    assert!(
        (PROFILE_COLS * 3 / 8..=PROFILE_COLS * 5 / 8).contains(&p.peak_col),
        "peak_col {} not near the middle",
        p.peak_col
    );
    assert!((0.375..=0.625).contains(&p.peak_frac()), "peak_frac {} not near 0.5", p.peak_frac());

    // The exact peak column drifts with the base resolution, so read it at peak_frac, not at 0.5.
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

    assert_eq!(p.ascent_to(0.0), 0);
    assert_eq!(p.ascent_to(1.0), r.total_ascent_m);
    assert_eq!(p.ascent_to(1.5), r.total_ascent_m);

    // PEAKED climbs and then descends, so essentially all of the ascent is done by the peak.
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

/// Pyramid depth. A structural constant: only the base width (`PROFILE_COLS`) is tunable, not the
/// number of levels.
const PYRAMID_LEVELS: usize = 4;

#[test]
fn pyramid_downsample_keeps_extremes() {
    // The coarse levels are min/max merges, not averages, so every level still spans the full
    // 200..300 m envelope.
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let p = r.elevation_profile();

    for level in 0..PYRAMID_LEVELS {
        assert_eq!(level_envelope(&p, level, 0.0, 1.0), (200, 300), "level {level} lost the envelope");
    }
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

/// A route with no `<ele>` anywhere. The converter stores 0 m elevation and a 0..0 header range,
/// so the band must stay gap-free through `fill_gaps`'s header fallback, the only path that uses it.
const NO_ELE: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"/>
  <trkpt lat="48.0050" lon="7.8000"/>
  <trkpt lat="48.0100" lon="7.8000"/>
</trkseg></trk></gpx>"#;

#[test]
fn no_elevation_route_has_unknown_band() {
    let bytes = convert("Unmeasured", NO_ELE);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    assert_eq!((r.min_ele_m, r.max_ele_m), (0, 0), "no <ele> → 0..0 header range");
    let p = r.elevation_profile();

    assert_eq!((p.min_ele_m, p.max_ele_m), (0, 0));
    assert_eq!(p.peak_ele_m(), i16::MIN);
    for (i, &(mn, mx)) in p.cols().iter().enumerate() {
        assert!(mn > mx, "column {i} has no measured elevation");
    }
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

    // At the very start or end, the fixed-width span slides flush against the edge.
    let start = p.window(0.0, 4.0, 216);
    assert_eq!(start.lo_frac, 0.0);
    assert!((start.hi_frac - 0.25).abs() < 1e-4);
    let end = p.window(1.0, 4.0, 216);
    assert_eq!(end.hi_frac, 1.0);
    assert!((end.lo_frac - 0.75).abs() < 1e-4);
}

/// The received-route card's mini sparkline: a min-max normalized `u8` band.
#[test]
fn sparkline_normalizes_and_peaks_mid() {
    use obc_route::{elevation_sparkline, SPARKLINE_BUCKETS};
    let bytes = convert("Peaked Ridge", PEAKED);
    let spark = elevation_sparkline(&SliceSource(&bytes)).expect("a route with elevation has a band");
    assert_eq!(spark.len(), SPARKLINE_BUCKETS);
    assert_eq!(*spark.iter().max().unwrap(), 255, "the peak pins to 255");
    assert_eq!(spark[0], 0, "the start sits at the min");
    assert_eq!(spark[SPARKLINE_BUCKETS - 1], 0, "the end sits at the min");
    let peak_b = spark.iter().position(|&v| v == 255).unwrap();
    assert!(
        (SPARKLINE_BUCKETS * 3 / 8..=SPARKLINE_BUCKETS * 5 / 8).contains(&peak_b),
        "peak bucket {peak_b} not near the middle"
    );
}

/// A flat route (a real but constant elevation) and an unmeasured one (no `<ele>`) both carry no
/// usable range, so the card omits the band rather than drawing a fake flat line.
#[test]
fn sparkline_is_none_without_a_range() {
    use obc_route::elevation_sparkline;
    assert!(elevation_sparkline(&SliceSource(&convert("Towpath", FLAT))).is_none(), "flat → no band");
    assert!(elevation_sparkline(&SliceSource(&convert("Unmeasured", NO_ELE))).is_none(), "no <ele> → no band");
}

#[test]
fn sparkline_refuses_incomplete_segments_and_unreadable_chunks() {
    use crate::common::{build_obcr, ChunkIn, RouteSpec};
    use obc_formats::io::{ByteSource, Error};
    let chunks = [
        ChunkIn { points: vec![(0, 0, 10), (1000, 0, 20)], cum_distance_m: 0, cum_ascent_m: 0 },
        ChunkIn { points: vec![(1000, 0, 20), (2000, 0, 30)], cum_distance_m: 111, cum_ascent_m: 10 },
    ];
    let (bytes, extents) =
        build_obcr(&RouteSpec { chunks: &chunks, totals: (222, 20, 0), seam_shared: true, ..RouteSpec::default() });
    assert!(obc_route::elevation_sparkline(&SliceSource(&bytes)).is_some());
    let mut gap = bytes.clone();
    gap[extents[1].start as usize + 6] = 8;
    assert!(
        obc_route::elevation_sparkline(&SliceSource(&gap)).is_none(),
        "valid endpoints cannot fill an incomplete span"
    );

    struct Fault<'a> {
        bytes: &'a [u8],
        offset: u64,
    }
    impl ByteSource for Fault<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            if offset == self.offset {
                return Err(Error::Io);
            }
            SliceSource(self.bytes).read_at(offset, out)
        }
    }
    let index_offset = u32::from_le_bytes(bytes[56..60].try_into().unwrap()) as u64;
    for offset in [index_offset + obc_formats::obcr::CHUNK_META_LEN as u64, extents[1].start as u64] {
        assert!(
            obc_route::elevation_sparkline(&Fault { bytes: &bytes, offset }).is_none(),
            "a failed later chunk must not be filled from the earlier chunk"
        );
    }
    let mut malformed = bytes;
    let count_at = index_offset as usize + obc_formats::obcr::CHUNK_META_LEN + 26;
    malformed[count_at..count_at + 2].copy_from_slice(&0u16.to_le_bytes());
    assert!(obc_route::elevation_sparkline(&SliceSource(&malformed)).is_none());
}

/// A day of stretches climbs what each route's own profile says between the stretch's ends: the
/// rest of one route up to its peak, then a whole route, which climbs its header figure.
#[test]
fn a_day_of_stretches_climbs_what_each_route_profile_says() {
    let bytes = convert("Peaked Ridge", PEAKED);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);
    let whole = r.elevation_profile();
    let (total, half) = (r.total_distance_m, r.total_distance_m / 2);

    let mut out = obc_route::Profile::EMPTY;
    let mut day = obc_route::DayProfile::start(half + total, &mut out);
    day.stretch(&src, 0, half, &mut out).unwrap();
    day.stretch(&src, 0, u32::MAX, &mut out).unwrap();
    let climb = day.finish(&mut out);

    assert_eq!(climb, whole.ascent_between_m(0, half, total) + r.total_ascent_m);
    assert_eq!(out.ascent_to(1.0), climb);
    assert_eq!((out.min_ele_m, out.max_ele_m), (200, 300));
    assert!(out.cols().iter().all(|&(mn, mx)| mn <= mx), "a gap-free band");
}

#[test]
fn combined_summaries_stream_once_and_keep_climb_gap_and_read_error_behavior() {
    use crate::common::{build_obcr, ChunkIn, RouteSpec};
    use core::cell::Cell;
    use obc_formats::io::{ByteSource, Error};

    struct Source<'a> {
        bytes: &'a [u8],
        reads: Cell<usize>,
        fail_at: Option<u64>,
        fail_once: bool,
        failed: Cell<bool>,
    }
    impl ByteSource for Source<'_> {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            self.reads.set(self.reads.get() + 1);
            if self.fail_at == Some(offset) && (!self.fail_once || !self.failed.get()) {
                self.failed.set(true);
                return Err(Error::BadOffset);
            }
            SliceSource(self.bytes).read_at(offset, out)
        }
    }
    let chunks: Vec<_> = (0..8)
        .map(|k| ChunkIn {
            points: (0..32)
                .map(|i| {
                    let station = k * 31 + i;
                    let height = if station == 110 { i16::MIN } else { (100 + (station % 70) * 3) as i16 };
                    (station * 200, 0, height)
                })
                .collect(),
            cum_distance_m: k as u32 * 690,
            cum_ascent_m: 0,
        })
        .collect();
    let (bytes, extents) =
        build_obcr(&RouteSpec { chunks: &chunks, totals: (5520, 600, 500), seam_shared: true, ..Default::default() });
    let index = RouteIndex::read(&SliceSource(&bytes)).unwrap();
    for (fail_at, fail_once) in
        [(None, false), (Some(extents[3].start as u64), false), (Some(extents[3].start as u64), true)]
    {
        let source = Source { bytes: &bytes, reads: Cell::new(0), fail_at, fail_once, failed: Cell::new(false) };
        let route = RouteReader::new(&index, &source);
        let expected_climbs = route.detect_climbs();
        let expected = route.elevation_profile();
        let separate_reads = source.reads.get();
        source.reads.set(0);
        source.failed.set(false);
        let mut profile = obc_route::Profile::EMPTY;
        let climbs = route.elevation_profile_and_climbs_into(&mut profile);
        if fail_at.is_none() {
            assert_eq!(separate_reads, 2 * chunks.len());
            assert_eq!(source.reads.get(), chunks.len(), "healthy summaries share one pass beyond cache capacity");
        } else {
            assert_eq!(source.reads.get(), separate_reads, "a failed pass retains the independent profile retry");
        }
        assert_eq!(climbs.as_slice(), expected_climbs.as_slice());
        assert_eq!(profile.cols(), expected.cols());
        assert_eq!(
            (profile.min_ele_m, profile.max_ele_m, profile.peak_col),
            (expected.min_ele_m, expected.max_ele_m, expected.peak_col)
        );
        for col in 0..PROFILE_COLS {
            let fraction = col as f32 / (PROFILE_COLS - 1) as f32;
            assert_eq!(profile.grade_at(fraction), expected.grade_at(fraction));
            assert_eq!(profile.ascent_to(fraction), expected.ascent_to(fraction));
        }
    }
}
