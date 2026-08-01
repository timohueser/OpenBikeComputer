//! Ride object tests: the Finish-time `.obct` → ride-object v2 conversion (coordinate/timestamp
//! translation, the wall-clock back-dating, the sensor summary + per-point sensor fields, the
//! held-back version commit point) and the header reader's validation across **v1 and v2** (the
//! torn-write / length rules a BLE `rideList` build leans on, and the old-ride compatibility rule).

use obc_formats::io::{Error, SliceSource};
use obc_formats::ride::{
    object_len as ride_object_len, CAD_NONE as RIDE_CAD_NONE, ELE_NONE as RIDE_ELE_NONE,
    HEADER_LEN_V1 as RIDE_HEADER_LEN_V1, HEADER_LEN_V2 as RIDE_HEADER_LEN_V2, HR_NONE as RIDE_HR_NONE,
    POINT_LEN_V1 as RIDE_POINT_LEN_V1, POINT_LEN_V2 as RIDE_POINT_LEN_V2, PWR_NONE as RIDE_PWR_NONE,
    VERSION as RIDE_VERSION,
};
use obc_formats::track::encode_record;
use obc_ports::TrackPoint;
use obc_route::{track_to_ride, RideInfo, RideStats};

mod common;
use common::VecSink;

const STATS: RideStats = RideStats {
    distance_m: 42_500,
    moving_time_s: 9_000,
    avg_speed_cms: 472,
    climb_m: 810,
    unix_at_anchor: 1_751_450_000,
    anchor_ms: 0,
    avg_hr: None,
    max_hr: None,
    avg_cadence: None,
    avg_power: None,
    max_power: None,
};

/// A `TrackPoint` with no sensor values.
fn pt(lon: i32, lat: i32, ele: i16, t_ms: u32, segment_start: bool) -> TrackPoint {
    TrackPoint { lon, lat, ele, t_ms, segment_start, hr: None, cadence: None, power: None }
}

/// Build a flat `.obct` log (concatenated 20-byte records) from points.
fn log_of(pts: &[TrackPoint]) -> Vec<u8> {
    let mut v = Vec::new();
    for p in pts {
        v.extend_from_slice(&encode_record(p));
    }
    v
}

fn to_ride(log: &[u8], name: &str, stats: &RideStats) -> Vec<u8> {
    let mut sink = VecSink::default();
    track_to_ride(&SliceSource(log), name, stats, &mut sink).unwrap();
    sink.buf
}

#[test]
fn converts_coordinates_timestamps_and_stats() {
    // Recorded 100 s into the boot; Finish (the anchor) at t = 400 s.
    let pts = [pt(7_842_000, 47_995_000, 300, 100_000, true), pt(-7_843_500, -47_996_000, -42, 161_500, false)];
    let stats = RideStats { unix_at_anchor: 1_751_450_000, anchor_ms: 400_000, ..STATS };
    let ride = to_ride(&log_of(&pts), "Höhenweg", &stats);

    let name_len = "Höhenweg".len(); // 9 bytes UTF-8
    assert_eq!(ride[0], RIDE_VERSION, "the writer emits v2");
    assert_eq!(ride.len() as u32, ride_object_len(2, name_len, 2));

    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.version, 2);
    assert_eq!(info.name.as_str(), "Höhenweg");
    // start_time = anchor unix − (anchor_ms − first t_ms)/1000 = 1_751_450_000 − 300.
    assert_eq!(info.start_time, 1_751_449_700);
    assert_eq!(
        (info.distance_m, info.moving_time_s, info.avg_speed_cms, info.climb_m, info.point_count),
        (42_500, 9_000, 472, 810, 2)
    );
    // A ride with no sensor samples decodes the summary as all-absent.
    assert_eq!(
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power),
        (None, None, None, None, None)
    );

    // Point records: t_offset relative to the first record, µ° × 10, lat-then-lon order, then the
    // sentinel sensor tail (no sensors on these points).
    let p = &ride[RIDE_HEADER_LEN_V2 + name_len..];
    let point = |k: usize| &p[k * RIDE_POINT_LEN_V2..(k + 1) * RIDE_POINT_LEN_V2];
    let (p0, p1) = (point(0), point(1));
    assert_eq!(u32::from_le_bytes(p0[0..4].try_into().unwrap()), 0);
    assert_eq!(i32::from_le_bytes(p0[4..8].try_into().unwrap()), 479_950_000, "lat first, ×10");
    assert_eq!(i32::from_le_bytes(p0[8..12].try_into().unwrap()), 78_420_000, "lon second, ×10");
    assert_eq!(i16::from_le_bytes(p0[12..14].try_into().unwrap()), 300);
    assert_eq!((p0[14], p0[15]), (RIDE_HR_NONE, RIDE_CAD_NONE), "no hr/cad → sentinels");
    assert_eq!(u16::from_le_bytes(p0[16..18].try_into().unwrap()), RIDE_PWR_NONE, "no pwr → sentinel");
    assert_eq!(u32::from_le_bytes(p1[0..4].try_into().unwrap()), 61, "61.5 s truncates to whole seconds");
    assert_eq!(i32::from_le_bytes(p1[4..8].try_into().unwrap()), -479_960_000);
    assert_eq!(i32::from_le_bytes(p1[8..12].try_into().unwrap()), -78_435_000);
    assert_eq!(i16::from_le_bytes(p1[12..14].try_into().unwrap()), -42);
}

