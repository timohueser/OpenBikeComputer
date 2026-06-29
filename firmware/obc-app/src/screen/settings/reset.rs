//! The Factory Reset screen — the one guarded, destructive action, so it earns the long-press:
//! you **hold** to erase and a progress ring/bar fills as you hold; let go early and nothing
//! happens (a stray tap can't fire it). On completion the settings drop back to
//! [`Settings::default`] and a brief "done" state shows before any key returns Home.
//!
//! Scope note: this resets the **persisted settings** (units, clock, intervals) — it does *not*
//! delete routes or saved tracks from the SD card (that destructive filesystem wipe is a
//! deliberate follow-up). The copy says exactly what it does.
//!
//! The fill is driven by [`Render::hold_progress`](crate::screen::Render) — the same in-flight
//! encoder-hold value the [`RideControl`](crate::screen::RideControl) confirm uses. As there, the
//! global hold-bulge overlay is the always-live feedback; this bar is the on-screen echo.

use embedded_graphics::prelude::{DrawTarget, Point};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::input::Gesture;
use crate::screen::{palette, title_frame, Ctx, Render, Transition, TITLE_BAR_H};
use crate::settings::Settings;

/// The Factory Reset prompt. `done` flips once the hold completes and the reset has been applied;
/// the screen then shows its confirmation until dismissed.
#[derive(Debug, Default)]
pub struct ResetScreen {
    done: bool,
}

impl ResetScreen {
    pub fn new() -> Self {
        ResetScreen { done: false }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            // The guarded confirm: a completed hold erases the settings to defaults. The
            // before/after diff in `apply_gesture` then flags the host to persist the cleared blob.
            Gesture::Hold if !self.done => {
                *cx.settings = Settings::default();
                self.done = true;
                Transition::None
            }
            // From the done state, any key clears the whole settings stack back to Home (the
            // device would reboot here; the sim just lands home).
            Gesture::Press | Gesture::Back if self.done => Transition::Home,
            // Before completion, back is the single-tap cancel — nothing erased.
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let mut cv = Canvas::new(target, color_fn);
        title_frame(&mut cv, w, h, "RESET", "");

        if self.done {
            cv.text("Reset complete", Point::new(w / 2, h / 2 - 24), Font::Body, TextAlign::Center, INK);
            cv.text(
                "settings restored to defaults",
                Point::new(w / 2, h / 2 + 8),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
            super::back_hint(&mut cv, w, h, "press to continue");
            return RenderStats::default();
        }

        let body_top = TITLE_BAR_H + 24;
        cv.text("Factory reset", Point::new(w / 2, body_top), Font::Body, TextAlign::Center, WARNING);
        cv.text("Resets all settings", Point::new(w / 2, body_top + 34), Font::Label, TextAlign::Center, SUBTEXT);
        cv.text("to factory defaults.", Point::new(w / 2, body_top + 54), Font::Label, TextAlign::Center, SUBTEXT);

        // Hold-to-erase progress bar: a parchment-shade track filling warning-red with the live
        // encoder hold. Empty at rest; full at the hold threshold (which fires `Gesture::Hold`).
        let p = rx.hold_progress.clamp(0.0, 1.0);
        let (bx, bw, by, bh) = (40, w - 80, h / 2 + 6, 16);
        let radius = (bh / 2) as u32;
        cv.round(rect(bx, by, bw, bh), radius, PARCHMENT_SHADE);
        let fill = (bw as f32 * p) as i32;
        if fill > 0 {
            cv.round(rect(bx, by, fill, bh), radius, WARNING);
        }

        let prompt = if p > 0.02 { "Keep holding" } else { "Hold to reset" };
        cv.text(prompt, Point::new(w / 2, by + bh + 14), Font::Body, TextAlign::Center, INK);
        super::back_hint(&mut cv, w, h, "tap back to cancel");
        RenderStats::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Units};

    fn run(scr: &mut ResetScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = Ctx { state: &mut st, activity: &mut act, settings: s, routes: &[], now_ms: 0 };
        scr.handle(g, &mut cx)
    }

    /// A completed hold erases the settings to defaults and enters the done state (it does not
    /// leave the screen yet — the confirmation shows until dismissed).
    #[test]
    fn hold_resets_to_defaults() {
        let mut s = Settings { units: Units::Imperial, power_saver: true, fix_interval_s: 30, ..Settings::default() };
        let mut scr = ResetScreen::new();
        let t = run(&mut scr, &mut s, Gesture::Hold);
        assert!(matches!(t, Transition::None), "stays to show the done message");
        assert_eq!(s, Settings::default(), "settings were cleared to factory defaults");
        assert!(scr.done);
        // From the done state, any key clears back Home.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Home));
    }

    /// Back before the hold completes is the single-tap cancel — nothing is erased.
    #[test]
    fn back_cancels_without_erasing() {
        let mut s = Settings { units: Units::Imperial, ..Settings::default() };
        let before = s;
        let mut scr = ResetScreen::new();
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
        assert!(!scr.done);
        assert_eq!(s, before, "cancel left the settings untouched");
    }
}
