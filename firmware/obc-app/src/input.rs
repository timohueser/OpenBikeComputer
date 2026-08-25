//! Gesture recognition — the shared input layer.
//!
//! Turns raw four-button edges plus directly injected steps and a millis clock into UI gestures
//! and global drawer chords, identically across host and MCU. `no_std`, zero-alloc, and
//! clock-agnostic — the caller passes the current time in, so no platform timer is baked in.

use obc_ports::{Button, ButtonEvent, InputEvent};

/// Default long-press threshold (ms): how long Select or Back must be held to read as
/// `Hold`/`BackHold` rather than `Press`/`Back`.
pub const DEFAULT_HOLD_MS: u32 = 500;

/// Default short-press (tap) window (ms): a release within this counts as a `Press`/`Back`. A
/// release *after* it but *before* [`DEFAULT_HOLD_MS`] is a **cancelled long-press** — the rider
/// started a hold and let go early — and fires **nothing**, rather than surprising them with a tap.
/// So the three outcomes are: release ≤ tap → press; tap < release < hold → ignored; held ≥ hold →
/// long-press.
pub const DEFAULT_TAP_MS: u32 = 200;

/// Maximum separation between the two press edges of a drawer chord. Directional movement waits
/// this long before firing, unless released first, so a chord never leaks a navigation step.
pub const DEFAULT_CHORD_MS: u32 = 100;
/// Delay from a directional press to its first repeat, measured from the original press edge.
pub const DEFAULT_REPEAT_DELAY_MS: u32 = 350;
/// Directional auto-repeat interval after the first repeat.
pub const DEFAULT_REPEAT_INTERVAL_MS: u32 = 120;

/// Device-wide button combinations handled above the current screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrawerGesture {
    /// Up + Select: toggle the quick-settings drawer.
    Quick,
    /// Down + Back: toggle the contextual-action drawer.
    Context,
}

/// The five UI gestures, recognized from raw [`InputEvent`]s + a millis clock by [`Gestures`]. A
/// screen's `handle` reacts to exactly these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Up/Down pressed; `n` signed steps (negative = Up / "previous", positive = Down / "next").
    Step(i32),
    /// Select short press (released within the tap window).
    Press,
    /// Select long press (held past the threshold).
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

/// State for one directional button. Its first step is delayed just long enough to arbitrate a
/// drawer chord; a quick release fires immediately.
#[derive(Debug, Clone, Copy, Default)]
struct DirectionHeld {
    since: Option<u32>,
    fired_initial: bool,
    next_repeat: Option<u32>,
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

/// Shared gesture recognizer. Feed it raw [`InputEvent`]s ([`on_event`](Gestures::on_event)) and
/// call [`tick`](Gestures::tick) once per frame with the current millis; it emits [`Gesture`]s and
/// exposes hold-progress (0.0–1.0) for the guarded-action confirm ring.
///
/// `Hold` fires the instant the threshold is crossed *while still held* (so the ring completes and
/// the action commits without waiting for release) — hence both an event hook and a per-frame
/// `tick`.
pub struct Gestures {
    hold_ms: u32,
    /// Max press duration that still counts as a tap (see [`DEFAULT_TAP_MS`]); a release between this
    /// and `hold_ms` is a cancelled long-press and fires nothing. Clamped to `hold_ms` so a tiny
    /// custom hold threshold can't invert the window.
    tap_ms: u32,
    select: Held,
    back: Held,
    up: DirectionHeld,
    down: DirectionHeld,
    chord: Option<DrawerGesture>,
    pending_drawer: Option<DrawerGesture>,
}

impl Gestures {
    /// A recognizer with a custom long-press threshold (ms) and the [`DEFAULT_TAP_MS`] tap window.
    pub fn new(hold_ms: u32) -> Self {
        Gestures {
            hold_ms,
            tap_ms: DEFAULT_TAP_MS.min(hold_ms),
            select: Held::default(),
            back: Held::default(),
            up: DirectionHeld::default(),
            down: DirectionHeld::default(),
            chord: None,
            pending_drawer: None,
        }
    }

