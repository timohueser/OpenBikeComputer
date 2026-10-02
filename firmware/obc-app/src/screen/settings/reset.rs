//! The Factory Reset screen. The long-press threshold is about 500 ms, which is too short to feel
//! safe alone, so a reset takes two steps: a press to arm, then a hold to erase. A hold on an
//! un-armed screen does nothing. A paired device waits for bond removal before it clears the
//! settings. Unconfirmed controller clearance asks for a restart before first-use setup. The
//! reset keeps maps and deletes personal card data before it clears settings.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::ble::BondStatus;
use crate::input::Gesture;
use crate::screen::vocab::chrome::{card_triangle, copy_w, title_frame, wrapped, TITLE_BAR_H};
use crate::screen::{palette, Ctx, QuickDrawerScreen, Render, Screen, Transition};
use crate::Msg;

#[derive(Debug, Default, PartialEq, Eq)]
enum DataReset {
    #[default]
    Waiting,
    Requested,
    Running,
    Failed,
    Done,
    Committed,
}

/// `armed` is set by the first press, and only an armed screen can erase.
#[derive(Debug, Default)]
pub struct ResetScreen {
    armed: bool,
    removing: bool,
    data: DataReset,
}

impl ResetScreen {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while the hold-to-erase bar is on screen and fills with the live hold progress.
    pub(crate) fn hold_fill_active(&self) -> bool {
        self.armed && !self.removing
    }

    pub(crate) fn removing(&self) -> bool {
        self.removing
    }

    pub(crate) fn request_data_clear(&mut self) {
        if self.removing && self.data == DataReset::Waiting {
            self.data = DataReset::Requested;
        }
    }

    pub(crate) fn data_clear_requested(&self) -> bool {
        self.removing && self.data == DataReset::Requested
    }

    pub(crate) fn data_clear_started(&mut self) {
        self.data = DataReset::Running;
    }

    pub(crate) fn data_clear_finished(&mut self, success: bool) {
        self.data = if success { DataReset::Done } else { DataReset::Failed };
    }

    pub(crate) fn data_cleared(&self) -> bool {
        self.data == DataReset::Done
    }

