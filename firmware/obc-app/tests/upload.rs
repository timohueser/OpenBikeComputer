//! The route-upload popups (epic #447, P4): [`App::notify_route_uploaded`] and the locked popup
//! rules — the three variants (idle "ROUTE RECEIVED" / tracking swap prompt / active-replaced
//! info card), replace-not-stack on consecutive uploads (by object id, selection reset), the 30 s
//! auto-close = dismiss, passkey priority in both directions, hold deferral, deleted-route
//! validation, and the forced-adoption invalidation of stale match state on an active replace.

mod common;

use common::{down, keys, up};
use obc_app::screen::UPLOAD_POPUP_TIMEOUT_MS;
use obc_app::{
    App, AppState, BleLink, BleStatus, Button, Gesture, IdleReturn, InputClock, Mode, RouteSummary, Screen, Settings,
};
use obc_reader::BBox;

/// A three-route catalog with deliberately non-positional durable ids (10 / 11 / 12), so any test
/// passing an *id* where an *index* is expected fails loudly.
fn ids() -> [u16; 3] {
    [10, 11, 12]
}

fn routes() -> [RouteSummary; 3] {
    let mk = |n: &str, d: u32, c: u32| {
        let mut name = heapless::String::<48>::new();
        let _ = name.push_str(n);
        RouteSummary {
            name,
            distance_km: d,
            climb_m: c,
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 1000, max_lat: 1000 },
            start_lon: 100,
            start_lat: 100,
        }
    };
    [mk("Alpha", 10, 100), mk("Beta", 20, 200), mk("Gamma", 30, 300)]
}

/// An idle app (Home root) with the id-carrying catalog loaded and the boot paint drained. The
/// idle-return timeout is disabled so these popup-timing tests (which advance the clock past the
/// 30 s popup deadline — the same span as the default idle return) isolate the popup auto-close.
fn idle_app() -> App {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.set_settings(Settings { idle_return: IdleReturn::Never, ..Settings::default() });
    app.set_routes_with_ids(&routes(), &ids());
    let _ = app.take_dirty();
    app
}

/// Start riding catalog route 0 (id 10) through the real navigation: Home `press` → Menu, `press`
/// (Routes) → Route menu, `press` → Route overview, `press` → START RIDE. Leaves the stack at
/// `[Home, Map]`, tracking.
fn start_riding(app: &mut App) {
    app.apply_gesture(Gesture::Press); // Home → Menu (Routes selected)
    app.apply_gesture(Gesture::Press); // press Routes → Route menu
    app.apply_gesture(Gesture::Press); // → Route overview
    app.apply_gesture(Gesture::Press); // → START RIDE → Map
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the ride opens on the Map");
    assert!(app.activity.is_tracking(), "START RIDE begins a tracking session");
    assert_eq!(app.activity.active_route, Some(0));
    let _ = app.take_dirty();
}

// --- variant 1: idle → "ROUTE RECEIVED", Start navigation / Dismiss ---------------------------

#[test]
fn idle_upload_opens_the_prompt_and_start_navigation_rides_it() {
    let mut app = idle_app();
    app.notify_route_uploaded(11, false); // id 11 = catalog index 1 ("Beta")
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)), "idle upload → ROUTE RECEIVED");
    assert!(app.take_dirty().map, "the popup covers the screen below — one repaint");

    // Start navigation (row 0) = exactly the Route-menu start path: riding view, Mode::Riding,
    // active_route by id, a fresh session, a clean [Home, Map] stack.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "Start navigation lands on the riding Map");
    assert_eq!(app.mode(), Mode::Riding);
    assert_eq!(app.activity.active_route, Some(1), "navigates the *uploaded* route, resolved by id");
    assert!(app.activity.is_tracking(), "starting from Idle begins a session, like the overview's START");
    assert!(app.state.heading_up, "the riding view is heading-up");
}

#[test]
fn idle_prompt_dismisses_on_back_and_on_the_dismiss_row() {
    // Back = dismiss; nothing is lost (the route stays in the catalog / menu).
    let mut app = idle_app();
    app.notify_route_uploaded(11, false);
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Home(_)), "Back dismisses to what was underneath");
    assert_eq!(app.mode(), Mode::Idle, "dismissing navigates nothing");

    // The Dismiss row (turn down, press) does the same.
    app.notify_route_uploaded(11, false);
    app.apply_gesture(Gesture::Turn(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Home(_)));
    assert_eq!(app.mode(), Mode::Idle);
    assert_eq!(app.activity.active_route, None, "still nothing loaded — the prompt was advisory");
}

