//! First-use setup. A new device, and one after a factory reset, boots into it instead of Home.
//! The step is persisted in [`Settings::setup`](crate::Settings), so setup resumes at its step
//! after a power loss. Each step is a screen: [`screen`] maps a step to it. Select ends the step
//! through [`finish`], and Back returns to the step before through [`back`]. A new step is a
//! [`SetupStep`] variant, its place in `SetupStep::ORDER`, its screen, and its arm in [`screen`].
//!
//! The first step is Hello: a greeting in the four UI languages, because the language is not chosen
//! yet. Every step after it is a titled page: [`title_bar`] at its head and the [`hint`] at its
//! foot.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::i18n::t;
use crate::input::Gesture;
use crate::settings::{Language, Settings, SetupStep};
use crate::Msg;

use super::home::contours;
use super::settings::LanguageScreen;
use super::vocab::chrome::{copy_w, title_frame, wrapped};
use super::vocab::rows::ROW_X;
use super::{palette, Ctx, Render, Screen, Transition};

/// The screen of the current setup step, or `None` once setup is done.
pub(crate) fn screen(s: &Settings) -> Option<Screen> {
    match s.setup {
        SetupStep::Hello => Some(Screen::Hello(HelloScreen)),
        SetupStep::Language => Some(Screen::SetupLanguage(SetupLanguageScreen::new(s.language))),
        SetupStep::Buttons => Some(Screen::SetupButtons(SetupButtonsScreen::default())),
        SetupStep::Done => None,
    }
}

/// Go to the current setup step: its screen over Home, or Home once setup is done.
pub(crate) fn go_to(s: &Settings) -> Transition {
    screen(s).map_or(Transition::Home, Transition::Root)
}

/// End setup step `step`: persist the next step and go to it.
fn finish(step: SetupStep, cx: &mut Ctx) -> Transition {
    cx.settings.setup = step.next();
    go_to(cx.settings)
}

/// Leave setup step `step` for the one before it. The persisted step follows, so a power loss
/// resumes on the step the rider sees.
fn back(step: SetupStep, cx: &mut Ctx) -> Transition {
    cx.settings.setup = step.prev();
    go_to(cx.settings)
}

/// The greeting, two languages a line. The language is not chosen yet, so it is not a catalog
/// string.
const GREETING: [(&str, &str); 2] = [("Hello", "Hallo"), ("Bonjour", "Hola")];

/// The contour backdrop's seed: Home's massif, drifted so that the signpost stands on its summit.
const BACKDROP_SEED: u32 = 20;

#[derive(Debug)]
pub struct HelloScreen;

impl HelloScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press => finish(SetupStep::Hello, cx),
            _ => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        cv.clear(HUD);
        contours(cv, w, h, BACKDROP_SEED);

        signpost(cv, w / 2, 14);
        cv.text("OpenBikeComputer", Point::new(w / 2, 112), Font::Body, TextAlign::Center, PARCHMENT);

        // Each pair is centred as one group round a drawn dot. A ` · ` in text takes three Display
        // cells, and "Bonjour · Hola" then fills the panel edge to edge.
        let gap = 26;
        for (i, (a, b)) in GREETING.into_iter().enumerate() {
            let y = 158 + i as i32 * (Font::Display.line_height() as i32 + 4);
            let (wa, wb) = (text_width(a, Font::Display) as i32, text_width(b, Font::Display) as i32);
            let left = (w - wa - gap - wb) / 2;
            cv.text(a, Point::new(left, y), Font::Display, TextAlign::Left, PARCHMENT);
            cv.disc(Point::new(left + wa + gap / 2, y + Font::Display.cap_mid() as i32), 3, AMBER);
            cv.text(b, Point::new(left + wa + gap, y), Font::Display, TextAlign::Left, PARCHMENT);
        }

        let label = rx.t(Msg::SetupOk);
        let (bw, bh) = (136, 44);
        let by = h - 20 - bh;
        cv.round(rect(w / 2 - bw / 2, by, bw, bh), 8, AMBER);
        cv.text_vcentered(label, w / 2, (by, bh), Font::Body, TextAlign::Center, ON_ACCENT);
    }
}

