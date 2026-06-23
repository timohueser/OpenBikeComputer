//! The two-plane input decomposition ([`InputPlane`] + [`App::apply_gesture`] +
//! [`App::advance_animations`], issue #48). Pins the contract the firmware's preemptive split
//! relies on: driving the app through the *decomposed* path (recognise on a standalone
//! `InputPlane`, apply each gesture, advance animations) is behaviour-identical to the
//! single-call [`App::handle_input`] the simulator uses — so relocating the recogniser off
//! `App` changed nothing observable, and the overlay the firmware drives on its own plane stays
//! in lock-step with the gestures the map plane applies.

use obc_app::{App, AppState, Button, Gesture, InputClock, InputEvent, InputPlane, RouteSummary};
use obc_reader::BBox;

mod common;
use common::{down, keys, turn, up};

/// One minimal route summary so the Route menu has something to load.
fn one_route() -> RouteSummary {
    let mut name = heapless::String::new();
    let _ = name.push_str("Loop");
    let bbox = BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 };
    RouteSummary { name, distance_km: 5, climb_m: 50, bbox, start_lon: 500, start_lat: 500 }
}

/// Drive `app` through the **two-plane decomposition**: recognise this frame's input on a
/// standalone `InputPlane` (the firmware's high-priority plane), apply each recognised gesture,
/// then advance the timed screen content — exactly what the firmware's map-plane loop does after
/// draining the gesture channel.
fn drive_split(app: &mut App, plane: &mut InputPlane, t: u32, evs: &[InputEvent]) {
    let mut pending: Vec<Gesture> = Vec::new();
    plane.recognize(InputClock(t), &mut keys(evs), |g| pending.push(g));
    for g in pending {
        app.apply_gesture(g);
    }
    app.advance_animations(InputClock(t));
}

/// A representative session: boot Home → load a route → Map → zoom → enter pan via a hold →
/// pan → exit. Each step is `(t_ms, events)`; the down/idle pairs straddle the 500 ms hold
/// threshold so both `Hold` (enter pan) and `BackHold` (exit pan) fire through `gestures.tick`.
fn script() -> Vec<(u32, Vec<InputEvent>)> {
    vec![
        (0, vec![down(Button::Encoder)]),
        (80, vec![up(Button::Encoder)]),     // Home press → Route menu
        (200, vec![down(Button::Encoder)]),  //
        (280, vec![up(Button::Encoder)]),    // load route → Map (riding)
        (400, vec![turn(1)]),                // Map zoom in
        (450, vec![turn(-1)]),               // Map zoom out
        (600, vec![down(Button::Encoder)]),  // begin an encoder hold…
        (800, vec![]),                       // …charging past the dead zone (overlay live)
        (1200, vec![]),                      // crosses 500 ms → Hold fires → enter pan
        (1260, vec![up(Button::Encoder)]),   // release (silent after the hold)
        (1300, vec![turn(2)]),               // pan along the axis
        (1340, vec![down(Button::Encoder)]), // encoder press in pan → toggle axis
        (1380, vec![up(Button::Encoder)]),
        (1400, vec![turn(-1)]),           // pan back
        (1500, vec![down(Button::Back)]), // begin a Back hold…
        (2100, vec![]),                   // …crosses 500 ms → BackHold → exit pan
        (2160, vec![up(Button::Back)]),   // release
        (2200, vec![down(Button::Back)]), // a Back tap…
        (2280, vec![up(Button::Back)]),   // …Map back → Statistics
    ]
}

#[test]
fn split_path_matches_handle_input_state_and_map_dirty() {
    // Reference app: the simulator's single-call path.
    let mut single = App::new_idle(AppState::new(0, 0, 0.05));
    single.set_routes(&[one_route()]);
    let _ = single.take_dirty(); // drain the construction frame

    // Decomposed app: the firmware's map plane + a standalone input plane.
    let mut split = App::new_idle(AppState::new(0, 0, 0.05));
    split.set_routes(&[one_route()]);
    let _ = split.take_dirty();
    let mut plane = InputPlane::new();

    for (t, evs) in script() {
        single.handle_input(InputClock(t), &mut keys(&evs));
        drive_split(&mut split, &mut plane, t, &evs);

        // The camera / mode / pan state a screen mutates must match exactly (AppState is
        // Copy + PartialEq), so every gesture had the identical effect on both paths.
        assert_eq!(split.state, single.state, "app state diverged at t={t} (evs={evs:?})");
        // And the map-plane repaint demand must match — the decomposition dirties the map on
        // exactly the same frames.
        assert_eq!(split.take_dirty().map, single.take_dirty().map, "map-dirty diverged at t={t} (evs={evs:?})");

        // The overlay the firmware drives on its *own* plane (the standalone `InputPlane`) stays
        // in lock-step with the reference app's overlay (driven inside `handle_input`).
        assert_eq!(plane.overlay_active(), single.overlay_active(), "overlay liveness diverged at t={t} (evs={evs:?})");
    }

    // `last_gesture` is a readout of the recogniser, so on the two-plane path it lives on the
    // standalone `plane` (the firmware's high-priority plane), not on the map-plane `App` whose
    // own recogniser stays dormant there. Read it from the plane that actually recognised.
    assert_eq!(plane.last_gesture(), single.last_gesture());
    assert!(split.last_gesture().is_none(), "the map plane's own recogniser stays dormant");
}

#[test]
fn standalone_input_plane_recognizes_the_same_gestures_as_handle_input() {
    // The recogniser is clock-driven, so a held encoder crossing 500 ms must yield `Hold` from
    // a standalone plane exactly as it does inside `App`.
    let mut plane = InputPlane::new();
    let mut got: Vec<Gesture> = Vec::new();

    plane.recognize(InputClock(0), &mut keys(&[down(Button::Encoder)]), |g| got.push(g));
    assert!(got.is_empty(), "a bare button-down recognises nothing");
    plane.recognize(InputClock(300), &mut keys(&[]), |g| got.push(g));
    assert!(got.is_empty(), "still charging before the threshold");
    assert!(plane.overlay_active(), "the bulge is live past the dead zone");

    plane.recognize(InputClock(600), &mut keys(&[]), |g| got.push(g));
    assert_eq!(got, vec![Gesture::Hold], "the long-press fires the instant it crosses 500 ms");

    // A turn detent recognises immediately; a release after the hold is silent.
    got.clear();
    plane.recognize(InputClock(620), &mut keys(&[turn(3), up(Button::Encoder)]), |g| got.push(g));
    assert_eq!(got, vec![Gesture::Turn(3)], "turn fires immediately; the post-hold release is silent");
}
