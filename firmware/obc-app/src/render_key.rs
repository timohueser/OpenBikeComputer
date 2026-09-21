//! Render keys: the exact facts the visible screens draw, compared once at the pass boundary.
//!
//! A screen row declares its [`RenderKeyKind`](crate::screen::RenderKeyKind) in the `screens!`
//! table. [`App::render_key`] reads the visible stack and answers with the exact values those kinds
//! name; [`App::run_pass`](crate::App::run_pass) builds that answer before its stages and again
//! after them, and dirties the map when the two differ.
//!
//! Every field is a value, not a digest: floats are stored as [`f32::to_bits`], so `-0.0` and `NaN`
//! compare by their exact representation. A hash would trade a missed redraw, the one failure the
//! dirty contract calls a bug, for a few bytes of stack.
//!
//! The comparison is stack-local and spans one pass, so it sees only the mutations inside that
//! pass. A host seam that mutates `App` between two passes has already moved the fact before the
//! next pass builds its first key, so each such seam keeps an explicit dirty request and says so at
//! its call site. The pre-draw [`prepare_base`](crate::ui_runtime::UiRuntime::prepare_base)
//! acquisition is invisible for the opposite reason: it runs inside the map render, so what it
//! resolves is drawn by the frame that produced it. What a key names there is the request, because
//! the query runs only during a render.

use crate::screen::{RenderKeyKind, Screen, ScreenRow, MAX_DEPTH};
use crate::App;

/// The shape of the visible screen stack: every row from the lowest opaque screen to the top, by
/// variant. A navigation, a card landing and a card dismissal all move it, so a screen transition
/// dirties the map without any screen having to say so.
pub(crate) type ShapeKey = heapless::Vec<ScreenRow, MAX_DEPTH>;

/// One GPS fix, exactly as the riding views draw it: position, course and speed. The camera derives
/// from it, so a fix that moved nothing compares equal and costs no repaint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FixKey {
    lon: i32,
    lat: i32,
    /// `course` / `speed_mps` as exact bit patterns — `None` reads as `u32::MAX`, which no finite
    /// float shares (it is a quiet NaN payload the sensors never produce).
    course: u32,
    speed: u32,
}

impl FixKey {
    fn of(fix: Option<obc_ports::Fix>) -> Option<FixKey> {
        fix.map(|f| FixKey {
            lon: f.lon,
            lat: f.lat,
            course: f.course.map_or(u32::MAX, f32::to_bits),
            speed: f.speed_mps.map_or(u32::MAX, f32::to_bits),
        })
    }
}

/// Home: the battery gauge, the connected indicator, and the backdrop's per-open jitter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HomeKey {
    battery_pct: u8,
    ble_link: crate::ble::BleLink,
    ble_paired: bool,
    /// The screensaver backdrop's seed: Home's whole animation state besides its minute ticker.
    backdrop_seed: u32,
}

/// A map base: the camera, the fix that drives it, the pan HUD, the route-relative chrome and the
/// low-battery cue. Catalog and table indices are narrowed to `u32`, which is lossless because a
/// route slot and a waypoint row are bounded by their catalogs' own low caps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MapKey {
    cam_lon: i32,
    cam_lat: i32,
    /// `zoom` and the projection's rotation as exact bit patterns. `course_rad` already folds
    /// heading-up, the pan freeze, the GPS course and the compass into one angle, so the map's
    /// orientation is one field rather than four that must be kept in step.
    zoom: u32,
    course_rad: u32,
    pan: Option<(crate::app::PanBasis, crate::app::PanTool, u32)>,
    fix: Option<FixKey>,
    active_route: Option<u32>,
    progress_m: u32,
    off_route: bool,
    dist_to_route_m: u32,
    next_waypoint: Option<u32>,
    no_fix: bool,
    tracking: bool,
    /// Whether the top-left low-battery glyph is up. It is the cue, not the level, so the 30 s
    /// poll only repaints the crossing.
    low_battery: bool,
}

