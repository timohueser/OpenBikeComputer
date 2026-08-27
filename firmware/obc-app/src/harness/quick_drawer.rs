//! The **quick drawer over a real host** (#1515 D2): the chord plane, the drawer owner, and the
//! four controls, driven through raw button edges and DeviceCore passes rather than by calling
//! `handle` on a screen.
//!
//! Everything here is a property of the *composition* — that the squeeze reaches the app at all,
//! that the sheet lands over whatever the rider was on without popping it, that the BLE toggle
//! reaches the persistence handshake, that the brightness the host would drive follows the editor.
//! The drawer's own page logic is unit-tested beside it in `screen/quick_drawer.rs`.
//!
//! The **contextual** sheet is here for the same reason, and D4a's nested value editor with it: a
//! commit that reaches the persistence handshake is a property of the App, not of the sheet.

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
    // Settle the sheet's own open so a following gesture is not eaten by the animation — read off
    // the drawer's constant, so retuning it cannot leave this helper acting mid-slide.
    let settled = crate::screen::QUICK_OPEN_MS + 60;
    frames.idle(app, ms + settled);
    ms + settled + 100
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

/// An app on `[Home, Map]` whose platform **has** a panel light — the simulator's shape, and the
/// four-icon arrangement every test below but one is about.
fn lit() -> App {
    let mut app = App::new(AppState::new(0, 0, 1.0));
    app.set_backlight_available(true);
    app
}

/// The squeeze opens the sheet **over** the screen the rider was on — the base is still there, so
/// closing puts them back where they were without a navigation — and the same squeeze closes it.
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

/// **The suppression set.** A genuinely blocking modal owns the device: no squeeze opens a sheet
/// over a pairing passkey, a running map transfer, or the terminal install card.
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
    // the domain's own landing seam, which is what actually puts the card up — `dfu_request` only
    // states the intent, and a pass alone never lands it.
    let mut app = lit();
    let mut f = Frames::new();
    app.post_dfu_landing(crate::card_scheduler::DfuLanding::InstallBegan);
    quiet_pass(&mut app, 100);
    // Required, not assumed: a setup that stops reaching the card would otherwise retire this case
    // silently, and the case is the whole point of the third `blocking()` row.
    assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "the install card is up");
    chord(&mut app, &mut f, Button::Up, Button::Select, 1_000);
    assert!(matches!(app.top_screen(), Screen::DfuInstalling(_)), "nothing opens over the install card");
}

/// The **contextual** chord reaches the app as one chord: over the riding Map it opens the ride
/// sheet and leaks neither a step nor a Back-tap onto the map under it (a leaked Back mid-ride
/// would have swapped the view to Statistics, which is what makes this checkable at all).
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

/// The Up-ahead sheet's **Sources** commit is the migrated Ride-settings row, and it reaches the
/// same persistence handshake that row did — the App's one before/after `==` over `Settings`, with
/// no new path (#1515 D4a). The sheet stays up afterwards: a commit returns to the row table, it
/// does not close the drawer.
#[test]
fn the_up_ahead_sources_commit_reaches_the_settings_save() {
    use crate::settings::UpAheadSource;
    let mut app = lit();
    let mut f = Frames::new();
    app.test_mount_store();
    app.test_start_ride();
    assert_eq!(app.settings().up_ahead_source, UpAheadSource::Both, "the factory scope");

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, 1_000); // the ride sheet
    let ms = at(&mut app, ms, Gesture::Press); // its first row -> the timeline
    assert!(matches!(app.top_screen(), Screen::UpAhead(_)));

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, ms); // the timeline's own sheet
    let ms = at(&mut app, ms, Gesture::Step(1)); // Filter -> Sources
    let ms = at(&mut app, ms, Gesture::Press); // -> the Sources editor
    let ms = at(&mut app, ms, Gesture::Step(2)); // stage Map POIs only
    assert_eq!(app.settings().up_ahead_source, UpAheadSource::Both, "staging commits nothing");

    let ms = at(&mut app, ms, Gesture::Press);
    assert_eq!(app.settings().up_ahead_source, UpAheadSource::MapPoisOnly, "Select wrote the row");
    assert!(matches!(app.top_screen(), Screen::ContextDrawer(_)), "…and the sheet is still up");
    assert!(!quiet_pass(&mut app, ms).effects.settings.is_empty(), "…and a persist is owed");

    // The scope the commit wrote is the scope the list under the sheet now reads: Map-POIs-only
    // still arms the corridor query, and Waypoints-only would not — the U4 rule, live.
    let ms = at(&mut app, ms, Gesture::Back); // close the sheet, back onto the timeline
    assert!(matches!(app.top_screen(), Screen::UpAhead(_)));
    assert!(app.corridor_snapshot_pending(), "Map POIs only still wants a snapshot");

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, ms);
    let ms = at(&mut app, ms, Gesture::Step(1));
    let ms = at(&mut app, ms, Gesture::Press);
    let ms = at(&mut app, ms, Gesture::Step(-1)); // Map POIs only -> Waypoints only
    let ms = at(&mut app, ms, Gesture::Press);
    at(&mut app, ms, Gesture::Back);
    assert_eq!(app.settings().up_ahead_source, UpAheadSource::WaypointsOnly);
    assert!(!app.corridor_snapshot_pending(), "…and Waypoints only disarms it, from the sheet");
}

