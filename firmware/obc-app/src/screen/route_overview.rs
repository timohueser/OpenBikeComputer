//! The Route overview — the look-before-you-ride page between picking a route and tracking it.
//! Shows the route's name, a **content-paired pager** (owner review round 3): the media band and
//! the stat rows flip together every 5 s — page A is the route's **track-shape preview** (the
//! host-decimated polyline, the NEW ROUTE page's sketch) over its DISTANCE, page B the full
//! **elevation profile** (the Statistics band, **non-interactive**: no cursor, no zoom, no live
//! shading) over CLIMB + DESCENT. (EST TIME joined page A with the elevation epic's time model
//! (#1068, EL9): it was omitted at review round 3 because the §8.6 nav profiles carry only
//! dimensionless edge-weight multipliers — no speed model existed anywhere — and
//! [`obc_route::eta`] is exactly that missing model, `dist / v_flat + ascent × k_climb` keyed by
//! the rider's bike profile.) Below the pager, a START RIDE
//! row and (when deletable) the Delete-route row under it. The two action rows are
//! the **Pause-menu (ride_control) row family** (owner review round 3 — the round-2 focus
//! outline read as one-off chrome): unselected rows are plain labels, the selected row wears the
//! standard amber fill, and the guarded Delete row shows its shaded base + warning hold-fill
//! only **while selected**. The cursor semantics are round 2's, unchanged: entry selects START
//! RIDE; *up/down* toggles between the two rows; *press* starts the session only from the START row
//! and drops into the riding Map — exactly what picking a route used to do directly; *hold*
//! charges the delete only while the Delete row is selected; *back* cancels and returns to the
//! Route menu. With the Delete row hidden (in use / computed) there is nothing to toggle: press
//! starts, hold is a no-op, and the lone START row keeps the amber selected face.
//!
//! Entering the overview sets [`Activity::active_route`](crate::Activity::active_route) — the
//! hosts key geometry loading on it, so the route streams open and the profile builds while the
//! rider is still looking at the page — but starts **no** session; the previous `active_route`
//! is remembered and restored on `back`, so browsing routes never clobbers a loaded one. The
//! descent figure comes from the opened route (`--` for the frame or two before it streams in).

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use super::vocab::band::{ElevationBand, PeakLabel};
use super::vocab::chrome::{empty_state, stroke2, title_frame, LIST_TOP};
use super::vocab::fmt::{duration_hms, expiry_short, write_distance_split};
use super::vocab::pager::ContentPager;
use super::vocab::rows::{draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use crate::activity::Activity;
use crate::input::Gesture;
use crate::retention::{RouteRetentionMeta, DAY_SECS};
use crate::route::RouteSummary;
use crate::screen::ScreenTick;
use crate::Msg;

use super::{palette, Ctx, Render, Transition};

/// Chart band: below the title bar, deep enough to read the terrain, clear of the stat tiles.
const BAND_TOP: i32 = LIST_TOP + 8;
/// The media band's top when the Auto-delete expiry row shows (epic #638 S5): 24 px lower, so the
/// row's compact caption line tucks between the title bar and the band. A `Never` route keeps
/// [`BAND_TOP`] (the row is absent), so existing routes render byte-identically.
const BAND_TOP_EXPIRY: i32 = LIST_TOP + 32;
const BAND_BOT: i32 = 140;
const SIDE_MARGIN: i32 = 12;

/// The Auto-delete expiry row (epic #638 S5): a compact caption line between the title bar and the
/// (lowered) media band — a muted "Auto-delete" label + the ink remaining-time value, centred as
/// one group. `Y` is its top; `X` is the minimum side inset the centred group clamps to.
const EXPIRY_ROW_Y: i32 = LIST_TOP + 4;
const EXPIRY_ROW_X: i32 = 12;

/// The stat ledger under the media band — the content-paired pager's stat half (owner review
/// round 3): page A (track shape) carries DISTANCE + EST TIME, page B (elevation) CLIMB + DESCENT;
/// [`ROW_PITCH`] is the row spacing within a page. Placed between the band and the action rows
/// (whose Pause-family block sits 4 px higher than the old START bar — the ledger moved up with
/// it, keeping the same breathing gap above the amber row).
const ROWS_TOP: i32 = 146;
const ROW_PITCH: i32 = 42;

/// The two action rows — the Pause-menu (ride_control) row family's exact geometry (owner review
/// round 3): 38 px rows, an 8 px gap, START RIDE over the guarded Delete-route row (owner review
/// round 1: the destructive row ranks under the primary action), the block anchored the standard
/// 10 px above the card bottom. START keeps its two-row position even with Delete hidden, so
/// nothing jumps when the row re-arms.
const OPTION_ROW_H: i32 = 38;
const OPTION_GAP: i32 = 8;

/// The START RIDE button bar of the **computed** (length-only) page — the screen-bottom anchor
/// shared with the POI detail's `Route here` footer (see [`draw_start_button`]).
const BUTTON_H: i32 = 34;

/// The two action rows the cursor walks (owner review round 2): the START RIDE bar and, when the
/// route is deletable, the guarded Delete-route button under it.
const START: usize = 0;
const DELETE: usize = 1;

/// The Route overview. State is which catalog route it previews, plus the `active_route` that was
/// loaded when it opened (restored on `back`), and whether the route is a **computed** one (the
/// on-device router's output, epic #116 R4) — which has no elevation data, so the page shows
/// length only.
#[derive(Debug, Default)]
pub struct RouteOverviewScreen {
    route: usize,
    prev_active: Option<usize>,
    /// The previewed route came from the on-device router (`/routes/_nav.obcr`): OSM highways
    /// carry no elevation and there is no DEM, so its per-point elevation is all zero — the page
    /// omits the elevation band and the climb/descent rows rather than showing a flat band and
    /// "+0 m" (the locked "length only" overview).
    computed: bool,
    /// The content-paired pager: page one is the track shape + DISTANCE, page two the elevation
    /// band + CLIMB + DESCENT. Unused on the computed (length-only) page, which never flips.
    pager: ContentPager,
    /// The action-row cursor ([`START`] / [`DELETE`], owner review round 2). Entry selects START;
    /// meaningful only while the Delete row exists — with it hidden the cursor pins to START.
    selected: usize,
}

impl RouteOverviewScreen {
    /// Preview catalog route `route`; `prev_active` is the `active_route` to restore on cancel.
    pub fn new(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: false, pager: ContentPager::default(), selected: START }
    }

    /// Preview a **computed** route (the router's output): length only — no elevation band, no
    /// climb/descent rows. Opened by the pass's fact stage.
    pub fn computed(route: usize, prev_active: Option<usize>) -> Self {
        RouteOverviewScreen { route, prev_active, computed: true, pager: ContentPager::default(), selected: START }
    }

    /// Whether the guarded **Delete route** row exists — a real, non-computed catalog route
    /// that isn't the actively-navigated route of a running tracking session. This is the exact
    /// predicate the old Route-menu footer greyed on, moved here (T3): deleting the file under an
    /// open geometry handle mid-ride would break navigation. Since owner review round 1 the row is
    /// **hidden entirely** while disallowed (no greyed face), and this guard keeps a hold a no-op
    /// regardless.
    pub(crate) fn delete_enabled(&self, activity: &Activity, recording: bool, routes: &[RouteSummary]) -> bool {
        !self.computed && self.route < routes.len() && !(recording && activity.active_route == Some(self.route))
    }

    /// True while a hold would charge the Delete row — it exists **and the cursor is on it**
    /// (owner review round 2: no more hold-anywhere) — the
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) predicate for this screen.
    pub(crate) fn selection_is_guarded(&self, activity: &Activity, recording: bool, routes: &[RouteSummary]) -> bool {
        self.selected == DELETE && self.delete_enabled(activity, recording, routes)
    }

    /// Content-paired pager tick: flip the two pages (track shape + DISTANCE / elevation band +
    /// CLIMB + DESCENT) on the shared dwell. The computed (length-only) page has a single fixed
    /// layout and no pager, so it never flips.
    pub fn tick_timers(&mut self, now_ms: u32) -> ScreenTick {
        if self.computed {
            return ScreenTick::idle();
        }
        self.pager.tick(now_ms)
    }

    /// Re-point both held indices after a live catalog rescan (#450). A vanished preview subject
    /// becomes an out-of-range index — exactly the missing-summary path `draw`/`handle` already
    /// have ("No route" + `press` pops); a vanished `prev_active` restores to `None` on cancel.
    pub(crate) fn remap_routes(&mut self, remap: &dyn Fn(usize) -> Option<usize>) {
        self.route = remap(self.route).unwrap_or(usize::MAX);
        self.prev_active = self.prev_active.and_then(remap);
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The action-row cursor (owner review round 2): a step toggles START ↔ Delete. With
            // the Delete row hidden (in use / computed) there is one row and the step is a no-op —
            // the cursor also clamps back to START first, in case the row vanished under it (the
            // route became the active ride's via the swap flow).
            Gesture::Step(n) => {
                let len = if self.delete_enabled(cx.activity, cx.recorder.recording(), cx.routes) { 2 } else { 1 };
                self.selected = self.selected.min(len - 1);
                self.selected = super::vocab::list::step_selection(self.selected, n, len);
                Transition::None
            }
            // Start — only from the selected START row (a press on the Delete row does nothing;
            // it's hold-guarded): the session begin that Route-menu `press` used to do — riding
            // camera on the route's start, tracking on, and a clean [Home, Map] stack. The shared
            // [`start_ride`](super::start_ride) path, also the upload popup's *Start navigation*.
            //
            // Mid-ride (a computed-route overview can open while tracking — the POI flow, epic
            // #116 R4), accepting is ambiguous the same way picking a route from the menu is, so
            // it opens the **same** save/swap prompt instead of silently restarting the session;
            // the Route menu's tracking arm never reaches an overview, so this arm fires only on
            // that flow today.
            Gesture::Press if self.selected == START => {
                if cx.recorder.recording() {
                    return Transition::Push(super::Screen::RouteSwap(super::RouteSwapScreen::new(self.route)));
                }
                super::start_ride(cx, self.route)
            }
            // Delete: a completed hold with the cursor on the live Delete row (the guarded hold is
            // the confirmation, no popup — but never hold-anywhere, owner review round 2) records
            // the delete by index. The host resolves it to the durable object id, deletes the
            // object — a created `_NAV.OBR` route the same way, no special casing — and the
            // store-changed rescan re-feeds the catalog. Restore the pre-preview active route and
            // pop to the refreshed Routes list. A hold while the route is in use (row hidden)
            // never reaches here.
            Gesture::Hold if self.selection_is_guarded(cx.activity, cx.recorder.recording(), cx.routes) => {
                cx.activity.request_route_delete(self.route);
                cx.activity.active_route = self.prev_active;
                Transition::Pop
            }
            // Cancel: put back whatever route was loaded before the preview.
            Gesture::Back => {
                cx.activity.active_route = self.prev_active;
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        let Some(summary) = rx.routes.get(self.route) else {
            title_frame(cv, w, h, rx.t(Msg::RouteOverviewTitle), "");
            empty_state(cv, w, h, rx.t(Msg::RouteOverviewNoRoute), rx.t(Msg::RouteOverviewNoRouteSub));
            return;
        };

        let chart_x = SIDE_MARGIN;
        let chart_w = w - 2 * SIDE_MARGIN;

        // A computed route (the on-device router's output) has no elevation data at all — the
        // locked "length only" page: a DISTANCE row (meter-resolution from the opened geometry,
        // where the whole-km catalog figure would read "0 km" on a short POI route), no elevation
        // band, no climb/descent — plus the shape preview (#685 §4). The START button is shared.
        if self.computed {
            // Static NEW ROUTE title; the destination name moves into the body as the first line
            // at full card width (#685 §4 — a title-bar name truncated to `Carrefour Mar..`).
            title_frame(cv, w, h, rx.t(Msg::RouteOverviewNewRoute), "");
            let x = 16;
            let name =
                super::route_menu::fit_name(&summary.name, ((w - 2 * x) / Font::Body.char_width() as i32) as usize);
            cv.text(&name, Point::new(x, LIST_TOP + 4), Font::Body, TextAlign::Left, INK);

            let units = rx.settings.units;
            let total_m = rx.route.map(|r| r.total_distance_m).unwrap_or(summary.distance_km * 1000);
            // Metres below 1 km (`600 m`, #685 §4 — `0.6 km` undersells a short POI route), the
            // one-decimal km above; imperial twin: whole feet below a mile, one-decimal miles.
            let mut dist: heapless::String<8> = heapless::String::new();
            let dist_unit = write_distance_split(&mut dist, total_m, units);
            let rows_top = LIST_TOP + 34;
            ledger_row(cv, w, rows_top, rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None);
            // EST TIME (EL9, #1077) — the one figure a length-only page was missing. A computed
            // route's points are all zero-elevation until EL7 fills them from terrain, so the
            // model's ascent term is zero and this reads exactly `distance / v_flat`; when EL7
            // lands the identical call starts answering with the real climb, with nothing here to
            // change. The BIKE TYPE row directly under it names the profile the figure is keyed to.
            let est = est_time_value(total_m, route_ascent_m(rx, summary), rx.settings.bike_profile_idx);
            ledger_row(cv, w, rows_top + ROW_PITCH, rx.t(Msg::RouteOverviewEstTime), &est, "h", None);
            // The bike profile the route was planned under (routing-v2 N5): the rider must be able to
            // tell a Road route from an MTB one they picked by accident. The name resolves against the
            // loaded map for the current selection — which is the profile the just-finished plan used,
            // since planning uses `bike_profile_idx` and the overview opens straight off it.
            draw_profile_label(cv, w, rx, rows_top + 2 * ROW_PITCH);
            // The route-shape preview fills the middle between the ledger and the START bar.
            draw_route_preview(cv, w, rows_top + 3 * ROW_PITCH, h - 10 - BUTTON_H, rx.nav_preview);
            draw_start_button(cv, w, h, rx.t(Msg::RouteOverviewStartRide));
            return;
        }

        let name = super::route_menu::fit_name(&summary.name, ((w - 28) / Font::Body.char_width() as i32) as usize);
        title_frame(cv, w, h, &name, "");

        // The Auto-delete expiry row (epic #638 S5): one muted metadata line between the title bar
        // and the media band — the label left, the time left right — shown only for a route that
        // actually expires (retention ≠ Never; hidden entirely otherwise). When shown, the media
        // band starts [`BAND_TOP_EXPIRY`] instead of [`BAND_TOP`] to make room; a `Never` route
        // keeps the full band, so nothing about the existing (all-`Never`) routes changes.
        let meta = rx.route_metas.get(self.route).copied().unwrap_or_default();
        let expiry = expiry_value(meta, rx.now_utc);
        let band_top = if expiry.is_some() { BAND_TOP_EXPIRY } else { BAND_TOP };
        if let Some(value) = &expiry {
            // A muted label + an ink value, drawn as one **centred group** with a one-space gap:
            // the label alone is nearly half the 240 px line, so left/right anchoring would leave
            // no room between them. Two-tone (SUBTEXT label, INK value) separates the two without a
            // separator glyph.
            let label = rx.t(Msg::RouteOverviewAutoDelete);
            let cw = Font::Label.char_width() as i32;
            let label_w = label.chars().count() as i32 * cw;
            let value_w = value.chars().count() as i32 * cw;
            let gap = cw; // one space between label and value
            let x0 = ((w - (label_w + gap + value_w)) / 2).max(EXPIRY_ROW_X);
            cv.text(label, Point::new(x0, EXPIRY_ROW_Y), Font::Label, TextAlign::Left, SUBTEXT);
            cv.text(value, Point::new(x0 + label_w + gap, EXPIRY_ROW_Y), Font::Label, TextAlign::Left, INK);
        }

        // The content-paired media band (owner review round 3): the auto-flip swaps this WITH the
        // stat rows below — page A the route's track-shape preview, page B the elevation profile.
        // Both draw in the same slot (`band_top`..[`BAND_BOT`]), so nothing jumps on the flip.
        let page_b = self.pager.on_second_page();
        if !page_b {
            // Page A: the host-decimated track shape (the NEW ROUTE page's sketch, aspect-fit,
            // start disc + destination diamond). An empty slice (the frame or two before the host
            // hands it in) just leaves the slot blank, like the shape preview always has.
            draw_route_preview(cv, w, band_top, BAND_BOT, rx.nav_preview);
        } else if let Some(profile) = rx.profile {
            // Page B: the shared full-route elevation band without any of Statistics' live layers
            // (no traveled shading, no cursor, no progress bar). A peak label over the apex gives
            // the vertical scale meaning.
            let band = ElevationBand::whole_route(profile, rect(chart_x, band_top, chart_w, BAND_BOT - band_top + 1));
            band.fill(cv, PARCHMENT_SHADE);
            band.stroke(cv, AMBER);
            band.peak_label(cv, rx.settings.units, PeakLabel::OverPeak);
        } else {
            // Route still streaming open: keep the band's footprint so the page doesn't jump.
            cv.text(
                rx.t(Msg::RouteOverviewLoadingProfile),
                Point::new(w / 2, (band_top + BAND_BOT) / 2 - 9),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
        }
        cv.hline(chart_x, BAND_BOT + 1, chart_w, RULE); // baseline marks the band slot on both pages

        // Headline stats as a ledger — olive caption left, big ink value right with a small unit
        // suffix, hairline rules between rows. Organized without the riding grid's panes, which
        // read as "live data" and swallow space this page doesn't need. Distance and climb come
        // from the catalog summary (always present); descent needs the opened route.
        let units = rx.settings.units;
        let mut dist: heapless::String<8> = heapless::String::new();
        let _ = write!(dist, "{}", (units.dist(summary.distance_km as f32) + 0.5) as u32);
        let dist_unit = if units.is_imperial() { "mi" } else { "km" };

        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", (units.elev(summary.climb_m as f32) + 0.5) as u32);

        let mut desc: heapless::String<8> = heapless::String::new();
        match rx.route {
            Some(r) => {
                let _ = write!(desc, "{}", (units.elev(r.total_descent_m as f32) + 0.5) as u32);
            }
            None => {
                let _ = desc.push_str("--");
            }
        }

        // EST TIME (EL9, #1077): the gradient-aware estimate for the whole route, keyed to the
        // rider's bike profile. It pairs with the track shape on page A — "how far, how long" — while
        // the climbing that drives it is spelled out on page B. Totals come from the opened route
        // when it has streamed in (metre/metre exact) and from the catalog summary before that, so
        // the row never has to show a placeholder.
        let est = est_time_value(route_total_m(rx, summary), route_ascent_m(rx, summary), rx.settings.bike_profile_idx);

        // The stats pair with their media (owner review round 3): DISTANCE + EST TIME belong to the
        // track shape (page A), CLIMB + DESCENT to the elevation band (page B). The flip itself is
        // the affordance — no page dots.
        let entries: [(&str, &str, &str, Option<bool>); 4] = [
            (rx.t(Msg::RouteOverviewDistance), &dist, dist_unit, None),
            (rx.t(Msg::RouteOverviewClimb), &climb, units.elev_label(), Some(true)),
            (rx.t(Msg::RouteOverviewDescent), &desc, units.elev_label(), Some(false)),
            (rx.t(Msg::RouteOverviewEstTime), &est, "h", None),
        ];
        let page_rows: &[usize] = if page_b { &[1, 2] } else { &[0, 3] };
        for (slot, &e) in page_rows.iter().enumerate() {
            let y = ROWS_TOP + slot as i32 * ROW_PITCH;
            let (caption, value, unit, arrow) = entries[e];
            ledger_row(cv, w, y, caption, value, unit, arrow);
            if slot + 1 < page_rows.len() {
                cv.hline(16, y + ROW_PITCH - 4, w - 32, RULE);
            }
        }

        // The two action rows — the Pause-menu (ride_control) row family (owner review round 3:
        // "make this styled just like the buttons in the Pause menu"): plain labels, the selected
        // row wearing the standard amber fill, the guarded Delete row its shaded base + warning
        // hold-fill only while selected. While the route is the active ride's the Delete row is
        // simply **not drawn** — no dim trash, no "In use" cue (owner review round 1: the state
        // can't act, so it doesn't show) — and the `selection_is_guarded` guard keeps a hold a
        // no-op regardless. START keeps the two-row block's top slot either way, so nothing jumps
        // when the Delete row re-arms.
        let geo = GuardedRowsGeometry::panel(w, action_rows_top(h), OPTION_ROW_H, OPTION_GAP);
        let items = [
            MenuItem { label: rx.t(Msg::RouteOverviewStartRide), guard: false },
            MenuItem { label: rx.t(Msg::RouteOverviewDelete), guard: true },
        ];
        let n = if self.delete_enabled(rx.activity, rx.recording, rx.routes) { 2 } else { 1 };
        draw_guarded_rows(cv, &items[..n], self.selected.min(n - 1), rx.hold_progress, WARNING, geo);
    }
}

/// Top of the two-row action block: two Pause-family rows + gap over the standard 10 px bottom
/// margin. Fixed at the two-row position regardless of whether Delete is drawn (see `draw`).
fn action_rows_top(h: i32) -> i32 {
    h - 10 - 2 * OPTION_ROW_H - OPTION_GAP
}

/// How close to its deletion deadline a route must be for the Auto-delete row to appear (epic #638
/// S5, owner review): **5 days**. The row is a "this route is about to be auto-deleted" heads-up,
/// not an always-on countdown — beyond this window it stays absent, reclaiming the vertical space.
/// Past-due (a deadline already elapsed, before the hourly sweep collects it) is inside the window.
const EXPIRY_SHOW_WINDOW: u32 = 5 * DAY_SECS;

/// The Route overview's **Auto-delete** row value for `meta` at `now_utc` (epic #638 S5), or `None`
/// when the row is **absent**. It shows **only** for a route with a *started* deadline
/// ([`expires_at`](RouteRetentionMeta::expires_at) is `Some` — retention ≠ `Never`
/// **and** `last_used != 0`) falling **within [`EXPIRY_SHOW_WINDOW`]** (past-due included). A `Never`
/// route, an unstarted clock (`last_used == 0`), and a deadline more than 5 days out all read `None`.
/// The value itself is [`expiry_short`]'s locked format ("in N d" / "in N h" / "soon").
fn expiry_value(meta: RouteRetentionMeta, now_utc: u32) -> Option<heapless::String<12>> {
    let deadline = meta.expires_at()?; // Never, or clock never started → absent
    (deadline.saturating_sub(now_utc) <= EXPIRY_SHOW_WINDOW).then(|| expiry_short(deadline, now_utc))
}

/// The route's length in metres for the time model: the **opened** route's exact total once it has
/// streamed in, else the catalog summary's whole-km figure. The estimate is a whole-minute readout,
/// so the km-grain fallback only matters for the frame or two before the geometry opens.
fn route_total_m(rx: &Render, summary: &RouteSummary) -> u32 {
    rx.route.map_or(summary.distance_km * 1000, |r| r.total_distance_m)
}

/// The route's total ascent in metres for the time model — the opened route's header figure, else
/// the catalog summary's. A route with no elevation at all (a computed one, until EL7) reports `0`
/// here, which is exactly what makes [`est_time_value`] fall back to `distance / v_flat` with no
/// branch of its own.
fn route_ascent_m(rx: &Render, summary: &RouteSummary) -> u32 {
    rx.route.map_or(summary.climb_m, |r| r.total_ascent_m)
}

/// The EST TIME ledger value: the whole route through the gradient-aware model
/// ([`obc_route::eta`], elevation epic #1068 / EL9) rendered `H:MM`, the same duration shape the
/// RIDE tile and the ride ledger use. Not localised and not unit-dependent — hours and minutes are
/// hours and minutes in every catalog language and both unit systems.
fn est_time_value(total_m: u32, ascent_m: u32, bike_profile_idx: u8) -> heapless::String<8> {
    duration_hms(obc_route::route_time_s(total_m, ascent_m, bike_profile_idx) as f32)
}

/// The "BIKE TYPE" ledger row: the profile name the computed route was planned under (routing-v2
/// N5), drawn at `y` under the DISTANCE row on the computed page in the same caption-left/
/// value-right shape. A stale/out-of-range index shows **profile 0's name** — the profile the
/// router actually fell back to for this plan (see [`NavProfiles::write_label`](crate::NavProfiles)).
fn draw_profile_label(cv: &mut impl Surface, w: i32, rx: &Render, y: i32) {
    let mut name: heapless::String<20> = heapless::String::new();
    rx.nav_profiles.write_label(rx.settings.bike_profile_idx, &mut name);
    ledger_row(cv, w, y, rx.t(Msg::RouteOverviewBikeType), &name, "", None);
}

/// The track-shape preview's box size (#685 §4): ≈212×90 px, horizontally centred, vertically
/// centred in whatever slot the caller hands it — the computed page's mid-gap, or the overview /
/// Ride detail media-band slots (owner review round 3's pagers).
const PREVIEW_W: i32 = 212;
const PREVIEW_H: i32 = 90;

/// Draw a track's shape preview: the host-decimated polyline (≤ 64 points) normalized
/// and aspect-fit into the [`PREVIEW_W`]×[`PREVIEW_H`] box — lon scaled by cos(mid-lat) so the
/// shape keeps its ground aspect — stroked 2 px INK (the doubled-1-px idiom), with a 4 px filled
/// disc at the start and a 6 px hollow diamond at the destination/end. An empty/short slice (the
/// frame or two before the host hands the preview in, or a stale one) draws nothing — the box
/// just stays empty, like the elevation page's "loading profile" band footprint. Shared with the
/// Ride detail's recorded-track page (#678 rework 3), so the two sketches can't drift.
pub(super) fn draw_route_preview(cv: &mut impl Surface, w: i32, top: i32, bot: i32, pts: &[(i32, i32)]) {
    use palette::*;
    if pts.len() < 2 {
        return;
    }
    // The box clamps to the caller's slot (the Ride detail's band is 82 px, under PREVIEW_H),
    // and the fit insets by the end markers' reach, so a disc/diamond on an extreme point can
    // never spill past the slot's baseline into the rows below (or graze the text above).
    const MARK: i32 = 4;
    let box_h = PREVIEW_H.min(bot - top);
    let (fit_w, fit_h) = (PREVIEW_W - 2 * MARK, box_h - 2 * MARK);
    let x0 = (w - PREVIEW_W) / 2 + MARK;
    let y0 = top + ((bot - top - box_h) / 2).max(0) + MARK;
    let (mut min_lon, mut max_lon, mut min_lat, mut max_lat) = (i32::MAX, i32::MIN, i32::MAX, i32::MIN);
    for &(lon, lat) in pts {
        min_lon = min_lon.min(lon);
        max_lon = max_lon.max(lon);
        min_lat = min_lat.min(lat);
        max_lat = max_lat.max(lat);
    }
    // Aspect-fit: one scale for both axes (the smaller of the two fits), the fitted shape
    // centred in the box. `max(1.0)` guards a degenerate straight north-south / east-west line.
    let clat = obc_map_scene::cos_lat((min_lat / 2) + (max_lat / 2));
    let geo_w = ((max_lon - min_lon) as f32 * clat).max(1.0);
    let geo_h = ((max_lat - min_lat) as f32).max(1.0);
    let scale = (fit_w as f32 / geo_w).min(fit_h as f32 / geo_h);
    let ox = x0 as f32 + (fit_w as f32 - geo_w * scale) / 2.0;
    let oy = y0 as f32 + (fit_h as f32 - geo_h * scale) / 2.0;
    let project = |(lon, lat): (i32, i32)| {
        Point::new((ox + (lon - min_lon) as f32 * clat * scale) as i32, (oy + (max_lat - lat) as f32 * scale) as i32)
    };
    let mut prev = project(pts[0]);
    for &p in &pts[1..] {
        let cur = project(p);
        stroke2(cv, prev, cur, INK);
        prev = cur;
    }
    // Start: a 4 px filled disc. Destination: a 6 px hollow diamond (its four 1 px edges).
    cv.disc(project(pts[0]), 2, INK);
    let d = project(pts[pts.len() - 1]);
    let k = 3;
    cv.line(Point::new(d.x, d.y - k), Point::new(d.x + k, d.y), INK);
    cv.line(Point::new(d.x + k, d.y), Point::new(d.x, d.y + k), INK);
    cv.line(Point::new(d.x, d.y + k), Point::new(d.x - k, d.y), INK);
    cv.line(Point::new(d.x - k, d.y), Point::new(d.x, d.y - k), INK);
}

/// START RIDE at the screen-bottom anchor (`h - 10 - BUTTON_H`): the computed-route variant and the
/// POI detail's `Route here` footer (#685), which is specified as exactly this bar, so the two can't
/// drift. Always armed (amber) with the play wedge — these pages have a single action, so there is
/// no cursor. The full page's START row is a Pause-family option row instead (owner review round 3).
pub(super) fn draw_start_button(cv: &mut impl Surface, w: i32, h: i32, label: &str) {
    use palette::*;
    let by = h - 10 - BUTTON_H;
    let bar = rect(SIDE_MARGIN, by, w - 2 * SIDE_MARGIN, BUTTON_H);
    cv.round(bar, 8, AMBER);
    let tx = w / 2 + 8;
    cv.text_vcentered(label, tx, (by, BUTTON_H), Font::Body, TextAlign::Center, INK);
    // Play wedge just left of the centred label — from its real half-width, so a longer
    // translation (or the POI detail's `Route here`) can't run into it.
    let px = tx - label.chars().count() as i32 * Font::Body.char_width() as i32 / 2 - 16;
    let mid = by + BUTTON_H / 2;
    cv.triangle(Point::new(px, mid - 7), Point::new(px, mid + 7), Point::new(px + 11, mid), INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Mode;
    use crate::retention::Retention;
    use crate::route::RouteSummary;
    use crate::screen::test_ctx;
    use crate::settings::Settings;
    use crate::AppState;
    use obc_map_scene::BBox;

    fn summary() -> RouteSummary {
        RouteSummary {
            name: heapless::String::try_from("A").unwrap(),
            distance_km: 10,
            climb_m: 100,
            bbox: BBox { min_lon: 0, min_lat: 0, max_lon: 0, max_lat: 0 },
            start_lon: 0,
            start_lat: 0,
        }
    }

    fn run(
        scr: &mut RouteOverviewScreen,
        act: &mut Activity,
        rec: &mut crate::RecorderMachine,
        routes: &[RouteSummary],
        g: Gesture,
    ) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        let mut cx = Ctx { routes, recorder: rec, ..test_ctx(&mut st, act, &mut settings) };
        scr.handle(g, &mut cx)
    }

    /// The action cursor (owner review round 2): entry selects START and a hold there does
    /// **nothing** — deleting takes a step onto the Delete row first, then the completed hold
    /// records the route's index, restores the pre-preview active route, and pops back to the
    /// Routes list.
    #[test]
    fn hold_deletes_only_from_the_selected_delete_row() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary(), summary()];
        let mut act = Activity::new(Mode::Idle);
        act.active_route = Some(1); // the menu preview
        let mut scr = RouteOverviewScreen::new(1, Some(0)); // was previewing route 0 before
        assert!(scr.delete_enabled(&act, rec.recording(), &routes), "an Idle preview is deletable");
        assert!(!scr.selection_is_guarded(&act, rec.recording(), &routes), "entry selects START — nothing armed");
        let t = run(&mut scr, &mut act, &mut rec, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold with START selected does not delete");
        assert_eq!(act.take_route_delete(), None);

        run(&mut scr, &mut act, &mut rec, &routes, Gesture::Step(1)); // → the Delete row
        assert!(scr.selection_is_guarded(&act, rec.recording(), &routes), "the hold fill is live on the Delete row");
        let t = run(&mut scr, &mut act, &mut rec, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::Pop), "the delete pops back to the Routes list");
        assert_eq!(act.take_route_delete(), Some(1), "records the previewed route's index");
        assert_eq!(act.active_route, Some(0), "the pre-preview route is restored");
    }

    /// A press fires the START action only from the START row — with the cursor on Delete a press
    /// does nothing (the row is hold-guarded), and a step brings the cursor back.
    #[test]
    fn press_on_the_delete_row_is_a_no_op() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary()];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RouteOverviewScreen::new(0, None);
        run(&mut scr, &mut act, &mut rec, &routes, Gesture::Step(1)); // → Delete
        let t = run(&mut scr, &mut act, &mut rec, &routes, Gesture::Press);
        assert!(matches!(t, Transition::None), "press on the Delete row starts nothing");
        assert!(!rec.recording(), "no session began");
        run(&mut scr, &mut act, &mut rec, &routes, Gesture::Step(1)); // wrap back → START
        let t = run(&mut scr, &mut act, &mut rec, &routes, Gesture::Press);
        assert!(!matches!(t, Transition::None), "press on START starts the ride");
    }

    /// The Delete row is hidden — a step has nothing to select and a hold does nothing — while
    /// this route is the active route of a running tracking session (the greying predicate moved
    /// off the old Route-menu footer).
    #[test]
    fn hold_over_the_active_ride_route_is_a_no_op() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary(), summary()];
        let mut act = Activity::new(Mode::Riding);
        rec.test_open(); // now tracking…
        act.active_route = Some(0); // …route 0
        let mut scr = RouteOverviewScreen::new(0, None);
        assert!(!scr.delete_enabled(&act, rec.recording(), &routes), "the active ride's route can't be deleted");
        run(&mut scr, &mut act, &mut rec, &routes, Gesture::Step(1));
        assert_eq!(scr.selected, START, "with the Delete row hidden there is nothing to toggle");
        let t = run(&mut scr, &mut act, &mut rec, &routes, Gesture::Hold);
        assert!(matches!(t, Transition::None), "a hold over the in-use route does nothing");
        assert_eq!(act.take_route_delete(), None);
    }

    /// A computed (length-only) overview has no Delete row, so a step stays on START and a hold is
    /// a no-op — the locked length-only page stays exactly as-is.
    #[test]
    fn computed_overview_has_no_delete() {
        let mut rec = crate::RecorderMachine::new();
        let routes = [summary()];
        let mut act = Activity::new(Mode::Idle);
        let mut scr = RouteOverviewScreen::computed(0, None);
        assert!(!scr.delete_enabled(&act, rec.recording(), &routes));
        run(&mut scr, &mut act, &mut rec, &routes, Gesture::Step(1));
        assert_eq!(scr.selected, START, "no Delete row — the step is a no-op");
        run(&mut scr, &mut act, &mut rec, &routes, Gesture::Hold);
        assert_eq!(act.take_route_delete(), None);
    }

    /// The page the pager selects is the page the content is paired with: the shape/DISTANCE page
    /// first, the elevation/CLIMB page after a dwell. The flip timing itself is the shared pager's
    /// (`vocab::pager`); what this pins is that this screen delegates it and reads it back.
    #[test]
    fn the_pager_drives_this_screen_s_paired_pages() {
        use super::super::vocab::pager::PAGE_FLIP_MS;
        let mut scr = RouteOverviewScreen::new(0, None);
        assert!(!scr.tick_timers(0).changed, "the first poll only anchors the dwell");
        assert!(!scr.pager.on_second_page(), "entry shows the track shape + DISTANCE page");
        assert!(scr.tick_timers(PAGE_FLIP_MS).changed, "the dwell flips the page");
        assert!(scr.pager.on_second_page(), "now on the elevation (CLIMB + DESCENT) page");
    }

    /// The computed page has a single fixed layout and no pager, so its tick never self-dirties.
    #[test]
    fn computed_overview_never_flips() {
        use super::super::vocab::pager::PAGE_FLIP_MS;
        let mut scr = RouteOverviewScreen::computed(0, None);
        assert!(!scr.tick_timers(PAGE_FLIP_MS).changed);
        assert_eq!(scr.tick_timers(PAGE_FLIP_MS), ScreenTick::idle());
    }

    /// The EST TIME ledger value (EL9, #1077): `H:MM` from the gradient-aware model, keyed by the
    /// rider's bike profile, with a zero-ascent route (a device-planned one, until EL7 fills its
    /// elevation from terrain) degrading to plain `distance / v_flat` — the natural behavior, not a
    /// special case, so the row never needs a "no elevation" branch or a `--`.
    #[test]
    fn est_time_value_is_the_gradient_aware_estimate() {
        // Road profile, 22 km/h flat: 44 km with no climbing is two hours.
        assert_eq!(est_time_value(44_000, 0, 0).as_str(), "2:00", "a flat route is distance / v_flat");
        // The same 44 km over a 1000 m col costs 1000 × 1.6 s = 26:40 more → 2:26.
        assert_eq!(est_time_value(44_000, 1_000, 0).as_str(), "2:26", "the col adds its climb term");
        // Same route, MTB profile (16 km/h, 2.3 s/m): 2:45 flat + 38:20 climbing → 3:23.
        assert_eq!(est_time_value(44_000, 1_000, 2).as_str(), "3:23", "a slower bike, a longer day");
        // A stale out-of-range index falls back to profile 0, the router's own rule.
        assert_eq!(est_time_value(44_000, 1_000, 99), est_time_value(44_000, 1_000, 0));
        // Degenerate inputs stay a readable zero rather than a placeholder.
        assert_eq!(est_time_value(0, 0, 0).as_str(), "0:00");
    }

    /// The Auto-delete row's ≤ 5-day presence gate (owner review): absent for `Never`, absent for an
    /// unstarted clock, absent for a deadline more than 5 days out, and shown (with the locked
    /// format) once the deadline is within 5 days — past-due included as "soon".
    #[test]
    fn expiry_value_gated_to_five_days() {
        let now = 100_000_000; // well past a month of seconds, so the `now - N*DAY_SECS` stamps don't underflow
                               // Never → absent.
        assert_eq!(expiry_value(RouteRetentionMeta::new(Retention::Never, now), now), None);
        // Retention set but the clock never started (last_used == 0) → absent (no more "--" state).
        assert_eq!(expiry_value(RouteRetentionMeta::new(Retention::Week1, 0), now), None);
        // A started deadline more than 5 days out → absent (30-day route used 24 days ago = 6 d left).
        let far = RouteRetentionMeta::new(Retention::Month1, now - 24 * DAY_SECS);
        assert_eq!(expiry_value(far, now), None, "6 days out → absent");
        // Exactly 5 days out → shown as "in 5 d" (30-day route used 25 days ago).
        let five = RouteRetentionMeta::new(Retention::Month1, now - 25 * DAY_SECS);
        assert_eq!(expiry_value(five, now).as_deref(), Some("in 5 d"), "exactly 5 days is inside the window");
        // A ≤ 48 h case → hours (1-day route used 19 h ago = 5 h left).
        let hours = RouteRetentionMeta::new(Retention::Day1, now - (DAY_SECS - 5 * 3600));
        assert_eq!(expiry_value(hours, now).as_deref(), Some("in 5 h"));
        // Sub-hour and past-due both fold to "soon", both inside the window.
        let subhour = RouteRetentionMeta::new(Retention::Day1, now - (DAY_SECS - 1800));
        assert_eq!(expiry_value(subhour, now).as_deref(), Some("soon"));
        let overdue = RouteRetentionMeta::new(Retention::Day1, now - 3 * DAY_SECS);
        assert_eq!(expiry_value(overdue, now).as_deref(), Some("soon"), "past-due is inside the window → soon");
    }
}
