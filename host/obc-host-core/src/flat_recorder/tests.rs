use super::*;
use crate::{FlatRideStore, RideRepository};
use obc_formats::io::ByteSource;

fn point(t_ms: u32) -> TrackPoint {
    TrackPoint {
        lon: 8_000_000,
        lat: 46_000_000,
        ele: 1000,
        t_ms,
        segment_start: false,
        hr: Some(130),
        cadence: Some(80),
        power: Some(200),
    }
}
fn context() -> RideContinuation {
    RideContinuation {
        ridden_m: 200.,
        moving_m: 180.,
        moving_s: 10.,
        climb_m: 12.5,
        descent_m: 3.5,
        hr_ms_sum: 1_300_000,
        hr_ms: 10_000,
        max_hr: 140,
        power_ms_sum: 2_000_000,
        power_ms: 10_000,
        max_power: 250,
        cadence_ms_sum: 800_000,
        cadence_ms: 10_000,
    }
}
fn stats() -> RideStats {
    RideStats {
        distance_m: 200,
        moving_time_s: 10,
        avg_speed_cms: 1800,
        climb_m: 12,
        unix_at_anchor: 1_720_000_000,
        anchor_ms: 12_000,
        clock_trusted: true,
        avg_hr: Some(130),
        max_hr: Some(140),
        avg_cadence: Some(80),
        avg_power: Some(200),
        max_power: Some(250),
    }
}
fn seed(owner: &HostStore) -> EntryMeta {
    let bytes = obc_route::encode_summary_footer("saved", &stats(), 0, None);
    owner.import(ObjectKind::Ride, None, &mut &bytes[..], bytes.len() as u64, DisplayName::default()).unwrap()
}
fn key(recorder: &FlatRideRecorder) -> Key {
    match &recorder.state {
        State::Live(live) => live.key,
        State::Closing(closing) => closing.live.key,
        State::Damaged(key, _) => *key,
        _ => panic!("no recording"),
    }
}
fn fail_sync(owner: &HostStore, n: usize) {
    let owner = owner.0.lock().unwrap();
    let crate::flat_store::HostMedia::File(card) = owner.card.device() else { panic!("file expected") };
    card.borrow().fail_sync_before.set(Some(n));
}
fn append(recorder: &mut FlatRideRecorder, points: &[TrackPoint]) {
    assert_eq!(recorder.append_batch(points, Some(context())), Ok(AppendStatus::Accepted(points.len() as u16)));
}

#[test]
fn batch_capacity_and_context_are_atomic_and_legacy_lifecycle_remains_compatible() {
    let owner = HostStore::memory().unwrap();
    let mut recorder = FlatRideRecorder::new(owner).unwrap();
    crate::conformance::track_lifecycle(&mut recorder);
    assert!(recorder.open(3, Some("boundary"), 0));
    append(&mut recorder, &[point(1000); 16]);
    let (crc, len, accepted) = match &recorder.state {
        State::Live(live) => (live.crc, live.len, live.continuation),
        _ => unreachable!(),
    };
    assert_eq!(
        recorder.append_batch(&[point(2000)], Some(RideContinuation::default())),
        Ok(AppendStatus::NeedsCheckpoint)
    );
    assert_eq!(recorder.append_batch(&[point(2000)], None), Ok(AppendStatus::Cancelled));
    let State::Live(live) = &recorder.state else { unreachable!() };
    assert_eq!((live.crc, live.len, live.continuation), (crc, len, accepted));
    assert_eq!(recorder.checkpoint(stats(), None), Ok(CheckpointStatus::Unsupported));
    append(&mut recorder, &[point(2000)]);
    assert!(matches!(recorder.finalize(stats()), RideClose::Committed(_)));
}

#[test]
fn reopen_continues_exact_object_clock_totals_with_all_reader_slots_full() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let mut recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    assert!(recorder.open(1, Some("original"), 9000));
    append(&mut recorder, &[point(10_000), point(12_000)]);
    assert_eq!(recorder.checkpoint(stats(), None), Ok(CheckpointStatus::Durable));
    let original = key(&recorder);
    drop((recorder, owner));

    let owner = HostStore::open_file(&path).unwrap();
    let mut recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    assert_eq!(recorder.recovered_continuation(), Some(context()));
    assert_eq!(key(&recorder).id, original.id);
    assert_eq!(owner.store_id().unwrap(), original.store);
    // Each finalized object occupies a distinct hold; recovery itself occupies none.
    let mut readers = Vec::new();
    for _ in 0..obc_storage::flat::store::MAX_OPEN_OBJECTS {
        let meta = seed(&owner);
        readers.push(owner.open(meta.id, meta.revision).unwrap());
    }
    assert!(recorder.open(2, Some("must not rename"), 500));
    append(&mut recorder, &[point(1500)]);
    let mut later_stats = stats();
    later_stats.anchor_ms = 1500;
    later_stats.unix_at_anchor += 3600;
    assert_eq!(recorder.finalize(later_stats), RideClose::Committed(original.id.0));
    assert!(matches!(owner.open(original.id, original.revision), Err(StoreError::Busy)));
    drop(readers);
    let source = owner.open(original.id, original.revision).unwrap();
    let info = obc_route::RideInfo::read(&source).unwrap();
    assert_eq!(info.name.as_str(), "original");
    assert_eq!(info.point_count, 3);
    assert_eq!(info.start_time, 1_719_999_998);
    assert_eq!((info.avg_hr, info.avg_power, info.avg_cadence, info.climb_m), (Some(130), Some(200), Some(80), 12));
    let mut bytes = [0; SAMPLE_LEN * 3];
    source.read_at(0, &mut bytes).unwrap();
    let times: Vec<_> =
        bytes.as_chunks::<SAMPLE_LEN>().0.iter().map(|sample| obc_formats::track::decode_record(sample).t_ms).collect();
    assert_eq!(times, [10_000, 12_000, 13_000]);
    let rides = FlatRideStore::new(owner).unwrap();
    assert!(rides.catalog().iter().any(|ride| ride.id == original.id.0 && !ride.summary.synced));
}

