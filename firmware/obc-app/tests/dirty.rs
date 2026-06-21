//! Render-on-demand dirty tracking ([`App::take_dirty`], issue #47). These pin the
//! contract the firmware's render loop relies on: the map plane re-renders only when
//! map-affecting state changed, and a static screen with no input and no motion drains
//! [`Dirty::CLEAN`] (zero map renders) — while the overlay plane stays free to repaint on
//! its own. The guiding rule is *over-redraw is safe, under-redraw is a bug*, so each
//! state mutation that affects the map must set `map`.
//!
//! `take_dirty` resets the accumulator, so every test drains the construction-time frame
//! first (the host's mandatory first paint) and then asserts about the *next* frame.

use std::collections::VecDeque;

use obc_app::{
    App, AppState, Button, ButtonEvent, Dirty, Fix, InputClock, InputEvent, InputSource,
    LocationSource, RideClock, RouteSummary, Sensors,
};
use obc_reader::BBox;

// --- scripted hardware -------------------------------------------------------

/// One scripted raw input event per `poll` — drives [`App::handle_input`].
struct Keys(VecDeque<InputEvent>);
impl InputSource for Keys {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}
fn keys(evs: &[InputEvent]) -> Keys {
    Keys(evs.iter().copied().collect())
}
fn turn(n: i32) -> InputEvent {
    InputEvent::Turn(n)
}
fn down(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Down(b))
}
fn up(b: Button) -> InputEvent {
    InputEvent::Button(ButtonEvent::Up(b))
}
/// A tap (down then up within the hold threshold) → a `Press` (Encoder) or `Back` gesture.
fn tap(b: Button) -> [InputEvent; 2] {
    [down(b), up(b)]
}

/// A [`LocationSource`] that emits its fix exactly once, then `None` — the real
/// one-fresh-fix-per-tick contract (no per-poll replay).
struct OneFix(Option<Fix>);
impl LocationSource for OneFix {
    fn poll(&mut self) -> Option<Fix> {
        self.0.take()
    }
}
struct NoFix;
impl LocationSource for NoFix {
    fn poll(&mut self) -> Option<Fix> {
        None
    }
}

const BERLIN: (i32, i32) = (52_520_000, 13_405_000); // (lat, lon) µdeg

/// Tick with one (or no) fix at `t`, no route, no other sensors.
fn tick(app: &mut App, loc: &mut dyn LocationSource, t: u32) {
    app.tick(RideClock(t), Sensors { loc, altimeter: None, track: None }, None);
}
/// Drive a full frame (input + tick) and drain it. `evs` is this frame's input.
fn frame(app: &mut App, loc: &mut dyn LocationSource, t: u32, evs: &[InputEvent]) -> Dirty {
    app.handle_input(InputClock(t), &mut keys(evs));
    tick(app, loc, t);
    app.take_dirty()
}
/// An idle frame: no input, no fix — the quiet tick the host runs between events.
fn idle_frame(app: &mut App, t: u32) -> Dirty {
    frame(app, &mut NoFix, t, &[])
}

// --- the first-frame contract ------------------------------------------------

#[test]
fn first_drain_is_map_dirty_then_idle_is_clean() {
    // A fresh app must paint once (nothing is on the panel yet)…
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05)); // [Home, Map]
    assert!(app.take_dirty().map, "the construction frame paints the first map");

    // …and then, with no input and no fix, fall completely quiet.
    assert_eq!(idle_frame(&mut app, 1), Dirty::CLEAN, "a static map with no input is clean");
    assert_eq!(idle_frame(&mut app, 9), Dirty::CLEAN, "and stays clean across idle frames");
}

// --- input dirties the map ---------------------------------------------------

#[test]
fn a_recognized_gesture_dirties_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let _ = app.take_dirty(); // drain the first frame

    // A turn detent is recognized immediately as a Turn gesture (Map zoom) → map dirty.
    assert!(frame(&mut app, &mut NoFix, 0, &[turn(1)]).map, "a turn (zoom) dirties the map");

    // The very next idle frame is clean again — the flag is one-shot, not sticky.
    assert_eq!(idle_frame(&mut app, 8), Dirty::CLEAN, "dirty is drained, not latched");
}