// --- variant 2: tracking → the retitled Route-swap prompt --------------------------------------

#[test]
fn tracking_upload_opens_the_swap_prompt_and_swap_keeps_the_session() {
    let mut app = idle_app();
    start_riding(&mut app);
    let session = app.activity.session;

    app.notify_route_uploaded(12, false); // id 12 = index 2 ("Gamma") arrives mid-ride
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "tracking upload → the swap prompt");

    // "Swap route" (row 0, press): re-navigate onto the received route, session untouched.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.activity.active_route, Some(2), "navigation swapped onto the uploaded route");
    assert_eq!(app.activity.session, session, "the recording session survives a swap");
}

#[test]
fn tracking_swap_prompt_cancel_keeps_the_current_route() {
    let mut app = idle_app();
    start_riding(&mut app);
    app.notify_route_uploaded(12, false);
    app.apply_gesture(Gesture::Back); // Back = Cancel
    assert!(matches!(app.top_screen(), Screen::Map(_)), "cancel returns to the ride");
    assert_eq!(app.activity.active_route, Some(0), "still navigating the original route");
}

// --- variant 3: replacing the actively-navigated route -----------------------------------------

#[test]
fn active_replace_shows_the_info_card_and_drops_stale_match_state() {
    let mut app = idle_app();
    start_riding(&mut app); // riding index 0 = id 10
    let session = app.activity.session;
    // Simulate an established match on the *old* geometry.
    app.activity.progress_m = 4_321;
    app.activity.off_route = true;
    app.activity.dist_to_route_m = 55;

    app.notify_route_uploaded(10, true); // the navigated id re-uploaded — bytes swapped
    assert!(matches!(app.top_screen(), Screen::RouteUpdated(_)), "active replace → the info-only card");
    // Forced adoption: everything derived from the old bytes is dropped — the matcher re-runs
    // from the current fix, the readouts clear until recomputed.
    assert_eq!(app.activity.progress_m, 0, "stale progress over the old geometry is dropped");
    assert!(!app.activity.off_route, "the stale off-route verdict is dropped");
    assert_eq!(app.activity.dist_to_route_m, 0);
    assert_eq!(app.activity.active_route, Some(0), "still navigating the same route (same id)");
    assert_eq!(app.activity.session, session, "the recording session is untouched");
    assert!(app.take_dirty().map, "the route line changed under the rider — repaint");

    // Info-only: press dismisses, nothing else moves.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.activity.active_route, Some(0));
}

#[test]
fn active_replace_adoption_happens_even_when_the_prompt_is_suppressed() {
    // The passkey card is up → the popup is dropped. The adoption must land anyway.
    let mut app = idle_app();
    start_riding(&mut app);
    app.activity.progress_m = 999;
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: Some(42), paired: false });
    assert!(app.passkey_card_up());

    app.notify_route_uploaded(10, true);
    assert_eq!(app.activity.progress_m, 0, "adoption is not optional — stale state drops regardless");
    assert!(app.passkey_card_up(), "the card stays; no popup landed under or over it");
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: None, paired: false });
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the suppressed prompt was dropped, not queued");
}

#[test]
fn replace_of_a_non_active_route_is_not_the_info_card() {
    // A replace of a route we are *not* navigating is an ordinary arrival: swap prompt while
    // tracking (nothing to invalidate — no cached state exists for a non-active route).
    let mut app = idle_app();
    start_riding(&mut app); // riding id 10
    app.activity.progress_m = 777;
    app.notify_route_uploaded(11, true);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "non-active replace → the swap prompt");
    assert_eq!(app.activity.progress_m, 777, "the active route's match state is untouched");
}

// --- replace-the-popup: consecutive uploads, and the manual swap prompt ------------------------

#[test]
fn consecutive_uploads_replace_the_popup_most_recent_wins() {
    let mut app = idle_app();
    app.notify_route_uploaded(10, false);
    app.apply_gesture(Gesture::Turn(1)); // move the highlight to Dismiss…
    app.notify_route_uploaded(11, false); // …then a newer upload lands
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)));

    // Selection reset with the fresh screen: press fires *Start navigation* (row 0 again), and it
    // rides the newest route — the popup was replaced, not stacked, and re-targeted by id.
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.activity.active_route, Some(1), "most recent upload wins (id 11 = index 1)");
}

