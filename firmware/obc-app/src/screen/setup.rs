//! First-use setup. A new device, and one after a factory reset, boots into it instead of Home.
//! The step is persisted in [`Settings::setup`](crate::Settings), so setup resumes at its step
//! after a power loss. Each step is a screen: [`screen`] maps a step to it. Select ends the step
//! through [`finish`], and Back returns to the step before through [`back`]. A new step is a
//! [`SetupStep`] variant, its place in `SetupStep::ORDER`, its screen, and its arm in [`screen`].
//!
//! The first step is Hello: a greeting in the four UI languages, because the language is not chosen
//! yet. Every step after it is a titled page with the [`hint`] at its foot.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::{Language, Settings, SetupStep, Theme, Units};
use crate::Msg;

use super::home::contours;
use super::settings::LanguageScreen;
use super::vocab::chrome::{title_frame, LIST_TOP};
use super::vocab::flags::{FLAG_H, FLAG_W};
use super::vocab::rows::{choice_row, nav_row, row_rect, row_tick, Line2, ROW_GAP, ROW_ONE, ROW_TWO, ROW_X};
use super::vocab::tiles::tile;
use super::{palette, Ctx, Render, Screen, Transition};

/// The screen of the current setup step, or `None` once setup is done.
pub(crate) fn screen(s: &Settings) -> Option<Screen> {
    match s.setup {
        SetupStep::Hello => Some(Screen::Hello(HelloScreen)),
        SetupStep::Language => Some(Screen::SetupLanguage(SetupLanguageScreen::new(s.language))),
        SetupStep::Units => Some(Screen::SetupUnits(SetupUnitsScreen(s.units))),
        SetupStep::Theme => Some(Screen::SetupTheme(SetupThemeScreen(s.theme))),
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

/// The language step: the Settings pick list, where Select also ends the step.
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
        self.0.draw(cv, rx);
        hint(cv, rx.w, rx.h, rx.t(Msg::SetupChoose), rx.t(Msg::SetupOk));
    }
}

/// The units step: Metric or Imperial, each with the symbols it reads in, over the ride tiles in
/// the units under the cursor.
#[derive(Debug)]
pub struct SetupUnitsScreen(pub(crate) Units);

impl SetupUnitsScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                self.0 = self.0.stepped(n);
                Transition::None
            }
            Gesture::Press => {
                cx.settings.units = self.0;
                finish(SetupStep::Units, cx)
            }
            Gesture::Back => back(SetupStep::Units, cx),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::SetupUnits), "");
        for (i, units) in Units::ALL.into_iter().enumerate() {
            let area = row_rect(LIST_TOP + i as i32 * (ROW_TWO + ROW_GAP), w, ROW_TWO);
            let symbols = if units.is_imperial() { "mi \u{b7} ft" } else { "km \u{b7} m" };
            let name = units.name(rx.settings.language);
            nav_row(cv, area, name, Some(Line2::text(symbols)), units == self.0, true, false);
            if units == rx.settings.units {
                row_tick(cv, area);
            }
        }
        ride_preview(cv, rx, self.0);
        hint(cv, w, h, rx.t(Msg::SetupChoose), rx.t(Msg::SetupOk));
    }
}

/// The theme step: Light or Dark. The whole page draws in the theme under the cursor (see
/// [`App::theme`](crate::App::theme)), and only Select commits it.
#[derive(Debug)]
pub struct SetupThemeScreen(pub(crate) Theme);

impl SetupThemeScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => {
                self.0 = self.0.stepped(n);
                Transition::None
            }
            Gesture::Press => {
                cx.settings.theme = self.0;
                finish(SetupStep::Theme, cx)
            }
            Gesture::Back => back(SetupStep::Theme, cx),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::SetupTheme), "");
        for (i, theme) in Theme::ALL.into_iter().enumerate() {
            let area = row_rect(LIST_TOP + i as i32 * (ROW_ONE + ROW_GAP), w, ROW_ONE);
            let name = theme.name(rx.settings.language);
            let committed = theme == rx.settings.theme;
            choice_row(cv, area, name, theme == self.0, committed, |cv, x, y| swatch(cv, x, y, theme));
        }
        ride_preview(cv, rx, rx.settings.units);
        hint(cv, w, h, rx.t(Msg::SetupChoose), rx.t(Msg::SetupOk));
    }
}

