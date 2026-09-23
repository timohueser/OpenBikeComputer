//! The Home screen: the Idle screensaver and the permanent root of the stack. It draws a
//! topographic backdrop, the wall clock and the battery gauge. Press and back-hold both open the
//! compass [`Menu`](MenuScreen), the one entry point into the app.
//!
//! The backdrop is procedural: a smooth height field (a sum of [`Bump`] Lorentzians, [`field`])
//! traced into iso-lines by marching squares ([`contours`]). The massif is fixed, but a per-open
//! [`seed`](HomeScreen::seed) jitters the bump centres, so the peaks drift on each return.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::settings::DateTime;
use crate::wall_clock::MinuteTicker;

use super::vocab::chrome::{ble_glyph, BLE_GLYPH_W};
use super::vocab::fmt::{clock_hm, write_date_weekday};
use super::{palette, Ctx, MenuScreen, Render, Screen, ScreenTick, Transition};

#[derive(Debug, Default)]
pub struct HomeScreen {
    /// Seed for the contour backdrop's per-open jitter. `0`, the boot default, is the un-jittered
    /// massif.
    seed: u32,
    ticker: MinuteTicker,
}

impl HomeScreen {
    pub fn new() -> Self {
        HomeScreen::default()
    }

    /// Re-roll the backdrop pattern. `seed` is the wall-clock millis, so each return to Home
    /// drifts the peaks to a new spot.
    pub fn reseed(&mut self, seed: u32) {
        self.seed = seed;
    }

    /// The backdrop's current jitter seed, which the
    /// [`RenderKeyKind::Home`](super::RenderKeyKind) key records.
    pub(crate) fn backdrop_seed(&self) -> u32 {
        self.seed
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            // The Menu is the single door into the app.
            Gesture::Press => Transition::Push(Screen::Menu(MenuScreen::new())),
            // Back-hold reaches the Menu through the global escape, not through this screen.
            Gesture::BackHold => Transition::None,
            _ => Transition::None,
        }
    }

    /// Poll the clock's minute tick: dirty once the wall clock crosses into a new minute, then
    /// wake again at the next minute boundary. An idle Home repaints at most once a minute.
    pub fn tick_timers(&mut self, now: DateTime, ms_to_next_minute: u32) -> ScreenTick {
        ScreenTick { changed: self.ticker.changed(now), next_wake_ms: Some(ms_to_next_minute), region: None }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        let (w, h) = (rx.w, rx.h);
        cv.clear(palette::HUD);

        contours(cv, w, h, self.seed);

        // `rx.now` is the live time, not the frozen set-point, so the clock ticks.
        let clock = clock_hm(rx.now.hour, rx.now.minute);
        let clock_top = h * 40 / 100 - Font::Huge.line_height() as i32 / 2;
        cv.text(&clock, Point::new(w / 2, clock_top), Font::Huge, TextAlign::Center, palette::PARCHMENT);

        // The date draws only when the wall clock has an established origin, because a date with
        // no trusted time behind it misleads.
        if rx.clock_set {
            let mut date: heapless::String<16> = heapless::String::new();
            write_date_weekday(&mut date, &rx.now, rx.settings.language);
            let date_y = clock_top + Font::Huge.cap_bottom() as i32 + 6;
            cv.text(&date, Point::new(w / 2, date_y), Font::Label, TextAlign::Center, palette::CONTOUR);
        }

        let device = rx.state.device;
        battery(cv, w, h * 64 / 100 + 6, device.battery_pct);

        // The BLE rune has a fixed top-right slot. Nothing else occupies this corner.
        if device.ble_connected() {
            ble_glyph(cv, w - 10 - BLE_GLYPH_W, 10 + 8, palette::PARCHMENT);
        }
    }
}

const BARS: i32 = 5;

