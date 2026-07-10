//! The POI **create-route** flow's screens (epic #116, R4 + the #499 resumable planner):
//!
//! - [`NavConfirmScreen`] — the "Create a route?" confirm, reached by pressing a POI's
//!   [detail](super::PoiDetailScreen). *Create route* records a one-shot
//!   [`NavRequest`](crate::activity::NavRequest) (rider fix → POI coord + the POI's name) and
//!   swaps itself for the planning screen; the host drains the request via
//!   [`App::take_nav_request`](crate::App::take_nav_request) and steps the resumable router.
//! - [`NavPlanningScreen`] — up while the host plans (#499): a **spinning compass needle** (the
//!   Menu dial's needle, shared drawing) over plain copy, animated by
//!   [`tick_timers`](NavPlanningScreen::tick_timers) between the host's planner steps. **Back
//!   cancels**: it pops straight back to the POI detail *and* records a one-shot the host drains
//!   ([`App::take_nav_cancel`](crate::App::take_nav_cancel)) to abort the plan and discard the
//!   partial file — no failure card, the rider changed their mind. The host's answer
//!   ([`App::notify_nav_result`](crate::App::notify_nav_result)) replaces this screen with the
//!   computed-route [overview](super::RouteOverviewScreen) (success) or the [`NavFailScreen`].
//! - [`NavFailScreen`] — the locked **two-tier failure** card: `Exhausted` → "Too far to route
//!   here." (there is no distance cap; the router's fixed table running out **is** the device's
//!   range limit), everything else → "Couldn't find a route." Info-only, like the
//!   [`RouteUpdated`](super::route_received::RouteUpdatedScreen) card: any press/Back dismisses,
//!   returning to the POI detail underneath.
//!
//! The request needs the rider's position; the POI browser already required a fix to snapshot, so
//! [`AppState::user_fix`](crate::AppState::user_fix) is essentially always present here. If it
//! genuinely isn't, the confirm degrades straight to the "Couldn't find a route." tier.

use embedded_graphics::{
    prelude::{Point, Size},
    primitives::Rectangle,
};
use obc_reader::{PoiCategory, POI_NAME_MAX};
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::NavRequest;
use crate::input::Gesture;
use crate::settings::Units;
use crate::Msg;

use super::{list, palette, title_frame, Ctx, MenuItem, Render, Screen, ScreenTick, Transition};

/// The two confirm rows (Create route / Cancel), neither guarded — labels looked up per language at
/// draw time (see [`NavConfirmScreen::draw`]).
const N_ITEMS: usize = 2;

const CREATE: usize = 0;

/// The "Create a route?" confirm. Carries the target POI's coordinate, its display/route name
/// (the stored name, or the subtype fallback label — the list row's convention), and its category
/// (for the glyph slot, #685 §3), plus the highlighted option.
#[derive(Debug)]
pub struct NavConfirmScreen {
    /// The POI coordinate, `(lon, lat)` µdeg — the route's goal.
    to: (i32, i32),
    /// The route's name-to-be (what the emitted OBCR is titled and the catalog lists).
    name: heapless::String<POI_NAME_MAX>,
    /// The destination's POI category, drawn as its pixel icon in the T1 glyph slot above the
    /// name (#685 §3) — `None` (an unmapped subtype; shouldn't happen for a queried POI) just
    /// leaves the slot empty.
    category: Option<PoiCategory>,
    selected: usize,
}