/// The v2 sensor summary heads the header (sentinels for absent quantities) and the per-point
/// sensor fields carry 1:1 from the track records — a streaming conversion with a mix of present
/// and absent fields across the header and the individual points.
#[test]
fn v2_streams_sensor_summary_and_per_point_fields() {
    let pts = [
        // all present
        TrackPoint {
            lon: 0,
            lat: 0,
            ele: 100,
            t_ms: 0,
            segment_start: true,
            hr: Some(140),
            cadence: Some(84),
            power: Some(205),
        },
        // all absent (a dropped fix)
        pt(0, 10_000, 110, 60_000, false),
        // partial: hr + power, cadence absent
        TrackPoint {
            lon: 0,
            lat: 20_000,
            ele: 120,
            t_ms: 120_000,
            segment_start: false,
            hr: Some(150),
            cadence: None,
            power: Some(215),
        },
    ];
    let stats = RideStats {
        avg_hr: Some(142),
        max_hr: Some(176),
        avg_cadence: Some(85),
        avg_power: Some(210),
        max_power: Some(480),
        ..STATS
    };
    let ride = to_ride(&log_of(&pts), "Sensors", &stats);

    // Header summary decodes back exactly.
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.avg_hr, Some(142));
    assert_eq!(info.max_hr, Some(176));
    assert_eq!(info.avg_cadence, Some(85));
    assert_eq!(info.avg_power, Some(210));
    assert_eq!(info.max_power, Some(480));
    assert_eq!(info.point_count, 3);

    // The reserved pad byte after avg_cad is 0.
    let f = 3 + "Sensors".len();
    assert_eq!(ride[f + 23], 0, "reserved pad byte is 0");

    // Per-point sensor tails at offsets 14 (hr), 15 (cad), 16..18 (pwr).
    let base = RIDE_HEADER_LEN_V2 + "Sensors".len();
    let point = |k: usize| &ride[base + k * RIDE_POINT_LEN_V2..base + (k + 1) * RIDE_POINT_LEN_V2];
    let sensors = |p: &[u8]| (p[14], p[15], u16::from_le_bytes([p[16], p[17]]));
    assert_eq!(sensors(point(0)), (140, 84, 205), "point 0 all present");
    assert_eq!(sensors(point(1)), (RIDE_HR_NONE, RIDE_CAD_NONE, RIDE_PWR_NONE), "point 1 all absent");
    assert_eq!(sensors(point(2)), (150, RIDE_CAD_NONE, 215), "point 2 hr+pwr, cad absent");
}

#[test]
fn empty_log_dates_itself_at_the_anchor() {
    let ride = to_ride(&[], "Leer", &STATS);
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.point_count, 0);
    assert_eq!(info.start_time, STATS.unix_at_anchor);
    assert_eq!(ride.len() as u32, ride_object_len(2, 4, 0));
}

#[test]
fn trailing_partial_record_is_ignored() {
    // A power-loss mid-append leaves a partial trailing record — same rule as the GPX pass.
    let mut log = log_of(&[pt(1, 2, 3, 0, true)]);
    log.extend_from_slice(&[0xAB; 7]);
    let ride = to_ride(&log, "R", &STATS);
    assert_eq!(RideInfo::read(&SliceSource(&ride)).unwrap().point_count, 1);
}

