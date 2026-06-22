//! Gesture recognition — the shared input layer.
//!
//! Turns raw [`InputEvent`]s (encoder detents + encoder/Back button edges) plus a
//! millis clock into the five UI [`Gesture`]s from the input model in
//! `docs/ui_framework_brief.md`. This is the brief's "one shared layer" with
//! long-press detection and hold-progress identical across host and MCU: the
//! simulator's knob/buttons and the firmware's GPIO produce the exact same
//! gestures. `no_std`, zero-alloc, and clock-agnostic — the host/MCU passes the
//! current time in, so there is no platform timer baked in.

use crate::hal::{Button, ButtonEvent, InputEvent};

/// Default long-press threshold (ms): how long the encoder or Back must be held
/// to read as `Hold`/`BackHold` rather than `Press`/`Back`. Spec open item #5
/// (tune later); 500 ms is a comfortable start.
pub const DEFAULT_HOLD_MS: u32 = 500;

/// The five UI gestures (input model in `docs/ui_framework_brief.md`), recognized
/// from raw [`InputEvent`]s + a millis clock by [`Gestures`]. A screen's `handle`
/// reacts to exactly these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Encoder rotated; `n` signed detents (positive = clockwise / "next").
    Turn(i32),
    /// Encoder short press (released before the hold threshold).
    Press,
    /// Encoder long press (held past the threshold).
    Hold,
    /// Back short press.
    Back,
    /// Back long press.
    BackHold,
}

/// Per-button hold timing.
#[derive(Debug, Clone, Copy, Default)]
struct Held {
    /// Millis the button went down, or `None` while it is up.
    since: Option<u32>,
    /// Whether the long-press already fired for the current press.
    fired_long: bool,
}

