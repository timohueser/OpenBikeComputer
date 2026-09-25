//! Recorded ride v6 contract: verbatim samples, one fixed footer, and footer-based readers.

use core::cell::RefCell;

use obc_formats::{
    bike::BikeType,
    io::{ByteSource, Error, SliceSource},
    ride::{decode_footer, EffortLimits, Name, TripRef, FOOTER_LEN, VERSION},
    track::encode_record,
};
use obc_ports::TrackPoint;
use obc_route::{encode_summary_footer, ride_track_into, Profile, RideInfo, RideStats, RideTrackFacts};

const STATS: RideStats = RideStats {
    distance_m: 2_224,
    moving_time_s: 9_000,
    avg_speed_cms: 472,
    climb_m: 200,
    descent_m: 180,
    unix_at_anchor: 1_751_450_000,
    anchor_ms: 400_000,
    clock_trusted: true,
    avg_hr: Some(142),
    max_hr: Some(176),
    avg_cadence: Some(85),
    avg_power: Some(210),
    max_power: Some(480),
    energy_kj: Some(756),
    bike: BikeType::Mtb,
    limits: EffortLimits { max_hr: 185, ftp_w: 250 },
    trip: TripRef::new(7, 0, 2),
    trip_name: Name::EMPTY,
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
    let ride = ride_of(&points, "Höhenweg", &RideStats { trip_name: Name::new("Alpen"), ..STATS });
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
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power, info.energy_kj),
        (Some(142), Some(176), Some(85), Some(210), Some(480), Some(756))
    );
    assert_eq!((info.bike, info.trip, info.trip_name.as_str()), (BikeType::Mtb, STATS.trip, "Alpen"));
    let footer = decode_footer(ride[ride.len() - FOOTER_LEN..].try_into().unwrap()).unwrap();
    assert_eq!(footer.limits, STATS.limits);
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
fn exact_length_and_v5_are_mandatory() {
    let ride = ride_of(&[pt(1, 2, 3, 4, true)], "R", &STATS);
    assert!(RideInfo::read(&SliceSource(&ride[..ride.len() - 1])).is_err());
    let mut long = ride.clone();
    long.push(0);
    assert!(RideInfo::read(&SliceSource(&long)).is_err());

    let mut old = ride;
    let version = old.len() - FOOTER_LEN + 4;
    old[version] = 4;
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
fn profile_and_preview_stream_the_samples() {
    let points = [pt(0, 0, 100, 0, true), pt(0, 10_000, 300, 60_000, false), pt(0, 20_000, 200, 120_000, false)];
    let ride = ride_of(&points, "Bergtour", &STATS);

    let (mut profile, mut facts) = (Profile::EMPTY, RideTrackFacts::EMPTY);
    let mut preview = heapless::Vec::<_, 3>::new();
    ride_track_into(&SliceSource(&ride), &mut profile, &mut facts, &mut preview).unwrap();
    assert_eq!((profile.min_ele_m, profile.max_ele_m), (100, 300));
    assert_eq!(profile.peak_ele_m(), 300);
    assert_eq!(profile.ascent_to(1.0), 200);

    assert_eq!(preview.as_slice(), &[(0, 0), (0, 10_000), (0, 20_000)]);
}

#[test]
fn preview_keeps_exact_endpoints_when_decimating() {
    let points: Vec<_> = (0..100).map(|i| pt(i * 10, 42, 100, i as u32 * 1_000, i == 0)).collect();
    let ride = ride_of(&points, "Shape", &STATS);
    let spy = ReadSpy { bytes: &ride, reads: RefCell::new(Vec::new()) };
    let (mut profile, mut facts) = (Profile::EMPTY, RideTrackFacts::EMPTY);
    let mut preview = heapless::Vec::<_, 8>::new();
    ride_track_into(&spy, &mut profile, &mut facts, &mut preview).unwrap();
    assert_eq!(&*spy.reads.borrow(), &[(2_000, FOOTER_LEN), (0, 640), (640, 640), (1_280, 640), (1_920, 80)]);
    assert_eq!((profile.min_ele_m, profile.max_ele_m), (100, 100));
    assert_eq!(preview.len(), 8);
    assert_eq!(preview[0], (0, 42));
    assert_eq!(preview[7], (990, 42));
}

#[test]
fn track_fill_handles_empty_rides_small_previews_and_read_failure() {
    let points: Vec<_> = (0..40).map(|i| pt(i, 42, 100, i as u32 * 1_000, i == 0)).collect();
    let ride = ride_of(&points, "Track", &STATS);
    let (mut profile, mut facts) = (Profile::EMPTY, RideTrackFacts::EMPTY);
    let mut no_preview = heapless::Vec::<_, 0>::new();
    ride_track_into(&SliceSource(&ride), &mut profile, &mut facts, &mut no_preview).unwrap();
    assert_eq!((profile.min_ele_m, profile.max_ele_m), (100, 100));
    let mut preview = heapless::Vec::<_, 1>::new();
    ride_track_into(&SliceSource(&ride), &mut profile, &mut facts, &mut preview).unwrap();
    assert_eq!(preview.as_slice(), &[(0, 42)]);

    struct FailSecondBlock<'a>(&'a [u8]);
    impl ByteSource for FailSecondBlock<'_> {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), Error> {
            if offset == 640 {
                return Err(Error::Io);
            }
            SliceSource(self.0).read_at(offset, out)
        }
    }
    assert_eq!(ride_track_into(&FailSecondBlock(&ride), &mut profile, &mut facts, &mut preview), Err(Error::Io));
    assert!(preview.is_empty(), "an error after the first preview point cannot return a partial track");

    let empty = ride_of(&[], "Empty", &STATS);
    ride_track_into(&SliceSource(&empty), &mut profile, &mut facts, &mut preview).unwrap();
    assert!(preview.is_empty());
    assert_eq!((profile.min_ele_m, profile.max_ele_m), (0, 0));
}

