//! Boot-time recovered-ride offer: one-shot card, exact continuation, and guarded discard.

mod common;

use common::NoFix;
use obc_app::{App, AppState, Gesture, Mode, RideContinuation, Screen, TrackAction};
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
    assert!(app.offer_recovered_ride(expected));
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));
    assert!(!app.offer_recovered_ride(RideContinuation::default()), "the boot offer is one-shot");

    app.apply_gesture(Gesture::Press); // entry selection = Continue ride
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.mode(), Mode::Riding);
    assert!(app.activity.is_tracking());
    assert_eq!(app.activity.ride_continuation(), expected, "the choice itself preserves every accumulator");

    app.tick(RideClock(0), Sensors::new(&mut NoFix), None);
    assert_eq!(
        app.activity.ride_continuation(),
        expected,
        "RideEngine must consume the continuation edge instead of applying the fresh-session reset"
    );
}

#[test]
fn discard_is_guarded_posts_the_existing_action_and_returns_home() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.offer_recovered_ride(continuation()));
    app.apply_gesture(Gesture::Step(1)); // Continue ride → Discard

    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)), "a tap cannot discard recovered bytes");
    assert_eq!(app.activity.take_track_action(), None);

    app.apply_gesture(Gesture::Hold);
    assert!(matches!(app.top_screen(), Screen::Home(_)));
    assert_eq!(app.mode(), Mode::Idle);
    assert!(!app.activity.is_tracking());
    assert_eq!(app.activity.take_track_action(), Some(TrackAction::Discard));
    assert!(!app.offer_recovered_ride(continuation()), "the decided offer never reopens this boot");
}

#[test]
fn back_cannot_dismiss_the_recovery_decision() {
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    assert!(app.offer_recovered_ride(continuation()));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::RideRecovery(_)));
}
