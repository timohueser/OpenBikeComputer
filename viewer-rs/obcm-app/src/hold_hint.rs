//! The global long-press hint — a small "charge-in-place" pill that surfaces the
//! device's central hold gesture wherever it's available.
//!
//! Long-press is the spine of the input model, but until now only the Ride-control
//! overlay showed it. This is the *one shared layer* that makes it visible
//! everywhere: [`App`](crate::app::App) folds each frame's encoder/Back
//! hold-progress into [`HoldHints`] and draws it on top of the screen stack.
//!
//! Per control a fixed-length pill sits at the screen edge nearest its physical
//! button (both on the right today — encoder up top, Back below). Holding fills the
//! pill from its centre outward (so it reads as "keep holding until it's full"); a
//! completed hold **pops** — a brief thickness kick — and an early release retracts.
//! Each pill is colour-coded (amber = encoder, teal = Back) and backed by a dark
//! casing so it stays legible over a busy map. The panel has no alpha and the frame
//! is fully redrawn, so everything here is opaque and simply absent once an
//! animation ends — no fades, no ghosting.
//!
//! Relocating or recolouring a pill is a one-line edit to its [`Style`].

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obcm_render::{rect, Canvas};

use crate::screen::palette;

// --- tunables -------------------------------------------------------------

/// Gap (px) between a pill's outer edge and the screen edge it hugs.
const INSET: i32 = 5;
/// Dark-casing thickness (px) drawn around the pill for legibility over any map.
/// Kept `<= INSET` so the casing never spills off the panel edge.
const PAD: i32 = 2;
/// Track / fill cross-thickness (px).
const W: i32 = 6;
/// Peak cross-thickness at the confirm "pop".
const WPOP: i32 = 11;
/// Half the fixed track length (px): the pill is `2 * HALF` long and stays put while
/// the fill grows inside it — the "charge in place" the goal length communicates.
const HALF: i32 = 24;
/// Pop animation duration (ms).
const POP_MS: u32 = 180;
/// Retract animation duration (ms) when a hold is released early.
const CANCEL_MS: u32 = 150;
/// Dead zone: the charge fraction a hold must pass before any pill is drawn, so a
/// quick tap stays completely clean (a short press shouldn't flicker a pill). The
/// drawn fill is remapped ([`shown`]) so the pill emerges empty right at this point
/// and reaches full at the threshold. `0.30` ≈ 150 ms of the 500 ms hold.
const DEAD: f32 = 0.30;

// --- placement ------------------------------------------------------------

/// Which screen edge a pill hugs. Both controls live on [`Right`](Edge::Right)
/// today; the other three are the supported relocations (change a [`Style::anchor`]).
#[allow(dead_code)] // Left / Top / Bottom are the relocation options, not dead.
#[derive(Clone, Copy)]
enum Edge {
    Right,
    Left,
    Top,
    Bottom,
}

/// Where a pill sits: an [`Edge`] plus a `0.0..=1.0` position along it (0 = top/left
/// end, 1 = bottom/right end).
#[derive(Clone, Copy)]
struct Anchor {
    edge: Edge,
    pos: f32,
}

impl Anchor {
    /// Resolve to a concrete [`Place`] for a `w`×`h` screen, clamping the centre so
    /// the whole pill (plus casing) stays on-panel.
    fn place(self, w: i32, h: i32) -> Place {
        let vertical = matches!(self.edge, Edge::Right | Edge::Left);
        let along = if vertical { h } else { w };
        let lo = HALF + PAD + 1;
        let hi = (along - HALF - PAD - 1).max(lo);
        let cc = ((self.pos * along as f32) as i32).clamp(lo, hi);
        match self.edge {
            Edge::Right => Place { vertical, outer: w - INSET, inward: -1, cc },
            Edge::Left => Place { vertical, outer: INSET, inward: 1, cc },
            Edge::Bottom => Place { vertical, outer: h - INSET, inward: -1, cc },
            Edge::Top => Place { vertical, outer: INSET, inward: 1, cc },
        }
    }
}

/// A resolved placement: orientation, the fixed outer (edge) coordinate, the inward
/// growth direction (`±1`), and the centre along the edge.
struct Place {
    vertical: bool,
    outer: i32,
    inward: i32,
    cc: i32,
}