#[test]
fn a_second_upload_does_not_stack_a_second_popup() {
    let mut app = idle_app();
    app.notify_route_uploaded(10, false);
    app.notify_route_uploaded(11, false);
    app.apply_gesture(Gesture::Back); // dismiss the (single) popup
    assert!(matches!(app.top_screen(), Screen::Home(_)), "one dismiss clears it — nothing was stacked");
}

#[test]
fn an_upload_replaces_the_manual_swap_prompt_too() {
    let mut app = idle_app();
    start_riding(&mut app);
    // Open the *manual* swap prompt from the menu: Map → Menu (BackHold) → Routes (press) →
    // highlight route 1 (turn) → press (tracking + different route ⇒ the swap prompt).
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Turn(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "the manual swap prompt is up");

    // An upload lands: the incoming popup replaces the manual prompt in place (same rule).
    app.notify_route_uploaded(12, false);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "still the swap shape, now the received one");

    // Proof it's the received popup, not the manual prompt: it auto-closes (a manual prompt
    // waits for the rider forever), revealing the Route menu it replaced the prompt over.
    app.advance_animations(InputClock(UPLOAD_POPUP_TIMEOUT_MS + 1));
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)), "auto-close reveals the screen underneath");
}

// --- the 30 s auto-close = dismiss --------------------------------------------------------------

#[test]
fn auto_close_timeout_is_a_dismiss() {
    let mut app = idle_app();
    app.advance_animations(InputClock(1_000)); // pin the popup's open anchor
    app.notify_route_uploaded(11, false);
    let _ = app.take_dirty();

    app.advance_animations(InputClock(1_000 + UPLOAD_POPUP_TIMEOUT_MS - 1));
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)), "still up just inside the window");
    assert!(!app.take_dirty().map, "nothing repaints while the popup just sits there");

    app.advance_animations(InputClock(1_000 + UPLOAD_POPUP_TIMEOUT_MS));
    assert!(matches!(app.top_screen(), Screen::Home(_)), "the deadline dismisses the popup");
    assert_eq!(app.mode(), Mode::Idle, "timeout = dismiss: nothing was navigated");
    assert!(app.take_dirty().map, "closing repaints what the popup covered");
}

#[test]
fn the_popup_arms_a_timed_wake_for_its_deadline() {
    // The event-driven host must be *woken* for the auto-close — the popup reports its residual
    // deadline through the shared tick machinery, no new timer path.
    let mut app = idle_app();
    app.advance_animations(InputClock(2_000));
    app.notify_route_uploaded(11, false);
    app.advance_animations(InputClock(2_000)); // same-frame re-poll now that the popup is up
    assert_eq!(app.ms_until_next_wake(2_000), Some(UPLOAD_POPUP_TIMEOUT_MS), "the whole window remains");
    app.advance_animations(InputClock(2_000 + 10_000));
    assert_eq!(app.ms_until_next_wake(12_000), Some(UPLOAD_POPUP_TIMEOUT_MS - 10_000), "10 s in, 20 s left");
}

// --- passkey priority, both directions ----------------------------------------------------------

#[test]
fn a_prompt_is_dropped_not_queued_while_the_passkey_card_shows() {
    let mut app = idle_app();
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: Some(123), paired: false });
    assert!(app.passkey_card_up());

    app.notify_route_uploaded(11, false);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the card outranks — no popup lands");

    // Pairing ends: the card closes and the suppressed prompt must NOT surface (dropped, the
    // route is in the menu anyway).
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: None, paired: false });
    app.advance_animations(InputClock(500));
    assert!(matches!(app.top_screen(), Screen::Home(_)), "the dropped prompt never resurfaces");
}

#[test]
fn a_passkey_replaces_an_open_route_popup() {
    let mut app = idle_app();
    app.notify_route_uploaded(11, false);
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)));

    // Pairing starts: the card outranks — it replaces the popup rather than stacking over it.
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: Some(77), paired: false });
    assert!(matches!(app.top_screen(), Screen::Passkey(_)));
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: None, paired: false });
    assert!(matches!(app.top_screen(), Screen::Home(_)), "closing the card reveals Home, not the popup");
}

