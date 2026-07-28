//! The Factory Reset screen — the one guarded, destructive action. The long-press threshold (~500 ms)
//! is too short to feel safe on its own, so reset is two deliberate steps: *press* to arm, then
//! *hold* to erase. A stray hold on an un-armed screen does nothing; on completion the settings drop
//! back to [`Settings::default`] and a brief "done" state shows.
//!
//! Scope: this resets the persisted settings — it does *not* delete routes or saved tracks from the
//! SD card (a deliberate follow-up).
//!
//! The hold bar is driven by [`Render::hold_progress`](crate::screen::Render), the on-screen echo of
//! the global hold-bulge overlay.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{card_check, card_triangle, palette, title_frame, Ctx, Render, Transition, TITLE_BAR_H};
use crate::settings::Settings;
use crate::Msg;

/// The Factory Reset screen. `armed` is set by the first press (you must arm before a hold can
/// erase); `done` is set once the hold completes and the reset has been applied.
#[derive(Debug, Default)]
pub struct ResetScreen {
    armed: bool,
    done: bool,
}

impl ResetScreen {
    pub fn new() -> Self {
        ResetScreen { armed: false, done: false }
    }

    /// True while the hold-to-erase bar is on screen (armed, not yet done) — it fills with the live
    /// hold progress, so [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports a
    /// charging hold as worth repainting here.
    pub(crate) fn hold_fill_active(&self) -> bool {
        self.armed && !self.done
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.done {
            // Reset applied; any key clears back to Home (the device would reboot here).
            return match g {
                Gesture::Press | Gesture::Back => Transition::Home,
                _ => Transition::None,
            };
        }
        match g {
            // Step 1: press arms the screen.
            Gesture::Press if !self.armed => {
                self.armed = true;
                Transition::None
            }
            // Step 2: once armed, a completed hold erases to defaults. The before/after diff in
            // `apply_gesture` flags the host to persist the cleared blob.
            Gesture::Hold if self.armed => {
                *cx.settings = Settings::default();
                self.done = true;
                Transition::None
            }
            // Back always exits to the Settings list — nothing erased.
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::ResetTitle), "");
        // Body content is positioned from the title bar so the armed/idle layouts stack cleanly.

        if self.done {
            card_check(cv, Point::new(w / 2, TITLE_BAR_H + 64), 26);
            cv.text(rx.t(Msg::ResetComplete), Point::new(w / 2, TITLE_BAR_H + 110), Font::Body, TextAlign::Center, INK);
            cv.text(
                rx.t(Msg::ResetRestarting),
                Point::new(w / 2, TITLE_BAR_H + 142),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
            return;
        }

        // Warning icon + title (kept short so nothing overruns the 240 px panel).
        card_triangle(cv, Point::new(w / 2, TITLE_BAR_H + 50), 24);
        cv.text(rx.t(Msg::ResetFactory), Point::new(w / 2, TITLE_BAR_H + 90), Font::Body, TextAlign::Center, WARNING);

        if !self.armed {
            // Step 1: the consequence + the arm prompt.
            cv.text(
                rx.t(Msg::ResetErases),
                Point::new(w / 2, TITLE_BAR_H + 124),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
            cv.text(
                rx.t(Msg::ResetSavedTime),
                Point::new(w / 2, TITLE_BAR_H + 144),
                Font::Label,
                TextAlign::Center,
                SUBTEXT,
            );
            // The arm action as an amber inset button.
            let label = rx.t(Msg::ResetConfirm);
            let (bw, bh) = (text_width(label, Font::Body) as i32 + 44, 42);
            let (bx, by) = (w / 2 - bw / 2, TITLE_BAR_H + 170);
            cv.round(rect(bx, by, bw, bh), 8, AMBER);
            cv.text_vcentered(label, w / 2, (by, bh), Font::Body, TextAlign::Center, INK);
            return;
        }

        // Step 2: armed → the hold-to-erase prompt over a bar that fills with the live Select hold.
        let p = rx.hold_progress.clamp(0.0, 1.0);
        let prompt = if p > 0.02 { rx.t(Msg::ResetKeepHolding) } else { rx.t(Msg::ResetHoldToErase) };
        cv.text(prompt, Point::new(w / 2, TITLE_BAR_H + 150), Font::Body, TextAlign::Center, INK);
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
    use crate::{AppState, Mode, Units};

    fn run(scr: &mut ResetScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            trips: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            waypoints: &[],
            corridor: &[],
            sensor_scan_hits: &[],
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// The two-step guard: a hold does nothing until the screen is armed by a press; press then
    /// hold erases the settings to defaults and enters the done state.
    #[test]
    fn arm_then_hold_resets_to_defaults() {
        let mut s = Settings { units: Units::Imperial, power_saver: true, fix_interval_s: 30, ..Settings::default() };
        let before = s;
        let mut scr = ResetScreen::new();

        // A hold before arming must not erase anything.
        run(&mut scr, &mut s, Gesture::Hold);
        assert!(!scr.done, "an un-armed hold does nothing");
        assert_eq!(s, before, "and changes no settings");

        run(&mut scr, &mut s, Gesture::Press); // arm
        assert!(scr.armed && !scr.done);
        let t = run(&mut scr, &mut s, Gesture::Hold); // erase
        assert!(matches!(t, Transition::None), "stays to show the done message");
        assert_eq!(s, Settings::default(), "settings were cleared to factory defaults");
        assert!(scr.done);
        // From the done state, any key clears back Home.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Home));
    }

    /// Back always exits (from the un-armed or armed state) and erases nothing.
    #[test]
    fn back_exits_without_erasing() {
        let mut s = Settings { units: Units::Imperial, ..Settings::default() };
        let before = s;
        let mut scr = ResetScreen::new();
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
        assert_eq!(s, before, "back from the prompt left the settings untouched");

        run(&mut scr, &mut s, Gesture::Press); // arm…
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "…and back still exits");
        assert_eq!(s, before, "still nothing erased");
    }
}