impl Place {
    /// The area for a pill of cross-thickness `t` and half-length `a`, pinned to the
    /// edge and growing inward — so a thicker pop never crosses the panel boundary.
    fn pill(&self, t: i32, a: i32) -> Rectangle {
        let near = if self.inward < 0 { self.outer - t } else { self.outer };
        if self.vertical {
            rect(near, self.cc - a, t, 2 * a)
        } else {
            rect(self.cc - a, near, 2 * a, t)
        }
    }
}

/// Grow a rectangle by `pad` on every side — the dark casing around a pill.
fn inflate(r: Rectangle, pad: i32) -> Rectangle {
    rect(
        r.top_left.x - pad,
        r.top_left.y - pad,
        r.size.width as i32 + 2 * pad,
        r.size.height as i32 + 2 * pad,
    )
}

/// Capsule corner radius for a pill rectangle (half its short side).
fn rad(r: &Rectangle) -> u32 {
    r.size.width.min(r.size.height) / 2
}

/// Linear `0.0..` progress through a `dur`-ms animation that began at `t0`.
/// `wrapping_sub` tolerates the millis clock wrapping.
fn frac(now: u32, t0: u32, dur: u32) -> f32 {
    now.wrapping_sub(t0) as f32 / dur as f32
}

/// Remap raw hold progress through the [`DEAD`] zone to a `0.0..=1.0` drawn fill:
/// `0` at the dead-zone exit, `1` at the threshold.
fn shown(progress: f32) -> f32 {
    ((progress - DEAD) / (1.0 - DEAD)).clamp(0.0, 1.0)
}

// --- per-control state ----------------------------------------------------

/// A hint's transient animation, layered on top of the live charge.
#[derive(Clone, Copy)]
enum Anim {
    /// No animation — drawing follows the live charge in [`Hint::prev`].
    Idle,
    /// The hold completed: a brief thickness pop that began at `t0`.
    Pop { t0: u32 },
    /// The hold was released early: the fill retracts from `from`, starting at `t0`.
    Cancel { t0: u32, from: f32 },
}

/// One control's hint: its current animation and the last charge fraction seen.
struct Hint {
    anim: Anim,
    prev: f32,
}

impl Hint {
    const fn new() -> Self {
        Hint { anim: Anim::Idle, prev: 0.0 }
    }

    /// Fold this frame's `progress` (`0.0..=1.0`; `0` once the hold has fired or the
    /// button is up) and whether the long-press `fired` this frame into the state.
    fn update(&mut self, now: u32, progress: f32, fired: bool) {
        // Retire a finished pop / retract first, so the next event starts clean.
        self.anim = match self.anim {
            Anim::Pop { t0 } if now.wrapping_sub(t0) >= POP_MS => Anim::Idle,
            Anim::Cancel { t0, .. } if now.wrapping_sub(t0) >= CANCEL_MS => Anim::Idle,
            a => a,
        };
        if fired {
            // Threshold crossed → the confirm pop (the live charge is already 0).
            self.anim = Anim::Pop { t0: now };
        } else if progress == 0.0 && self.prev > DEAD && matches!(self.anim, Anim::Idle) {
            // Released past the dead zone but before the threshold → retract the fill.
            self.anim = Anim::Cancel { t0: now, from: shown(self.prev) };
        }
        self.prev = progress;
    }

    /// Draw the hint for this control, or nothing when idle and uncharged.
    fn draw<D, F>(&self, cv: &mut Canvas<D, F>, style: &Style, now: u32, w: i32, h: i32)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let place = style.anchor.place(w, h);
        match self.anim {
            Anim::Pop { t0 } => {
                let e = frac(now, t0, POP_MS);
                if e >= 1.0 {
                    return;
                }
                let bump = 4.0 * e * (1.0 - e); // 0 → 1 → 0 parabola (no libm)
                let t = W + ((WPOP - W) as f32 * bump) as i32;
                let pill = place.pill(t, HALF);
                let case = inflate(pill, PAD);
                cv.round(case, rad(&case), palette::HUD);
                cv.round(pill, rad(&pill), style.bright);
            }
            Anim::Cancel { t0, from } => {
                let e = frac(now, t0, CANCEL_MS);
                if e >= 1.0 {
                    return;
                }
                charging(cv, &place, from * (1.0 - e), style);
            }
            Anim::Idle => {
                if self.prev > DEAD {
                    charging(cv, &place, shown(self.prev), style);
                }
            }
        }
    }
}

