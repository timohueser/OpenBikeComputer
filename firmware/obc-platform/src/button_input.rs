//! Board-agnostic pushbutton input → the shared gesture recognizer.
//!
//! Turns raw GPIO levels into [`InputEvent`]s and implements [`InputSource`], so a board
//! drops it into the app's input handler, which runs the shared gesture recognizer — the same path
//! the host uses with its keyboard / on-screen controls. This only manufactures the raw events.
//!
//! The device's controls are four pushbuttons sharing one common pin — **UP** / **DOWN** on the
//! left flank, **SELECT** / **BACK** on the right — so:
//! - **UP** / **DOWN** forward [`Button::Up`] / [`Button::Down`] edges. The shared recognizer owns
//!   stepping, repeat, and drawer chords.
//! - **SELECT** forwards [`Button::Select`] edges → `Gestures` yields `Press` (short)
//!   / `Hold` (long).
//! - **BACK** forwards [`Button::Back`] edges → `Back` / `BackHold`.
//!
//! ## Wiring convention — active-low
//! Each switch connects its GPIO to the shared **GND** common pin, and the input uses its
//! **internal pull-up** (no external parts). So a released line reads high and a press pulls it low:
//! pressed ≡ [`InputPin::is_low`].
//!
//! ## Time
//! Debounce needs a clock, but [`InputSource::poll`] is clockless, so the board calls
//! [`ButtonInput::update`] with the current wall-clock millis once per loop *before* `handle_input`;
//! `update` samples the pins and queues events, `poll` drains the queue. Injecting the clock keeps
//! the crate board-agnostic and host-testable.

use embedded_hal::digital::InputPin;
use heapless::Deque;
use obc_ports::{Button, ButtonEvent, InputEvent, InputSource};

/// Default contact-settle window (ms): a level must hold this long before its edge is reported.
/// 8 ms rejects switch bounce without a perceptible press delay.
pub const DEFAULT_DEBOUNCE_MS: u32 = 8;
/// Capacity of the event ring between [`ButtonInput::update`] and the app's drain. One `update`
/// queues at most one event per button (four) and the app drains to empty every frame, so this
/// never fills.
const QUEUE_LEN: usize = 8;

/// Raw button debounce timing.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Contact-settle window for every button (ms).
    pub debounce_ms: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { debounce_ms: DEFAULT_DEBOUNCE_MS }
    }
}

/// One debounced active-low input: a level is committed — and its edge reported — only once it has
/// held steady for the debounce window.
struct Debounced<P> {
    pin: P,
    /// Committed (debounced) state: `true` = pressed.
    pressed: bool,
    /// Last raw read (active-low: a low line is `true` = pressed).
    candidate: bool,
    /// Millis the current `candidate` was first seen.
    since: u32,
}

/// A debounced transition reported by [`Debounced::update`].
enum Edge {
    Press,
    Release,
}

impl<P: InputPin> Debounced<P> {
    fn new(pin: P) -> Self {
        Debounced { pin, pressed: false, candidate: false, since: 0 }
    }

    /// Sample the pin at `now` (ms) and report a debounced edge if the level just
    /// committed. A bounce shorter than `debounce_ms` keeps resetting `since` and so
    /// is never committed.
    fn update(&mut self, now: u32, debounce_ms: u32) -> Option<Edge> {
        // Active-low: a *low* read is a press. A read error is impossible on real GPIO
        // (`Infallible`); treat any as "released".
        let raw = self.pin.is_low().unwrap_or(false);
        if raw != self.candidate {
            // New raw level — restart the settle timer; don't commit yet.
            self.candidate = raw;
            self.since = now;
            None
        } else if raw != self.pressed && now.wrapping_sub(self.since) >= debounce_ms {
            // Raw has held steady past the window and differs from the committed
            // state: commit it and report the edge.
            self.pressed = raw;
            Some(if raw { Edge::Press } else { Edge::Release })
        } else {
            None
        }
    }