impl NavConfirmScreen {
    /// The confirm for a route to `to`, named `name` (truncated to the POI name cap), showing
    /// `category`'s glyph.
    pub fn new(to: (i32, i32), name: &str, category: Option<PoiCategory>) -> Self {
        let mut nm = heapless::String::new();
        for ch in name.chars() {
            if nm.push(ch).is_err() {
                break;
            }
        }
        NavConfirmScreen { to, name: nm, category, selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => list::on_turn(&mut self.selected, n, N_ITEMS),
            Gesture::Press if self.selected == CREATE => {
                // The request's start is the rider's fix. No position at all (never a fix this
                // session) can't be routed — degrade to the generic failure tier rather than
                // sending the host a garbage start.
                let Some(fix) = cx.state.user_fix else {
                    return Transition::Replace(Screen::NavFail(NavFailScreen::not_found()));
                };
                cx.activity.request_nav(NavRequest::new((fix.lon, fix.lat), self.to, &self.name));
                // Swap to the planning screen (#499): the host steps the resumable router across
                // its passes and answers into it — the UI stays live (spinner + Back-to-cancel).
                Transition::Replace(Screen::NavPlanning(NavPlanningScreen::new(&self.name)))
            }
            Gesture::Press => Transition::Pop, // Cancel
            Gesture::Back => Transition::Pop,  // Back = Cancel (return to the POI detail)
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::NavRouteTitle), "");
        // The destination's category glyph in the T1 glyph slot (the failure cards' triangle
        // position) — what kind of place the route goes to, at a glance (#685 §3).
        if let Some(cat) = self.category {
            super::poi_menu::draw_category_icon(cv, cat, Point::new(w / 2, super::TITLE_BAR_H + 40), INK, PARCHMENT);
        }
        // The destination's name — what the rider is routing to.
        let max = (((w - 24) / Font::Label.char_width() as i32).max(6)) as usize;
        let name = super::route_menu::fit_name(&self.name, max);
        let name_y = super::TITLE_BAR_H + 68;
        cv.text(&name, Point::new(w / 2, name_y), Font::Label, TextAlign::Center, SUBTEXT);
        // The straight-line distance to it (#685 §3) — the number that sets expectations before
        // committing to a plan. Current fix → POI; the browser required a fix to get here, so it's
        // essentially always present (a fix-less confirm just omits the line).
        if let Some(fix) = rx.state.user_fix {
            let d_m = obc_route::ground_dist_m((fix.lon, fix.lat), self.to) as u32;
            let mut away: heapless::String<20> = heapless::String::new();
            write_away(&mut away, d_m, rx.settings.units, rx.t(Msg::NavRouteAway));
            cv.text(&away, Point::new(w / 2, name_y + 24), Font::Label, TextAlign::Center, SUBTEXT);
        }

        let geo = super::GuardedRowsGeometry {
            x: 12,
            w: w - 24,
            top: super::TITLE_BAR_H + 122,
            row_h: 46,
            gap: 8,
            label_dx: 16,
            label_dy: 11,
        };
        let items = [
            MenuItem { label: rx.t(Msg::NavRouteCreateRoute), guard: false },
            MenuItem { label: rx.t(Msg::NavRouteCancel), guard: false },
        ];
        super::draw_guarded_rows(cv, &items, self.selected, rx.hold_progress, AMBER, geo);
    }
}

/// Write the confirm's straight-line readout: `600 m away` below 1 km, else `2.3 km away` (one
/// decimal) — the imperial twin is whole feet below a mile, else one-decimal miles (the
/// [`write_off_route`](super::write_off_route) thresholds). `away` is the catalog's trailing word,
/// so the phrase translates as a unit-value + suffix.
fn write_away<const N: usize>(s: &mut heapless::String<N>, d_m: u32, units: Units, away: &str) {
    use crate::settings::{FT_PER_M, FT_PER_MI};
    use core::fmt::Write;
    if units.is_imperial() {
        let ft = (d_m as f32 * FT_PER_M) as u32;
        if ft >= FT_PER_MI {
            let _ = write!(s, "{:.1} mi {away}", ft as f32 / FT_PER_MI as f32);
        } else {
            let _ = write!(s, "{ft} ft {away}");
        }
    } else if d_m >= 1000 {
        let _ = write!(s, "{:.1} km {away}", d_m as f32 / 1000.0);
    } else {
        let _ = write!(s, "{d_m} m {away}");
    }
}

/// Degrees per second the planning spinner sweeps — a calm, steady rotation (one revolution per
/// 1.5 s), advanced by real elapsed millis so the speed reads the same at any host frame rate.
const SPIN_DPS: f32 = 240.0;

/// Frame cadence the spinner repaints at *and* asks the host to wake for — smooth enough for a
/// needle, cheap enough that a multi-second plan isn't dominated by repaints. This is a hard
/// throttle, not just a wake request: during a plan the ride loop passes by every planner step
/// (far faster than this), and each claimed repaint costs a full chrome render + push (~40 ms on
/// glass) — unthrottled, the spinner starves the planner it's decorating (#500).
const SPIN_FRAME_MS: u32 = 66;

