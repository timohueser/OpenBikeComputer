//! Trip-object codec: round-trip, the committed `specs/vectors/trip-v3.bin` pin, and the read
//! guards.

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_route::{
    read_trip_day, trip_object_len, write_trip, TripDay, TripMeta, TripSummary, MAX_TRIP_DAYS, TRIP_HEADER_LEN,
    TRIP_VERSION,
};

/// A `ByteSink` over a `Vec`: the host's whole-object staging buffer.
#[derive(Default)]
struct VecSink(Vec<u8>);
impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.0.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.0[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

fn encode(key: u64, name: &str, start_date: u16, days: &[TripDay]) -> Vec<u8> {
    let mut sink = VecSink::default();
    write_trip(key, name, start_date, days, &mut sink).unwrap();
    sink.0
}

fn whole(routes: &[u64]) -> Vec<TripDay> {
    routes.iter().copied().map(TripDay::whole).collect()
}

#[test]
fn round_trip() {
    let days = [
        TripDay { route: 7, join_m: 0, leave_m: 82_000 },
        TripDay { route: 8, join_m: 350, leave_m: 73_600 },
        TripDay::whole(99),
    ];
    let bytes = encode(42, "Alpen Traverse", 20_360, &days);
    assert_eq!(bytes.len() as u64, trip_object_len(days.len() as u16));
    assert_eq!(bytes[0], TRIP_VERSION);

    let src = SliceSource(&bytes);
    let meta = TripMeta::read(&src).unwrap();
    assert_eq!((meta.key, meta.name.as_str(), meta.start_date), (42, "Alpen Traverse", 20_360));
    assert_eq!(meta.day_routes.as_slice(), &[7, 8, 99]);
    assert!(!meta.truncated);
    for (k, day) in days.iter().enumerate() {
        assert_eq!(read_trip_day(&src, k as u16), Ok(*day));
    }
    assert_eq!(read_trip_day(&src, 3), Err(Error::BadOffset));

    let summary = TripSummary::read(&src).unwrap();
    assert_eq!((summary.key, summary.start_date, summary.day_count), (42, 20_360, 3));
}

/// An empty trip is a valid 64-byte header. The codec is policy-free, even though the app
/// dissolves a trip that loses its last day.
#[test]
fn empty_trip_is_header_only() {
    let bytes = encode(1, "Loose", 0, &[]);
    assert_eq!(bytes.len(), TRIP_HEADER_LEN);
    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert!(meta.day_routes.is_empty());
    assert!(!meta.truncated);
    assert_eq!(TripSummary::read(&SliceSource(&bytes)).unwrap().day_count, 0);
}

/// The committed vector: "Alpen Traverse", three days that pin every day field. The full-width
/// third route id is a dangling ref the codec carries verbatim; validation is the app's job.
#[test]
fn pins_the_committed_trip_v3_vector() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/trip-v3.bin");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {} not reachable", path.display());
        return;
    };
    let days = [
        TripDay { route: 7, join_m: 0, leave_m: 82_000 },
        TripDay { route: 8, join_m: 0, leave_m: 73_600 },
        TripDay { route: 0x1_0000_0063, join_m: 400, leave_m: u32::MAX },
    ];
    let src = SliceSource(&bytes);
    let meta = TripMeta::read(&src).unwrap();
    assert_eq!(meta.key, 0x0123_4567_89AB_CDEF);
    assert_eq!(meta.name, "Alpen Traverse");
    assert_eq!(meta.start_date, 20_360);
    assert_eq!(meta.day_routes.as_slice(), &[7u64, 8, 0x1_0000_0063]);
    for (k, day) in days.iter().enumerate() {
        assert_eq!(read_trip_day(&src, k as u16), Ok(*day));
    }

    // The production writer reproduces the fixture exactly.
    assert_eq!(encode(0x0123_4567_89AB_CDEF, "Alpen Traverse", 20_360, &days), bytes);
}

#[test]
fn rejects_wrong_version() {
    let mut bytes = encode(1, "X", 0, &whole(&[1, 2]));
    bytes[0] = 2;
    assert_eq!(TripMeta::read(&SliceSource(&bytes)), Err(Error::BadVersion));
    assert_eq!(TripSummary::read(&SliceSource(&bytes)), Err(Error::BadVersion));
    assert_eq!(read_trip_day(&SliceSource(&bytes), 0), Err(Error::BadVersion));
}

/// A file shorter than `64 + 16·day_count` (a torn write) is rejected on the length check.
#[test]
fn rejects_length_mismatch() {
    let bytes = encode(1, "X", 0, &whole(&[1, 2, 3]));
    let short = &bytes[..bytes.len() - 1];
    assert_eq!(TripMeta::read(&SliceSource(short)), Err(Error::BadOffset));
    assert_eq!(read_trip_day(&SliceSource(short), 0), Err(Error::BadOffset));
    // A stray trailing byte (over-long) is rejected the same way.
    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(TripSummary::read(&SliceSource(&long)), Err(Error::BadOffset));
}

/// A trip with more days than the resident cap windows to the first `MAX_TRIP_DAYS` on read,
/// with `truncated = true`. The summary keeps the true stored count.
#[test]
fn windows_a_trip_past_the_day_cap() {
    let over = MAX_TRIP_DAYS + 5;
    let routes: Vec<u64> = (0..over as u64).collect();
    let bytes = encode(1, "Long", 0, &whole(&routes));

    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert_eq!(meta.day_routes.as_slice(), &routes[..MAX_TRIP_DAYS]);
    assert!(meta.truncated);
    assert_eq!(TripSummary::read(&SliceSource(&bytes)).unwrap().day_count, over as u16);
}

/// A name longer than the 48-byte cap is truncated on a char boundary by the writer.
#[test]
fn truncates_a_long_name() {
    let long = "ä".repeat(40); // 80 bytes — over the 48-byte cap
    let bytes = encode(1, &long, 0, &whole(&[1]));
    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert!(long.starts_with(meta.name.as_str()));
    // The cut landed on a char boundary: 24 two-byte 'ä's = 48 bytes.
    assert_eq!(meta.name.chars().count(), 24);
}
