//! The Display screen — the Map's chrome overlays plus the idle-return timeout. `Clock`,
//! `Scale bar` and `Contours` are click-to-flip toggles that show/hide the Map's `HH:MM` pill, the
//! bottom-left scale bar, and the map's terrain layer. `Idle` is a left/right value picker
//! (15 s / 30 s / 1 min / 5 min / Never) — the same
//! [`IdleReturn`](crate::settings::IdleReturn) picker that used to live on the Power page, moved here
//! so all the "how the screen behaves" settings sit together.
//!
//! The `Contours` row is **provisional** (elevation EL10c, #1096): it exists so the #1097 ride review
//! can A/B contours on the same ride, and is expected to be removed with that review's verdict —
//! along with [`map_contours`](crate::Settings::map_contours) and its four catalog strings.

use obc_render::{rect, text::Font, Surface};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::rows::{row_cursor, row_rect};
use crate::screen::{Ctx, Render, Transition};
use crate::Msg;

/// Row height — fits a two-line label (Body + sub-caption) plus a toggle / value cell with arrow room.
const ROW_H: i32 = 58;

/// The rows: the three Map-overlay toggles, then the idle-return picker.
const CLOCK: usize = 0;
const SCALE_BAR: usize = 1;
const CONTOURS: usize = 2;
const IDLE_RETURN: usize = 3;
const ROWS: usize = 4;

/// The Display screen. `selected` is the highlighted row; `editing` is set only while the
/// idle-return picker is open (the toggles have no edit sub-mode).
#[derive(Debug, Default)]
pub struct DisplayScreen {
    selected: usize,
    editing: bool,
}