/// The Statistics grid: the ride readouts, the route-relative fields, and the live sensor tiles —
/// the last gated per quantity on the field actually being pinned to the grid, so an unconfigured
/// sensor never forces a full map render at its notification rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct StatsKey {
    fix: Option<FixKey>,
    progress_m: u32,
    off_route: bool,
    no_fix: bool,
    active_climb: Option<u32>,
    next_waypoint: Option<u32>,
    /// The displayed heart-rate, power and cadence values, each `None` unless its field is on the
    /// grid, so an unconfigured sensor forces no render at its notification rate.
    live: (Option<u16>, Option<u16>, Option<u8>),
    /// The refresh the [`NextAhead`](crate::next_ahead::NextAhead) cache behind the six
    /// `Next: <category>` tiles asks for: which category is being re-taken, and the progress it is
    /// anchored at. `None` is the settled state.
    ///
    /// It is the request, not the six cached entries, because the answer is distilled inside the
    /// map render and drawn by the frame that produced it. The arming is the half a stack-local
    /// comparison does see, and must act on: the query runs only during a render, so a
    /// render-on-demand host that stays clean here never runs it at all.
    next_ahead: Option<(obc_reader::PoiCategory, u32)>,
}

/// The Climb view: which climb, how far along it, and the grade the cursor sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClimbKey {
    active_climb: Option<u32>,
    progress_m: u32,
    fix: Option<FixKey>,
}

/// The Sensors settings pages: the saved sensors' per-slot status.
///
/// The scan list is deliberately absent. Both it and the status are fed by host seams that run
/// between two passes, so neither edge is visible to a stack-local comparison and each seam asks
/// for its own repaint. The status is here because it is free, and because it is what the row
/// declares it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SensorsKey {
    status: [crate::sensors::SensorStatus; crate::settings::SENSOR_SLOTS],
}

/// The Up-ahead timeline: the live progress every row's distance-to-go is measured from, the route
/// length the ascent figures are taken over, and the corridor snapshot the rows are merged from.
/// Naming progress itself refreshes every row's figure with the fix that changed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct UpAheadKey {
    progress_m: u32,
    route_total_m: u32,
    active_route: Option<u32>,
    /// The merged rows' POI half: how many the snapshot holds, and whether it has settled (an
    /// unsettled list draws its "still looking" hint instead of the rows).
    corridor: (usize, bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DrawerKey {
    page: u8,
    selected: u8,
    staged: u8,
    /// The committed brightness the editor marks while the rider browses alternatives.
    committed: u8,
    /// Which of the sheet's rows are live, as a bitmask: the contextual drawer's equivalent of
    /// `committed`. It is the cue, not the values behind it, so a rider drifting off the route
    /// redraws the sheet once and a moving map under it still costs nothing. For the quick drawer
    /// this byte records whether Bluetooth is enabled.
    enabled: u8,
}

/// One pass's answer: the visible stack's shape, plus the exact facts each declared kind names.
///
/// A kind's slot is `Some` exactly when some visible row declares it. Kinds are facts about the
/// device, not about a screen instance, so two visible rows declaring the same kind fill the same
/// slot with the same value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RenderKey {
    shape: ShapeKey,
    home: Option<HomeKey>,
    map: Option<MapKey>,
    stats: Option<StatsKey>,
    climb: Option<ClimbKey>,
    sensors: Option<SensorsKey>,

    up_ahead: Option<UpAheadKey>,
    drawer: Option<DrawerKey>,
}

const _: () =
    assert!(core::mem::size_of::<RenderKey>() <= 304, "a render key is the visible facts, not a copy of the app state");

