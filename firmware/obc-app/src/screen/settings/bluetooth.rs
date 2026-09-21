//! Bluetooth settings and the guarded phone-key removal action. The host takes the request in the
//! next pass. A pending removal disables the action, a failure permits a retry, and unconfirmed
//! controller cleanup asks for a restart instead of reporting completion.

use embedded_graphics::prelude::Point;
use obc_render::{
    text::{Font, TextAlign},
    Surface,
};

use crate::ble::BleLink;
use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::rows::{row_cursor, row_rect, toggle_slider, ROW_X};
use crate::screen::{palette, Ctx, Render, Transition};
use crate::settings::{Language, Settings};
use crate::Msg;

/// Toggle-row height. It matches the two-line rows of the other settings screens.
const ROW_H: i32 = 58;
/// The two selectable rows. The lines between them are read-only.
const TOGGLE: usize = 0;
const FORGET: usize = 1;

/// How many rows the cursor can reach. Without a stored bond the Forget row is not drawn.
fn rows(paired: bool) -> usize {
    if paired {
        2
    } else {
        1
    }
}

#[derive(Debug, Default)]
pub struct BluetoothScreen {
    selected: usize,
}

impl BluetoothScreen {
    pub fn new() -> Self {
        BluetoothScreen { selected: 0 }
    }

    /// True while the Forget row is selected and a bond is stored, so its hold fill draws.
    pub(crate) fn selection_is_guarded(&self, paired: bool) -> bool {
        self.selected == FORGET && paired
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                // A completed forget removes a row, so clamp first. A step never walks a hidden row.
                let len = rows(cx.state.bond_status.can_forget(cx.state.device.ble_paired));
                self.selected = self.selected.min(len - 1);
                self.selected = crate::screen::vocab::list::step_selection(self.selected, n, len);
                Transition::None
            }
            Gesture::Press if self.selected == TOGGLE => {
                cx.settings.ble_enabled = !cx.settings.ble_enabled;
                Transition::None
            }
            // The hold records a one-shot request. The host does the removal.
            Gesture::Hold if self.selected == FORGET && cx.state.bond_status.can_forget(cx.state.device.ble_paired) => {
                cx.state.ble_forget_requested = true;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::BluetoothTitle), "");

        // A stale cursor past the end reads as the toggle row.
        let device = rx.state.device;
        let selected = self.selected.min(rows(rx.state.bond_status.can_forget(device.ble_paired)) - 1);

        let r0 = row_rect(LIST_TOP + 8, w, ROW_H);
        row_cursor(cv, r0, selected == TOGGLE, false);
        super::row_label(cv, r0, rx.t(Msg::BluetoothRadio), Some(rx.t(Msg::BluetoothRadioSub)));
        toggle_slider(cv, r0, rx.settings.ble_enabled);

        // The read-only lines stack the caption over the value. A right-aligned value would touch
        // its caption on the 240 px panel.
        let info_x = ROW_X + 10;
        let y0 = LIST_TOP + 8 + ROW_H + 16;
        cv.text(rx.t(Msg::BluetoothStatus), Point::new(info_x, y0), Font::Label, TextAlign::Left, SUBTEXT);
        cv.text(
            status_label(rx.settings, device.ble_link, rx.settings.language),
            Point::new(info_x, y0 + 24),
            Font::Body,
            TextAlign::Left,
            INK,
        );
        let y1 = y0 + 62;
        cv.text(rx.t(Msg::BluetoothPaired), Point::new(info_x, y1), Font::Label, TextAlign::Left, SUBTEXT);
        let paired = match rx.state.bond_status {
            crate::ble::BondStatus::Pending => rx.t(Msg::BluetoothRemoving),
            crate::ble::BondStatus::Failed(_) => rx.t(Msg::BluetoothRemoveFailed),
            crate::ble::BondStatus::RestartRequired => rx.t(Msg::BluetoothRestart),
            _ if device.ble_paired => rx.t(Msg::BluetoothYes),
            _ => rx.t(Msg::BluetoothNo),
        };
        cv.text(paired, Point::new(info_x, y1 + 24), Font::Body, TextAlign::Left, INK);

        if rx.state.bond_status.can_forget(device.ble_paired) {
            super::forget_footer(cv, w, h, rx.t(Msg::BluetoothForget), selected == FORGET, rx.hold_progress);
        }
    }
}

/// The status line text. The setting decides `Off`, so the line flips with the toggle and does not
/// wait for the radio to wind down.
fn status_label(settings: &Settings, link: BleLink, lang: Language) -> &'static str {
    if !settings.ble_enabled || link == BleLink::Off {
        crate::t(Msg::BluetoothOff, lang)
    } else if link == BleLink::Connected {
        crate::t(Msg::BluetoothConnected, lang)
    } else {
        crate::t(Msg::BluetoothAdvertising, lang)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode};

    fn run(scr: &mut BluetoothScreen, st: &mut AppState, s: &mut Settings, g: Gesture) -> Transition {
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn press_flips_the_radio_switch() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = BluetoothScreen::new();
        assert!(s.ble_enabled, "on by default");
        run(&mut scr, &mut st, &mut s, Gesture::Press);
        assert!(!s.ble_enabled, "press flips it off");
        run(&mut scr, &mut st, &mut s, Gesture::Press);
        assert!(s.ble_enabled, "and back on");
    }

    #[test]
    fn forget_hold_is_guarded_and_paired_gated() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = BluetoothScreen::new();

        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, TOGGLE, "unpaired: the cursor can't leave the toggle");
        assert!(!scr.selection_is_guarded(false), "unpaired: nothing armed");
        run(&mut scr, &mut st, &mut s, Gesture::Hold);
        assert!(!st.ble_forget_requested, "unpaired: a hold does nothing (nothing to forget)");

        st.device.ble_paired = true;
        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, FORGET);
        assert!(scr.selection_is_guarded(true), "paired + selected: the hold fill is live");
        run(&mut scr, &mut st, &mut s, Gesture::Press);
        assert!(!st.ble_forget_requested, "a plain press never forgets");
        run(&mut scr, &mut st, &mut s, Gesture::Hold);
        assert!(st.ble_forget_requested, "the completed hold records the forget request");

        st.ble_forget_requested = false;
        run(&mut scr, &mut st, &mut s, Gesture::Step(-1));
        run(&mut scr, &mut st, &mut s, Gesture::Hold);
        assert!(!st.ble_forget_requested, "a hold elsewhere doesn't forget");
    }

    #[test]
    fn forget_completing_clamps_the_cursor() {
        let mut st = AppState::new(0, 0, 1.0);
        let mut s = Settings::default();
        let mut scr = BluetoothScreen::new();
        st.device.ble_paired = true;
        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, FORGET);
        st.device.ble_paired = false;
        assert!(!scr.selection_is_guarded(false), "no fill on a hidden row");
        run(&mut scr, &mut st, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, TOGGLE, "the step lands on the one remaining row");
    }

    #[test]
    fn status_line_prefers_the_switch_then_the_link() {
        let on = Settings::default();
        let off = Settings { ble_enabled: false, ..Settings::default() };
        let en = Language::En;
        assert_eq!(status_label(&off, BleLink::Connected, en), "Off", "the switch wins even mid-drop");
        assert_eq!(status_label(&on, BleLink::Off, en), "Off", "the radio's own Off reads Off too");
        assert_eq!(status_label(&on, BleLink::Advertising, en), "Advertising");
        assert_eq!(status_label(&on, BleLink::Connected, en), "Connected");
    }
}
