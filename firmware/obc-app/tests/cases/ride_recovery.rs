//! Boot-time recovered-ride offer: one-shot card, exact continuation, and guarded discard.

use crate::common::NoFix;
use obc_app::device_core::{ExternalFacts, OutcomeSlots};
use obc_app::recorder::{RecorderEffect, RecorderError, RecorderOutcome};
use obc_app::{App, AppState, Gesture, Mode, RideContinuation, RideDamage, RideOrigin, Screen, TripInput};
use obc_formats::bike::BikeType;
use obc_formats::ride::TripRef;
use obc_ports::{RideClock, Sensors};

fn continuation() -> RideContinuation {
    RideContinuation {
        origin: RideOrigin { bike: BikeType::Touring, trip: TripRef::new(1, 1, 2) },
        ridden_m: 12_345.0,
        moving_m: 12_000.0,
        moving_s: 2_700.0,
        climb_m: 456.0,
        descent_m: 321.0,
        hr_ms_sum: 150 * 10_000,
        hr_ms: 10_000,
        max_hr: 181,
        power_ms_sum: 220 * 8_000,
        power_ms: 8_000,
        max_power: 640,
        cadence_ms_sum: 84 * 9_000,
        cadence_ms: 9_000,
    }
}

#[test]
fn continue_preserves_restored_totals_through_the_first_tick() {
    let expected = continuation();
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    crate::common::mount_store(&mut app);
    assert!(app.offer_recovered_ride(expected));
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));
    assert!(!app.offer_recovered_ride(RideContinuation::default()), "the boot offer is one-shot");

    app.apply_gesture(Gesture::Press); // entry selection = Continue ride
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.mode(), Mode::Riding);
    assert!(app.recording());
    assert_eq!(app.recorder.continuation(), expected, "the choice itself preserves every accumulator");

    app.tick(RideClock(0), Sensors::new(&mut NoFix), None);
    assert_eq!(
        app.recorder.continuation(),
        expected,
        "Navigator must consume the continuation edge instead of applying the fresh-session reset"
    );

    app.set_trips(&[TripInput { id: 5, key: 1, name: "Alps", start_date: 0, stage_ids: &[] }]);
    let stats = app.ride_stats();
    assert_eq!(
        (stats.bike, stats.trip, stats.trip_name.as_str()),
        (BikeType::Touring, TripRef::new(1, 1, 2), "Alps"),
        "the continued ride keeps its trip day and names the trip at save"
    );
}

#[test]
fn discard_is_guarded_becomes_a_discard_effect_and_returns_home() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    crate::common::mount_store(&mut app);
    assert!(app.offer_recovered_ride(continuation()));
    app.apply_gesture(Gesture::Step(1)); // Continue ride → Discard

    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "a tap cannot discard recovered bytes");
    assert!(crate::common::quiet_pass(&mut app, 1).effects.recorder.is_empty(), "and orders nothing");

    app.apply_gesture(Gesture::Hold);
    assert!(matches!(app.top_screen(), Screen::Home(_)));
    assert_eq!(app.mode(), Mode::Idle);
    assert!(!app.recording());
    // The recovered object belongs to no session, and it still has to leave the store.
    let mut plan = crate::common::quiet_pass(&mut app, 2);
    assert!(matches!(plan.effects.recorder.take(), Some(RecorderEffect::Discard { .. })));
    assert!(!app.offer_recovered_ride(continuation()), "the decided offer never reopens this boot");
}

#[test]
fn back_cannot_dismiss_the_recovery_decision() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    crate::common::mount_store(&mut app);
    assert!(app.offer_recovered_ride(continuation()));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));
}

fn start_from_home(app: &mut App) {
    assert!(matches!(app.top_screen(), Screen::Home(_)));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Menu(_)));
    app.apply_gesture(Gesture::Step(2)); // Routes → Rides → Map
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RideStart(_)));
    app.apply_gesture(Gesture::Press); // Start ride
}