impl App {
    /// The exact facts the currently visible screens draw.
    ///
    /// It reads DeviceCore state and mutates nothing: [`run_pass`](App::run_pass) calls it twice per
    /// pass, so a side effect here would be applied twice and once out of order.
    ///
    /// It must stay `#[inline(never)]`: the two calls are the same walk over the same stack, and
    /// pasting it into the ride loop's own frame twice costs flash and the deepest stack frame on
    /// the board.
    #[inline(never)]
    pub(crate) fn render_key(&self) -> RenderKey {
        let mut key = RenderKey {
            shape: ShapeKey::new(),
            home: None,
            map: None,
            stats: None,
            climb: None,
            sensors: None,

            up_ahead: None,
            drawer: None,
        };
        // Drawing starts at the lowest opaque screen: anything below it is covered and draws
        // nothing, so it is not part of the frame and not part of the key.
        let base = self.ui.stack.iter().rposition(|s| !s.is_overlay()).unwrap_or(0);
        let no_fix = !self.has_live_fix(self.ui.now_ms);
        for screen in self.ui.stack.iter().skip(base) {
            // Cannot overflow: the stack itself is `MAX_DEPTH` long.
            let _ = key.shape.push(screen.row());
        }
        if let Some(drawer) = self.drawer_key() {
            key.drawer = Some(drawer);
            return key;
        }
        for screen in self.ui.stack.iter().skip(base) {
            let caps = screen.caps();
            match caps.render_key {
                RenderKeyKind::Static => {}
                RenderKeyKind::Home => key.home = Some(self.home_key(screen)),
                RenderKeyKind::Map => key.map = Some(self.map_key(no_fix)),
                RenderKeyKind::Statistics => key.stats = Some(self.stats_key(no_fix)),
                RenderKeyKind::Climb => key.climb = Some(self.climb_key()),
                RenderKeyKind::SensorSettings => key.sensors = Some(self.sensors_key()),

                RenderKeyKind::UpAhead => key.up_ahead = Some(self.up_ahead_key()),
                // Unreachable: a `Drawer` row returned above. Named rather than wildcarded so a
                // second drawer kind has to state where it belongs.
                RenderKeyKind::Drawer => {}
            }
        }
        key
    }

    /// The visible drawer's facts, or `None` when no row declares [`RenderKeyKind::Drawer`].
    fn drawer_key(&self) -> Option<DrawerKey> {
        self.ui.stack.iter().find_map(|screen| match screen {
            Screen::QuickDrawer(d) => {
                let (page, selected, staged) = d.key();
                Some(DrawerKey {
                    page,
                    selected,
                    staged,
                    committed: self.settings().brightness,
                    enabled: u8::from(self.settings().ble_enabled),
                })
            }
            Screen::ContextDrawer(d) => {
                // The contextual sheet's five facts: its page, the cursor, the value the nested
                // editor has staged, the value already committed underneath, and which rows are
                // live.
                let (page, selected, staged, committed, enabled) = d.key(&crate::screen::ContextFacts {
                    state: &self.state,
                    navigation: self.navigator.route_state(),
                    settings: self.settings(),
                    recording: self.recorder.recording(),

                    nav_profiles: self.nav_profiles(),
                });
                Some(DrawerKey { page, selected, staged, committed, enabled })
            }
            _ => None,
        })
    }

    fn home_key(&self, screen: &Screen) -> HomeKey {
        HomeKey {
            battery_pct: self.state.device.battery_pct,
            ble_link: self.state.device.ble_link,
            ble_paired: self.state.device.ble_paired,
            backdrop_seed: match screen {
                Screen::Home(home) => home.backdrop_seed(),
                _ => 0,
            },
        }
    }

    fn map_key(&self, no_fix: bool) -> MapKey {
        let navigation = self.navigator.route_state();
        MapKey {
            cam_lon: self.state.cam_lon,
            cam_lat: self.state.cam_lat,
            zoom: self.state.zoom.to_bits(),
            course_rad: self.state.course_rad().to_bits(),
            pan: self.state.pan.map(|p| (p.basis, p.tool, p.route_progress_m)),
            fix: FixKey::of(self.state.user_fix),
            active_route: navigation.active_route.map(|i| i as u32),
            progress_m: navigation.progress_m,
            off_route: navigation.off_route,
            dist_to_route_m: navigation.dist_to_route_m,
            next_waypoint: navigation.next_waypoint.map(|i| i as u32),
            no_fix,
            tracking: self.recorder.recording(),
            low_battery: crate::screen::low_battery_cue(self.state.device.battery_pct),
        }
    }

    fn stats_key(&self, no_fix: bool) -> StatsKey {
        use crate::stat_fields::StatField;
        let fields = &self.settings().stat_fields;
        let navigation = self.navigator.route_state();
        StatsKey {
            fix: FixKey::of(self.state.user_fix),
            progress_m: navigation.progress_m,
            off_route: navigation.off_route,
            no_fix,
            active_climb: navigation.active_climb.map(|i| i as u32),
            next_waypoint: navigation.next_waypoint.map(|i| i as u32),
            live: (
                fields.contains(StatField::HeartRate).then(|| self.recorder.live_hr_display()).flatten(),
                fields.contains(StatField::Power).then(|| self.recorder.live_power_display()).flatten(),
                fields.contains(StatField::Cadence).then(|| self.recorder.live_cadence_display()).flatten(),
            ),
            next_ahead: self.ui.next_ahead.pending_refresh(),
        }
    }

