//! Render-on-demand dirty tracking ([`App::take_dirty`], issue #47): the map plane re-renders only
//! when map-affecting state changed, and a static screen with no input and no motion drains
//! [`Dirty::CLEAN`]. The guiding rule is *over-redraw is safe, under-redraw is a bug*.
//!
//! `take_dirty` resets the accumulator, so every test drains the construction-time frame first and
//! then asserts about the next frame.

use obc_app::{App, AppState, Dirty, RouteSummary};
use obc_map_scene::BBox;
use obc_ports::{Button, Fix, FuelGauge, InputClock, InputEvent, LocationSource, RideClock, Sensors};

mod common;
use common::{down, keys, step, tap, NoFix, OnceFix};

const BERLIN: (i32, i32) = (52_520_000, 13_405_000); // (lat, lon) µdeg

/// Tick with one (or no) fix at `t`, no route, no other sensors.
fn tick(app: &mut App, loc: &mut dyn LocationSource, t: u32) {
    app.tick(RideClock(t), Sensors::new(loc), None);
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

    // An Up/Down step is recognized immediately as a Step gesture (Map zoom) → map dirty.
    assert!(frame(&mut app, &mut NoFix, 0, &[step(1)]).map, "a step (zoom) dirties the map");

    // The very next idle frame is clean again — the flag is one-shot, not sticky.
    assert_eq!(idle_frame(&mut app, 8), Dirty::CLEAN, "dirty is drained, not latched");
}

#[test]
fn charging_a_hold_dirties_only_the_overlay_then_fires_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let _ = app.take_dirty();

    // The press down alone recognizes no gesture, so the map stays clean.
    let d = frame(&mut app, &mut NoFix, 0, &[down(Button::Select)]);
    assert!(!d.map, "a bare button-down changes no screen — the map stays clean");

    // Past the dead zone the hold ring is live — overlay only, the map underneath unchanged.
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
    let mut loc = OnceFix(Some(Fix::at(BERLIN.0, BERLIN.1)));
    assert!(frame(&mut app, &mut loc, 0, &[]).map, "a fix that moves the camera dirties the map");

    // A second, *identical* fix moves nothing → the map stays clean, honouring "re-render only
    // on fixes that actually move the camera".
    let mut same = OnceFix(Some(Fix::at(BERLIN.0, BERLIN.1)));
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
        let mut loc = OnceFix(Some(Fix::at(lat, BERLIN.1))); // a genuinely moving fix
        assert!(!frame(&mut app, &mut loc, t, &[]).map, "a fix must not redraw static Home");
    }
}

// --- Statistics Inspect is persistent and redraws only on explicit input -----

/// One minimal route summary so the Route menu has something to load.
fn one_route() -> RouteSummary {
    let mut name = heapless::String::new();
    let _ = name.push_str("Loop");
    let bbox = BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 };
    RouteSummary { name, distance_km: 5, climb_m: 50, bbox, start_lon: 500, start_lat: 500 }
}

#[test]
fn statistics_inspect_has_no_automatic_snap_back_dirty_edge() {
    // Navigate to Statistics, enter profile Inspect with a step, then prove it remains quiet and
    // persistent until an explicit Back resets it.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.set_routes(&[one_route()]);

    let _ = frame(&mut app, &mut NoFix, 0, &tap(Button::Select)); // Home press → Menu (Routes selected)
    let _ = frame(&mut app, &mut NoFix, 5, &tap(Button::Select)); // press Routes → Route menu
    let _ = frame(&mut app, &mut NoFix, 10, &tap(Button::Select)); // pick route → Route overview
    let _ = frame(&mut app, &mut NoFix, 15, &tap(Button::Select)); // START RIDE → Map
    let _ = frame(&mut app, &mut NoFix, 20, &tap(Button::Back)); // Map `back` → Statistics

    // Scrub the cursor (a Step on Statistics) → that frame is dirty (the scrub moved it and entered
    // Inspect/Pan)…
    assert!(frame(&mut app, &mut NoFix, 30, &[step(1)]).map, "the scrub itself dirties the map");
    // …then idle frames remain quiet indefinitely — no old four-second spring-back may redraw or
    // throw away the chosen point.
    assert_eq!(idle_frame(&mut app, 100), Dirty::CLEAN, "frozen cursor → no idle redraw");
    assert_eq!(idle_frame(&mut app, 30 + 4_000), Dirty::CLEAN, "the old spring-back deadline is gone");
    assert_eq!(idle_frame(&mut app, 60_000), Dirty::CLEAN, "Inspect remains explicit even much later");

    // Back is the one reset edge; its gesture dirties the Statistics frame, then Follow is quiet.
    assert!(frame(&mut app, &mut NoFix, 60_010, &tap(Button::Back)).map, "Back resets Inspect visibly");
    assert_eq!(idle_frame(&mut app, 60_100), Dirty::CLEAN, "the reset adds no follow-up timer redraw");
}