/// Draw the battery gauge, its body centred on `cy`: a rounded shell with a nub, the first
/// `filled` of `BARS` segments in the level colour, and the `NN%` readout. The shell, the gap and
/// the readout are laid out as one group, centred at `w/2`.
fn battery(cv: &mut impl Surface, w: i32, cy: i32, pct: u8) {
    let level = match pct {
        0..=19 => palette::WARNING,
        81..=u8::MAX => palette::ON,
        _ => palette::AMBER,
    };
    let (bw, bh, pad, nub) = (98, 38, 5, 5);

    let mut label: heapless::String<8> = heapless::String::new();
    let _ = write!(label, "{pct}%");
    let gap = 8;
    let lw = text_width(&label, Font::Body) as i32;
    let group = bw + nub + gap + lw;
    let x = (w - group) / 2; // the shell's left edge
    let y = cy - bh / 2;

    cv.round_outline(rect(x, y, bw, bh), 6, palette::PARCHMENT);
    cv.round(rect(x + bw, cy - bh / 6, nub, bh / 3), 2, palette::PARCHMENT);

    let filled = ((pct as i32 * BARS + 50) / 100).clamp(if pct > 0 { 1 } else { 0 }, BARS);
    let cell = (bw - 2 * pad) / BARS;
    let seg = cell - 2; // a 2 px channel between segments
    for i in 0..BARS {
        let color = if i < filled { level } else { palette::CONTOUR };
        cv.round(rect(x + pad + i * cell, y + pad, seg, bh - 2 * pad), 1, color);
    }

    cv.text(
        &label,
        Point::new(x + bw + nub + gap, cy - Font::Body.line_height() as i32 / 2),
        Font::Body,
        TextAlign::Left,
        level,
    );
}

/// One smooth radial hill: a Lorentzian `amp / (1 + r²/σ²)` centred at `(u, v)` in width-normalised
/// coordinates (`u = x/w`, so `v` runs `0..h/w`). Its tails are heavy, so a few bumps cover the
/// whole panel, and it needs no `exp`.
struct Bump {
    u: f32,
    v: f32,
    amp: f32,
    sg: f32,
}

/// The fixed massif: positive is a peak, negative is a basin. If you change these, re-tune
/// [`F_MIN`] and [`F_MAX`] to the new field range.
const BUMPS: [Bump; 8] = [
    Bump { u: 0.50, v: 0.62, amp: 1.00, sg: 0.34 }, // main massif, just above centre
    Bump { u: 0.30, v: 0.30, amp: 0.55, sg: 0.22 }, // NW shoulder
    Bump { u: 0.78, v: 0.42, amp: 0.50, sg: 0.24 }, // NE shoulder
    Bump { u: 0.22, v: 0.95, amp: -0.55, sg: 0.30 }, // SW basin
    Bump { u: 0.80, v: 1.02, amp: -0.45, sg: 0.28 }, // SE basin
    Bump { u: 0.55, v: 1.25, amp: 0.40, sg: 0.26 }, // south knoll
    Bump { u: 0.10, v: 0.55, amp: 0.30, sg: 0.20 }, // W spur
    Bump { u: 0.92, v: 0.78, amp: -0.30, sg: 0.20 }, // E dip
];

/// Sampled range of [`field`] over the panel, for spacing the contour levels inside it. They are
/// constants, not a runtime min/max pass, because the field is fixed.
const F_MIN: f32 = -0.13;
const F_MAX: f32 = 1.15;
/// Number of contour lines. Each one is a full marching-squares pass, so this sets the draw
/// cost.
const LEVELS: usize = 6;
/// Sample columns across the width; the rows follow, to keep the cells square. This is the main
/// speed control: the field evaluations go with `COLS²`.
const COLS: usize = 28;
/// Largest per-open jitter of a bump centre, in width-normalised units.
const JITTER: f32 = 0.08;

/// The height field at width-normalised `(u, v)`: the sum of every [`Bump`] at its jittered
/// centre. `inv_sg2[i]` is the precomputed `1/σ²`, so the inner term is a multiply.
fn field(u: f32, v: f32, centres: &[(f32, f32)], inv_sg2: &[f32]) -> f32 {
    let mut s = 0.0;
    for ((c, b), &iv) in centres.iter().zip(&BUMPS).zip(inv_sg2) {
        let (du, dv) = (u - c.0, v - c.1);
        s += b.amp / (1.0 + (du * du + dv * dv) * iv);
    }
    s
}

/// A deterministic centre offset for bump `i` from the open `seed`, in `[-JITTER, JITTER)`. The
/// hash makes each bump drift independently, but the same seed always gives the same massif.
fn jitter(seed: u32, i: usize) -> (f32, f32) {
    let mut z = seed.wrapping_add(0x9E37_79B9_u32.wrapping_mul(i as u32 + 1));
    z = (z ^ (z >> 16)).wrapping_mul(0x85EB_CA6B);
    z = (z ^ (z >> 13)).wrapping_mul(0xC2B2_AE35);
    z ^= z >> 16;
    // Two independent halves, each scaled from [-1, 1) to the jitter radius.
    let du = ((z & 0xFFFF) as f32 / 32768.0 - 1.0) * JITTER;
    let dv = (((z >> 16) & 0xFFFF) as f32 / 32768.0 - 1.0) * JITTER;
    (du, dv)
}