#[test]
fn version_is_the_commit_point() {
    // The version byte is written 0 and patched to RIDE_VERSION last: a save that never reached the
    // patch (simulated by zeroing it) must be rejected, like an aborted route commit's held magic.
    let mut ride = to_ride(&log_of(&[pt(0, 0, 0, 0, true)]), "R", &STATS);
    assert_eq!(ride[0], RIDE_VERSION, "a completed save carries the real version");
    ride[0] = 0;
    assert!(RideInfo::read(&SliceSource(&ride)).is_err(), "a held-back version byte is invisible");
}

#[test]
fn reader_rejects_length_disagreement() {
    // Spec §7.2: the length is fully determined by the header — a torn tail must be rejected.
    let ride = to_ride(&log_of(&[pt(0, 0, 0, 0, true)]), "R", &STATS);
    assert!(RideInfo::read(&SliceSource(&ride[..ride.len() - 1])).is_err(), "truncated");
    let mut long = ride.clone();
    long.push(0);
    assert!(RideInfo::read(&SliceSource(&long)).is_err(), "over-long");
}

#[test]
fn reader_rejects_wrapped_point_count_length() {
    // v2 stride 18 × 0x8000_0000 wraps to zero under unchecked u32 arithmetic, which used to make
    // this 31-byte header-only object appear length-correct.
    let mut ride = [0u8; RIDE_HEADER_LEN_V2];
    ride[0] = 2;
    ride[19..23].copy_from_slice(&0x8000_0000u32.to_le_bytes());
    assert!(matches!(RideInfo::read(&SliceSource(&ride)), Err(Error::BadOffset)));
}

/// The length check is **per version**: a v1 header claiming a point count whose v1 length (14 B
/// points) disagrees with the file is rejected, and so is a v2 one (18 B points). Guards against a
/// decoder applying the wrong version's stride.
#[test]
fn reader_rejects_length_per_version() {
    // A well-formed v2 object with one point is exactly 31 + 1 + 18. If the same bytes were read as
    // if a point were 14 B (v1 stride) the length wouldn't match — the version-keyed check catches
    // both directions. Build a valid v1 and a valid v2 of the same one-point ride and cross-check.
    let v2 = to_ride(&log_of(&[pt(5, 6, 7, 0, true)]), "R", &STATS);
    assert_eq!(v2.len() as u32, ride_object_len(2, 1, 1));
    assert_ne!(v2.len() as u32, ride_object_len(1, 1, 1), "v1 and v2 lengths differ for the same ride");

    let v1 = build_v1("R", &[(0, 60, 70, 80)]);
    assert_eq!(v1.len() as u32, ride_object_len(1, 1, 1));
    // Corrupt the v1 point_count so the v1-length check fails.
    let mut torn = v1.clone();
    let pc_off = 3 + 1 + 16; // version + name_len + name + (start..avg_speed..climb) → point_count
    torn[pc_off..pc_off + 4].copy_from_slice(&2u32.to_le_bytes());
    assert!(RideInfo::read(&SliceSource(&torn)).is_err(), "v1 length must match its own stride");
}

/// A **v1** object (23-byte header, 14-byte points, no sensor bytes) still reads — old rides on the
/// card list, download and delete — decoding with every sensor field absent.
#[test]
fn reader_accepts_v1_with_all_sensors_absent() {
    let v1 = build_v1("Altfahrt", &[(0, 480_000_000, 78_000_000, 214), (60, 480_010_000, 78_012_000, 219)]);
    let info = RideInfo::read(&SliceSource(&v1)).unwrap();
    assert_eq!(info.version, 1);
    assert_eq!(info.name.as_str(), "Altfahrt");
    assert_eq!(info.point_count, 2);
    assert_eq!(
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power),
        (None, None, None, None, None),
        "a v1 object has no sensor data"
    );
    assert_eq!(info.distance_m, 42_500);
}

