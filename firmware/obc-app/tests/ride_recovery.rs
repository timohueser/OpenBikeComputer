//! Boot-time recovered-ride offer: one-shot card, exact continuation, and guarded discard.

mod common;

use common::NoFix;
use obc_app::device_core::{ExternalFacts, OutcomeSlots};
use obc_app::recorder::{RecorderEffect, RecorderError, RecorderOutcome};
use obc_app::{App, AppState, Gesture, Mode, RecorderIntent, RideContinuation, RideDamage, Screen};
use obc_ports::{RideClock, Sensors};

fn continuation() -> RideContinuation {
    RideContinuation {
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
    common::mount_store(&mut app);
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
}

#[test]
fn discard_is_guarded_becomes_a_discard_effect_and_returns_home() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    common::mount_store(&mut app);
    assert!(app.offer_recovered_ride(continuation()));
    app.apply_gesture(Gesture::Step(1)); // Continue ride → Discard

    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "a tap cannot discard recovered bytes");
    assert!(common::quiet_pass(&mut app, 1).effects.recorder.is_empty(), "and orders nothing");

    app.apply_gesture(Gesture::Hold);
    assert!(matches!(app.top_screen(), Screen::Home(_)));
    assert_eq!(app.mode(), Mode::Idle);
    assert!(!app.recording());
    // The recovered object belongs to no session, and it still has to leave the store.
    let mut plan = common::quiet_pass(&mut app, 2);
    assert!(matches!(plan.effects.recorder.take(), Some(RecorderEffect::Discard { .. })));
    assert!(!app.offer_recovered_ride(continuation()), "the decided offer never reopens this boot");
}

#[test]
fn back_cannot_dismiss_the_recovery_decision() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    common::mount_store(&mut app);
    assert!(app.offer_recovered_ride(continuation()));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));
}

/// The whole rider path through a failed repair, over real gestures and real passes: the damaged
/// offer, one confirmed removal, the store's refusal, the terminal card, a device that stays usable,
/// a START that re-raises the decision instead of opening a phantom session, the retry the rider
/// gets without a reboot, and the ride that opens the moment the removal lands.
#[test]
fn the_failed_repair_card_retries_without_a_reboot() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    common::mount_store(&mut app);
    assert!(app.offer_damaged_ride(RideDamage::Payload), "the boot offer names the damage");
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));

    // The rider confirms. One hold, one effect.
    app.apply_gesture(Gesture::Hold);
    assert!(matches!(app.top_screen(), Screen::Home(_)), "the confirmed card returns Home");
    let effect = common::quiet_pass(&mut app, 1).effects.recorder.take().expect("the confirmed removal");
    let RecorderEffect::Discard { token } = effect else { panic!("the repair is the exact removal: {effect:?}") };

    // The store refuses it. The card comes back in its failed mode — and **no warning card lands on
    // top of it**: the typed card is the one explanation, and `REC_ERROR` means a ride log went
    // incomplete, which is not what happened here.
    let mut outcomes = OutcomeSlots::new();
    outcomes.recorder.try_put(RecorderOutcome::Failed { token, error: RecorderError::Write }).unwrap();
    let mut facts = ExternalFacts::NONE;
    let plan = common::pass(&mut app, 2, &mut outcomes, &mut facts, None);
    assert!(plan.effects.recorder.is_empty(), "the failure ordered nothing behind itself");
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the card is back, with no warning over it");

    // Nothing happens by itself from here — the finding this slice closes.
    for pass in 0..5 {
        assert!(
            common::quiet_pass(&mut app, 3 + pass * 15_000).effects.recorder.is_empty(),
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

    // A START against the standing damaged object opens no session and puts the decision back.
    app.recorder.request(RecorderIntent::Start);
    common::quiet_pass(&mut app, 100_000);
    assert!(!app.recording(), "no phantom session opens against a damaged object");
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "the rider is shown what stands in the way");

    // Retry, with no reboot anywhere in this test. This time the removal commits.
    app.apply_gesture(Gesture::Hold);
    let retry = common::quiet_pass(&mut app, 100_001).effects.recorder.take().expect("the retried removal");
    assert!(matches!(retry, RecorderEffect::Discard { .. }));
    let mut outcomes = OutcomeSlots::new();
    outcomes.recorder.try_put(RecorderOutcome::Discarded { token: retry.token() }).unwrap();
    let mut facts = ExternalFacts::NONE;
    common::pass(&mut app, 100_002, &mut outcomes, &mut facts, None);

    // Recording is available at once, in the same boot.
    app.recorder.request(RecorderIntent::Start);
    common::quiet_pass(&mut app, 100_003);
    assert!(app.recording(), "the repaired card records again without a reboot");
}