    /// Whether this button is fully released *and* settled — neither committed-pressed nor
    /// mid-bounce toward a press. The idle check the event-driven input plane gates its edge-wake on.
    fn settled_released(&self) -> bool {
        !self.pressed && !self.candidate
    }
}

/// Four pushbuttons → raw [`InputEvent`]s for the shared app. Generic over any [`InputPin`], so the
/// same type serves the nRF board (`embassy_nrf::gpio::Input`) and the host test mock. Drive it as:
/// [`update`](Self::update) once per loop with the current millis, then hand `&mut self` to
/// the app's input handler, which drains it through [`InputSource`].
pub struct ButtonInput<P> {
    up: Debounced<P>,
    down: Debounced<P>,
    select: Debounced<P>,
    back: Debounced<P>,
    timing: Timing,
    queue: Deque<InputEvent, QUEUE_LEN>,
}

impl<P: InputPin> ButtonInput<P> {
    /// Build from the four pins (UP, DOWN, SELECT, BACK) with [`Timing::default`].
    pub fn new(up: P, down: P, select: P, back: P) -> Self {
        Self::with_timing(up, down, select, back, Timing::default())
    }

    /// Build from the four pins with explicit [`Timing`].
    pub fn with_timing(up: P, down: P, select: P, back: P, timing: Timing) -> Self {
        ButtonInput {
            up: Debounced::new(up),
            down: Debounced::new(down),
            select: Debounced::new(select),
            back: Debounced::new(back),
            timing,
            queue: Deque::new(),
        }
    }

    /// Sample all four pins at wall-clock `now_ms` and queue any resulting events.
    /// Call once per loop, before the app's input handler; the app then drains
    /// the queue via [`InputSource::poll`].
    pub fn update(&mut self, now_ms: u32) {
        let t = self.timing;
        // All four controls forward edges. The shared Gestures layer owns timing, repeat, and
        // two-button chord arbitration.
        Self::edge(&mut self.up, Button::Up, now_ms, t.debounce_ms, &mut self.queue);
        Self::edge(&mut self.down, Button::Down, now_ms, t.debounce_ms, &mut self.queue);
        Self::edge(&mut self.select, Button::Select, now_ms, t.debounce_ms, &mut self.queue);
        Self::edge(&mut self.back, Button::Back, now_ms, t.debounce_ms, &mut self.queue);
    }

    /// Whether nothing is in flight: every button is released + settled and the event queue is
    /// drained. The event-driven input plane polls at the loop rate while a press is debouncing or
    /// held; once idle it sleeps on [`wait_for_any_press`](ButtonInput::wait_for_any_press).
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
            && self.up.settled_released()
            && self.down.settled_released()
            && self.select.settled_released()
            && self.back.settled_released()
    }

    /// Forward each debounced edge. Meaning and timing live in the shared recognizer.
    fn edge(
        btn: &mut Debounced<P>,
        which: Button,
        now: u32,
        debounce_ms: u32,
        queue: &mut Deque<InputEvent, QUEUE_LEN>,
    ) {
        match btn.update(now, debounce_ms) {
            Some(Edge::Press) => push(queue, InputEvent::Button(ButtonEvent::Down(which))),
            Some(Edge::Release) => push(queue, InputEvent::Button(ButtonEvent::Up(which))),
            None => {}
        }
    }
}

impl<P: InputPin> InputSource for ButtonInput<P> {
    fn poll(&mut self) -> Option<InputEvent> {
        self.queue.pop_front()
    }
}

