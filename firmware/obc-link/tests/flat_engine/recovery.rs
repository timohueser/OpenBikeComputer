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

/// `specs/vectors/ride-v4.bin`: three 20-byte `TrackPoint` samples, then the fixed footer.
const RIDE_V4: &[u8] = include_bytes!("../../../../specs/vectors/ride-v4.bin");
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
///
/// The length and CRC are the caller's, because the retry after a cut must publish what the
/// *recovery* handed back rather than what the test knows the ride to be.
fn finalise<D: BlockDevice>(
    device: &Device<D>,
    (id, revision): (u64, u64),
    payload_len: u64,
    payload_crc: u32,
) -> Result<u64, StoreError> {
    let meta = EntryMeta {
        added_at_utc: 0,
        id: ObjectId(id),
        revision: Revision(revision),
        kind: ObjectKind::Ride,
        flags: EntryFlags::NONE,
        payload_len,
        payload_crc,
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
        let (samples, footer) = RIDE_V4.split_at(RIDE_V4.len() - FOOTER_LEN);
        checkpoint(&device, key, samples, samples);
        checkpoint(&device, key, footer, RIDE_V4);
        key
    };
    (disk, key)
}

/// Where finalisation first touches the fixed region, measured on a throwaway card's own write log.
/// Cutting before that operation means every journaled byte is durable and no catalog byte is.
///
/// It copies `obc-storage`'s `finalisation_retry_does_not_rewrite_an_intact_tail` recipe rather than
/// calling it, so neither suite can move the other's cut point without noticing.
fn catalog_write_offset() -> u32 {
    let (disk, key) = journaled_recording(1_420);
    let device = boot(&disk);
    let baseline = disk.ops();
    finalise(&device, key, RIDE_V4.len() as u64, crc32(RIDE_V4)).expect("the probe finalises");
    let (op, _, _) = disk
        .write_log()
        .into_iter()
        .find(|(op, lba, _)| *op > baseline && *lba < EXTENT_AREA)
        .expect("finalisation never reached its catalog");
    op - baseline
}

/// The whole chain a rider's interrupted ride travels: journaled on the card, finalisation cut
/// between the last journal write and the first catalog write, recovered by the store on the next
/// mount, finalised by the retried commit, served over `GET`, and exported to the pinned GPX.
///
/// It crosses the line `flat_break_matrix.rs` draws on purpose: each link is proved elsewhere, but
/// only here do the recovered bytes leave over the wire and become the file the phone saves.
#[test]
fn an_interrupted_recording_recovers_and_exports_the_pinned_gpx() {
    let cut_at = catalog_write_offset();
    let (disk, key) = journaled_recording(1_421);
    let (id, revision) = key;

    // The power fails between the last journal write and the first catalog write.
    let device = boot(&disk);
    let baseline = disk.ops();
    disk.plan(FaultPlan { op: baseline + cut_at, when: When::Before });
    assert_eq!(
        finalise(&device, key, RIDE_V4.len() as u64, crc32(RIDE_V4)),
        Err(StoreError::Media),
        "the cut did not land inside finalisation",
    );
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
    assert_eq!(recovered.payload_len(), RIDE_V4.len() as u64, "the recovery lost journaled bytes");
    assert_eq!(recovered.payload_crc, crc32(RIDE_V4), "the recovery reconstructed a different ride");

    // The recorder retries the commit, publishing what the recovery handed back — not what this test
    // knows the ride to be, so a recovery that rebuilt the wrong length or CRC cannot pass here.
    finalise(&device, key, recovered.payload_len(), recovered.payload_crc)
        .expect("the retried finalisation publishes the ride");
    let entry = device.entry(id).expect("the final catalog names the ride");
    assert!(!entry.flags.has(EntryFlags::RECORDING));
    assert_eq!((entry.payload_len, entry.payload_crc), (RIDE_V4.len() as u64, crc32(RIDE_V4)));
    assert!(device.store.recovered_ride().is_none(), "a published ride is still offered as recording");

    // What the phone asks for, and what it gets.
    let wire = device.control(&client::get(1, id, revision));
    let answer = Answer::of(wire.answer());
    assert!(!answer.is_error(), "{answer:?}");
    let payload = wire.payload();
    assert_eq!(payload, RIDE_V4, "GET served something other than the recovered ride");

    // What the phone makes of those bytes: the vector's own totals, then the pinned export.
    let source = SliceSource(&payload);
    let info = RideInfo::read(&source).expect("the recovered bytes are a finished ride");
    assert_eq!(info.name.as_str(), "Sensor Ride");
    assert_eq!(info.point_count, 3);
    assert_eq!((info.distance_m, info.moving_time_s, info.avg_speed_cms, info.climb_m), (12_345, 3_600, 343, 120));

    let mut sink = VecSink::default();
    track_to_gpx(&source, EXPORT_NAME, &mut sink).expect("the export runs");
    let gpx = String::from_utf8(sink.0).expect("the export is UTF-8");
    assert_eq!(gpx, include_str!("../../../../specs/vectors/track-export.gpx"), "the export is not the pinned one");
}
