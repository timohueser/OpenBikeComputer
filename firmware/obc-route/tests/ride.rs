//! Recorded ride v3 contract: verbatim samples, one fixed footer, and footer-based readers.

use core::cell::RefCell;

use obc_formats::{
    io::{ByteSource, Error, SliceSource},
    ride::{FOOTER_LEN, VERSION},
    track::encode_record,
};
use obc_ports::TrackPoint;
use obc_route::{
    encode_summary_footer, ride_elevation_profile, ride_elevation_profile_into, ride_preview_polyline, Profile,
    RideInfo, RideStats,
};

const STATS: RideStats = RideStats {
    distance_m: 2_224,
    moving_time_s: 9_000,
    avg_speed_cms: 472,
    climb_m: 200,
    unix_at_anchor: 1_751_450_000,
    anchor_ms: 400_000,
    clock_trusted: true,
    avg_hr: Some(142),
    max_hr: Some(176),
    avg_cadence: Some(85),
    avg_power: Some(210),
    max_power: Some(480),
};

fn pt(lon: i32, lat: i32, ele: i16, t_ms: u32, segment_start: bool) -> TrackPoint {
    TrackPoint { lon, lat, ele, t_ms, segment_start, hr: Some(140), cadence: Some(84), power: Some(205) }
}

fn ride_of(points: &[TrackPoint], name: &str, stats: &RideStats) -> Vec<u8> {
    let mut bytes = Vec::new();
    for point in points {
        bytes.extend_from_slice(&encode_record(point));
    }
    bytes.extend_from_slice(&encode_summary_footer(name, stats, points.len() as u32, points.first().map(|p| p.t_ms)));
    bytes
}

#[test]
fn recorded_samples_are_the_served_bytes() {
    let points = [pt(7_842_000, 47_995_000, 300, 100_000, true), pt(-7_843_500, -47_996_000, -42, 161_500, false)];
    let ride = ride_of(&points, "Höhenweg", &STATS);
    let recorded: Vec<u8> = points.iter().flat_map(encode_record).collect();

    assert_eq!(&ride[..recorded.len()], recorded, "GET prefix is the exact recorded sample stream");
    assert_eq!(ride.len(), points.len() * 20 + FOOTER_LEN);

    let info = RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.version, VERSION);
    assert_eq!(info.name.as_str(), "Höhenweg");
    assert_eq!(info.start_time, 1_751_449_700);
    assert_eq!(
        (info.distance_m, info.moving_time_s, info.avg_speed_cms, info.climb_m, info.point_count),
        (2_224, 9_000, 472, 200, 2)
    );
    assert_eq!(
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power),
        (Some(142), Some(176), Some(85), Some(210), Some(480))
    );
}

struct ReadSpy<'a> {
    bytes: &'a [u8],
    reads: RefCell<Vec<(u64, usize)>>,
}

impl ByteSource for ReadSpy<'_> {
    fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
        self.reads.borrow_mut().push((offset, out.len()));
        SliceSource(self.bytes).read_at(offset, out)
    }

    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
}

#[test]
fn list_summary_is_one_footer_only_random_read() {
    let ride = ride_of(&[pt(1, 2, 3, 4, true)], "R", &STATS);
    let spy = ReadSpy { bytes: &ride, reads: RefCell::new(Vec::new()) };
    assert_eq!(RideInfo::read(&spy).unwrap().name.as_str(), "R");
    assert_eq!(&*spy.reads.borrow(), &[(20, FOOTER_LEN)]);
}

#[test]
fn exact_length_and_v3_are_mandatory() {
    let ride = ride_of(&[pt(1, 2, 3, 4, true)], "R", &STATS);
    assert!(RideInfo::read(&SliceSource(&ride[..ride.len() - 1])).is_err());
    let mut long = ride.clone();
    long.push(0);
    assert!(RideInfo::read(&SliceSource(&long)).is_err());

    let mut old = ride;
    let version = old.len() - FOOTER_LEN + 4;
    old[version] = 2;
    assert!(matches!(RideInfo::read(&SliceSource(&old)), Err(Error::BadVersion)));
}

#[test]
fn empty_ride_and_wrapped_clock_date_correctly() {
    let empty = ride_of(&[], "Leer", &STATS);
    assert_eq!(RideInfo::read(&SliceSource(&empty)).unwrap().start_time, STATS.unix_at_anchor);

    let p = pt(0, 0, 0, u32::MAX - 5_000, true);
    let stats = RideStats { unix_at_anchor: 2_000_000_000, anchor_ms: u32::MAX.wrapping_add(15_001), ..STATS };
    let wrapped = ride_of(&[p], "W", &stats);
    assert_eq!(RideInfo::read(&SliceSource(&wrapped)).unwrap().start_time, 2_000_000_000 - 20);
}

#[test]
fn an_untrusted_boot_never_dates_a_ride_from_stale_persisted_time() {
    let stats = RideStats { clock_trusted: false, ..STATS };
    let ride = ride_of(&[pt(0, 0, 0, 10_000, true)], "Untrusted", &stats);
    assert_eq!(RideInfo::read(&SliceSource(&ride)).unwrap().start_time, 0);
}

#[test]
fn profile_and_preview_stream_the_v3_samples() {
    let points = [pt(0, 0, 100, 0, true), pt(0, 10_000, 300, 60_000, false), pt(0, 20_000, 200, 120_000, false)];
    let ride = ride_of(&points, "Bergtour", &STATS);

    let profile = ride_elevation_profile(&SliceSource(&ride)).unwrap();
    assert_eq!((profile.min_ele_m, profile.max_ele_m), (100, 300));
    assert_eq!(profile.peak_ele_m(), 300);
    assert_eq!(profile.ascent_to(1.0), 200);

    let mut in_place = Profile::EMPTY;
    ride_elevation_profile_into(&SliceSource(&ride), &mut in_place).unwrap();
    assert_eq!(in_place.cols(), profile.cols());
    assert_eq!(in_place.ascent_to(1.0), profile.ascent_to(1.0));
    assert_eq!(in_place.peak_ele_m(), profile.peak_ele_m());

    let preview = ride_preview_polyline::<3>(&SliceSource(&ride)).unwrap();
    assert_eq!(preview.as_slice(), &[(0, 0), (0, 10_000), (0, 20_000)]);
}

#[test]
fn preview_keeps_exact_endpoints_when_decimating() {
    let points: Vec<_> = (0..100).map(|i| pt(i * 10, 42, 100, i as u32 * 1_000, i == 0)).collect();
    let ride = ride_of(&points, "Shape", &STATS);
    let preview = ride_preview_polyline::<8>(&SliceSource(&ride)).unwrap();
    assert_eq!(preview.len(), 8);
    assert_eq!(preview[0], (0, 42));
    assert_eq!(preview[7], (990, 42));
}
