//! The global long-press hint — an on-screen "frame bulge" that surfaces the
//! device's central hold gesture wherever it's available.
//!
//! Long-press is the spine of the input model, but until now only the Ride-control
//! overlay showed it. This is the *one shared layer* that makes it visible
//! everywhere: [`App`](crate::app::App) folds each frame's encoder/Back
//! hold-progress into [`HoldHints`] and draws it on top of the screen stack.
//!
//! Per control a black hump swells inward from the screen edge nearest its physical
//! button (both on the right today — encoder up top, Back below), so it reads as the
//! black bezel *bulging into* the display — the iOS volume/lock-button press effect.
//! The hump has a **fixed base width** along the edge ([`BASE_HALF`]) and only its
//! inward *depth* tracks the hold, so holding pushes it deeper in place rather than
//! fanning it wider. Its silhouette is a quartic bump (flat tangent shoulders that
//! melt into the edge, a softly rounded apex — deliberately *not* a round circular
//! cap), rasterized as edge-perpendicular strips. A completed hold **pops** — a
//! quick deeper lunge that eases back out — and an early release retracts it.
//!
//! Drawn in [`palette::HUD`] (the near-black frame colour), so it needs no gradient
//! or alpha and renders on the real 8-color panel exactly as in the simulator. The
//! panel has no alpha and the frame is fully redrawn, so the bulge is opaque and
//! simply absent once an animation ends — no fades, no ghosting.
//!
//! Encoder vs. Back is told apart by *position* (the bulge erupts next to the button
//! held), not colour. Relocating a bulge is a one-line edit to its [`Style::anchor`].

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_render::{rect, Canvas};

use crate::screen::palette;

// tunables

/// Half the bulge's fixed base width (px) along the edge — the hump spans `2 *
/// BASE_HALF` regardless of how deep it bulges, so depth and width are independent.
const BASE_HALF: i32 = 22;
/// Inward depth (px) the bulge reaches at a full charge, just before the threshold.
const DEPTH: f32 = 12.0;
/// Peak inward depth (px) at the confirm "pop" — a brief deeper lunge past [`DEPTH`].
const POP_DEPTH: f32 = 22.0;
/// Pop animation duration (ms).
const POP_MS: u32 = 220;
/// Fraction of the pop spent lunging in to [`POP_DEPTH`] before it eases back out —
/// a fast attack, slow release, so the confirm reads as a snap.
const POP_ATTACK: f32 = 0.22;
/// Retract animation duration (ms) when a hold is released early.
const CANCEL_MS: u32 = 150;
/// Dead zone: the charge fraction a hold must pass before any bulge is drawn, so a
/// quick tap stays completely clean (a short press shouldn't flicker a bulge). The
/// drawn depth is remapped ([`shown`]) so the bulge emerges flat right at this point
/// and reaches full depth at the threshold. `0.30` ≈ 150 ms of the 500 ms hold.
const DEAD: f32 = 0.30;

// placement

/// Which screen edge a bulge erupts from. Both controls live on [`Right`](Edge::Right)
/// today; the other three are the supported relocations (change a [`Style::anchor`]).
#[allow(dead_code)] // Left / Top / Bottom are the relocation options, not dead.
#[derive(Clone, Copy)]
enum Edge {
    Right,
    Left,
    Top,
    Bottom,
}

/// Where a bulge sits: an [`Edge`] plus a `0.0..=1.0` position along it (0 = top/left
/// end, 1 = bottom/right end).
#[derive(Clone, Copy)]
struct Anchor {
    edge: Edge,
    pos: f32,
}

impl Anchor {
    /// Resolve to a concrete [`Place`] for a `w`×`h` screen, clamping the centre so
    /// the fixed-width base stays on-panel along the edge.
    fn place(self, w: i32, h: i32) -> Place {
        let vertical = matches!(self.edge, Edge::Right | Edge::Left);
        let along = if vertical { h } else { w };
        let lo = BASE_HALF + 1;
        let hi = (along - BASE_HALF - 1).max(lo);
        let cc = ((self.pos * along as f32) as i32).clamp(lo, hi);
        match self.edge {
            Edge::Right => Place { vertical, outer: w, inward: -1, cc },
            Edge::Left => Place { vertical, outer: 0, inward: 1, cc },
            Edge::Bottom => Place { vertical, outer: h, inward: -1, cc },
            Edge::Top => Place { vertical, outer: 0, inward: 1, cc },
        }
    }
}

