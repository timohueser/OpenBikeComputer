//! The Route menu — pick a route. The shared list chrome with taller panes showing each route's
//! distance and climb. Reached from the main Menu's Routes station (Home → Menu → Routes); `press`
//! opens the [`Route overview`](super::RouteOverviewScreen) (or, mid-ride, resumes / opens the
//! swap flow), `back` returns.
//!
//! Routes come from the app's catalog ([`Render::routes`]/[`Ctx::routes`]), populated by the host
//! from its store. Picking one sets [`Activity::active_route`](crate::Activity::active_route); the
//! host keys geometry loading on it, so the route is streaming open while the overview shows.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::Activity;
use crate::input::Gesture;

use super::list::{self, ListGeometry, Separators};
use super::{palette, Ctx, MapScreen, Render, RouteOverviewScreen, RouteSwapScreen, Screen, Transition};

/// Height of the hold-to-delete footer reserved below the list, so the row window is sized around it
/// and a route pane never draws under the footer. Matches the Fields screen's footer band.
const FOOTER_H: i32 = 34;

/// Per-route pane height (two lines: name + stats), sized so the routes fill the list area above the
/// hold-to-delete footer.
const ROW_H: i32 = 66;

/// Left/right inset of the hold-to-delete footer's rule + contents — the list's own side inset, so
/// the footer aligns under the route panes.
const FOOTER_X: i32 = 12;

/// The route list. State is the highlighted route.
#[derive(Debug, Default)]
pub struct RouteMenuScreen {
    selected: usize,
}

impl RouteMenuScreen {
    pub fn new() -> Self {
        RouteMenuScreen { selected: 0 }
    }

    /// Re-point the highlight after a live catalog rescan (#450): the selection follows the
    /// previously-highlighted route's *identity* to its new index; if that route vanished it falls
    /// back to the nearest row (clamped near its old position — never a dangling index). The list
    /// itself refreshes in place via the shared catalog.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>, new_len: usize) {
        self.selected = remap(self.selected).unwrap_or_else(|| self.selected.min(new_len.saturating_sub(1)));
    }

    /// Whether the highlighted route's hold-to-delete footer is **live** — there is a route to delete
    /// and it isn't referenced by the live ride. "Referenced" = it is the actively-navigated route of
    /// a running tracking session ([`Activity::active_route`] while [`is_tracking`](Activity::is_tracking)):
    /// deleting the file under an open geometry handle mid-ride would break navigation, so the footer
    /// greys out there (a hold does nothing). A route merely *previewed* from Idle (a loaded, un-ridden
    /// `active_route`) is still deletable — no session is keyed to it, and the host closes + reopens
    /// its handle on the store-changed rescan. A side-loaded route (session id) is as deletable as an
    /// uploaded one; the phone holds no bookkeeping on session ids.
    fn delete_enabled(&self, activity: &Activity, len: usize) -> bool {
        len > 0
            && self.selected < len
            && !(activity.is_tracking() && activity.active_route == Some(self.selected.min(len.saturating_sub(1))))
    }

    /// True while the hold-to-delete footer would fill for the current highlight — the footer draws
    /// its live bar then, so [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports a
    /// charging hold as worth repainting here. Mirrors the Fields screen's `selection_is_deletable`.
    pub(crate) fn selection_is_deletable(&self, activity: &Activity, routes_len: usize) -> bool {
        self.delete_enabled(activity, routes_len)
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.routes.len();
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, len),
            Gesture::Press if len > 0 => {
                let i = self.selected.min(len - 1);
                // With a session running, picking a *different* route asks whether to swap
                // navigation only or save-and-start-fresh; re-picking the active route just rides it.
                if cx.activity.is_tracking() {
                    if cx.activity.active_route == Some(i) {
                        return Transition::Root(Screen::Map(MapScreen::new()));
                    }
                    return Transition::Push(Screen::RouteSwap(RouteSwapScreen::new(i)));
                }
                // No session (picking from Idle): open the Route overview. Setting `active_route`
                // makes the host stream the route open (and the profile build) behind the page;
                // the overview's `press` starts the session, its `back` restores the previous one.
                let prev = cx.activity.active_route.replace(i);
                Transition::Push(Screen::RouteOverview(RouteOverviewScreen::new(i, prev)))
            }
            // A completed hold over a *deletable* highlighted route requests its deletion — the guarded
            // hold is the confirmation (no popup), the footer bar its live feedback. Records the delete
            // by index; the host resolves it to the durable object id, deletes the object, and the
            // store-changed rescan re-feeds the catalog (P3 remap keeps the highlight sane). A hold
            // over the active-ride route (greyed footer) does nothing — deleting it mid-ride would
            // yank the geometry out from under navigation.
            Gesture::Hold if self.delete_enabled(cx.activity, len) => {
                cx.activity.request_route_delete(self.selected.min(len - 1));
                Transition::None
            }
            Gesture::Back => Transition::Pop, // return to caller (Home / Menu)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let routes = rx.routes;
        let total = routes.len();
        // Reserve the footer band so a route pane never draws under the hold-to-delete bar.
        let geo = ListGeometry::below_title(w, h - FOOTER_H, ROW_H, 8, 12, Separators::Unselected);

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, "ROUTES", pos, total, geo.visible);

        if total == 0 {
            super::empty_state(cv, w, h, "No routes yet", "Import a GPX file");
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let route = &routes[row.index];
            let y = row.area.top_left.y;

            // Pointer bullet + name, truncated with ".." when it overruns (no ellipsis glyph).
            let accent = if row.selected { INK } else { SUBTEXT };
            let row_mid = y + 33;
            cv.triangle(Point::new(24, row_mid - 8), Point::new(24, row_mid + 8), Point::new(36, row_mid), accent);
            let name_max = (((w - 20) - 44) / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&route.name, name_max);
            cv.text(&name, Point::new(44, y + 9), Font::Body, TextAlign::Left, INK);

            // Stats line: "NNN km" then an up-triangle + "NNNN m" of climb. The climb column sits at
            // a fixed x with room for 5-digit metres.
            let sy = y + 35;
            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", route.distance_km);
            cv.text(&dist, Point::new(44, sy), Font::Label, TextAlign::Left, accent);

            let cx0 = 126;
            // The climb arrow, vertically centred on the Label cap (cap 18 px from `sy`, arrow 9).
            cv.triangle(Point::new(cx0, sy + 14), Point::new(cx0 + 9, sy + 14), Point::new(cx0 + 4, sy + 5), accent);
            let mut climb: heapless::String<12> = heapless::String::new();
            let _ = write!(climb, "{} m", route.climb_m);
            cv.text(&climb, Point::new(cx0 + 16, sy), Font::Label, TextAlign::Left, accent);
        });

        // The hold-to-delete footer over the highlighted route — greyed while it's the active-ride
        // route (a hold does nothing there). Same idiom as the Fields screen's delete bar.
        delete_footer(cv, w, h, self.delete_enabled(rx.activity, total), rx.hold_progress);
    }
}

