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

/// Which plan the spinner fronts. It selects the copy and the cancel intent that Back posts; the
/// spinner mechanics are the same, so both flows share one screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanKind {
    /// The route plan to a POI.
    Nav,
    /// The mid-ride detour plan, which pops to the Detour chooser.
    Detour,
    /// The way to a route join point, which pops to the Start-away prompt.
    Approach,
    /// The rest of the day before and the next day, which the start card asked for.
    Day,
}

/// The planning screen: it stays up from the accepted confirmation until the host's answer
/// replaces it. Back cancels, pops to the caller, and records the one-shot that aborts the plan.
#[derive(Debug)]
pub struct NavPlanningScreen {
    kind: PlanKind,
    /// The name of the destination. It is empty for a detour, whose rejoin point the chooser
    /// already showed.
    name: heapless::String<POI_NAME_LEN>,
    spin: Spinner,
}

impl NavPlanningScreen {
    /// The planning screen for a route to `name`, which truncates at the POI name cap.
    pub fn new(name: &str) -> Self {
        let mut nm = heapless::String::new();
        for ch in name.chars() {
            if nm.push(ch).is_err() {
                break;
            }
        }
        NavPlanningScreen { kind: PlanKind::Nav, name: nm, spin: Spinner::default() }
    }

    /// The planning screen for a detour. The host's answer replaces it with the preview or the
    /// failure card.
    pub fn detour() -> Self {
        NavPlanningScreen { kind: PlanKind::Detour, name: heapless::String::new(), spin: Spinner::default() }
    }

    /// The planning screen for either route connection. Its plan is a detour-family search, so it cancels
    /// like one.
    pub fn approach() -> Self {
        NavPlanningScreen { kind: PlanKind::Approach, name: heapless::String::new(), spin: Spinner::default() }
    }

    /// The planning screen for a trip day built from the rest of the day before. Its splice is a
    /// detour-family operation, so it cancels like one.
    pub fn day(name: &str) -> Self {
        NavPlanningScreen { kind: PlanKind::Day, ..NavPlanningScreen::new(name) }
    }

    /// The event router keys the landing of an answer on this.
    pub fn kind(&self) -> PlanKind {
        self.kind
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // Back to the caller, and ring the host so it aborts the plan and discards the
            // partial work. There is no failure card: the rider changed their mind.
            Gesture::Back => {
                match self.kind {
                    PlanKind::Nav => cx.navigator.admit_intent(NavigatorIntent::CancelPlan),
                    PlanKind::Detour | PlanKind::Approach | PlanKind::Day => {
                        cx.navigator.admit_intent(NavigatorIntent::CancelDetour)
                    }
                }
                Transition::Pop
            }
            _ => Transition::None,
        }
    }

    /// Spin the needle between the planner steps of the host.
    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.spin.tick(now_ms, w, h)
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);

        let (title, copy) = match self.kind {
            PlanKind::Nav => (Msg::NavRouteTitle, Msg::NavRouteFinding),
            PlanKind::Detour => (Msg::DetourTitle, Msg::DetourPlanning),
            PlanKind::Approach => (Msg::StartAwayTitle, Msg::NavRouteFinding),
            PlanKind::Day => (Msg::RideStartTitle, Msg::RideStartLoadingDay),
        };
        title_frame(cv, w, h, rx.t(title), "");
        if !self.name.is_empty() {
            let name_row = rect(12, TITLE_BAR_H + 16, w - 24, Font::Label.line_height() as i32);
            let name = rx.marquee.fit(&self.name, w - 24, Font::Label, Some(name_row));
            cv.text(&name, Point::new(w / 2, TITLE_BAR_H + 16), Font::Label, TextAlign::Center, SUBTEXT);
        }

        self.spin.draw_needle(cv, w, h);

        // The copy is Label tier: the full phrase overruns the panel at Body width.
        cv.text(rx.t(copy), Point::new(w / 2, h * 72 / 100), Font::Label, TextAlign::Center, INK);
    }
}

/// The routing-failure card. It shows only information, and any press dismisses it to the caller.
#[derive(Debug)]
pub struct NavFailScreen {
    /// True for the range tier: the search table of the router became empty before the goal.
    /// False for every other failure.
    too_far: bool,
    /// True for a detour failure, where both tiers share the one remedy hint.
    detour: bool,
}

impl NavFailScreen {
    /// The range tier: the target is beyond what the device can plan.
    pub fn too_far() -> Self {
        NavFailScreen { too_far: true, detour: false }
    }

    /// The generic tier: no snap, no path, an aborted search, or any host I/O failure.
    pub fn not_found() -> Self {
        NavFailScreen { too_far: false, detour: false }
    }

    /// The detour range tier: the search in the corridor emptied the table.
    pub fn detour_too_far() -> Self {
        NavFailScreen { too_far: true, detour: true }
    }

    /// The detour generic tier: the corridor sealed every path, or any other failure.
    pub fn detour_not_found() -> Self {
        NavFailScreen { too_far: false, detour: true }
    }

    /// True when the card shows the range tier.
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
        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 46), 22);
        // Each line is one catalog string, wrapped at draw time, because either overruns the panel.
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
