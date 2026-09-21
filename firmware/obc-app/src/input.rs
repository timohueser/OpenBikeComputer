//! Gesture recognition — the shared input layer.
//!
//! Turns raw [`InputEvent`]s and a millis clock into the five UI [`Gesture`]s and the device-wide
//! [`Chord`]s, identically across host and MCU. `no_std`, zero-alloc and clock-agnostic: the caller
//! passes the current time in. Every timing the rider feels lives here, so a board contributes
//! debounced edges and nothing else.
//!
//! Two buttons pressed within [`DEFAULT_CHORD_MS`] of each other, in either order, are a chord and
//! not two gestures. A latched chord emits nothing from either constituent and both releases are
//! silent; the latch clears only once both buttons are up. The price is the step deferral: a
//! directional press cannot step until the window passes without a partner, but a release inside
//! the window still steps at once, so a tap feels the same.

use obc_ports::{Button, ButtonEvent, InputEvent};

/// Long-press threshold (ms): how long Select or Back must be held to read as a long press.
pub const DEFAULT_HOLD_MS: u32 = 500;

/// Maximum separation between a chord's two press edges (ms), and therefore also how long a
/// directional press defers its first step while it waits to learn whether a partner is coming.
pub const DEFAULT_CHORD_MS: u32 = 100;

/// Delay from an Up/Down press to its first auto-repeat step (ms). It is measured from the press
/// edge, not from the deferred first step, so the chord window costs the cadence nothing.
const REPEAT_DELAY_MS: u32 = 350;
/// Interval between auto-repeat steps while Up/Down stays held (ms) — ~8 steps/s.
const REPEAT_INTERVAL_MS: u32 = 120;

/// Short-press (tap) window (ms). The three outcomes are: a release at or inside it is a press; a
/// release after it but before [`DEFAULT_HOLD_MS`] is a cancelled long-press and fires nothing; a
/// press held past [`DEFAULT_HOLD_MS`] is a long press.
pub const DEFAULT_TAP_MS: u32 = 200;

/// A device-wide two-button chord, recognised above the screen stack: it never reaches a screen's
/// `handle`, and neither do the presses it is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chord {
    /// Up + Select tap: open (or close) the universal quick drawer.
    Quick,
    /// Up + Select hold: open the Assistant directly.
    Assistant,
    /// Down + Back: open the current screen's contextual drawer, where one is declared.
    Context,
}

/// Every recognised button pair: the two that mean something plus the two that are reserved.
///
/// A reserved pair still latches, so it swallows its constituents and performs nothing. Leaving it
/// unrecognised would make a squeeze of Up+Down read as two independent steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pair {
    Quick,
    Context,
    UpDown,
    /// Select + Back. Reserved: shutdown deliberately gets no hidden chord.
    SelectBack,
}

impl Pair {
    /// The four pairs with their buttons, in the order [`Gestures::latch_chord`] tries them.
    ///
    /// The order decides only the case where one press completes two pairs at the same instant,
    /// which then resolves to the first meaningful pair. It does not re-open an already-latched
    /// pair: Up at 0 ms and Down at 10 ms latch the reserved `UpDown` there and then, and a Select
    /// at 20 ms is a separate press on a latch that will not look again.
    const ALL: [(Pair, Button, Button); 4] = [
        (Pair::Quick, Button::Up, Button::Select),
        (Pair::Context, Button::Down, Button::Back),
        (Pair::UpDown, Button::Up, Button::Down),
        (Pair::SelectBack, Button::Select, Button::Back),
    ];

    fn buttons(self) -> (Button, Button) {
        let (_, a, b) = Pair::ALL[self as usize];
        (a, b)
    }

    /// Whether `b` is one of this pair's constituents.
    fn holds(self, b: Button) -> bool {
        let (x, y) = self.buttons();
        x == b || y == b
    }

    /// Immediate action for this pair. Quick waits for release or the hold threshold.
    fn chord(self) -> Option<Chord> {
        match self {
            Pair::Quick => None,
            Pair::Context => Some(Chord::Context),
            Pair::UpDown | Pair::SelectBack => None,
        }
    }
}