/// The **Filter** commit is the other half, and it is *not* a settings field: it reaches the
/// timeline's live scope, which re-keys the corridor snapshot — the thing the Hold picker used to do
/// from inside the list. Back out of the editor discards instead.
#[test]
fn the_up_ahead_filter_commit_re_keys_the_snapshot_and_back_discards() {
    use obc_reader::{PoiCategory, PoiCategorySet};
    let mut app = lit();
    let mut f = Frames::new();
    app.test_mount_store();
    app.test_start_ride();

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, 1_000);
    let ms = at(&mut app, ms, Gesture::Press); // -> the timeline, which opens on Everything
    assert_eq!(app.state.up_ahead_filter, PoiCategorySet::ALL);

    // Staged, then discarded: the list is still unfiltered.
    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, ms);
    let ms = at(&mut app, ms, Gesture::Press); // -> the Filter editor
    let ms = at(&mut app, ms, Gesture::Step(1));
    let ms = at(&mut app, ms, Gesture::Back);
    assert_eq!(app.state.up_ahead_filter, PoiCategorySet::ALL, "Back discarded the staged choice");

    // Staged, then committed: the list is filtered, and the sheet's own row reads it back.
    let ms = at(&mut app, ms, Gesture::Press);
    let ms = at(&mut app, ms, Gesture::Step(1));
    let ms = at(&mut app, ms, Gesture::Press);
    assert_eq!(app.state.up_ahead_filter, PoiCategorySet::only(PoiCategory::Water));
    at(&mut app, ms, Gesture::Back); // close the sheet
    assert!(matches!(app.top_screen(), Screen::UpAhead(_)));

    // Leaving and re-entering the timeline resets it: the list opens on Everything, every time.
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Map(_)));
    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, 20_000);
    at(&mut app, ms, Gesture::Press);
    assert_eq!(app.state.up_ahead_filter, PoiCategorySet::ALL, "predictable beats sticky (epic #946, U3)");
}

