//! The boot-time recovered-ride decision card.
//!
//! A durable `RECORDING` object survived a reset, so the rider must explicitly choose what happens
//! to it. **Continue ride** attaches a new app session to the recovered recorder without clearing
//! the restored totals. **Discard** is destructive and therefore fires only on a completed Select
//! hold; it posts the ordinary [`TrackAction::Discard`] for the host and returns Home. Back cannot
//! dismiss the decision and strand the recovered object.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::activity::TrackAction;
use crate::input::Gesture;
use crate::stat_fields::{fmt_hms, fmt_km};
use crate::Msg;

use super::{
    ledger_row, list, palette, title_frame, Ctx, GuardedRowsGeometry, MapScreen, MenuItem, Render, Screen, Transition,
};

const CONTINUE: usize = 0;
const DISCARD: usize = 1;
const GUARDS: [bool; 2] = [false, true];

const LEDGER_TOP: i32 = 70;
const LEDGER_PITCH: i32 = 38;
const OPTIONS_TOP: i32 = 204;

/// The one-shot recovered-ride offer. State is only the highlighted choice.
#[derive(Debug, Default)]
pub struct RideRecoveryScreen {
    selected: usize,
    can_continue: bool,
}

impl RideRecoveryScreen {
    pub fn new() -> Self {
        Self { selected: CONTINUE, can_continue: true }
    }

    /// A fail-closed recovery whose bytes/metadata could not be proven. It may be discarded but
    /// never attached to a live session.
    pub fn damaged() -> Self {
        Self { selected: DISCARD, can_continue: false }
    }

    /// Whether the highlighted choice needs a completed hold.
    pub fn selection_is_guarded(&self) -> bool {
        !self.can_continue || GUARDS[self.selected.min(GUARDS.len() - 1)]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) if self.can_continue => list::on_step(&mut self.selected, n, GUARDS.len()),
            Gesture::Press if self.can_continue && self.selected == CONTINUE => {
                // The host's recovered recorder has no app session attached yet. Mint one through
                // the continuation edge, which tells RideEngine not to run its fresh-session reset.
                let (lon, lat) = cx.state.user_fix.map_or((cx.state.cam_lon, cx.state.cam_lat), |f| (f.lon, f.lat));
                cx.state.enter_riding_view(lon, lat);
                cx.activity.mode = crate::activity::Mode::Riding;
                cx.activity.active_route = None;
                cx.activity.continue_session();
                Transition::Root(Screen::Map(MapScreen::new()))
            }
            Gesture::Hold if self.selected == DISCARD => {
                cx.activity.request_track(TrackAction::Discard);
                cx.activity.end_session();
                cx.activity.mode = crate::activity::Mode::Idle;
                cx.activity.active_route = None;
                Transition::Home
            }
            // Press on Discard is deliberately inert, and Back cannot bypass the decision.
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::RideRecoveryTitle), "");
        cv.text(
            rx.t(if self.can_continue { Msg::RideRecoveryBody } else { Msg::RideRecoveryDamaged }),
            Point::new(w / 2, super::TITLE_BAR_H + 12),
            Font::Label,
            TextAlign::Center,
            SUBTEXT,
        );

        // Show the totals the continuation path promises to keep. At a recovery boundary where a
        // host could restore only part of the summary these safely render zero, never invented data.
        let units = rx.settings.units;
        let activity = rx.activity;
        let time = fmt_hms(activity.moving_s);
        let distance = fmt_km(units.dist(activity.ridden_m / 1000.0));
        let distance_unit = if units.is_imperial() { "mi" } else { "km" };
        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", units.elev(activity.climb_m()) as u32);
        let rows: [(&str, &str, &str, Option<bool>); 3] = [
            (rx.t(Msg::RideControlRideTime), &time, "", None),
            (rx.t(Msg::RideControlDistance), &distance, distance_unit, None),
            (rx.t(Msg::RideControlClimb), &climb, units.elev_label(), Some(true)),
        ];
        for (i, (caption, value, unit, arrow)) in rows.iter().enumerate() {
            let y = LEDGER_TOP + i as i32 * LEDGER_PITCH;
            ledger_row(cv, w, y, caption, value, unit, *arrow);
            if i + 1 < rows.len() {
                cv.hline(16, y + LEDGER_PITCH - 4, w - 32, RULE);
            }
        }

        if self.can_continue {
            let items = [
                MenuItem { label: rx.t(Msg::RideRecoveryContinueRide), guard: GUARDS[CONTINUE] },
                MenuItem { label: rx.t(Msg::RideRecoveryDiscard), guard: GUARDS[DISCARD] },
            ];
            draw_rows(cv, &items, self.selected, rx.hold_progress, w);
        } else {
            let items = [MenuItem { label: rx.t(Msg::RideRecoveryDiscard), guard: true }];
            draw_rows(cv, &items, 0, rx.hold_progress, w);
        }
    }
}

fn draw_rows(cv: &mut impl Surface, items: &[MenuItem], selected: usize, hold_progress: f32, w: i32) {
    super::draw_guarded_rows(
        cv,
        items,
        selected,
        hold_progress,
        palette::WARNING,
        GuardedRowsGeometry::panel(w, OPTIONS_TOP, 42, 8),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    fn handle(screen: &mut RideRecoveryScreen, activity: &mut Activity, gesture: Gesture) -> Transition {
        let mut state = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        screen.handle(gesture, &mut test_ctx(&mut state, activity, &mut settings))
    }

    #[test]
    fn continue_mints_a_session_and_uses_the_recovery_transition() {
        let mut activity = Activity::new(Mode::Idle);
        let transition = handle(&mut RideRecoveryScreen::new(), &mut activity, Gesture::Press);
        assert!(matches!(transition, Transition::Root(Screen::Map(_))));
        assert!(activity.is_tracking());
        assert_eq!(activity.mode, Mode::Riding);
        assert_eq!(activity.active_route, None);
    }

    #[test]
    fn discard_needs_a_hold_and_posts_the_existing_action() {
        let mut activity = Activity::new(Mode::Idle);
        let mut screen = RideRecoveryScreen::new();
        handle(&mut screen, &mut activity, Gesture::Step(1));
        assert!(screen.selection_is_guarded());
        assert!(matches!(handle(&mut screen, &mut activity, Gesture::Press), Transition::None));
        assert_eq!(activity.take_track_action(), None, "a tap cannot discard recovered bytes");

        assert!(matches!(handle(&mut screen, &mut activity, Gesture::Hold), Transition::Home));
        assert_eq!(activity.take_track_action(), Some(TrackAction::Discard));
        assert!(!activity.is_tracking());
        assert_eq!(activity.mode, Mode::Idle);
    }

    #[test]
    fn back_cannot_strand_the_recovered_recording() {
        let mut activity = Activity::new(Mode::Idle);
        assert!(matches!(handle(&mut RideRecoveryScreen::new(), &mut activity, Gesture::Back), Transition::None));
    }

    #[test]
    fn damaged_recording_can_only_be_discarded() {
        let mut activity = Activity::new(Mode::Idle);
        let mut screen = RideRecoveryScreen::damaged();
        assert!(screen.selection_is_guarded());
        assert!(matches!(handle(&mut screen, &mut activity, Gesture::Press), Transition::None));
        assert!(!activity.is_tracking());
        assert_eq!(activity.take_track_action(), None);
        assert!(matches!(handle(&mut screen, &mut activity, Gesture::Hold), Transition::Home));
        assert_eq!(activity.take_track_action(), Some(TrackAction::Discard));
    }
}