impl DisplayScreen {
    pub fn new() -> Self {
        DisplayScreen::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                if self.editing {
                    // Only the idle-return row has an editable value; a turn walks it in place.
                    if self.selected == IDLE_RETURN {
                        cx.settings.idle_return = cx.settings.idle_return.stepped(n);
                    }
                } else {
                    self.selected = crate::screen::vocab::list::step_selection(self.selected, n, ROWS);
                }
                Transition::None
            }
            Gesture::Press => {
                match self.selected {
                    CLOCK => cx.settings.map_clock = !cx.settings.map_clock,
                    SCALE_BAR => cx.settings.map_scale_bar = !cx.settings.map_scale_bar,
                    // #1096, provisional: the Map re-reads this every frame, so the next map frame
                    // already draws (or drops) the terrain layer — no reboot, no reload.
                    CONTOURS => cx.settings.map_contours = !cx.settings.map_contours,
                    // The value row: press enters the picker, press again (there's one field) steps
                    // back out — so press just toggles editing.
                    IDLE_RETURN => self.editing = !self.editing,
                    _ => {}
                }
                Transition::None
            }
            // Back steps out of an open field first, else climbs to the Settings list.
            Gesture::Back => super::back_out_of_field(self.editing, || self.editing = false),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::DisplayTitle), "");

        // Row 0 — Clock (toggle). Label + sub kept as short as the GPS-fix row's so they clear the
        // right-hand toggle slider.
        let r0 = row_rect(LIST_TOP + 8, w, ROW_H);
        row_cursor(cv, r0, self.selected == CLOCK, false);
        super::row_label(cv, r0, rx.t(Msg::DisplayClock), Some(rx.t(Msg::DisplayClockSub)));
        super::toggle_slider(cv, r0, rx.settings.map_clock);

        // Row 1 — Scale bar (toggle).
        let r1 = row_rect(LIST_TOP + 8 + ROW_H + 6, w, ROW_H);
        row_cursor(cv, r1, self.selected == SCALE_BAR, false);
        super::row_label(cv, r1, rx.t(Msg::DisplayScaleBar), Some(rx.t(Msg::DisplayScaleBarSub)));
        super::toggle_slider(cv, r1, rx.settings.map_scale_bar);

        // Row 2 — Contours (toggle, #1096, provisional). Same shape as the two above; the fr/es
        // catalogs split "courbes de niveau" / "curvas de nivel" over the label + sub lines so the
        // term clears the slider.
        let r2 = row_rect(LIST_TOP + 8 + 2 * (ROW_H + 6), w, ROW_H);
        row_cursor(cv, r2, self.selected == CONTOURS, false);
        super::row_label(cv, r2, rx.t(Msg::DisplayContours), Some(rx.t(Msg::DisplayContoursSub)));
        super::toggle_slider(cv, r2, rx.settings.map_contours);

        // Row 3 — Idle return (value picker: 15 s / 30 s / 1 min / 5 min / Never).
        let r3 = row_rect(LIST_TOP + 8 + 3 * (ROW_H + 6), w, ROW_H);
        let editing = self.editing && self.selected == IDLE_RETURN;
        row_cursor(cv, r3, self.selected == IDLE_RETURN, editing);
        super::row_label(cv, r3, rx.t(Msg::DisplayIdle), Some(rx.t(Msg::DisplayIdleSub)));
        let val = rx.settings.idle_return.name(rx.settings.language);
        let (cw, ch) = (76, 32);
        let cell = rect(r3.top_left.x + r3.size.width as i32 - cw - 6, r3.top_left.y + (ROW_H - ch) / 2, cw, ch);
        super::stepper_field(cv, cell, val, editing, Font::Label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::settings::IdleReturn;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut DisplayScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    /// The three overlay toggles are click-to-flip; the row cursor walks all four rows.
    #[test]
    fn clock_and_scale_bar_toggle() {
        let mut s = Settings::default();
        let mut scr = DisplayScreen::new();
        assert!(s.map_clock && s.map_scale_bar && s.map_contours, "all three default on");
        run(&mut scr, &mut s, Gesture::Press); // flip Clock
        assert!(!s.map_clock, "press flips the clock toggle");
        run(&mut scr, &mut s, Gesture::Step(1)); // → Scale bar row
        assert_eq!(scr.selected, SCALE_BAR);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!s.map_scale_bar, "press flips the scale-bar toggle");
        run(&mut scr, &mut s, Gesture::Step(1)); // → Contours row
        assert_eq!(scr.selected, CONTOURS);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!s.map_contours, "press flips the contour toggle");
    }

    /// The provisional contour toggle (#1096) flips **both ways** from the same row — the A/B #1097
    /// needs — and has no edit sub-mode, exactly like the other two toggles.
    #[test]
    fn contours_toggle_flips_both_ways() {
        let mut s = Settings::default();
        let mut scr = DisplayScreen::new();
        run(&mut scr, &mut s, Gesture::Step(2)); // Clock → Scale bar → Contours
        assert_eq!(scr.selected, CONTOURS);
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!s.map_contours, "on → off");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(s.map_contours, "off → on again, no reboot in between");
        assert!(!scr.editing, "a toggle row never opens a picker");
        // Back pops straight out: nothing to close first.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }

    /// Every toggle row's label + sub-caption must **clear the slider** in all four languages — the
    /// standing render-and-eyeball rule, pinned. The slider is 50 px wide with a 4 px right margin,
    /// so it starts `row_w - 54` into the row; the text starts 10 px in. (This caught German
    /// "Höhenlinien" — 11 Body glyphs, 154 px — running under the slider, which is why de reads
    /// "Konturen".)
    #[test]
    fn every_toggle_label_clears_the_slider_in_every_language() {
        use crate::i18n::t;
        use crate::settings::Language;
        use obc_render::text::text_width;

        // `row_rect(_, 240, _)` is `ROW_X..(240 - ROW_X)`; the slider's left edge inside it:
        let row_w = 240 - 2 * crate::screen::vocab::rows::ROW_X;
        let limit = row_w - 50 - 4 - 10;
        for lang in [Language::En, Language::De, Language::Fr, Language::Es] {
            for (row, label, sub) in [
                ("clock", Msg::DisplayClock, Msg::DisplayClockSub),
                ("scale bar", Msg::DisplayScaleBar, Msg::DisplayScaleBarSub),
                ("contours", Msg::DisplayContours, Msg::DisplayContoursSub),
            ] {
                for (msg, font) in [(label, Font::Body), (sub, Font::Label)] {
                    let s = t(msg, lang);
                    let w = text_width(s, font) as i32;
                    assert!(w <= limit, "{lang:?} {row} (\"{s}\") is {w} px, past the {limit} px label column");
                }
            }
        }
    }

    /// The idle-return row moved here verbatim: press opens its picker, a turn walks the values in
    /// place, and Back closes an open picker before it pops the screen.
    #[test]
    fn idle_return_picker() {
        let mut s = Settings { idle_return: IdleReturn::S30, ..Settings::default() };
        let mut scr = DisplayScreen::new();
        run(&mut scr, &mut s, Gesture::Step(3)); // Clock → Scale bar → Contours → Idle
        assert_eq!(scr.selected, IDLE_RETURN);
        run(&mut scr, &mut s, Gesture::Press); // open the picker
        assert!(scr.editing);
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(s.idle_return, IdleReturn::M1, "a step walks 30 s → 1 min");
        run(&mut scr, &mut s, Gesture::Step(-1));
        assert_eq!(s.idle_return, IdleReturn::S30, "and back");
        // Back closes the open picker first (no pop), then a second Back pops the screen.
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.editing);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }
}