/// The brand signpost from `assets/brand/signpost.svg`, its 100-unit view box drawn at one pixel
/// a unit with the top-left at `(cx - 50, top)`: an olive post under two amber arrow boards, each
/// tilted 4° about its own centre line.
fn signpost(cv: &mut impl Surface, cx: i32, top: i32) {
    use palette::*;
    let (x0, y0) = (cx - 50, top);
    cv.fill(rect(x0 + 44, y0 + 10, 12, 81), SUBTEXT);
    // `sin` and `cos` of 4°, because `core` has no trigonometry.
    const S: f32 = 0.069_756;
    const C: f32 = 0.997_564;
    let board = |pts: &[(f32, f32)], pivot: (f32, f32), sin: f32| -> [Point; 5] {
        core::array::from_fn(|i| {
            let (dx, dy) = (pts[i].0 - pivot.0, pts[i].1 - pivot.1);
            let (x, y) = (pivot.0 + dx * C - dy * sin, pivot.1 + dx * sin + dy * C);
            Point::new(x0 + (x + 0.5) as i32, y0 + (y + 0.5) as i32)
        })
    };
    // Each board is a five-point arrow: the tip, then the body's corners in order round the outline.
    let left = board(&[(13.0, 29.0), (27.0, 18.0), (78.0, 18.0), (78.0, 40.0), (27.0, 40.0)], (50.0, 29.0), S);
    let right = board(&[(88.0, 63.0), (74.0, 74.0), (22.0, 74.0), (22.0, 52.0), (74.0, 52.0)], (50.0, 63.0), -S);
    for [tip, a, b, c, d] in [left, right] {
        cv.triangle(tip, a, d, AMBER);
        cv.triangle(a, b, c, AMBER);
        cv.triangle(a, c, d, AMBER);
    }
}

/// The language step: the Settings pick list, where Select also ends the step. The page speaks the
/// language under the cursor, so a rider finds their own by reading it. Only Select commits it.
#[derive(Debug)]
pub struct SetupLanguageScreen(LanguageScreen);

impl SetupLanguageScreen {
    pub fn new(current: Language) -> Self {
        SetupLanguageScreen(LanguageScreen::new(current))
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Press => {
                cx.settings.language = self.0.cursor();
                finish(SetupStep::Language, cx)
            }
            Gesture::Back => back(SetupStep::Language, cx),
            _ => self.0.handle(g, cx),
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let lang = self.0.cursor();
        title_bar(cv, rx.w, rx.h, SetupStep::Language, t(Msg::LanguageTitle, lang));
        self.0.draw_list(cv, rx);
        hint(cv, rx.w, rx.h, true, t(Msg::SetupChoose, lang), Some(t(Msg::SetupOk, lang)));
    }
}

/// The lesson's buttons, in the order of [`SetupButtonsScreen::pressed`]: the left flank top to
/// bottom, then the right.
const BUTTON_NAMES: [Msg; 4] = [Msg::SetupUp, Msg::SetupDown, Msg::SetupSelect, Msg::SetupBack];

/// The button lesson: a signpost board beside each button, pointing at it, and the one gesture
/// worth knowing before the first ride. A press fills its board. Until all four are filled every
/// press only fills, Back included, so the lesson cannot be left by a press it asks for. Then the
/// page acts as every setup page: Select continues and Back returns to the language step.
#[derive(Debug, Default)]
pub struct SetupButtonsScreen {
    /// Up, Down, Select and Back.
    pressed: [bool; 4],
}

