//! One recording interrupted by a power cut, carried all the way to the file a rider receives.
use super::*;
use flat_harness::VecSink;
use obc_flat_device::EXTENT_AREA;
use obc_formats::io::SliceSource;
use obc_formats::ride::FOOTER_LEN;
use obc_route::{track_to_gpx, RideInfo};
use obc_storage::flat::sim::{FaultPlan, SparseDisk, When};
use obc_storage::flat::{
    DisplayName, EntryFlags, EntryMeta, Mutation, ObjectId, ObjectKind, PutSource, Revision, RideCheckpoint, Store,
    StoreError, RIDE_RESUME_LEN,
};

/// `specs/vectors/ride-v3.bin`: three 20-byte `TrackPoint` samples, then the 84-byte v3 footer.
const RIDE_V3: &[u8] = include_bytes!("../../../../specs/vectors/ride-v3.bin");
/// The export `specs/vectors/track-export.gpx` was taken under (`obc_vectors::TRACK_NAME`); the
/// ride's own footer carries a different one, which is what [`RideInfo`] reads.
const EXPORT_NAME: &str = "Schauinsland & back";
/// What the recorder reserves when a ride starts: room for a whole one.
const RESERVE: u64 = 1024 * 1024;

/// One ride checkpoint, the `Store::journal` call the recorder makes: `append` is the delta since
/// the last one, `payload_crc` covers everything recorded so far.
fn checkpoint<D: BlockDevice>(device: &Device<D>, (id, revision): (u64, u64), append: &[u8], whole: &[u8]) {
    Store::journal(
        &device.store,
        RideCheckpoint {
            id: ObjectId(id),
            revision: Revision(revision),
            append,
            payload_crc: crc32(whole),
            resume: &[0; RIDE_RESUME_LEN],
        },
    )
    .expect("the checkpoint journals");
}

/// Finalisation: publish the journaled extents by clearing `RECORDING` in one amend commit.
fn finalise<D: BlockDevice>(device: &Device<D>, (id, revision): (u64, u64)) -> Result<u64, StoreError> {
    let meta = EntryMeta {
        added_at_utc: 0,
        id: ObjectId(id),
        revision: Revision(revision),
        kind: ObjectKind::Ride,
        flags: EntryFlags::NONE,
        payload_len: RIDE_V3.len() as u64,
        payload_crc: crc32(RIDE_V3),
        name: DisplayName::new("Sensor Ride").expect("a ride name"),
    };
    Store::commit(&device.store, &[Mutation::Put { meta, source: PutSource::Amend }])
}

/// A card holding the vector ride as the recorder left it: reserved, journaled in two checkpoints —
/// the samples as they were ridden, then the footer finalize appends — and not yet published.
fn journaled_recording(seed: u64) -> (SparseDisk, (u64, u64)) {
    let disk = formatted_card(seed);
    let key = {
        let mut device = boot(&disk);
        let key = device.seed_recording(RESERVE);
        let (samples, footer) = RIDE_V3.split_at(RIDE_V3.len() - FOOTER_LEN);
        checkpoint(&device, key, samples, samples);
        checkpoint(&device, key, footer, RIDE_V3);
        key
    };
    (disk, key)
}

/// Where finalisation first touches the fixed region, measured on a throwaway card's own write log.
/// Cutting before that operation means every payload byte is durable and no catalog byte is.
///
/// This is a copy of `obc-storage`'s `finalisation_retry_does_not_rewrite_an_intact_tail` recipe and
/// not a call into it, so neither suite can move the other's cut point without noticing.
fn catalog_write_offset() -> u32 {
    let (disk, key) = journaled_recording(1_420);
    let device = boot(&disk);
    let baseline = disk.ops();
    finalise(&device, key).expect("the probe finalises");
    let (op, _, _) = disk
        .write_log()
        .into_iter()
        .find(|(op, lba, _)| *op > baseline && *lba < EXTENT_AREA)
        .expect("finalisation never reached its catalog");
    op - baseline
}

