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

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::effort::Metric;
use crate::i18n::t;
use crate::input::Gesture;
use crate::settings::{Language, Settings, SetupStep, Theme, Units, SENSOR_SLOTS};
use crate::Msg;

use super::context_drawer::{ContextDrawerScreen, ContextValue};
use super::home::contours;
use super::settings::{kind_msg, status_line, wake_msg, LanguageScreen, SensorScanScreen};
use super::vocab::chrome::{copy_w, title_frame, wrapped, LIST_TOP};
use super::vocab::flags::{FLAG_H, FLAG_W};
use super::vocab::list::on_step;
use super::vocab::rows::{
    action_row, choice_row, nav_row, row_rect, row_tick, Line2, ROW_GAP, ROW_ONE, ROW_TWO, ROW_X,
};
use super::vocab::tiles::{tile, zone_tile};
use super::{palette, Ctx, Render, Screen, Transition};

/// The screen of the current setup step, or `None` once setup is done.
pub(crate) fn screen(s: &Settings) -> Option<Screen> {
    match s.setup {
        SetupStep::Hello => Some(Screen::Hello(HelloScreen)),
        SetupStep::Language => Some(Screen::SetupLanguage(SetupLanguageScreen::new(s.language))),
        SetupStep::Buttons => Some(Screen::SetupButtons(SetupButtonsScreen::default())),
        SetupStep::Units => Some(Screen::SetupUnits(SetupUnitsScreen(s.units))),
        SetupStep::Theme => Some(Screen::SetupTheme(SetupThemeScreen(s.theme))),
        SetupStep::Sensors => Some(Screen::SetupSensors(SetupSensorsScreen::default())),
        SetupStep::Effort => Some(Screen::SetupEffort(SetupEffortScreen::default())),
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
/// bottom, then the right. They are English in every language, so they are not catalog strings.
const BUTTON_NAMES: [&str; 4] = ["UP", "DOWN", "SELECT", "BACK"];

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
            board(cv, w, BOARD_ROWS[i % 2], i >= 2, name, self.pressed[i]);
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
        title_bar(cv, w, h, SetupStep::Units, rx.t(Msg::SetupUnits));
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
        hint(cv, w, h, true, rx.t(Msg::SetupChoose), Some(rx.t(Msg::SetupOk)));
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
        title_bar(cv, w, h, SetupStep::Theme, rx.t(Msg::SetupTheme));
        for (i, theme) in Theme::ALL.into_iter().enumerate() {
            let area = row_rect(LIST_TOP + i as i32 * (ROW_ONE + ROW_GAP), w, ROW_ONE);
            let name = theme.name(rx.settings.language);
            let committed = theme == rx.settings.theme;
            choice_row(cv, area, name, theme == self.0, committed, |cv, x, y| swatch(cv, x, y, theme));
        }
        ride_preview(cv, rx, rx.settings.units);
        hint(cv, w, h, true, rx.t(Msg::SetupChoose), Some(rx.t(Msg::SetupOk)));
    }
}

/// The sensors step: the three slots of Settings' Sensors page over Skip, which reads Continue
/// once a sensor is saved. Select on a slot opens the Settings scan list for its kind, where a pick
/// saves the sensor and returns here. An empty slot says how to wake its sensor, and a saved one
/// shows its live status.
#[derive(Debug, Default)]
pub struct SetupSensorsScreen {
    /// A slot, or [`SKIP`].
    selected: usize,
}

/// The Skip row's index, after the slots.
const SKIP: usize = SENSOR_SLOTS;

impl SetupSensorsScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => on_step(&mut self.selected, n, SKIP + 1),
            Gesture::Press if self.selected == SKIP => finish(SetupStep::Sensors, cx),
            // Scan mode makes the host run a discovery scan. The scan list lowers it on exit.
            Gesture::Press => {
                cx.activity.request_sensor_scan(true);
                Transition::Push(Screen::SetupSensorScan(SensorScanScreen::new(self.selected as u8)))
            }
            Gesture::Back => back(SetupStep::Sensors, cx),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        title_bar(cv, w, h, SetupStep::Sensors, rx.t(Msg::SensorsTitle));
        let saved = rx.settings.saved_sensors;
        for (slot, sensor) in saved.iter().enumerate() {
            let area = row_rect(LIST_TOP + slot as i32 * (ROW_TWO + ROW_GAP), w, ROW_TWO);
            let mut line = heapless::String::<24>::new();
            if sensor.present {
                let status = rx.sensor_status.get(slot).copied().unwrap_or_default();
                status_line(&mut line, true, status, rx.settings.language);
            } else {
                let _ = line.push_str(rx.t(wake_msg(slot)));
            }
            nav_row(cv, area, rx.t(kind_msg(slot)), Some(Line2::text(&line)), slot == self.selected, true, false);
        }
        let label = if saved.iter().any(|s| s.present) { Msg::SetupContinue } else { Msg::SetupSkip };
        let area = row_rect(h - 8 - HINT_H - 12 - ROW_ONE, w, ROW_ONE);
        nav_row(cv, area, rx.t(label), None, self.selected == SKIP, true, true);
        hint(cv, w, h, true, rx.t(Msg::SetupChoose), Some(rx.t(Msg::SetupOk)));
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

