//! The Route menu — pick a route, or open a trip folder. The shared list chrome with taller panes.
//!
//! Two levels, one screen (epic #526, TR3 — "share, don't fork"). The [`scope`](RouteMenuScreen)
//! decides what the rows are:
//!
//! - [`TopLevel`](RouteMenuScope::TopLevel): **trip folder rows first, then the unfiled routes**,
//!   each group in the catalog's order. A folder row is visually distinct (see [`FolderRowStyle`]);
//!   pressing it pushes a second [`RouteMenuScreen`] scoped to that trip, long-pressing it opens the
//!   [`TripDeleteScreen`] cascade-delete confirm. A route row behaves exactly as it always has.
//! - [`Trip`](RouteMenuScope::Trip): that trip's **member routes as completely standard route rows**
//!   — select/load, per-route delete via the Route overview, everything downstream trip-unaware.
//!   Back pops to the top level; the hierarchy is exactly one level (a stage list never nests).
//!
//! Routes come from the app's catalog ([`Render::routes`]/[`Ctx::routes`]); trips from the resolved
//! [`Render::trips`]/[`Ctx::trips`] catalog (each carries its member routes' catalog indices, ride
//! order, dangling refs already dropped — TR2). Picking a route sets
//! [`Activity::active_route`](crate::Activity::active_route) and streams it open behind the overview,
//! identically whether the route was reached at the top level or from inside a folder.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::route::RouteSummary;
use crate::trip::TripSummary;
use crate::Msg;

use super::list::{self, ListGeometry, Separators};
use super::{
    palette, Ctx, MapScreen, Render, RouteOverviewScreen, RouteSwapScreen, Screen, Transition, TripDeleteScreen,
};

/// Per-route pane height (two lines: name + stats), sized so the routes fill the full list area
/// (the hold-to-delete footer is gone — deleting a route now lives on the Route overview, T3).
const ROW_H: i32 = 66;

/// Text inset of the name/stats column from the row area's left edge. The per-row `▶` triangle is
/// gone (T3); the name column shifts left to this inset and gains the triangle's width. Distance on
/// line 2 shares this x, so name and distance form one left column.
const NAME_INSET: i32 = 12;

/// The list side inset (the row area's left/right margin from the panel edge) — shared so the folder
/// and route rows recover the climb column from the same geometry.
const SIDE_INSET: i32 = 12;

/// The stats line's second column — the climb group (`▲` + metres) — as a fraction of the row's
/// inner width, so the climb figures line up across every row regardless of distance width (T3).
const CLIMB_COL_PCT: i32 = 55;

/// Which of the folder-row visual variants the top-level menu draws (epic #526, TR3). The full trip
/// machinery is identical across all three — only [`draw_folder_row`] branches on this — so the
/// owner can pick the look by flipping this const (the two other variants are rendered to
/// `tr3-variants/` for the decision). **Variant A is active**; the pick will collapse this to one.
///
/// - [`A`](FolderRowStyle::A): a small folder glyph before the name + the summed distance right-
///   aligned on line 1; the count on the left of line 2, the summed climb at the route-row climb
///   column. Shows all four figures (name truncates when long).
/// - [`B`](FolderRowStyle::B): no glyph; the **full name** gets line 1 (never crowded), a single
///   `N routes · km` caption on line 2 — the legible, name-first look (drops the summed climb).
/// - [`C`](FolderRowStyle::C): a drawn folder-tab icon + a rounded count badge on the name line, with
///   the summed `km` / climb on line 2 in the **same two columns as a route row**, so the stats align
///   down the list and the folderness is carried by the icon + badge (uniform-chrome / field-map).
const FOLDER_ROW_STYLE: FolderRowStyle = FolderRowStyle::A;

/// The switchable folder-row look — see [`FOLDER_ROW_STYLE`]. The two variants the active const
/// doesn't select are dead until the owner flips it (that's the whole point of the knob), so the
/// unused-variant lint is expected here.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum FolderRowStyle {
    A,
    B,
    C,
}

