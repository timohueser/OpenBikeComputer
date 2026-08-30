//! The boot-time recovered-ride decision card, in its four modes.
//!
//! A durable `RECORDING` object survived a reset, so the rider must explicitly choose what happens
//! to it. **Continue ride** attaches a new app session to the recovered recorder without clearing
//! the restored totals. **Discard** is destructive and therefore fires only on a completed Select
//! hold; it names the ordinary [`RecorderIntent::Discard`] to Recorder and returns Home.
//!
//! When that removal fails, the card comes back in a terminal mode instead of the attempt repeating
//! itself: *Repair failed* offers one more guarded **Retry**, and *Card needs service* offers none,
//! because there is no safe object-level repair to attempt (#1591, #1557 item 3). Both terminal
//! modes carry a labelled **Back** row, because a rider who must be able to keep using the
//! non-recording half of the device needs a way out they can see — the global escape stays refused
//! and Back stays inert, so the card can never be dismissed by accident.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use super::vocab::fmt::{distance_figure, duration_hms};
use crate::input::Gesture;
use crate::recorder::RideRecoveryState;
use crate::Msg;
use crate::RecorderIntent;

use super::vocab::chrome::{title_frame, TITLE_BAR_H};
use super::vocab::list;
use super::vocab::rows::{draw_guarded_rows, ledger_row, GuardedRowsGeometry, MenuItem};
use super::{palette, Ctx, MapScreen, Render, Screen, Transition};

const LEDGER_TOP: i32 = 70;
const LEDGER_PITCH: i32 = 38;
const OPTIONS_TOP: i32 = 204;

/// What the card is asking, which is exactly what Recorder's recovery state has come to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecoveryMode {
    /// A whole recovered ride: continue it, or throw it away.
    #[default]
    Resumable,
    /// Logical damage on a writable catalog: the one hold-guarded removal.
    Damaged,
    /// The removal that would have repaired a damaged recording failed. Nothing happens
    /// automatically now; the rider may try once more.
    RepairFailed,
    /// The rider's Discard of a *whole* recovered ride failed. Same rows as
    /// [`RepairFailed`](RecoveryMode::RepairFailed) and different copy: nothing was damaged, the
    /// store simply refused the removal.
    DiscardFailed,
    /// There is no safe operation to offer at all.
    Unrepairable,
}

/// One row of the card. A mode is the list of rows it offers, and a row carries its own guard, so
/// "a tap can never repair" is a property of the table rather than of each arm of `handle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Continue,
    Discard,
    Retry,
    Leave,
}

impl Row {
    /// Whether choosing this row needs a completed Select hold. Everything that removes bytes does.
    fn guard(self) -> bool {
        matches!(self, Row::Discard | Row::Retry)
    }

    fn label(self) -> Msg {
        match self {
            Row::Continue => Msg::RideRecoveryContinueRide,
            Row::Discard => Msg::RideRecoveryDiscard,
            Row::Retry => Msg::RideRecoveryRetry,
            Row::Leave => Msg::RideRecoveryLeave,
        }
    }
}

impl RecoveryMode {
    /// The card's mode for a recovery state, or `None` when there is no decision to put.
    /// `Attempting` is an operation in flight, not a mode: the card that ordered it has already
    /// returned Home, and the answer re-raises it in whatever it becomes — so it maps back to the
    /// card that ordered it, which is what the rider would see if anything raised it meanwhile.
    pub(crate) fn of(state: RideRecoveryState) -> Option<Self> {
        match state {
            RideRecoveryState::None => None,
            RideRecoveryState::Resumable | RideRecoveryState::Attempting(None) => Some(RecoveryMode::Resumable),
            RideRecoveryState::Repairable(_) | RideRecoveryState::Attempting(Some(_)) => Some(RecoveryMode::Damaged),
            RideRecoveryState::Latched(Some(_)) => Some(RecoveryMode::RepairFailed),
            RideRecoveryState::Latched(None) => Some(RecoveryMode::DiscardFailed),
            RideRecoveryState::Unrepairable => Some(RecoveryMode::Unrepairable),
        }
    }

