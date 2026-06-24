//! GPX → OBCR conversion tests: convert an in-memory GPX, read it back with the
//! reader, and check the geometry round-trips and the stats are exact.

use obc_route::{gpx_to_obcr, Error, RouteIndex, RoutePoint, RouteReader, SliceSource};

mod common;
use common::{convert, decode, VecSink};

/// A straight, gently rolling eastward track. The four points are collinear, so the
/// geometry decimates to its two endpoints — but the stats come from every raw point.
const STRAIGHT: &str = r#"<?xml version="1.0"?>
<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"><ele>200.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8030"><ele>210.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8060"><ele>225.0</ele></trkpt>
  <trkpt lat="48.0000" lon="7.8090"><ele>215.0</ele></trkpt>
</trkseg></trk></gpx>"#;

#[test]
fn straight_track_stats_and_decimation() {
    let bytes = convert("Rhine Path", STRAIGHT);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.name(), "Rhine Path");
    assert_eq!(r.start_lon, 7_800_000);
    assert_eq!(r.start_lat, 48_000_000);

    // ~223 m per 0.003° lon step at 48° lat × 3 steps ≈ 670 m.
    assert!((665..=675).contains(&r.total_distance_m), "dist {}", r.total_distance_m);
    // Ascent: +10 then +15 (both over the 3 m dead-band); the −10 is descent.
    assert_eq!(r.total_ascent_m, 25);
    assert_eq!(r.total_descent_m, 10);
    assert_eq!(r.min_ele_m, 200);
    assert_eq!(r.max_ele_m, 225);

    // Collinear → decimated to the two endpoints, one chunk.
    assert_eq!(r.point_count, 2);
    assert_eq!(r.chunks().len(), 1);
    let pts = decode(&r, 0);
    assert_eq!(
        pts,
        vec![
            RoutePoint { lon: 7_800_000, lat: 48_000_000, ele: 200 },
            RoutePoint { lon: 7_809_000, lat: 48_000_000, ele: 215 },
        ]
    );
}

/// An L-shaped track: north then east. The decimator must keep the corner.
const CORNER: &str = r#"<gpx><trk><trkseg>
  <trkpt lat="48.0000" lon="7.8000"/>
  <trkpt lat="48.0100" lon="7.8000"/>
  <trkpt lat="48.0100" lon="7.8100"/>
</trkseg></trk></gpx>"#;

#[test]
fn corner_is_preserved() {
    let bytes = convert("Jura Heights", CORNER);
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    // The corner vertex is kept: all three points survive decimation.
    assert_eq!(r.point_count, 3);
    let pts = decode(&r, 0);
    assert_eq!(pts.len(), 3);
    assert_eq!(pts[1], RoutePoint { lon: 7_800_000, lat: 48_010_000, ele: 0 });
}

#[test]
fn empty_gpx_is_an_error() {
    let src = SliceSource(b"<gpx></gpx>".as_slice());
    let mut sink = VecSink::default();
    assert_eq!(gpx_to_obcr(&src, "x", &mut sink), Err(Error::Empty));
}

/// Build GPX text from `(lat_deg, lon_deg, ele_m?)` track points; `None` omits `<ele>`
/// entirely (a planner export with no elevation). The `const &str` fixtures above can't
/// express either an omitted `<ele>` or computed coordinates, which the decimation /
/// elevation tests below need.
fn gpx(pts: &[(f64, f64, Option<f64>)]) -> String {
    let mut s = String::from("<?xml version=\"1.0\"?><gpx><trk><trkseg>");
    for &(lat, lon, ele) in pts {
        match ele {
            Some(e) => s.push_str(&format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"><ele>{e}</ele></trkpt>")),
            None => s.push_str(&format!("<trkpt lat=\"{lat:.6}\" lon=\"{lon:.6}\"/>")),
        }
    }
    s.push_str("</trkseg></trk></gpx>");
    s
}

/// Item 8 (decimation tolerance) — **a vertex *just inside* `EPSILON_M` is dropped.** The
/// existing `STRAIGHT` fixture is perfectly collinear, so the perpendicular-distance test
/// (`convert.rs`, `EPSILON_M=1.0`) never actually fires on a non-zero deviation. Here the
/// middle point bulges 0.8 m off the A→C chord — inside the 1 m tolerance — so the decimator
/// must drop it, leaving just the two endpoints. This is the branch that decides whether a
/// near-straight road keeps spurious wobble vertices (storage) or smooths them.
#[test]
fn vertex_just_inside_epsilon_is_decimated() {
    let dlat = 0.8 / 111_320.0; // ~0.8 m north — inside EPSILON_M = 1.0 m
    let bytes = convert(
        "Nearly Straight",
        &gpx(&[(48.0, 7.800, Some(100.0)), (48.0 + dlat, 7.801, Some(100.0)), (48.0, 7.802, Some(100.0))]),
    );
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.point_count, 2, "a 0.8 m bulge is within tolerance → the middle vertex is dropped");
    let pts = decode(&r, 0);
    assert_eq!(
        pts,
        vec![
            RoutePoint { lon: 7_800_000, lat: 48_000_000, ele: 100 },
            RoutePoint { lon: 7_802_000, lat: 48_000_000, ele: 100 },
        ]
    );
}