/// The effort step: the two limits the effort zones are cut from, and a row that continues. A
/// value row opens the drawer editor as a sheet over the page, as on the Ride settings page, and
/// the editor's Select commits and saves the value. The last row reads Skip while neither limit
/// is set. The ride tiles below show the zones a limit gives, and a plain tile without one.
#[derive(Debug, Default)]
pub struct SetupEffortScreen {
    selected: usize,
}

/// The value rows, then the continue row at `LIMITS.len()`.
const LIMITS: [(Msg, ContextValue); 2] = [(Msg::RideMaxHr, ContextValue::MaxHr), (Msg::RideFtp, ContextValue::Ftp)];

/// The sample efforts the preview shows, in bpm and watts.
const SAMPLE_EFFORT: [(Metric, u32); 2] = [(Metric::Hr, 152), (Metric::Power, 210)];

impl SetupEffortScreen {
    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Step(n) => on_step(&mut self.selected, n, LIMITS.len() + 1),
            Gesture::Press => match LIMITS.get(self.selected) {
                Some(&(label, value)) => Transition::Push(Screen::ContextDrawer(ContextDrawerScreen::editor(
                    value,
                    label,
                    &cx.context_facts(),
                ))),
                None => finish(SetupStep::Effort, cx),
            },
            Gesture::Back => back(SetupStep::Effort, cx),
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        let (w, h) = (rx.w, rx.h);
        title_bar(cv, w, h, SetupStep::Effort, rx.t(Msg::SetupEffort));
        let facts = rx.context_facts();
        let mut y = LIST_TOP;
        for (i, (label, value)) in LIMITS.into_iter().enumerate() {
            let mut buf = heapless::String::<24>::new();
            let text = value.choice_label(value.committed(&facts), rx, &mut buf);
            nav_row(cv, row_rect(y, w, ROW_TWO), rx.t(label), Some(Line2::text(text)), i == self.selected, true, true);
            y += ROW_TWO + ROW_GAP;
        }
        let unset = rx.settings.max_hr == 0 && rx.settings.ftp_w == 0;
        let go = rx.t(if unset { Msg::SetupSkip } else { Msg::SetupContinue });
        action_row(cv, row_rect(y, w, ROW_ONE), go, None, self.selected == LIMITS.len(), true, false, 0.0);

        let limits = rx.settings.effort_limits();
        for (i, (metric, value)) in SAMPLE_EFFORT.into_iter().enumerate() {
            let caption = rx.t(if metric == Metric::Hr { Msg::TileHr } else { Msg::TilePwr });
            let mut text = heapless::String::<8>::new();
            let _ = write!(text, "{value}");
            let area = preview_tile(rx, i);
            match metric.limit(limits) {
                Some(limit) => zone_tile(cv, area, caption, &text, metric.zone_of(value, limit)),
                None => {
                    tile(cv, area, &rx.marquee, caption, &text, false, TextAlign::Left, PARCHMENT_SHADE, SUBTEXT, INK)
                }
            }
        }
        hint(cv, w, h, true, rx.t(Msg::SetupChoose), Some(rx.t(Msg::SetupOk)));
    }
}

/// The sample ride the preview shows: a speed in km/h and a distance in km.
const SAMPLE: (f32, f32) = (24.5, 86.4);

/// Two ride tiles over the [`hint`], so a step shows what it changes on the ride screens.
fn ride_preview(cv: &mut impl Surface, rx: &Render, units: Units) {
    use palette::*;
    let (mut speed, mut dist) = (heapless::String::<8>::new(), heapless::String::<8>::new());
    let _ = write!(speed, "{:.1}", units.speed(SAMPLE.0));
    let _ = write!(dist, "{:.1}", units.dist(SAMPLE.1));
    for (i, (caption, value)) in [(units.speed_label(), speed), (units.dist_label(), dist)].into_iter().enumerate() {
        let area = preview_tile(rx, i);
        tile(cv, area, &rx.marquee, caption, &value, false, TextAlign::Left, PARCHMENT_SHADE, SUBTEXT, INK);
    }
}

/// The left (`i` 0) or right preview tile over the [`hint`]. The tiles sit at one place on every
/// step, so a step changes them in place.
fn preview_tile(rx: &Render, i: usize) -> Rectangle {
    let (gap, tile_h) = (6, 54);
    let tile_w = (rx.w - 2 * ROW_X - gap) / 2;
    rect(ROW_X + i as i32 * (tile_w + gap), rx.h - 8 - HINT_H - 12 - tile_h, tile_w, tile_h)
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
        let places = [
            SetupStep::Language,
            SetupStep::Buttons,
            SetupStep::Units,
            SetupStep::Theme,
            SetupStep::Sensors,
            SetupStep::Effort,
        ];
        assert_eq!(places.map(place), [(1, 6), (2, 6), (3, 6), (4, 6), (5, 6), (6, 6)]);
    }
}