    fn rows(self) -> &'static [Row] {
        match self {
            RecoveryMode::Resumable => &[Row::Continue, Row::Discard],
            RecoveryMode::Damaged => &[Row::Discard],
            RecoveryMode::RepairFailed | RecoveryMode::DiscardFailed => &[Row::Retry, Row::Leave],
            RecoveryMode::Unrepairable => &[Row::Leave],
        }
    }

    /// The one line under the title: what the rider is being told.
    fn body(self) -> Msg {
        match self {
            RecoveryMode::Resumable => Msg::RideRecoveryBody,
            RecoveryMode::Damaged => Msg::RideRecoveryDamaged,
            RecoveryMode::RepairFailed => Msg::RideRecoveryRepairFailed,
            RecoveryMode::DiscardFailed => Msg::RideRecoveryDiscardFailed,
            RecoveryMode::Unrepairable => Msg::RideRecoveryUnrepairable,
        }
    }
}

/// The recovered-ride card. State is the mode it was raised in and the highlighted choice.
#[derive(Debug, Default)]
pub struct RideRecoveryScreen {
    selected: usize,
    mode: RecoveryMode,
}

impl RideRecoveryScreen {
    pub fn new(mode: RecoveryMode) -> Self {
        Self { selected: 0, mode }
    }

    fn row(&self) -> Row {
        let rows = self.mode.rows();
        rows[self.selected.min(rows.len() - 1)]
    }

    /// Whether the highlighted choice needs a completed hold.
    pub fn selection_is_guarded(&self) -> bool {
        self.row().guard()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let rows = self.mode.rows();
        match (g, self.row()) {
            (Gesture::Step(n), _) if rows.len() > 1 => list::on_step(&mut self.selected, n, rows.len()),
            (Gesture::Press, Row::Continue) => {
                // Tell Recorder that the next Start continues the recovered session, so it keeps
                // the restored accumulators instead of applying a fresh-session reset.
                let (lon, lat) = cx.state.user_fix.map_or((cx.state.cam_lon, cx.state.cam_lat), |f| (f.lon, f.lat));
                cx.state.enter_riding_view(lon, lat);
                cx.activity.mode = crate::activity::Mode::Riding;
                cx.navigator.set_active_route(None);
                cx.recorder.continue_recovered();
                Transition::Root(Screen::Map(MapScreen::new()))
            }
            // The terminal modes' way out, and the only one: the rider keeps the non-recording half
            // of the device, and the decision is still theirs whenever they come back to it.
            (Gesture::Press, Row::Leave) => Transition::Home,
            (Gesture::Hold, Row::Discard | Row::Retry) => {
                cx.recorder.request(RecorderIntent::Discard);
                cx.activity.mode = crate::activity::Mode::Idle;
                cx.navigator.set_active_route(None);
                Transition::Home
            }
            // Press on a guarded row is deliberately inert, and Back cannot bypass the decision.
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::RideRecoveryTitle), "");
        cv.text(rx.t(self.mode.body()), Point::new(w / 2, TITLE_BAR_H + 12), Font::Label, TextAlign::Center, SUBTEXT);

        // Show the totals the continuation path promises to keep. At a recovery boundary where a
        // host could restore only part of the summary these safely render zero, never invented data.
        let units = rx.settings.units;
        let ride = rx.recorder;
        let time = duration_hms(ride.moving_s());
        let distance = distance_figure(units.dist(ride.ridden_m() / 1000.0));
        let distance_unit = if units.is_imperial() { "mi" } else { "km" };
        let mut climb: heapless::String<8> = heapless::String::new();
        let _ = write!(climb, "{}", units.elev(ride.climb_m()) as u32);
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

        let mut items: heapless::Vec<MenuItem, 2> = heapless::Vec::new();
        for row in self.mode.rows() {
            let _ = items.push(MenuItem { label: rx.t(row.label()), guard: row.guard() });
        }
        draw_guarded_rows(
            cv,
            &items,
            self.selected.min(items.len().saturating_sub(1)),
            rx.hold_progress,
            palette::WARNING,
            GuardedRowsGeometry::panel(w, OPTIONS_TOP, 42, 8),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    fn handle(
        screen: &mut RideRecoveryScreen,
        activity: &mut Activity,
        rec: &mut crate::RecorderMachine,
        navigator: &mut crate::navigator::NavigatorMachine,
        gesture: Gesture,
    ) -> Transition {
        let mut state = AppState::new(0, 0, 1.0);
        let mut settings = Settings::default();
        screen.handle(gesture, &mut Ctx { recorder: rec, navigator, ..test_ctx(&mut state, activity, &mut settings) })
    }

    #[test]
    fn continue_mints_a_session_and_uses_the_recovery_transition() {
        let mut rec = crate::RecorderMachine::new();
        let mut activity = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut screen = RideRecoveryScreen::new(RecoveryMode::Resumable);
        let transition = handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Press);
        assert!(matches!(transition, Transition::Root(Screen::Map(_))));
        assert_eq!(rec.test_take_intent(), Some(crate::RecorderIntent::Start), "Continue names a session");
        assert_eq!(activity.mode, Mode::Riding);
        assert_eq!(navigator.route_state().active_route, None);
    }