/// The charge-in-place body: dark casing, the dim full-length track (the "hold until
/// here" goal), then the bright fill growing from the centre out.
fn charging<D, F>(cv: &mut Canvas<D, F>, place: &Place, fill: f32, style: &Style)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let track = place.pill(W, HALF);
    let case = inflate(track, PAD);
    cv.round(case, rad(&case), palette::HUD);
    cv.round(track, rad(&track), style.dim);
    let a = (HALF as f32 * fill.clamp(0.0, 1.0)) as i32;
    if a > 0 {
        let f = place.pill(W, a);
        cv.round(f, rad(&f), style.bright);
    }
}

// --- the overlay ----------------------------------------------------------

/// Per-control colour + anchor. Relocating a pill or recolouring it is a one-line
/// change to one of the [`ENCODER`] / [`BACK`] constants below.
struct Style {
    anchor: Anchor,
    bright: u16,
    dim: u16,
}

/// Encoder hint — amber, upper-right edge (the encoder wheel sits near the top right).
const ENCODER: Style = Style {
    anchor: Anchor { edge: Edge::Right, pos: 0.30 },
    bright: palette::AMBER,
    dim: palette::WOOD,
};

/// Back hint — teal, lower-right edge (the Back button sits below the encoder).
const BACK: Style = Style {
    anchor: Anchor { edge: Edge::Right, pos: 0.70 },
    bright: palette::TEAL,
    dim: palette::TEAL_DIM,
};

/// The global long-press overlay: one [`Hint`] per control, drawn above every screen.
///
/// [`App`](crate::app::App) feeds each frame's hold-progress and long-press firings
/// through [`update`](HoldHints::update), then [`draw`](HoldHints::draw)s this on top
/// of the screen stack.
pub struct HoldHints {
    encoder: Hint,
    back: Hint,
}

impl HoldHints {
    pub const fn new() -> Self {
        HoldHints { encoder: Hint::new(), back: Hint::new() }
    }

    /// Advance both hints one frame. `enc`/`back` are the live hold fractions
    /// (`0.0..=1.0`); `enc_fired`/`back_fired` mark the frame each long-press crossed
    /// its threshold (so the pill pops the instant the action commits).
    pub fn update(&mut self, now: u32, enc: f32, back: f32, enc_fired: bool, back_fired: bool) {
        self.encoder.update(now, enc, enc_fired);
        self.back.update(now, back, back_fired);
    }

    /// Draw both hints above the current screen, into a `w`×`h` target.
    pub fn draw<D, F>(&self, target: &mut D, color_fn: &F, w: i32, h: i32, now: u32)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let mut cv = Canvas::new(target, color_fn);
        self.encoder.draw(&mut cv, &ENCODER, now, w, h);
        self.back.draw(&mut cv, &BACK, now, w, h);
    }
}

impl Default for HoldHints {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_idle(h: &Hint) -> bool {
        matches!(h.anim, Anim::Idle)
    }
    fn is_pop(h: &Hint) -> bool {
        matches!(h.anim, Anim::Pop { .. })
    }
    fn is_cancel(h: &Hint) -> bool {
        matches!(h.anim, Anim::Cancel { .. })
    }

    #[test]
    fn a_completed_hold_pops_then_clears() {
        let mut h = Hint::new();
        h.update(0, 0.5, false);
        h.update(100, 0.99, false);
        assert!(is_idle(&h), "still just charging before the threshold");
        h.update(150, 0.0, true); // long-press fires (progress already cleared)
        assert!(is_pop(&h));
        h.update(150 + POP_MS, 0.0, false); // pop runs its course
        assert!(is_idle(&h));
    }

    #[test]
    fn an_early_release_retracts_then_clears() {
        let mut h = Hint::new();
        h.update(0, 0.4, false);
        h.update(50, 0.0, false); // let go before the threshold
        assert!(is_cancel(&h));
        h.update(50 + CANCEL_MS, 0.0, false);
        assert!(is_idle(&h));
    }

    #[test]
    fn a_tap_stays_quiet() {
        let mut h = Hint::new();
        h.update(0, 0.03, false); // barely brushed, inside the dead zone
        h.update(20, 0.0, false);
        assert!(is_idle(&h), "an early release inside the DEAD zone must not animate");
    }

    #[test]
    fn firing_beats_retracting_on_the_crossing_frame() {
        let mut h = Hint::new();
        h.update(0, 0.99, false);
        h.update(10, 0.0, true); // threshold crossed and progress cleared same frame
        assert!(is_pop(&h), "crossing the threshold is a pop, never a retract");
    }
}