/// Item 8 (decimation tolerance) — **a vertex *just outside* `EPSILON_M` is kept.** The
/// companion to the test above: bump the same middle point to 1.5 m off the chord — past the
/// 1 m tolerance — and the decimator must keep all three, preserving the bend. Together the
/// two pin both sides of the `perp > EPSILON_M` decision the collinear fixture never reached.
#[test]
fn vertex_just_outside_epsilon_is_kept() {
    let dlat = 1.5 / 111_320.0; // ~1.5 m north — outside EPSILON_M = 1.0 m
    let bytes = convert(
        "Bent",
        &gpx(&[(48.0, 7.800, Some(100.0)), (48.0 + dlat, 7.801, Some(100.0)), (48.0, 7.802, Some(100.0))]),
    );
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.point_count, 3, "a 1.5 m deviation exceeds tolerance → the bend vertex is kept");
    let pts = decode(&r, 0);
    // The kept middle vertex: lon 7_801_000, lat rounded from 48.0 + 1.5/111320 deg = 48_000_013.
    assert_eq!(pts[1], RoutePoint { lon: 7_801_000, lat: 48_000_013, ele: 100 });
}

/// Item 8 (densification / `MAX_SPAN_M`) — **a long collinear run keeps an intermediate vertex
/// so deltas stay inside `int16`.** `MAX_SPAN_M=1200` forces a kept vertex at least that often,
/// which also bounds the stored `(Δlon, Δlat)` to the `int16` range. Three collinear points
/// 0.03° apart (~2234 m/segment) would, if the middle were dropped as collinear, leave a single
/// segment whose Δ overflows `int16`; the span rule keeps the middle, so the decoded geometry
/// round-trips exactly. Guards against the silent geometry corruption item 8 warns about.
#[test]
fn long_collinear_run_keeps_an_intermediate_vertex() {
    let bytes = convert(
        "Long Straight",
        &gpx(&[(48.0, 7.80, Some(100.0)), (48.0, 7.83, Some(100.0)), (48.0, 7.86, Some(100.0))]),
    );
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    // The middle point survives despite being collinear — kept by the MAX_SPAN_M rule.
    assert_eq!(r.point_count, 3, "the span rule must keep the middle of a ~4.5 km collinear run");
    let pts = decode(&r, 0);
    // Decoded geometry round-trips exactly — no int16 wrap.
    assert_eq!(
        pts,
        vec![
            RoutePoint { lon: 7_800_000, lat: 48_000_000, ele: 100 },
            RoutePoint { lon: 7_830_000, lat: 48_000_000, ele: 100 },
            RoutePoint { lon: 7_860_000, lat: 48_000_000, ele: 100 },
        ]
    );
}

/// Item 8 (densification / int16 safety) — **a single oversized segment with no intermediate
/// raw candidate is split so the stored Δ never wraps** (issue #110). `MAX_SPAN_M` only
/// force-keeps a *pending* candidate between two kept points; a 2-point GPX has none, so a 0.04°
/// (~3.3 km) lon step (40_000 µdeg) used to be stored as `40000 as i16`, wrapping to `-25_536`
/// and decoding to a corrupt 7_774_464. The converter now densifies the span itself — splitting
/// it into ≤`MAX_SEGMENT_UDEG` (30_000) pieces with interpolated vertices — so the geometry
/// round-trips exactly. This test asserts the *fixed* behaviour (was `…_overflows_int16_today`,
/// which pinned the bug).
#[test]
fn single_oversized_segment_is_densified() {
    let bytes = convert("Densified", &gpx(&[(48.0, 7.80, Some(100.0)), (48.0, 7.84, Some(100.0))]));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    // 40_000 µdeg > 30_000 → one synthetic midpoint, so three stored vertices.
    assert_eq!(r.point_count, 3, "the oversized span is split with one interpolated vertex");
    let pts = decode(&r, 0);
    assert_eq!(pts[0].lon, 7_800_000, "the anchor is stored absolutely and is correct");
    assert_eq!(pts[1].lon, 7_820_000, "the interpolated midpoint sits halfway along the span");
    assert_eq!(pts[2].lon, 7_840_000, "the endpoint round-trips exactly — no int16 wrap");
    // The interpolated vertices stay on the flat, equator-parallel line.
    assert!(pts.iter().all(|p| p.lat == 48_000_000 && p.ele == 100), "interpolated vertices stay on the line");
}