impl Held {
    /// Progress 0.0–1.0 toward the long-press threshold for an in-flight hold,
    /// or 0 when up. `now.wrapping_sub` tolerates the millis clock wrapping.
    fn progress(&self, now: u32, hold_ms: u32) -> f32 {
        match self.since {
            Some(t0) if hold_ms > 0 => (now.wrapping_sub(t0) as f32 / hold_ms as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
}

/// Shared gesture recognizer. Feed it raw [`InputEvent`]s as they arrive
/// ([`on_event`](Gestures::on_event)) and call [`tick`](Gestures::tick) once per
/// frame with the current millis; it emits [`Gesture`]s and exposes hold-progress
/// (0.0–1.0) for the guarded-action confirm ring.
///
/// `press` fires on *release before* the threshold; `hold` fires the instant the
/// threshold is crossed *while still held* (so the ring completes and the action
/// commits without waiting for release) — hence both an event hook and a per-frame
/// `tick`.
pub struct Gestures {
    hold_ms: u32,
    encoder: Held,
    back: Held,
}

impl Gestures {
    /// A recognizer with a custom long-press threshold (ms).
    pub fn new(hold_ms: u32) -> Self {
        Gestures { hold_ms, encoder: Held::default(), back: Held::default() }
    }

    /// A recognizer with the [`DEFAULT_HOLD_MS`] threshold.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_HOLD_MS)
    }

    /// Feed one raw event captured at time `now` (ms). Returns the gesture it
    /// completes, if any: `Turn` fires immediately; `Press`/`Back` fire on release
    /// before the threshold (a release *after* the long-press already fired yields
    /// nothing, since the `Hold`/`BackHold` was the gesture).
    pub fn on_event(&mut self, ev: InputEvent, now: u32) -> Option<Gesture> {
        match ev {
            InputEvent::Turn(0) => None,
            InputEvent::Turn(n) => Some(Gesture::Turn(n)),
            InputEvent::Button(ButtonEvent::Down(b)) => {
                let h = self.btn(b);
                h.since = Some(now);
                h.fired_long = false;
                None
            }
            InputEvent::Button(ButtonEvent::Up(b)) => {
                let h = self.btn(b);
                let fired = h.fired_long;
                *h = Held::default();
                if fired {
                    None
                } else {
                    Some(match b {
                        Button::Encoder => Gesture::Press,
                        Button::Back => Gesture::Back,
                    })
                }
            }
        }
    }

    /// Call once per frame with the current millis. Fires `Hold`/`BackHold` the
    /// instant a held button crosses the threshold. At most one gesture per call;
    /// if both buttons cross in the same frame, the other fires next frame.
    pub fn tick(&mut self, now: u32) -> Option<Gesture> {
        let hold_ms = self.hold_ms;
        for (b, long) in [(Button::Encoder, Gesture::Hold), (Button::Back, Gesture::BackHold)] {
            let h = self.btn(b);
            if let Some(t0) = h.since {
                if !h.fired_long && now.wrapping_sub(t0) >= hold_ms {
                    h.fired_long = true;
                    return Some(long);
                }
            }
        }
        None
    }

    /// Hold-progress (0.0–1.0) of an in-flight encoder long-press, for the confirm
    /// ring. 0 when not held or once the `Hold` has already fired.
    pub fn encoder_progress(&self, now: u32) -> f32 {
        if self.encoder.fired_long {
            0.0
        } else {
            self.encoder.progress(now, self.hold_ms)
        }
    }

    /// Hold-progress of an in-flight Back long-press.
    pub fn back_progress(&self, now: u32) -> f32 {
        if self.back.fired_long {
            0.0
        } else {
            self.back.progress(now, self.hold_ms)
        }
    }

    fn btn(&mut self, b: Button) -> &mut Held {
        match b {
            Button::Encoder => &mut self.encoder,
            Button::Back => &mut self.back,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hal::{Button, ButtonEvent, InputEvent};

    fn down(b: Button) -> InputEvent {
        InputEvent::Button(ButtonEvent::Down(b))
    }
    fn up(b: Button) -> InputEvent {
        InputEvent::Button(ButtonEvent::Up(b))
    }

    #[test]
    fn turn_emits_immediately_and_ignores_zero() {
        let mut g = Gestures::with_defaults();
        assert_eq!(g.on_event(InputEvent::Turn(2), 0), Some(Gesture::Turn(2)));
        assert_eq!(g.on_event(InputEvent::Turn(-1), 0), Some(Gesture::Turn(-1)));
        assert_eq!(g.on_event(InputEvent::Turn(0), 0), None);
    }

    #[test]
    fn short_press_fires_on_release() {
        let mut g = Gestures::new(500);
        assert_eq!(g.on_event(down(Button::Encoder), 0), None);
        assert_eq!(g.tick(100), None, "no long-press before the threshold");
        assert_eq!(g.on_event(up(Button::Encoder), 200), Some(Gesture::Press));
    }

    #[test]
    fn long_press_fires_at_threshold_then_release_is_silent() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Encoder), 0);
        assert_eq!(g.tick(499), None);
        assert_eq!(g.tick(500), Some(Gesture::Hold), "fires the instant it crosses");
        assert_eq!(g.tick(700), None, "only fires once");
        assert_eq!(g.on_event(up(Button::Encoder), 900), None, "release after a hold is silent");
    }

    #[test]
    fn back_button_maps_to_back_and_back_hold() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Back), 0);
        assert_eq!(g.on_event(up(Button::Back), 100), Some(Gesture::Back));
        g.on_event(down(Button::Back), 1000);
        assert_eq!(g.tick(1500), Some(Gesture::BackHold));
    }

    #[test]
    fn hold_progress_ramps_then_clears() {
        let mut g = Gestures::new(500);
        assert_eq!(g.encoder_progress(0), 0.0, "0 while up");
        g.on_event(down(Button::Encoder), 0);
        assert!((g.encoder_progress(250) - 0.5).abs() < 1e-6);
        assert_eq!(g.encoder_progress(600), 1.0, "clamps at the threshold");
        g.tick(600); // fires Hold
        assert_eq!(g.encoder_progress(700), 0.0, "clears once fired");
    }

    #[test]
    fn the_two_buttons_are_independent() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Encoder), 0);
        g.on_event(down(Button::Back), 100);
        // Encoder crosses first; Back is still mid-hold.
        assert_eq!(g.tick(500), Some(Gesture::Hold));
        assert!(g.back_progress(500) > 0.0 && g.back_progress(500) < 1.0);
        // Back crosses next.
        assert_eq!(g.tick(600), Some(Gesture::BackHold));
    }
}
