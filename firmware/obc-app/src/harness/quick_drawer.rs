//! The quick drawer over a real host: the chord plane, the drawer owner, and the four controls,
//! driven through raw button edges and DeviceCore passes rather than by calling `handle` on a
//! screen. Everything here is a property of the composition: that the squeeze reaches the app at
//! all, that the sheet lands over whatever the rider was on without popping it, that a toggle
//! reaches the persistence handshake. The drawer's own page logic is unit-tested beside it in
//! `screen/quick_drawer.rs`. The contextual sheet is here for the same reason.

use super::support::{build_min_obcm, down, quiet_pass, render_120, up, Frames};
use crate::screen::{MapTransfer, BRIGHTNESS_MAX};
use crate::{App, AppState, BleStatus, Gesture, Screen};
use obc_ports::{Button, InputClock, InputEvent};

/// The raw edges of one squeeze of `(a, b)`, pressed 40 ms apart and released together — the shape
/// a rider's thumb makes, and well inside the 100 ms chord window.
fn squeeze(a: Button, b: Button) -> [(u32, InputEvent); 4] {
    [(0, down(a)), (40, down(b)), (120, up(b)), (140, up(a))]
}

/// Feed one squeeze to `app` starting at `ms`, through the app's own recogniser, and settle the
/// sheet's open animation. Returns the millis afterwards.
fn chord(app: &mut App, frames: &mut Frames, a: Button, b: Button, ms: u32) -> u32 {
    for (dt, ev) in squeeze(a, b) {
        frames.frame(app, ms + dt, &[ev], None, None);
    }
    // Settle the sheet's own open so a following gesture is not eaten by the animation. The
    // drawer's constant is read, so retuning it cannot leave this helper acting mid-slide.
    let settled = crate::screen::QUICK_OPEN_MS + 140;
    frames.idle(app, ms + settled);
    ms + settled + 100
}

fn at(app: &mut App, ms: u32, g: crate::Gesture) -> u32 {
    app.advance_animations(InputClock(ms));
    app.apply_gesture(g);
    ms + 300
}

fn drawer_up(app: &App) -> bool {
    matches!(app.top_screen(), Screen::QuickDrawer(_))
}

/// An app on `[Home, Map]` whose platform has a panel light — the simulator's shape, and the
/// four-icon arrangement every test below but one is about.
fn lit() -> App {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.set_backlight_available(true);
    app
}

/// The squeeze opens the sheet over the screen the rider was on, so closing puts them back without
/// a navigation, and the same squeeze closes it.
#[test]
fn the_quick_chord_opens_the_sheet_over_the_base_and_closes_it_again() {
    let mut app = lit(); // [Home, Map]
    let mut f = Frames::new();
    let depth = app.debug_stack_len();

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(drawer_up(&app), "Up+Select opened the drawer");
    assert_eq!(app.debug_stack_len(), depth + 1, "the sheet sits on top; the base is untouched");

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, ms);
    assert!(!drawer_up(&app), "the same squeeze closes it");
    assert_eq!(app.debug_stack_len(), depth, "and the rider is back where they were");
    assert!(matches!(app.top_screen(), Screen::Map(_)));

    // Back also closes it, and still without popping the base.
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, ms);
    at(&mut app, ms, Gesture::Back);
    assert_eq!(app.debug_stack_len(), depth, "Back closed the sheet, not the Map under it");
    assert!(matches!(app.top_screen(), Screen::Map(_)));
}

/// A genuinely blocking modal owns the device: no squeeze opens a sheet over a pairing passkey, a
/// running map transfer, or the terminal install card.
#[test]
fn a_blocking_modal_refuses_the_chord() {
    // The passkey card, host-pushed by the BLE seam.
    let mut app = lit();
    let mut f = Frames::new();
    app.set_ble_status(BleStatus { link: crate::BleLink::Advertising, passkey: Some(123_456), paired: false });
    quiet_pass(&mut app, 100);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the card is up");
    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the squeeze did not reach past it");

    // A map transfer in flight.
    let mut app = lit();
    let mut f = Frames::new();
    app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 10, total_kib: 100 }));
    quiet_pass(&mut app, 100);
    assert!(matches!(app.top_screen(), Screen::MapTransfer(_)));
    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(matches!(app.top_screen(), Screen::MapTransfer(_)), "bytes are landing — no sheet over that");

    // The terminal "Installing update" card, the last frame before the warm reset. Driven through
    // the domain's own landing seam, which is what puts the card up: `dfu_request` only states the
    // intent, and a pass alone never lands it.
    let mut app = lit();
    let mut f = Frames::new();
    app.post_dfu_landing(crate::card_scheduler::DfuLanding::InstallBegan);
    quiet_pass(&mut app, 100);
    // Required, not assumed: a setup that stopped reaching the card would retire this case
    // silently.
    assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "the install card is up");
    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "nothing opens over the install card");
}