    pub(crate) fn committed(&mut self) {
        self.data = DataReset::Committed;
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.removing {
            if self.data == DataReset::Failed && g == Gesture::Press {
                self.data = DataReset::Requested;
                return Transition::None;
            }
            if matches!(self.data, DataReset::Requested | DataReset::Running | DataReset::Failed) {
                return Transition::None;
            }
            return match (g, cx.state.bond_status) {
                (Gesture::Press, BondStatus::Failed(_)) => {
                    cx.state.ble_forget_requested = true;
                    Transition::None
                }
                (Gesture::Back, BondStatus::Failed(_)) => Transition::Pop,
                (Gesture::Press, BondStatus::RestartRequired) => {
                    Transition::Push(Screen::QuickDrawer(QuickDrawerScreen::power_confirmation()))
                }
                _ => Transition::None,
            };
        }
        match g {
            Gesture::Press if !self.armed => {
                self.armed = true;
                Transition::None
            }
            Gesture::Hold if self.armed && cx.state.bond_status == BondStatus::RestartRequired => {
                self.removing = true;
                self.data = DataReset::Requested;
                Transition::None
            }
            // Keep the settings until bond removal succeeds. A failed clear must not boot setup
            // with the old phone still paired. The App commits the reset on its removal receipt.
            Gesture::Hold
                if self.armed
                    && cx.state.bonding
                    && (cx.state.device.ble_paired
                        || matches!(cx.state.bond_status, BondStatus::Pending | BondStatus::Failed(_))) =>
            {
                self.removing = true;
                cx.state.ble_forget_requested = true;
                Transition::None
            }
            Gesture::Hold if self.armed => {
                self.removing = true;
                self.data = DataReset::Requested;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::ResetTitle), "");

        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 50), 24);
        if self.removing {
            let (message, action) = if self.data == DataReset::Failed {
                (Msg::ResetDataFailed, Some(Msg::ResetRetry))
            } else if matches!(self.data, DataReset::Requested | DataReset::Running) {
                (Msg::ResetErasing, None)
            } else {
                match rx.state.bond_status {
                    BondStatus::Failed(_) => (Msg::ResetPhoneFailed, Some(Msg::ResetRetry)),
                    BondStatus::RestartRequired => (Msg::BluetoothRestart, Some(Msg::QuickPower)),
                    _ => (Msg::BluetoothRemoving, None),
                }
            };
            let y = wrapped(cv, rx.t(message), w / 2, TITLE_BAR_H + 96, copy_w(w), Font::Body, INK);
            if rx.state.bond_status == BondStatus::RestartRequired {
                wrapped(cv, rx.t(Msg::ResetPowerCycle), w / 2, y + 12, copy_w(w), Font::Label, SUBTEXT);
            }
            if let Some(action) = action {
                let area = super::super::vocab::rows::row_rect(h - 58, w, 42);
                super::super::vocab::rows::action_row(cv, area, rx.t(action), None, true, true, false, 0.0);
            }
            return;
        }
        wrapped(cv, rx.t(Msg::ResetFactory), w / 2, TITLE_BAR_H + 90, copy_w(w), Font::Body, WARNING);

        if !self.armed {
            // The warning is one sentence over two catalog lines, and the button follows the last
            // of them: each stacks on the y the wrap reports, so a longer translation pushes down
            // instead of drawing over its neighbour.
            let y = wrapped(cv, rx.t(Msg::ResetErases), w / 2, TITLE_BAR_H + 124, copy_w(w), Font::Label, SUBTEXT);
            let y = wrapped(cv, rx.t(Msg::ResetSavedTime), w / 2, y, copy_w(w), Font::Label, SUBTEXT);
            let label = rx.t(Msg::ResetConfirm);
            let (bw, bh) = (text_width(label, Font::Body) as i32 + 44, 42);
            let (bx, by) = (w / 2 - bw / 2, y + 8);
            cv.round(rect(bx, by, bw, bh), 8, AMBER);
            cv.text_vcentered(label, w / 2, (by, bh), Font::Body, TextAlign::Center, ON_ACCENT);
            return;
        }

        let p = rx.hold_progress.clamp(0.0, 1.0);
        let prompt = if p > 0.02 { rx.t(Msg::ResetKeepHolding) } else { rx.t(Msg::ResetHoldToErase) };
        wrapped(cv, prompt, w / 2, TITLE_BAR_H + 150, copy_w(w), Font::Body, INK);
        let (bx, bw, by, bh) = (40, w - 80, TITLE_BAR_H + 184, 16);
        let radius = (bh / 2) as u32;
        cv.round(rect(bx, by, bw, bh), radius, PARCHMENT_SHADE);
        let fill = (bw as f32 * p) as i32;
        if fill > 0 {
            cv.round(rect(bx, by, fill, bh), radius, WARNING);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::settings::Settings;
    use crate::{AppState, Mode, Units};

    fn run(scr: &mut ResetScreen, s: &mut Settings, g: Gesture) -> Transition {
        run_in(scr, s, &mut AppState::new(0, 0, 1.0), g)
    }

    fn run_in(scr: &mut ResetScreen, s: &mut Settings, st: &mut AppState, g: Gesture) -> Transition {
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn arm_then_hold_waits_for_personal_data_clear_on_an_unpaired_device() {
        let mut s = Settings { units: Units::Imperial, power_saver: true, fix_interval_s: 30, ..Settings::default() };
        let before = s;
        let mut st = AppState::new(0, 0, 1.0);
        let mut scr = ResetScreen::new();

        let t = run_in(&mut scr, &mut s, &mut st, Gesture::Hold);
        assert!(matches!(t, Transition::None), "an un-armed hold does nothing");
        assert_eq!((s, st.ble_forget_requested), (before, false), "and changes no settings");

        run_in(&mut scr, &mut s, &mut st, Gesture::Press);
        assert!(scr.armed);
        let t = run_in(&mut scr, &mut s, &mut st, Gesture::Hold);
        assert!(matches!(t, Transition::None));
        assert_eq!(s, before, "settings wait for confirmed deletion");
        assert!(scr.data_clear_requested());
        assert!(!st.ble_forget_requested, "there is no phone to forget");
    }

    #[test]
    fn back_exits_without_erasing() {
        let mut s = Settings { units: Units::Imperial, ..Settings::default() };
        let before = s;
        let mut scr = ResetScreen::new();
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
        assert_eq!(s, before, "back from the prompt left the settings untouched");

        run(&mut scr, &mut s, Gesture::Press);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "…and back still exits");
        assert_eq!(s, before, "still nothing erased");
    }
}