#[test]
fn a_passkey_does_not_remove_the_manual_swap_prompt() {
    // The rider opened the manual swap prompt themselves — it is not an advisory popup, so the
    // card composites over it and returns to it after pairing.
    let mut app = idle_app();
    start_riding(&mut app);
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Turn(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)));

    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: Some(9), paired: false });
    assert!(matches!(app.top_screen(), Screen::Passkey(_)));
    app.set_ble_status(BleStatus { link: BleLink::Connected, passkey: None, paired: false });
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)), "the manual prompt is restored, not dropped");
}

// --- hold deferral -------------------------------------------------------------------------------

#[test]
fn a_charging_hold_defers_the_prompt_a_tick() {
    let mut app = idle_app();
    app.set_hold_progress(0.5); // a hold is charging (the two-plane firmware's live feed)
    app.notify_route_uploaded(11, false);
    assert!(matches!(app.top_screen(), Screen::Home(_)), "no host-pushed screen lands mid-hold");

    // The hold settles; the next pass delivers the pending prompt.
    app.set_hold_progress(0.0);
    app.advance_animations(InputClock(100));
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)), "the deferred prompt lands");
}

#[test]
fn a_charging_hold_defers_the_auto_close_too() {
    let mut app = idle_app();
    app.advance_animations(InputClock(1_000));
    app.notify_route_uploaded(11, false);

    // The deadline passes mid-hold: never pop a screen out from under a charging hold.
    app.set_hold_progress(0.7);
    app.advance_animations(InputClock(1_000 + UPLOAD_POPUP_TIMEOUT_MS + 5));
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)), "the close is deferred while charging");

    app.set_hold_progress(0.0);
    app.advance_animations(InputClock(1_000 + UPLOAD_POPUP_TIMEOUT_MS + 60));
    assert!(matches!(app.top_screen(), Screen::Home(_)), "…and lands once the hold settles");
}

// --- deleted-route validation --------------------------------------------------------------------

#[test]
fn a_route_deleted_under_the_popup_dismisses_on_action() {
    let mut app = idle_app();
    app.notify_route_uploaded(11, false);
    assert!(matches!(app.top_screen(), Screen::RouteReceived(_)));

    // The phone deletes the route while the popup is open: the rescan removes id 11 and the
    // remap leaves the popup targeting nothing.
    let r = routes();
    app.set_routes_with_ids(&[r[0].clone(), r[2].clone()], &[10, 12]);

    // Acting on it validates and self-dismisses instead of riding whatever slid into the index.
    app.apply_gesture(Gesture::Press); // Start navigation
    assert!(matches!(app.top_screen(), Screen::Home(_)), "the vanished route dismisses the popup");
    assert_eq!(app.mode(), Mode::Idle);
    assert_eq!(app.activity.active_route, None, "nothing was navigated");
}

#[test]
fn a_pending_deferred_prompt_for_a_deleted_route_is_dropped() {
    // Arrival is deferred by a hold; the route is deleted before delivery. The id no longer
    // resolves in the catalog, so the prompt is dropped entirely.
    let mut app = idle_app();
    app.set_hold_progress(0.5);
    app.notify_route_uploaded(11, false);
    let r = routes();
    app.set_routes_with_ids(&[r[0].clone(), r[2].clone()], &[10, 12]);

    app.set_hold_progress(0.0);
    app.advance_animations(InputClock(100));
    assert!(matches!(app.top_screen(), Screen::Home(_)), "a prompt for a vanished id never lands");
}

// --- stray holds across a popup dismissal (the #480 vanishing-routes delete) --------------------

