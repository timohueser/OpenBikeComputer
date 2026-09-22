//! The host→app BLE state seam: [`App::set_ble_status`], the three-state link and paired flag the
//! Bluetooth screen reads, and the connected indicator's dirty-tracking contract — a link change
//! repaints only where the state is drawn (Home, the menu title bar, the Bluetooth screen), never
//! on a riding view or a static screen whose status is unchanged.

use obc_app::{App, AppState, BleLink, BleStatus, DeviceStatus, Dirty};

fn connected() -> BleStatus {
    BleStatus { link: BleLink::Connected, passkey: None, paired: true }
}

#[test]
fn set_ble_status_records_link_paired_and_passkey() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    assert_eq!(app.state.device.ble_link, BleLink::Advertising, "boots unlinked (radio on, nobody connected)");
    assert!(!app.state.device.ble_connected(), "…which reads as not connected for the indicator");
    assert!(!app.state.device.ble_paired, "no bond at boot");
    assert_eq!(app.ble_passkey(), None, "no passkey at boot");

    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: Some(123_456), paired: true });
    assert_eq!(
        app.state.device,
        DeviceStatus { battery_pct: 75, ble_link: BleLink::Connected, ble_paired: true },
        "the focused status owns only the small platform facts",
    );
    assert!(core::mem::size_of::<DeviceStatus>() <= 4, "the stored status stays register-sized");
    assert!(app.state.device.ble_connected(), "connection is recorded on DeviceStatus (the indicator reads it)");
    assert!(app.state.device.ble_paired, "the stored-bond flag rides the seam (the Paired row reads it)");
    assert_eq!(app.ble_passkey(), Some(123_456), "passkey rides the seam (P2 consumes it)");

    app.set_ble_status(BleStatus { link: BleLink::Off, ..BleStatus::DISCONNECTED });
    assert_eq!(app.state.device.ble_link, BleLink::Off, "the radio-off state crosses the seam (P8 status line)");
    assert!(!app.state.device.ble_connected(), "Off is not connected");
    assert_eq!(app.ble_passkey(), None);

    app.set_ble_status(BleStatus::DISCONNECTED);
    assert_eq!(app.state.device.ble_link, BleLink::Advertising, "back to the powered-and-unlinked default");
}

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

/// The Bluetooth screen draws the status line and Paired row, so every seam change repaints it,
/// including transitions the indicator ignores (Advertising ↔ Off, a paired flip).
#[test]
fn a_link_change_repaints_the_bluetooth_screen() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05)); // [Home]
    app.apply_gesture(obc_app::Gesture::BackHold); // → Menu
    app.apply_gesture(obc_app::Gesture::Step(-1)); // compass: one ccw step to Settings
    app.apply_gesture(obc_app::Gesture::Press); // → Settings list
    app.apply_gesture(obc_app::Gesture::Step(2)); // → Connections row (Ride, Display, Connections)
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
    app.apply_gesture(obc_app::Gesture::Step(1));
    assert!(app.passkey_card_up(), "a step does not dismiss the card");

    // Only the seam clearing the passkey closes it.
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.passkey_card_up());
}

#[test]
fn a_hold_charging_defers_the_card_until_the_hold_settles() {
    // A host-pushed screen must never land mid-hold — it would yank the hold target out from under
    // the rider. The board feeds the live hold progress of both hold buttons via
    // `set_hold_progress`, because `App`'s own recogniser sees nothing there.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));

    app.set_hold_progress(0.5, 0.0); // a hold is charging
    app.set_ble_status(pairing(1));
    assert!(!app.passkey_card_up(), "the card is deferred while a hold charges");

    // The desired state is re-fed every pass; once the hold settles the reconcile lands.
    app.set_hold_progress(0.0, 0.0);
    app.set_ble_status(pairing(1));
    assert!(app.passkey_card_up(), "the card opens once the hold settles");

    // Closing is deferred too: don't pop mid-hold.
    app.set_hold_progress(0.5, 0.0);
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(app.passkey_card_up(), "the card is held up while a hold charges");
    app.set_hold_progress(0.0, 0.0);
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(!app.passkey_card_up(), "the card closes once the hold settles");

    // Back charges on the same plane and defers the same way.
    app.set_hold_progress(0.0, 0.5);
    app.set_ble_status(pairing(1));
    assert!(!app.passkey_card_up(), "a charging Back hold defers the card too");
    app.set_hold_progress(0.0, 0.0);
    app.set_ble_status(pairing(1));
    assert!(app.passkey_card_up(), "…and the card lands once the Back hold settles");
}