// --- the battery gauge: slow polling + redraw only on an actual change -------

/// A [`FuelGauge`] that counts its polls and reports a settable level.
struct CountingGauge {
    value: u8,
    polls: u32,
}
impl FuelGauge for CountingGauge {
    fn poll(&mut self) -> Option<u8> {
        self.polls += 1;
        Some(self.value)
    }
}

#[test]
fn battery_is_polled_on_a_slow_cadence_and_redraws_home_only_on_change() {
    // Two guarantees: the gauge is read a few times a minute (not every ~8 ms frame, so a real I²C
    // read never spins), and an unchanged level repaints nothing.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]; battery_pct defaults to 75
    let _ = app.take_dirty(); // drain the mandatory first frame

    let mut gauge = CountingGauge { value: 75, polls: 0 };
    // Tick on Home with the gauge, returning whether Home (the map plane) was dirtied.
    let beat = |app: &mut App, gauge: &mut CountingGauge, t: u32| {
        let mut nofix = NoFix;
        let s = Sensors { fuel: Some(gauge), ..Sensors::new(&mut nofix) };
        app.tick(RideClock(t), s, None);
        app.take_dirty().map
    };

    // The first tick forces a read; 75 % matches the boot default, so nothing redraws.
    assert!(!beat(&mut app, &mut gauge, 0), "an unchanged level redraws nothing");
    assert_eq!(gauge.polls, 1, "polled once on the first tick");

    // A burst of frames inside the 30 s window: the gauge is NOT re-read, nothing redraws.
    for t in [10, 250, 5_000, 29_999] {
        assert!(!beat(&mut app, &mut gauge, t), "no redraw between cadence reads");
    }
    assert_eq!(gauge.polls, 1, "not re-read every frame — no per-frame I²C traffic");

    // At the cadence it is read again, but the value is unchanged ⇒ Home still does not redraw.
    assert!(!beat(&mut app, &mut gauge, 30_000), "an unchanged reading at the cadence still redraws nothing");
    assert_eq!(gauge.polls, 2, "read once the 30 s cadence elapsed");

    // A changed level is only seen at the next cadence — and then it repaints Home and is stored.
    gauge.value = 60;
    assert!(!beat(&mut app, &mut gauge, 45_000), "still inside the window since the last read");
    assert!(beat(&mut app, &mut gauge, 60_000), "a changed level repaints Home");
    assert_eq!(app.state.device.battery_pct, 60, "and the new level is stored");
}

/// The #209 flip side: the riding views don't draw the gauge, so a battery-level change must not
/// dirty the map there — otherwise a stationary rider eats a wasted ~97 ms full render every 30 s
/// battery tick. (The bug snapshotted the live-data baseline *before* the battery poll, so a pure
/// `battery_pct` delta tripped the `state != state_before` check.)
#[test]
fn a_battery_change_does_not_redraw_the_riding_views() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map] → base is Map, which shows live data
    let _ = app.take_dirty(); // drain the mandatory first frame

    let mut gauge = CountingGauge { value: 75, polls: 0 }; // 75 % = the boot default
    let beat = |app: &mut App, gauge: &mut CountingGauge, t: u32| {
        let mut nofix = NoFix;
        let s = Sensors { fuel: Some(gauge), ..Sensors::new(&mut nofix) };
        app.tick(RideClock(t), s, None);
        app.take_dirty().map
    };

    // First poll matches the default, so nothing changes regardless of the screen.
    assert!(!beat(&mut app, &mut gauge, 0), "an unchanged level redraws nothing");

    // A genuinely changed level at the next cadence: it is stored, but the Map view must stay put —
    // the gauge isn't on it, so the riding render-on-demand budget isn't spent on a battery tick.
    gauge.value = 60;
    assert!(!beat(&mut app, &mut gauge, 30_000), "a battery delta must not dirty the riding map (#209)");
    assert_eq!(app.state.device.battery_pct, 60, "the new level is still stored, just not drawn on the riding view");
}