/// The upper bound on rows the merged top-level list can hold: every trip folder plus every route
/// (when nothing is filed). A stage list is a subset of the routes, so this bounds both scopes.
const ROW_CAP: usize = crate::trip::MAX_TRIPS + crate::route::MAX_ROUTES;

/// One row of the menu: a trip **folder** (by trip-catalog index) or a **route** (by route-catalog
/// index). The route index is a real catalog index in **either** scope, so pressing a route row is
/// the same code path at the top level and inside a folder.
#[derive(Clone, Copy)]
enum Row {
    Folder(usize),
    Route(usize),
}

/// The **identity** of the highlighted row, kept so an open menu's highlight follows the thing it
/// pointed at across a live catalog rescan (#450). A route is pinned by its catalog **index**
/// (remapped by the app's id-based remap); a folder by its trip's durable **id** (route rescans
/// never move it, and a trip delete re-feeds the trip catalog separately).
#[derive(Debug, Clone, Copy)]
enum SelId {
    Folder(u16),
    Route(usize),
}

impl Row {
    /// This row's [`SelId`] — a folder resolves to its trip's durable id, a route to its index.
    fn identity(self, trips: &[TripSummary]) -> Option<SelId> {
        match self {
            Row::Folder(ti) => trips.get(ti).map(|t| SelId::Folder(t.id)),
            Row::Route(ri) => Some(SelId::Route(ri)),
        }
    }

    /// Whether this row is the one `id` identifies.
    fn is(self, id: SelId, trips: &[TripSummary]) -> bool {
        match (self, id) {
            (Row::Folder(ti), SelId::Folder(tid)) => trips.get(ti).is_some_and(|t| t.id == tid),
            (Row::Route(ri), SelId::Route(i)) => ri == i,
            _ => false,
        }
    }
}

/// What this menu instance lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RouteMenuScope {
    /// The top level: trip folders first, then the unfiled routes.
    TopLevel,
    /// Inside one trip's folder: that trip's member routes only, keyed by the trip's **durable id**
    /// (resolved against the live trip catalog each frame, so a rescan that reorders trips can't
    /// mis-scope it, and a trip that vanished shows the empty state).
    Trip { trip_id: u16 },
}

/// The route list. State is the highlighted row, its pinned identity (for the rescan remap), and
/// what the list is scoped to.
#[derive(Debug)]
pub struct RouteMenuScreen {
    selected: usize,
    /// The identity of the highlighted row, pinned each `handle`/`draw` so a live rescan can follow
    /// it to its new row (see [`remap_routes`](RouteMenuScreen::remap_routes)). `None` before the
    /// first frame — a rescan then just clamps.
    sel_id: Option<SelId>,
    scope: RouteMenuScope,
}

impl Default for RouteMenuScreen {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteMenuScreen {
    /// The top-level menu (trip folders + unfiled routes).
    pub fn new() -> Self {
        RouteMenuScreen { selected: 0, sel_id: None, scope: RouteMenuScope::TopLevel }
    }

    /// A stage list scoped to the trip with durable id `trip_id` — pushed when a folder row is
    /// pressed at the top level.
    pub fn trip(trip_id: u16) -> Self {
        RouteMenuScreen { selected: 0, sel_id: None, scope: RouteMenuScope::Trip { trip_id } }
    }

