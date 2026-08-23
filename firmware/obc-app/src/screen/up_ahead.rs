//! The **"Up ahead" timeline** (epic #946, U3) — the ride compass's north station, replacing the
//! plain waypoint list. One route-ordered list answering one question: *what is coming up on my
//! route?* Two sources feed it and neither is copied:
//!
//! * the resident **waypoint table** ([`Waypoints`](obc_route::Waypoints)) the route loader owns —
//!   now categorized (U1), so a `<sym>`-tagged GPX waypoint carries a [`PoiCategory`], and
//! * the frozen **route-corridor POI snapshot** ([`CorridorScratch`](crate::corridor::CorridorScratch))
//!   the App owns (U2) — the map POIs within 300 m of the route ahead, projected onto the same
//!   along-route axis.
//!
//! # One list, one axis
//!
//! Both sources place an entry at a `dist_along_m`, so the merge is a two-finger walk over the two
//! slices ([`Merge`]) — **no sections, no headers, and no merged copy anywhere**: the screen holds
//! a cursor, a category filter and its snapshot anchor, nothing else (the #425 rule — a `Screen`
//! variant is a slot in a `.bss` union, so a buffer here would multiply across the whole stack).
//! Provenance is carried per row instead: a custom waypoint's icon draws in
//! [`AMBER`](super::palette::AMBER) with a small diamond pip beside it, a map POI's in the
//! browser's [`SUBTEXT`](super::palette::SUBTEXT)/[`INK`](super::palette::INK) scheme.
//!
//! # The snapshot lifecycle
//!
//! The screen never queries anything itself. It *declares* what it wants —
//! [`corridor_key`](UpAheadScreen::corridor_key), the `(filter, anchor)` pair — and
//! [`App`](crate::App) arms the App-owned scratch from that declaration whenever the stack settles
//! ([`UiRuntime::reconcile_corridor`](crate::ui_runtime::UiRuntime)). The anchor is **live progress
//! frozen at entry**, so riding on never re-runs the query and rows can never shift under the
//! cursor; applying a filter changes the key, which is what re-queries. Closing the screen drops
//! the request and the reader seam goes quiet.
//!
//! # The source scope (U4)
//!
//! *Who* feeds the list is a stable preference, not a moment-to-moment intent, so it lives in Ride
//! settings as [`UpAheadSource`] and is read **once, at entry** — the same freeze as the anchor
//! (the setting can only be edited from a screen this one is not under, so live-reading it would
//! differ from freezing it only in theory). Under **Waypoints only** the screen declares *no*
//! corridor key at all: nothing is armed, the query never runs, and
//! [`base_needs_reader`](crate::App::base_needs_reader) never asks the board to build a map
//! `Reader` for it — the list costs exactly what the plain waypoint list cost. Under **Map POIs
//! only** the key is declared as usual and the resident waypoint table is simply not walked.
//!
//! # Hold opens the category picker
//!
//! `Hold` swaps the list body for a 7-row picker (Everything + the six categories, the POI-menu
//! row style and icons) — the same "hold opens this screen's own config" idiom as the map's pan
//! mode, advertised by the global hold-hint bulge. `Press` applies (and re-keys the snapshot),
//! `Back` cancels. The list always **opens on Everything**: predictable beats sticky.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_formats::obcm::{poi_category_of, poi_label_of};
use obc_reader::{CorridorPoi, PoiCategory, PoiCategorySet};
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};
use obc_route::{Profile, WptEntry};

use crate::corridor::CorridorKey;
use crate::input::Gesture;
use crate::settings::{Units, UpAheadSource};
use crate::Msg;

use super::vocab::chrome::empty_state;
use super::vocab::list::{self, ListGeometry, Separators};
use super::vocab::tiles::fit_caption;
use super::{palette, poi_menu::draw_category_icon, Ctx, Render, Screen, Transition};

/// The two-line row pitch: icon + name above, distance-to-go + climb-to-go (+ the off-route side
/// hint) below. Inherited unchanged from the waypoint list this screen replaces, so the ride
/// menu's north station keeps its rhythm.
const ROW_H: i32 = 66;
const SIDE_INSET: i32 = 12;
/// Left inset of the row's ~22 px icon box from the row area. Tight on purpose: at Body's 14 px
/// per char the 240 px panel only fits ~17 characters of name, and every pixel the gutter takes is
/// a character of "Drinking water" the row can't show.
const ICON_INSET: i32 = 2;
/// Where the name starts — just past the icon box.
const NAME_INSET: i32 = 28;
/// Right margin the name keeps from the row area's edge.
const NAME_TAIL: i32 = 4;
/// Left inset of line 2's first column (the distance) — a hair in from the icon so the two lines
/// read as one block without the numbers hugging the amber cursor's rounded corner.
const LINE2_INSET: i32 = 8;
/// Nominal x of the climb column, as a percentage of the row area's width. The column slides left
/// when an off-route hint claims the right edge (see [`draw_row`]).
const CLIMB_COL_PCT: i32 = 55;

/// Lateral offset (m) an entry must exceed before the row shows a side hint. **Tunable** (epic
/// #946, amended 2026-07-28): below it the entry is "on the route" for a rider's purposes and the
/// hint would be noise on every row; above it the detour is a decision. Sim-validated against the
/// 300 m corridor half-width, which is this hint's natural maximum for a map POI.
pub const OFF_ROUTE_HINT_M: i32 = 50;

/// Half-width (px) of the row's side-arrow glyph.
pub(super) const ARROW_W: i32 = 7;
/// Gap (px) between the side arrow and its distance.
pub(super) const ARROW_GAP: i32 = 4;

/// Nominal row height in the picker. Tighter than the POI menu's 52 px on purpose: at that pitch
/// only five of the seven rows fit the panel, and a *filter* the rider has to scroll to reach is a
/// filter they won't use. 38 px fits all seven flush, still a full Body row with the same icon.
const PICKER_ROW_H: i32 = 38;
/// Picker rows: "Everything" plus the six categories.
const PICKER_ROWS: usize = 1 + PoiCategory::ALL.len();

/// One row of the merged timeline: a custom waypoint from the resident table, or a map POI from
/// the corridor snapshot. Borrowed — the merge never copies (see the module docs).
#[derive(Clone, Copy)]
pub(crate) enum Entry<'a> {
    /// A route waypoint (the rider's own plan). `category` is `None` for a generic waypoint.
    Waypoint(&'a WptEntry),
    /// A map POI projected onto the route by the corridor query.
    Poi(&'a CorridorPoi),
}

impl<'a> Entry<'a> {
    /// Where this entry sits on the route axis (meters from the route start) — the merge key.
    fn dist_along_m(&self) -> u32 {
        match self {
            Entry::Waypoint(w) => w.dist_along_m,
            Entry::Poi(p) => p.dist_along_m,
        }
    }

    /// The row's name: the waypoint's stored name, or the POI's name falling back to its subtype
    /// label (the POI browser's fallback, so a row reads the same in both lists).
    fn name(&self) -> &'a str {
        match self {
            Entry::Waypoint(w) => w.name.as_str(),
            Entry::Poi(p) => poi_row_name(&p.poi),
        }
    }

    /// The entry's category, or `None` for a **generic** waypoint (the diamond).
    fn category(&self) -> Option<PoiCategory> {
        match self {
            Entry::Waypoint(w) => w.category,
            Entry::Poi(p) => poi_category_of(p.poi.subtype),
        }
    }

    /// Signed lateral offset from the route line (m); positive = right of the direction of travel.
    /// Waypoints carry it in the OBCR record (U1), POIs get it from the projection (U2).
    fn offset_m(&self) -> i32 {
        match self {
            Entry::Waypoint(w) => w.lateral_offset_m as i32,
            Entry::Poi(p) => p.offset_m,
        }
    }

    /// Whether this is a custom waypoint — the source cue (amber icon + diamond pip).
    fn is_waypoint(&self) -> bool {
        matches!(self, Entry::Waypoint(_))
    }

    /// Whether `filter` keeps this entry. A generic waypoint has no category, so it belongs to
    /// "Everything" only — a category filter is a question it can't answer.
    fn kept_by(&self, filter: PoiCategorySet) -> bool {
        match self.category() {
            Some(c) => filter.contains(c),
            None => filter == PoiCategorySet::ALL,
        }
    }
}