/// A resolved placement: orientation, the edge coordinate (`outer`), the inward
/// growth direction (`±1`), and the centre along the edge.
struct Place {
    vertical: bool,
    outer: i32,
    inward: i32,
    cc: i32,
}

impl Place {
    /// One rasterizer strip: a 1px-wide slice `along_off` from the centre that pokes
    /// `depth` px inward from the edge. The bulge is the union of these across the
    /// fixed base width.
    fn strip(&self, along_off: i32, depth: i32) -> Rectangle {
        let a = self.cc + along_off;
        let near = if self.inward < 0 { self.outer - depth } else { self.outer };
        if self.vertical {
            rect(near, a, depth, 1)
        } else {
            rect(a, near, 1, depth)
        }
    }
}

/// Linear `0.0..` progress through a `dur`-ms animation that began at `t0`.
/// `wrapping_sub` tolerates the millis clock wrapping.
fn frac(now: u32, t0: u32, dur: u32) -> f32 {
    now.wrapping_sub(t0) as f32 / dur as f32
}

/// Remap raw hold progress through the [`DEAD`] zone to a `0.0..=1.0` drawn depth:
/// `0` at the dead-zone exit, `1` at the threshold.
fn shown(progress: f32) -> f32 {
    ((progress - DEAD) / (1.0 - DEAD)).clamp(0.0, 1.0)
}

// per-control state

/// A hint's transient animation, layered on top of the live charge.
#[derive(Clone, Copy)]
enum Anim {
    /// No animation — drawing follows the live charge in [`Hint::prev`].
    Idle,
    /// The hold completed: a brief deeper lunge that began at `t0`.
    Pop { t0: u32 },
    /// The hold was released early: the bulge retracts from `from`, starting at `t0`.
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
            // Released past the dead zone but before the threshold → retract the bulge.
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
                // Fast lunge past the charge depth to the overshoot, then ease back
                // out to nothing — a snap inward rather than a symmetric pulse.
                let depth = if e < POP_ATTACK {
                    DEPTH + (POP_DEPTH - DEPTH) * (e / POP_ATTACK)
                } else {
                    POP_DEPTH * (1.0 - (e - POP_ATTACK) / (1.0 - POP_ATTACK))
                };
                bulge(cv, &place, depth);
            }
            Anim::Cancel { t0, from } => {
                let e = frac(now, t0, CANCEL_MS);
                if e >= 1.0 {
                    return;
                }
                bulge(cv, &place, DEPTH * from * (1.0 - e));
            }
            Anim::Idle => {
                if self.prev > DEAD {
                    bulge(cv, &place, DEPTH * shown(self.prev));
                }
            }
        }
    }
}

/// Draw the bulge: a black hump of the fixed [`BASE_HALF`] base width poking `depth`
/// px inward from the edge, its profile a quartic bump (flat tangent shoulders, soft
/// apex) rasterized as edge-perpendicular strips. Nothing when uncharged.
fn bulge<D, F>(cv: &mut Canvas<D, F>, place: &Place, depth: f32)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    if depth < 0.5 {
        return;
    }
    for i in -BASE_HALF..=BASE_HALF {
        let t = i as f32 / BASE_HALF as f32; // -1..1 across the base
        let s = 1.0 - t * t;
        let d = (depth * s * s + 0.5) as i32; // quartic: melts into the edge at ±1
        if d > 0 {
            cv.fill(place.strip(i, d), palette::HUD);
        }
    }
}

// the overlay

/// Per-control anchor. Relocating a bulge is a one-line change to one of the
/// [`ENCODER`] / [`BACK`] constants below.
struct Style {
    anchor: Anchor,
}

/// Encoder hint — upper-right edge (the encoder wheel sits near the top right).
const ENCODER: Style = Style { anchor: Anchor { edge: Edge::Right, pos: 0.30 } };

/// Back hint — lower-right edge (the Back button sits below the encoder).
const BACK: Style = Style { anchor: Anchor { edge: Edge::Right, pos: 0.70 } };

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
    /// its threshold (so the bulge pops the instant the action commits).
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
