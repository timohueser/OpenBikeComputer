//! Gesture recognition — the shared input layer.
//!
//! Turns raw [`InputEvent`]s — edges from all four buttons, plus directly injected steps — and a
//! millis clock into the five UI [`Gesture`]s, identically across host and MCU. `no_std`,
//! zero-alloc, and clock-agnostic — the caller passes the current time in, so no platform timer is
//! baked in.
//!
//! Every timing the rider feels lives here: the long-press threshold, the tap window, and the
//! Up/Down step cadence (first step on the press edge, then auto-repeat while held). A board
//! contributes debounced edges and nothing else, so hardware and hosts cannot drift apart.

use obc_ports::{Button, ButtonEvent, InputEvent};

/// Default long-press threshold (ms): how long Select or Back must be held to read as
/// `Hold`/`BackHold` rather than `Press`/`Back`.
pub const DEFAULT_HOLD_MS: u32 = 500;

/// Delay from an Up/Down press to its first auto-repeat step (ms) — long enough that a single tap
/// never double-fires, short enough to feel responsive on a hold.
const REPEAT_DELAY_MS: u32 = 350;
/// Interval between auto-repeat steps while Up/Down stays held (ms) — ~8 steps/s.
const REPEAT_INTERVAL_MS: u32 = 120;

/// Default short-press (tap) window (ms): a release within this counts as a `Press`/`Back`. A
/// release *after* it but *before* [`DEFAULT_HOLD_MS`] is a **cancelled long-press** — the rider
/// started a hold and let go early — and fires **nothing**, rather than surprising them with a tap.
/// So the three outcomes are: release ≤ tap → press; tap < release < hold → ignored; held ≥ hold →
/// long-press.
pub const DEFAULT_TAP_MS: u32 = 200;

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

/// Auto-repeat state for the Up/Down pair: a due-time each plus one "still down" bit each, in
/// 12 B. Indexed by [`step_axis`] — `0` = Up, `1` = Down.
///
/// The shape is chosen for size, and only for size. `Gestures` is owned by the `InputPlane` inside
/// `App`, whose target-side `size_of` the board's resource baseline pins, so a byte here is a
/// resident byte on the part; the natural `Option<u32>` per direction reads better and costs 16.
#[derive(Debug, Clone, Copy, Default)]
struct Steps {
    /// Millis the next repeat step is due. Meaningful only while the matching `held` bit is set.
    due: [u32; 2],
    /// Bit 0 = Up is down, bit 1 = Down is down.
    held: u8,
}

impl Steps {
    /// A press edge: latch the direction down and schedule its first repeat.
    fn arm(&mut self, axis: usize, due: u32) {
        self.held |= 1 << axis;
        self.due[axis] = due;
    }

    /// A release edge: the direction stops repeating.
    fn disarm(&mut self, axis: usize) {
        self.held &= !(1 << axis);
    }

    /// The signed step of the first direction whose repeat has fallen due at `now`, rebasing that
    /// direction to `now + `[`REPEAT_INTERVAL_MS`] — so a stalled loop emits exactly **one**
    /// catch-up step rather than one per missed interval.
    fn due_step(&mut self, now: u32) -> Option<i32> {
        for (axis, dir) in [(0usize, -1i32), (1, 1)] {
            // Wrap-tolerant "due reached": `now` ∈ [due, due + 2^31). A plain `now >= due` would
            // stop repeating forever after the u32-millis rollover (~49.7 days).
            if self.held & (1 << axis) != 0 && now.wrapping_sub(self.due[axis]) < u32::MAX / 2 {
                self.due[axis] = now.wrapping_add(REPEAT_INTERVAL_MS);
                return Some(dir);
            }
        }
        None
    }
}

// The size claim above, held. No pointers or `usize`s inside, so it reads the same on host and
// target: `Gestures` is 44 B and `size_of::<App>()` carries 8 of the 12 (padding absorbs the rest).
const _: () = assert!(core::mem::size_of::<Steps>() == 12);