/// A theme's page in the flag slot: its paper, its ink round the edge, and two lines of text. The
/// colours are ones the theme mapping keeps, so each swatch shows its own theme on either page.
fn swatch(cv: &mut impl Surface, x: i32, y: i32, theme: Theme) {
    use palette::*;
    let (paper, ink) = match theme {
        Theme::Light => (ART_WHITE, ON_ACCENT),
        Theme::Dark => (ON_ACCENT, ART_WHITE),
    };
    cv.fill(rect(x, y, FLAG_W, FLAG_H), ink);
    cv.fill(rect(x + 1, y + 1, FLAG_W - 2, FLAG_H - 2), paper);
    cv.fill(rect(x + 4, y + 3, FLAG_W - 8, 2), ink);
    cv.fill(rect(x + 4, y + 7, FLAG_W - 12, 2), ink);
}

/// The sample ride the preview shows: a speed in km/h and a distance in km.
const SAMPLE: (f32, f32) = (24.5, 86.4);

/// Two ride tiles over the [`hint`], so a step shows what it changes on the ride screens. They sit
/// at one place on every step, so a step changes them in place.
fn ride_preview(cv: &mut impl Surface, rx: &Render, units: Units) {
    use palette::*;
    let (gap, tile_h) = (6, 54);
    let tile_w = (rx.w - 2 * ROW_X - gap) / 2;
    let y = rx.h - 8 - HINT_H - 12 - tile_h;
    let (mut speed, mut dist) = (heapless::String::<8>::new(), heapless::String::<8>::new());
    let _ = write!(speed, "{:.1}", units.speed(SAMPLE.0));
    let _ = write!(dist, "{:.1}", units.dist(SAMPLE.1));
    for (i, (caption, value)) in [(units.speed_label(), speed), (units.dist_label(), dist)].into_iter().enumerate() {
        let area = rect(ROW_X + i as i32 * (tile_w + gap), y, tile_w, tile_h);
        tile(cv, area, &rx.marquee, caption, &value, false, TextAlign::Left, PARCHMENT_SHADE, SUBTEXT, INK);
    }
}

/// The height of the [`hint`] band.
const HINT_H: i32 = 40;

/// The controls hint at the foot of a setup page: Up and Down choose, Select confirms. The amber OK
/// is Hello's button in small, so it names the press the rider has already made.
fn hint(cv: &mut impl Surface, w: i32, h: i32, choose: &str, ok: &str) {
    use palette::*;
    let font = Font::Label;
    let top = h - 8 - HINT_H;
    cv.hline(ROW_X, top, w - 2 * ROW_X, RULE);
    let span = (top, HINT_H);
    let cy = top + HINT_H / 2;

    // Up and Down side by side, each `2k` wide.
    let k = 5;
    let arrows_w = 4 * k + 3;
    let (choose_w, ok_w) = (text_width(choose, font) as i32, text_width(ok, font) as i32);
    let pill_w = ok_w + 16;
    let left = (w - (arrows_w + 8 + choose_w + 24 + pill_w)) / 2;

    let x = left;
    cv.triangle(Point::new(x + k, cy - k), Point::new(x, cy + k), Point::new(x + 2 * k, cy + k), INK);
    let x = left + arrows_w - 2 * k;
    cv.triangle(Point::new(x, cy - k), Point::new(x + 2 * k, cy - k), Point::new(x + k, cy + k), INK);
    let x = left + arrows_w + 8;
    cv.text_vcentered(choose, x, span, font, TextAlign::Left, SUBTEXT);

    let px = x + choose_w + 24;
    cv.round(rect(px, cy - 12, pill_w, 24), 6, AMBER);
    cv.text_vcentered(ok, px + pill_w / 2, span, font, TextAlign::Center, ON_ACCENT);
}