impl SetupButtonsScreen {
    fn learnt(&self) -> bool {
        self.pressed == [true; 4]
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.learnt() {
            return match g {
                Gesture::Press => finish(SetupStep::Buttons, cx),
                Gesture::Back => back(SetupStep::Buttons, cx),
                _ => Transition::None,
            };
        }
        // Back-hold never arrives: the app answers it, and setup refuses it.
        let button = match g {
            Gesture::Step(n) if n < 0 => 0,
            Gesture::Step(_) => 1,
            Gesture::Press | Gesture::Hold => 2,
            Gesture::Back | Gesture::BackHold => 3,
        };
        self.pressed[button] = true;
        Transition::None
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_bar(cv, w, h, SetupStep::Buttons, rx.t(Msg::SetupButtonsTitle));
        for (i, name) in BUTTON_NAMES.into_iter().enumerate() {
            board(cv, w, BOARD_ROWS[i % 2], i >= 2, rx.t(name), self.pressed[i]);
        }
        let y = wrapped(cv, rx.t(Msg::SetupHoldBack), w / 2, TIP_TOP, copy_w(w), Font::Label, INK);
        wrapped(cv, rx.t(Msg::SetupMenuAnywhere), w / 2, y, copy_w(w), Font::Label, SUBTEXT);
        if self.learnt() {
            hint(cv, w, h, false, rx.t(Msg::SetupContinue), Some(rx.t(Msg::SetupOk)));
        } else {
            hint(cv, w, h, false, rx.t(Msg::SetupPressEach), None);
        }
    }
}

/// The tops of the upper and the lower row of boards. Each row stands near the height of its
/// buttons, which sit on the flanks below the panel's middle.
const BOARD_ROWS: [i32; 2] = [130, 194];
const BOARD_H: i32 = 40;
const TIP_TOP: i32 = 64;

/// One lesson board, Hello's signpost board laid flat: its tip points at the flank the button is
/// on, left or `right`. A pressed board is filled amber like the brand signpost's.
fn board(cv: &mut impl Surface, w: i32, top: i32, right: bool, name: &str, pressed: bool) {
    use palette::*;
    // The board as `(tip, base, end)` x values, mirrored for the right flank, and its half-height.
    let (tip, base, end) = if right { (w - 10, w - 22, w / 2 + 3) } else { (10, 22, w / 2 - 3) };
    let (cy, r) = (top + BOARD_H / 2, BOARD_H / 2);
    if pressed {
        arrow(cv, (tip, base, end), cy, r, AMBER);
    } else {
        // A 2 px wood outline: the inner board's edges run parallel to the outer ones.
        let s = if right { -1 } else { 1 };
        arrow(cv, (tip, base, end), cy, r, WOOD);
        arrow(cv, (tip + 3 * s, base + s, end - 2 * s), cy, r - 2, PARCHMENT);
    }
    let color = if pressed { ON_ACCENT } else { INK };
    cv.text_vcentered(name, (base + end) / 2, (top, BOARD_H), Font::Label, TextAlign::Center, color);
}

/// Fill an arrow board centred on `cy`, `r` its half-height: the tip at x `tip`, the body from
/// `base` to `end`. It is three triangles, as the signpost's boards are.
fn arrow(cv: &mut impl Surface, (tip, base, end): (i32, i32, i32), cy: i32, r: i32, color: u16) {
    let t = Point::new(tip, cy);
    let (a, b) = (Point::new(base, cy - r), Point::new(end, cy - r));
    let (c, d) = (Point::new(end, cy + r), Point::new(base, cy + r));
    cv.triangle(t, a, d, color);
    cv.triangle(a, b, c, color);
    cv.triangle(a, c, d, color);
}

/// The rider's place among the titled steps, as `(n, of)`. Hello, the first step, has no title
/// bar, so the count starts at the step after it.
fn place(step: SetupStep) -> (usize, usize) {
    let titled = &SetupStep::ORDER[1..];
    (titled.iter().position(|&s| s == step).map_or(0, |i| i + 1), titled.len())
}

