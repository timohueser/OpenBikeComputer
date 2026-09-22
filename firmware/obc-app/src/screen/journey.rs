//! Informational arrival and explicit recovery of the saved route, ordinary or Assistant.
use super::{
    palette::*,
    vocab::{
        card::{ActionRows, CardEvent},
        chrome::{title_frame, wrapped},
        rows::{GuardedRowsGeometry, MenuItem},
    },
    Ctx, Render, Transition,
};
use crate::{find_place::Action, navigator::ReviewStatus, Gesture, Msg};
use obc_render::{text::Font, Surface};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum JourneyError {
    NoFix,
    Unmatched,
    SourceChanged,
}
impl JourneyError {
    fn message(self) -> Msg {
        match self {
            Self::NoFix => Msg::AssistantNoFix,
            Self::Unmatched => Msg::AssistantUnmatched,
            Self::SourceChanged => Msg::AssistantUnavailable,
        }
    }
}
#[derive(Debug)]
pub struct JourneyScreen {
    pub(crate) resume: bool,
    rows: ActionRows,
    pub(crate) error: Option<JourneyError>,
}
impl JourneyScreen {
    pub(crate) fn new(resume: bool) -> Self {
        Self { resume, rows: ActionRows::new(0), error: None }
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
            rx.t(if self.resume { Msg::AssistantResumeNavigation } else { Msg::AssistantArrival }),
            "",
        );
        wrapped(
            cv,
            rx.t(self.error.map(JourneyError::message).unwrap_or(if self.resume {
                Msg::AssistantSavedRoute
            } else {
                Msg::AssistantGuidanceContinues
            })),
            rx.w / 2,
            112,
            rx.w - 32,
            Font::Label,
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