    /// A recognizer with the [`DEFAULT_HOLD_MS`] threshold.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_HOLD_MS)
    }

    /// Feed one raw event captured at time `now` (ms). Returns the gesture it completes, if any:
    /// Directly injected `Step` fires immediately; Up/Down edges are chord-arbitrated.
    /// `Press`/`Back` fire on a release **within the tap window**
    /// ([`tap_ms`](Self::tap_ms)). A release after the tap window (cancelled long-press) or after
    /// the long-press already fired yields nothing.
    pub fn on_event(&mut self, ev: InputEvent, now: u32) -> Option<Gesture> {
        match ev {
            InputEvent::Step(0) => None,
            InputEvent::Step(n) => Some(Gesture::Step(n)),
            InputEvent::Button(ButtonEvent::Down(b)) => {
                match b {
                    Button::Up | Button::Down => {
                        let h = self.direction(b);
                        if h.since.is_none() {
                            *h = DirectionHeld { since: Some(now), fired_initial: false, next_repeat: None };
                        }
                    }
                    Button::Select | Button::Back => {
                        let h = self.action(b);
                        if h.since.is_none() {
                            h.since = Some(now);
                            h.fired_long = false;
                        }
                    }
                }
                self.try_chord(now);
                None
            }
            InputEvent::Button(ButtonEvent::Up(b)) => {
                if self.release_chord_part(b) {
                    return None;
                }
                if matches!(b, Button::Up | Button::Down) {
                    let dir = direction_step(b);
                    let h = self.direction(b);
                    let fire = h.since.is_some() && !h.fired_initial;
                    *h = DirectionHeld::default();
                    return fire.then_some(Gesture::Step(dir));
                }
                let tap_ms = self.tap_ms;
                let h = self.action(b);
                let since = h.since;
                let fired = h.fired_long;
                *h = Held::default();
                // A tap only registers if released within the tap window. A longer-but-not-held-enough
                // release is a cancelled long-press → nothing (neither tap nor hold).
                let is_tap = !fired && matches!(since, Some(t0) if now.wrapping_sub(t0) <= tap_ms);
                is_tap.then_some(match b {
                    Button::Select => Gesture::Press,
                    Button::Back => Gesture::Back,
                    Button::Up | Button::Down => unreachable!(),
                })
            }
        }
    }

    /// Take the global drawer chord recognized by the most recent event drain, if any.
    pub fn take_drawer(&mut self) -> Option<DrawerGesture> {
        self.pending_drawer.take()
    }

    /// Call once per frame with the current millis. Fires `Hold`/`BackHold` the
    /// instant a held button crosses the threshold. At most one gesture per call;
    /// if both buttons cross in the same frame, the other fires next frame.
    pub fn tick(&mut self, now: u32) -> Option<Gesture> {
        for b in [Button::Up, Button::Down] {
            if self.chord_uses(b) {
                continue;
            }
            let h = self.direction(b);
            let Some(t0) = h.since else { continue };
            let elapsed = now.wrapping_sub(t0);
            if !h.fired_initial && elapsed >= DEFAULT_CHORD_MS {
                h.fired_initial = true;
                h.next_repeat = Some(t0.wrapping_add(DEFAULT_REPEAT_DELAY_MS));
                return Some(Gesture::Step(direction_step(b)));
            }
            if h.fired_initial && h.next_repeat.is_some_and(|due| now.wrapping_sub(due) < u32::MAX / 2) {
                h.next_repeat = Some(now.wrapping_add(DEFAULT_REPEAT_INTERVAL_MS));
                return Some(Gesture::Step(direction_step(b)));
            }
        }

        let hold_ms = self.hold_ms;
        for (b, long) in [(Button::Select, Gesture::Hold), (Button::Back, Gesture::BackHold)] {
            if self.chord_uses(b) {
                continue;
            }
            let h = self.action(b);
            if let Some(t0) = h.since {
                if !h.fired_long && now.wrapping_sub(t0) >= hold_ms {
                    h.fired_long = true;
                    return Some(long);
                }
            }
        }
        None
    }