#[test]
fn wrap_safe_offsets_across_the_millis_wrap() {
    // A ride recorded across the u32 millis wrap (~49.7 days of uptime) still yields small,
    // monotonic offsets — the same wrapping_sub discipline as the wall clock.
    let pts = [pt(0, 0, 0, u32::MAX - 5_000, true), pt(0, 0, 0, u32::MAX.wrapping_add(5_001), false)];
    let stats = RideStats { unix_at_anchor: 2_000_000_000, anchor_ms: u32::MAX.wrapping_add(15_001), ..STATS };
    let ride = to_ride(&log_of(&pts), "W", &stats);
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    // The anchor sits 20 s after the first record.
    assert_eq!(info.start_time, 2_000_000_000 - 20);
    let p = &ride[RIDE_HEADER_LEN_V2 + 1..];
    assert_eq!(u32::from_le_bytes(p[RIDE_POINT_LEN_V2..RIDE_POINT_LEN_V2 + 4].try_into().unwrap()), 10);
}

#[test]
fn over_long_name_is_truncated_on_a_char_boundary() {
    // 1 + 2×30 = 61 UTF-8 bytes; the 48-byte cap falls mid-"ü" (1 + 2×23 = 47, next ends at 49),
    // so the truncation must step back to the 47-byte char boundary, not split the code point.
    let name = format!("a{}", "ü".repeat(30));
    let ride = to_ride(&[], &name, &STATS);
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.name.as_str(), format!("a{}", "ü".repeat(23)));
    assert_eq!(ride.len() as u32, ride_object_len(2, 47, 0));
}

/// The Ride detail's band source (epic #678 T2 / #680): `ride_elevation_profile` streams a stored
/// ride object once and yields the same `Profile` shape the route band uses — y-range from the
/// sweep, the peak where the track put it, elevation gaps carried, and the ascent curve pinned to
/// the header's climb total. Runs over a **v2** object (the current writer's output).
#[test]
fn ride_elevation_profile_reads_the_recorded_track() {
    use obc_route::ride_elevation_profile;
    let pts = [pt(0, 0, 100, 0, true), pt(0, 10_000, 300, 60_000, false), pt(0, 20_000, 200, 120_000, false)];
    let stats = RideStats { distance_m: 2_224, climb_m: 200, ..STATS };
    let ride = to_ride(&log_of(&pts), "Bergtour", &stats);

    let p = ride_elevation_profile(&SliceSource(&ride)).unwrap();
    assert_eq!((p.min_ele_m, p.max_ele_m), (100, 300), "y-range from the sweep (the header stores none)");
    assert_eq!(p.peak_ele_m(), 300);
    let frac = p.peak_frac();
    assert!((0.4..=0.6).contains(&frac), "the peak sits mid-track, got {frac}");
    assert_eq!(p.at(0.0).0, 100, "the start column holds the first sample");
    assert_eq!(p.at(1.0).1, 200, "the end column holds the last sample");
    assert_eq!(p.ascent_to(1.0), 200);
    assert_eq!(p.ascent_to(0.0), 0);
}

/// The same band source reads a **v1** object correctly — an old ride's detail screen still draws
/// (the reader keys the point stride + offset off the header version).
#[test]
fn ride_elevation_profile_reads_a_v1_object() {
    use obc_route::ride_elevation_profile;
    // A v1 ride with the same three-point shape as the v2 test above; header climb/distance set to
    // match so the columns bucket across the whole band.
    let v1 =
        build_v1_stats("Alt", &[(0, 0, 0, 100), (60, 100_000_000, 0, 300), (120, 200_000_000, 0, 200)], 2_224, 200);
    let p = ride_elevation_profile(&SliceSource(&v1)).unwrap();
    assert_eq!((p.min_ele_m, p.max_ele_m), (100, 300), "v1 points read with the same lat/lon/ele fields");
    assert_eq!(p.peak_ele_m(), 300);
    assert_eq!(p.ascent_to(1.0), 200);
}

