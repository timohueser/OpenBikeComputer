//! The **contextual drawer** (#1515 D3): the bottom sheet the Down+Back chord opens on a screen
//! that declares secondary actions, and the declarative model those screens declare them with.
//!
//! ## Data, not behaviour
//!
//! A screen does not implement a drawer. It names one — a `&'static` [`ContextMenu`], returned from
//! [`Screen::context`](super::Screen::context) in the same partial-match idiom as
//! [`corridor_request`](super::Screen::corridor_request). Everything else lives here: the cursor,
//! which rows are inert, what a press resolves to, how the sheet is drawn and how it animates. A
//! screen that declares nothing gets no drawer, and the chord does nothing on it.
//!
//! That is what keeps the grammar one grammar. The compass ride menu this replaced was a *screen*,
//! so every station it offered was a line of navigation code on a page nothing else shared; here the
//! four ride actions are four rows of a table and the sheet that draws them is the same sheet every
//! later context gets.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::Activity;
use crate::input::Gesture;
use crate::Msg;

use super::{
    palette, Ctx, DetourScreen, PoiMenuScreen, Render, RouteMenuScreen, Screen, ScreenTick, Transition, UpAheadScreen,
};

/// How long the sheet takes to slide up from the bottom edge on open (ms).
const OPEN_MS: u32 = 220;
/// Repaint cadence while the sheet animates (ms) — the wake the event-driven host arms.
const FRAME_MS: u32 = 16;

/// One row's height, and the padding above the first row / below the last.
const ROW_H: i32 = 44;
const SHEET_PAD: i32 = 12;

// ---- The declarative model ------------------------------------------------------------------

/// What pressing a context row does — a **destination**, not a closure. The drawer resolves it
/// against the live [`Ctx`] at press time, which is what lets the table be `&'static` and lets one
/// table serve four screens whose activity state differs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    /// The merged waypoint + corridor-POI timeline, anchored on live progress at entry.
    UpAhead,
    /// The rejoin chooser (#882). Inert without a route, a nav graph and an on-route rider.
    Detour,
    /// The POIs browser's category list.
    Pois,
    /// The stored-route / trip menu.
    Routes,
}

impl ContextAction {
    /// Whether the row can be pressed right now. An unavailable row draws recessed and does
    /// nothing — the drawer's dim means *inert*, unlike the compass dial's, which dimmed stations
    /// a press still opened.
    fn available(self, activity: &Activity, has_nav_graph: bool) -> bool {
        match self {
            // The timeline opens on its own empty state without a route, which is informative
            // rather than dead, so it is always live.
            ContextAction::UpAhead | ContextAction::Pois | ContextAction::Routes => true,
            // #882: a detour needs a route to leave, a graph to route on, and a rider on the route
            // (the corridor anchors on live progress, which off-route freezes).
            ContextAction::Detour => activity.active_route.is_some() && has_nav_graph && !activity.off_route,
        }
    }

    /// The screen this row opens. Every context row **replaces** the sheet, so Back out of the
    /// destination lands on the base screen the rider squeezed from rather than back inside a
    /// drawer they are finished with — the same rule the quick drawer's settings icon follows.
    fn open(self, cx: &Ctx) -> Transition {
        Transition::Replace(match self {
            ContextAction::UpAhead => {
                Screen::UpAhead(UpAheadScreen::new(cx.activity.progress_m, cx.settings.up_ahead_source))
            }
            ContextAction::Detour => Screen::Detour(DetourScreen::new(cx.activity)),
            ContextAction::Pois => Screen::PoiMenu(PoiMenuScreen::new()),
            ContextAction::Routes => Screen::RouteMenu(RouteMenuScreen::new()),
        })
    }
}

/// One row of a declared context: its catalog label and its destination.
#[derive(Clone, Copy)]
pub struct ContextRow {
    pub label: Msg,
    pub action: ContextAction,
}

/// A screen's declared contextual content — the rows the bottom sheet offers, in sheet order.
pub struct ContextMenu {
    pub rows: &'static [ContextRow],
}

/// The **ride context**: the secondary actions the four riding views share (Map, Statistics,
/// Climb, Ride control). It is the compass ride menu's row inventory minus its *Main menu* station,
/// which the global Back-hold escape now serves from everywhere instead of from one screen.
pub static RIDE: ContextMenu = ContextMenu {
    rows: &[
        ContextRow { label: Msg::RideContextUpAhead, action: ContextAction::UpAhead },
        ContextRow { label: Msg::RideContextDetour, action: ContextAction::Detour },
        ContextRow { label: Msg::MenuPois, action: ContextAction::Pois },
        ContextRow { label: Msg::MenuRoutes, action: ContextAction::Routes },
    ],
};

// ---- The generic drawer ------------------------------------------------------------------------

/// The contextual drawer's whole state: when it opened, the table it was opened over, and the
/// cursor. The menu is a `&'static` pointer, so the state is one word wider than the quick
/// drawer's and still far inside the `Screen` slot.
pub struct ContextDrawerScreen {
    opened_ms: u32,
    menu: &'static ContextMenu,
    selected: u8,
    /// Whether the open slide's **landing frame** has been reported. The frame the sheet lands on
    /// still differs from the one before it, and a host that only renders when asked would
    /// otherwise keep the last mid-slide frame on the panel for as long as the sheet is up.
    landed: bool,
}

