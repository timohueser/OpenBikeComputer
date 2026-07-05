//! The host→app BLE event/state seam (epic #447, P1): [`App::set_ble_status`] and
//! [`App::notify_store_changed`], and the connected indicator's dirty-tracking contract — a link
//! change repaints only where the glyph is drawn (Home / the menu title bar), never on a riding view
//! or a static screen whose status is unchanged.

use obc_app::{App, AppState, BleStatus, Dirty};

mod common;

fn connected() -> BleStatus {
    BleStatus { connected: true, passkey: None }
}

// --- the state seam ----------------------------------------------------------

#[test]
fn set_ble_status_records_connection_and_passkey() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    assert!(!app.state.ble_connected, "boots disconnected");
    assert_eq!(app.ble_passkey(), None, "no passkey at boot");

    app.set_ble_status(BleStatus { connected: true, passkey: Some(123_456) });
    assert!(app.state.ble_connected, "connection is recorded on AppState (the indicator reads it)");
    assert_eq!(app.ble_passkey(), Some(123_456), "passkey rides the seam (P2 consumes it)");

    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.state.ble_connected, "disconnect clears it");
    assert_eq!(app.ble_passkey(), None);
}

#[test]
fn notify_store_changed_counts_pending_signals() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    assert_eq!(app.store_changed_pending(), 0, "nothing pending at boot");
    app.notify_store_changed();
    app.notify_store_changed();
    assert_eq!(app.store_changed_pending(), 2, "a burst of commits accumulates — never coalesced away");
}

// --- the indicator's dirty contract (on the connected-glyph screens) --------

#[test]
fn a_link_change_repaints_the_home_indicator() {
    // Home draws the indicator beside the battery gauge, so a link change must dirty the map…
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    let _ = app.take_dirty(); // drain the boot paint

    app.set_ble_status(connected());
    assert!(app.take_dirty().map, "connecting repaints Home (the indicator appears)");

    // …but re-feeding the same status — the steady state, pushed every pass — repaints nothing.
    app.set_ble_status(connected());
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "an unchanged status is a no-op (no repaint)");

    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(app.take_dirty().map, "disconnecting repaints Home (the indicator vanishes)");
}

#[test]
fn a_link_change_repaints_the_menu_title_bar() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.apply_gesture(obc_app::Gesture::BackHold); // Home → Menu (the connected indicator is in its title bar)
    let _ = app.take_dirty();

    app.set_ble_status(connected());
    assert!(app.take_dirty().map, "connecting on the Menu repaints its title bar");
    app.set_ble_status(connected());
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "an unchanged status doesn't re-dirty the Menu");
}

// --- the indicator is NOT on the riding views: no repaint there --------------

#[test]
fn a_link_change_does_not_repaint_the_map_or_statistics() {
    // The Map / Statistics views deliberately omit the glyph, so a link change must not force their
    // expensive redraw. `App::new` boots straight onto [Home, Map].
    let mut app = App::new(AppState::new(0, 0, 0.05)); // base = Map
    let _ = app.take_dirty();
    app.set_ble_status(connected());
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "a link change never redraws the Map");

    // Map → Statistics (`back`), then the same must hold.
    app.apply_gesture(obc_app::Gesture::Back);
    let _ = app.take_dirty();
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "a link change never redraws Statistics");
}
