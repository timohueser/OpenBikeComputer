//! Shared route planning and failure screens.
use embedded_graphics::prelude::Point;
use obc_formats::obcm::POI_NAME_LEN;
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::navigator::NavigatorIntent;
use crate::Msg;

use super::vocab::chrome::{card_triangle, title_frame, wrapped, TITLE_BAR_H};
use super::vocab::spinner::Spinner;
use super::{palette, Ctx, Render, ScreenTick, Transition};

/// Which plan a [`NavPlanningScreen`] spinner fronts — the POI create-route (#116) or the
/// mid-ride detour (#882). Selects the title/copy and which cancel one-shot Back posts; the
/// spinner mechanics are identical, so the two flows share one screen instead of a near-duplicate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    /// The POI create-route plan (`CancelRoutePlan` on Back).
    Nav,
    /// The detour plan (`CancelDetour` on Back; pops to the Detour chooser).
    Detour,
}

/// The planning screen (#499): up from confirm-accept until the host's answer replaces it. Shows
/// the shared compass needle spinning over plain copy. **Back = cancel** — pops to the caller
/// (POI detail / Detour chooser) and records the one-shot the host drains to abort the plan; no
/// failure card.
#[derive(Debug)]
pub struct NavPlanningScreen {
    /// Which flow's plan this spinner fronts (title/copy + which cancel Back posts).
    kind: PlanKind,
    /// The destination's name, echoed so the rider sees what's being planned (empty for a
    /// detour — its "destination" is the rejoin point already shown on the chooser).
    name: heapless::String<POI_NAME_LEN>,
    /// The shared wait spinner — the sweep, the repaint throttle, and the dirty disc.
    spin: Spinner,
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
        NavPlanningScreen { kind: PlanKind::Nav, name: nm, spin: Spinner::default() }
    }

    /// The planning screen for a detour plan (#882): detour title/copy, Back posts the detour
    /// cancel, and the host's `DetourPlanned` answer replaces it with the preview or fail card.
    pub fn detour() -> Self {
        NavPlanningScreen { kind: PlanKind::Detour, name: heapless::String::new(), spin: Spinner::default() }
    }

    /// Which flow's plan this spinner fronts — the event router keys its answer landing on it.
    pub fn kind(&self) -> PlanKind {
        self.kind
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Cancel: back to the caller (this screen replaced the confirm / was pushed over the
            // chooser, so one pop lands there), and ring the host so it aborts the plan and
            // discards the partial work. No failure card — the rider changed their mind.
            Gesture::Back => {
                match self.kind {
                    PlanKind::Nav => cx.navigator.admit_intent(NavigatorIntent::CancelPlan),
                    PlanKind::Detour => cx.navigator.admit_intent(NavigatorIntent::CancelDetour),
                }
                Transition::Pop
            }
            _ => Transition::None, // nothing else to do here — the plan finishes or is cancelled
        }
    }

    /// [`Screen::tick_timers`] arm: spin the shared needle while the host steps the planner —
    /// between the ride loop's planner steps this is what animates.
    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.spin.tick(now_ms, w, h)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        let (title, copy) = match self.kind {
            PlanKind::Nav => (Msg::NavRouteTitle, Msg::NavRouteFinding),
            PlanKind::Detour => (Msg::DetourTitle, Msg::DetourPlanning),
        };
        title_frame(cv, w, h, rx.t(title), "");
        // The destination, so the wait reads as *this* route being found (a detour has none —
        // its rejoin point was just shown on the chooser).
        if !self.name.is_empty() {
            let max = (((w - 24) / Font::Label.char_width() as i32).max(6)) as usize;
            let name_row = rect(12, TITLE_BAR_H + 16, w - 24, Font::Label.line_height() as i32);
            let name = rx.marquee.fit(&self.name, max, Some(name_row));
            cv.text(&name, Point::new(w / 2, TITLE_BAR_H + 16), Font::Label, TextAlign::Center, SUBTEXT);
        }

        // The spinner: the shared wait needle, free-spinning while the host steps the planner.
        self.spin.draw_needle(cv, w, h);

        // Label-tier: the full phrase overruns the panel at Body width (18 × 14 px > 240).
        cv.text(rx.t(copy), Point::new(w / 2, h * 72 / 100), Font::Label, TextAlign::Center, INK);
    }
}

/// The routing-failure card — the locked two-tier copy, info-only (any press/Back dismisses back
/// to the caller: the POI detail, or the Detour chooser for a detour-mode failure).
#[derive(Debug)]
pub struct NavFailScreen {
    /// `true` = the range tier ("Too far to route here."): the router's fixed table exhausted
    /// before the goal — with no distance cap, that **is** the device's range limit.
    /// `false` = every other failure ("Couldn't find a route.").
    too_far: bool,
    /// `true` = a detour-mode failure (#882): detour title, and both tiers share the one honest
    /// remedy hint — "try a farther rejoin" (the rejoin-distance escalation is the mechanism's
    /// semantic backstop, whatever the error kind).
    detour: bool,
}

impl NavFailScreen {
    /// The range tier: the search exhausted the fixed table — the target is beyond what the
    /// device can plan.
    pub fn too_far() -> Self {
        NavFailScreen { too_far: true, detour: false }
    }

    /// The generic tier: no snap, no path, a host-aborted search, or any host I/O failure.
    pub fn not_found() -> Self {
        NavFailScreen { too_far: false, detour: false }
    }

    /// The detour range tier (#882): the corridor-constrained search exhausted the table.
    pub fn detour_too_far() -> Self {
        NavFailScreen { too_far: true, detour: true }
    }

    /// The detour generic tier (#882): the corridor sealed every path, or any other failure.
    pub fn detour_not_found() -> Self {
        NavFailScreen { too_far: false, detour: true }
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

        let title = if self.detour { Msg::DetourTitle } else { Msg::NavRouteTitle };
        title_frame(cv, w, h, rx.t(title), "");
        // The shared warning triangle in the glyph slot (dialog anatomy, #678 T1) — the DFU error
        // cards' composition: title bar, gap, triangle, gap, message.
        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 46), 22);
        // The two-tier message (ink, Body) over its one olive guidance line (Label) — each authored
        // as one catalog string and word-wrapped at draw time (either overruns the 240 px panel).
        // A detour failure keeps the two-tier message but shares the one honest remedy hint (#882).
        let msg = if self.too_far { rx.t(Msg::NavRouteTooFar) } else { rx.t(Msg::NavRouteNotFound) };
        let hint = match (self.detour, self.too_far) {
            (true, _) => rx.t(Msg::DetourRejoinHint),
            (false, true) => rx.t(Msg::NavRouteTooFarHint),
            (false, false) => rx.t(Msg::NavRouteNotFoundHint),
        };
        let y = wrapped(cv, msg, w / 2, TITLE_BAR_H + 84, w - 32, Font::Body, INK);
        wrapped(cv, hint, w / 2, y + 12, w - 32, Font::Label, SUBTEXT);
    }
}