    /// Cancel any in-flight press on either button: mark it already-fired, so the pending
    /// `Hold`/`BackHold` never emits, the eventual release is silent (no surprise tap), and the
    /// hold-progress reads 0 (the bulge retracts). A fresh press recognises normally.
    ///
    /// Called when a gesture-driven screen **transition** changes what is under the rider's
    /// finger: a long-press that started charging over one screen must not complete onto
    /// whatever replaced it — e.g. a hold aimed at a popup's "Save & new" landing on the Route
    /// menu's hold-to-**delete** footer after a Back tap dismissed the popup (issue #480).
    pub fn cancel_holds(&mut self) {
        for h in [&mut self.select, &mut self.back] {
            if h.since.is_some() {
                h.fired_long = true;
            }
        }
    }

    /// Hold-progress (0.0–1.0) of an in-flight Select long-press, for the confirm
    /// ring. 0 when not held or once the `Hold` has already fired.
    pub fn select_progress(&self, now: u32) -> f32 {
        if self.select.fired_long {
            0.0
        } else {
            self.select.progress(now, self.hold_ms)
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

    fn action(&mut self, b: Button) -> &mut Held {
        match b {
            Button::Select => &mut self.select,
            Button::Back => &mut self.back,
            Button::Up | Button::Down => unreachable!(),
        }
    }

    fn direction(&mut self, b: Button) -> &mut DirectionHeld {
        match b {
            Button::Up => &mut self.up,
            Button::Down => &mut self.down,
            Button::Select | Button::Back => unreachable!(),
        }
    }

    fn try_chord(&mut self, now: u32) {
        if self.chord.is_some() {
            return;
        }
        let candidate = [
            (DrawerGesture::Quick, self.up.since, self.up.fired_initial, self.select.since, self.select.fired_long),
            (DrawerGesture::Context, self.down.since, self.down.fired_initial, self.back.since, self.back.fired_long),
        ]
        .into_iter()
        .find(|(_, direction, moved, action, held)| {
            !*moved
                && !*held
                && direction.is_some_and(|t| now.wrapping_sub(t) <= DEFAULT_CHORD_MS)
                && action.is_some_and(|t| now.wrapping_sub(t) <= DEFAULT_CHORD_MS)
        })
        .map(|(drawer, ..)| drawer);

        if let Some(drawer) = candidate {
            self.chord = Some(drawer);
            self.pending_drawer = Some(drawer);
            match drawer {
                DrawerGesture::Quick => {
                    self.up.fired_initial = true;
                    self.up.next_repeat = None;
                    self.select.fired_long = true;
                }
                DrawerGesture::Context => {
                    self.down.fired_initial = true;
                    self.down.next_repeat = None;
                    self.back.fired_long = true;
                }
            }
        }
    }

    fn chord_uses(&self, b: Button) -> bool {
        matches!(
            (self.chord, b),
            (Some(DrawerGesture::Quick), Button::Up | Button::Select)
                | (Some(DrawerGesture::Context), Button::Down | Button::Back)
        )
    }

    fn release_chord_part(&mut self, b: Button) -> bool {
        if !self.chord_uses(b) {
            return false;
        }
        match b {
            Button::Up | Button::Down => *self.direction(b) = DirectionHeld::default(),
            Button::Select | Button::Back => *self.action(b) = Held::default(),
        }
        let released = match self.chord {
            Some(DrawerGesture::Quick) => self.up.since.is_none() && self.select.since.is_none(),
            Some(DrawerGesture::Context) => self.down.since.is_none() && self.back.since.is_none(),
            None => false,
        };
        if released {
            self.chord = None;
        }
        true
    }
}

fn direction_step(b: Button) -> i32 {
    match b {
        Button::Up => -1,
        Button::Down => 1,
        Button::Select | Button::Back => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_ports::{Button, ButtonEvent, InputEvent};

    fn down(b: Button) -> InputEvent {
        InputEvent::Button(ButtonEvent::Down(b))
    }
    fn up(b: Button) -> InputEvent {
        InputEvent::Button(ButtonEvent::Up(b))
    }

    #[test]
    fn step_emits_immediately_and_ignores_zero() {
        let mut g = Gestures::with_defaults();
        assert_eq!(g.on_event(InputEvent::Step(2), 0), Some(Gesture::Step(2)));
        assert_eq!(g.on_event(InputEvent::Step(-1), 0), Some(Gesture::Step(-1)));
        assert_eq!(g.on_event(InputEvent::Step(0), 0), None);
    }

    #[test]
    fn short_press_fires_on_release() {
        let mut g = Gestures::new(500);
        assert_eq!(g.on_event(down(Button::Select), 0), None);
        assert_eq!(g.tick(100), None, "no long-press before the threshold");
        assert_eq!(g.on_event(up(Button::Select), 150), Some(Gesture::Press), "a quick release taps");
    }

    /// A press released *after* the tap window but *before* the hold threshold is a cancelled
    /// long-press — the rider started to hold and let go early — and must fire **nothing** (not a
    /// surprise tap). Covers both Select and Back.
    #[test]
    fn cancelled_long_press_fires_nothing() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        assert!(g.select_progress(300) > 0.0, "the hold was visibly in progress");
        assert_eq!(g.tick(300), None, "no long-press yet");
        assert_eq!(g.on_event(up(Button::Select), 300), None, "a release in the dead zone fires nothing");

        // Same for Back: started to BackHold, let go at 350 ms → nothing.
        g.on_event(down(Button::Back), 1000);
        assert_eq!(g.on_event(up(Button::Back), 1350), None, "a cancelled Back-hold fires nothing");
    }

    /// The tap window is inclusive at [`DEFAULT_TAP_MS`]: a release at exactly the window taps, one
    /// millisecond past it is a cancelled long-press.
    #[test]
    fn tap_window_boundary() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        assert_eq!(g.on_event(up(Button::Select), DEFAULT_TAP_MS), Some(Gesture::Press), "exactly at the window taps");
        g.on_event(down(Button::Select), 1000);
        assert_eq!(g.on_event(up(Button::Select), 1000 + DEFAULT_TAP_MS + 1), None, "one ms past is cancelled");
    }

    #[test]
    fn long_press_fires_at_threshold_then_release_is_silent() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        assert_eq!(g.tick(499), None);
        assert_eq!(g.tick(500), Some(Gesture::Hold), "fires the instant it crosses");
        assert_eq!(g.tick(700), None, "only fires once");
        assert_eq!(g.on_event(up(Button::Select), 900), None, "release after a hold is silent");
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
        assert_eq!(g.select_progress(0), 0.0, "0 while up");
        g.on_event(down(Button::Select), 0);
        assert!((g.select_progress(250) - 0.5).abs() < 1e-6);
        assert_eq!(g.select_progress(600), 1.0, "clamps at the threshold");
        g.tick(600); // fires Hold
        assert_eq!(g.select_progress(700), 0.0, "clears once fired");
    }