    fn climb_key(&self) -> ClimbKey {
        let navigation = self.navigator.route_state();
        ClimbKey {
            active_climb: navigation.active_climb.map(|i| i as u32),
            progress_m: navigation.progress_m,
            fix: FixKey::of(self.state.user_fix),
        }
    }

    fn sensors_key(&self) -> SensorsKey {
        SensorsKey { status: self.ui.sensor_status }
    }

    fn up_ahead_key(&self) -> UpAheadKey {
        let navigation = self.navigator.route_state();
        UpAheadKey {
            progress_m: navigation.progress_m,
            route_total_m: navigation.route_total_m,
            active_route: navigation.active_route.map(|i| i as u32),
            corridor: (self.ui.corridor_scratch.len(), !self.ui.corridor_scratch.pending()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppState;
    use crate::screen::{MenuScreen, RenderKeyKind};

    #[test]
    fn every_screen_row_declares_a_render_key_kind() {
        let declared: std::vec::Vec<(&str, RenderKeyKind)> = Screen::NAMES
            .iter()
            .zip(Screen::CAPS)
            .filter(|(_, caps)| caps.render_key != RenderKeyKind::Static)
            .map(|(name, caps)| (*name, caps.render_key))
            .collect();
        assert_eq!(
            declared,
            [
                ("Home", RenderKeyKind::Home),
                ("Map", RenderKeyKind::Map),
                ("Landmarks", RenderKeyKind::Map),
                ("Statistics", RenderKeyKind::Statistics),
                ("Climb", RenderKeyKind::Climb),
                ("PeakView", RenderKeyKind::Statistics),
                ("Detour", RenderKeyKind::Map),
                ("DetourPreview", RenderKeyKind::Map),
                ("WhatsNext", RenderKeyKind::UpAhead),
                ("FindPlace", RenderKeyKind::Map),
                ("VisitReview", RenderKeyKind::Map),
                ("Easier", RenderKeyKind::Map),
                ("Sensors", RenderKeyKind::SensorSettings),
                ("SensorScan", RenderKeyKind::SensorSettings),
                ("QuickDrawer", RenderKeyKind::Drawer),
                ("ContextDrawer", RenderKeyKind::Drawer),
            ],
            "the dynamic rows, in table order — add a row here when a screen starts drawing a live fact"
        );
    }

    #[test]
    fn an_unchanged_device_answers_the_same_key() {
        let app = App::new(AppState::new(0, 0, 1.0));
        assert_eq!(app.render_key(), app.render_key(), "reading the key must not change it");
    }

    /// The same-kind move is the one that needs the shape: swapping the Map for the Detour chooser
    /// leaves every declared fact identical, because both rows declare [`Map`](RenderKeyKind::Map)
    /// and the facts are the device's, so the identity of the row is all that moved.
    #[test]
    fn a_screen_transition_moves_the_key() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        let before = app.render_key();
        let _ = app.ui.stack.push(Screen::Menu(MenuScreen::new()));
        assert_ne!(app.render_key(), before, "a move to another kind changes which slot is filled");

        // Back to a map base, then sideways to a screen declaring the very same kind.
        app.ui.stack.truncate(2);
        let on_map = app.render_key();
        app.ui.stack[1] = Screen::Detour(crate::screen::DetourScreen::new(app.navigator.route_state()));
        assert_eq!(
            app.render_key().map,
            on_map.map,
            "the two rows declare one kind over one device, so the facts are the same value"
        );
        assert_ne!(app.render_key(), on_map, "…and the shape is what tells the two frames apart");
    }

    #[test]
    fn floats_are_compared_by_their_exact_bits() {
        let mut app = App::new(AppState::new(0, 0, 1.0));
        let before = app.render_key();
        app.state.zoom = -0.0;
        let negative_zero = app.render_key();
        app.state.zoom = 0.0;
        assert_eq!(app.state.zoom, -0.0, "IEEE says these two zooms are the same number");
        assert_ne!(app.render_key(), negative_zero, "…and the key says they are two different frames");
        assert_ne!(negative_zero, before);
    }

    #[test]
    fn the_battery_level_is_in_homes_key_and_in_no_other() {
        let mut home = App::new_idle(AppState::new(0, 0, 1.0));
        let before = home.render_key();
        home.state.device.battery_pct = 42;
        assert_ne!(home.render_key(), before, "Home draws the gauge");

        let mut map = App::new(AppState::new(0, 0, 1.0));
        let before = map.render_key();
        map.state.device.battery_pct = 42;
        assert_eq!(map.render_key(), before, "a level a map base never draws costs it no render");
    }

    /// A helper: `[Home, Map]` with the quick drawer squeezed open on top.
    fn map_under_a_drawer() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(app.apply_chord(crate::input::Chord::Quick), "the chord opened a drawer");
        app
    }

    /// Every base slot is empty while a drawer is visible: that is what "the base is frozen" is,
    /// said once, in the only place the repaint decision is made.
    #[test]
    fn a_drawer_on_top_leaves_every_base_fact_out_of_the_key() {
        let app = map_under_a_drawer();
        let key = app.render_key();
        assert!(key.drawer.is_some(), "the drawer names its own facts");
        assert!(
            key.map.is_none() && key.home.is_none() && key.stats.is_none() && key.climb.is_none(),
            "no base fact survives under a drawer"
        );
        assert!(key.sensors.is_none() && key.up_ahead.is_none());
        // The shape still carries the covered rows, which is what makes the *close* visible.
        assert_eq!(key.shape.len(), 2, "Map + the sheet above it");
    }

    #[test]
    fn moving_the_camera_under_a_drawer_does_not_move_the_key() {
        let mut app = map_under_a_drawer();
        let quiet = app.render_key();
        app.state.cam_lon += 5_000;
        app.state.cam_lat -= 4_000;
        app.state.zoom = 3.0;
        app.state.user_fix = Some(obc_ports::Fix::at(1_000, 2_000));
        app.state.device.battery_pct = 9;
        assert_eq!(app.render_key(), quiet, "a moving map under a sheet asks for no repaint");
    }

    #[test]
    fn closing_a_drawer_invalidates_the_base_exactly_once() {
        let mut app = map_under_a_drawer();
        app.state.cam_lon += 5_000; // moved invisibly while the sheet was up
        let covered = app.render_key();

        assert!(app.apply_chord(crate::input::Chord::Quick), "the same chord closes it");
        let uncovered = app.render_key();
        assert_ne!(uncovered, covered, "the close is the one invalidation");
        assert!(uncovered.drawer.is_none() && uncovered.map.is_some(), "…and the base is back in the key");
        assert_eq!(app.render_key(), uncovered, "exactly one: the next frame asks for nothing");
    }

    /// `[Home, Map]` with the ride context sheet squeezed open on top.
    fn map_under_the_context_sheet() -> App {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(app.apply_chord(crate::input::Chord::Context), "the riding Map declares a context");
        app
    }

    /// The kind is declared on the row, not on the drawer's identity, so the frozen base is one
    /// rule and not two.
    #[test]
    fn a_context_sheet_leaves_every_base_fact_out_of_the_key() {
        let app = map_under_the_context_sheet();
        let key = app.render_key();
        assert!(key.drawer.is_some(), "the sheet names its own facts");
        assert!(key.map.is_none() && key.home.is_none() && key.stats.is_none() && key.climb.is_none());
        assert!(key.sensors.is_none() && key.up_ahead.is_none());
        assert_eq!(key.shape.len(), 2, "Map + the sheet above it");
    }

    /// Unlike the quick drawer, this sheet draws a fact derived from the base, so the case is
    /// worth its own assertion.
    #[test]
    fn moving_the_camera_under_the_context_sheet_does_not_move_the_key() {
        let mut app = map_under_the_context_sheet();
        let quiet = app.render_key();
        app.state.cam_lon += 5_000;
        app.state.zoom = 3.0;
        app.state.user_fix = Some(obc_ports::Fix::at(1_000, 2_000));
        app.state.device.battery_pct = 9;
        app.navigator.route_state_mut().progress_m += 900;
        assert_eq!(app.render_key(), quiet, "a moving map under a sheet asks for no repaint");
    }

    /// A row going inert is a pixel the sheet draws, so that one does move the key.
    #[test]
    fn a_row_going_inert_under_the_sheet_moves_the_key() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        app.test_start_ride();
        app.navigator.route_state_mut().active_route = Some(0);
        app.state.has_nav_graph = true;
        assert!(app.apply_chord(crate::input::Chord::Context));
        let live = app.render_key();
        app.navigator.route_state_mut().off_route = true;
        assert_ne!(app.render_key(), live, "the Detour row went recessed — the sheet must redraw");
        app.navigator.route_state_mut().off_route = false;
        assert_eq!(app.render_key(), live, "…and back on route is the same sheet again");
    }