/// The whole chain a rider's interrupted ride travels: journaled on the card, finalisation cut
/// between the last payload write and the catalog commit, recovered by the store on the next mount,
/// finalised by the same retried commit, served over `GET`, and exported to the pinned GPX.
///
/// This crosses the line `flat_break_matrix.rs` draws — a cut *inside* a commit belongs to the
/// storage crash matrix, a link break to this suite — deliberately, because the composition is the
/// claim: nowhere else do the recovered bytes leave over the wire and become the file the phone
/// saves. Each link of it is proved elsewhere; that they hold end to end is proved only here.
#[test]
fn an_interrupted_recording_recovers_and_exports_the_pinned_gpx() {
    let cut_at = catalog_write_offset();
    let (disk, key) = journaled_recording(1_421);
    let (id, revision) = key;

    // The power fails between the last payload write and the catalog commit.
    let device = boot(&disk);
    let baseline = disk.ops();
    disk.plan(FaultPlan { op: baseline + cut_at, when: When::Before });
    assert_eq!(finalise(&device, key), Err(StoreError::Media), "the cut did not land inside finalisation");
    // Nothing more can be asked of an unpowered card, so what the cut left is read after the reboot.
    drop(device);
    disk.reboot();

    // The next mount still calls it a recording, and finds every journaled byte of it.
    let mut device = boot(&disk);
    assert!(
        device.entry(id).expect("the ride is still catalogued").flags.has(EntryFlags::RECORDING),
        "a cut finalisation published the ride anyway",
    );
    let recovered = device.store.recovered_ride().expect("the store recovers the interrupted ride");
    assert_eq!((recovered.id.0, recovered.revision.0), key);
    assert_eq!(recovered.payload_len(), RIDE_V3.len() as u64, "the recovery lost journaled bytes");

    // The recorder retries the identical commit, which is all finalisation ever was.
    finalise(&device, key).expect("the retried finalisation publishes the ride");
    let entry = device.entry(id).expect("the final catalog names the ride");
    assert!(!entry.flags.has(EntryFlags::RECORDING));
    assert_eq!((entry.payload_len, entry.payload_crc), (RIDE_V3.len() as u64, crc32(RIDE_V3)));
    assert!(device.store.recovered_ride().is_none(), "a published ride is still offered as recording");

    // What the phone asks for, and what it gets.
    let wire = device.control(&client::get(1, id, revision));
    let answer = Answer::of(wire.answer());
    assert!(!answer.is_error(), "{answer:?}");
    let payload = wire.payload();
    assert_eq!(payload, RIDE_V3, "GET served something other than the recovered ride");

    // What the phone makes of those bytes: the vector's own totals, then the pinned export.
    let source = SliceSource(&payload);
    let info = RideInfo::read(&source).expect("the recovered bytes are a finished v3 ride");
    assert_eq!(info.name.as_str(), "Sensor Ride");
    assert_eq!(info.point_count, 3);
    assert_eq!((info.distance_m, info.moving_time_s, info.avg_speed_cms, info.climb_m), (12_345, 3_600, 343, 120));

    let mut sink = VecSink::default();
    track_to_gpx(&source, EXPORT_NAME, &mut sink).expect("the export runs");
    let gpx = String::from_utf8(sink.0).expect("the export is UTF-8");
    assert_eq!(gpx, include_str!("../../../../specs/vectors/track-export.gpx"), "the export is not the pinned one");
    assert_eq!(gpx.matches("<trkseg>").count(), 2, "the pause opens a second segment");
    let points: Vec<&str> = gpx.lines().filter(|line| line.starts_with("<trkpt")).collect();
    assert_eq!(points.len(), info.point_count as usize);
    assert_eq!(
        points.iter().collect::<std::collections::BTreeSet<_>>().len(),
        points.len(),
        "a point was exported twice"
    );
}
