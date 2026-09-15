//! Storage-full dialog: choose an age, then explicitly confirm route deletion.
use super::vocab::{
    card::{ActionRows, CardEvent},
    chrome::{title_frame, wrapped, TITLE_BAR_H},
    rows::{GuardedRowsGeometry, MenuItem},
};
use super::{palette, Ctx, Render, Transition};
use crate::device_core::StoreIdentity;
use crate::{input::Gesture, Msg};
use obc_render::Surface;

const WEEKS: [u32; 6] = [1, 2, 4, 8, 12, 26];
const GUARDS: [bool; 3] = [false, true, false];

#[derive(Debug)]
pub struct RouteCleanupScreen {
    now_utc: Option<u32>,
    store: StoreIdentity,
    age: usize,
    result: Option<bool>,
    running: bool,
    removed: u16,
    actions: ActionRows,
}

impl RouteCleanupScreen {
    pub fn new(now_utc: Option<u32>, store: StoreIdentity) -> Self {
        Self { now_utc, store, age: 2, result: None, running: false, removed: 0, actions: ActionRows::new(2) }
    }
    pub fn selection_is_guarded(&self) -> bool {
        !self.running && self.result.is_none() && self.actions.selection_is_guarded(&GUARDS)
    }
    pub(crate) fn cancel(&mut self) {
        self.running = false;
        self.result = Some(false);
    }
    pub(crate) fn progress(&mut self, outcome: crate::catalog_state::CatalogOutcome) {
        use crate::catalog_state::CatalogOutcome::*;
        if !self.running {
            return;
        }
        match outcome {
            ObjectRemoved { .. } => self.removed = self.removed.saturating_add(1),
            CleanupFinished { .. } => {
                self.running = false;
                self.result = Some(true);
            }
            Failed { .. } | Cancelled { .. } => {
                self.running = false;
                self.result = Some(false);
            }
            _ => {}
        }
    }
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.running {
            return Transition::None;
        }
        if self.result == Some(true) && self.removed == 0 && matches!(g, Gesture::Press) {
            self.result = None;
            return Transition::None;
        }
        if self.result.is_some() || self.now_utc.is_none() {
            return if matches!(g, Gesture::Press | Gesture::Hold | Gesture::Back) {
                Transition::Pop
            } else {
                Transition::None
            };
        }
        match self.actions.handle(g, &GUARDS) {
            CardEvent::Activate(0) => {
                self.age = (self.age + 1) % WEEKS.len();
                Transition::None
            }
            CardEvent::Activate(1) => {
                cx.activity.cleanup_routes = Some(crate::catalog_state::CatalogIntent::CleanupRoutes {
                    before_utc: self.now_utc.unwrap().saturating_sub(WEEKS[self.age] * 7 * 86400),
                    store: self.store,
                });
                self.running = true;
                Transition::None
            }
            CardEvent::Activate(2) | CardEvent::Dismiss => Transition::Pop,
            _ => Transition::None,
        }
    }
    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        title_frame(cv, rx.w, rx.h, rx.t(Msg::RouteCleanupTitle), "");
        if self.running || self.result.is_some() {
            let message = match self.result {
                None => Msg::RouteCleanupWorking,
                Some(false) => Msg::RouteCleanupFailed,
                Some(true) if self.removed == 0 => Msg::RouteCleanupNoMatches,
                Some(true) => Msg::RouteCleanupDone,
            };
            wrapped(
                cv,
                rx.t(message),
                rx.w / 2,
                TITLE_BAR_H + 24,
                rx.w - 24,
                obc_render::text::Font::Label,
                palette::SUBTEXT,
            );
            return;
        }
        let message = if self.now_utc.is_some() { Msg::RouteCleanupBody } else { Msg::RouteCleanupUnknownClock };
        let end = wrapped(
            cv,
            rx.t(message),
            rx.w / 2,
            TITLE_BAR_H + 12,
            rx.w - 24,
            obc_render::text::Font::Label,
            palette::SUBTEXT,
        );
        if self.now_utc.is_none() {
            return;
        }
        let age = rx.t([
            Msg::RouteCleanupAge1,
            Msg::RouteCleanupAge2,
            Msg::RouteCleanupAge4,
            Msg::RouteCleanupAge8,
            Msg::RouteCleanupAge12,
            Msg::RouteCleanupAge26,
        ][self.age]);
        let items = [
            MenuItem { label: age, guard: false },
            MenuItem { label: rx.t(Msg::RouteCleanupDelete), guard: true },
            MenuItem { label: rx.t(Msg::TripDeleteCancel), guard: false },
        ];
        self.actions.draw(cv, &items, rx.hold_progress, palette::WARNING, GuardedRowsGeometry::card(rx.w, end + 8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        activity::{Activity, Mode},
        AppState, Settings,
    };

    #[test]
    fn age_picker_requires_an_explicit_hold_and_defaults_to_cancel() {
        let mut screen = RouteCleanupScreen::new(Some(10_000_000), StoreIdentity::new(1));
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = super::super::test_ctx(&mut state, &mut activity, &mut settings);
        assert!(!screen.selection_is_guarded());
        assert!(matches!(screen.handle(Gesture::Press, &mut cx), Transition::Pop));
        screen.handle(Gesture::Step(1), &mut cx); // Cancel -> age picker
        screen.handle(Gesture::Press, &mut cx); // four -> eight weeks
        screen.handle(Gesture::Step(1), &mut cx);
        screen.handle(Gesture::Press, &mut cx);
        assert!(cx.activity.cleanup_routes.is_none());
        screen.handle(Gesture::Hold, &mut cx);
        assert_eq!(
            cx.activity.cleanup_routes,
            Some(crate::catalog_state::CatalogIntent::CleanupRoutes {
                before_utc: 10_000_000 - 8 * 7 * 86400,
                store: StoreIdentity::new(1),
            })
        );
    }

    #[test]
    fn unknown_clock_never_offers_deletion() {
        let mut screen = RouteCleanupScreen::new(None, StoreIdentity::new(1));
        let mut state = AppState::new(0, 0, 1.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = super::super::test_ctx(&mut state, &mut activity, &mut settings);
        screen.handle(Gesture::Step(-1), &mut cx);
        screen.handle(Gesture::Hold, &mut cx);
        assert!(cx.activity.cleanup_routes.is_none());
    }
}