/// How a map POI names itself in a route-ordered readout: its own name, or — unnamed — its subtype
/// label (the POI browser's fallback), so it reads the same on an Up-ahead row as in a
/// [`Next: <category>` tile](crate::stat_fields::StatField). Never empty.
pub(crate) fn poi_row_name(poi: &obc_reader::Poi) -> &str {
    if poi.name.is_empty() {
        poi_label_of(poi.subtype).unwrap_or("POI")
    } else {
        poi.name.as_str()
    }
}

/// The route-ordered merge of the two source tables — a two-finger walk, allocating nothing and
/// copying nothing. Both inputs are already ascending by `dist_along_m` (the loader sorts the
/// waypoint table, the corridor query emits in route order), so one pass is enough.
///
/// Ties break **waypoint first**: at the same spot the rider's own plan entry outranks a map POI.
pub(crate) struct Merge<'a> {
    waypoints: &'a [WptEntry],
    pois: &'a [CorridorPoi],
    filter: PoiCategorySet,
    i: usize,
    j: usize,
}

impl<'a> Merge<'a> {
    pub(crate) fn new(waypoints: &'a [WptEntry], pois: &'a [CorridorPoi], filter: PoiCategorySet) -> Self {
        Merge { waypoints, pois, filter, i: 0, j: 0 }
    }
}

impl<'a> Iterator for Merge<'a> {
    type Item = Entry<'a>;

    fn next(&mut self) -> Option<Entry<'a>> {
        // Skip whatever the filter drops on each side first, so the comparison below only ever
        // sees rows that will actually be drawn.
        while self.i < self.waypoints.len() && !Entry::Waypoint(&self.waypoints[self.i]).kept_by(self.filter) {
            self.i += 1;
        }
        while self.j < self.pois.len() && !Entry::Poi(&self.pois[self.j]).kept_by(self.filter) {
            self.j += 1;
        }
        let wp = self.waypoints.get(self.i).map(Entry::Waypoint);
        let poi = self.pois.get(self.j).map(Entry::Poi);
        match (wp, poi) {
            (Some(w), Some(p)) if p.dist_along_m() < w.dist_along_m() => {
                self.j += 1;
                Some(p)
            }
            (Some(w), _) => {
                self.i += 1;
                Some(w)
            }
            (None, Some(p)) => {
                self.j += 1;
                Some(p)
            }
            (None, None) => None,
        }
    }
}

/// The waypoint table the list may walk: empty unless a route is loaded **and** the rider's source
/// scope includes custom waypoints. A route-less ride must never leak the previous route's resident
/// cache — the empty state covers it.
fn scoped_waypoints(waypoints: &[WptEntry], source: UpAheadSource, route_loaded: bool) -> &[WptEntry] {
    if route_loaded && source.shows_waypoints() {
        waypoints
    } else {
        &[]
    }
}

/// The corridor snapshot the list may walk — the twin of [`scoped_waypoints`]. Belt-and-braces
/// under [`WaypointsOnly`](UpAheadSource::WaypointsOnly): the scratch is already disarmed there
/// (so the slice is empty anyway), but a snapshot another consumer armed must never leak into a
/// list the rider scoped away from map POIs.
fn scoped_corridor(corridor: &[CorridorPoi], source: UpAheadSource, route_loaded: bool) -> &[CorridorPoi] {
    if route_loaded && source.shows_pois() {
        corridor
    } else {
        &[]
    }
}

/// Pure route-relative figures for one row. The distance axis is the entry's `dist_along_m` against
/// matched activity progress. Remaining ascent is the cached profile's own
/// [`ascent_between_m`](obc_route::Profile::ascent_between_m) over those two along-route distances —
/// not entry elevation (which may be absent or off the line), and not coarse chunk metadata. That
/// is the shared climb-between-two-points lookup the `TO CLIMB` tile and the ETA model read too
/// (elevation epic #1068, EL9), so no two readouts can disagree about the same stretch of route.
/// Identical math for both sources: a corridor POI *is* a point on the route axis, so the waypoint
/// arithmetic applies unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Figures {
    pub passed: bool,
    pub distance_m: u32,
    pub climb_m: Option<u32>,
}

pub(crate) fn figures(dist_along_m: u32, progress_m: u32, route_total_m: u32, profile: Option<&Profile>) -> Figures {
    let passed = progress_m > dist_along_m;
    let distance_m = dist_along_m.saturating_sub(progress_m);
    let climb_m =
        profile.filter(|_| route_total_m > 0).map(|p| p.ascent_between_m(progress_m, dist_along_m, route_total_m));
    Figures { passed, distance_m, climb_m }
}

/// The remaining-ascent readout, compact (`250m`) — the row's third column shares line 2 with the
/// off-route hint, so the unit rides tight against the number.
fn write_climb(value_m: Option<u32>, units: Units) -> heapless::String<12> {
    let mut s = heapless::String::new();
    match value_m {
        Some(m) => {
            let shown = (units.elev(m as f32) + 0.5) as u32;
            let _ = write!(s, "{shown}{}", units.elev_label());
        }
        None => {
            let _ = s.push_str("--");
        }
    }
    s
}

/// The off-route side hint for `offset_m`, or `None` inside [`OFF_ROUTE_HINT_M`]. `true` = the
/// entry sits to the **right** of the direction of travel.
fn side_hint(offset_m: i32, units: Units) -> Option<(bool, heapless::String<12>)> {
    if offset_m.unsigned_abs() as i32 <= OFF_ROUTE_HINT_M {
        return None;
    }
    let mut s = heapless::String::new();
    super::write_off_route(&mut s, "", offset_m.unsigned_abs(), units);
    Some((offset_m > 0, s))
}

/// The Up-ahead timeline. State is the cursor, the category filter, the snapshot anchor, and the
/// picker's cursor while it is open — **no rows**: they're iterated from the two App-owned tables
/// on every draw (#425).
#[derive(Debug)]
pub struct UpAheadScreen {
    /// The highlighted merged row, or `None` before the first frame resolves "the first unpassed
    /// entry" (the merge — and therefore that index — isn't knowable when the ride menu pushes
    /// this screen: the corridor snapshot lands a frame later).
    selected: Option<usize>,
    /// The active category filter; [`PoiCategorySet::ALL`] is "Everything", which the list always
    /// opens on.
    filter: PoiCategorySet,
    /// Live route progress (m) frozen at entry — the corridor snapshot's anchor.
    anchor_m: u32,
    /// Which sources feed the list (epic #946, U4), read from Ride settings at entry — see the
    /// module docs. Also decides whether a corridor snapshot is requested at all.
    source: UpAheadSource,
    /// The picker's highlighted row while it is open (`0` = Everything, `1..=6` = the categories).
    picker: Option<usize>,
}

impl UpAheadScreen {
    /// Open the timeline anchored at `anchor_m` (live progress at entry), unfiltered, showing the
    /// sources `source` scopes it to (the rider's Ride-settings preference, frozen here).
    pub fn new(anchor_m: u32, source: UpAheadSource) -> Self {
        UpAheadScreen { selected: None, filter: PoiCategorySet::ALL, anchor_m, source, picker: None }
    }

    /// What corridor snapshot this screen wants — read by
    /// [`reconcile_corridor`](crate::ui_runtime::UiRuntime::reconcile_corridor) whenever the stack
    /// settles. Stable across frames (the anchor is frozen), so re-arming is a no-op until the
    /// filter changes. `None` when the source scope excludes map POIs
    /// ([`WaypointsOnly`](UpAheadSource::WaypointsOnly)): the scratch then stays **disarmed**, so
    /// neither the query nor the board's `Reader` build ever runs for this screen.
    pub(crate) fn corridor_key(&self) -> Option<CorridorKey> {
        self.source.shows_pois().then_some(CorridorKey { filter: self.filter, anchor_m: self.anchor_m })
    }

    /// The merged rows for this frame's tables, in route order, already source-scoped and filtered.
    fn rows<'a>(&self, waypoints: &'a [WptEntry], corridor: &'a [CorridorPoi], route_loaded: bool) -> Merge<'a> {
        Merge::new(
            scoped_waypoints(waypoints, self.source, route_loaded),
            scoped_corridor(corridor, self.source, route_loaded),
            self.filter,
        )
    }

