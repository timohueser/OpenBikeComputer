//! Render-on-demand dirty tracking: the map plane re-renders only when what a visible screen draws
//! actually changed, and a static screen with no input and no motion plans [`Dirty::CLEAN`]. The
//! rule is that over-redraw is safe and under-redraw is a bug. Every test here drives the DeviceCore
//! pass ([`crate::common::Frames`]), because the pass is where the render keys are compared and it
//! is the frame every runtime host runs.

use obc_app::{App, AppState, Dirty, RouteSummary};
use obc_map_scene::BBox;
use obc_ports::{Button, Fix};

use crate::common::{down, step, tap, up, Frames};

const BERLIN: (i32, i32) = (52_520_000, 13_405_000); // (lat, lon) µdeg

#[test]
fn first_drain_is_map_dirty_then_idle_is_clean() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05)); // [Home, Map]
    let mut host = Frames::new();
    assert!(host.idle(&mut app, 0).map, "the construction frame paints the first map");

    assert_eq!(host.idle(&mut app, 1), Dirty::CLEAN, "a static map with no input is clean");
    assert_eq!(host.idle(&mut app, 9), Dirty::CLEAN, "and stays clean across idle frames");
}

#[test]
fn a_recognized_gesture_dirties_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let mut host = Frames::new();
    host.idle(&mut app, 0); // drain the first frame

    assert!(host.frame(&mut app, 1, &[step(1)], None, None).map, "a step (zoom) dirties the map");

    assert_eq!(host.idle(&mut app, 8), Dirty::CLEAN, "dirty is drained, not latched");
}

#[test]
fn charging_a_hold_dirties_only_the_overlay_then_fires_the_map() {
    let mut app = App::new(AppState::new(BERLIN.1, BERLIN.0, 0.05));
    let mut host = Frames::new();
    host.idle(&mut app, 0);

    let d = host.frame(&mut app, 1, &[down(Button::Select)], None, None);
    assert!(!d.map, "a bare button-down changes no screen — the map stays clean");

    // Past the dead zone the hold ring is live.
    let d = host.idle(&mut app, 300);
    assert!(d.overlay, "the charging ring lives on the overlay plane");
    assert!(!d.map, "…and never touches the map while charging");

    assert!(host.idle(&mut app, 600).map, "the hold firing (enter pan) dirties the map");
}

#[test]
fn a_camera_moving_fix_on_the_map_dirties_it_but_a_stationary_one_does_not() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map], Follow
    let mut host = Frames::new();
    host.idle(&mut app, 0);

    let fix = Some(Fix::at(BERLIN.0, BERLIN.1));
    assert!(host.frame(&mut app, 1, &[], fix, None).map, "a fix that moves the camera dirties the map");

    assert!(!host.frame(&mut app, 1_000, &[], fix, None).map, "a stationary fix does not redraw");
}

#[test]
fn fixes_do_not_redraw_the_home_screensaver() {
    // On Home the camera still follows fixes, but nothing Home draws depends on them, so a moving
    // fix must not redraw it. `new_idle` boots to [Home], Follow.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    let mut host = Frames::new();
    host.idle(&mut app, 0);

    for (i, lat) in [BERLIN.0, BERLIN.0 + 5_000, BERLIN.0 + 10_000].into_iter().enumerate() {
        let t = 1 + i as u32 * 1000;
        let fix = Some(Fix::at(lat, BERLIN.1)); // a genuinely moving fix
        assert!(!host.frame(&mut app, t, &[], fix, None).map, "a fix must not redraw static Home");
    }
}

/// One minimal route summary so the Route menu has something to load.
fn one_route() -> RouteSummary {
    let mut name = heapless::String::new();
    let _ = name.push_str("Loop");
    let bbox = BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 };
    RouteSummary { name, distance_km: 5, climb_m: 50, bbox, start_lon: 500, start_lat: 500 }
}

#[test]
fn statistics_inspect_has_no_automatic_snap_back_dirty_edge() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
                                                            // The idle return is off: this test asserts quiet across a full minute, and a 30 s timeout
                                                            // would sweep the screen out from under it — a repaint that is correct and not the one at issue.
    app.set_settings(obc_app::Settings { idle_return: obc_app::IdleReturn::Never, ..*app.settings() });
    app.set_routes_with_ids(&[one_route()], &[0]);
    crate::common::mount_store(&mut app); // a device with a card — START RIDE needs somewhere to record
    let mut host = Frames::new();

    host.frame(&mut app, 0, &tap(Button::Select), None, None); // Home press → Menu (Routes selected)
    host.frame(&mut app, 5, &tap(Button::Select), None, None); // press Routes → Route menu
    host.frame(&mut app, 10, &tap(Button::Select), None, None); // pick route → Route overview
    host.frame(&mut app, 15, &tap(Button::Select), None, None); // START RIDE → Map
    host.frame(&mut app, 20, &tap(Button::Back), None, None); // Map `back` → Statistics

    assert!(host.frame(&mut app, 30, &[step(1)], None, None).map, "the scrub itself dirties the map");
    assert_eq!(host.idle(&mut app, 100), Dirty::CLEAN, "frozen cursor → no idle redraw");
    assert_eq!(host.idle(&mut app, 30 + 4_000), Dirty::CLEAN, "the old spring-back deadline is gone");
    assert_eq!(host.idle(&mut app, 60_000), Dirty::CLEAN, "Inspect remains explicit even much later");

    assert!(host.frame(&mut app, 60_010, &tap(Button::Back), None, None).map, "Back resets Inspect visibly");
    assert_eq!(host.idle(&mut app, 60_100), Dirty::CLEAN, "the reset adds no follow-up timer redraw");
}