/// Async edge-wake for the event-driven input plane, gated behind `input-wait` so the host/sim
/// build never pulls the async machinery. Available when the pin type also implements async
/// [`Wait`](embedded_hal_async::digital::Wait) — `embassy_nrf::gpio::Input` does.
#[cfg(feature = "input-wait")]
impl<P: embedded_hal::digital::InputPin + embedded_hal_async::digital::Wait> ButtonInput<P> {
    /// Resolve as soon as **any** of the four buttons goes low (a press) — the edge that ends an
    /// idle sleep. Called only once [`is_idle`](ButtonInput::is_idle) holds. Active-low, so
    /// `wait_for_low` completes immediately if a button is already down (no missed press across the
    /// poll→sleep handoff). Awaits the four pins in parallel, returning on the first.
    pub async fn wait_for_any_press(&mut self) {
        use embassy_futures::select::{select4, Either4};
        // `Infallible` on real GPIO; ignore an error (a dead wait falls through to the input plane's
        // guard-tick re-poll).
        let _ = match select4(
            self.up.pin.wait_for_low(),
            self.down.pin.wait_for_low(),
            self.select.pin.wait_for_low(),
            self.back.pin.wait_for_low(),
        )
        .await
        {
            Either4::First(r) | Either4::Second(r) | Either4::Third(r) | Either4::Fourth(r) => r,
        };
    }
}