/// The five UI gestures a screen's `handle` reacts to, recognized from raw [`InputEvent`]s and a
/// millis clock by [`Gestures`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gesture {
    /// Up/Down pressed; `n` signed steps (negative = Up / "previous", positive = Down / "next").
    Step(i32),
    /// Select short press (released within the tap window).
    Press,
    /// Select long press (held past the threshold).
    Hold,
    Back,
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
    /// Progress 0.0-1.0 toward the long-press threshold, or 0 while the button is up.
    /// `now.wrapping_sub` tolerates the millis clock wrapping.
    fn progress(&self, now: u32, hold_ms: u32) -> f32 {
        match self.since {
            Some(t0) if hold_ms > 0 => (now.wrapping_sub(t0) as f32 / hold_ms as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    }
}

/// The between-event state that does not fit in a [`Held`]: the Up/Down pair's step timing, and
/// the chord latch. Directions are indexed by [`step_axis`]: `0` = Up, `1` = Down.
///
/// The two live in one struct for size alone. The `[u32; 2]` of due-times leaves exactly three
/// padding bytes, and the latch is three bytes, so a chord field of its own would cost four
/// resident bytes on the part.
#[derive(Debug, Clone, Copy, Default)]
struct Edges {
    /// Millis the direction's next step is due: the deferred first step while its `DEFER` bit
    /// is set, an auto-repeat afterwards. The latched Up slot times a pending Assistant hold.
    due: [u32; 2],
    /// Per-direction bits: `DOWN << axis` = the button is down, `DEFER << axis` = its first step
    /// has not been emitted yet. `QUICK_PENDING` waits for the quick pair's release or hold.
    flags: u8,
    /// The chord currently latched, if any. Set on the second press edge of a pair; cleared once
    /// both its constituents are up.
    latch: Option<Pair>,
    /// The chord this drain still owes the caller ([`Gestures::take_chord`]). Never set for a
    /// reserved pair, which latches — and so swallows — but means nothing.
    pending: Option<Chord>,
}

/// `flags` bit for "this direction is down", shifted by axis.
const DOWN: u8 = 1;
/// `flags` bit for "this direction still owes its first step", shifted by axis.
const DEFER: u8 = 4;
/// Up + Select is waiting for its first release or hold deadline.
const QUICK_PENDING: u8 = 16;

impl Edges {
    /// A press edge at `now`: the direction goes down owing a first step, deferred to the end of
    /// the chord window.
    fn arm(&mut self, axis: usize, now: u32) {
        self.flags |= (DOWN | DEFER) << axis;
        self.due[axis] = now.wrapping_add(DEFAULT_CHORD_MS);
    }

    /// A release edge: the direction stops. Returns whether it still owed its first step — the
    /// quick tap that must step now rather than pay the deferral it never needed.
    fn disarm(&mut self, axis: usize) -> bool {
        let owed = self.deferred(axis);
        self.flags &= !((DOWN | DEFER) << axis);
        owed
    }

    /// Whether this direction is down and still owes its first step.
    fn deferred(&self, axis: usize) -> bool {
        self.flags & (DEFER << axis) != 0
    }

    fn down(&self, axis: usize) -> bool {
        self.flags & (DOWN << axis) != 0
    }

    /// The millis of this direction's press edge — recoverable from the deferred due-time, so the
    /// press edge costs no storage of its own. Meaningful only while [`deferred`](Self::deferred).
    fn pressed_at(&self, axis: usize) -> u32 {
        self.due[axis].wrapping_sub(DEFAULT_CHORD_MS)
    }

    /// The signed step of the first direction whose step has fallen due at `now`.
    ///
    /// A deferred first step re-bases the repeat to the press edge (`t0 + `[`REPEAT_DELAY_MS`]), so
    /// the chord window shifts only the first step. An auto-repeat re-bases to
    /// `now + `[`REPEAT_INTERVAL_MS`], so a stalled loop emits one catch-up step rather than one per
    /// missed interval. A latched direction is skipped: its whole press belongs to the chord.
    fn due_step(&mut self, now: u32) -> Option<i32> {
        for (axis, b, dir) in [(0usize, Button::Up, -1i32), (1, Button::Down, 1)] {
            if !self.down(axis) || self.latch.is_some_and(|p| p.holds(b)) {
                continue;
            }
            // Wrap-tolerant "due reached": `now` ∈ [due, due + 2^31). A plain `now >= due` would
            // stop repeating forever after the u32-millis rollover (~49.7 days).
            if now.wrapping_sub(self.due[axis]) >= u32::MAX / 2 {
                continue;
            }
            self.due[axis] = if self.deferred(axis) {
                let t0 = self.pressed_at(axis);
                self.flags &= !(DEFER << axis);
                t0.wrapping_add(REPEAT_DELAY_MS)
            } else {
                now.wrapping_add(REPEAT_INTERVAL_MS)
            };
            return Some(dir);
        }
        None
    }
}

// No pointers or `usize`s inside, so this size reads the same on host and target.
const _: () = assert!(core::mem::size_of::<Edges>() == 12);

/// The [`Edges`] axis and step sign of a direction button — Up is "previous" (−1), Down is "next"
/// (+1) — or `None` for the two timed buttons, which have no step.
fn step_axis(b: Button) -> Option<(usize, i32)> {
    match b {
        Button::Up => Some((0, -1)),
        Button::Down => Some((1, 1)),
        Button::Select | Button::Back => None,
    }
}

/// Shared gesture recognizer. Feed it raw [`InputEvent`]s ([`on_event`](Gestures::on_event)) and
/// call [`tick`](Gestures::tick) once per frame with the current millis. It emits [`Gesture`]s,
/// hands the caller any [`Chord`] through [`take_chord`](Gestures::take_chord), and exposes
/// hold-progress (0.0-1.0) for the guarded-action confirm ring.
///
/// `Hold` fires the instant the threshold is crossed while still held, so the ring completes
/// without waiting for release. That is why there is both an event hook and a per-frame `tick`.
pub struct Gestures {
    hold_ms: u32,
    /// Max press duration that still counts as a tap. Clamped to `hold_ms`, so a tiny custom hold
    /// threshold cannot invert the window.
    tap_ms: u32,
    select: Held,
    back: Held,
    edges: Edges,
}

impl Gestures {
    /// A recognizer with a custom long-press threshold (ms) and the [`DEFAULT_TAP_MS`] tap window.
    pub fn new(hold_ms: u32) -> Self {
        Gestures {
            hold_ms,
            tap_ms: DEFAULT_TAP_MS.min(hold_ms),
            select: Held::default(),
            back: Held::default(),
            edges: Edges::default(),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_HOLD_MS)
    }

    /// Feed one raw event captured at time `now` (ms). Returns the gesture it completes, if any.
    ///
    /// An injected `Step` fires at once: it models no button, so no chord can be made of it. A
    /// directional press fires nothing, because its step is owed until the chord window closes or
    /// the button is released. `Press` and `Back` fire on a release within the tap window. Every
    /// event of a latched chord yields `None`.
    pub fn on_event(&mut self, ev: InputEvent, now: u32) -> Option<Gesture> {
        match ev {
            InputEvent::Step(0) => None,
            InputEvent::Step(n) => Some(Gesture::Step(n)),
            InputEvent::Button(ButtonEvent::Down(b)) => self.press(b, now),
            InputEvent::Button(ButtonEvent::Up(b)) => self.release(b, now),
        }
    }

    /// Take the [`Chord`] recognised since the last call, if any: the device-wide gesture the
    /// caller resolves above the screen stack. Reserved pairs are latched, and so swallowed, but
    /// never reported here.
    pub fn take_chord(&mut self) -> Option<Chord> {
        self.edges.pending.take()
    }

    /// A press edge. Up/Down go down owing a deferred first step; Select/Back start their hold
    /// clock — what they mean is settled on release or at the long-press threshold. Either way the
    /// new edge may complete a chord, which latches and swallows both constituents.
    fn press(&mut self, b: Button, now: u32) -> Option<Gesture> {
        match step_axis(b) {
            Some((axis, _)) => self.edges.arm(axis, now),
            None => {
                if let Some((h, _)) = self.timed(b) {
                    h.since = Some(now);
                    h.fired_long = false;
                }
            }
        }
        // A constituent re-pressed while the latch still holds is still the chord's. The arm above
        // just gave it a fresh, unspent press, and [`latch_chord`] will not spend it. Without this,
        // a thumb rolling off and back on mid-squeeze charges the confirm ring and lands a `Hold`
        // on the screen the sheet covers.
        if self.edges.latch.is_some_and(|p| p.holds(b)) {
            self.spend(b);
        }
        self.latch_chord(now);
        None
    }

    /// Mark one constituent spent by the latch: a direction stops owing its first step, a timed
    /// button counts its long-press as already fired, which is also the bit that keeps the confirm
    /// ring at zero.
    fn spend(&mut self, b: Button) {
        match step_axis(b) {
            Some((axis, _)) => self.edges.flags &= !(DEFER << axis),
            None => {
                if let Some((h, _)) = self.timed(b) {
                    h.fired_long = true;
                }
            }
        }
    }

    /// A release edge. A latched chord's constituent is silent. Otherwise Up/Down emit the step
    /// they still owe (a tap inside the chord window, which therefore feels immediate) and are
    /// silent once it has fired; Select/Back tap only when released inside the tap window.
    fn release(&mut self, b: Button, now: u32) -> Option<Gesture> {
        if self.edges.latch.is_some_and(|p| p.holds(b)) {
            self.resolve_quick(now, true);
        }
        if self.release_latched(b) {
            return None;
        }
        if let Some((axis, dir)) = step_axis(b) {
            return self.edges.disarm(axis).then_some(Gesture::Step(dir));
        }
        let tap_ms = self.tap_ms;
        let (h, tap) = self.timed(b)?;
        let since = h.since;
        let fired = h.fired_long;
        *h = Held::default();
        let is_tap = !fired && matches!(since, Some(t0) if now.wrapping_sub(t0) <= tap_ms);
        is_tap.then_some(tap)
    }

    /// Call once per frame with the current millis. Fires a long press the instant a held button
    /// crosses the threshold, the deferred first `Step` when the chord window closes on a still-held
    /// direction, and one auto-repeat `Step` each time a held Up/Down falls due. At most one gesture
    /// per call; anything else due fires on the next call.
    pub fn tick(&mut self, now: u32) -> Option<Gesture> {
        self.resolve_quick(now, false);
        let hold_ms = self.hold_ms;
        // No latch check for the two timed buttons: every press edge under a held latch is spent
        // by [`press`](Self::press), so a latched Select or Back always reads `fired_long`. The
        // direction pair needs its own gate in [`Edges::due_step`] only because a spent direction
        // still carries a live repeat due-time.
        for (b, long) in [(Button::Select, Gesture::Hold), (Button::Back, Gesture::BackHold)] {
            let Some((h, _)) = self.timed(b) else { continue };
            if let Some(t0) = h.since {
                if !h.fired_long && now.wrapping_sub(t0) >= hold_ms {
                    h.fired_long = true;
                    return Some(long);
                }
            }
        }
        self.edges.due_step(now).map(Gesture::Step)
    }

    /// Try to complete a chord at `now`: the first [`Pair`] whose both buttons are down, pressed
    /// within [`DEFAULT_CHORD_MS`] of `now`, and have emitted nothing yet. Latching marks each
    /// constituent as spent and keeps [`tick`](Self::tick) off both until they are up again.
    fn latch_chord(&mut self, now: u32) {
        if self.edges.latch.is_some() {
            return;
        }
        let Some(pair) =
            Pair::ALL.into_iter().find(|(_, a, b)| self.joins_chord(*a, now) && self.joins_chord(*b, now)).map(|p| p.0)
        else {
            return;
        };
        self.edges.latch = Some(pair);
        self.edges.pending = pair.chord();
        let (a, b) = pair.buttons();
        self.spend(a);
        self.spend(b);
        if pair == Pair::Quick {
            self.edges.flags |= QUICK_PENDING;
            // The latched Up cannot step, so its due-time can time the shared hold instead.
            self.edges.due[0] = now.wrapping_add(self.hold_ms);
        }
    }

    /// Time until the pending chord needs recognition, including on an otherwise idle host.
    pub fn chord_remaining_ms(&self, now: u32) -> Option<u32> {
        (self.edges.flags & QUICK_PENDING != 0).then(|| {
            let remaining = self.edges.due[0].wrapping_sub(now);
            if remaining < u32::MAX / 2 {
                remaining
            } else {
                0
            }
        })
    }

    fn resolve_quick(&mut self, now: u32, released: bool) {
        let Some(remaining) = self.chord_remaining_ms(now) else { return };
        if released || remaining == 0 {
            self.edges.flags &= !QUICK_PENDING;
            self.edges.pending = Some(if remaining == 0 { Chord::Assistant } else { Chord::Quick });
        }
    }

    /// Whether `b` may still join a chord at `now`: down, pressed inside the window, and nothing
    /// emitted for it yet.
    fn joins_chord(&self, b: Button, now: u32) -> bool {
        match step_axis(b) {
            Some((axis, _)) => {
                self.edges.deferred(axis) && now.wrapping_sub(self.edges.pressed_at(axis)) <= DEFAULT_CHORD_MS
            }
            None => {
                let h = if b == Button::Select { &self.select } else { &self.back };
                h.since.is_some_and(|t0| !h.fired_long && now.wrapping_sub(t0) <= DEFAULT_CHORD_MS)
            }
        }
    }

    /// Handle a release that belongs to the latched chord: clear that button's state, and drop the
    /// latch once both constituents are up so the next squeeze re-arms. Returns whether the
    /// release was the chord's (and therefore silent).
    fn release_latched(&mut self, b: Button) -> bool {
        let Some(pair) = self.edges.latch.filter(|p| p.holds(b)) else {
            return false;
        };
        match step_axis(b) {
            Some((axis, _)) => {
                self.edges.disarm(axis);
            }
            None => {
                if let Some((h, _)) = self.timed(b) {
                    *h = Held::default();
                }
            }
        }
        let (x, y) = pair.buttons();
        if self.is_up(x) && self.is_up(y) {
            self.edges.latch = None;
        }
        true
    }

    /// Whether `b` is physically up, as the recogniser sees it.
    fn is_up(&self, b: Button) -> bool {
        match step_axis(b) {
            Some((axis, _)) => !self.edges.down(axis),
            None => (if b == Button::Select { &self.select } else { &self.back }).since.is_none(),
        }
    }

    /// Cancel any pending Assistant chord and any in-flight Select or Back press: mark it spent, so
    /// the long press never emits, the eventual release is silent and the hold-progress reads 0. A
    /// fresh press recognises normally, and Up/Down auto-repeat is untouched.
    ///
    /// Called when a gesture-driven screen transition changes what is under the rider's finger: a
    /// long-press that started charging over one screen must not complete onto what replaced it.
    pub fn cancel_holds(&mut self) {
        self.edges.flags &= !QUICK_PENDING;
        for h in [&mut self.select, &mut self.back] {
            if h.since.is_some() {
                h.fired_long = true;
            }
        }
    }

    /// Hold-progress (0.0-1.0) of an in-flight Select long-press, for the confirm ring. It reads 0
    /// when not held and once the `Hold` has fired.
    pub fn select_progress(&self, now: u32) -> f32 {
        if self.select.fired_long {
            0.0
        } else {
            self.select.progress(now, self.hold_ms)
        }
    }

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

    #[test]
    fn cancelled_long_press_fires_nothing() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        assert!(g.select_progress(300) > 0.0, "the hold was visibly in progress");
        assert_eq!(g.tick(300), None, "no long-press yet");
        assert_eq!(g.on_event(up(Button::Select), 300), None, "a release in the dead zone fires nothing");

        g.on_event(down(Button::Back), 1000);
        assert_eq!(g.on_event(up(Button::Back), 1350), None, "a cancelled Back-hold fires nothing");
    }

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

    /// The two timed buttons are pressed far enough apart to be two presses rather than the
    /// reserved Select+Back chord.
    #[test]
    fn the_two_buttons_are_independent() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        g.on_event(down(Button::Back), DEFAULT_CHORD_MS + 1);
        assert_eq!(g.tick(500), Some(Gesture::Hold));
        assert!(g.back_progress(500) > 0.0 && g.back_progress(500) < 1.0);
        assert_eq!(g.tick(700), Some(Gesture::BackHold));
    }

    #[test]
    fn cancel_holds_suppresses_the_pending_long_press_and_the_release() {
        let mut g = Gestures::new(500);
        g.on_event(down(Button::Select), 0);
        assert!(g.select_progress(300) > 0.0, "charging before the cancel");
        g.cancel_holds();
        assert_eq!(g.select_progress(300), 0.0, "a cancelled hold reads no progress");
        assert_eq!(g.tick(600), None, "the long-press never fires");
        assert_eq!(g.on_event(up(Button::Select), 650), None, "the release is silent, not a tap");
        g.on_event(down(Button::Select), 1_000);
        assert_eq!(g.tick(1_500), Some(Gesture::Hold), "the next hold recognises normally");
        g.cancel_holds();
        g.on_event(down(Button::Back), 2_000);
        assert_eq!(g.on_event(up(Button::Back), 2_100), Some(Gesture::Back), "an idle cancel affects nothing");
    }

    #[test]
    fn both_holds_crossing_one_frame_fire_one_then_the_other() {
        let mut g = Gestures::new(500);
        // Staggered past the chord window, so this is not the reserved Select+Back chord.
        g.on_event(down(Button::Select), 0);
        g.on_event(down(Button::Back), 200);
        assert_eq!(g.tick(700), Some(Gesture::Hold), "the Select long-press wins the shared frame");
        assert_eq!(g.tick(700), Some(Gesture::BackHold), "the other hold fires next frame, not dropped");
        assert_eq!(g.tick(700), None, "and each long-press fires exactly once");
    }

    #[test]
    fn a_direction_tap_steps_on_release_and_a_fired_step_releases_silently() {
        let mut g = Gestures::with_defaults();
        assert_eq!(g.on_event(down(Button::Down), 8), None, "the press waits out the chord window");
        assert_eq!(g.on_event(up(Button::Down), 28), Some(Gesture::Step(1)), "released inside it: DOWN is +1, now");
        assert_eq!(g.on_event(down(Button::Up), 40), None);
        assert_eq!(g.on_event(up(Button::Up), 60), Some(Gesture::Step(-1)), "UP is -1");

        // Held past the window instead: the step comes from `tick`, and the release adds nothing.
        assert_eq!(g.on_event(down(Button::Down), 100), None);
        assert_eq!(g.tick(200), Some(Gesture::Step(1)));
        assert_eq!(g.on_event(up(Button::Down), 260), None, "the step already fired");
    }

    #[test]
    fn a_held_direction_steps_exactly_at_the_chord_window() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Down), 1_000);
        assert_eq!(g.tick(1_000 + DEFAULT_CHORD_MS - 1), None, "still inside the window: a partner could arrive");
        assert_eq!(g.tick(1_000 + DEFAULT_CHORD_MS), Some(Gesture::Step(1)), "the window closed alone");
    }

    #[test]
    fn holding_down_auto_repeats_then_stops_on_release() {
        let mut g = Gestures::with_defaults(); // chord 100, delay 350, interval 120

        assert_eq!(g.on_event(down(Button::Down), 8), None);
        assert_eq!(g.tick(100), None, "before the chord window closes");
        assert_eq!(g.tick(108), Some(Gesture::Step(1)), "the deferred first step");
        // The repeat re-bases to the press edge: 8 + 350 = 358.
        assert_eq!(g.tick(357), None, "the deferral does not push the repeat out");
        assert_eq!(g.tick(358), Some(Gesture::Step(1)), "first auto-repeat; next due 358 + 120 = 478");
        assert_eq!(g.tick(478), Some(Gesture::Step(1)), "second auto-repeat");

        g.on_event(up(Button::Down), 498);
        assert_eq!(g.tick(700), None, "long after the release: no more steps");
    }

    #[test]
    fn the_two_directions_and_select_stay_independent() {
        let mut g = Gestures::with_defaults();

        assert_eq!(g.on_event(down(Button::Select), 0), None, "a Select press emits nothing yet");
        assert_eq!(g.on_event(down(Button::Down), 8), None, "Down + Select is not a pair");
        assert_eq!(g.on_event(down(Button::Up), 150), None, "…and Up arrives past both open windows");
        assert_eq!(g.take_chord(), None, "no chord: no two presses were a pair inside one window");

        // Both deferred firsts fall due in the same frame; one gesture per `tick`.
        assert_eq!(g.tick(400), Some(Gesture::Step(-1)), "UP is checked first");
        assert_eq!(g.tick(400), Some(Gesture::Step(1)), "DOWN's due step is deferred, not lost");

        assert_eq!(g.on_event(up(Button::Select), 410), None, "Select released past the tap window: no tap");
    }

    /// A long stall must emit one catch-up step, not a burst that makes the menu jump.
    #[test]
    fn a_stalled_loop_emits_one_catch_up_step_then_rearms() {
        let mut g = Gestures::with_defaults(); // chord 100, delay 350, interval 120

        g.on_event(down(Button::Down), 8);
        assert_eq!(g.tick(108), Some(Gesture::Step(1))); // repeat armed for 358

        // The loop stalls, then resumes at 10_000 — past `due` by ~80 intervals.
        assert_eq!(g.tick(10_000), Some(Gesture::Step(1)), "exactly one catch-up step");
        assert_eq!(g.tick(10_000), None, "not a burst — only one step for the whole stall");

        assert_eq!(g.tick(10_119), None, "next step rebased to now + interval, not the old due");
        assert_eq!(g.tick(10_120), Some(Gesture::Step(1)), "the next interval fires off the rebased due");
    }

    /// The wrap-tolerant due comparison holds the step cadence across a u32-millis rollover. A
    /// press just before the wrap arms a due time numerically smaller than `now`, which a naive
    /// `now >= due` would read as already reached.
    #[test]
    fn the_step_cadence_survives_a_millis_wrap() {
        let mut g = Gestures::with_defaults(); // chord 100, delay 350, interval 120

        let t0 = u32::MAX - 50; // press ~50 ms before the rollover; the deferred due wraps to 49
        g.on_event(down(Button::Down), t0);

        // 8 ms into the 100 ms deferral, still before the wrap: nothing is due.
        assert_eq!(g.tick(t0.wrapping_add(8)), None, "the rollover does not short-circuit the deferral");

        // After the wrap, now = 60 is past due (49); `wrapping_sub` keeps that small and positive.
        assert_eq!(g.tick(60), Some(Gesture::Step(1)), "the first step fires across the millis rollover");
        // …and the repeat, re-based to the (pre-wrap) press edge, is due at t0 + 350 → 299.
        assert_eq!(g.tick(298), None);
        assert_eq!(g.tick(299), Some(Gesture::Step(1)), "the repeat re-bases across the rollover too");
    }

    #[test]
    fn quick_tap_and_assistant_hold_are_exclusive_and_rearm_in_both_orders() {
        for (first, second) in [(Button::Up, Button::Select), (Button::Select, Button::Up)] {
            for (a, b) in [(first, second), (second, first)] {
                let mut g = Gestures::with_defaults();
                for (start, held) in [(0u32, false), (1_000, true), (2_000, false)] {
                    assert_eq!(g.on_event(down(first), start), None);
                    assert_eq!(g.on_event(down(second), start + 60), None);
                    assert_eq!(g.take_chord(), None, "no drawer on the press edges");
                    assert_eq!(g.chord_remaining_ms(start + 60), Some(DEFAULT_HOLD_MS));
                    assert_eq!(g.tick(start + 100), None);
                    assert_eq!(g.select_progress(start + 100), 0.0);
                    if held {
                        assert_eq!(g.tick(start + 559), None);
                        assert_eq!(g.take_chord(), None);
                        assert_eq!(g.tick(start + 560), None, "no constituent Hold");
                        assert_eq!(g.take_chord(), Some(Chord::Assistant));
                        assert_eq!(g.tick(start + 800), None);
                    }
                    let released = start + if held { 820 } else { 180 };
                    assert_eq!(g.on_event(up(a), released), None);
                    assert_eq!(g.take_chord(), if held { None } else { Some(Chord::Quick) });
                    assert_eq!(g.on_event(up(b), released + 20), None);
                    assert_eq!(g.take_chord(), None);
                    assert_eq!(g.chord_remaining_ms(released + 20), None);
                }
                g.on_event(down(Button::Up), 3_000);
                assert_eq!(g.on_event(up(Button::Up), 3_040), Some(Gesture::Step(-1)));
            }
        }
    }

    #[test]
    fn quick_release_at_threshold_and_clock_rollover_resolve_the_hold() {
        let mut g = Gestures::with_defaults();
        let start = u32::MAX - 100;
        g.on_event(down(Button::Select), start);
        g.on_event(down(Button::Up), start + 40);
        let deadline = (start + 40).wrapping_add(DEFAULT_HOLD_MS);
        assert_eq!(g.chord_remaining_ms(deadline - 1), Some(1));
        assert_eq!(g.on_event(up(Button::Up), deadline), None);
        assert_eq!(g.take_chord(), Some(Chord::Assistant), "release need not wait for an idle tick");
        assert_eq!(g.on_event(up(Button::Select), deadline + 20), None);
        assert_eq!(g.take_chord(), None);
    }

    #[test]
    fn cancel_holds_cancels_pending_assistant_without_a_quick_tap() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Up), 0);
        g.on_event(down(Button::Select), 40);
        g.cancel_holds();
        assert_eq!(g.chord_remaining_ms(100), None);
        assert_eq!(g.tick(600), None);
        assert_eq!(g.take_chord(), None);
        assert_eq!(g.on_event(up(Button::Up), 650), None);
        assert_eq!(g.on_event(up(Button::Select), 670), None);
        assert_eq!(g.take_chord(), None);
        g.on_event(down(Button::Select), 700);
        assert_eq!(g.on_event(up(Button::Select), 760), Some(Gesture::Press));
    }

    #[test]
    fn every_pair_is_swallowed_and_only_the_two_meaningful_ones_are_reported() {
        let squeeze = |a: Button, b: Button| {
            let mut g = Gestures::with_defaults();
            assert_eq!(g.on_event(down(a), 0), None);
            assert_eq!(g.on_event(down(b), 50), None);
            let chord = g.take_chord();
            assert_eq!(g.tick(700), None, "{a:?}+{b:?}: nothing leaks while the chord is held");
            assert_eq!(g.on_event(up(a), 800), None);
            assert_eq!(g.on_event(up(b), 810), None);
            assert_eq!(g.tick(900), None, "{a:?}+{b:?}: and nothing leaks after");
            chord
        };
        assert_eq!(squeeze(Button::Down, Button::Back), Some(Chord::Context));
        assert_eq!(squeeze(Button::Up, Button::Down), None, "Up+Down is reserved: swallowed, meaning nothing");
        assert_eq!(squeeze(Button::Select, Button::Back), None, "Select+Back is reserved");
    }

    #[test]
    fn a_late_partner_is_two_independent_gestures() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Up), 0);
        assert_eq!(g.tick(DEFAULT_CHORD_MS), Some(Gesture::Step(-1)), "the window closed alone");
        g.on_event(down(Button::Select), DEFAULT_CHORD_MS + 1);
        assert_eq!(g.take_chord(), None, "one ms too late is not a chord");
        assert_eq!(g.on_event(up(Button::Select), DEFAULT_CHORD_MS + 60), Some(Gesture::Press), "…and Select taps");
        assert_eq!(g.on_event(up(Button::Up), DEFAULT_CHORD_MS + 80), None);
    }

    #[test]
    fn an_early_release_is_two_independent_gestures() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Up), 0);
        assert_eq!(g.on_event(up(Button::Up), 40), Some(Gesture::Step(-1)), "the tap steps on release");
        g.on_event(down(Button::Select), 50);
        assert_eq!(g.take_chord(), None, "the partner is already gone — nothing to pair with");
        assert_eq!(g.on_event(up(Button::Select), 110), Some(Gesture::Press));
    }

    #[test]
    fn the_chord_window_is_inclusive() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Down), 0);
        g.on_event(down(Button::Back), DEFAULT_CHORD_MS);
        assert_eq!(g.take_chord(), Some(Chord::Context), "exactly at the window still pairs");
    }

    /// The thumb roll: a rider squeezing slowly lets one button go and puts it back while the
    /// other is still down. Nothing may leak from that unspent press edge: no step, no long press,
    /// and no confirm ring charging at the screen under the sheet.
    #[test]
    fn a_repressed_constituent_inside_a_held_chord_leaks_nothing() {
        // Up + Select: the rolled button is Select, so the leak would be a `Hold`.
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Up), 0);
        g.on_event(down(Button::Select), 30);
        assert_eq!(g.take_chord(), None);
        assert_eq!(g.on_event(up(Button::Select), 100), None, "Up is still down, so the latch holds");
        assert_eq!(g.take_chord(), Some(Chord::Quick));
        assert_eq!(g.on_event(down(Button::Select), 120), None, "…and the thumb rolls back on");
        assert_eq!(g.select_progress(400), 0.0, "no confirm ring charges under a held chord");
        assert_eq!(g.tick(620), None, "and no Hold reaches the screen the sheet covers");
        assert_eq!(g.take_chord(), None, "a rolled thumb cannot restart the Assistant hold");
        assert_eq!(g.on_event(up(Button::Select), 700), None);
        assert_eq!(g.on_event(up(Button::Up), 720), None, "the last release ends the chord, silently");

        // Down + Back: the rolled button is Back, so the leak would be a `BackHold`.
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Down), 0);
        g.on_event(down(Button::Back), 30);
        assert_eq!(g.take_chord(), Some(Chord::Context));
        assert_eq!(g.on_event(up(Button::Back), 100), None);
        assert_eq!(g.on_event(down(Button::Back), 120), None);
        assert_eq!(g.back_progress(400), 0.0);
        assert_eq!(g.tick(620), None, "no BackHold either");
        assert_eq!(g.on_event(up(Button::Back), 700), None);
        assert_eq!(g.on_event(up(Button::Down), 720), None);

        g.on_event(down(Button::Back), 800);
        assert_eq!(g.on_event(up(Button::Back), 860), Some(Gesture::Back), "the latch re-armed");
    }

    #[test]
    fn the_latch_holds_until_both_buttons_are_up() {
        let mut g = Gestures::with_defaults();
        g.on_event(down(Button::Up), 0);
        g.on_event(down(Button::Select), 30);
        assert_eq!(g.take_chord(), None);

        assert_eq!(g.on_event(up(Button::Up), 100), None);
        assert_eq!(g.take_chord(), Some(Chord::Quick));
        assert_eq!(g.tick(700), None, "the still-held Select is still the chord's");
        assert_eq!(g.select_progress(700), 0.0);

        // The thumb rolls back onto Up while Select is still down. The latch has not cleared, so
        // that re-press is the chord's too.
        assert_eq!(g.on_event(down(Button::Up), 720), None);
        assert_eq!(g.tick(900), None, "a re-press inside a held chord steps nothing");
        assert_eq!(g.on_event(up(Button::Up), 920), None);

        assert_eq!(g.on_event(up(Button::Select), 940), None, "the last release is not a tap");

        g.on_event(down(Button::Select), 1_000);
        assert_eq!(g.on_event(up(Button::Select), 1_060), Some(Gesture::Press), "and now Select taps again");
    }
}
