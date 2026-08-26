//! Render-on-demand dirty tracking (issue #47): the map plane re-renders only when what a visible
//! screen draws actually changed, and a static screen with no input and no motion plans
//! [`Dirty::CLEAN`]. The guiding rule is *over-redraw is safe, under-redraw is a bug*.
//!
//! Every test here drives the **DeviceCore pass** ([`common::Frames`]) rather than the frame
//! methods, because the pass is where the render keys are compared (#1447) — and it is the frame
//! every runtime host runs. The pass drains the demand, so the construction-time frame is drained
//! by the first frame and each assertion is about its own.

use obc_app::{App, AppState, Dirty, RouteSummary};
use obc_map_scene::BBox;
use obc_ports::{Button, Fix};

mod common;
use common::{down, step, tap, Frames};

const BERLIN: (i32, i32) = (52_520_000, 13_405_000); // (lat, lon) µdeg

// --- the first-frame contract ------------------------------------------------

#[test]
fn first_drain_is_map_dirty_then_idle_is_clean() {
    // A fresh app must paint once (nothing is on the panel yet)…
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05)); // [Home, Map]
    let mut host = Frames::new();
    assert!(host.idle(&mut app, 0).map, "the construction frame paints the first map");

    // …and then, with no input and no fix, fall completely quiet.
    assert_eq!(host.idle(&mut app, 1), Dirty::CLEAN, "a static map with no input is clean");
    assert_eq!(host.idle(&mut app, 9), Dirty::CLEAN, "and stays clean across idle frames");
}

// --- input dirties the map ---------------------------------------------------

#[test]
fn a_recognized_gesture_dirties_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let mut host = Frames::new();
    host.idle(&mut app, 0); // drain the first frame

    // An Up/Down step is recognized immediately as a Step gesture (Map zoom) → map dirty.
    assert!(host.frame(&mut app, 1, &[step(1)], None, None).map, "a step (zoom) dirties the map");

    // The very next idle frame is clean again — the flag is one-shot, not sticky.
    assert_eq!(host.idle(&mut app, 8), Dirty::CLEAN, "dirty is drained, not latched");
}

#[test]
fn charging_a_hold_dirties_only_the_overlay_then_fires_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let mut host = Frames::new();
    host.idle(&mut app, 0);

    // The press down alone recognizes no gesture, so the map stays clean.
    let d = host.frame(&mut app, 1, &[down(Button::Select)], None, None);
    assert!(!d.map, "a bare button-down changes no screen — the map stays clean");

    // Past the dead zone the hold ring is live — overlay only, the map underneath unchanged.
    let d = host.idle(&mut app, 300);
    assert!(d.overlay, "the charging ring lives on the overlay plane");
    assert!(!d.map, "…and never touches the map while charging");

    // Crossing the threshold fires the Hold gesture (Map → enter Pan) → the map changes.
    assert!(host.idle(&mut app, 600).map, "the hold firing (enter pan) dirties the map");
}

// --- fixes: only a camera-moving fix on a live view dirties the map ----------

#[test]
fn a_camera_moving_fix_on_the_map_dirties_it_but_a_stationary_one_does_not() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map], Follow
    let mut host = Frames::new();
    host.idle(&mut app, 0);

    // A fresh fix recenters the Follow camera → the map moved → dirty. No input this frame.
    let fix = Some(Fix::at(BERLIN.0, BERLIN.1));
    assert!(host.frame(&mut app, 1, &[], fix, None).map, "a fix that moves the camera dirties the map");

    // A second, *identical* fix moves nothing → the map stays clean, honouring "re-render only
    // on fixes that actually move the camera".
    assert!(!host.frame(&mut app, 1_000, &[], fix, None).map, "a stationary fix does not redraw");
}