/// Enqueue, dropping on overflow. Overflow can't happen in practice: the app drains
/// the queue to empty each frame and `update` queues at most one event per button.
fn push(queue: &mut Deque<InputEvent, QUEUE_LEN>, ev: InputEvent) {
    let _ = queue.push_back(ev);
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;

    /// Test pin over a shared cell: `cell == true` means the line is *low* (pressed),
    /// matching the active-low wiring. Tests flip the cell to drive the button.
    struct MockPin<'a> {
        low: &'a Cell<bool>,
    }
    impl embedded_hal::digital::ErrorType for MockPin<'_> {
        type Error = core::convert::Infallible;
    }
    impl embedded_hal::digital::InputPin for MockPin<'_> {
        fn is_high(&mut self) -> Result<bool, Self::Error> {
            Ok(!self.low.get())
        }
        fn is_low(&mut self) -> Result<bool, Self::Error> {
            Ok(self.low.get())
        }
    }

    /// The four cells backing a [`ButtonInput`] of [`MockPin`]s, in UP/DOWN/SELECT/
    /// BACK order — kept alive alongside the input the pins borrow from.
    struct Pins {
        up: Cell<bool>,
        down: Cell<bool>,
        select: Cell<bool>,
        back: Cell<bool>,
    }
    impl Pins {
        fn new() -> Self {
            Pins { up: Cell::new(false), down: Cell::new(false), select: Cell::new(false), back: Cell::new(false) }
        }
        fn input(&self) -> ButtonInput<MockPin<'_>> {
            ButtonInput::new(
                MockPin { low: &self.up },
                MockPin { low: &self.down },
                MockPin { low: &self.select },
                MockPin { low: &self.back },
            )
        }
    }

    #[test]
    fn select_debounces_then_emits_button_edges() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.select.set(true); // press
        bi.update(0); // candidate flips; not committed yet
        assert!(bi.poll().is_none(), "no edge before the debounce window");
        bi.update(8); // held steady past 8 ms → Down
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Select))));
        assert!(bi.poll().is_none());

        pins.select.set(false); // release
        bi.update(20);
        bi.update(28); // settled → Up
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Select))));
        assert!(bi.poll().is_none());
    }

    /// `is_idle` is true only when every button is released + settled and the queue is drained. A
    /// press makes it non-idle and it stays non-idle until the edge drains, then idles on release.
    #[test]
    fn is_idle_tracks_in_flight_input() {
        let pins = Pins::new();
        let mut bi = pins.input();
        assert!(bi.is_idle(), "a fresh, untouched set is idle");

        pins.select.set(true);
        bi.update(0); // candidate flips — a press is now bouncing
        assert!(!bi.is_idle(), "mid-bounce toward a press is not idle");
        bi.update(8); // committed Down → an event is queued
        assert!(!bi.is_idle(), "a held button with a queued edge is not idle");
        assert!(bi.poll().is_some()); // drain the Down

        assert!(!bi.is_idle(), "still held (committed-pressed) → keep polling for the hold/release");
        pins.select.set(false);
        bi.update(20);
        bi.update(28); // settled released → Up queued
        assert!(bi.poll().is_some()); // drain the Up
        assert!(bi.is_idle(), "released, settled, queue drained → idle again");
    }

    #[test]
    fn back_maps_to_the_back_button() {
        let pins = Pins::new();
        let mut bi = pins.input();
        pins.back.set(true);
        bi.update(0);
        bi.update(8);
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Back))));
    }

    #[test]
    fn a_glitch_shorter_than_debounce_is_ignored() {
        let pins = Pins::new();
        let mut bi = pins.input();
        pins.select.set(true);
        bi.update(0); // candidate true @0
        pins.select.set(false);
        bi.update(4); // bounced back before 8 ms — candidate resets, never committed
        bi.update(20); // stable released
        assert!(bi.poll().is_none(), "a sub-debounce glitch emits nothing");
    }

    #[test]
    fn up_and_down_emit_button_edges() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.down.set(true);
        bi.update(0);
        bi.update(8);
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Down))));
        pins.down.set(false);
        bi.update(20);
        bi.update(28);
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Down))));

        pins.up.set(true);
        bi.update(40);
        bi.update(48);
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Up))));
    }

    /// SELECT held while DOWN taps still forwards both independent edge streams. Chord arbitration
    /// belongs to the shared recognizer, not this raw GPIO layer.
    #[test]
    fn select_held_while_down_taps_keeps_both_independent() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.select.set(true); // hold SELECT down
        bi.update(0);
        bi.update(8); // SELECT commits → Down(Select)
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Select))));
        assert!(bi.poll().is_none());

        // DOWN taps while SELECT is still held — independent debounce, independent edge.
        pins.down.set(true);
        bi.update(20);
        bi.update(28);
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Down))));
        assert!(bi.poll().is_none(), "held SELECT does not re-emit while DOWN taps");
    }

    /// UP and DOWN committed on the same update enqueue in sampling order and drain intact.
    #[test]
    fn up_and_down_both_pressed_enqueue_in_call_order() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.up.set(true);
        pins.down.set(true);
        bi.update(0); // both candidates flip
        bi.update(8);
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Up))), "UP is sampled first");
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Down))), "DOWN second");
        assert!(bi.poll().is_none());
    }

    /// Force the "can't happen" queue overflow by cycling all controls without draining, and prove
    /// the queue caps at `QUEUE_LEN` rather than panicking or wrapping.
    #[test]
    fn queue_caps_at_eight_and_drops_overflow() {
        let pins = Pins::new();
        let mut bi = pins.input();

        // First press and release fill all eight slots.
        pins.up.set(true);
        pins.down.set(true);
        pins.select.set(true);
        pins.back.set(true);
        bi.update(0);
        bi.update(8);
        pins.up.set(false);
        pins.down.set(false);
        pins.select.set(false);
        pins.back.set(false);
        bi.update(16);
        bi.update(24);

        // Another four committed edges hit the drop path while the queue remains full.
        pins.up.set(true);
        pins.down.set(true);
        pins.select.set(true);
        pins.back.set(true);
        bi.update(32);
        bi.update(40);

        // Drain: exactly QUEUE_LEN events come out, the rest were dropped.
        let mut drained = 0;
        while bi.poll().is_some() {
            drained += 1;
        }
        assert_eq!(drained, QUEUE_LEN, "queue holds at most QUEUE_LEN; overflow is dropped, not panicked");
    }

    /// A level held for *exactly* `debounce_ms` commits (`>=`, not `>`): at `now - since == 8` the
    /// edge fires this frame; at `== 7` it does not. Pins the off-by-one a `>` would introduce.
    #[test]
    fn commit_lands_exactly_on_the_debounce_boundary() {
        let pins = Pins::new();
        let mut bi = pins.input(); // debounce 8 ms

        pins.select.set(true);
        bi.update(0); // candidate seen at 0
        bi.update(7); // 7 ms held: one short of the window → no commit
        assert!(bi.poll().is_none(), "7 ms < 8 ms window: not yet committed");
        bi.update(8); // exactly 8 ms: `now - since >= debounce_ms` → commit
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Select))), "8 ms >= window commits");
    }
}