    #[test]
    fn the_two_buttons_are_independent() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        g.on_event(down(Button::Back), 100);
        // Select crosses first; Back is still mid-hold.
        assert_eq!(g.tick(500), Some(Gesture::Hold));
        assert!(g.back_progress(500) > 0.0 && g.back_progress(500) < 1.0);
        // Back crosses next.
        assert_eq!(g.tick(600), Some(Gesture::BackHold));
    }

    /// A cancelled in-flight hold fires nothing — not the pending long-press, not a tap on
    /// release, no progress — while a fresh press afterwards recognises normally.
    #[test]
    fn cancel_holds_suppresses_the_pending_long_press_and_the_release() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        assert!(g.select_progress(300) > 0.0, "charging before the cancel");
        g.cancel_holds();
        assert_eq!(g.select_progress(300), 0.0, "a cancelled hold reads no progress");
        assert_eq!(g.tick(600), None, "the long-press never fires");
        assert_eq!(g.on_event(up(Button::Select), 650), None, "the release is silent, not a tap");
        // A fresh press afterwards is a clean slate.
        g.on_event(down(Button::Select), 1_000);
        assert_eq!(g.tick(1_500), Some(Gesture::Hold), "the next hold recognises normally");
        // Cancelling with nothing in flight is a no-op.
        g.cancel_holds();
        g.on_event(down(Button::Back), 2_000);
        assert_eq!(g.on_event(up(Button::Back), 2_100), Some(Gesture::Back), "an idle cancel affects nothing");
    }

    /// `tick` emits **at most one** gesture per call, so when both buttons cross the threshold in
    /// the same frame Select's `Hold` fires now and the `BackHold` on the next `tick` — the
    /// second long-press is deferred, not dropped.
    #[test]
    fn both_holds_crossing_one_frame_fire_one_then_the_other() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        g.on_event(down(Button::Back), 0); // pressed in the very same frame
                                           // Both are past 500 ms at t=500: only Select's Hold comes out this call.
        assert_eq!(g.tick(500), Some(Gesture::Hold), "the Select long-press wins the shared frame");
        // The Back long-press wasn't lost — it fires on the next tick, even with no new input.
        assert_eq!(g.tick(500), Some(Gesture::BackHold), "the other hold fires next frame, not dropped");
        assert_eq!(g.tick(500), None, "and each long-press fires exactly once");
    }

    #[test]
    fn directional_tap_steps_on_release_and_hold_repeats() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Up), 0);
        assert_eq!(g.on_event(up(Button::Up), 50), Some(Gesture::Step(-1)), "a quick tap has no grace-window lag");

        g.on_event(down(Button::Down), 1_000);
        assert_eq!(g.tick(1_099), None, "movement waits while a chord is still possible");
        assert_eq!(g.tick(1_100), Some(Gesture::Step(1)));
        assert_eq!(g.tick(1_349), None);
        assert_eq!(g.tick(1_350), Some(Gesture::Step(1)), "repeat keeps the established 350 ms cadence");
        assert_eq!(g.tick(1_470), Some(Gesture::Step(1)));
        assert_eq!(g.on_event(up(Button::Down), 1_500), None, "release after movement is silent");
    }

    #[test]
    fn quick_drawer_chord_suppresses_both_constituent_actions() {
        let mut g = Gestures::with_defaults();
        assert_eq!(g.on_event(down(Button::Up), 0), None);
        assert_eq!(g.on_event(down(Button::Select), 60), None);
        assert_eq!(g.take_drawer(), Some(DrawerGesture::Quick));
        assert_eq!(g.tick(600), None, "neither a step nor Select hold leaks through");
        assert_eq!(g.on_event(up(Button::Up), 610), None);
        assert_eq!(g.on_event(up(Button::Select), 620), None, "the last release is not a tap");

        g.on_event(down(Button::Up), 700);
        assert_eq!(g.on_event(up(Button::Up), 750), Some(Gesture::Step(-1)), "the latch clears after both releases");
    }

    #[test]
    fn context_drawer_chord_works_in_reverse_press_order() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Back), 10);
        g.on_event(down(Button::Down), 80);
        assert_eq!(g.take_drawer(), Some(DrawerGesture::Context));
        assert_eq!(g.on_event(up(Button::Back), 100), None);
        assert_eq!(g.on_event(up(Button::Down), 120), None);
    }

    #[test]
    fn presses_outside_the_chord_window_remain_independent() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Select), 0);
        g.on_event(down(Button::Up), DEFAULT_CHORD_MS + 1);
        assert_eq!(g.take_drawer(), None);
        assert_eq!(g.on_event(up(Button::Up), 150), Some(Gesture::Step(-1)));
        assert_eq!(g.on_event(up(Button::Select), 180), Some(Gesture::Press));
    }

    #[test]
    fn directional_repeat_survives_millis_wrap_and_rebases_after_a_stall() {
        let mut g = Gestures::with_defaults();
        let t0 = u32::MAX - 50;
        g.on_event(down(Button::Down), t0);
        assert_eq!(g.tick(t0.wrapping_add(DEFAULT_CHORD_MS)), Some(Gesture::Step(1)));
        assert_eq!(g.tick(10_000), Some(Gesture::Step(1)), "one catch-up repeat across wrap/stall");
        assert_eq!(g.tick(10_119), None, "the next repeat is rebased to the resumed tick");
        assert_eq!(g.tick(10_120), Some(Gesture::Step(1)));
    }
}