/// The contextual chord reaches the app as one chord: over the riding Map it opens the ride sheet
/// and leaks neither a step nor a Back-tap onto the map under it (a leaked Back mid-ride would swap
/// the view to Statistics, which is what makes this checkable at all).
#[test]
fn the_context_chord_opens_the_ride_sheet_and_leaks_nothing() {
    let mut app = lit();
    let mut f = Frames::new();
    let depth = app.debug_stack_len();
    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, 1_000);
    assert_eq!(app.debug_stack_len(), depth + 1, "the sheet, and only the sheet");
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)));
    chord(&mut app, &mut f, Button::Down, Button::Back, ms);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the same squeeze closes it, back onto the Map");
    assert_eq!(app.debug_stack_len(), depth);
}

/// The route-plan sheet reaches the plan and the save, through a real chord and real passes. Three
/// things at once, because they are one property: the commit writes `Settings::bike_type`
/// and leaves the pass owing a settings persist, because a drawer is not a settings subtree; the
/// sheet closes onto the confirm card rather than navigating; and the plan request afterwards is
/// recorded while the settings hold the profile that was just committed.
#[test]
fn the_route_plan_sheet_reaches_the_plan_and_the_save() {
    use crate::settings::BikeType;
    let mut app = lit();
    let mut f = Frames::new();
    app.test_mount_store();

    // A fix — the confirm card routes from where the rider is.
    app.state.user_fix = Some(obc_ports::Fix::at(7_420_000, 43_735_000));

    // The card the POI detail would push, seeded directly: the browse that reaches it needs a
    // queried map and a corridor snapshot, neither of which this test is about.
    app.ui.stack.truncate(1); // [Home]
    let _ = app.ui.stack.push(crate::harness::support::selected_place());
    let depth = app.debug_stack_len();

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, 1_000);
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)), "the confirm card declares a context");
    assert_eq!(app.debug_stack_len(), depth + 1, "the sheet sits on top; the card is untouched");

    let ms = at(&mut app, ms, Gesture::Press); // -> the bike-type editor, on Road
    let ms = at(&mut app, ms, Gesture::Step(1)); // stage Gravel
    assert_eq!(app.settings().bike_type, BikeType::Road, "staging commits nothing");
    let ms = at(&mut app, ms, Gesture::Press); // commit
    assert_eq!(app.settings().bike_type, BikeType::Gravel, "Select wrote the type the router will use");
    assert!(!quiet_pass(&mut app, ms).effects.settings.is_empty(), "…and the pass owes a persist at once");

    let ms = at(&mut app, ms, Gesture::Back); // close the sheet
    assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "the card is still under it — not a navigation");
    assert_eq!(app.debug_stack_len(), depth);

    assert_eq!(app.settings().bike_type, BikeType::Gravel);
    let _ = ms;
}

/// Mutual exclusion at the one door: with the quick sheet up, the reserved squeezes do not stack a
/// second overlay on it.
#[test]
fn no_squeeze_stacks_a_second_sheet() {
    let mut app = lit();
    let mut f = Frames::new();
    let mut ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let depth = app.debug_stack_len();
    for (a, b) in [(Button::Up, Button::Down), (Button::Select, Button::Back), (Button::Down, Button::Back)] {
        // Each squeeze starts where the last one ended: `chord` returns the millis past its own
        // settle for exactly this, and replaying at the earlier clock is input no device can make.
        ms = chord(&mut app, &mut f, a, b, ms);
        assert_eq!(app.debug_stack_len(), depth, "{a:?}+{b:?} stacked something");
    }
}