/// Draw the hold-to-delete footer: a trash can + a warning-red progress bar filled by the live
/// encoder hold. `enabled` greys the whole footer (rule only, no trash/bar) when the highlighted
/// route can't be deleted (it's the actively-navigated ride route). The delete itself fires from
/// `handle`'s `Hold` arm.
fn delete_footer(cv: &mut impl Surface, w: i32, h: i32, enabled: bool, hold: f32) {
    use palette::*;
    let fy = h - FOOTER_H;
    cv.hline(FOOTER_X, fy, w - 2 * FOOTER_X, RULE);
    let midy = fy + FOOTER_H / 2;
    if !enabled {
        // Greyed: a dim trash + a "route in use" hint so the disabled state reads deliberately.
        draw_trash(cv, FOOTER_X + 16, midy, RULE);
        cv.text_vcentered("In use", FOOTER_X + 36, (fy, FOOTER_H), Font::Label, TextAlign::Left, SUBTEXT);
        return;
    }
    let p = hold.clamp(0.0, 1.0);
    draw_trash(cv, FOOTER_X + 16, midy, WARNING);
    let bh = 12;
    let (bx, by) = (FOOTER_X + 36, midy - bh / 2);
    let bw = w - FOOTER_X - 4 - bx;
    cv.round(rect(bx, by, bw, bh), 6, PARCHMENT_SHADE);
    let fill = (bw as f32 * p) as i32;
    if fill > 0 {
        cv.round(rect(bx, by, fill, bh), 6, WARNING);
    }
}

/// Draw a small trash-can glyph centred at `(cx, cy)`: a lidded can with a handle and ribs. The
/// Fields screen's twin — kept local so the two footers stay independent.
fn draw_trash(cv: &mut impl Surface, cx: i32, cy: i32, color: u16) {
    let (bw, bh) = (11, 12);
    let (bx, by) = (cx - bw / 2, cy - bh / 2 + 1);
    cv.round_outline(rect(bx, by, bw, bh), 2, color); // can body
    cv.hline(bx - 2, by - 2, bw + 4, color); // lid
    cv.hline(cx - 2, by - 4, 5, color); // handle
    cv.vline(cx - 2, by + 3, bh - 5, 1, color); // ribs
    cv.vline(cx + 2, by + 3, bh - 5, 1, color);
}

