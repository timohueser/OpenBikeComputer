//! The host→app BLE event/state seam (epic #447, P1 + the P8 extension): [`App::set_ble_status`]
//! and the `StoreChanged` event, the three-state link + paired flag the Bluetooth screen
//! reads, and the connected indicator's dirty-tracking contract — a link change repaints only
//! where the state is drawn (Home / the menu title bar / the Bluetooth screen), never on a riding
//! view or a static screen whose status is unchanged.

use obc_app::{App, AppState, BleLink, BleStatus, Dirty, HostCommand, HostMailbox};

mod common;

fn connected() -> BleStatus {
    BleStatus { link: BleLink::Connected, passkey: None, paired: true }
}

/// Whether a `ForgetBond` is pending (the `take_ble_forget` successor). FAR-19, #812.
fn took_forget(app: &mut App) -> bool {
    let mut mb: HostMailbox = HostMailbox::new();
    let _ = app.drain_host_commands(&mut mb);
    core::iter::from_fn(|| mb.pop()).any(|c| matches!(c, HostCommand::ForgetBond))
}

// --- the state seam ----------------------------------------------------------

#[test]
fn set_ble_status_records_link_paired_and_passkey() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    assert_eq!(app.state.ble_link, BleLink::Advertising, "boots unlinked (radio on, nobody connected)");
    assert!(!app.state.ble_connected(), "…which reads as not connected for the indicator");
    assert!(!app.state.ble_paired, "no bond at boot");
    assert_eq!(app.ble_passkey(), None, "no passkey at boot");

    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: Some(123_456), paired: true });
    assert!(app.state.ble_connected(), "connection is recorded on AppState (the indicator reads it)");
    assert!(app.state.ble_paired, "the stored-bond flag rides the seam (the Paired row reads it)");
    assert_eq!(app.ble_passkey(), Some(123_456), "passkey rides the seam (P2 consumes it)");

    app.set_ble_status(BleStatus { link: BleLink::Off, ..BleStatus::DISCONNECTED });
    assert_eq!(app.state.ble_link, BleLink::Off, "the radio-off state crosses the seam (P8 status line)");
    assert!(!app.state.ble_connected(), "Off is not connected");
    assert_eq!(app.ble_passkey(), None);

    app.set_ble_status(BleStatus::DISCONNECTED);
    assert_eq!(app.state.ble_link, BleLink::Advertising, "back to the powered-and-unlinked default");
}

/// The Forget-phone one-shot: the `ForgetBond` command drains the screen's pending request once.
#[test]
fn take_ble_forget_is_a_one_shot() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    assert!(!took_forget(&mut app), "nothing pending at boot");
    app.state.ble_forget_pending = true; // as the Bluetooth screen's guarded hold sets it
    assert!(took_forget(&mut app), "the pending request drains…");
    assert!(!took_forget(&mut app), "…exactly once");
}

#[test]
fn notify_store_changed_counts_pending_signals() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    assert_eq!(app.store_changed_pending(), 0, "nothing pending at boot");
    app.apply_event(obc_app::HostEvent::StoreChanged);
    app.apply_event(obc_app::HostEvent::StoreChanged);
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

/// The Bluetooth screen (P8) draws the status line + Paired row, so every seam change repaints it
/// — including transitions the indicator ignores (Advertising ↔ Off, a paired flip).
#[test]
fn a_link_change_repaints_the_bluetooth_screen() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.apply_gesture(obc_app::Gesture::BackHold); // → Menu
    app.apply_gesture(obc_app::Gesture::Turn(-1)); // compass: one ccw detent to Settings
    app.apply_gesture(obc_app::Gesture::Press); // → Settings list
    app.apply_gesture(obc_app::Gesture::Turn(2)); // → Connections row (Ride, Display, Connections)
    app.apply_gesture(obc_app::Gesture::Press); // → Connections menu (Phone is the first row)
    app.apply_gesture(obc_app::Gesture::Press); // → Bluetooth screen (opened via the Phone row)
    assert!(matches!(app.top_screen(), obc_app::Screen::Bluetooth(_)), "navigated to the Bluetooth screen");
    let _ = app.take_dirty();

    app.set_ble_status(connected());
    assert!(app.take_dirty().map, "connecting repaints the status line");
    app.set_ble_status(BleStatus { link: BleLink::Off, passkey: None, paired: true });
    assert!(app.take_dirty().map, "the radio winding down to Off repaints it too");
    app.set_ble_status(BleStatus { link: BleLink::Off, passkey: None, paired: false });
    assert!(app.take_dirty().map, "a forget's paired yes→no repaints the Paired row");
    app.set_ble_status(BleStatus { link: BleLink::Off, passkey: None, paired: false });
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "the steady state repaints nothing");
}