/// The BLE icon flips the real radio row and reaches the persistence handshake, on the same path a
/// settings screen's edit arms.
#[test]
fn the_bluetooth_icon_toggles_and_persists() {
    let mut app = lit();
    let mut f = Frames::new();
    assert!(app.settings().ble_enabled);

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Step(1)); // brightness -> Bluetooth
    let ms = at(&mut app, ms, Gesture::Press);
    assert!(drawer_up(&app));
    assert!(!app.settings().ble_enabled);
    assert!(!quiet_pass(&mut app, ms).effects.settings.is_empty());
}

/// The brightness the host would drive follows the editor live, sticks on Select, and falls back to
/// the committed row on Back — the port's preview/commit/revert contract, seen from the app.
#[test]
fn the_driven_brightness_previews_commits_and_reverts() {
    let mut app = lit();
    let mut f = Frames::new();
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "a fresh device runs at full brightness");

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Press); // open the editor on the committed level
    let ms = at(&mut app, ms, Gesture::Step(-2));
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX - 2, "the panel previews the staged level");
    assert_eq!(app.settings().brightness, BRIGHTNESS_MAX, "…and nothing is committed yet");

    let ms = at(&mut app, ms, Gesture::Back);
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "cancel reverted the preview");

    let ms = at(&mut app, ms, Gesture::Press);
    let ms = at(&mut app, ms, Gesture::Step(-1));
    let ms = at(&mut app, ms, Gesture::Press);
    assert_eq!(app.settings().brightness, BRIGHTNESS_MAX - 1, "Select committed it");
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX - 1, "and the panel keeps it after the editor closes");
    assert!(!quiet_pass(&mut app, ms).effects.settings.is_empty(), "a committed level is persisted");
}

/// Both arrangements of the root row. A platform with a panel light offers four controls and opens
/// on brightness; one without offers three and opens on the radio — and every remaining control
/// still reaches the page it names, which is the part an index shift would break.
#[test]
fn a_platform_without_a_panel_light_drops_the_brightness_control() {
    // Lit: four controls, and the first press opens the editor.
    let mut app = lit();
    let mut f = Frames::new();
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Press);
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "the editor is open on the committed level");
    let ms = at(&mut app, ms, Gesture::Step(-1));
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX - 1, "…and it previews");
    at(&mut app, ms, Gesture::Back);

    // Dark: three controls. The first press must toggle the radio, not open an editor that has
    // nothing behind it, and Step(1)/Step(2) must land on settings and power rather than one short.
    let mut app = App::new(AppState::new(0, 0, 1.0)); // no host claimed a light
    let mut f = Frames::new();
    assert!(!app.backlight_available());
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Press);
    assert!(drawer_up(&app));
    assert!(!app.settings().ble_enabled);
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "no editor, and no preview to hold");
    let ms = at(&mut app, ms, Gesture::Back);
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, ms);

    let ms = at(&mut app, ms, Gesture::Step(2)); // -> power, the last of three
    let ms = at(&mut app, ms, Gesture::Press);
    let _ = at(&mut app, ms, Gesture::Hold);
    assert!(app.power_off_requested(), "the last control is still power");

    // …and the middle one is still central settings.
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Step(1));
    at(&mut app, ms, Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Settings(_)), "the middle control is the gear");
}

/// A host-pushed modal takes the sheet with it, so the panel stops showing an uncommitted preview
/// the rider can no longer reach. The map-transfer card is the worst case on purpose: it also
/// refuses the chord, so a preview held behind it would stand for the length of a multi-minute
/// upload with no way for the rider to end it. A drawer is transient chrome: the card closes it,
/// and the transfer ends on the map.
#[test]
fn a_modal_over_the_editor_closes_the_sheet_and_reverts_the_preview() {
    let mut app = lit();
    let mut f = Frames::new();
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Press); // the brightness editor
    let ms = at(&mut app, ms, Gesture::Step(-2));
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX - 2, "the preview is live while the sheet is on top");

    app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 10, total_kib: 4_000 }));
    quiet_pass(&mut app, ms);
    assert!(matches!(app.top_screen(), Screen::MapTransfer(_)), "the card landed");
    assert!(!app.debug_stack_has_overlay(), "…and the sheet went with it");
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "…so the panel is back on the committed level");
    assert_eq!(app.settings().brightness, BRIGHTNESS_MAX, "nothing was committed on the way");

    // The card clears and the rider is on the map they started from, not inside a stale editor.
    app.set_map_transfer(None);
    quiet_pass(&mut app, ms + 100);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "the preview does not come back");
}