    /// Re-point the highlight after a live catalog rescan (#450). The highlighted row's *identity*
    /// was pinned in the last `handle` (a route by catalog index, a folder by its trip id); here it's
    /// mapped across the rescan (the route index through the app's id-based `remap`, the folder id
    /// unchanged) and re-found in the regrouped list, so the highlight follows the same route/folder
    /// to its new row. A vanished route (its index remaps to nothing) clamps near its old position,
    /// never a dangling index. The app hands the fresh `trips` (already re-resolved) + the new route
    /// count so the new row layout can be rebuilt here.
    pub(crate) fn remap_routes(
        &mut self,
        remap: &dyn Fn(usize) -> Option<usize>,
        trips: &[TripSummary],
        routes_len: usize,
    ) {
        let mapped = self.sel_id.and_then(|id| match id {
            SelId::Route(i) => remap(i).map(SelId::Route),
            SelId::Folder(tid) => Some(SelId::Folder(tid)),
        });
        let mut rows: heapless::Vec<Row, ROW_CAP> = heapless::Vec::new();
        self.build_rows(trips, routes_len, &mut rows);
        self.selected = match mapped.and_then(|id| rows.iter().position(|r| r.is(id, trips))) {
            Some(row) => row,
            // Vanished (or nothing pinned): clamp near the old position, never a dangling index.
            None => self.selected.min(rows.len().saturating_sub(1)),
        };
        // Re-pin to whatever row the highlight now rests on, so a later rescan follows from here.
        self.sel_id = rows.get(self.selected).and_then(|r| r.identity(trips));
    }

    /// Pin the identity of the current selection over `rows` — called after any selection change so
    /// the next rescan-remap can follow it.
    fn pin(&mut self, rows: &[Row], trips: &[TripSummary]) {
        self.sel_id = rows.get(self.selected).and_then(|r| r.identity(trips));
    }