// --- the passkey card (P2): host-pushed open/close on the seam's passkey --------

fn pairing(passkey: u32) -> BleStatus {
    BleStatus { link: BleLink::Advertising, passkey: Some(passkey), paired: false }
}

#[test]
fn a_passkey_opens_the_card_and_clearing_it_closes_the_card() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    assert!(!app.passkey_card_up(), "no card at boot");
    let _ = app.take_dirty();

    // A passkey going Some opens the host-pushed card over whatever is up, dirtying the map once.
    app.set_ble_status(pairing(42));
    assert!(app.passkey_card_up(), "a passkey opens the card");
    assert!(app.take_dirty().map, "opening the card dirties the map (it covers the screen below)");

    // The same passkey re-fed each pass (the steady state the board pushes) is a no-op — no re-dirty.
    app.set_ble_status(pairing(42));
    assert!(app.passkey_card_up(), "the card stays up");
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "an unchanged passkey never re-dirties");

    // Clearing the passkey (pairing complete/failed, or disconnect) removes the card and repaints
    // what it covered — again exactly once.
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.passkey_card_up(), "clearing the passkey closes the card");
    assert!(app.take_dirty().map, "closing the card repaints the screen it covered");

    // And clearing again (steady disconnected state) does nothing.
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "no card, no passkey — a no-op");
}

#[test]
fn the_card_opens_over_whatever_screen_is_up_and_restores_it_on_close() {
    // The rider is deep in a menu when pairing starts: the card overlays it, and closing returns to it.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.apply_gesture(obc_app::Gesture::BackHold); // Home → Menu
    let _ = app.take_dirty();

    app.set_ble_status(pairing(7));
    assert!(app.passkey_card_up(), "the card opens over the Menu");

    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.passkey_card_up(), "the card is gone");
    // The Menu is the input-receiving screen again (the card left no residue on the stack).
    app.apply_gesture(obc_app::Gesture::Back); // Menu → Home (proves the Menu, not the card, took it)
    assert!(!app.passkey_card_up());
}

#[test]
fn the_card_is_not_dismissible_by_input() {
    // Pairing is modal + time-boxed: Back/press on the card do nothing (the rider can't lose the code).
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.set_ble_status(pairing(99));
    assert!(app.passkey_card_up());

    app.apply_gesture(obc_app::Gesture::Back);
    assert!(app.passkey_card_up(), "Back does not dismiss the card");
    app.apply_gesture(obc_app::Gesture::Press);
    assert!(app.passkey_card_up(), "press does not dismiss the card");
    app.apply_gesture(obc_app::Gesture::Turn(1));
    assert!(app.passkey_card_up(), "a turn does not dismiss the card");

    // Only the seam clearing the passkey closes it.
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.passkey_card_up());
}

#[test]
fn a_hold_charging_defers_the_card_until_the_hold_settles() {
    // A host-pushed screen must never land mid-hold — it would yank the hold target out from under
    // the rider. The board feeds the live encoder hold-progress via `set_hold_progress`.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));

    app.set_hold_progress(0.5); // a hold is charging
    app.set_ble_status(pairing(1));
    assert!(!app.passkey_card_up(), "the card is deferred while a hold charges");

    // The desired state is re-fed every pass; once the hold settles the reconcile lands.
    app.set_hold_progress(0.0);
    app.set_ble_status(pairing(1));
    assert!(app.passkey_card_up(), "the card opens once the hold settles");

    // Closing is deferred too: don't pop mid-hold.
    app.set_hold_progress(0.5);
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(app.passkey_card_up(), "the card is held up while a hold charges");
    app.set_hold_progress(0.0);
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.passkey_card_up(), "the card closes once the hold settles");
}

// --- the indicator is NOT on the riding views: no repaint there --------------

#[test]
fn a_link_change_does_not_repaint_the_map_or_statistics() {
    // The Map / Statistics views deliberately omit the glyph, so a link change must not force their
    // expensive redraw. `App::new` boots straight onto [Home, Map].
    let mut app = App::new(AppState::new(0, 0, 0.05)); // base = Map
                                                       // Start a tracking session so the Map↔Statistics sibling ring exists (the Map's `back` swaps to
                                                       // Statistics only while tracking; without a ride it pops back to the Menu).
    app.activity.start_session();
    let _ = app.take_dirty();
    app.set_ble_status(connected());
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "a link change never redraws the Map");

    // Map → Statistics (`back`), then the same must hold.
    app.apply_gesture(obc_app::Gesture::Back);
    let _ = app.take_dirty();
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "a link change never redraws Statistics");
}