    /// The twin on the map display sheet, where the moving fact is a switch's own bit rather than
    /// a row's availability. `DrawerKey` needs no field for it: the three switches are device-only,
    /// so every state change comes with a `committed` change and every cursor move with a
    /// `selected` change, which the two existing bytes already carry.
    #[test]
    fn a_flip_under_the_sheet_moves_only_the_drawer_key() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        assert!(app.apply_chord(crate::input::Chord::Context), "the Map declares a context");
        app.apply_gesture(crate::input::Gesture::Step(-1)); // → the Map display row
        app.apply_gesture(crate::input::Gesture::Press); // → the display sheet

        let quiet = app.render_key();
        assert!(quiet.map.is_none(), "no map fact survives under either sheet");
        assert_eq!(quiet.drawer.map(|d| d.enabled), Some(0b1111), "all display rows are always live");
        assert_eq!(quiet.drawer.map(|d| d.committed), Some(1), "the selected row reads its own bit");

        app.state.cam_lon += 5_000;
        app.state.user_fix = Some(obc_ports::Fix::at(1_000, 2_000));
        assert_eq!(app.render_key(), quiet, "a moving map under the display sheet asks for no repaint");

        // A flip is, and it moves the key through `committed` alone.
        app.apply_gesture(crate::input::Gesture::Press);
        let flipped = app.render_key();
        assert_ne!(flipped, quiet, "the slider moved — the sheet must redraw");
        assert_eq!(flipped.drawer.map(|d| d.committed), Some(0));

