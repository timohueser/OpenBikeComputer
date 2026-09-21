//! The global long-press hint: an on-screen frame bulge that surfaces the device's central hold
//! gestures. [`App`](crate::app::App) folds each frame's hold-progress into [`HoldHints`] and draws
//! it on top of the screen stack.
//!
//! Per control a black hump swells inward from the screen edge nearest its physical button, so it
//! reads as the black bezel bulging into the display. The hump has a fixed base width along the
//! edge and only its inward depth tracks the hold. A completed hold pops with a quick deeper lunge;
//! an early release retracts it.
//!
//! It is drawn in [`palette::HUD`], so it needs no alpha on the real 8-colour panel. Select and
//! Back are told apart by position, not colour, so relocating a bulge is a one-line edit to its
//! [`Style::anchor`].

use embedded_graphics::{draw_target::DrawTarget, primitives::Rectangle};
use obc_render::{rect, Canvas, Surface};

use crate::screen::palette;

/// Pop animation duration (ms).
const POP_MS: u32 = 220;
/// Fraction of the pop spent lunging in to the pop depth before it eases back out —
/// a fast attack, slow release, so the confirm reads as a snap.
const POP_ATTACK: f32 = 0.22;
/// Retract animation duration (ms) when a hold is released early.
const CANCEL_MS: u32 = 150;
/// Dead zone: the charge fraction a hold must pass before any bulge is drawn, so a
/// quick tap stays completely clean (a short press shouldn't flicker a bulge). The
/// drawn depth is remapped ([`shown`]) so the bulge emerges flat right at this point
/// and reaches full depth at the threshold. `0.30` ≈ 150 ms of the 500 ms hold.
const DEAD: f32 = 0.30;

/// A bulge's `0.0..=1.0` position down the screen edge.
#[derive(Clone, Copy)]
struct Anchor {
    pos: f32,
    left: bool,
}

impl Anchor {
    /// Resolve to a concrete [`Place`] for a `w`×`h` screen, clamping the centre so
    /// the `base_half`-wide base stays on-panel along the edge.
    fn place(self, w: i32, h: i32, base_half: i32) -> Place {
        let lo = base_half + 1;
        let hi = (h - 1 - lo).max(lo);
        let cc = (self.pos * (h - 1) as f32 + 0.5) as i32;
        Place { outer: if self.left { 0 } else { w }, cc: cc.clamp(lo, hi) }
    }
}

/// A resolved placement: the edge coordinate and the centre along it.
struct Place {
    outer: i32,
    cc: i32,
}

impl Place {
    /// One rasterizer strip: a 1px-wide slice `along_off` from the centre that pokes `depth` px
    /// inward from the edge. The bulge is the union of these across the fixed base width.
    fn strip(&self, along_off: i32, depth: i32) -> Rectangle {
        let x = if self.outer == 0 { 0 } else { self.outer - depth };
        rect(x, self.cc + along_off, depth, 1)
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

    /// Whether this hint has something to draw at `now`. It mirrors the [`draw`](Hint::draw)
    /// decision exactly, so a host repaints the overlay only when it would change a pixel.
    fn active(&self, now: u32) -> bool {
        match self.anim {
            Anim::Pop { t0 } => frac(now, t0, POP_MS) < 1.0,
            Anim::Cancel { t0, .. } => frac(now, t0, CANCEL_MS) < 1.0,
            Anim::Idle => self.prev > DEAD,
        }
    }

    /// Draw the hint for this control, or nothing when idle and uncharged.
    fn draw(&self, cv: &mut impl Surface, style: &Style, now: u32, w: i32, h: i32) {
        let (base_half, flat_half) = (style.base_half, style.flat_half);
        let (charge_depth, pop_depth) = (style.depth, style.pop_depth);
        let place = style.anchor.place(w, h, base_half);
        let mut draw = |depth| bulge(cv, &place, depth, base_half, flat_half);
        match self.anim {
            Anim::Pop { t0 } => {
                let e = frac(now, t0, POP_MS);
                if e >= 1.0 {
                    return;
                }
                // A fast lunge past the charge depth, then an ease back out: a snap inward rather
                // than a symmetric pulse.
                let depth = if e < POP_ATTACK {
                    charge_depth + (pop_depth - charge_depth) * (e / POP_ATTACK)
                } else {
                    pop_depth * (1.0 - (e - POP_ATTACK) / (1.0 - POP_ATTACK))
                };
                draw(depth);
            }
            Anim::Cancel { t0, from } => {
                let e = frac(now, t0, CANCEL_MS);
                if e >= 1.0 {
                    return;
                }
                draw(charge_depth * from * (1.0 - e));
            }
            Anim::Idle => {
                if self.prev > DEAD {
                    draw(charge_depth * shown(self.prev));
                }
            }
        }
    }
}

/// Profile height (`0.0..=1.0`) at along-offset `i`: a flat shelf at full height
/// within `±flat_half`, then a quartic shoulder easing to `0` at `±base_half` (flat
/// tangent at both the shelf and the edge, so it melts into each).
fn top_profile(i: i32, base_half: i32, flat_half: i32) -> f32 {
    let a = i.abs();
    if a <= flat_half {
        return 1.0;
    }
    let u = (a - flat_half) as f32 / (base_half - flat_half).max(1) as f32; // 0..1 shoulder
    let s = 1.0 - u * u;
    s * s
}

/// Draw the bulge: a black hump of `base_half` base width with a `flat_half`-wide
/// flat top, poking `depth` px inward from the edge, rasterized as edge-perpendicular
/// strips (see [`top_profile`]). Nothing when uncharged.
fn bulge(cv: &mut impl Surface, place: &Place, depth: f32, base_half: i32, flat_half: i32) {
    if depth < 0.5 {
        return;
    }
    for i in -base_half..=base_half {
        let d = (depth * top_profile(i, base_half, flat_half) + 0.5) as i32;
        if d > 0 {
            cv.fill(place.strip(i, d), palette::HUD);
        }
    }
}

/// Per-control look: where the bulge sits and its size along the edge.
#[derive(Clone, Copy)]
struct Style {
    anchor: Anchor,
    /// Half the base width (px) along the edge, so size along the edge and inward depth are
    /// independent.
    base_half: i32,
    /// Half the flat *top* width (px): strips within `±flat_half` sit at full depth (a flat shelf),
    /// and the quartic shoulder eases to zero across the remaining `base_half - flat_half` per side.
    /// `0` is a pure quartic bump; keep it `< base_half` to leave room for the shoulders.
    flat_half: i32,
    /// Inward depth (px) the bulge reaches at a full charge, just before the threshold.
    depth: f32,
    /// Peak inward depth (px) at the confirm "pop" — a brief deeper lunge past `depth`.
    pop_depth: f32,
}

/// Equal-sized hints mirrored about the display centre.
const SELECT: Style =
    Style { anchor: Anchor { pos: 0.35, left: false }, base_half: 44, flat_half: 15, depth: 7.0, pop_depth: 12.0 };
const BACK: Style = Style { anchor: Anchor { pos: 0.65, left: false }, ..SELECT };
const UP: Style = Style { anchor: Anchor { left: true, ..SELECT.anchor }, ..SELECT };

/// The global long-press overlay. The Assistant chord shares one hint at Up and Select.
pub struct HoldHints {
    select: Hint,
    back: Hint,
    /// Mirror Select at Up throughout the chord charge, pop, and retract.
    assistant: bool,
}

impl HoldHints {
    pub const fn new() -> Self {
        HoldHints { select: Hint::new(), back: Hint::new(), assistant: false }
    }