#[test]
fn charging_a_hold_dirties_only_the_overlay_then_fires_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let _ = app.take_dirty();

    // Press-and-hold the encoder. The press *down* alone recognizes no gesture, so the map
    // stays clean.
    let d = frame(&mut app, &mut NoFix, 0, &[down(Button::Encoder)]);
    assert!(!d.map, "a bare button-down changes no screen — the map stays clean");

    // Once the charge crosses the dead zone the hold *ring* is live — overlay only, the map
    // underneath is unchanged.
    let d = idle_frame(&mut app, 300);
    assert!(d.overlay, "the charging ring lives on the overlay plane");
    assert!(!d.map, "…and never touches the map while charging");

    // Crossing the threshold fires the Hold gesture (Map → enter Pan) → the map changes.
    assert!(idle_frame(&mut app, 600).map, "the hold firing (enter pan) dirties the map");
}

// --- fixes: only a camera-moving fix on a live view dirties the map ----------

#[test]
fn a_camera_moving_fix_on_the_map_dirties_it_but_a_stationary_one_does_not() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map], Follow
    let _ = app.take_dirty();

    // A fresh fix recenters the Follow camera → the map moved → dirty. No input this frame.
    let mut loc = OneFix(Some(Fix::at(BERLIN.0, BERLIN.1)));
    assert!(frame(&mut app, &mut loc, 0, &[]).map, "a fix that moves the camera dirties the map");

    // A second, *identical* fix moves nothing → the map stays clean, honouring "re-render only
    // on fixes that actually move the camera".
    let mut same = OneFix(Some(Fix::at(BERLIN.0, BERLIN.1)));
    assert!(!frame(&mut app, &mut same, 1000, &[]).map, "a stationary fix does not redraw");
}

#[test]
fn fixes_do_not_redraw_the_home_screensaver() {
    // The headline criterion: on Home the camera still follows fixes, but nothing Home draws
    // depends on them — so a moving fix must NOT redraw it. `new_idle` boots to [Home], Follow.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    let _ = app.take_dirty();

    for (i, lat) in [BERLIN.0, BERLIN.0 + 5_000, BERLIN.0 + 10_000].into_iter().enumerate() {
        let t = i as u32 * 1000;
        let mut loc = OneFix(Some(Fix::at(lat, BERLIN.1))); // a genuinely moving fix
        assert!(!frame(&mut app, &mut loc, t, &[]).map, "a fix must not redraw static Home");
    }
}

// --- the Statistics spring-back is a timed, observable map dirty -------------

/// One minimal route summary so the Route menu has something to load.
fn one_route() -> RouteSummary {
    let mut name = heapless::String::new();
    let _ = name.push_str("Loop");
    let bbox = BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 };
    RouteSummary { name, distance_km: 5, climb_m: 50, bbox, start_lon: 500, start_lat: 500 }
}

#[test]
fn statistics_spring_back_is_wired_into_the_dirty_signal() {
    // Navigate Home → Route menu → load → Map → Statistics, then prove the Statistics cursor's
    // *timed* spring-back surfaces as a map-dirty through `handle_input`'s animate sweep — with
    // no input and no fix in between (so nothing else could have dirtied it).
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.set_routes(&[one_route()]);

    let _ = frame(&mut app, &mut NoFix, 0, &tap(Button::Encoder)); // Home press → Route menu
    let _ = frame(&mut app, &mut NoFix, 10, &tap(Button::Encoder)); // load route → Map
    let _ = frame(&mut app, &mut NoFix, 20, &tap(Button::Back)); // Map `back` → Statistics

    // Scrub the cursor (a Turn on Statistics) → that frame is dirty (the scrub moved it)…
    assert!(frame(&mut app, &mut NoFix, 30, &[turn(1)]).map, "the scrub itself dirties the map");
    // …then idle frames inside the spring-back window are quiet — the frozen cursor draws the
    // same thing, so the map must not re-render.
    assert_eq!(idle_frame(&mut app, 100), Dirty::CLEAN, "frozen cursor → no idle redraw");
    assert_eq!(idle_frame(&mut app, 2_000), Dirty::CLEAN, "still frozen mid-window");
    // …and at the 4 s idle deadline the cursor springs back to live: a redraw driven purely by
    // the timer, with no input and no fix.
    assert!(
        idle_frame(&mut app, 30 + 4_000).map,
        "the spring-back dirties the map at the deadline"
    );
    // One-shot: the frame after is quiet again.
    assert_eq!(idle_frame(&mut app, 30 + 4_100), Dirty::CLEAN, "spring-back fires only once");
}
