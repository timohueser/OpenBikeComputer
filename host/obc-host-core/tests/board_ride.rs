//! Actual board recorder over real journal bytes; only diagnostic/signal/writer transport is replaced.
extern crate self as defmt;
extern crate self as embassy_sync;
#[macro_export]
macro_rules! info {
    ($format:literal $(, $arg:expr)* $(,)?) => {{ $(let _ = &$arg;)* }};
}
#[macro_export]
macro_rules! warn { ($($args:tt)*) => { $crate::info!($($args)*) }; }
#[macro_export]
macro_rules! error { ($($args:tt)*) => { $crate::info!($($args)*) }; }
pub struct Debug2Format<T>(pub T);
pub mod blocking_mutex {
    pub mod raw {
        pub struct CriticalSectionRawMutex;
    }
}
pub mod signal {
    pub struct Signal<M, T>(core::marker::PhantomData<fn() -> (M, T)>);
    impl<M, T> Default for Signal<M, T> {
        fn default() -> Self {
            Self::new()
        }
    }
    impl<M, T> Signal<M, T> {
        pub const fn new() -> Self {
            Self(core::marker::PhantomData)
        }
    }
}
#[path = "../../../firmware/obc-fw-nrf54l/src/flat_ride.rs"]
#[allow(dead_code, unexpected_cfgs)]
mod flat_ride;
#[path = "board_ride_support/flat_store.rs"]
mod flat_store;

use flat_ride::{AppendResult, Recorder};
use flat_store::{FlatCard, Writer};
use obc_app::recorder::{CheckpointStatus, RecorderError, RideContinuation};
use obc_formats::ride::SAMPLE_LEN;
use obc_ports::TrackPoint;
use obc_storage::flat::{sim, FlatStore, StoreId};
use std::future::Future;

// The board owns one static delta/resume buffer. These tests preserve that same ownership.
static RECORDER: std::sync::Mutex<()> = std::sync::Mutex::new(());
fn complete<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => panic!("the test writer must complete synchronously"),
    }
}
fn context(n: u32) -> RideContinuation {
    RideContinuation {
        ridden_m: n as f32 * 4.25,
        moving_m: n as f32 * 3.75,
        moving_s: n as f32,
        climb_m: n as f32 * 0.375,
        descent_m: n as f32 * 0.125,
        hr_ms_sum: u64::from(n) * 301_000,
        hr_ms: n * 1_000,
        max_hr: 301,
        power_ms_sum: u64::from(n) * 200_000,
        power_ms: n * 1_000,
        max_power: 200,
        cadence_ms_sum: u64::from(n) * 85_000,
        cadence_ms: n * 1_000,
    }
}
fn point(n: u32) -> TrackPoint {
    TrackPoint {
        lat: 500_000 + n as i32,
        lon: 500_000,
        t_ms: n * 1_000,
        ele: 0,
        segment_start: false,
        hr: None,
        cadence: None,
        power: None,
    }
}
fn stats() -> obc_route::RideStats {
    obc_route::RideStats {
        distance_m: 0,
        moving_time_s: 0,
        avg_speed_cms: 0,
        climb_m: 0,
        unix_at_anchor: 0,
        anchor_ms: 0,
        clock_trusted: false,
        avg_hr: None,
        max_hr: None,
        avg_cadence: None,
        avg_power: None,
        max_power: None,
    }
}
fn setup() -> (&'static sim::SparseDisk, &'static FlatStore<FlatCard>, Writer, Recorder) {
    let media = Box::leak(Box::new(sim::SparseDisk::blank(2_000_000, 7)));
    let disk = Box::leak(Box::new(sim::FaultOnce::new(&*media)));
    let store = Box::leak(Box::new(FlatStore::initialize(&*disk, StoreId([0x71; 16])).unwrap()));
    let writer = Writer::new(store);
    let mut recorder = Recorder::new(store, writer, 0);
    complete(recorder.open(store, 1, "ride", 0));
    (media, store, writer, recorder)
}
fn recover(media: &'static sim::SparseDisk, disk: FlatCard) -> (&'static FlatStore<FlatCard>, Recorder) {
    media.reboot();
    let store = Box::leak(Box::new(FlatStore::mount(disk)));
    let recorder = Recorder::new(store, Writer::new(store), 0);
    (store, recorder)
}
fn recovered_points(store: &FlatStore<FlatCard>) -> Vec<TrackPoint> {
    let recovery = store.recovered_ride().unwrap();
    let mut bytes = vec![0; recovery.payload_len() as usize];
    assert_eq!(store.read_recovered(0, &mut bytes).unwrap(), bytes.len());
    bytes.chunks_exact(SAMPLE_LEN).map(|record| obc_formats::track::decode_record(record.try_into().unwrap())).collect()
}