        assert!(app.apply_chord(crate::input::Chord::Context), "the same chord closes it");
        let uncovered = app.render_key();
        assert_ne!(uncovered, flipped);
        assert!(uncovered.drawer.is_none() && uncovered.map.is_some(), "the base is back");
        assert_eq!(app.render_key(), uncovered, "exactly one: the next frame asks for nothing");
    }

    #[test]
    fn closing_the_context_sheet_invalidates_the_base_exactly_once() {
        let mut app = map_under_the_context_sheet();
        app.state.cam_lon += 5_000; // moved invisibly while the sheet was up
        let covered = app.render_key();
        assert!(app.apply_chord(crate::input::Chord::Context), "the same chord closes it");
        let uncovered = app.render_key();
        assert_ne!(uncovered, covered, "the close is the one invalidation");
        assert!(uncovered.drawer.is_none() && uncovered.map.is_some(), "…and the base is back in the key");
        assert_eq!(app.render_key(), uncovered, "exactly one: the next frame asks for nothing");
    }

    /// The nested editor's own three facts: the page, the value staged on it and the value
    /// committed underneath are three separate reasons to repaint, and the base under the sheet
    /// stays frozen through all of them.
    #[test]
    fn the_nested_editor_puts_its_page_staged_and_committed_value_in_the_key() {
        let mut app = App::new(AppState::new(0, 0, 1.0)); // [Home, Map]
        app.apply_gesture(crate::Gesture::Press); // -> the Menu, then a route-less Up-ahead list
        app.ui.stack.truncate(2);
        app.ui.stack[1] = Screen::WhatsNext(crate::screen::WhatsNextScreen::new());
        app.apply_gesture(crate::Gesture::Press);
        assert!(app.apply_chord(crate::input::Chord::Context), "the timeline declares a context");

        let root = app.render_key();
        // A page slide owns the sheet's input while it runs, so each gesture waits for the last
        // one's slide to land — the same clock the host advances.
        let mut ms = 0;
        let mut act = |app: &mut App, g: crate::Gesture| {
            ms += 400;
            app.advance_animations(obc_ports::InputClock(ms));
            app.apply_gesture(g);
        };

        act(&mut app, crate::Gesture::Press); // -> the Filter editor
        let opened = app.render_key();
        assert_ne!(opened, root, "the page is part of the frame");
        assert!(opened.map.is_none() && opened.up_ahead.is_none(), "…and the base is still frozen");

        act(&mut app, crate::Gesture::Step(2)); // stage two choices on
        let staged = app.render_key();
        assert_ne!(staged, opened, "the staged choice is what the editor draws");

        let with_staged = app.render_key();
        app.state.up_ahead_filter = obc_reader::PoiCategorySet::only(obc_reader::PoiCategory::Pharmacy);
        assert_ne!(app.render_key(), with_staged, "the committed mark is a pixel the sheet draws");

        let quiet = app.render_key();
        app.state.cam_lon += 5_000;
        app.navigator.route_state_mut().progress_m += 900;
        assert_eq!(app.render_key(), quiet, "a moving base under an open editor asks for no repaint");
    }

    /// The twin on the route-plan sheet, where the moving fact comes from outside the app: the
    /// host loading a map changes how many routing profiles exist, and so whether the sheet's one
    /// row is live and which profile it marks.
    #[test]
    fn a_map_load_under_the_sheet_moves_the_row_and_nothing_else() {
        let mut app = App::new_idle(AppState::new(0, 0, 1.0)); // [Home]
        let _ = app.ui.stack.push(crate::harness::support::selected_place());
        assert!(app.apply_chord(crate::input::Chord::Context), "the confirm card declares a context");

        let inert = app.render_key();
        assert_eq!(inert.drawer.map(|d| d.enabled), Some(0), "with no map the row has no choice to offer");
        assert!(inert.map.is_none() && inert.home.is_none());

        // The host loads a map under the open sheet, through the real parse path.
        let bytes = crate::harness::support::build_min_obcm_profiles(0, &["Road", "Gravel", "MTB", "Touring"]);
        let src = obc_reader::SliceSource(&bytes);
        let tables = obc_reader::MapTables::parse(&src).expect("valid fixture");
        app.set_nav_profiles(tables.nav_profiles());
        let live = app.render_key();
        assert_ne!(live, inert, "the row went live — the sheet must redraw");
        assert_eq!(live.drawer.map(|d| d.enabled), Some(1));

        // A stale index resolved to profile 0 while the map was empty and resolves to itself now.
        app.set_settings(crate::settings::Settings { bike_profile_idx: 2, ..Default::default() });
        let marked = app.render_key();
        assert_ne!(marked, live, "the tick moved to the profile the router will use");
        assert_eq!(marked.drawer.map(|d| d.committed), Some(2));

        let quiet = app.render_key();
        app.state.cam_lon += 5_000;
        app.state.device.battery_pct = 9;
        assert_eq!(app.render_key(), quiet, "a moving base under the route-plan sheet asks for no repaint");

        assert!(app.apply_chord(crate::input::Chord::Context), "the same chord closes it");
        let uncovered = app.render_key();
        assert_ne!(uncovered, quiet);
        assert!(uncovered.drawer.is_none(), "the sheet is gone from the key");
        assert_eq!(app.render_key(), uncovered, "exactly one: the next frame asks for nothing");
    }

    /// The two sheets are different frames: the shape carries which drawer is up, so swapping one
    /// for the other repaints even though both fill the same key slot.
    #[test]
    fn the_two_sheets_are_told_apart_by_the_shape() {
        let mut app = map_under_the_context_sheet();
        let context = app.render_key();
        assert!(app.apply_chord(crate::input::Chord::Quick), "the other chord swaps the sheet");
        assert_ne!(app.render_key(), context, "a different sheet is a different frame");
    }

    /// The map base does draw the low-battery glyph, so the pass must see the threshold crossing,
    /// both on the way down and on a charge back over it.
    #[test]
    fn the_low_battery_cue_moves_the_map_key_in_both_directions() {
        let mut map = App::new(AppState::new(0, 0, 1.0)); // [Home, Map] — a map base
        map.state.device.battery_pct = 11;
        let above = map.render_key();
        map.state.device.battery_pct = 9;
        let below = map.render_key();
        assert_ne!(below, above, "crossing into the cue must repaint — the glyph has to appear");
        map.state.device.battery_pct = 40;
        assert_eq!(map.render_key(), above, "and charging back over it takes the glyph away again");
    }
}
