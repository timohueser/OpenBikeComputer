//! First-use setup. A new device, and one after a factory reset, boots into it instead of Home.
//! The step is persisted in [`Settings::setup`](crate::Settings), so setup resumes at its step
//! after a power loss. Each step is a screen: [`screen`] maps a step to it, and the screen calls
//! [`finish`] when its step is done. A new step is a [`SetupStep`] variant, its place in
//! [`SetupStep::next`], its screen, and its arm in [`screen`].
//!
//! The first step is Hello: a greeting in the four UI languages, because the language is not chosen
//! yet.

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::SetupStep;
use crate::Msg;

use super::home::contours;
use super::{palette, Ctx, Render, Screen, Transition};

/// The screen of setup step `step`, or `None` once setup is done.
pub(crate) fn screen(step: SetupStep) -> Option<Screen> {
    match step {
        SetupStep::Hello => Some(Screen::Hello(HelloScreen)),
        SetupStep::Done => None,
    }
}

/// Go to setup step `step`: its screen over Home, or Home once setup is done.
pub(crate) fn go_to(step: SetupStep) -> Transition {
    screen(step).map_or(Transition::Home, Transition::Root)
}

/// End setup step `step`: persist the next step and go to it.
fn finish(step: SetupStep, cx: &mut Ctx) -> Transition {
    cx.settings.setup = step.next();
    go_to(cx.settings.setup)
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