#[test]
fn fixes_do_not_redraw_the_home_screensaver() {
    // The headline criterion: on Home the camera still follows fixes, but nothing Home draws
    // depends on them — so a moving fix must NOT redraw it. `new_idle` boots to [Home], Follow.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    let mut host = Frames::new();
    host.idle(&mut app, 0);

    for (i, lat) in [BERLIN.0, BERLIN.0 + 5_000, BERLIN.0 + 10_000].into_iter().enumerate() {
        let t = 1 + i as u32 * 1000;
        let fix = Some(Fix::at(lat, BERLIN.1)); // a genuinely moving fix
        assert!(!host.frame(&mut app, t, &[], fix, None).map, "a fix must not redraw static Home");
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
                                                            // The idle return is off: this test asserts quiet across a full minute, and a 30 s timeout
                                                            // would sweep the screen out from under it — a repaint that is correct and not the one at issue.
    app.set_settings(obc_app::Settings { idle_return: obc_app::IdleReturn::Never, ..*app.settings() });
    app.set_routes_with_ids(&[one_route()], &[0]);
    let mut host = Frames::new();

    host.frame(&mut app, 0, &tap(Button::Select), None, None); // Home press → Menu (Routes selected)
    host.frame(&mut app, 5, &tap(Button::Select), None, None); // press Routes → Route menu
    host.frame(&mut app, 10, &tap(Button::Select), None, None); // pick route → Route overview
    host.frame(&mut app, 15, &tap(Button::Select), None, None); // START RIDE → Map
    host.frame(&mut app, 20, &tap(Button::Back), None, None); // Map `back` → Statistics

    // Scrub the cursor (a Step on Statistics) → that frame is dirty (the scrub moved it and entered
    // Inspect/Pan)…
    assert!(host.frame(&mut app, 30, &[step(1)], None, None).map, "the scrub itself dirties the map");
    // …then idle frames remain quiet indefinitely — no old four-second spring-back may redraw or
    // throw away the chosen point.
    assert_eq!(host.idle(&mut app, 100), Dirty::CLEAN, "frozen cursor → no idle redraw");
    assert_eq!(host.idle(&mut app, 30 + 4_000), Dirty::CLEAN, "the old spring-back deadline is gone");
    assert_eq!(host.idle(&mut app, 60_000), Dirty::CLEAN, "Inspect remains explicit even much later");

    // Back is the one reset edge; its gesture dirties the Statistics frame, then Follow is quiet.
    assert!(host.frame(&mut app, 60_010, &tap(Button::Back), None, None).map, "Back resets Inspect visibly");
    assert_eq!(host.idle(&mut app, 60_100), Dirty::CLEAN, "the reset adds no follow-up timer redraw");
}

/// The idle return moves the visible stack on a timer, with no gesture behind it — the shape half
/// of the render key is what repaints it, and this pins that the pass actually reports it.
#[test]
fn the_idle_return_repaints_what_it_navigated_to() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    let mut host = Frames::new();
    host.frame(&mut app, 0, &tap(Button::Select), None, None); // Home press → Menu

    assert_eq!(host.idle(&mut app, 20_000), Dirty::CLEAN, "inside the 30 s window nothing moves");
    assert!(host.idle(&mut app, 31_000).map, "past it the return lands on Home and repaints");
    assert_eq!(host.idle(&mut app, 32_000), Dirty::CLEAN, "…once, not every pass hereafter");
}

// --- the battery gauge: slow polling + redraw only on an actual change -------

#[test]
fn battery_is_polled_on_a_slow_cadence_and_redraws_home_only_on_change() {
    // Two guarantees: the gauge is read a few times a minute (not every ~8 ms frame, so a real I²C
    // read never spins), and an unchanged level repaints nothing.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]; battery_pct defaults to 75
    let mut host = Frames::new();
    host.frame(&mut app, 0, &[], None, Some(75)); // drain the mandatory first frame
    assert_eq!(host.fuel_polls, 1, "polled once on the first frame");

    // A burst of frames inside the 30 s window: the gauge is NOT re-read, nothing redraws.
    for t in [10, 250, 5_000, 29_999] {
        assert!(!host.frame(&mut app, t, &[], None, Some(75)).map, "no redraw between cadence reads");
    }
    assert_eq!(host.fuel_polls, 1, "not re-read every frame — no per-frame I²C traffic");

    // At the cadence it is read again, but the value is unchanged ⇒ Home still does not redraw.
    assert!(!host.frame(&mut app, 30_000, &[], None, Some(75)).map, "an unchanged reading still redraws nothing");
    assert_eq!(host.fuel_polls, 2, "read once the 30 s cadence elapsed");

    // A changed level is only seen at the next cadence — and then it repaints Home and is stored.
    assert!(!host.frame(&mut app, 45_000, &[], None, Some(60)).map, "still inside the window since the last read");
    assert!(host.frame(&mut app, 60_000, &[], None, Some(60)).map, "a changed level repaints Home");
    assert_eq!(app.state.device.battery_pct, 60, "and the new level is stored");
}

/// The #209 flip side: the riding views don't draw the gauge, so a battery-level change must not
/// dirty the map there — otherwise a stationary rider eats a wasted ~97 ms full render every 30 s
/// battery tick. The level is now named by Home's render key alone, which is what makes this hold
/// by construction rather than by a hand-placed snapshot order.
#[test]
fn a_battery_change_does_not_redraw_the_riding_views() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map] → base is Map, a riding view
    let mut host = Frames::new();
    host.frame(&mut app, 0, &[], None, Some(75)); // 75 % = the boot default; drains the first frame
                                                  // The browse map's "Press to start a ride" hint auto-hides after 4 s — a timed change of the
                                                  // screen's own, drained here so the assertion below is about the battery and nothing else.
    host.frame(&mut app, 5_000, &[], None, Some(75));

    // A genuinely changed level at the next cadence: it is stored, but the Map view must stay put —
    // the gauge isn't on it, so the riding render-on-demand budget isn't spent on a battery tick.
    assert!(!host.frame(&mut app, 30_000, &[], None, Some(60)).map, "a battery delta must not dirty the map (#209)");
    assert_eq!(app.state.device.battery_pct, 60, "the new level is still stored, just not drawn here");
}