/// The [`Steps`] axis and step sign of a direction button — Up is "previous" (−1), Down is "next"
/// (+1) — or `None` for the two timed buttons, which have no step.
fn step_axis(b: Button) -> Option<(usize, i32)> {
    match b {
        Button::Up => Some((0, -1)),
        Button::Down => Some((1, 1)),
        Button::Select | Button::Back => None,
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
    /// Up/Down auto-repeat — the cadence the board's `ButtonInput` used to synthesize itself.
    steps: Steps,
}

impl Gestures {
    /// A recognizer with a custom long-press threshold (ms) and the [`DEFAULT_TAP_MS`] tap window.
    pub fn new(hold_ms: u32) -> Self {
        Gestures {
            hold_ms,
            tap_ms: DEFAULT_TAP_MS.min(hold_ms),
            select: Held::default(),
            back: Held::default(),
            steps: Steps::default(),
        }
    }

    /// A recognizer with the [`DEFAULT_HOLD_MS`] threshold.
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_HOLD_MS)
    }

    /// Feed one raw event captured at time `now` (ms). Returns the gesture it completes, if any: an
    /// Up/Down press and an injected `Step` fire immediately; `Press`/`Back` fire on a release
    /// **within the tap window** ([`tap_ms`](Self::tap_ms)). A release after the tap window
    /// (cancelled long-press) or after the long-press already fired yields nothing.
    pub fn on_event(&mut self, ev: InputEvent, now: u32) -> Option<Gesture> {
        match ev {
            InputEvent::Step(0) => None,
            InputEvent::Step(n) => Some(Gesture::Step(n)),
            InputEvent::Button(ButtonEvent::Down(b)) => self.press(b, now),
            InputEvent::Button(ButtonEvent::Up(b)) => self.release(b, now),
        }
    }

    /// A press edge. Up/Down step **on the press** and arm auto-repeat; Select/Back only start
    /// their hold clock — what they mean is settled on release or at the long-press threshold.
    fn press(&mut self, b: Button, now: u32) -> Option<Gesture> {
        if let Some((axis, dir)) = step_axis(b) {
            self.steps.arm(axis, now.wrapping_add(REPEAT_DELAY_MS));
            return Some(Gesture::Step(dir));
        }
        let (h, _) = self.timed(b)?;
        h.since = Some(now);
        h.fired_long = false;
        None
    }

    /// A release edge. Up/Down disarm auto-repeat and are silent — their step already fired on the
    /// press. Select/Back tap only when released inside the tap window.
    fn release(&mut self, b: Button, now: u32) -> Option<Gesture> {
        if let Some((axis, _)) = step_axis(b) {
            self.steps.disarm(axis);
            return None;
        }
        let tap_ms = self.tap_ms;
        let (h, tap) = self.timed(b)?;
        let since = h.since;
        let fired = h.fired_long;
        *h = Held::default();
        // A tap only registers if released within the tap window. A longer-but-not-held-enough
        // release is a cancelled long-press → nothing (neither tap nor hold).
        let is_tap = !fired && matches!(since, Some(t0) if now.wrapping_sub(t0) <= tap_ms);
        is_tap.then_some(tap)
    }

    /// Call once per frame with the current millis. Fires `Hold`/`BackHold` the instant a held
    /// button crosses the threshold, and one auto-repeat `Step` each time a held Up/Down falls due.
    /// At most one gesture per call; anything else due fires on the next call.
    pub fn tick(&mut self, now: u32) -> Option<Gesture> {
        let hold_ms = self.hold_ms;
        for (b, long) in [(Button::Select, Gesture::Hold), (Button::Back, Gesture::BackHold)] {
            let Some((h, _)) = self.timed(b) else { continue };
            if let Some(t0) = h.since {
                if !h.fired_long && now.wrapping_sub(t0) >= hold_ms {
                    h.fired_long = true;
                    return Some(long);
                }
            }
        }
        self.steps.due_step(now).map(Gesture::Step)
    }

    /// Cancel any in-flight press on Select or Back: mark it already-fired, so the pending
    /// `Hold`/`BackHold` never emits, the eventual release is silent (no surprise tap), and the
    /// hold-progress reads 0 (the bulge retracts). A fresh press recognises normally. Up/Down
    /// auto-repeat is untouched — a step charges nothing and lands on the new screen just as a
    /// fresh press would.
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

    /// The hold state of a timed button and the tap gesture it completes; `None` for the Up/Down
    /// pair, which has neither a hold nor a tap.
    fn timed(&mut self, b: Button) -> Option<(&mut Held, Gesture)> {
        match b {
            Button::Select => Some((&mut self.select, Gesture::Press)),
            Button::Back => Some((&mut self.back, Gesture::Back)),
            Button::Up | Button::Down => None,
        }
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

    // ---- Up/Down: the step cadence the board's `ButtonInput` used to own -------------------
    //
    // These four came over from `obc_platform::button_input` when auto-repeat moved here (D1,
    // #1515). One of its tests did **not**: `auto_repeat_can_be_disabled` covered a
    // `Timing::auto_repeat` flag that no board, host or test ever set to `false`. The knob was
    // deleted with the move rather than re-created here, so there is nothing left to disable —
    // holding Up or Down always repeats.

    /// A press steps at once — the same instant the old board-side path stepped, so nothing about
    /// press-to-move latency changed — and the release is silent.
    #[test]
    fn a_direction_press_steps_immediately_and_the_release_is_silent() {
        let mut g = Gestures::with_defaults();
        assert_eq!(g.on_event(down(Button::Down), 8), Some(Gesture::Step(1)), "DOWN is +1, on the press edge");
        assert_eq!(g.on_event(up(Button::Down), 28), None, "the release adds nothing");
        assert_eq!(g.on_event(down(Button::Up), 40), Some(Gesture::Step(-1)), "UP is -1");
        assert_eq!(g.on_event(up(Button::Up), 60), None);
    }

    #[test]
    fn holding_down_auto_repeats_then_stops_on_release() {
        let mut g = Gestures::with_defaults(); // delay 350, interval 120

        assert_eq!(g.on_event(down(Button::Down), 8), Some(Gesture::Step(1))); // repeat armed for 8 + 350 = 358
        assert_eq!(g.tick(100), None, "before the repeat delay");
        assert_eq!(g.tick(358), Some(Gesture::Step(1)), "first auto-repeat; next due 358 + 120 = 478");
        assert_eq!(g.tick(478), Some(Gesture::Step(1)), "second auto-repeat");

        g.on_event(up(Button::Down), 498); // release disarms
        assert_eq!(g.tick(700), None, "long after the release: no more steps");
    }

    /// Up and Down repeat independently: holding one while the other is tapped keeps both
    /// cadences intact, and Select held alongside neither blocks nor swallows the steps.
    #[test]
    fn the_two_directions_and_select_stay_independent() {
        let mut g = Gestures::with_defaults();

        assert_eq!(g.on_event(down(Button::Select), 0), None, "a Select press emits nothing yet");
        assert_eq!(g.on_event(down(Button::Down), 8), Some(Gesture::Step(1)), "the held Select does not swallow it");
        assert_eq!(g.on_event(down(Button::Up), 20), Some(Gesture::Step(-1)), "both directions can be down at once");

        // Both repeats fall due in the same frame: Up (armed for 370) fires first, Down (358) next
        // call — one gesture per `tick`, nothing dropped.
        assert_eq!(g.tick(400), Some(Gesture::Step(-1)), "UP is checked first");
        assert_eq!(g.tick(400), Some(Gesture::Step(1)), "DOWN's due repeat is deferred, not lost");

        assert_eq!(g.on_event(up(Button::Select), 410), None, "Select released past the tap window: no tap");
    }

    /// A stalled loop that only ticks again long after several intervals must emit exactly ONE
    /// catch-up step (not one per missed interval) and rebase the next due time to `now` — else a
    /// long stall would dump a burst and the menu would jump wildly.
    #[test]
    fn a_stalled_loop_emits_one_catch_up_step_then_rearms() {
        let mut g = Gestures::with_defaults(); // delay 350, interval 120

        assert_eq!(g.on_event(down(Button::Down), 8), Some(Gesture::Step(1))); // repeat armed for 358

        // The loop stalls, then resumes at 10_000 — past `due` by ~80 intervals.
        assert_eq!(g.tick(10_000), Some(Gesture::Step(1)), "exactly one catch-up step");
        assert_eq!(g.tick(10_000), None, "not a burst — only one step for the whole stall");

        // Rearmed relative to `now`: nothing is due before 10_000 + 120 = 10_120.
        assert_eq!(g.tick(10_119), None, "next step rebased to now + interval, not the old due");
        assert_eq!(g.tick(10_120), Some(Gesture::Step(1)), "the next interval fires off the rebased due");
    }

    /// The wrap-tolerant due comparison keeps auto-repeat firing across a u32-millis rollover
    /// (~49.7 days). A naive `now >= due` would suppress it forever after the wrap.
    #[test]
    fn auto_repeat_survives_a_millis_wrap() {
        let mut g = Gestures::with_defaults(); // delay 350, interval 120

        let t0 = u32::MAX - 92; // press ~92 ms before the rollover; the armed due wraps to 257
        assert_eq!(g.on_event(down(Button::Down), t0), Some(Gesture::Step(1)));

        // After the wrap, now = 300 is past due (257) by 43 ms; `wrapping_sub` keeps that small
        // and positive → the step fires.
        assert_eq!(g.tick(300), Some(Gesture::Step(1)), "repeat fires across the millis rollover");
    }
}