/// The user-reported loss path: mid-ride, an upload popup covers the Route menu; the rider starts
/// a hold on the popup (aiming at the hold-guarded "Save & new"), thinks better of it and taps
/// **Back while the encoder is still held**. Back pops the popup — and the encoder hold then
/// crosses its threshold with the Route menu as the new top, whose own hold is hold-to-**delete**:
/// without the transition-cancels-holds rule, the highlighted route is silently deleted from SD.
#[test]
fn a_hold_charging_when_back_dismisses_the_popup_cannot_delete_a_route() {
    let mut app = idle_app();
    start_riding(&mut app); // tracking id 10 (index 0), stack [Home, Map]

    // Open the Route menu over the ride and highlight a non-active route (row 1, "Beta") — the
    // hold-to-delete footer is live there (only the active-ride route's is greyed).
    app.apply_gesture(Gesture::BackHold); // Menu
    app.apply_gesture(Gesture::Press); // Routes
    app.apply_gesture(Gesture::Turn(1)); // highlight index 1
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)));

    // A route lands mid-ride: the swap popup covers the menu.
    app.notify_route_uploaded(12, false);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)));

    // Encoder down (the hold starts charging on the popup)…
    app.handle_input(InputClock(1_000), &mut keys(&[down(Button::Encoder)]));
    // …then a Back tap while the encoder is still held: the popup pops, the Route menu is top.
    app.handle_input(InputClock(1_100), &mut keys(&[down(Button::Back)]));
    app.handle_input(InputClock(1_180), &mut keys(&[up(Button::Back)]));
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)), "Back dismissed the popup");

    // The encoder hold crosses its 500 ms threshold — over the Route menu now. It was aimed at
    // the popup, so it must be cancelled by the transition, not delivered as a delete.
    app.handle_input(InputClock(1_700), &mut keys(&[]));
    assert_eq!(app.take_route_delete(), None, "a stray hold must never delete the highlighted route");
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)));

    // And the eventual release stays silent — no surprise Press either.
    app.handle_input(InputClock(1_800), &mut keys(&[up(Button::Encoder)]));
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)));

    // A fresh, deliberate hold afterwards still deletes (the cancel is one-shot, not a lockout).
    app.handle_input(InputClock(2_000), &mut keys(&[down(Button::Encoder)]));
    app.handle_input(InputClock(2_600), &mut keys(&[]));
    assert_eq!(app.take_route_delete(), Some(11), "a real hold on the menu still requests its delete");
}

/// The same rule holds within one recognition batch: a `Hold` recognised *behind* the
/// stack-changing gesture (both queued before the app saw either) is dropped, not delivered to
/// the screen that replaced its target.
#[test]
fn a_hold_queued_behind_the_dismissing_back_in_one_batch_is_dropped() {
    let mut app = idle_app();
    start_riding(&mut app);
    app.apply_gesture(Gesture::BackHold);
    app.apply_gesture(Gesture::Press);
    app.apply_gesture(Gesture::Turn(1));
    app.notify_route_uploaded(12, false);
    assert!(matches!(app.top_screen(), Screen::RouteSwap(_)));

    // One long-unpolled frame delivers everything at once: the encoder went down at 1 000; at
    // 1 700 a Back tap arrives *and* the encoder hold has crossed its threshold. Recognition
    // emits [Back, Hold] into one batch; the Back's pop must swallow the trailing Hold.
    app.handle_input(InputClock(1_000), &mut keys(&[down(Button::Encoder)]));
    app.handle_input(InputClock(1_700), &mut keys(&[down(Button::Back), up(Button::Back)]));
    assert!(matches!(app.top_screen(), Screen::RouteMenu(_)), "Back dismissed the popup");
    assert_eq!(app.take_route_delete(), None, "the batched stray hold is dropped too");
}

/// The two-plane firmware's surface for the same rule: a gesture that changes the screen stack
/// raises the one-shot [`App::take_hold_cancel`] edge (its input plane charges holds out of the
/// app's sight, so the board must be told to cancel them).
#[test]
fn a_stack_transition_raises_the_hold_cancel_edge_for_the_two_plane_host() {
    let mut app = idle_app();
    assert!(!app.take_hold_cancel(), "quiet at boot");
    app.apply_gesture(Gesture::Press); // Home → Menu: a stack change
    assert!(app.take_hold_cancel(), "the transition rings the cancel edge");
    assert!(!app.take_hold_cancel(), "one-shot: drained");
    app.apply_gesture(Gesture::Turn(1)); // a highlight move transitions nothing
    assert!(!app.take_hold_cancel(), "no stack change, no cancel");
}

#[test]
fn an_upload_for_an_unknown_id_is_dropped() {
    // Defensive: the ordering contract says the rescan lands first, so an unknown id means the
    // route vanished again already — advisory, drop it.
    let mut app = idle_app();
    app.notify_route_uploaded(99, false);
    assert!(matches!(app.top_screen(), Screen::Home(_)), "no popup for an id the catalog doesn't hold");
}