/// Trace [`field`] into `LEVELS` iso-lines by marching squares. One pass over the grid keeping two
/// rolling sample rows (no full-grid buffer). `seed` jitters the bump centres for this open.
fn contours(cv: &mut impl Surface, w: i32, h: i32, seed: u32) {
    let step = w as f32 / COLS as f32;
    let rows = ((h as f32 / step + 0.5) as usize).max(1); // rounded, to keep the cells square
    let stepy = h as f32 / rows as f32;
    let mut levels = [0.0f32; LEVELS];
    for (i, l) in levels.iter_mut().enumerate() {
        *l = F_MIN + (F_MAX - F_MIN) * (i as f32 + 0.5) / LEVELS as f32;
    }

    // The centres and each bump's `1/σ²` are precomputed, so the per-sample loop has no divide
    // beyond the Lorentzian. The `1/w` normalisation is folded into `sx` and `sy`.
    let centres: [(f32, f32); BUMPS.len()] = core::array::from_fn(|i| {
        let (du, dv) = jitter(seed, i);
        (BUMPS[i].u + du, BUMPS[i].v + dv)
    });
    let inv_sg2: [f32; BUMPS.len()] = core::array::from_fn(|i| 1.0 / (BUMPS[i].sg * BUMPS[i].sg));
    let (sx, sy) = (step / w as f32, stepy / w as f32);
    // Two rolling rows: `prev` = grid row r, `cur` = row r+1.
    let sample = |c: usize, r: usize| field(c as f32 * sx, r as f32 * sy, &centres, &inv_sg2);
    let mut prev = [0.0f32; COLS + 1];
    let mut cur = [0.0f32; COLS + 1];
    for (c, p) in prev.iter_mut().enumerate() {
        *p = sample(c, 0);
    }
    for r in 0..rows {
        for (c, q) in cur.iter_mut().enumerate() {
            *q = sample(c, r + 1);
        }
        let (y0, y1) = (r as f32 * stepy, (r + 1) as f32 * stepy);
        for c in 0..COLS {
            let (x0, x1) = (c as f32 * step, (c + 1) as f32 * step);
            let corners = [prev[c], prev[c + 1], cur[c + 1], cur[c]]; // tl, tr, br, bl
            for &l in &levels {
                cell(cv, (x0, y0), (x1, y1), corners, l);
            }
        }
        core::mem::swap(&mut prev, &mut cur);
    }
}

/// Marching-squares cell: stroke the segment(s) of contour `l` crossing the cell whose corners
/// (clockwise from top-left) are `vals`, interpolating each edge crossing for smoothness.
fn cell(cv: &mut impl Surface, tl: (f32, f32), br: (f32, f32), vals: [f32; 4], l: f32) {
    let [vtl, vtr, vbr, vbl] = vals;
    let case = (vtl >= l) as u8 * 8 + (vtr >= l) as u8 * 4 + (vbr >= l) as u8 * 2 + (vbl >= l) as u8;
    if case == 0 || case == 15 {
        return;
    }
    let (x0, y0, x1, y1) = (tl.0, tl.1, br.0, br.1);
    // Edge crossing between two corners, rounded to a pixel. `denom == 0` means equal corners,
    // which happens only on an edge this case does not use.
    let lerp = |a: (f32, f32), va: f32, b: (f32, f32), vb: f32| {
        let denom = vb - va;
        let t = if denom != 0.0 { ((l - va) / denom).clamp(0.0, 1.0) } else { 0.5 };
        Point::new((a.0 + t * (b.0 - a.0) + 0.5) as i32, (a.1 + t * (b.1 - a.1) + 0.5) as i32)
    };
    let (tlp, trp, brp, blp) = ((x0, y0), (x1, y0), (x1, y1), (x0, y1));
    let top = lerp(tlp, vtl, trp, vtr);
    let right = lerp(trp, vtr, brp, vbr);
    let bot = lerp(blp, vbl, brp, vbr);
    let left = lerp(tlp, vtl, blp, vbl);
    let mut stroke = |a, b| cv.line(a, b, palette::CONTOUR);
    match case {
        1 | 14 => stroke(left, bot),
        2 | 13 => stroke(bot, right),
        3 | 12 => stroke(left, right),
        4 | 11 => stroke(top, right),
        6 | 9 => stroke(top, bot),
        7 | 8 => stroke(top, left),
        5 => {
            stroke(top, left);
            stroke(bot, right);
        }
        10 => {
            stroke(top, right);
            stroke(left, bot);
        }
        _ => {}
    }
}