#[test]
fn whole_batch_refusal_keeps_the_checkpoint_at_its_accepted_boundary() {
    let _owner = RECORDER.lock().unwrap();
    let (media, store, writer, mut recorder) = setup();
    let first: Vec<_> = (1..=15).map(point).collect();
    let tail = [point(16), point(17)];
    let stats = stats();
    assert_eq!(recorder.append(&first, context(15)), AppendResult::Accepted);
    assert_eq!(recorder.append(&tail, context(17)), AppendResult::NeedsCheckpoint);
    assert_eq!(complete(recorder.checkpoint(20_000, &stats, None)), Ok(CheckpointStatus::Durable));
    assert_eq!(writer.attempts.borrow()[0].append.len(), 15 * SAMPLE_LEN);
    // A power cut here restores exactly the previous accepted context, including unrounded sensors.
    let (reopened, mut recorder) = recover(media, store.device());
    assert_eq!(recorder.recovered_continuation(), Some(context(15)));
    assert_eq!(recovered_points(reopened), first);
    complete(recorder.open(reopened, 2, "ride", 15_000));
    assert_eq!(recorder.append(&tail, context(17)), AppendResult::Accepted);
    assert_eq!(complete(recorder.checkpoint(21_000, &stats, None)), Ok(CheckpointStatus::Durable));
    let (reopened, recorder) = recover(media, store.device());
    assert_eq!(recorder.recovered_continuation(), Some(context(17)));
    assert_eq!(recovered_points(reopened), [first.as_slice(), &tail].concat());
}

#[test]
fn failed_checkpoint_replays_before_a_new_context_only_checkpoint() {
    let _owner = RECORDER.lock().unwrap();
    let (media, store, writer, mut recorder) = setup();
    let points = [point(1), point(2)];
    let stats = stats();
    assert_eq!(recorder.append(&points, context(2)), AppendResult::Accepted);
    store.device().fault_next(sim::MediaOp::Sync);
    assert_eq!(complete(recorder.checkpoint(10_000, &stats, None)), Err(RecorderError::Write));
    assert_eq!(recorder.append(&[point(3)], context(3)), AppendResult::NeedsCheckpoint);
    // Even a fresh empty-staging context and upgraded wall clock cannot replace the failed tuple.
    let newer = obc_route::RideStats { clock_trusted: true, unix_at_anchor: 1_000_000, anchor_ms: 20_000, ..stats };
    assert_eq!(complete(recorder.checkpoint(20_000, &newer, Some(context(9)))), Ok(CheckpointStatus::Durable));
    assert_eq!(writer.attempts.borrow()[0], writer.attempts.borrow()[1]);
    let (reopened, mut recovered) = recover(media, store.device());
    assert_eq!(recovered.recovered_continuation(), Some(context(2)));
    assert_eq!(recovered_points(reopened), points);
    complete(recovered.open(reopened, 2, "ride", 20_000));
    let mut barometric = context(2);
    barometric.climb_m += 3.125;
    barometric.descent_m += 1.25;
    assert_eq!(complete(recovered.checkpoint(30_000, &newer, Some(barometric))), Ok(CheckpointStatus::Durable));
    let (reopened, recovered) = recover(media, store.device());
    assert_eq!(recovered.recovered_continuation(), Some(barometric));
    assert_eq!(recovered_points(reopened), points);
}

#[test]
fn empty_payload_preserves_valid_context_and_rejects_invalid_checkpoint_metadata() {
    use obc_storage::flat::{RideCheckpoint, Store, RIDE_RESUME_LEN};
    let _owner = RECORDER.lock().unwrap();
    let (media, store, _, _) = setup();
    let (reopened, mut recorder) = recover(media, store.device());
    assert_eq!(reopened.recovered_ride().unwrap().checkpoint_sequence, 0);
    assert_eq!(recorder.recovered_continuation(), Some(RideContinuation::default()));
    complete(recorder.open(reopened, 2, "ride", 0));
    let barometric = RideContinuation { climb_m: 3.125, descent_m: 1.25, ..RideContinuation::default() };
    assert_eq!(complete(recorder.checkpoint(10_000, &stats(), Some(barometric))), Ok(CheckpointStatus::Durable));
    let (mut reopened, recorder) = recover(media, store.device());
    assert_eq!(reopened.recovered_ride().unwrap().payload_len(), 0);
    assert_eq!(recorder.recovered_continuation(), Some(barometric));
    for resume in [[0; RIDE_RESUME_LEN], [1; RIDE_RESUME_LEN]] {
        let recovery = reopened.recovered_ride().unwrap();
        reopened
            .journal(RideCheckpoint {
                id: recovery.id,
                revision: recovery.revision,
                append: &[],
                payload_crc: 0,
                resume: &resume,
            })
            .unwrap();
        let (next, recorder) = recover(media, store.device());
        assert_eq!(recorder.recovered_continuation(), None);
        assert_eq!(recorder.recovery_damage(), Some(obc_app::RideDamage::Metadata));
        reopened = next;
    }
}