fn facts_of(ride: &[u8]) -> RideTrackFacts {
    let (mut profile, mut facts) = (Profile::EMPTY, RideTrackFacts::EMPTY);
    let mut preview = heapless::Vec::<_, 2>::new();
    ride_track_into(&SliceSource(ride), &mut profile, &mut facts, &mut preview).unwrap();
    facts
}

#[test]
fn the_track_fill_reads_the_descent_and_averages_each_bucket() {
    // Two samples a bucket. HR stops halfway, as when a strap drops out.
    let points: Vec<_> = (0..120)
        .map(|i| TrackPoint {
            hr: (i < 60).then_some(100 + i as u8),
            power: Some(2 * i as u16),
            ..pt(i, 42, 100, i as u32 * 1_000, i == 0)
        })
        .collect();
    let facts = facts_of(&ride_of(&points, "Sensors", &STATS));
    assert_eq!(facts.descent_m, 180, "the footer's descent");
    assert_eq!(facts.hr().len(), 60);
    assert!((0..30).all(|b| facts.hr()[b] == 100 + 2 * b as u8), "{:?}", facts.hr());
    assert!(facts.hr()[30..].iter().all(|&v| v == 0), "a bucket without a reading is empty");
    let power: Vec<u16> = facts.power().collect();
    assert!((0..60).all(|b| power[b] == (4 * b as u16 + 1).div_ceil(4) * 4), "{power:?}");
}

#[test]
fn a_short_ride_has_one_bucket_per_sample_and_an_empty_ride_none() {
    let points = [pt(0, 0, 100, 0, true), TrackPoint { hr: None, power: None, ..pt(0, 1, 100, 1_000, false) }];
    let facts = facts_of(&ride_of(&points, "Short", &STATS));
    assert_eq!(facts.hr(), [140, 0], "one bucket per sample");
    assert_eq!(facts.power().collect::<Vec<_>>(), [208, 0], "power rounds up to its 4 W step");

    let facts = facts_of(&ride_of(&[], "Empty", &STATS));
    assert!(facts.hr().is_empty() && facts.power().len() == 0);
    assert_eq!(facts.descent_m, 180);
}