/// The spinner needle's sweep radius (px) — [`draw`](NavPlanningScreen::draw) passes it to the
/// shared `draw_needle`, and [`needle_region`] sizes the reported dirty disc from it, so the two
/// can't drift.
const NEEDLE_R: f32 = 42.0;

/// Half-extent (px) of [`needle_region`]'s square around the needle's centre: the [`NEEDLE_R`]
/// sweep plus a rounding margin for the rasterizer (`draw_needle` rounds its triangle vertices
/// away from zero, and the hub discs sit inside the sweep).
const NEEDLE_CLIP_HALF: i32 = NEEDLE_R as i32 + 2;

/// The square the spinning needle repaints inside, centred on the `(w/2, h/2)` the
/// [`draw`](NavPlanningScreen::draw) spins it at — everything else on the planning screen (title
/// bar, destination name, copy) is static while a plan runs. This is the dirty region
/// [`tick_timers`](NavPlanningScreen::tick_timers) reports so the host can clip the repaint;
/// `nav.rs`'s `needle_region_covers_the_spin` pins that the sweep never escapes it.
pub fn needle_region(w: i32, h: i32) -> Rectangle {
    let (cx, cy) = (w / 2, h / 2);
    Rectangle::new(
        Point::new(cx - NEEDLE_CLIP_HALF, cy - NEEDLE_CLIP_HALF),
        Size::new(2 * NEEDLE_CLIP_HALF as u32 + 1, 2 * NEEDLE_CLIP_HALF as u32 + 1),
    )
}

/// The planning screen (#499): up from confirm-accept until the host's answer replaces it. Shows
/// the shared compass needle spinning over plain copy. **Back = cancel** — pops to the POI detail
/// and records the one-shot the host drains to abort the plan; no failure card.
#[derive(Debug)]
pub struct NavPlanningScreen {
    /// The destination's name, echoed so the rider sees what's being planned.
    name: heapless::String<POI_NAME_MAX>,
    /// The spinner's current angle (0° = N, clockwise), advanced in [`tick_timers`].
    needle_deg: f32,
    /// Clock of the previous spin tick, for the per-frame `dt`; `None` before the first.
    last_ms: Option<u32>,
    /// Clock of the last tick that **claimed a repaint** — the [`SPIN_FRAME_MS`] throttle's
    /// anchor, distinct from `last_ms` (the needle advances every tick; the glass repaints at
    /// the spinner cadence).
    last_paint_ms: Option<u32>,
}

impl NavPlanningScreen {
    /// The planning screen for a route to `name` (truncated to the POI name cap).
    pub fn new(name: &str) -> Self {
        let mut nm = heapless::String::new();
        for ch in name.chars() {
            if nm.push(ch).is_err() {
                break;
            }
        }
        NavPlanningScreen { name: nm, needle_deg: 0.0, last_ms: None, last_paint_ms: None }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Cancel: back to the POI detail (this screen replaced the confirm, so one pop lands
            // there), and ring the host so it aborts the plan + discards the partial file. No
            // failure card — the rider changed their mind, nothing failed.
            Gesture::Back => {
                cx.activity.request_nav_cancel();
                Transition::Pop
            }
            _ => Transition::None, // nothing else to do here — the plan finishes or is cancelled
        }
    }