    /// Build the current scope's rows into `out` (folder rows first, then unfiled routes, at the top
    /// level; the trip's member routes inside a folder). Reads only `routes_len` (row *structure* is
    /// which catalog index each row is), so a rescan-time rebuild needs no route slice.
    fn build_rows(&self, trips: &[TripSummary], routes_len: usize, out: &mut heapless::Vec<Row, ROW_CAP>) {
        out.clear();
        match self.scope {
            RouteMenuScope::TopLevel => {
                for ti in 0..trips.len() {
                    let _ = out.push(Row::Folder(ti));
                }
                for ri in 0..routes_len {
                    // A filed route shows only inside its folder — skip it at the top level.
                    let filed = trips.iter().any(|t| t.stage_indices.contains(&(ri as u16)));
                    if !filed {
                        let _ = out.push(Row::Route(ri));
                    }
                }
            }
            RouteMenuScope::Trip { trip_id } => {
                if let Some(t) = trips.iter().find(|t| t.id == trip_id) {
                    for &idx in t.stage_indices.iter() {
                        let _ = out.push(Row::Route(idx as usize));
                    }
                }
            }
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let mut rows: heapless::Vec<Row, ROW_CAP> = heapless::Vec::new();
        self.build_rows(cx.trips, cx.routes.len(), &mut rows);
        let len = rows.len();
        // Keep the selection in range against a list that may have regrouped since last frame, and
        // pin its identity so the next rescan-remap can follow it.
        if len > 0 {
            self.selected = self.selected.min(len - 1);
        }
        self.pin(&rows, cx.trips);
        match g {
            Gesture::Turn(n) => {
                let t = list::on_turn(&mut self.selected, n, len);
                self.pin(&rows, cx.trips);
                t
            }
            Gesture::Press if len > 0 => match rows[self.selected.min(len - 1)] {
                // A folder press opens its stage list — a second Route menu scoped to the trip.
                Row::Folder(ti) => Transition::Push(Screen::RouteMenu(RouteMenuScreen::trip(cx.trips[ti].id))),
                // A route press is the unchanged route flow — identical at the top level and inside a
                // folder (the index is a real catalog index either way).
                Row::Route(ri) => self.press_route(ri, cx),
            },
            // A long-press on a folder opens the cascade-delete confirm. On a route row it does
            // nothing (top-level routes delete from the Route overview, as before).
            Gesture::Hold if len > 0 => match rows[self.selected.min(len - 1)] {
                Row::Folder(ti) => {
                    let t = &cx.trips[ti];
                    Transition::Push(Screen::TripDelete(TripDeleteScreen::new(t.id, &t.name)))
                }
                Row::Route(_) => Transition::None,
            },
            Gesture::Back => Transition::Pop, // top level → Home/Menu; stage list → top level
            _ => Transition::None,
        }
    }

    /// The route-row press flow, shared by both scopes (unchanged from the flat menu): mid-ride it
    /// asks whether to swap or re-ride; from Idle it opens the Route overview with the route
    /// streaming open behind it.
    fn press_route(&self, i: usize, cx: &mut Ctx) -> Transition {
        if cx.activity.is_tracking() {
            if cx.activity.active_route == Some(i) {
                return Transition::Root(Screen::Map(MapScreen::new()));
            }
            return Transition::Push(Screen::RouteSwap(RouteSwapScreen::new(i)));
        }
        let prev = cx.activity.active_route.replace(i);
        Transition::Push(Screen::RouteOverview(RouteOverviewScreen::new(i, prev)))
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let routes = rx.routes;
        let trips = rx.trips;

        let mut rows: heapless::Vec<Row, ROW_CAP> = heapless::Vec::new();
        self.build_rows(trips, routes.len(), &mut rows);
        let total = rows.len();

        let geo = ListGeometry::below_title(w, h, ROW_H, 8, SIDE_INSET, Separators::Unselected);

        // The title: "ROUTES" at the top level; the trip's own name inside a folder (names over
        // metadata). A vanished trip falls back to the plain title. `title_buf` is filled only on the
        // trip-name path, so it's declared uninitialized and borrowed there.
        let title_buf: heapless::String<64>;
        let title = match self.scope {
            RouteMenuScope::TopLevel => rx.t(Msg::RouteMenuTitle),
            RouteMenuScope::Trip { trip_id } => match trips.iter().find(|t| t.id == trip_id) {
                Some(t) => {
                    // Leave room for the scroll counter the title bar's right slot may show.
                    let max = (((w - 72) / Font::Body.char_width() as i32).max(6)) as usize;
                    title_buf = fit_name(&t.name, max);
                    &title_buf
                }
                None => rx.t(Msg::RouteMenuTitle),
            },
        };

        let pos = if total == 0 { 0 } else { self.selected.min(total - 1) + 1 };
        list::list_frame(cv, w, h, title, pos, total, geo.visible);

        if total == 0 {
            // The top level with no routes at all → "No routes yet / Import a GPX file"; an empty
            // folder (all its refs dangled) → the same empty-list state with a trip-specific sub.
            let sub = match self.scope {
                RouteMenuScope::TopLevel => rx.t(Msg::RouteMenuNoRoutesSub),
                RouteMenuScope::Trip { .. } => rx.t(Msg::RouteMenuFolderEmptySub),
            };
            super::empty_state(cv, w, h, rx.t(Msg::RouteMenuNoRoutes), sub);
            return;
        }

        let sel = self.selected.min(total - 1);
        let first = list::window_start(sel, geo.visible, total) as i32;
        list::draw_rows(cv, geo, total, sel, first, |cv, row| {
            let accent = if row.selected { INK } else { SUBTEXT };
            match rows[row.index] {
                Row::Folder(ti) => draw_folder_row(cv, &row.area, &trips[ti], w, accent, rx),
                Row::Route(ri) => draw_route_row(cv, &row.area, &routes[ri], w, accent),
            }
        });
    }
}

/// The climb column's x, recovered from the row area's left edge (shared so folder + route rows line
/// their climb figures up down the list).
fn climb_col_x(area_x: i32, w: i32) -> i32 {
    area_x + (w - 2 * SIDE_INSET) * CLIMB_COL_PCT / 100
}

/// A standard route row: name on line 1, distance under it, and the climb group (`▲` + metres) at
/// the fixed second column. Unchanged from the flat menu — used verbatim inside a folder.
fn draw_route_row(cv: &mut impl Surface, area: &Rectangle, route: &RouteSummary, w: i32, accent: u16) {
    use palette::*;
    let y = area.top_left.y;
    let name_x = area.top_left.x + NAME_INSET;
    let name_max = (((w - 20) - name_x) / Font::Body.char_width() as i32).max(6) as usize;
    let name = fit_name(&route.name, name_max);
    cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

    let sy = y + 35;
    let mut dist: heapless::String<12> = heapless::String::new();
    let _ = write!(dist, "{} km", route.distance_km);
    cv.text(&dist, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);

    climb_group(cv, climb_col_x(area.top_left.x, w), sy, route.climb_m, accent);
}

/// Draw the climb group — an up-triangle (the panel font has no `↑` glyph, so climb is *always* a
/// drawn triangle) at `x` with `{m} m` just right of it — and return the x past its text, so a
/// caller flowing an inline run can continue after it. Shared by the route rows and the folder rows.
fn climb_group(cv: &mut impl Surface, x: i32, sy: i32, climb_m: u32, accent: u16) -> i32 {
    cv.triangle(Point::new(x, sy + 14), Point::new(x + 9, sy + 14), Point::new(x + 4, sy + 5), accent);
    let mut climb: heapless::String<12> = heapless::String::new();
    let _ = write!(climb, "{climb_m} m");
    cv.text(&climb, Point::new(x + 16, sy), Font::Label, TextAlign::Left, accent);
    x + 16 + text_width(&climb, Font::Label) as i32
}

/// A trip **folder** row, drawn in the active [`FOLDER_ROW_STYLE`]. `t` is the resolved trip
/// (member count = `stage_indices.len()`, summed `distance_km` / `climb_m` over the resolvable
/// stages). An empty folder (all refs dangled) reads `0 routes`.
fn draw_folder_row(cv: &mut impl Surface, area: &Rectangle, t: &TripSummary, w: i32, accent: u16, rx: &Render) {
    use palette::*;
    let y = area.top_left.y;
    let n = t.stage_indices.len();
    // "1 route" vs "N routes" — the singular/plural word from the i18n catalog.
    let word = if n == 1 { rx.t(Msg::RouteMenuFolderRoute) } else { rx.t(Msg::RouteMenuFolderRoutes) };

    match FOLDER_ROW_STYLE {
        FolderRowStyle::A => {
            // Leading folder glyph; the name shifts right of it, with the summed distance right-
            // aligned on line 1. Line 2: the count on the left, the climb group at the route-row
            // climb column so it lines up down the list.
            let gx = area.top_left.x + NAME_INSET;
            folder_glyph(cv, gx, y + 16, accent);
            let name_x = gx + FOLDER_GLYPH_W + 6;

            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", t.distance_km);
            let dist_w = text_width(&dist, Font::Label) as i32;
            cv.text(&dist, Point::new(w - 20, y + 12), Font::Label, TextAlign::Right, accent);

            let name_max = (((w - 26 - dist_w) - name_x) / Font::Body.char_width() as i32).max(4) as usize;
            let name = fit_name(&t.name, name_max);
            cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

            let sy = y + 35;
            let mut count: heapless::String<16> = heapless::String::new();
            let _ = write!(count, "{n} {word}");
            cv.text(&count, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);
            climb_group(cv, climb_col_x(area.top_left.x, w), sy, t.climb_m, accent);
        }
        FolderRowStyle::B => {
            // No glyph; the **full name** gets line 1 (the priority — never crowded by a right-hand
            // stat). Line 2 is layout-distinct from a route row: the count · distance on the left,
            // the climb group right-aligned — a folder's headline is how many routes + how far, so
            // this variant leads with those and drops the two-column route grammar.
            let name_x = area.top_left.x + NAME_INSET;
            let name_max = (((w - 20) - name_x) / Font::Body.char_width() as i32).max(6) as usize;
            let name = fit_name(&t.name, name_max);
            cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

            let sy = y + 35;
            let mut meta: heapless::String<32> = heapless::String::new();
            let _ = write!(meta, "{n} {word} \u{00b7} {} km", t.distance_km);
            cv.text(&meta, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);
        }
        FolderRowStyle::C => {
            // A drawn folder-tab icon + a rounded count badge on the name line; line 2 carries the
            // summed km / ↑m in the SAME two columns as a route row, so the stats align down the list.
            let gx = area.top_left.x + NAME_INSET;
            folder_glyph(cv, gx, y + 16, accent);
            let name_x = gx + FOLDER_GLYPH_W + 6;

            // The count badge, right-aligned on line 1: a rounded wood pill with the number in
            // parchment, so a folder wears its size like a chip.
            let mut nbuf: heapless::String<8> = heapless::String::new();
            let _ = write!(nbuf, "{n}");
            let badge_w = (nbuf.len() as i32) * Font::Label.char_width() as i32 + 14;
            let badge_x = w - 20 - badge_w;
            cv.round(rect(badge_x, y + 8, badge_w, 20), 5, WOOD);
            cv.text(&nbuf, Point::new(badge_x + badge_w / 2, y + 11), Font::Label, TextAlign::Center, PARCHMENT);

            let name_max = (((badge_x - 6) - name_x) / Font::Body.char_width() as i32).max(4) as usize;
            let name = fit_name(&t.name, name_max);
            cv.text(&name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, INK);

            // Line 2: distance in col 1, climb group in col 2 — the route-row layout.
            let sy = y + 35;
            let mut dist: heapless::String<12> = heapless::String::new();
            let _ = write!(dist, "{} km", t.distance_km);
            cv.text(&dist, Point::new(name_x, sy), Font::Label, TextAlign::Left, accent);
            climb_group(cv, climb_col_x(area.top_left.x, w), sy, t.climb_m, accent);
        }
    }
}

/// Total width (px) the [`folder_glyph`] occupies, so callers can inset the name past it.
const FOLDER_GLYPH_W: i32 = 18;

/// Draw a small **folder** pictogram — a rounded body with a tab on its top-left — centred vertically
/// on `cy`, its left edge at `x`, in `color`. Hand-plotted in the panel's own glyph idiom (like the
/// climb triangles / BLE rune) so it quantizes at the device's pixel scale.
fn folder_glyph(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    // The tab: a short rounded nub sitting on top of the body's back edge.
    cv.round(rect(x + 1, cy - 8, 8, 4), 1, color);
    // The body: a rounded rectangle, with a thin parchment "lid" line so it reads as a folder, not a
    // plain box, at 18×12.
    cv.round(rect(x, cy - 5, FOLDER_GLYPH_W, 12), 2, color);
    cv.hline(x + 2, cy - 2, FOLDER_GLYPH_W - 4, palette::PARCHMENT);
}

/// Fit a route name into `max_chars`, appending ".." when truncated (no ellipsis glyph).
/// Truncates on a char boundary. Shared with the Route overview's title, the swap card, and the
/// trip-delete confirm.
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
    use crate::activity::{Activity, Mode};
    use crate::trip::TripInput;
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