    /// The effective cursor: the rider's, or — before they've turned — the first row still ahead
    /// (today's `next_waypoint` behavior, generalized to the merged list).
    fn cursor(&self, rows: Merge, progress_m: u32, total: usize) -> usize {
        match self.selected {
            Some(s) => s.min(total.saturating_sub(1)),
            None => rows
                .enumerate()
                .find(|(_, e)| progress_m <= e.dist_along_m())
                .map(|(i, _)| i)
                .unwrap_or(0)
                .min(total.saturating_sub(1)),
        }
    }

    /// The picker row showing the active filter, so it opens on what is already on.
    fn picker_row(&self) -> usize {
        PoiCategory::ALL.iter().position(|c| self.filter == PoiCategorySet::only(*c)).map_or(0, |i| i + 1)
    }

    /// The filter a picker row selects.
    fn filter_of_row(row: usize) -> PoiCategorySet {
        match row {
            0 => PoiCategorySet::ALL,
            r => PoiCategorySet::only(PoiCategory::ALL[(r - 1).min(PoiCategory::ALL.len() - 1)]),
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        // The picker owns every gesture while it is open — it is a mode, not an overlay.
        if let Some(row) = self.picker {
            match g {
                Gesture::Step(n) => {
                    let mut r = row;
                    list::on_step(&mut r, n, PICKER_ROWS);
                    self.picker = Some(r);
                }
                Gesture::Press => {
                    // Apply: the new filter re-keys the corridor snapshot (the App re-arms from
                    // `corridor_key`, which drops the stale rows and re-queries), and the cursor
                    // re-homes to the first entry still ahead in the new list.
                    self.filter = Self::filter_of_row(row);
                    self.selected = None;
                    self.picker = None;
                }
                Gesture::Back => self.picker = None, // cancel: the filter is untouched
                Gesture::Hold | Gesture::BackHold => {}
            }
            return Transition::None;
        }

        let route_loaded = cx.activity.active_route.is_some();
        let total = self.rows(cx.waypoints, cx.corridor, route_loaded).count();
        let mut sel = self.cursor(self.rows(cx.waypoints, cx.corridor, route_loaded), cx.activity.progress_m, total);
        match g {
            // A step commits the cursor — but only over a list that actually has rows. Stepping an
            // empty one (the snapshot hasn't landed, or the route has nothing ahead) must stay a
            // no-op *and* leave the cursor unresolved, or the first turn before the first frame
            // would pin it at row 0 and quietly lose the open-at-the-first-unpassed-entry homing.
            Gesture::Step(n) if total > 0 => {
                list::on_step(&mut sel, n, total);
                self.selected = Some(sel);
                Transition::None
            }
            Gesture::Step(_) => Transition::None,
            // A POI row opens the existing detail screen, carrying its signed off-route offset so
            // the detail can spell the side out. A custom waypoint has no detail child yet — the
            // row stays inert rather than advertising a screen that doesn't exist (copy-tone rule).
            Gesture::Press => match self.rows(cx.waypoints, cx.corridor, route_loaded).nth(sel) {
                Some(Entry::Poi(p)) => Transition::Push(Screen::PoiDetail(
                    super::PoiDetailScreen::new(p.poi.clone()).off_route(p.offset_m),
                )),
                _ => Transition::None,
            },
            // Hold opens this screen's own config — the stats/fields and map/pan-mode idiom, which
            // the global hold-hint bulge advertises.
            Gesture::Hold => {
                self.picker = Some(self.picker_row());
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        if let Some(row) = self.picker {
            let names = PoiCategory::ALL.map(|c| rx.t(super::poi_menu::category_msg(c)));
            draw_picker(cv, rx.w, rx.h, rx.t(Msg::UpAheadFilterTitle), rx.t(Msg::UpAheadEverything), names, row);
            return;
        }
        let route_loaded = rx.activity.active_route.is_some();
        let total = self.rows(rx.waypoints.as_slice(), rx.corridor, route_loaded).count();
        draw_list(
            cv,
            &View {
                w: rx.w,
                h: rx.h,
                title: rx.t(Msg::RideMenuUpAhead),
                copy: EmptyCopy {
                    no_route: rx.t(Msg::RideMenuNoRoute),
                    no_route_sub: rx.t(Msg::UpAheadNoRouteSub),
                    none: rx.t(Msg::UpAheadNone),
                    none_sub: rx.t(Msg::UpAheadNoneSub),
                    none_category_sub: rx.t(Msg::UpAheadNoneCategorySub),
                    none_waypoints_sub: rx.t(Msg::UpAheadNoneWaypointsSub),
                    none_pois_sub: rx.t(Msg::UpAheadNonePoisSub),
                },
                waypoints: rx.waypoints.as_slice(),
                corridor: rx.corridor,
                filter: self.filter,
                source: self.source,
                route_loaded,
                settled: rx.corridor_settled,
                progress_m: rx.activity.progress_m,
                route_total_m: rx.activity.route_total_m,
                profile: rx.profile,
                units: rx.settings.units,
                cursor: self.cursor(
                    self.rows(rx.waypoints.as_slice(), rx.corridor, route_loaded),
                    rx.activity.progress_m,
                    total,
                ),
                total,
            },
        );
    }
}

/// The catalog copy for the three empty states, resolved once by the caller so the draw itself is a
/// pure function of plain values (which is what makes the render tests cheap).
struct EmptyCopy<'a> {
    no_route: &'a str,
    no_route_sub: &'a str,
    none: &'a str,
    none_sub: &'a str,
    none_category_sub: &'a str,
    /// The sub-line under a source scope of "waypoints only" — the generic `none_sub` ("no stops on
    /// route") would be a **lie** there: the route may be lined with map POIs the rider scoped away.
    none_waypoints_sub: &'a str,
    /// The same, the other way round (map POIs only, and the corridor came back empty).
    none_pois_sub: &'a str,
}

/// Everything one Up-ahead frame draws from: the two source slices, the filter, the route-relative
/// axis, and the resolved cursor. Built from [`Render`] by [`UpAheadScreen::draw`]; built directly
/// by the render tests.
struct View<'a> {
    w: i32,
    h: i32,
    title: &'a str,
    copy: EmptyCopy<'a>,
    waypoints: &'a [WptEntry],
    corridor: &'a [CorridorPoi],
    filter: PoiCategorySet,
    /// The rider's source scope (epic #946, U4) — which of the two slices above may feed the list,
    /// and which empty-state sub-line tells the truth when it doesn't.
    source: UpAheadSource,
    route_loaded: bool,
    /// Whether the corridor snapshot has landed (or settled empty on a query error, U2) — `false`
    /// only while the query is still waiting for its inputs.
    settled: bool,
    progress_m: u32,
    route_total_m: u32,
    profile: Option<&'a Profile>,
    units: Units,
    cursor: usize,
    total: usize,
}

impl View<'_> {
    fn rows(&self) -> Merge<'_> {
        Merge::new(
            scoped_waypoints(self.waypoints, self.source, self.route_loaded),
            scoped_corridor(self.corridor, self.source, self.route_loaded),
            self.filter,
        )
    }
}

fn draw_list(cv: &mut impl Surface, v: &View) {
    let geo = ListGeometry::below_title(v.w, v.h, ROW_H, 8, SIDE_INSET, Separators::Unselected);
    let pos = if v.total == 0 { 0 } else { v.cursor + 1 };
    list::list_frame(cv, v.w, v.h, v.title, pos, v.total, geo.visible);

    if v.total == 0 {
        // The distinct empty states. Order matters: "no route" outranks everything (the list is
        // route-relative by definition), and a snapshot that hasn't landed yet draws **nothing** —
        // a transient one-frame state, not an answer. A query that *errored* settles empty (U2), so
        // it lands on the honest "nothing ahead" rather than spinning forever. Under "Waypoints
        // only" nothing is ever armed, so `settled` is true from the first frame and the list
        // resolves at once.
        //
        // Below that, the *category* filter outranks the *source* scope: the filter is what the
        // rider just did (a Hold, a row, a Press), the scope is a preference they set some other
        // day — so "None of this kind" is the more immediate truth. With no filter on, the sub-line
        // names the scope, because the plain "no stops on route" is only true when **both** sources
        // were allowed to answer (U4).
        if !v.route_loaded {
            empty_state(cv, v.w, v.h, v.copy.no_route, v.copy.no_route_sub);
        } else if !v.settled {
            // Still waiting for the map reader / route geometry — say nothing for the frame or two
            // it takes rather than flashing "nothing ahead" at a list that is about to fill.
        } else {
            let sub = if v.filter != PoiCategorySet::ALL {
                v.copy.none_category_sub
            } else {
                match v.source {
                    UpAheadSource::Both => v.copy.none_sub,
                    UpAheadSource::WaypointsOnly => v.copy.none_waypoints_sub,
                    UpAheadSource::MapPoisOnly => v.copy.none_pois_sub,
                }
            };
            empty_state(cv, v.w, v.h, v.copy.none, sub);
        }
        return;
    }

    let first = list::window_start(v.cursor, geo.visible, v.total);
    // Advance the merge to the window's first row once, then draw forward — the iterator is the
    // only cursor into either table, so no row is ever materialized twice.
    let mut rows = v.rows();
    let mut entry = rows.nth(first);
    list::draw_rows(cv, geo, v.total, v.cursor, first as i32, |cv, row| {
        if let Some(e) = entry {
            let f = figures(e.dist_along_m(), v.progress_m, v.route_total_m, v.profile);
            draw_row(cv, e, &row, f, v.units);
        }
        entry = rows.next();
    });
}