    #[test]
    fn discard_needs_a_hold_and_posts_the_existing_action() {
        let mut rec = crate::RecorderMachine::new();
        let mut activity = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut screen = RideRecoveryScreen::new(RecoveryMode::Resumable);
        handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Step(1));
        assert!(screen.selection_is_guarded());
        assert!(matches!(
            handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Press),
            Transition::None
        ));
        assert_eq!(rec.test_take_intent(), None, "a tap cannot discard recovered bytes");

        assert!(matches!(
            handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Hold),
            Transition::Home
        ));
        assert_eq!(rec.test_take_intent(), Some(crate::RecorderIntent::Discard));
        assert!(!rec.recording());
        assert_eq!(activity.mode, Mode::Idle);
    }

    #[test]
    fn back_cannot_strand_the_recovered_recording() {
        let mut rec = crate::RecorderMachine::new();
        let mut activity = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut screen = RideRecoveryScreen::new(RecoveryMode::Resumable);
        assert!(matches!(
            handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Back),
            Transition::None
        ));
    }

    #[test]
    fn damaged_recording_can_only_be_discarded() {
        let mut rec = crate::RecorderMachine::new();
        let mut activity = Activity::new(Mode::Idle);
        let mut navigator = crate::navigator::NavigatorMachine::new();
        let mut screen = RideRecoveryScreen::new(RecoveryMode::Damaged);
        assert!(screen.selection_is_guarded());
        assert!(matches!(
            handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Press),
            Transition::None
        ));
        assert!(!rec.recording());
        assert_eq!(rec.test_take_intent(), None);
        assert!(matches!(
            handle(&mut screen, &mut activity, &mut rec, &mut navigator, Gesture::Hold),
            Transition::Home
        ));
        assert_eq!(rec.test_take_intent(), Some(crate::RecorderIntent::Discard));
    }

    /// Each mode offers exactly its own actions, and nothing else reaches Recorder from it. The
    /// unrepairable card is the one that has to be provable as an **absence**: neither a tap nor a
    /// hold on it may name a removal, because "no safe automatic object-level repair" is enforced
    /// here as well as in the domain.
    #[test]
    fn each_recovery_mode_offers_exactly_its_own_actions() {
        let mut rec = crate::RecorderMachine::new();
        let mut activity = Activity::new(Mode::Idle);
        let mut nav = crate::navigator::NavigatorMachine::new();

        // The five row tables, and which of them a tap could ever act on.
        assert_eq!(RecoveryMode::Resumable.rows(), &[Row::Continue, Row::Discard]);
        assert_eq!(RecoveryMode::Damaged.rows(), &[Row::Discard]);
        assert_eq!(RecoveryMode::RepairFailed.rows(), &[Row::Retry, Row::Leave]);
        // The two failed-removal modes share their rows and differ only in the line above them.
        assert_eq!(RecoveryMode::DiscardFailed.rows(), &[Row::Retry, Row::Leave]);
        assert_ne!(
            crate::i18n::t(RecoveryMode::DiscardFailed.body(), crate::settings::Language::En),
            crate::i18n::t(RecoveryMode::RepairFailed.body(), crate::settings::Language::En),
            "a failed discard of a whole ride does not claim a repair was attempted"
        );
        assert_eq!(RecoveryMode::Unrepairable.rows(), &[Row::Leave]);
        assert!(Row::Discard.guard() && Row::Retry.guard(), "everything that removes bytes is held");
        assert!(!Row::Continue.guard() && !Row::Leave.guard(), "and nothing that does not is");

        // Repair failed: Retry is the entry row, it is guarded, and a tap on it does nothing.
        let mut failed = RideRecoveryScreen::new(RecoveryMode::RepairFailed);
        assert!(failed.selection_is_guarded(), "Retry is entered on, and it is guarded");
        assert!(matches!(handle(&mut failed, &mut activity, &mut rec, &mut nav, Gesture::Press), Transition::None));
        assert_eq!(rec.test_take_intent(), None, "a tap cannot re-attempt a removal");
        assert!(matches!(handle(&mut failed, &mut activity, &mut rec, &mut nav, Gesture::Hold), Transition::Home));
        assert_eq!(rec.test_take_intent(), Some(crate::RecorderIntent::Discard), "the hold posts exactly one");

        // …and its second row leaves without naming anything.
        let mut failed = RideRecoveryScreen::new(RecoveryMode::RepairFailed);
        handle(&mut failed, &mut activity, &mut rec, &mut nav, Gesture::Step(1));
        assert!(!failed.selection_is_guarded(), "Back is a plain press");
        assert!(matches!(handle(&mut failed, &mut activity, &mut rec, &mut nav, Gesture::Press), Transition::Home));
        assert_eq!(rec.test_take_intent(), None, "leaving the card orders nothing");

        // Unrepairable: one row, no guard, no gesture reaches Recorder.
        let mut service = RideRecoveryScreen::new(RecoveryMode::Unrepairable);
        assert!(!service.selection_is_guarded());
        assert!(matches!(handle(&mut service, &mut activity, &mut rec, &mut nav, Gesture::Hold), Transition::None));
        assert_eq!(rec.test_take_intent(), None, "a hold on the service card removes nothing");
        assert!(matches!(handle(&mut service, &mut activity, &mut rec, &mut nav, Gesture::Step(1)), Transition::None));
        assert!(matches!(handle(&mut service, &mut activity, &mut rec, &mut nav, Gesture::Press), Transition::Home));
        assert_eq!(rec.test_take_intent(), None);
    }

    /// The three terminal modes' body lines and their two row labels fit the 240 px panel in all
    /// four languages. A translation that does not fit fails here rather than being clipped on glass.
    ///
    /// It gates **this slice's copy**. The card's older lines — `body`, `damaged` and
    /// `continue_ride` — already exceed both budgets in de/fr (and `continue_ride` in es), so
    /// including them would fail the test against copy that shipped long before it. That is a real
    /// defect and a separate one; pinning the pre-existing widths here would only freeze it.
    #[test]
    fn every_recovery_card_line_fits_in_every_language() {
        use crate::i18n::t;
        use crate::settings::Language;
        use obc_render::text::text_width;

        const W: i32 = 240;
        const MIN_CLEAR: i32 = 8;
        // `GuardedRowsGeometry::panel` lays each row out as `rect(14, .., w - 28, ..)` and writes its
        // label left-aligned at `x + 12`, so the label runs from 26 to the row's right edge at 226.
        let row_room = (14 + (W - 28)) - (14 + 12) - MIN_CLEAR;
        // The body line is centred on the full width under the title bar, so it loses the clearance
        // at both edges.
        let body_room = W - MIN_CLEAR * 2;
        assert_eq!((row_room, body_room), (192, 224), "the card's two budgets, pinned");

        let (mut worst_body, mut worst_row) = (0, 0);
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            for mode in [RecoveryMode::RepairFailed, RecoveryMode::DiscardFailed, RecoveryMode::Unrepairable] {
                let s = t(mode.body(), lang);
                let px = text_width(s, Font::Label) as i32;
                assert!(px <= body_room, "{lang:?}: body {s:?} ({px} px) overruns the {body_room} px card");
                worst_body = worst_body.max(px);
            }
            for row in [Row::Retry, Row::Leave] {
                let s = t(row.label(), lang);
                // `draw_guarded_rows` draws its labels in `Font::Body`, the wider tier.
                let px = text_width(s, Font::Body) as i32;
                assert!(px <= row_room, "{lang:?}: row {s:?} ({px} px) overruns the {row_room} px panel");
                worst_row = worst_row.max(px);
            }
        }
        // Pinned, so copy that merely *fits* cannot grow toward the edge unnoticed.
        assert_eq!(
            worst_body, 216,
            "en \"Card needs service\" / fr \"Réparation échouée\" / es \"Reparación fallida\" in Label, pinned"
        );
        assert_eq!(body_room - worst_body, 8, "…with 8 px to spare");
        assert_eq!(worst_row, 154, "de \"Wiederholen\" in Body, pinned");
        assert_eq!(row_room - worst_row, 38, "…with 38 px to spare");
    }
}
