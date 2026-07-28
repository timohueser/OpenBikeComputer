//! Trip-object codec (`obc-ble-interface-spec.md` §7.7): round-trip, the committed
//! `specs/vectors/trip-v1.bin` pin (dangling-ref fixture), and the read guards.

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_route::{trip_object_len, write_trip, TripMeta, TripSummary, MAX_TRIP_STAGES, TRIP_HEADER_LEN, TRIP_VERSION};

/// A `ByteSink` over a `Vec` — the host's whole-object staging buffer.
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

fn encode(name: &str, stages: &[u16]) -> Vec<u8> {
    let mut sink = VecSink::default();
    write_trip(name, stages, &mut sink).unwrap();
    sink.0
}

/// Writer → reader round-trip: the header and every stage id survive, in order.
#[test]
fn round_trip() {
    let stages = [7u16, 8, 99, 3, 5];
    let bytes = encode("Alpen Traverse", &stages);
    assert_eq!(bytes.len() as u32, trip_object_len(stages.len() as u16));
    assert_eq!(bytes[0], TRIP_VERSION);

    let src = SliceSource(&bytes);
    let meta = TripMeta::read(&src).unwrap();
    assert_eq!(meta.name, "Alpen Traverse");
    assert_eq!(meta.stage_ids.as_slice(), &stages);
    assert!(!meta.truncated);

    let summary = TripSummary::read(&src).unwrap();
    assert_eq!(summary.name, "Alpen Traverse");
    assert_eq!(summary.stage_count, stages.len() as u16);
}

/// An empty trip (no stages) is a valid 56-byte header. The format tolerates it even though the app
/// dissolves a trip that loses its last stage — the codec is policy-free.
#[test]
fn empty_trip_is_header_only() {
    let bytes = encode("Loose", &[]);
    assert_eq!(bytes.len(), TRIP_HEADER_LEN);
    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert!(meta.stage_ids.is_empty());
    assert!(!meta.truncated);
    assert_eq!(TripSummary::read(&SliceSource(&bytes)).unwrap().stage_count, 0);
}

/// The committed vector: "Alpen Traverse", stages `[7, 8, 99]` — 99 is the deliberate dangling ref
/// the codec carries verbatim (validation is the app's job). Re-encoding reproduces the file
/// byte-for-byte, so the production writer is pinned to the spec builder too.
#[test]
fn pins_the_committed_trip_v1_vector() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/trip-v1.bin");
    let Ok(bytes) = std::fs::read(&path) else {
        eprintln!("skipping: {} not reachable", path.display());
        return;
    };
    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert_eq!(meta.name, "Alpen Traverse");
    assert_eq!(meta.stage_ids.as_slice(), &[7u16, 8, 99]);
    assert!(!meta.truncated, "3 stages fit the resident cap");
    assert_eq!(TripSummary::read(&SliceSource(&bytes)).unwrap().stage_count, 3);

    // The production writer reproduces the fixture exactly.
    assert_eq!(encode("Alpen Traverse", &[7, 8, 99]), bytes);
}

/// A version byte other than 1 is rejected (the OBCR-style version gate).
#[test]
fn rejects_wrong_version() {
    let mut bytes = encode("X", &[1, 2]);
    bytes[0] = 2;
    assert_eq!(TripMeta::read(&SliceSource(&bytes)), Err(Error::BadVersion));
    assert_eq!(TripSummary::read(&SliceSource(&bytes)), Err(Error::BadVersion));
}

/// A file shorter than `56 + 2·stage_count` (a torn write) is rejected on the length check.
#[test]
fn rejects_length_mismatch() {
    let bytes = encode("X", &[1, 2, 3]);
    let short = &bytes[..bytes.len() - 1];
    assert_eq!(TripMeta::read(&SliceSource(short)), Err(Error::BadOffset));
    // A stray trailing byte (over-long) is rejected the same way.
    let mut long = bytes.clone();
    long.push(0);
    assert_eq!(TripSummary::read(&SliceSource(&long)), Err(Error::BadOffset));
}

/// A trip with more stages than the resident cap windows to the first `MAX_TRIP_STAGES` on read
/// (mirroring the waypoint-section windowing), with `truncated = true`; the summary keeps the true
/// stored count.
#[test]
fn windows_a_trip_past_the_stage_cap() {
    let over = MAX_TRIP_STAGES + 5;
    let stages: Vec<u16> = (0..over as u16).collect();
    let bytes = encode("Long", &stages);

    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert_eq!(meta.stage_ids.len(), MAX_TRIP_STAGES);
    assert_eq!(meta.stage_ids.as_slice(), &stages[..MAX_TRIP_STAGES]);
    assert!(meta.truncated);
    assert_eq!(TripSummary::read(&SliceSource(&bytes)).unwrap().stage_count, over as u16);
}

/// A name longer than the 48-byte cap is truncated on a char boundary by the writer (no panic, no
/// split multi-byte char).
#[test]
fn truncates_a_long_name() {
    let long = "ä".repeat(40); // 80 bytes — over the 48-byte cap
    let bytes = encode(&long, &[1]);
    let meta = TripMeta::read(&SliceSource(&bytes)).unwrap();
    assert!(meta.name.len() <= 48);
    assert!(long.starts_with(meta.name.as_str()));
    // The cut landed on a char boundary: 24 two-byte 'ä's = 48 bytes.
    assert_eq!(meta.name.chars().count(), 24);
}
