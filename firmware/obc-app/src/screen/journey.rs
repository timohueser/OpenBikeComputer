//! Informational arrival and explicit recovery of an accepted Assistant journey.
use super::{
    palette::*,
    vocab::{
        card::{ActionRows, CardEvent},
        chrome::title_frame,
        rows::{GuardedRowsGeometry, MenuItem},
    },
    Ctx, Render, Transition,
};
use crate::{find_place::Action, navigator::ReviewStatus, Gesture, Msg};
use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

#[derive(Debug)]
pub struct JourneyScreen {
    pub(crate) resume: bool,
    rows: ActionRows,
    pub(crate) no_fix: bool,
}
impl JourneyScreen {
    pub(crate) fn new(resume: bool) -> Self {
        Self { resume, rows: ActionRows::new(0), no_fix: false }
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match self.rows.handle(g, &[false]) {
            CardEvent::Dismiss => {
                cx.find.action = if self.resume { Action::DismissResume } else { Action::DismissArrival };
                Transition::Pop
            }
            CardEvent::Activate(_) if !self.resume => {
                cx.find.action = Action::DismissArrival;
                Transition::Pop
            }
            CardEvent::Activate(_) if cx.find.review == ReviewStatus::ResumeAvailable => {
                cx.find.action = Action::Resume;
                Transition::None
            }
            _ => Transition::None,
        }
    }
    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        title_frame(
            cv,
            rx.w,
            rx.h,
            rx.t(if self.resume { Msg::AssistantResumeJourney } else { Msg::AssistantArrival }),
            "",
        );
        cv.text(
            rx.t(if self.no_fix {
                Msg::AssistantNoFix
            } else if self.resume {
                Msg::AssistantSavedJourney
            } else {
                Msg::AssistantGuidanceContinues
            }),
            Point::new(rx.w / 2, 112),
            Font::Label,
            TextAlign::Center,
            INK,
        );
        let label = if !self.resume {
            Msg::AssistantBackMap
        } else {
            match rx.find.review {
                ReviewStatus::ResumeAvailable => Msg::AssistantResumeRoute,
                ReviewStatus::Saving => Msg::AssistantSaving,
                ReviewStatus::Unresolved => Msg::AssistantSaveUnknown,
                _ => Msg::AssistantVisitUnavailable,
            }
        };
        self.rows.draw(
            cv,
            &[MenuItem { label: rx.t(label), guard: false }],
            0.0,
            AMBER,
            GuardedRowsGeometry::panel(rx.w, 248, 42, 8),
        );
    }
}