#[test]
fn failed_journal_replays_frozen_bytes_context_and_start_before_accepting_more() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let mut recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    assert!(recorder.open(1, Some("retry"), 0));
    append(&mut recorder, &[point(10_000)]);
    fail_sync(&owner, 1);
    assert_eq!(recorder.checkpoint(stats(), None), Err(RecorderError::Write));
    let State::Live(live) = &recorder.state else { unreachable!() };
    let frozen = (live.pending, live.crc, recorder.delta);
    assert_eq!(recorder.append_batch(&[point(11_000)], Some(context())), Ok(AppendStatus::NeedsCheckpoint));
    let mut changed = stats();
    changed.unix_at_anchor += 50;
    assert_eq!(recorder.checkpoint(changed, Some(RideContinuation::default())), Ok(CheckpointStatus::Durable));
    assert_eq!(recorder.delta, frozen.2);
    let id = key(&recorder).id.0;
    drop((recorder, owner));
    let owner = HostStore::open_file(&path).unwrap();
    let recovered = owner.0.lock().unwrap().card.recovered_ride().unwrap();
    assert_eq!(Some(recovered.resume), frozen.0);
    assert_eq!(recovered.payload_crc, frozen.1.finalize());
    let mut recorder = FlatRideRecorder::new(owner).unwrap();
    assert_eq!(recorder.recovered_continuation(), Some(context()));
    assert!(recorder.open(2, None, 0));
    assert_eq!(recorder.finalize(changed), RideClose::Committed(id));
}

#[test]
fn footer_recovery_is_terminal_and_failed_settlement_stops_startup() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let mut recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    assert!(recorder.open(1, Some("finish"), 0));
    append(&mut recorder, &[point(10_000)]);
    let original = key(&recorder);
    // The real finalization journals its footer, then its catalog publication fails.
    fail_sync(&owner, 3);
    assert_eq!(recorder.finalize(stats()), RideClose::Failed);
    assert!(matches!(&recorder.state, State::Closing(closing) if closing.journaled));
    drop((recorder, owner));

    let owner = HostStore::open_file(&path).unwrap();
    fail_sync(&owner, 2); // confirmation succeeds; the amendment's first sync fails.
    assert!(FlatRideRecorder::new(owner.clone()).is_err());
    assert!(owner.entries().is_err(), "uncertain catalog write fences the shared owner");
    drop(owner);
    let owner = HostStore::open_file(&path).unwrap();
    let recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    assert!(recorder.recovered_continuation().is_none());
    assert!(matches!(recorder.state, State::Idle));
    let source = owner.open(original.id, original.revision).unwrap();
    assert_eq!(source.len(), (SAMPLE_LEN + FOOTER_LEN) as u64);
    assert_eq!(obc_route::RideInfo::read(&source).unwrap().point_count, 1);
    drop((recorder, owner, source));
    let owner = HostStore::open_file(&path).unwrap();
    let before = owner.0.lock().unwrap().card.sequence();
    FlatRideRecorder::new(owner.clone()).unwrap();
    assert_eq!(owner.0.lock().unwrap().card.sequence(), before, "settled Save is not published twice");
}

#[test]
fn failed_open_and_live_remount_confirmation_never_authorize_new_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("card.obc");
    let owner = HostStore::create_file(&path).unwrap();
    let mut recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    fail_sync(&owner, 4);
    assert!(!recorder.open(1, None, 0));
    assert!(!recorder.open(1, None, 0));
    assert_eq!(recorder.finalize(stats()), RideClose::Failed);
    drop((recorder, owner));
    let owner = HostStore::open_file(&path).unwrap();
    fail_sync(&owner, 1);
    assert!(FlatRideRecorder::new(owner.clone()).is_err());
    assert!(owner.entries().is_err());
    assert!(crate::FlatRouteStore::new(owner, &[]).is_err());
}

#[test]
fn damaged_discard_checks_recording_kind_card_and_exact_revision() {
    let owner = HostStore::memory().unwrap();
    let mut recorder = FlatRideRecorder::new(owner.clone()).unwrap();
    assert!(recorder.open(1, Some("damaged"), 0));
    let original = key(&recorder);
    let unrelated = seed(&owner);
    recorder.state = State::Damaged(original, RideDamage::Metadata);
    let other = HostStore::memory().unwrap();
    let saved_owner = std::mem::replace(&mut recorder.owner, other);
    assert_eq!(recorder.discard(), Err(RecorderError::ReadOnly));
    recorder.owner = saved_owner;
    let mut stale = original;
    stale.revision = Revision(original.revision.0 + 1);
    recorder.state = State::Damaged(stale, RideDamage::Metadata);
    assert_eq!(recorder.discard(), Err(RecorderError::ReadOnly));
    recorder.state = State::Damaged(original, RideDamage::Metadata);
    assert_eq!(recorder.discard(), Ok(()));
    assert!(owner.open(unrelated.id, unrelated.revision).is_ok());
    assert_eq!(owner.0.lock().unwrap().card.recovered_ride(), None);
    recorder.state = State::Damaged(
        Key { store: original.store, id: unrelated.id, revision: unrelated.revision },
        RideDamage::Payload,
    );
    assert_eq!(recorder.discard(), Err(RecorderError::ReadOnly));
    assert!(owner.open(unrelated.id, unrelated.revision).is_ok());
}