#[test]
fn a_link_change_does_not_repaint_the_map_or_statistics() {
    // The Map / Statistics views deliberately omit the glyph, so a link change must not force their
    // expensive redraw. `App::new` boots straight onto [Home, Map].
    let mut app = App::new(AppState::new(0, 0, 0.05)); // base = Map
                                                       // Start a tracking session so the Map↔Statistics sibling ring exists (the Map's `back` swaps to
                                                       // Statistics only while tracking; without a ride it pops back to the Menu).
    crate::common::mount_store(&mut app);
    app.recorder.request(obc_app::RecorderIntent::Start);
    crate::common::quiet_pass(&mut app, 1);
    assert!(app.recording(), "the ride the sibling ring depends on is actually open");
    let _ = app.take_dirty();
    app.set_ble_status(connected());
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "a link change never redraws the Map");

    // Map → Statistics (`back`), then the same must hold.
    app.apply_gesture(obc_app::Gesture::Back);
    let _ = app.take_dirty();
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert_eq!(app.take_dirty(), Dirty::CLEAN, "a link change never redraws Statistics");
}

#[test]
fn forget_requires_its_exact_result_even_after_disconnect_and_allows_explicit_retry() {
    use obc_app::ble::{BondError, BondOutcome, BondStatus, ControllerClearance};
    use obc_app::device_core::{ExternalFacts, OutcomeSlots};
    let mut app = App::new_idle(AppState::new(0, 0, 1.0));
    app.state.device.ble_paired = true;
    app.state.ble_forget_requested = true;
    let first = crate::common::quiet_pass(&mut app, 1).effects.bond.take().unwrap();
    assert_eq!(app.state.bond_status, BondStatus::Pending);
    app.set_ble_status(BleStatus::DISCONNECTED);
    assert!(crate::common::quiet_pass(&mut app, 2).effects.bond.is_empty());
    assert_eq!(app.state.bond_status, BondStatus::Pending, "a disconnect proves no deletion");
    let mut outcomes = OutcomeSlots::new();
    let mut facts = ExternalFacts::NONE;
    let failed = BondOutcome::Failed { token: first.token(), error: BondError::StoreWriteFailed };
    outcomes.bond.try_put(failed).unwrap();
    crate::common::pass(&mut app, 3, &mut outcomes, &mut facts, None);
    assert_eq!(app.state.bond_status, BondStatus::Failed(BondError::StoreWriteFailed));
    assert!(crate::common::quiet_pass(&mut app, 4).effects.bond.is_empty(), "no automatic destructive retry");
    app.state.ble_forget_requested = true;
    let retry = crate::common::quiet_pass(&mut app, 5).effects.bond.take().unwrap();
    assert_ne!(first.token(), retry.token());
    outcomes
        .bond
        .try_put(BondOutcome::KeysRemoved { token: first.token(), controller: ControllerClearance::Confirmed })
        .unwrap();
    crate::common::pass(&mut app, 6, &mut outcomes, &mut facts, None);
    assert_eq!(app.state.bond_status, BondStatus::Pending, "a stale success cannot finish the retry");
    let partial = BondOutcome::KeysRemoved { token: retry.token(), controller: ControllerClearance::Unconfirmed };
    outcomes.bond.try_put(partial).unwrap();
    crate::common::pass(&mut app, 7, &mut outcomes, &mut facts, None);
    assert_eq!(app.state.bond_status, BondStatus::RestartRequired);
    outcomes
        .bond
        .try_put(BondOutcome::KeysRemoved { token: retry.token(), controller: ControllerClearance::Confirmed })
        .unwrap();
    crate::common::pass(&mut app, 8, &mut outcomes, &mut facts, None);
    assert_eq!(app.state.bond_status, BondStatus::RestartRequired, "terminal answers are one-shot");
    let mut restarted = App::new_idle(AppState::new(0, 0, 1.0));
    outcomes.bond.try_put(partial).unwrap();
    crate::common::pass(&mut restarted, 9, &mut outcomes, &mut facts, None);
    assert_eq!(restarted.state.bond_status, BondStatus::Idle, "reset does not inherit an operation");
}