/// A `RIDE_ELE_NONE` point contributes distance but neither the y-range nor a column sample; a
/// held-back (torn) version byte is rejected exactly as `RideInfo::read` rejects it.
#[test]
fn ride_elevation_profile_skips_ele_none_and_rejects_torn_saves() {
    use obc_route::ride_elevation_profile;
    let pts = [pt(0, 0, 150, 0, true), pt(0, 10_000, 0, 60_000, false), pt(0, 20_000, 250, 120_000, false)];
    let stats = RideStats { distance_m: 2_224, climb_m: 100, ..STATS };
    let mut ride = to_ride(&log_of(&pts), "R", &stats);
    // Patch the middle point's elevation to the "no elevation" sentinel (offset within its record:
    // t_offset u32 + lat i32 + lon i32 = 12).
    let mid = RIDE_HEADER_LEN_V2 + 1 + RIDE_POINT_LEN_V2 + 12;
    ride[mid..mid + 2].copy_from_slice(&RIDE_ELE_NONE.to_le_bytes());

    let p = ride_elevation_profile(&SliceSource(&ride)).unwrap();
    assert_eq!((p.min_ele_m, p.max_ele_m), (150, 250), "the sentinel point is no sample");
    let (lo, hi) = p.at(0.5);
    assert!((150..=250).contains(&lo) && (150..=250).contains(&hi), "gap-filled mid col, got {lo}/{hi}");

    ride[0] = 0; // a torn save's held-back version byte
    assert!(ride_elevation_profile(&SliceSource(&ride)).is_err(), "a torn save is rejected");
}

/// The Ride detail's track-shape source (#678 rework 3): `ride_preview_polyline` streams a stored
/// ride once and yields the decimated `(lon, lat)` µdeg polyline — exact endpoints, uniform by
/// point index between them, capped at `N`.
#[test]
fn ride_preview_polyline_decimates_with_exact_endpoints() {
    use obc_route::ride_preview_polyline;
    // 100 points marching east along the equator, 10 µ° apart (recorded µdeg → stored ×10).
    let pts: Vec<TrackPoint> = (0..100).map(|i| pt(i * 10, 42, 100, i as u32 * 1_000, i == 0)).collect();
    let ride = to_ride(&log_of(&pts), "Shape", &STATS);

    let p = ride_preview_polyline::<8>(&SliceSource(&ride)).unwrap();
    assert_eq!(p.len(), 8);
    assert_eq!(p[0], (0, 42), "the first point is exact (and back in microdegrees)");
    assert_eq!(p[7], (990, 42), "the last point is exact");
    for w in p.windows(2) {
        let step = w[1].0 - w[0].0;
        assert!((130..=150).contains(&step), "≈ even stride over 99 segments, got {step}");
    }

    let all = ride_preview_polyline::<128>(&SliceSource(&ride)).unwrap();
    assert_eq!(all.len(), 100);
    assert_eq!(all[41], (410, 42));

    let mut torn = ride;
    torn[0] = 0;
    assert!(ride_preview_polyline::<8>(&SliceSource(&torn)).is_err());
}

// --- v1 fixtures: hand-build the legacy 23-byte-header / 14-byte-point object the way older
// firmware wrote it, so the v1-acceptance + per-version-length tests exercise real v1 bytes.

/// A v1 ride object with the given name and `(t_offset, lat_1e7, lon_1e7, ele)` points, using the
/// `STATS` header totals.
fn build_v1(name: &str, points: &[(u32, i32, i32, i16)]) -> Vec<u8> {
    build_v1_stats(name, points, STATS.distance_m, STATS.climb_m)
}

fn build_v1_stats(name: &str, points: &[(u32, i32, i32, i16)], distance_m: u32, climb_m: u16) -> Vec<u8> {
    let name = name.as_bytes();
    let mut v = Vec::new();
    v.push(1); // version 1
    v.extend_from_slice(&(name.len() as u16).to_le_bytes());
    v.extend_from_slice(name);
    v.extend_from_slice(&STATS.unix_at_anchor.to_le_bytes()); // start_time
    v.extend_from_slice(&distance_m.to_le_bytes());
    v.extend_from_slice(&STATS.moving_time_s.to_le_bytes());
    v.extend_from_slice(&STATS.avg_speed_cms.to_le_bytes());
    v.extend_from_slice(&climb_m.to_le_bytes());
    v.extend_from_slice(&(points.len() as u32).to_le_bytes());
    for &(t, lat, lon, ele) in points {
        v.extend_from_slice(&t.to_le_bytes());
        v.extend_from_slice(&lat.to_le_bytes());
        v.extend_from_slice(&lon.to_le_bytes());
        v.extend_from_slice(&ele.to_le_bytes());
    }
    // Sanity: the hand-built object matches the v1 length formula and the v1 header size.
    assert_eq!(v.len() as u32, ride_object_len(1, name.len(), points.len() as u32));
    assert_eq!(RIDE_HEADER_LEN_V1 + name.len() + points.len() * RIDE_POINT_LEN_V1, v.len());
    v
}
