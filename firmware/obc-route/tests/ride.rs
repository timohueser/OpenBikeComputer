//! Ride object v1 tests (issue #275): the Finish-time `.obct` → ride-object conversion
//! (coordinate/timestamp translation, the wall-clock back-dating, the held-back version
//! commit point) and the header reader's validation (the torn-write / length rules a BLE
//! `rideList` build leans on).

use obc_route::{
    encode_record, ride_object_len, track_to_ride, RideInfo, RideStats, SliceSource, TrackPoint, RIDE_HEADER_LEN,
    RIDE_POINT_LEN, RIDE_VERSION,
};

mod common;
use common::VecSink;

const STATS: RideStats = RideStats {
    distance_m: 42_500,
    moving_time_s: 9_000,
    avg_speed_cms: 472,
    climb_m: 810,
    unix_at_anchor: 1_751_450_000,
    anchor_ms: 0,
};

/// Build a flat `.obct` log (concatenated records) from points.
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
    let pts = [
        TrackPoint { lon: 7_842_000, lat: 47_995_000, ele: 300, t_ms: 100_000, segment_start: true },
        TrackPoint { lon: -7_843_500, lat: -47_996_000, ele: -42, t_ms: 161_500, segment_start: false },
    ];
    let stats = RideStats { unix_at_anchor: 1_751_450_000, anchor_ms: 400_000, ..STATS };
    let ride = to_ride(&log_of(&pts), "Höhenweg", &stats);

    let name_len = "Höhenweg".len(); // 9 bytes UTF-8
    assert_eq!(ride.len() as u32, ride_object_len(name_len, 2));

    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.name.as_str(), "Höhenweg");
    // start_time = anchor unix − (anchor_ms − first t_ms)/1000 = 1_751_450_000 − 300.
    assert_eq!(info.start_time, 1_751_449_700);
    assert_eq!(
        (info.distance_m, info.moving_time_s, info.avg_speed_cms, info.climb_m, info.point_count),
        (42_500, 9_000, 472, 810, 2)
    );

    // Point records: t_offset relative to the first record, µ° × 10, lat-then-lon order.
    let p = &ride[RIDE_HEADER_LEN + name_len..];
    let point = |k: usize| &p[k * RIDE_POINT_LEN..(k + 1) * RIDE_POINT_LEN];
    let (p0, p1) = (point(0), point(1));
    assert_eq!(u32::from_le_bytes(p0[0..4].try_into().unwrap()), 0);
    assert_eq!(i32::from_le_bytes(p0[4..8].try_into().unwrap()), 479_950_000, "lat first, ×10");
    assert_eq!(i32::from_le_bytes(p0[8..12].try_into().unwrap()), 78_420_000, "lon second, ×10");
    assert_eq!(i16::from_le_bytes(p0[12..14].try_into().unwrap()), 300);
    assert_eq!(u32::from_le_bytes(p1[0..4].try_into().unwrap()), 61, "61.5 s truncates to whole seconds");
    assert_eq!(i32::from_le_bytes(p1[4..8].try_into().unwrap()), -479_960_000);
    assert_eq!(i32::from_le_bytes(p1[8..12].try_into().unwrap()), -78_435_000);
    assert_eq!(i16::from_le_bytes(p1[12..14].try_into().unwrap()), -42);
}

#[test]
fn empty_log_dates_itself_at_the_anchor() {
    let ride = to_ride(&[], "Leer", &STATS);
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.point_count, 0);
    assert_eq!(info.start_time, STATS.unix_at_anchor);
    assert_eq!(ride.len() as u32, ride_object_len(4, 0));
}

#[test]
fn trailing_partial_record_is_ignored() {
    // A power-loss mid-append leaves a partial trailing record — same rule as the GPX pass.
    let mut log = log_of(&[TrackPoint { lon: 1, lat: 2, ele: 3, t_ms: 0, segment_start: true }]);
    log.extend_from_slice(&[0xAB; 7]);
    let ride = to_ride(&log, "R", &STATS);
    assert_eq!(RideInfo::read(&SliceSource(&ride)).unwrap().point_count, 1);
}

#[test]
fn version_is_the_commit_point() {
    // The version byte is written 0 and patched to 1 last: a save that never reached the patch
    // (simulated by zeroing it) must be rejected, like an aborted route commit's held magic.
    let mut ride =
        to_ride(&log_of(&[TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: 0, segment_start: true }]), "R", &STATS);
    assert_eq!(ride[0], RIDE_VERSION, "a completed save carries the real version");
    ride[0] = 0;
    assert!(RideInfo::read(&SliceSource(&ride)).is_err(), "a held-back version byte is invisible");
}

#[test]
fn reader_rejects_length_disagreement() {
    // Spec §7.2: the length is fully determined by the header — a torn tail must be rejected.
    let ride = to_ride(&log_of(&[TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: 0, segment_start: true }]), "R", &STATS);
    assert!(RideInfo::read(&SliceSource(&ride[..ride.len() - 1])).is_err(), "truncated");
    let mut long = ride.clone();
    long.push(0);
    assert!(RideInfo::read(&SliceSource(&long)).is_err(), "over-long");
}

#[test]
fn wrap_safe_offsets_across_the_millis_wrap() {
    // A ride recorded across the u32 millis wrap (~49.7 days of uptime) still yields small,
    // monotonic offsets — the same wrapping_sub discipline as the wall clock.
    let pts = [
        TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: u32::MAX - 5_000, segment_start: true },
        TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: u32::MAX.wrapping_add(5_001), segment_start: false },
    ];
    let stats = RideStats { unix_at_anchor: 2_000_000_000, anchor_ms: u32::MAX.wrapping_add(15_001), ..STATS };
    let ride = to_ride(&log_of(&pts), "W", &stats);
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    // The anchor sits 20 s after the first record.
    assert_eq!(info.start_time, 2_000_000_000 - 20);
    let p = &ride[RIDE_HEADER_LEN + 1..];
    assert_eq!(u32::from_le_bytes(p[RIDE_POINT_LEN..RIDE_POINT_LEN + 4].try_into().unwrap()), 10);
}

#[test]
fn over_long_name_is_truncated_on_a_char_boundary() {
    // 1 + 2×30 = 61 UTF-8 bytes; the 48-byte cap falls mid-"ü" (1 + 2×23 = 47, next ends at 49),
    // so the truncation must step back to the 47-byte char boundary, not split the code point.
    let name = format!("a{}", "ü".repeat(30));
    let ride = to_ride(&[], &name, &STATS);
    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.name.as_str(), format!("a{}", "ü".repeat(23)));
    assert_eq!(ride.len() as u32, ride_object_len(47, 0));
}
