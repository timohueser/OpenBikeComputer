//! The Factory Reset screen. The long-press threshold is about 500 ms, which is too short to feel
//! safe alone, so a reset takes two steps: a press to arm, then a hold to erase. A hold on an
//! un-armed screen does nothing. The reset clears the settings, but keeps the files on the card.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{card_check, card_triangle, copy_w, title_frame, wrapped, TITLE_BAR_H};
use crate::screen::{palette, Ctx, Render, Transition};
use crate::settings::Settings;
use crate::Msg;

/// `armed` is set by the first press, and only an armed screen can erase. `done` is set when the
/// reset is applied.
#[derive(Debug, Default)]
pub struct ResetScreen {
    armed: bool,
    done: bool,
}

impl ResetScreen {
    pub fn new() -> Self {
        ResetScreen { armed: false, done: false }
    }

    /// True while the hold-to-erase bar is on screen and fills with the live hold progress.
    pub(crate) fn hold_fill_active(&self) -> bool {
        self.armed && !self.done
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.done {
            // Any key starts setup again. The device would reboot here.
            return match g {
                Gesture::Press | Gesture::Back => crate::screen::setup::go_to(cx.settings),
                _ => Transition::None,
            };
        }
        match g {
            Gesture::Press if !self.armed => {
                self.armed = true;
                Transition::None
            }
            // `apply_gesture` sees the change and flags the host to persist the cleared settings.
            Gesture::Hold if self.armed => {
                *cx.settings = Settings::FACTORY;
                self.done = true;
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

        if self.done {
            card_check(cv, Point::new(w / 2, TITLE_BAR_H + 64), 26);
            let y = wrapped(cv, rx.t(Msg::ResetComplete), w / 2, TITLE_BAR_H + 110, copy_w(w), Font::Body, INK);
            wrapped(cv, rx.t(Msg::ResetRestarting), w / 2, y + 9, copy_w(w), Font::Label, SUBTEXT);
            return;
        }

        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 50), 24);
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
    use crate::{AppState, Mode, Units};

    fn run(scr: &mut ResetScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn arm_then_hold_resets_to_factory_and_starts_setup() {
        let mut s = Settings { units: Units::Imperial, power_saver: true, fix_interval_s: 30, ..Settings::default() };
        let before = s;
        let mut scr = ResetScreen::new();

        run(&mut scr, &mut s, Gesture::Hold);
        assert!(!scr.done, "an un-armed hold does nothing");
        assert_eq!(s, before, "and changes no settings");

        run(&mut scr, &mut s, Gesture::Press);
        assert!(scr.armed && !scr.done);
        let t = run(&mut scr, &mut s, Gesture::Hold);
        assert!(matches!(t, Transition::None), "stays to show the done message");
        assert_eq!(s, Settings::FACTORY, "settings were cleared to factory defaults");
        assert!(scr.done);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Root(crate::Screen::Hello(_))));
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