    /// [`Screen::tick_timers`] arm: spin the needle by real elapsed time and keep the host's
    /// frame cadence armed — between the ride loop's planner steps this is what animates.
    /// `changed` is claimed at most once per [`SPIN_FRAME_MS`] no matter how often the loop
    /// passes (see the constant's doc — an unthrottled claim per planner step starved the plan
    /// with ~40 ms chrome repaints, #500); the needle still advances by the full elapsed time,
    /// so a throttled frame just shows a slightly larger sweep.
    ///
    /// The claim carries the [`needle_region`] as its dirty region — the chrome around the
    /// needle never changes while planning — so the host repaints (and pushes) only the disc.
    /// `w`/`h` of 0 (no frame rendered yet) abstains: `None` = full repaint.
    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        let dt = self.last_ms.map_or(0.0, |last| now_ms.wrapping_sub(last) as f32 / 1000.0);
        self.last_ms = Some(now_ms);
        self.needle_deg = (self.needle_deg + SPIN_DPS * dt.min(0.25)) % 360.0;
        let due = self.last_paint_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= SPIN_FRAME_MS);
        if due {
            self.last_paint_ms = Some(now_ms);
        }
        let region = (w > 0 && h > 0).then(|| needle_region(w, h));
        ScreenTick { changed: due && dt > 0.0, next_wake_ms: Some(SPIN_FRAME_MS), region }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::NavRouteTitle), "");
        // The destination, so the wait reads as *this* route being found.
        let max = (((w - 24) / Font::Label.char_width() as i32).max(6)) as usize;
        let name = super::route_menu::fit_name(&self.name, max);
        cv.text(&name, Point::new(w / 2, super::TITLE_BAR_H + 16), Font::Label, TextAlign::Center, SUBTEXT);

        // The spinner: the Menu dial's needle (shared drawing), free-spinning while the host
        // steps the planner. Centre + radius are what `needle_region` promises the host — keep
        // any change to them inside its bound.
        super::menu::draw_needle(cv, Point::new(w / 2, h / 2), self.needle_deg, NEEDLE_R, 10.0);

        // Label-tier: the full phrase overruns the panel at Body width (18 × 14 px > 240).
        cv.text(rx.t(Msg::NavRouteFinding), Point::new(w / 2, h * 72 / 100), Font::Label, TextAlign::Center, INK);
    }
}

/// The routing-failure card — the locked two-tier copy, info-only (any press/Back dismisses back
/// to the POI detail).
#[derive(Debug)]
pub struct NavFailScreen {
    /// `true` = the range tier ("Too far to route here."): the router's fixed table exhausted
    /// before the goal — with no distance cap, that **is** the device's range limit.
    /// `false` = every other failure ("Couldn't find a route.").
    too_far: bool,
}

impl NavFailScreen {
    /// The range tier: the search exhausted the fixed table — the target is beyond what the
    /// device can plan.
    pub fn too_far() -> Self {
        NavFailScreen { too_far: true }
    }

    /// The generic tier: no snap, no path, a host-aborted search, or any host I/O failure.
    pub fn not_found() -> Self {
        NavFailScreen { too_far: false }
    }

    /// Which tier the card shows (`true` = "Too far to route here.") — lets the seam tests pin
    /// the error→tier mapping without reading pixels.
    pub fn shows_too_far(&self) -> bool {
        self.too_far
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press | Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        title_frame(cv, w, h, rx.t(Msg::NavRouteTitle), "");
        // The shared warning triangle in the glyph slot (dialog anatomy, #678 T1) — the DFU error
        // cards' composition: title bar, gap, triangle, gap, message.
        super::card_triangle(cv, Point::new(w / 2, super::TITLE_BAR_H + 46), 22);
        // The two-tier message (ink, Body) over its one olive guidance line (Label) — each authored
        // as one catalog string and word-wrapped at draw time (either overruns the 240 px panel).
        let (msg, hint) = if self.too_far {
            (rx.t(Msg::NavRouteTooFar), rx.t(Msg::NavRouteTooFarHint))
        } else {
            (rx.t(Msg::NavRouteNotFound), rx.t(Msg::NavRouteNotFoundHint))
        };
        let y = super::wrapped(cv, msg, w / 2, super::TITLE_BAR_H + 84, w - 32, Font::Body, INK);
        super::wrapped(cv, hint, w / 2, y + 12, w - 32, Font::Label, SUBTEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn away(d_m: u32, units: Units) -> heapless::String<20> {
        let mut s = heapless::String::new();
        write_away(&mut s, d_m, units, "away");
        s
    }

    /// Metric: whole metres below 1 km, one-decimal km from there (#685 §3's exact examples).
    #[test]
    fn away_metric_switches_at_one_km() {
        assert_eq!(away(600, Units::Metric).as_str(), "600 m away");
        assert_eq!(away(999, Units::Metric).as_str(), "999 m away");
        assert_eq!(away(1000, Units::Metric).as_str(), "1.0 km away");
        assert_eq!(away(2300, Units::Metric).as_str(), "2.3 km away");
    }

    /// Imperial twin: whole feet below a mile, one-decimal miles above (write_off_route's
    /// thresholds).
    #[test]
    fn away_imperial_switches_at_one_mile() {
        assert_eq!(away(100, Units::Imperial).as_str(), "328 ft away");
        assert_eq!(away(2000, Units::Imperial).as_str(), "1.2 mi away");
    }
}