/// Draw one timeline row. Line 1 is the category icon (a diamond for a generic waypoint) plus the
/// ellipsized name; line 2 the distance-to-go, the climb-to-go, and — past
/// [`OFF_ROUTE_HINT_M`] — the side arrow at the right edge.
fn draw_row(cv: &mut impl Surface, entry: Entry, row: &list::RowCtx, values: Figures, units: Units) {
    use palette::*;

    // Passed rows stay muted even under the amber cursor: the highlight still locates the row while
    // both lines read as behind the plan.
    let name_color = if values.passed { SUBTEXT } else { INK };
    let stat_color = if values.passed || !row.selected { SUBTEXT } else { INK };
    // Source colour (epic #946): map POIs keep the browser's SUBTEXT/INK scheme; custom waypoints
    // draw AMBER — except on the amber cursor row, where amber-on-amber would vanish and INK takes
    // over. The diamond pip beside the icon carries the distinction in every state, so the cue is
    // never colour-only.
    let icon_color = if values.passed {
        SUBTEXT
    } else if row.selected {
        INK
    } else if entry.is_waypoint() {
        AMBER
    } else {
        SUBTEXT
    };
    let bg = if row.selected { AMBER } else { PARCHMENT };

    let x = row.area.top_left.x;
    let y = row.area.top_left.y;
    let right = x + row.area.size.width as i32;
    let icon_c = Point::new(x + ICON_INSET + 11, y + 9 + Font::Body.cap_height() as i32 / 2);
    match entry.category() {
        Some(cat) => draw_category_icon(cv, cat, icon_c, icon_color, bg),
        None => draw_diamond(cv, icon_c, 8, icon_color),
    }
    if entry.is_waypoint() {
        // The redundant, colourblind-safe source cue: a small pip riding the icon's upper right.
        draw_diamond(cv, Point::new(icon_c.x + 12, icon_c.y - 10), 3, icon_color);
    }

    let name_x = x + NAME_INSET;
    let mut name_buf: heapless::String<24> = heapless::String::new();
    let name = fit_caption(entry.name(), right - name_x - NAME_TAIL, &mut name_buf, Font::Body);
    cv.text(name, Point::new(name_x, y + 9), Font::Body, TextAlign::Left, name_color);

    // Line 2 — distance, climb, side hint. The three compete for a 216 px line on the 240 px panel,
    // so the climb column slides left of the hint block rather than colliding with it.
    let sy = y + 36;
    let dist = crate::stat_fields::fmt_dist_short(values.distance_m, units);
    cv.text(&dist, Point::new(x + LINE2_INSET, sy), Font::Label, TextAlign::Left, stat_color);
    let dist_end = x + LINE2_INSET + text_width(&dist, Font::Label) as i32;

    let hint = side_hint(entry.offset_m(), units);
    let hint_left = match &hint {
        Some((to_right, txt)) => {
            let tw = text_width(txt, Font::Label) as i32;
            let tx = right - 8 - tw;
            let ax = tx - ARROW_GAP - ARROW_W;
            draw_side_arrow(cv, Point::new(ax, sy + Font::Label.cap_height() as i32 / 2), *to_right, stat_color);
            cv.text(txt, Point::new(tx, sy), Font::Label, TextAlign::Left, stat_color);
            ax
        }
        None => right,
    };

    let climb = write_climb(values.climb_m, units);
    let climb_w = 16 + text_width(&climb, Font::Label) as i32;
    let climb_x = (x + row.area.size.width as i32 * CLIMB_COL_PCT / 100).min(hint_left - 8 - climb_w).max(dist_end + 8);
    cv.triangle(
        Point::new(climb_x, sy + 14),
        Point::new(climb_x + 9, sy + 14),
        Point::new(climb_x + 4, sy + 5),
        stat_color,
    );
    cv.text(&climb, Point::new(climb_x + 16, sy), Font::Label, TextAlign::Left, stat_color);
}

/// A filled diamond of half-diagonal `r` centred at `c` — the generic-waypoint glyph and the
/// custom-source pip, both drawn as the map's two-triangle rhombus so one shape means "waypoint"
/// everywhere.
fn draw_diamond(cv: &mut impl Surface, c: Point, r: i32, color: u16) {
    let (l, rt) = (Point::new(c.x - r, c.y), Point::new(c.x + r, c.y));
    cv.triangle(Point::new(c.x, c.y - r), l, rt, color);
    cv.triangle(Point::new(c.x, c.y + r), l, rt, color);
}

/// The off-route side arrow: a solid triangle pointing left or right, drawn (not typed) because
/// the device font's Latin strip carries no arrow glyph — a `←` would render as a silent `?`.
/// `at` is the block's left edge, vertically centred on `at.y`. Shared with the
/// [POI detail](super::PoiDetailScreen), which draws it beside the spelled-out side.
pub(super) fn draw_side_arrow(cv: &mut impl Surface, at: Point, to_right: bool, color: u16) {
    let (h, y) = (5, at.y);
    if to_right {
        cv.triangle(Point::new(at.x, y - h), Point::new(at.x, y + h), Point::new(at.x + ARROW_W, y), color);
    } else {
        cv.triangle(Point::new(at.x + ARROW_W, y - h), Point::new(at.x + ARROW_W, y + h), Point::new(at.x, y), color);
    }
}