/// Item 8 (densification, multi-step + diagonal) — **a span far past one `MAX_SEGMENT_UDEG`
/// piece is split into several, interpolating both axes and elevation.** A 2-point diagonal of
/// 0.09° (90_000 µdeg) in lon *and* lat needs `90_000 / 30_000 + 1 = 4` pieces (three synthetic
/// vertices). Pins that every consecutive Δ stays inside `int16`, the endpoints round-trip
/// exactly, and elevation is carried linearly across the inserted vertices.
#[test]
fn oversized_diagonal_span_splits_into_several() {
    let bytes = convert("Diagonal", &gpx(&[(48.0, 7.80, Some(100.0)), (48.09, 7.89, Some(200.0))]));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.point_count, 5, "anchor + three interpolated vertices + endpoint");
    let pts = decode(&r, 0);
    assert_eq!(
        pts,
        vec![
            RoutePoint { lon: 7_800_000, lat: 48_000_000, ele: 100 },
            RoutePoint { lon: 7_822_500, lat: 48_022_500, ele: 125 },
            RoutePoint { lon: 7_845_000, lat: 48_045_000, ele: 150 },
            RoutePoint { lon: 7_867_500, lat: 48_067_500, ele: 175 },
            RoutePoint { lon: 7_890_000, lat: 48_090_000, ele: 200 },
        ]
    );
    // No stored (Δlon, Δlat) can overflow the int16 the reader decodes them as.
    for w in pts.windows(2) {
        assert!((w[1].lon - w[0].lon).abs() <= i16::MAX as i32, "Δlon fits int16");
        assert!((w[1].lat - w[0].lat).abs() <= i16::MAX as i32, "Δlat fits int16");
    }
}

/// Item 10 (convert: carry-last-known elevation) — **a point missing `<ele>` carries the last
/// known height** (`convert.rs`, the `if let Some(e) = p.ele` carry). Every existing fixture
/// has `<ele>` on every point, so this path — common when a planner drops elevation partway —
/// was untested. Here points 1–2 climb 200→250 m and point 3 omits `<ele>`: it must inherit
/// 250 m (not reset to 0), so the stored geometry and the ascent stat stay sane.
#[test]
fn missing_elevation_carries_last_known() {
    let dlat = 5.0 / 111_320.0; // zigzag north so all three vertices survive decimation
    let bytes = convert(
        "Partial Ele",
        &gpx(&[(48.0, 7.800, Some(200.0)), (48.0 + dlat, 7.801, Some(250.0)), (48.0, 7.802, None)]),
    );
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!(r.point_count, 3, "the zigzag keeps all three vertices");
    assert_eq!((r.min_ele_m, r.max_ele_m), (200, 250), "min/max ignore the carried (not measured) point");
    assert_eq!(r.total_ascent_m, 50, "the 200→250 climb; the carried point adds no further ascent");
    let pts = decode(&r, 0);
    assert_eq!(pts[2].ele, 250, "the <ele>-less third point carries the last known 250 m, not 0");
}

/// Item 10 (convert: no elevation at all) — **a route with no `<ele>` anywhere** (a bare
/// planner GPX). The carry starts at 0 and never updates, and `min_ele > max_ele` after the
/// sweep, so the converter falls back to a 0..0 range. Distance/geometry are still computed
/// from the positions; only elevation is flat zero. Pins that a no-elevation route converts
/// cleanly rather than producing garbage ele extremes.
#[test]
fn no_elevation_anywhere_yields_zero_range() {
    let bytes = convert("No Ele", &gpx(&[(48.0, 7.80, None), (48.005, 7.80, None), (48.01, 7.80, None)]));
    let src = SliceSource(&bytes);
    let ridx = RouteIndex::read(&src).unwrap();
    let r = RouteReader::new(&ridx, &src);

    assert_eq!((r.min_ele_m, r.max_ele_m), (0, 0), "no <ele> → the converter's 0..0 fallback");
    assert_eq!((r.total_ascent_m, r.total_descent_m), (0, 0));
    assert!(r.total_distance_m > 1000, "distance is still measured from positions, got {}", r.total_distance_m);
    let pts = decode(&r, 0);
    assert!(pts.iter().all(|p| p.ele == 0), "every stored elevation is 0");
}