/// The head of a titled setup page: the framed title bar, with the rider's [`place`] in its right
/// slot.
fn title_bar(cv: &mut impl Surface, w: i32, h: i32, step: SetupStep, title: &str) {
    let (n, of) = place(step);
    let mut right = heapless::String::<8>::new();
    let _ = write!(right, "{n}/{of}");
    title_frame(cv, w, h, title, &right);
}

/// The height of the [`hint`] band.
const HINT_H: i32 = 40;

/// The controls hint at the foot of a titled setup page: the Up and Down glyphs when they choose,
/// the `text`, and the amber `ok` when Select confirms. The amber OK is Hello's button in small, so
/// it names the press the rider has already made.
fn hint(cv: &mut impl Surface, w: i32, h: i32, arrows: bool, text: &str, ok: Option<&str>) {
    use palette::*;
    let font = Font::Label;
    let top = h - 8 - HINT_H;
    cv.hline(ROW_X, top, w - 2 * ROW_X, RULE);
    let span = (top, HINT_H);
    let cy = top + HINT_H / 2;

    // Up and Down side by side, each `2k` wide.
    let k = 5;
    let arrows_w = if arrows { 4 * k + 3 + 8 } else { 0 };
    let pill_w = ok.map_or(0, |ok| text_width(ok, font) as i32 + 16);
    let text_w = text_width(text, font) as i32;
    let left = (w - (arrows_w + text_w + if ok.is_some() { 24 + pill_w } else { 0 })) / 2;

    if arrows {
        let x = left;
        cv.triangle(Point::new(x + k, cy - k), Point::new(x, cy + k), Point::new(x + 2 * k, cy + k), INK);
        let x = left + arrows_w - 8 - 2 * k;
        cv.triangle(Point::new(x, cy - k), Point::new(x + 2 * k, cy - k), Point::new(x + k, cy + k), INK);
    }
    let x = left + arrows_w;
    cv.text_vcentered(text, x, span, font, TextAlign::Left, SUBTEXT);

    if let Some(ok) = ok {
        let px = x + text_w + 24;
        cv.round(rect(px, cy - 12, pill_w, 24), 6, AMBER);
        cv.text_vcentered(ok, px + pill_w / 2, span, font, TextAlign::Center, ON_ACCENT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode};

    /// Every press only fills its board until all four are filled, Back and Select included. Then
    /// Back returns to the language step, and the persisted step follows.
    #[test]
    fn the_lesson_takes_every_button_before_back_or_select_act() {
        let (mut st, mut act) = (AppState::new(0, 0, 1.0), Activity::new(Mode::Idle));
        let mut s = Settings { setup: SetupStep::Buttons, ..Settings::default() };
        let mut cx = test_ctx(&mut st, &mut act, &mut s);
        let mut lesson = SetupButtonsScreen::default();
        for g in [Gesture::Back, Gesture::Press, Gesture::Back, Gesture::Step(-2)] {
            assert!(matches!(lesson.handle(g, &mut cx), Transition::None), "{g:?} only fills its board");
        }
        assert_eq!(lesson.pressed, [true, false, true, true]);
        assert!(matches!(lesson.handle(Gesture::Step(1), &mut cx), Transition::None), "the last press fills");
        assert!(matches!(lesson.handle(Gesture::Back, &mut cx), Transition::Root(Screen::SetupLanguage(_))));
        assert_eq!(cx.settings.setup, SetupStep::Language);
    }

    /// The count starts at the first titled step and ends at the last step.
    #[test]
    fn the_title_bar_counts_the_titled_steps() {
        let of = SetupStep::ORDER.len() - 1;
        assert_eq!(place(SetupStep::Language), (1, of));
        assert_eq!(place(SetupStep::Buttons), (2, of));
        assert_eq!(place(SetupStep::ORDER[of]), (of, of));
    }
}