/// Fit a route name into `max_chars`, appending ".." when truncated (no ellipsis glyph).
/// Truncates on a char boundary. Shared with the Route overview's title.
pub(crate) fn fit_name(name: &str, max_chars: usize) -> heapless::String<64> {
    let mut s = heapless::String::new();
    if name.chars().count() <= max_chars {
        let _ = s.push_str(name);
    } else {
        for c in name.chars().take(max_chars.saturating_sub(2)) {
            let _ = s.push(c);
        }
        let _ = s.push_str("..");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::route::RouteSummary;
    use crate::{AppState, Settings};
    use obc_route::BBox;

    /// A minimal named route summary — the Route menu only reads `name` in `handle`.
    fn summary(name: &str) -> RouteSummary {
        RouteSummary {
            name: heapless::String::try_from(name).unwrap(),
            distance_km: 10,
            climb_m: 100,
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
        }
    }

    fn run(scr: &mut RouteMenuScreen, act: &mut Activity, routes: &[RouteSummary], g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes,
            rides: &[],
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// A completed hold over the highlighted route records a delete request for **that** row's index.
    #[test]
    fn hold_records_delete_for_the_highlighted_route() {
        let routes = [summary("A"), summary("B"), summary("C")];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RouteMenuScreen::new();
        run(&mut scr, &mut act, &routes, Gesture::Turn(1)); // highlight row 1 ("B")
        assert_eq!(scr.selected, 1);
        let t = run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::None), "the hold stays on the menu");
        assert_eq!(act.take_route_delete(), Some(1), "the highlighted route's index is requested");
    }

    /// The footer is greyed — and a hold does nothing — while the highlighted route is the active
    /// route of a running tracking session.
    #[test]
    fn hold_over_the_active_ride_route_is_a_no_op() {
        let routes = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // now tracking
        act.active_route = Some(0); // navigating route 0
        let mut scr = RouteMenuScreen::new(); // highlight starts on row 0 = the active route

        assert!(!scr.selection_is_deletable(&act, routes.len()), "the active-ride route's footer is greyed");
        run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert_eq!(act.take_route_delete(), None, "a hold over the active route records nothing");

        // Moving off it re-enables the footer, and a hold there deletes.
        run(&mut scr, &mut act, &routes, Gesture::Turn(1)); // highlight row 1 (not navigated)
        assert!(scr.selection_is_deletable(&act, routes.len()), "a non-active route stays deletable");
        run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert_eq!(act.take_route_delete(), Some(1));
    }

    /// A loaded-but-not-tracking route (an Idle preview left `active_route` set) is still deletable —
    /// no live session is keyed to it.
    #[test]
    fn idle_previewed_route_is_still_deletable() {
        let routes = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Idle);
        act.active_route = Some(0); // previewed from Idle, but no session
        let mut scr = RouteMenuScreen::new();
        assert!(scr.selection_is_deletable(&act, routes.len()), "no session → the footer stays live");
        run(&mut scr, &mut act, &routes, Gesture::Hold);
        assert_eq!(act.take_route_delete(), Some(0));
    }

    /// Picking a route **mid route-less ride** (a running session with no `active_route`) goes
    /// through the guarded swap flow — the "ROUTE ACTIVE" card — exactly as picking a *different*
    /// route mid-ride does. There's no route yet to re-navigate, but a session is live, so the
    /// keep-or-restart choice still applies; it must never silently start navigating.
    #[test]
    fn picking_a_route_during_a_route_less_ride_opens_the_swap() {
        let routes = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Riding);
        act.start_session(); // tracking, but route-less…
        assert_eq!(act.active_route, None, "…so no route is navigated");
        let mut scr = RouteMenuScreen::new(); // highlight row 0
        let t = run(&mut scr, &mut act, &routes, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteSwap(_))), "the guarded swap card opens");
        assert_eq!(act.active_route, None, "picking alone doesn't attach the route — the card decides");
    }

    /// An empty catalog offers no delete (the footer draws its rule only).
    #[test]
    fn empty_catalog_has_no_delete() {
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RouteMenuScreen::new();
        assert!(!scr.selection_is_deletable(&act, 0));
        run(&mut scr, &mut act, &[], Gesture::Hold);
        assert_eq!(act.take_route_delete(), None);
    }
}