    /// Advance each hold from its live fraction and threshold-crossing flag.
    pub fn update(&mut self, now: u32, sel: (f32, bool), back: (f32, bool), assistant: (f32, bool)) {
        let chord = assistant.0 > 0.0 || assistant.1;
        if chord || sel.0 > 0.0 || sel.1 {
            self.assistant = chord;
        }
        let (progress, fired) = if chord { assistant } else { sel };
        self.select.update(now, progress, fired);
        self.back.update(now, back.0, back.1);
    }

    /// Whether either hint has live content at `now`. `false` exactly when
    /// [`draw`](HoldHints::draw) would paint nothing, so a host can leave the overlay untouched.
    pub fn active(&self, now: u32) -> bool {
        self.select.active(now) || self.back.active(now)
    }

    /// The bounding rows `[y0, y0 + rows)` of every hint live at `now`: the dirty region a
    /// partial-overlay host re-presents. `None` exactly when [`active`](HoldHints::active) is false.
    pub fn active_rows(&self, now: u32, w: i32, h: i32) -> Option<(u16, u16)> {
        let span = |hint: &Hint, style: &Style| -> Option<(i32, i32)> {
            if !hint.active(now) {
                return None;
            }
            let place = style.anchor.place(w, h, style.base_half);
            Some((place.cc - style.base_half, place.cc + style.base_half))
        };
        let merged = match (span(&self.select, &SELECT), span(&self.back, &BACK)) {
            (Some((a0, a1)), Some((b0, b1))) => Some((a0.min(b0), a1.max(b1))),
            (Some(s), None) | (None, Some(s)) => Some(s),
            (None, None) => None,
        };
        merged.map(|(lo, hi)| {
            let lo = lo.clamp(0, h);
            let hi = hi.clamp(0, h);
            (lo as u16, (hi - lo).max(0) as u16)
        })
    }

    /// Draw the active hints above the current screen, into a `w`×`h` target.
    pub fn draw<D, F>(&self, target: &mut D, color_fn: &F, w: i32, h: i32, now: u32)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let mut cv = Canvas::new(target, color_fn);
        self.select.draw(&mut cv, &SELECT, now, w, h);
        if self.assistant {
            self.select.draw(&mut cv, &UP, now, w, h);
        }
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
        h.update(150 + POP_MS, 0.0, false);
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
    fn active_tracks_the_whole_charge_pop_retract_lifecycle() {
        let mut h = Hint::new();
        h.update(0, 0.0, false);
        assert!(!h.active(0), "an idle, uncharged hint draws nothing");

        h.update(10, DEAD - 0.01, false);
        assert!(!h.active(10), "a charge inside the dead zone stays quiet");

        h.update(20, DEAD + 0.01, false);
        assert!(h.active(20), "charging past the dead zone shows a bulge");

        h.update(30, 0.0, true);
        assert!(h.active(30), "the confirm pop is live the frame it fires");
        assert!(h.active(30 + POP_MS - 1), "still live mid-pop");
        assert!(!h.active(30 + POP_MS), "quiet once the pop has run its course");
    }

    #[test]
    fn active_spans_an_early_release_retract() {
        let mut h = Hint::new();
        h.update(0, 0.4, false); // charging past the dead zone
        assert!(h.active(0));
        h.update(50, 0.0, false); // released early → retract begins
        assert!(h.active(50), "the retract is live");
        assert!(h.active(50 + CANCEL_MS - 1), "still live mid-retract");
        assert!(!h.active(50 + CANCEL_MS), "quiet once the retract finishes");
    }

    #[test]
    fn firing_beats_retracting_on_the_crossing_frame() {
        let mut h = Hint::new();
        h.update(0, 0.99, false);
        h.update(10, 0.0, true); // threshold crossed and progress cleared same frame
        assert!(is_pop(&h), "crossing the threshold is a pop, never a retract");
    }
}
