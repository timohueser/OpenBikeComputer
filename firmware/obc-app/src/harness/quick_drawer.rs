//! The **quick drawer over a real host** (#1515 D2): the chord plane, the drawer owner, and the
//! four controls, driven through raw button edges and DeviceCore passes rather than by calling
//! `handle` on a screen.
//!
//! Everything here is a property of the *composition* — that the squeeze reaches the app at all,
//! that the sheet lands over whatever the rider was on without popping it, that the BLE toggle
//! reaches the persistence handshake, that the brightness the host would drive follows the editor.
//! The drawer's own page logic is unit-tested beside it in `screen/quick_drawer.rs`.

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
    // The sheet slides down over ~220 ms; settle it so a following gesture is not eaten by the
    // animation.
    frames.idle(app, ms + 500);
    ms + 600
}

/// One gesture on the sheet at `ms`, straight to the map plane — the clock first (so the sheet's
/// page slide has settled), then the gesture. Returns a millis past the slide it may have started,
/// so calls chain. Raw edges are the chord's business, tested above and in `input.rs`; from here on
/// what matters is what the drawer does with a recognised gesture.
fn at(app: &mut App, ms: u32, g: crate::Gesture) -> u32 {
    app.advance_animations(InputClock(ms));
    app.apply_gesture(g);
    ms + 300
}

fn drawer_up(app: &App) -> bool {
    matches!(app.top_screen(), Screen::QuickDrawer(_))
}

/// The squeeze opens the sheet **over** the screen the rider was on — the base is still there, so
/// closing puts them back where they were without a navigation — and the same squeeze closes it.
#[test]
fn the_quick_chord_opens_the_sheet_over_the_base_and_closes_it_again() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
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

/// **The suppression set.** A genuinely blocking modal owns the device: no squeeze opens a sheet
/// over a pairing passkey, a running map transfer, or the terminal install card.
#[test]
fn a_blocking_modal_refuses_the_chord() {
    // The passkey card, host-pushed by the BLE seam.
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    app.set_ble_status(BleStatus { link: crate::BleLink::Advertising, passkey: Some(123_456), paired: false });
    quiet_pass(&mut app, 100);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the card is up");
    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(matches!(app.top_screen(), Screen::Passkey(_)), "the squeeze did not reach past it");

    // A map transfer in flight.
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    app.set_map_transfer(Some(MapTransfer::Receiving { received_kib: 10, total_kib: 100 }));
    quiet_pass(&mut app, 100);
    assert!(matches!(app.top_screen(), Screen::MapTransfer(_)));
    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(matches!(app.top_screen(), Screen::MapTransfer(_)), "bytes are landing — no sheet over that");

    // The terminal "Installing update" card, the last frame before the warm reset.
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    app.debug_request_dfu_install();
    for ms in [100, 200, 300, 400] {
        quiet_pass(&mut app, ms);
    }
    if matches!(app.top_screen(), Screen::DfuInstalling(_)) {
        chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
        assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "nothing opens over the install card");
    }
}

/// The **contextual** chord is recognised — so a Down+Back squeeze can never leak a step and a
/// Back onto the screen under it — and, until D3 declares content for it, does nothing at all.
#[test]
fn the_context_chord_is_swallowed_and_does_nothing() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    let depth = app.debug_stack_len();
    chord(&mut app, &mut f, Button::Down, Button::Back, 1_000);
    assert_eq!(app.debug_stack_len(), depth, "no sheet, and no Back-tap either");
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the squeeze changed nothing");
}

/// **Mutual exclusion**, at the one door: with the quick sheet up, the reserved squeezes do not
/// stack a second overlay on it.
#[test]
fn no_squeeze_stacks_a_second_sheet() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let depth = app.debug_stack_len();
    for (a, b) in [(Button::Up, Button::Down), (Button::Select, Button::Back), (Button::Down, Button::Back)] {
        chord(&mut app, &mut f, a, b, ms);
        assert_eq!(app.debug_stack_len(), depth, "{a:?}+{b:?} stacked something");
    }
}

/// The BLE icon flips the real radio row **and** reaches the persistence handshake — the same
/// before/after `==` a settings screen's edit arms, with no new path.
#[test]
fn the_ble_icon_toggle_reaches_the_settings_save() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    let mut f = Frames::new();
    assert!(app.settings().ble_enabled);

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let ms = at(&mut app, ms, Gesture::Step(1)); // brightness -> BLE
    let ms = at(&mut app, ms, Gesture::Press);
    assert!(!app.settings().ble_enabled, "the radio row flipped");
    assert!(!quiet_pass(&mut app, ms).effects.settings.is_empty(), "…and a persist is owed");
}

/// The brightness the host would drive follows the editor live, sticks on Select, and falls back
/// to the committed row on Back — the port's preview/commit/revert contract, seen from the app.
#[test]
fn the_driven_brightness_previews_commits_and_reverts() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
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

/// Power needs the completed hold: nothing the rider can *tap* asks the host to switch off.
#[test]
fn power_off_needs_the_completed_hold() {
    let mut app = App::new(AppState::new(0, 0, 1.0));
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

/// The settings icon **replaces** the sheet, so a Back out of central settings lands on the base
/// screen — not back inside a drawer the rider has finished with.
#[test]
fn central_settings_replaces_the_sheet_and_back_lands_on_the_base() {
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
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

/// The frame draws the base through the dim LUT while a sheet is up, and stops the instant it
/// closes — the two-`Canvas` split, seen as pixels rather than as a colour function.
#[test]
fn the_base_is_recessed_only_while_the_sheet_is_up() {
    use embedded_graphics::pixelcolor::Rgb888;

    let bytes = build_min_obcm(0x0000);
    let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map] over the flat blue backdrop
    let mut f = Frames::new();
    // The backdrop as the host's own colour policy renders it, and the same colour one device-64
    // level down — what `dim_color` turns it into.
    let plain_blue = Rgb888::new(0, 0, 255);
    let dim_blue = Rgb888::new(0, 0, 173);

    let before = render_120(&mut app, &bytes);
    assert!(before.count(plain_blue) > 0 && before.count(dim_blue) == 0, "the bare map is drawn at full colour");

    let ms = chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    let covered = render_120(&mut app, &bytes);
    assert_eq!(covered.count(plain_blue), 0, "no pixel of the base is drawn at full colour under a sheet");
    assert!(covered.count(dim_blue) > 0, "the map that is still visible around the sheet has receded");

    chord(&mut app, &mut f, Button::Up, Button::Select, ms);
    let after = render_120(&mut app, &bytes);
    assert_eq!(after.count(plain_blue), before.count(plain_blue), "closing restores the base exactly");
    assert_eq!(after.count(dim_blue), 0, "…and nothing stays recessed");
}