/// The same property end to end, through the real sheet: **scroll the timeline, filter it, and the
/// rider lands on the nearest match ahead.** The Select-hold picker used to clear the cursor itself
/// when it applied; the sheet sits above the screen and cannot, so the cursor states which list it
/// indexes into and re-homes when that list changes.
#[test]
fn filtering_a_scrolled_timeline_from_the_sheet_re_homes_the_cursor() {
    use obc_reader::{PoiCategory, PoiCategorySet};

    let mut app = lit();
    let mut f = Frames::new();
    app.test_mount_store();
    app.test_start_ride();
    app.activity.active_route = Some(0);
    // Four stops ahead, two of them Water and 3.6 km apart, so clamping and re-homing differ.
    app.ride.waypoints = crate::harness::support::wpts_detailed(&[
        (400, "Brunnen", Some(PoiCategory::Water), 0),
        (500, "Bakery", Some(PoiCategory::Resupply), 0),
        (600, "Camp", Some(PoiCategory::Campsite), 0),
        (4_000, "Far water", Some(PoiCategory::Water), 0),
    ]);

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, 1_000);
    let mut ms = at(&mut app, ms, Gesture::Press); // -> the timeline
    assert!(matches!(app.top_screen(), Screen::UpAhead(_)));

    for _ in 0..3 {
        ms = at(&mut app, ms, Gesture::Step(1)); // scroll to the last row
    }
    assert_eq!(cursor_row(&app), 3, "the rider is on \"Far water\"");

    let ms = chord(&mut app, &mut f, Button::Down, Button::Back, ms); // the timeline's own sheet
    let ms = at(&mut app, ms, Gesture::Press); // -> the Filter editor
    let ms = at(&mut app, ms, Gesture::Step(1)); // stage Water
    let ms = at(&mut app, ms, Gesture::Press); // commit
    assert_eq!(app.state.up_ahead_filter, PoiCategorySet::only(PoiCategory::Water));
    at(&mut app, ms, Gesture::Back); // close the sheet, back onto the timeline

    assert!(matches!(app.top_screen(), Screen::UpAhead(_)));
    assert_eq!(cursor_row(&app), 0, "re-homed onto the nearest Water stop, not clamped onto the far one");

    /// The row the timeline would draw the amber cursor on, resolved the way `draw` resolves it.
    fn cursor_row(app: &App) -> usize {
        let Some(Screen::UpAhead(s)) = app.ui.stack.iter().find(|s| matches!(s, Screen::UpAhead(_))) else {
            panic!("the timeline is not on the stack")
        };
        s.test_cursor(
            app.ride.waypoints.as_slice(),
            &[],
            app.activity.active_route.is_some(),
            app.activity.progress_m,
            app.up_ahead_scope(),
        )
    }
}

/// **Mutual exclusion**, at the one door: with the quick sheet up, the reserved squeezes do not
/// stack a second overlay on it.
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

/// The BLE icon flips the real radio row **and** reaches the persistence handshake — the same
/// before/after `==` a settings screen's edit arms, with no new path.
#[test]
fn the_ble_icon_toggle_reaches_the_settings_save() {
    let mut app = lit();
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

/// **Both arrangements of the root row.** A platform with a panel light offers four controls and
/// opens on brightness; one without offers three and opens on the radio — and every remaining
/// control still reaches the page it names, which is the part an index shift would break.
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
    assert!(!app.settings().ble_enabled, "the first control is the radio");
    assert_eq!(app.backlight_level(), BRIGHTNESS_MAX, "no editor, and no preview to hold");
    assert!(drawer_up(&app), "the sheet is still up");

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

/// A host-pushed modal **takes the sheet with it**, so the panel stops showing an uncommitted
/// preview the rider can no longer reach — and, since #1515 D3, cannot be walked back into either.
///
/// The map-transfer card is the worst case on purpose: it also refuses the chord, so a preview held
/// behind it would stand for the length of a multi-minute upload with no way for the rider to end
/// it.
///
/// **The second half changed in D3.** D2 buried the sheet under the card and gave it back when the
/// card cleared. That left the sheet reachable by dismissing a card — the rider ends a map transfer
/// and lands inside a brightness editor they opened minutes ago. A drawer is transient chrome now:
/// the card closes it, and the transfer ends on the map.
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

/// The settings icon **replaces** the sheet, so a Back out of central settings lands on the base
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

/// **The per-screen dim, in pixels** (#1559). A sheet over a *map* base leaves it exactly as it
/// was — the map reads fine at full colour, and dimming it would mean re-rendering it. A sheet over
/// a *menu* base still recesses it through the dim LUT, because that second draw is a handful of
/// rules and glyphs.
///
/// The mutant: flip `Caps::map()`'s `recess` back to `true` and the first half fails; drop the
/// `caps().recess` read in `render_map` and the second half does.
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

    // --- a map base: untouched ---------------------------------------------------------------
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

    // --- a menu base: recessed ---------------------------------------------------------------
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