impl ContextDrawerScreen {
    /// A freshly opened drawer over `menu`, sliding up from `now_ms` with the first row selected.
    pub fn new(now_ms: u32, menu: &'static ContextMenu) -> Self {
        ContextDrawerScreen { opened_ms: now_ms, menu, selected: 0, landed: false }
    }

    /// The exact facts this drawer draws, for the pass's render key: the selected row, and which
    /// rows are live. The identity of the sheet itself is the stack shape, which the key already
    /// carries.
    ///
    /// Availability is derived from the *base* — the route, the graph, the off-route flag — and is
    /// therefore the one thing under an open sheet that may still move a pixel, exactly as the
    /// quick drawer's committed brightness is. It is the cue, not the values behind it, so a rider
    /// drifting off route re-draws the sheet once and a moving map still costs nothing.
    pub(crate) fn key(&self, activity: &Activity, has_nav_graph: bool) -> (u8, u8) {
        let mut live = 0u8;
        for (i, row) in self.menu.rows.iter().enumerate().take(8) {
            if row.action.available(activity, has_nav_graph) {
                live |= 1 << i;
            }
        }
        (self.selected, live)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = self.menu.rows;
        match g {
            Gesture::Step(n) => {
                self.selected = super::vocab::list::step_selection(self.selected as usize, n, rows.len()) as u8;
                Transition::None
            }
            Gesture::Press => match rows.get(self.selected as usize) {
                Some(row) if row.action.available(cx.activity, cx.state.has_nav_graph) => row.action.open(cx),
                // An inert row, or an empty table (which the chord refuses to open a sheet for).
                _ => Transition::None,
            },
            Gesture::Back => Transition::Pop,
            // Select-hold stays local and object-scoped; a context row has no held action.
            // Back-hold never arrives — `App` resolves the global escape above screen dispatch.
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    /// The sheet's slide-up, at frame cadence until it lands.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        let remaining = OPEN_MS.saturating_sub(now_ms.wrapping_sub(self.opened_ms));
        if remaining > 0 {
            return ScreenTick { changed: true, next_wake_ms: Some(FRAME_MS.min(remaining)), region: None };
        }
        if !self.landed {
            self.landed = true;
            return ScreenTick { changed: true, next_wake_ms: None, region: None };
        }
        ScreenTick::idle()
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let rows = self.menu.rows;
        let sheet_h = SHEET_PAD * 2 + ROW_H * rows.len() as i32;
        let visible = self.visible_height(rx.now_ms, sheet_h);
        if visible == 0 {
            return;
        }
        // The sheet stays attached to the bottom edge: it slides up by drawing its full height with
        // its bottom off-screen, so the rounded top lip is what the rider sees arriving.
        let top = rx.h - visible;
        cv.round(rect(4, top, rx.w - 8, sheet_h + 8), 10, palette::PARCHMENT);
        cv.round_outline(rect(4, top, rx.w - 8, sheet_h + 8), 10, palette::WOOD_LIGHT);
        // The grab lip, so the sheet reads as pulled up from the bottom rather than as a card.
        cv.round(rect(rx.w / 2 - 18, top + 7, 36, 4), 2, palette::WOOD_LIGHT);

        let has_nav_graph = rx.state.has_nav_graph;
        for (i, row) in rows.iter().enumerate() {
            let area = rect(14, top + SHEET_PAD + i as i32 * ROW_H, rx.w - 36, ROW_H - 4);
            let live = row.action.available(rx.activity, has_nav_graph);
            super::vocab::rows::row_cursor(cv, area, i as u8 == self.selected, false);
            let ink = if live { palette::INK } else { palette::CONTOUR };
            cv.text_vcentered(
                rx.t(row.label),
                area.top_left.x + 14,
                (area.top_left.y, ROW_H - 4),
                Font::Body,
                TextAlign::Left,
                ink,
            );
            // The go-there chevron, so a row reads as a door rather than as a value.
            if live {
                let (x, cy) = (area.top_left.x + area.size.width as i32 - 18, area.top_left.y + (ROW_H - 4) / 2);
                cv.triangle(Point::new(x, cy - 8), Point::new(x, cy + 8), Point::new(x + 9, cy), ink);
            }
        }
    }

    /// How much of the sheet has arrived from the bottom edge, on the open animation's ease-out.
    fn visible_height(&self, now_ms: u32, sheet_h: i32) -> i32 {
        let t = now_ms.wrapping_sub(self.opened_ms).min(OPEN_MS) as f32 / OPEN_MS as f32;
        let eased = 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t);
        (sheet_h as f32 * eased + 0.5) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::screen::test_ctx;
    use crate::{Activity, AppState, Settings};

    struct World {
        state: AppState,
        activity: Activity,
        settings: Settings,
    }

    impl World {
        /// A routed, nav-graph, on-route ride — every row of the ride context live.
        fn riding() -> Self {
            let mut state = AppState::new(0, 0, 1.0);
            state.has_nav_graph = true;
            let mut activity = Activity::new(Mode::Riding);
            activity.active_route = Some(0);
            World { state, activity, settings: Settings::default() }
        }

        fn press(&mut self, d: &mut ContextDrawerScreen, g: Gesture) -> Transition {
            d.handle(g, &mut test_ctx(&mut self.state, &mut self.activity, &mut self.settings))
        }
    }

    fn drawer() -> ContextDrawerScreen {
        // Opened far enough in the past that the slide has landed.
        ContextDrawerScreen::new(0, &RIDE)
    }

    /// The ride context is the compass menu's inventory minus its Main-menu station, and each row
    /// **replaces** the sheet so Back from the destination lands on the base.
    #[test]
    fn every_ride_row_replaces_the_sheet_with_its_destination() {
        let mut w = World::riding();
        let mut d = drawer();
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::UpAhead(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::Detour(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(2));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::PoiMenu(_))));

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(3));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::RouteMenu(_))));
    }

    /// The cursor wraps both ways over the declared rows, and Back closes the sheet.
    #[test]
    fn the_cursor_wraps_and_back_closes_the_sheet() {
        let mut w = World::riding();
        let mut d = drawer();
        w.press(&mut d, Gesture::Step(-1));
        assert!(
            matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::RouteMenu(_))),
            "wrapped to last"
        );

        let mut d = drawer();
        w.press(&mut d, Gesture::Step(4));
        assert!(matches!(w.press(&mut d, Gesture::Press), Transition::Replace(Screen::UpAhead(_))), "wrapped to first");
        assert!(matches!(w.press(&mut d, Gesture::Back), Transition::Pop));
    }

    /// An inert row is inert: pressing Detour without a route, without a graph, or off route does
    /// nothing at all — and the key says so, so the sheet redraws when the answer changes.
    #[test]
    fn an_unavailable_row_is_inert_and_shows_in_the_key() {
        let live_bit = 1 << 1; // the Detour row

        let mut w = World::riding();
        let mut d = drawer();
        w.press(&mut d, Gesture::Step(1));
        assert_eq!(d.key(&w.activity, w.state.has_nav_graph).1 & live_bit, live_bit, "on route, with a graph: live");

        for break_it in [
            (|w: &mut World| w.activity.active_route = None) as fn(&mut World),
            |w: &mut World| w.state.has_nav_graph = false,
            |w: &mut World| w.activity.off_route = true,
        ] {
            let mut w = World::riding();
            break_it(&mut w);
            let mut d = drawer();
            w.press(&mut d, Gesture::Step(1));
            assert_eq!(d.key(&w.activity, w.state.has_nav_graph).1 & live_bit, 0, "the row went inert");
            assert!(matches!(w.press(&mut d, Gesture::Press), Transition::None), "and a press does nothing");
        }
    }

    /// The timeline anchors its corridor snapshot on **live progress at entry** (epic #946 U3) and
    /// leaves the recording session exactly as it found it — the property the compass menu's north
    /// station carried, now carried by the row that replaced it.
    #[test]
    fn up_ahead_anchors_on_live_progress_and_preserves_the_session() {
        let mut w = World::riding();
        w.activity.mode = Mode::Paused;
        w.activity.start_session();
        w.activity.mode = Mode::Paused;
        w.activity.progress_m = 4_200;
        let session = w.activity.session;
        let mut d = drawer();
        match w.press(&mut d, Gesture::Press) {
            Transition::Replace(Screen::UpAhead(screen)) => {
                let key = screen.corridor_key().expect("the default source scope wants a snapshot");
                assert_eq!(key.anchor_m, 4_200, "the snapshot anchors where the rider is");
                assert_eq!(key.filter, obc_reader::PoiCategorySet::ALL, "the list opens on Everything, every time");
            }
            _ => panic!("the Up ahead row did not open its timeline"),
        }
        assert_eq!(w.activity.mode, Mode::Paused, "opening ride chrome never resumes/pauses the session");
        assert_eq!(w.activity.session, session, "…and never starts a new one");
    }

    /// The sheet arrives monotonically from the bottom and lands exactly on its content height.
    #[test]
    fn the_sheet_slides_up_monotonically_and_lands_exactly() {
        let d = ContextDrawerScreen::new(1_000, &RIDE);
        let target = SHEET_PAD * 2 + ROW_H * RIDE.rows.len() as i32;
        let frames: heapless::Vec<i32, 8> =
            [0, 55, 110, 165, OPEN_MS].iter().map(|dt| d.visible_height(1_000 + dt, target)).collect();
        assert_eq!(frames[0], 0, "nothing is visible on the opening frame");
        assert_eq!(frames[4], target, "and the sheet lands exactly on its height");
        assert!(frames.windows(2).all(|p| p[0] < p[1]), "monotonic: {frames:?}");
        assert!(target < 240, "the ride sheet stays a sheet, not a page: {target} px");
    }
}