/// The idle return moves the visible stack on a timer, with no gesture behind it: the shape half of
/// the render key is what repaints it.
#[test]
fn the_idle_return_repaints_what_it_navigated_to() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    let mut host = Frames::new();
    host.frame(&mut app, 0, &tap(Button::Select), None, None); // Home press → Menu

    assert_eq!(host.idle(&mut app, 20_000), Dirty::CLEAN, "inside the 30 s window nothing moves");
    assert!(host.idle(&mut app, 31_000).map, "past it the return lands on Home and repaints");
    assert_eq!(host.idle(&mut app, 32_000), Dirty::CLEAN, "…once, not every pass hereafter");
}

#[test]
fn battery_is_polled_on_a_slow_cadence_and_redraws_home_only_on_change() {
    // Two guarantees: the gauge is read a few times a minute (not every ~8 ms frame, so a real I²C
    // read never spins), and an unchanged level repaints nothing.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]; battery_pct defaults to 75
    let mut host = Frames::new();
    host.frame(&mut app, 0, &[], None, Some(75)); // drain the mandatory first frame
    assert_eq!(host.fuel_polls, 1, "polled once on the first frame");

    for t in [10, 250, 5_000, 29_999] {
        assert!(!host.frame(&mut app, t, &[], None, Some(75)).map, "no redraw between cadence reads");
    }
    assert_eq!(host.fuel_polls, 1, "not re-read every frame — no per-frame I²C traffic");

    assert!(!host.frame(&mut app, 30_500, &[], None, Some(75)).map, "an unchanged reading still redraws nothing");
    assert_eq!(host.fuel_polls, 2, "read once the 30 s cadence elapsed");

    // Home's clock rolls into a new minute at 60 s and repaints itself for it, drained on its own
    // frame so the gauge assertions below are about the gauge.
    assert!(host.frame(&mut app, 61_000, &[], None, Some(75)).map, "the minute rollover is Home's own repaint");
    assert!(!host.frame(&mut app, 61_500, &[], None, Some(75)).map, "…and it settles straight back to quiet");

    assert!(!host.frame(&mut app, 75_000, &[], None, Some(60)).map, "still inside the window since the last read");
    assert!(host.frame(&mut app, 91_500, &[], None, Some(60)).map, "a changed level repaints Home");
    assert_eq!(app.state.device.battery_pct, 60, "and the new level is stored");
}

/// The riding views do not draw the gauge, so a battery-level change must not dirty the map there:
/// a stationary rider would otherwise eat a wasted ~97 ms full render every 30 s battery tick.
#[test]
fn a_battery_change_does_not_redraw_the_riding_views() {
    let mut app = App::new(AppState::new(0, 0, 0.05)); // [Home, Map] → base is Map, a riding view
    let mut host = Frames::new();
    host.frame(&mut app, 0, &[], None, Some(75)); // 75 % = the boot default; drains the first frame
                                                  // The browse map's "Press to start a ride" hint auto-hides after 4 s — a timed change of the
                                                  // screen's own, drained here so the assertion below is about the battery and nothing else.
    host.frame(&mut app, 5_000, &[], None, Some(75));

    assert!(!host.frame(&mut app, 30_000, &[], None, Some(60)).map, "a battery delta must not dirty the map (#209)");
    assert_eq!(app.state.device.battery_pct, 60, "the new level is still stored, just not drawn here");
}

/// A frozen base's timers are frozen with it. On a resident host the base's draw is skipped under a
/// sheet, so a base tick asking for a repaint would get a frame that never draws the base — pixels
/// asked for and not delivered, which the dirty contract calls a bug. Home's minute ticker is the
/// probe: it is the one base timer that needs no rendered frame to arm.
#[test]
fn a_minute_rollover_under_an_open_drawer_asks_for_nothing() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
                                                            // The idle return off: this test waits out a whole minute under the sheet, and a 30 s timeout
                                                            // would close the drawer mid-assertion — a repaint that is correct and not the one at issue.
    app.set_settings(obc_app::Settings { idle_return: obc_app::IdleReturn::Never, ..*app.settings() });
    let mut host = Frames::new();
    host.idle(&mut app, 0);
    assert!(host.idle(&mut app, 60_000).map, "Home repaints on the minute");
    assert_eq!(host.idle(&mut app, 60_100), Dirty::CLEAN, "…and then goes quiet again");

    // Squeeze the quick drawer open over it and let the sheet land.
    for (dt, ev) in
        [(0, down(Button::Up)), (40, down(Button::Select)), (100, up(Button::Select)), (120, up(Button::Up))]
    {
        host.frame(&mut app, 61_000 + dt, &[ev], None, None);
    }
    host.idle(&mut app, 62_000);
    assert_eq!(app.top_screen().name(), "QuickDrawer", "the sheet is up");
    assert_eq!(host.idle(&mut app, 62_500), Dirty::CLEAN, "a settled sheet over a frozen base is quiet");

    assert_eq!(host.idle(&mut app, 120_000), Dirty::CLEAN, "the base under a sheet does not tick");
    assert_eq!(host.idle(&mut app, 120_500), Dirty::CLEAN, "and stays quiet across the boundary");
}