/// The whole rider path through a failed repair, over real gestures and real passes: the damaged
/// offer, one confirmed removal, the store's refusal, the terminal card, a device that stays usable,
/// a START that re-raises the decision instead of opening a phantom session, the retry the rider
/// gets without a reboot, and the ride that opens the moment the removal lands.
#[test]
fn the_failed_repair_card_retries_without_a_reboot() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    crate::common::mount_store(&mut app);
    assert!(app.offer_damaged_ride(RideDamage::Payload), "the boot offer names the damage");
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));

    // The rider confirms. One hold, one effect.
    app.apply_gesture(Gesture::Hold);
    assert!(matches!(app.top_screen(), Screen::Home(_)), "the confirmed card returns Home");
    let effect = crate::common::quiet_pass(&mut app, 1).effects.recorder.take().expect("the confirmed removal");
    let RecorderEffect::Discard { token } = effect else { panic!("the repair is the exact removal: {effect:?}") };

    // The store refuses it. The card comes back in its failed mode, and no warning card lands on
    // top of it: the typed card is the one explanation, and `REC_ERROR` means a ride log went
    // incomplete, which is not what happened here.
    let mut outcomes = OutcomeSlots::new();
    outcomes.recorder.try_put(RecorderOutcome::Failed { token, error: RecorderError::Write }).unwrap();
    let mut facts = ExternalFacts::NONE;
    let plan = crate::common::pass(&mut app, 2, &mut outcomes, &mut facts, None);
    assert!(plan.effects.recorder.is_empty(), "the failure ordered nothing behind itself");
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the card is back, with no warning over it");

    // Nothing happens by itself from here.
    for pass in 0..5 {
        assert!(
            crate::common::quiet_pass(&mut app, 3 + pass * 15_000).effects.recorder.is_empty(),
            "pass {pass}: a latched failure re-attempts nothing"
        );
    }

    // The failed card's second row leaves it, and the device is usable again: the global escape,
    // refused while the card was rooted, opens the Menu.
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Home(_)), "the Back row is a plain press");
    app.apply_gesture(Gesture::BackHold);
    assert!(matches!(app.top_screen(), Screen::Menu(_)), "non-recording functions are the rider's again");
    app.apply_gesture(Gesture::Back);

    // The real Start card must not overwrite Recorder's recovery decision with its Map transition.
    start_from_home(&mut app);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the decision wins over the requested Map");
    let plan = crate::common::quiet_pass(&mut app, 100_000);
    assert!(plan.effects.recorder.is_empty(), "a refused Start cannot write to the standing object");
    assert!(!app.recording(), "no phantom session opens against a damaged object");
    assert_eq!(app.mode(), Mode::Idle, "a refused Start leaves the device in its non-recording mode");
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the decision survives the next pass");

    // Leaving and asking again remains usable and raises the same rider-controlled decision.
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert_eq!(app.mode(), Mode::Idle);
    start_from_home(&mut app);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));

    // Retry, with no reboot anywhere in this test. This time the removal commits.
    app.apply_gesture(Gesture::Hold);
    let retry = crate::common::quiet_pass(&mut app, 100_001).effects.recorder.take().expect("the retried removal");
    assert!(matches!(retry, RecorderEffect::Discard { .. }));
    let mut outcomes = OutcomeSlots::new();
    outcomes.recorder.try_put(RecorderOutcome::Discarded { token: retry.token() }).unwrap();
    let mut facts = ExternalFacts::NONE;
    crate::common::pass(&mut app, 100_002, &mut outcomes, &mut facts, None);

    // The same visible Start action records at once after repair, in the same boot.
    start_from_home(&mut app);
    crate::common::quiet_pass(&mut app, 100_003);
    assert!(app.recording(), "the repaired card records again without a reboot");
    assert_eq!(app.mode(), Mode::Riding);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
}