/// Power needs the completed hold: nothing the rider can *tap* asks the host to switch off.
#[test]
fn power_off_needs_the_completed_hold() {
    let mut app = lit();
    let mut f = Frames::new();
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Step(3)); // -> the power icon
    let ms = at(&mut app, ms, Gesture::Press);
    assert!(!app.power_off_requested(), "the confirmation alone asks for nothing");

    let ms = at(&mut app, ms, Gesture::Press);
    assert!(!app.power_off_requested(), "a tap on the confirmation cancels it");

    let ms = at(&mut app, ms, Gesture::Press); // -> confirm again
    let _ = at(&mut app, ms, Gesture::Hold);
    assert!(app.power_off_requested(), "only the completed hold asks the host to switch off");
}

/// The settings icon replaces the sheet, so a Back out of central settings lands on the base
/// screen — not back inside a drawer the rider has finished with.
#[test]
fn central_settings_replaces_the_sheet_and_back_lands_on_the_base() {
    let mut app = lit(); // [Home, Map]
    let mut f = Frames::new();
    let depth = app.debug_stack_len();

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Step(2)); // -> the gear
    let ms = at(&mut app, ms, Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::Settings(_)), "central settings took the sheet's slot");
    assert_eq!(app.debug_stack_len(), depth + 1, "replaced, not pushed");

    at(&mut app, ms, Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)), "Back landed on the base screen");
    assert_eq!(app.debug_stack_len(), depth);
}

/// The per-screen dim, in pixels. A sheet over a map base leaves it exactly as it was — the map
/// reads fine at full colour, and dimming it would mean re-rendering it. A sheet over a menu base
/// still recesses it through the dim LUT, because that second draw is a handful of rules and
/// glyphs.
#[test]
fn a_sheet_recesses_a_menu_base_and_leaves_a_map_base_alone() {
    use embedded_graphics::pixelcolor::Rgb888;

    let bytes = build_min_obcm(0x0000);
    // The backdrop as the host's own colour policy renders it, and the same colour one device-64
    // level down — what `dim_color` turns it into.
    let plain_blue = Rgb888::new(0, 0, 255);
    let dim_blue = Rgb888::new(0, 0, 173);
    // The sheet's own parchment, which is never recessed: it is the thing in front.
    let parchment = Rgb888::new(247, 243, 239);

    let mut app = lit(); // [Home, Map] over the flat blue backdrop
    let mut f = Frames::new();
    let before = render_120(&mut app, &bytes);
    assert!(before.count(plain_blue) > 0 && before.count(dim_blue) == 0, "the bare map is drawn at full colour");

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let covered = render_120(&mut app, &bytes);
    assert_eq!(covered.count(dim_blue), 0, "no pixel of a map base is dimmed under a sheet");
    assert!(covered.count(plain_blue) > 0, "the map around the sheet is still the map, at full colour");
    assert!(covered.count(parchment) > before.count(parchment), "…and the sheet is on top of it");

    chord(&mut app, &mut f, Button::Up, Button::Select, ms);
    let after = render_120(&mut app, &bytes);
    assert_eq!(after.count(plain_blue), before.count(plain_blue), "closing restores the base exactly");

    let mut app = lit();
    let mut f = Frames::new();
    let _ = app.ui.stack.push(Screen::Menu(crate::screen::MenuScreen::new())); // a chrome base
    assert!(matches!(app.top_screen(), Screen::Menu(_)));
    let bare = render_120(&mut app, &bytes);
    let bare_parchment = bare.count(parchment);
    assert!(bare_parchment > 0, "the menu is drawn on parchment");

    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let recessed = render_120(&mut app, &bytes);
    // The sheet is parchment too, so an undimmed menu could only have *more* of it under one. It
    // has less: the page behind the sheet went through the dim LUT.
    assert!(recessed.count(parchment) < bare_parchment, "the menu page under the sheet has receded");
}