/// The category picker: "Everything" over the six categories, in the POI menu's row style (icon in
/// a fixed gutter, name beside it) so the two controls read as one language.
fn draw_picker(
    cv: &mut impl Surface,
    w: i32,
    h: i32,
    title: &str,
    everything: &str,
    names: [&str; 6],
    selected: usize,
) {
    use palette::*;
    let geo = ListGeometry::filling_below_title(w, h, PICKER_ROW_H, 6, 16, Separators::All);
    list::list_frame(cv, w, h, title, selected + 1, PICKER_ROWS, geo.visible);
    let first = list::window_start(selected, geo.visible, PICKER_ROWS) as i32;
    list::draw_rows(cv, geo, PICKER_ROWS, selected, first, |cv, row| {
        let a = row.area;
        let mid = a.top_left.y + a.size.height as i32 / 2;
        let ink = if row.selected { INK } else { SUBTEXT };
        let bg = if row.selected { AMBER } else { PARCHMENT };
        let label = match row.index {
            0 => {
                // "Everything" has no icon of its own; the row's bullet is the shared list pointer.
                cv.triangle(
                    Point::new(a.top_left.x + 16, mid - 8),
                    Point::new(a.top_left.x + 16, mid + 8),
                    Point::new(a.top_left.x + 28, mid),
                    ink,
                );
                everything
            }
            r => {
                let cat = PoiCategory::ALL[r - 1];
                draw_category_icon(cv, cat, Point::new(a.top_left.x + 22, mid), ink, bg);
                names[r - 1]
            }
        };
        cv.text(label, Point::new(a.top_left.x + 44, mid - 14), Font::Body, TextAlign::Left, INK);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::harness::support::wpts_detailed;
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};
    use embedded_graphics::primitives::Rectangle;
    use obc_formats::io::{ByteSink, Error, SliceSource};
    use obc_reader::Poi;
    use obc_route::{gpx_to_obcr, RouteIndex, RouteReader, Waypoints};

    #[derive(Debug)]
    struct TextCall {
        text: std::string::String,
        at: Point,
        color: u16,
    }

    /// Records the list's text calls so render tests can pin row order, figures, empty copy and the
    /// muted passed-row colour without coupling to raster-font pixels.
    #[derive(Default)]
    struct TextRec {
        calls: std::vec::Vec<TextCall>,
        /// Every triangle's colour + the three vertices — the side arrows and the source pips are
        /// primitives, not glyphs, so this is how the tests see them.
        triangles: std::vec::Vec<(u16, Point, Point, Point)>,
    }

    impl Surface for TextRec {
        fn clear(&mut self, _: u16) {}
        fn fill(&mut self, _: Rectangle, _: u16) {}
        fn round(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn round_outline(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn line(&mut self, _: Point, _: Point, _: u16) {}
        fn triangle(&mut self, a: Point, b: Point, c: Point, color: u16) {
            self.triangles.push((color, a, b, c));
        }
        fn disc(&mut self, _: Point, _: u32, _: u16) {}
        fn text(&mut self, s: &str, at: Point, _: Font, _: TextAlign, color: u16) -> Point {
            self.calls.push(TextCall { text: s.into(), at, color });
            at
        }
    }

    #[derive(Default)]
    struct VecSink(std::vec::Vec<u8>);

    impl ByteSink for VecSink {
        fn write(&mut self, b: &[u8]) -> Result<(), Error> {
            self.0.extend_from_slice(b);
            Ok(())
        }
        fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
            let off = off as usize;
            self.0[off..off + b.len()].copy_from_slice(b);
            Ok(())
        }
    }

    /// Three named waypoints deliberately listed out of order in GPX, one of them categorized by
    /// `<sym>` (U1). The track climbs 100 m to First climb, descends to Valley, then climbs 100 m
    /// to Finish; all corners survive geometry decimation, making the expected ascent between
    /// Valley and Finish ~100 m.
    const WAYPOINT_GPX: &str = r#"<gpx>
  <wpt lat="48.0200" lon="7.8100"><name>Finish</name></wpt>
  <wpt lat="48.0100" lon="7.8000"><name>First climb</name></wpt>
  <wpt lat="48.0100" lon="7.8100"><name>Valley</name><sym>Drinking Water</sym></wpt>
  <trk><trkseg>
    <trkpt lat="48.0000" lon="7.8000"><ele>100</ele></trkpt>
    <trkpt lat="48.0100" lon="7.8000"><ele>200</ele></trkpt>
    <trkpt lat="48.0100" lon="7.8100"><ele>150</ele></trkpt>
    <trkpt lat="48.0200" lon="7.8100"><ele>250</ele></trkpt>
  </trkseg></trk>
</gpx>"#;

    fn fixture_bytes() -> std::vec::Vec<u8> {
        let mut sink = VecSink::default();
        gpx_to_obcr(&SliceSource(WAYPOINT_GPX.as_bytes()), "Fixture", &mut sink).unwrap();
        sink.0
    }

    /// The fixture route's waypoint table + elevation profile + total, as the App holds them.
    fn fixture() -> (Waypoints, Profile, u32) {
        let bytes = fixture_bytes();
        let src = SliceSource(&bytes);
        let idx = RouteIndex::read(&src).unwrap();
        let route = RouteReader::new(&idx, &src);
        (route.load_waypoints(0), route.elevation_profile(), route.total_distance_m)
    }

    /// A synthetic corridor snapshot (distance, name, subtype, lateral offset).
    fn corridor(items: &[(u32, &str, u8, i32)]) -> std::vec::Vec<CorridorPoi> {
        items
            .iter()
            .map(|(d, n, subtype, off)| {
                let mut name = heapless::String::new();
                name.push_str(n).unwrap();
                CorridorPoi {
                    poi: Poi { lat: 0, lon: 0, subtype: *subtype, name, hours_ref: 0xFFFF, distance_m: *d },
                    dist_along_m: *d,
                    offset_m: *off,
                }
            })
            .collect()
    }

    fn names(
        waypoints: &Waypoints,
        pois: &[CorridorPoi],
        filter: PoiCategorySet,
    ) -> std::vec::Vec<std::string::String> {
        Merge::new(waypoints.as_slice(), pois, filter).map(|e| e.name().to_string()).collect()
    }

    /// Subtype 1 = Drinking water (Water); 18 = a bike shop; 5 = Campsite. Pinned here so a subtype
    /// table edit that moves a category shows up as a test failure, not a mystery icon.
    const WATER: u8 = 1;
    const BIKE: u8 = 18;

    /// The merge is one list on one axis: entries from both sources interleave strictly by
    /// `dist_along_m`, and a tie puts the rider's own waypoint first.
    #[test]
    fn merge_interleaves_both_sources_by_distance() {
        let w = wpts_detailed(&[(500, "Start gate", None, 0), (2_000, "Pass", None, 0), (3_000, "Tie", None, 0)]);
        let p = corridor(&[(100, "Fountain", WATER, 0), (2_500, "Bike shop", BIKE, 0), (3_000, "Tap", WATER, 0)]);
        assert_eq!(
            names(&w, &p, PoiCategorySet::ALL),
            ["Fountain", "Start gate", "Pass", "Bike shop", "Tie", "Tap"],
            "route order, both sources, no sections"
        );
    }

    /// A category filter applies within both sources: it keeps the categorized entries of that
    /// kind — waypoint or POI — and drops everything else, generic waypoints included (a waypoint
    /// with no category can't answer a category question).
    #[test]
    fn a_category_filter_cuts_both_sources() {
        let w = wpts_detailed(&[(400, "Brunnen", Some(PoiCategory::Water), 0), (900, "Turn left", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0), (700, "Cycles", BIKE, 0)]);
        assert_eq!(names(&w, &p, PoiCategorySet::ALL), ["Brunnen", "Fountain", "Cycles", "Turn left"]);
        assert_eq!(
            names(&w, &p, PoiCategorySet::only(PoiCategory::Water)),
            ["Brunnen", "Fountain"],
            "Water keeps the categorized waypoint and the map POI, drops the generic waypoint"
        );
        assert_eq!(names(&w, &p, PoiCategorySet::only(PoiCategory::BikeShop)), ["Cycles"]);
    }

    /// The **source scope** (U4) decides which of the two tables the list may walk at all, and the
    /// category filter still applies *within* whatever is left: Waypoints-only + Water is exactly
    /// the rider's categorized GPX water stops, with no map fountain in sight.
    #[test]
    fn the_source_scope_picks_which_tables_feed_the_list() {
        let w = wpts_detailed(&[(400, "Brunnen", Some(PoiCategory::Water), 0), (900, "Turn left", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0), (700, "Cycles", BIKE, 0)]);
        let listed = |source, filter| {
            let mut s = UpAheadScreen::new(0, source);
            s.filter = filter;
            s.rows(w.as_slice(), &p, true).map(|e| e.name().to_string()).collect::<std::vec::Vec<_>>()
        };

        assert_eq!(
            listed(UpAheadSource::Both, PoiCategorySet::ALL),
            ["Brunnen", "Fountain", "Cycles", "Turn left"],
            "Both is the merged timeline U3 shipped"
        );
        assert_eq!(
            listed(UpAheadSource::WaypointsOnly, PoiCategorySet::ALL),
            ["Brunnen", "Turn left"],
            "Waypoints only drops every map POI, generic waypoints kept"
        );
        assert_eq!(
            listed(UpAheadSource::MapPoisOnly, PoiCategorySet::ALL),
            ["Fountain", "Cycles"],
            "Map POIs only drops the whole waypoint plan — the setting's documented trade"
        );

        // The two controls compose: the scope picks the tables, the filter cuts within them.
        let water = PoiCategorySet::only(PoiCategory::Water);
        assert_eq!(listed(UpAheadSource::Both, water), ["Brunnen", "Fountain"]);
        assert_eq!(listed(UpAheadSource::WaypointsOnly, water), ["Brunnen"]);
        assert_eq!(listed(UpAheadSource::MapPoisOnly, water), ["Fountain"]);

        // And a route-less ride still shows nothing at all, whatever the scope says.
        for source in [UpAheadSource::Both, UpAheadSource::WaypointsOnly, UpAheadSource::MapPoisOnly] {
            let s = UpAheadScreen::new(0, source);
            assert_eq!(s.rows(w.as_slice(), &p, false).count(), 0, "{source:?}: no route, no rows");
        }
    }

    /// The arming decision (U4): **Waypoints only** declares no corridor key at all, so the App-owned
    /// scratch stays disarmed and the board never builds a map `Reader` for this screen — and a
    /// snapshot that somehow *is* in the slice still never reaches the list. The other two scopes
    /// declare the key exactly as U3 did.
    #[test]
    fn waypoints_only_never_asks_for_a_corridor_snapshot() {
        let mut screen = UpAheadScreen::new(1_500, UpAheadSource::WaypointsOnly);
        assert_eq!(screen.corridor_key(), None, "no key ⇒ nothing armed ⇒ the query never runs");
        // Even after a filter change (which is what re-keys a snapshot) it keeps asking for nothing.
        screen.filter = PoiCategorySet::only(PoiCategory::Water);
        assert_eq!(screen.corridor_key(), None, "a category filter doesn't turn the query back on");
        // Belt and braces: a stale/foreign snapshot in the slice is not drawn under this scope.
        let p = corridor(&[(600, "Fountain", WATER, 0)]);
        assert_eq!(UpAheadScreen::new(0, UpAheadSource::WaypointsOnly).rows(&[], &p, true).count(), 0);

        for source in [UpAheadSource::Both, UpAheadSource::MapPoisOnly] {
            assert_eq!(
                UpAheadScreen::new(1_500, source).corridor_key(),
                Some(CorridorKey { filter: PoiCategorySet::ALL, anchor_m: 1_500 }),
                "{source:?} still declares the frozen (filter, anchor) key"
            );
        }
    }

    /// Passed **waypoints** stay as muted whole-plan context with both figures clamped to zero;
    /// passed **POIs** are absent by construction — the snapshot is anchored at entry progress, so
    /// the query never returns one behind the rider.
    #[test]
    fn passed_waypoints_stay_and_clamp_while_passed_pois_never_arrive() {
        let w = wpts_detailed(&[(100, "Behind", None, 0), (900, "Ahead", None, 0)]);
        let p = corridor(&[(800, "Fountain", WATER, 0)]); // anchored at 500: nothing behind
        assert_eq!(names(&w, &p, PoiCategorySet::ALL), ["Behind", "Fountain", "Ahead"]);

        let (_, profile, total) = fixture();
        let behind = figures(100, 500, total, Some(&profile));
        assert_eq!(behind, Figures { passed: true, distance_m: 0, climb_m: Some(0) });
        let ahead = figures(900, 500, total, Some(&profile));
        assert!(!ahead.passed && ahead.distance_m == 400);
    }

    /// The exact `waypoint_figures` math the old list used, now applied to both sources through the
    /// shared `dist_along_m` axis: distance is the along-route delta, climb the cached profile's
    /// cumulative-ascent delta at the two fractions.
    #[test]
    fn figures_use_exact_route_distance_and_profile_ascent_deltas() {
        let (waypoints, profile, total) = fixture();
        let w = waypoints.as_slice();
        assert_eq!(
            w.iter().map(|e| e.name.as_str()).collect::<std::vec::Vec<_>>(),
            ["First climb", "Valley", "Finish"],
            "the resident source is route order, not GPX declaration order"
        );
        let progress = w[1].dist_along_m; // Valley
        let finish = figures(w[2].dist_along_m, progress, total, Some(&profile));
        assert_eq!(finish.distance_m, w[2].dist_along_m - progress, "distance is the exact along-route delta");
        let frac = |m: u32| m as f32 / total as f32;
        let exact = profile.ascent_to(frac(w[2].dist_along_m)) - profile.ascent_to(frac(progress));
        assert_eq!(finish.climb_m, Some(exact), "climb is the exact cached cumulative-ascent delta");
        assert!((95..=105).contains(&exact), "the fixture's final leg climbs ~100 m, got {exact}");

        // A corridor POI at the very same spot reports the very same figures — one axis, one math.
        let poi = corridor(&[(w[2].dist_along_m, "Spring", WATER, 0)]);
        assert_eq!(figures(poi[0].dist_along_m, progress, total, Some(&profile)), finish);
    }

    /// The side hint fires past the threshold on **both** sides and stays quiet inside it — for a
    /// waypoint's stored offset (U1) and a POI's projected offset (U2) alike.
    #[test]
    fn the_side_hint_threshold_is_symmetric_and_source_blind() {
        let w = wpts_detailed(&[(0, "On line", None, 50), (1, "Right", None, 300), (2, "Left", None, -300)]);
        let p = corridor(&[(3, "Near", WATER, -50), (4, "Far right", WATER, 51)]);
        let e: std::vec::Vec<Entry> = Merge::new(w.as_slice(), &p, PoiCategorySet::ALL).collect();
        let hint = |i: usize| side_hint(e[i].offset_m(), Units::Metric).map(|(r, s)| (r, s.as_str().to_string()));
        assert_eq!(hint(0), None, "exactly at the threshold is still on-route");
        assert_eq!(hint(1), Some((true, "300m".into())), "positive = right of travel");
        assert_eq!(hint(2), Some((false, "300m".into())), "negative = left of travel");
        assert_eq!(hint(3), None, "a POI just inside the threshold is quiet too");
        assert_eq!(hint(4), Some((true, "51m".into())), "one metre past the threshold shows");
    }

    /// The picker maps rows to filters both ways: row 0 is Everything, rows 1..=6 the categories in
    /// canonical id order, and the cursor opens on whatever is already active.
    #[test]
    fn the_picker_rows_round_trip_the_filter() {
        assert_eq!(UpAheadScreen::filter_of_row(0), PoiCategorySet::ALL);
        for (i, cat) in PoiCategory::ALL.iter().enumerate() {
            assert_eq!(UpAheadScreen::filter_of_row(i + 1), PoiCategorySet::only(*cat));
            let mut s = UpAheadScreen::new(0, UpAheadSource::Both);
            s.filter = PoiCategorySet::only(*cat);
            assert_eq!(s.picker_row(), i + 1, "the picker opens on the active filter");
        }
        assert_eq!(UpAheadScreen::new(0, UpAheadSource::Both).picker_row(), 0, "a fresh list is on Everything");
    }

    // --- Render pins -------------------------------------------------------

    /// The English copy the draw resolves from the catalog, so the render pins read like the panel.
    const COPY: EmptyCopy<'static> = EmptyCopy {
        no_route: "No route loaded",
        no_route_sub: "Load a route first",
        none: "Nothing up ahead",
        none_sub: "No stops on route",
        none_category_sub: "None of this kind",
        none_waypoints_sub: "Waypoints only",
        none_pois_sub: "Map POIs only",
    };

    /// One rendered frame of the list, from plain values — the same shape [`UpAheadScreen::draw`]
    /// builds from [`Render`], minus the context plumbing.
    fn render(screen: &UpAheadScreen, waypoints: &Waypoints, pois: &[CorridorPoi], progress_m: u32) -> TextRec {
        render_frame(screen, waypoints, pois, progress_m, true, true)
    }

    fn render_frame(
        screen: &UpAheadScreen,
        waypoints: &Waypoints,
        pois: &[CorridorPoi],
        progress_m: u32,
        route_loaded: bool,
        settled: bool,
    ) -> TextRec {
        let (_, profile, total_m) = fixture();
        let mut cv = TextRec::default();
        if let Some(row) = screen.picker {
            let names = ["Water", "Campsite", "Lodging", "Resupply", "Pharmacy", "Bike shop"];
            draw_picker(&mut cv, 240, 320, "SHOW", "Everything", names, row);
            return cv;
        }
        let rows = screen.rows(waypoints.as_slice(), pois, route_loaded);
        let total = screen.rows(waypoints.as_slice(), pois, route_loaded).count();
        let v = View {
            w: 240,
            h: 320,
            title: "Up ahead",
            copy: COPY,
            waypoints: waypoints.as_slice(),
            corridor: pois,
            filter: screen.filter,
            source: screen.source,
            route_loaded,
            settled,
            progress_m,
            route_total_m: total_m,
            profile: Some(&profile),
            units: Units::Metric,
            cursor: screen.cursor(rows, progress_m, total),
            total,
        };
        draw_list(&mut cv, &v);
        cv
    }

    fn texts(rec: &TextRec) -> std::vec::Vec<&str> {
        rec.calls.iter().map(|c| c.text.as_str()).collect()
    }

    /// The rendered list is the merge, in order, each row `name / distance / climb` — and a passed
    /// waypoint's three strings are all muted while the plan ahead stays full ink.
    #[test]
    fn render_keeps_route_order_and_mutes_passed_rows() {
        let w = wpts_detailed(&[(100, "Behind", None, 0), (1_000, "Pass", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0)]);
        let rec = render(&UpAheadScreen::new(200, UpAheadSource::Both), &w, &p, 200);
        let body = &texts(&rec)[2..]; // title + the pos/total slot
        assert_eq!(&body[..9], ["Behind", "0m", "0m", "Fountain", "400m", "0m", "Pass", "800m", "0m"]);
        assert_eq!(rec.calls[2].color, palette::SUBTEXT, "the passed waypoint's name is muted");
        assert_eq!(rec.calls[3].color, palette::SUBTEXT, "…and so are both of its clamped figures");
        assert_eq!(rec.calls[4].color, palette::SUBTEXT);
        assert_eq!(rec.calls[5].color, palette::INK, "the corridor POI ahead is full ink");
        assert!(
            rec.calls[2].at.y < rec.calls[5].at.y && rec.calls[5].at.y < rec.calls[8].at.y,
            "route order is also visual top-to-bottom — one list, one axis"
        );
    }

    /// The side hint lives at line 2's right edge and never collides with the climb column: the
    /// climb slides left of the hint block rather than overprinting it on the 240 px panel.
    #[test]
    fn the_side_hint_sits_at_the_right_edge_clear_of_the_climb() {
        let w = wpts_detailed(&[(2_000, "Spring in the valley", None, -280)]);
        let rec = render(&UpAheadScreen::new(0, UpAheadSource::Both), &w, &[], 0);
        // Line 2 is everything sharing the distance's baseline; draw order is distance, hint, climb.
        let sy = rec.calls[3].at.y;
        let line2: std::vec::Vec<&TextCall> = rec.calls[2..].iter().filter(|c| c.at.y == sy).collect();
        assert_eq!(
            line2.iter().map(|c| c.text.as_str()).collect::<std::vec::Vec<_>>(),
            ["2.0km", "280m", "100m"],
            "distance-to-go, the off-route hint, and climb-to-go all fit one line"
        );
        let (dist, hint, climb) = (line2[0], line2[1], line2[2]);
        assert!(dist.at.x < climb.at.x && climb.at.x < hint.at.x, "left to right: distance, climb, hint");
        let climb_end = climb.at.x + text_width(climb.text.as_str(), Font::Label) as i32;
        let arrow_left = hint.at.x - ARROW_GAP - ARROW_W;
        assert!(climb_end < arrow_left, "the climb column clears the arrow ({climb_end} vs {arrow_left})");
        let hint_end = hint.at.x + text_width(hint.text.as_str(), Font::Label) as i32;
        assert!(hint_end <= 240 - SIDE_INSET, "the hint stays inside the row area ({hint_end})");
        // The arrow points **left** for a negative offset, and it is a drawn triangle, not a glyph
        // (the device font's Latin strip has no arrow — a `<-` char would render as a silent `?`).
        let arrow = rec
            .triangles
            .iter()
            .find(|(_, a, b, c)| a.x == b.x && a.x > c.x && (a.y - b.y).abs() == 10)
            .expect("the side arrow draws as a triangle");
        assert_eq!(arrow.3.x, arrow_left, "its tip sits just left of the distance");
        // A long name ellipsizes rather than running under the scrollbar.
        assert!(rec.calls[2].text.ends_with(".."), "an over-long name is cut with the house ellipsis");
    }

    /// The custom-source cue: an unselected waypoint row's icon draws AMBER and carries the diamond
    /// pip; a POI row's draws SUBTEXT with no pip.
    #[test]
    fn waypoint_rows_carry_the_amber_icon_and_the_diamond_pip() {
        let w = wpts_detailed(&[(1_000, "Pass", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0)]);
        let mut screen = UpAheadScreen::new(0, UpAheadSource::Both);
        screen.selected = Some(0); // the POI row is the cursor, so the waypoint row is unselected
        let rec = render(&screen, &w, &p, 0);
        let amber = rec.triangles.iter().filter(|(c, ..)| *c == palette::AMBER).count();
        assert!(amber >= 4, "the generic diamond (2) + the pip (2) both draw in amber, got {amber}");
        assert!(
            rec.triangles.iter().any(|(c, ..)| *c == palette::SUBTEXT),
            "the unselected POI row's icon stays in the browser's muted scheme"
        );
    }

    /// The empty-state trio: no route, nothing ahead, and nothing of *this category* ahead — three
    /// distinct sentences, never the same one twice.
    #[test]
    fn the_three_empty_states_are_distinct() {
        let stale = wpts_detailed(&[(10, "Stale", None, 0)]);
        let none = Waypoints::new();

        let render_with = |screen: &UpAheadScreen, w: &Waypoints, route: bool, settled: bool| {
            let rec = render_frame(screen, w, &[], 0, route, settled);
            rec.calls.iter().map(|c| c.text.clone()).collect::<std::vec::Vec<_>>()
        };

        let route_less = render_with(&UpAheadScreen::new(0, UpAheadSource::Both), &stale, false, true);
        assert!(route_less.iter().any(|s| s == "No route loaded"));
        assert!(route_less.iter().any(|s| s == "Load a route first"));
        assert!(!route_less.iter().any(|s| s == "Stale"), "a route-less frame never leaks the resident cache");

        let nothing = render_with(&UpAheadScreen::new(0, UpAheadSource::Both), &none, true, true);
        assert!(nothing.iter().any(|s| s == "Nothing up ahead"));
        assert!(nothing.iter().any(|s| s == "No stops on route"));

        let mut filtered = UpAheadScreen::new(0, UpAheadSource::Both);
        filtered.filter = PoiCategorySet::only(PoiCategory::Water);
        let none_of_kind = render_with(&filtered, &none, true, true);
        assert!(none_of_kind.iter().any(|s| s == "Nothing up ahead"));
        assert!(none_of_kind.iter().any(|s| s == "None of this kind"));

        // Not settled yet (no reader / no route geometry this frame) — say nothing at all rather
        // than flash an answer the next frame contradicts.
        let pending = render_with(&UpAheadScreen::new(0, UpAheadSource::Both), &none, true, false);
        assert_eq!(pending.len(), 2, "only the title bar (+ its empty counter slot) draws in flight");
    }

    /// The rendered list obeys the scope too — not just the merge behind it: a Waypoints-only frame
    /// draws no POI row and a Map-POIs-only frame draws no waypoint row (nor its amber source pip).
    #[test]
    fn the_rendered_list_shows_only_the_scoped_source() {
        let w = wpts_detailed(&[(1_000, "Pass", None, 0), (2_000, "Col", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0)]);
        let drawn = |source| {
            let rec = render(&UpAheadScreen::new(0, source), &w, &p, 0);
            (texts(&rec).iter().map(|s| s.to_string()).collect::<std::vec::Vec<_>>(), rec)
        };

        let (both, _) = drawn(UpAheadSource::Both);
        assert!(both.iter().any(|s| s == "Fountain") && both.iter().any(|s| s == "Pass"));

        let (wpt_only, rec) = drawn(UpAheadSource::WaypointsOnly);
        assert!(wpt_only.iter().any(|s| s == "Pass"), "the rider's own plan is all that's left");
        assert!(!wpt_only.iter().any(|s| s == "Fountain"), "no map POI row under Waypoints only");
        assert_eq!(wpt_only.len(), 2 + 2 * 3, "two rows' worth of strings (name + distance + climb) past the title");
        assert!(
            rec.triangles.iter().any(|(c, ..)| *c == palette::AMBER),
            "the unselected waypoint row keeps its amber source icon + pip"
        );

        let (poi_only, rec) = drawn(UpAheadSource::MapPoisOnly);
        assert!(poi_only.iter().any(|s| s == "Fountain"));
        assert!(!poi_only.iter().any(|s| s == "Pass"), "the waypoint plan leaves the ride menu entirely");
        assert!(
            !rec.triangles.iter().any(|(c, ..)| *c == palette::AMBER),
            "…taking the amber custom-source cue with it (the cursor's own fill is a round, not a triangle)"
        );
    }

    /// A scoped list's empty state stays **truthful**: "no stops on route" is only true when both
    /// sources were allowed to answer, so a single-source scope names itself instead. A live
    /// category filter still outranks it — that's the thing the rider just did.
    #[test]
    fn the_empty_state_names_the_source_scope() {
        let none = Waypoints::new();
        let sub_of = |source, filter| {
            let mut s = UpAheadScreen::new(0, source);
            s.filter = filter;
            let rec = render_frame(&s, &none, &[], 0, true, true);
            texts(&rec).last().expect("the empty state draws a sub-line").to_string()
        };

        assert_eq!(sub_of(UpAheadSource::Both, PoiCategorySet::ALL), "No stops on route");
        assert_eq!(
            sub_of(UpAheadSource::WaypointsOnly, PoiCategorySet::ALL),
            "Waypoints only",
            "a route lined with map POIs must never be called stop-less"
        );
        assert_eq!(sub_of(UpAheadSource::MapPoisOnly, PoiCategorySet::ALL), "Map POIs only");

        // The category filter is the more immediate truth — it wins under every scope.
        let water = PoiCategorySet::only(PoiCategory::Water);
        for source in [UpAheadSource::Both, UpAheadSource::WaypointsOnly, UpAheadSource::MapPoisOnly] {
            assert_eq!(sub_of(source, water), "None of this kind", "{source:?}");
        }
    }

    /// The picker draws all seven rows in canonical order, cursor on the active filter.
    #[test]
    fn the_picker_draws_everything_over_the_six_categories() {
        let mut screen = UpAheadScreen::new(0, UpAheadSource::Both);
        screen.picker = Some(screen.picker_row());
        let rec = render(&screen, &Waypoints::new(), &[], 0);
        let body = texts(&rec);
        assert_eq!(body[0], "SHOW", "the picker retitles the frame");
        assert_eq!(
            &body[body.len() - PICKER_ROWS..],
            ["Everything", "Water", "Campsite", "Lodging", "Resupply", "Pharmacy", "Bike shop"],
            "all seven rows fit the panel — a filter you must scroll to is a filter you won't use"
        );
    }

    // --- Gesture pins ------------------------------------------------------

    fn ctx<'a>(activity: &'a mut Activity, waypoints: &'a Waypoints, corridor: &'a [CorridorPoi]) -> Ctx<'a> {
        let state = std::boxed::Box::leak(std::boxed::Box::new(AppState::new(0, 0, 1.0)));
        let settings = std::boxed::Box::leak(std::boxed::Box::new(Settings::default()));
        Ctx { waypoints: waypoints.as_slice(), corridor, ..test_ctx(state, activity, settings) }
    }

    fn riding() -> Activity {
        let mut a = Activity::new(Mode::Riding);
        a.active_route = Some(0);
        a.route_total_m = 10_000;
        a
    }

    /// The cursor opens on the first entry still ahead — across *both* sources, not the waypoint
    /// table's index — and a turn takes over from there.
    #[test]
    fn the_list_opens_at_the_first_unpassed_row() {
        let w = wpts_detailed(&[(100, "Behind", None, 0), (1_000, "Pass", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0)]);
        let mut act = riding();
        act.progress_m = 200;
        let mut screen = UpAheadScreen::new(200, UpAheadSource::Both);
        let rows = Merge::new(w.as_slice(), &p, PoiCategorySet::ALL);
        assert_eq!(screen.cursor(rows, 200, 3), 1, "row 0 is passed; the Fountain is the first ahead");

        screen.handle(Gesture::Step(1), &mut ctx(&mut act, &w, &p));
        assert_eq!(screen.selected, Some(2), "a turn steps on from the resolved home row");
        screen.handle(Gesture::Step(1), &mut ctx(&mut act, &w, &p));
        assert_eq!(screen.selected, Some(0), "and wraps over the merged count, not either table's");

        // Before the first frame there is no snapshot and (on a fresh route load) no waypoint table
        // either: a turn then leaves the cursor *unresolved*, so the list still opens where it
        // should once the rows arrive.
        let mut fresh = UpAheadScreen::new(200, UpAheadSource::Both);
        fresh.handle(Gesture::Step(1), &mut ctx(&mut act, &Waypoints::new(), &[]));
        assert_eq!(fresh.selected, None, "a step over an empty list keeps the homing");
    }

    /// Press on a POI row opens the detail carrying the signed offset; press on a waypoint row is
    /// inert; Back leaves; and none of it touches the ride session.
    #[test]
    fn press_opens_poi_rows_only() {
        let w = wpts_detailed(&[(1_000, "Pass", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, -220)]);
        let mut act = riding();
        act.start_session();
        let session = act.session;
        let mut screen = UpAheadScreen::new(0, UpAheadSource::Both);

        screen.selected = Some(0); // the Fountain
        match screen.handle(Gesture::Press, &mut ctx(&mut act, &w, &p)) {
            Transition::Push(Screen::PoiDetail(d)) => assert_eq!(d.off_route_m(), Some(-220)),
            _ => panic!("a POI row must open the detail"),
        }
        screen.selected = Some(1); // the waypoint
        assert!(matches!(screen.handle(Gesture::Press, &mut ctx(&mut act, &w, &p)), Transition::None));
        assert!(matches!(screen.handle(Gesture::Back, &mut ctx(&mut act, &w, &p)), Transition::Pop));
        assert_eq!(act.mode, Mode::Riding);
        assert_eq!(act.session, session);
    }

    /// Hold opens the picker on the active filter; Press applies it (re-keying the snapshot and
    /// re-homing the cursor); Back cancels, leaving both the filter and the key untouched.
    #[test]
    fn the_picker_applies_on_press_and_cancels_on_back() {
        let w = wpts_detailed(&[(1_000, "Pass", None, 0)]);
        let p = corridor(&[]);
        let mut act = riding();
        let mut screen = UpAheadScreen::new(4_000, UpAheadSource::Both);
        let before = screen.corridor_key();
        assert_eq!(before.expect("Both asks for a snapshot").anchor_m, 4_000, "the key freezes progress at entry");

        screen.handle(Gesture::Hold, &mut ctx(&mut act, &w, &p));
        assert_eq!(screen.picker, Some(0), "the picker opens on Everything, the active filter");
        screen.handle(Gesture::Step(1), &mut ctx(&mut act, &w, &p)); // → Water
        screen.handle(Gesture::Back, &mut ctx(&mut act, &w, &p));
        assert_eq!(screen.picker, None, "Back closes the picker");
        assert_eq!(screen.corridor_key(), before, "…and cancels: the snapshot key is untouched");

        screen.handle(Gesture::Hold, &mut ctx(&mut act, &w, &p));
        screen.handle(Gesture::Step(1), &mut ctx(&mut act, &w, &p));
        screen.handle(Gesture::Press, &mut ctx(&mut act, &w, &p));
        assert_eq!(screen.picker, None, "applying returns to the list");
        assert_eq!(
            screen.corridor_key(),
            Some(CorridorKey { filter: PoiCategorySet::only(PoiCategory::Water), anchor_m: 4_000 }),
            "the applied filter re-keys the snapshot (which is what re-queries it)"
        );
        assert_eq!(screen.selected, None, "and the cursor re-homes to the first row ahead");
    }

    /// While the picker is open it owns every gesture: Back closes it instead of leaving the
    /// screen, and a Press never falls through to a row.
    #[test]
    fn the_open_picker_swallows_navigation() {
        let w = wpts_detailed(&[(1_000, "Pass", None, 0)]);
        let p = corridor(&[(600, "Fountain", WATER, 0)]);
        let mut act = riding();
        let mut screen = UpAheadScreen::new(0, UpAheadSource::Both);
        for g in [Gesture::Step(1), Gesture::Back, Gesture::Press, Gesture::BackHold, Gesture::Hold] {
            screen.picker = Some(3);
            assert!(
                matches!(screen.handle(g, &mut ctx(&mut act, &w, &p)), Transition::None),
                "{g:?} must not navigate out of the picker"
            );
        }
    }
}