    /// A resolved trip grouping the given catalog indices (stage ids == indices here — the positional
    /// id case), against a catalog whose ids equal their indices.
    fn trip(id: u16, name: &str, stages: &[u16], catalog: &[RouteSummary]) -> TripSummary {
        let ids: heapless::Vec<u16, { crate::route::MAX_ROUTES }> = (0..catalog.len() as u16).collect();
        TripSummary::resolve(&TripInput { id, name, stage_ids: stages }, catalog, &ids)
    }

    fn run(
        scr: &mut RouteMenuScreen,
        act: &mut Activity,
        routes: &[RouteSummary],
        trips: &[TripSummary],
        g: Gesture,
    ) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: act,
            settings: &mut settings,
            routes,
            rides: &[],
            trips,
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Picking a route **mid route-less ride** goes through the guarded swap flow — unchanged by the
    /// trip rework (a route row is a route row).
    #[test]
    fn picking_a_route_during_a_route_less_ride_opens_the_swap() {
        let routes = [summary("A"), summary("B")];
        let mut act = Activity::new(Mode::Riding);
        act.start_session();
        assert_eq!(act.active_route, None);
        let mut scr = RouteMenuScreen::new();
        let t = run(&mut scr, &mut act, &routes, &[], Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteSwap(_))), "the guarded swap card opens");
        assert_eq!(act.active_route, None);
    }

    /// The top level lists folders first, then only the **unfiled** routes: with a trip grouping
    /// routes 0 + 1, row 0 is the folder and row 1 is the loose route 2. Pressing the folder opens a
    /// trip-scoped stage list; pressing the loose route opens its overview at the right catalog index.
    #[test]
    fn top_level_lists_folders_then_unfiled_routes() {
        let routes = [summary("A"), summary("B"), summary("C")];
        let trips = [trip(9, "Trip", &[0, 1], &routes)];
        // Row 0 = folder → press pushes a trip-scoped menu.
        let mut scr = RouteMenuScreen::new();
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, &routes, &trips, Gesture::Press);
        assert!(
            matches!(t, Transition::Push(Screen::RouteMenu(_))),
            "pressing a folder opens its stage list (a scoped Route menu)"
        );
        // Row 1 = the one unfiled route (index 2) → its overview, active_route = 2.
        let mut scr = RouteMenuScreen::new();
        run(&mut scr, &mut Activity::new(Mode::Idle), &routes, &trips, Gesture::Turn(1)); // → row 1
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))), "the loose route opens its overview");
        assert_eq!(act.active_route, Some(2), "and it's the unfiled catalog route (index 2), not a filed one");
    }

    /// Long-pressing a folder opens the cascade-delete confirm carrying the trip's durable id.
    #[test]
    fn long_press_folder_opens_the_delete_confirm() {
        let routes = [summary("A"), summary("B")];
        let trips = [trip(42, "Trip", &[0, 1], &routes)];
        let mut scr = RouteMenuScreen::new();
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &routes, &trips, Gesture::Hold);
        assert!(matches!(t, Transition::Push(Screen::TripDelete(_))), "a folder long-press confirms a cascade delete");
    }

    /// A long-press on a **route** row does nothing (top-level routes delete from the Route overview).
    #[test]
    fn long_press_route_does_nothing() {
        let routes = [summary("A")];
        let mut scr = RouteMenuScreen::new();
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &routes, &[], Gesture::Hold);
        assert!(matches!(t, Transition::None));
    }

    /// Inside a folder the rows are the trip's member routes at their real catalog indices; pressing
    /// one opens that exact route — nothing trip-aware downstream.
    #[test]
    fn a_folder_stage_list_presses_the_member_route() {
        let routes = [summary("A"), summary("B"), summary("C")];
        let trips = [trip(9, "Trip", &[1, 2], &routes)]; // members: catalog indices 1, 2
        let mut scr = RouteMenuScreen::trip(9);
        let mut act = Activity::new(Mode::Idle);
        let t = run(&mut scr, &mut act, &routes, &trips, Gesture::Press); // row 0 → member 1
        assert!(matches!(t, Transition::Push(Screen::RouteOverview(_))));
        assert_eq!(act.active_route, Some(1), "the first stage is catalog route 1");
    }

    /// An empty folder (all refs dangling) shows no rows; Back pops out and a press is inert.
    #[test]
    fn empty_folder_has_no_rows_and_backs_out() {
        let routes = [summary("A")];
        let trips = [trip(9, "Trip", &[99], &routes)]; // the only ref dangles
        assert!(trips[0].is_empty_folder());
        let mut scr = RouteMenuScreen::trip(9);
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &routes, &trips, Gesture::Back);
        assert!(matches!(t, Transition::Pop), "Back leaves the empty folder");
        let mut scr = RouteMenuScreen::trip(9);
        let t = run(&mut scr, &mut Activity::new(Mode::Idle), &routes, &trips, Gesture::Press);
        assert!(matches!(t, Transition::None), "a press in an empty folder does nothing");
    }
}
