//! Board-agnostic pushbutton input → the shared gesture recognizer.
//!
//! Turns raw GPIO levels into [`InputEvent`]s and implements [`InputSource`], so a board
//! drops it into the app's input handler, which runs the shared gesture recognizer — the same path
//! the host uses with its keyboard / on-screen controls. This only manufactures the raw events.
//!
//! The device's controls are four pushbuttons sharing one common pin — **UP** / **DOWN** on the
//! left flank, **SELECT** / **BACK** on the right — so:
//! - **UP** / **DOWN** emit signed selection steps — [`InputEvent::Step(-1)`] /
//!   [`InputEvent::Step(+1)`] — with auto-repeat while held, for fast menu scrolling.
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
//! Debounce and auto-repeat need a clock, but [`InputSource::poll`] is clockless, so the board calls
//! [`ButtonInput::update`] with the current wall-clock millis once per loop *before* `handle_input`;
//! `update` samples the pins and queues events, `poll` drains the queue. Injecting the clock keeps
//! the crate board-agnostic and host-testable.

use embedded_hal::digital::InputPin;
use heapless::Deque;
use obc_ports::{Button, ButtonEvent, InputEvent, InputSource};

/// Default contact-settle window (ms): a level must hold this long before its edge is reported.
/// 8 ms rejects switch bounce without a perceptible press delay.
pub const DEFAULT_DEBOUNCE_MS: u32 = 8;
/// Default delay before a held UP/DOWN starts auto-repeating (ms) — long enough that a single tap
/// never double-fires, short enough to feel responsive on a hold.
pub const DEFAULT_REPEAT_DELAY_MS: u32 = 350;
/// Default interval between auto-repeat steps while UP/DOWN stays held (ms) — ~8 steps/s.
pub const DEFAULT_REPEAT_INTERVAL_MS: u32 = 120;

/// Capacity of the event ring between [`ButtonInput::update`] and the app's drain. One `update`
/// queues at most one event per button (four) and the app drains to empty every frame, so this
/// never fills.
const QUEUE_LEN: usize = 8;

/// Debounce + auto-repeat timing. Set [`auto_repeat`](Timing::auto_repeat) `false` to make
/// UP/DOWN one step per press.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Contact-settle window for every button (ms).
    pub debounce_ms: u32,
    /// Whether holding UP/DOWN repeats the step (vs. one per press).
    pub auto_repeat: bool,
    /// Delay from press to the first auto-repeat step (ms).
    pub repeat_delay_ms: u32,
    /// Interval between subsequent auto-repeat steps while held (ms).
    pub repeat_interval_ms: u32,
}

impl Default for Timing {
    fn default() -> Self {
        Timing {
            debounce_ms: DEFAULT_DEBOUNCE_MS,
            auto_repeat: true,
            repeat_delay_ms: DEFAULT_REPEAT_DELAY_MS,
            repeat_interval_ms: DEFAULT_REPEAT_INTERVAL_MS,
        }
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
    /// Millis the next auto-repeat step is due for UP / DOWN, or `None` while up
    /// (or when [`Timing::auto_repeat`] is off).
    up_repeat: Option<u32>,
    down_repeat: Option<u32>,
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
            up_repeat: None,
            down_repeat: None,
            queue: Deque::new(),
        }
    }

    /// Sample all four pins at wall-clock `now_ms` and queue any resulting events.
    /// Call once per loop, before the app's input handler; the app then drains
    /// the queue via [`InputSource::poll`].
    pub fn update(&mut self, now_ms: u32) {
        let t = self.timing;
        // UP / DOWN emit signed selection steps, with auto-repeat while held.
        Self::step(&mut self.up, &mut self.up_repeat, -1, now_ms, &t, &mut self.queue);
        Self::step(&mut self.down, &mut self.down_repeat, 1, now_ms, &t, &mut self.queue);
        // SELECT / BACK forward debounced button edges; the shared Gestures layer
        // turns those + the clock into Press/Hold and Back/BackHold.
        Self::edge(&mut self.select, Button::Select, now_ms, t.debounce_ms, &mut self.queue);
        Self::edge(&mut self.back, Button::Back, now_ms, t.debounce_ms, &mut self.queue);
    }

    /// Whether nothing is in flight: every button is released + settled and the event queue is
    /// drained. The event-driven input plane polls at the loop rate only while *not* idle (a press
    /// debouncing, a hold repeating); once idle it sleeps on
    /// [`wait_for_any_press`](ButtonInput::wait_for_any_press). (Auto-repeat is disarmed on release,
    /// so a settled-released set implies no pending repeat.)
    pub fn is_idle(&self) -> bool {
        self.queue.is_empty()
            && self.up.settled_released()
            && self.down.settled_released()
            && self.select.settled_released()
            && self.back.settled_released()
    }

    /// UP/DOWN handling: a debounced press emits one step and arms auto-repeat; a
    /// release disarms it; while held, a step is emitted each time the repeat falls
    /// due (rebased to `now`, so a stalled loop emits one catch-up step, not a burst).
    fn step(
        btn: &mut Debounced<P>,
        repeat: &mut Option<u32>,
        dir: i32,
        now: u32,
        t: &Timing,
        queue: &mut Deque<InputEvent, QUEUE_LEN>,
    ) {
        match btn.update(now, t.debounce_ms) {
            Some(Edge::Press) => {
                push(queue, InputEvent::Step(dir));
                *repeat = t.auto_repeat.then(|| now.wrapping_add(t.repeat_delay_ms));
            }
            Some(Edge::Release) => *repeat = None,
            None => {
                if let Some(due) = *repeat {
                    // `due` reached? (wrap-tolerant: now ∈ [due, due + 2^31) ⇒ due).
                    if btn.pressed && now.wrapping_sub(due) < u32::MAX / 2 {
                        push(queue, InputEvent::Step(dir));
                        *repeat = Some(now.wrapping_add(t.repeat_interval_ms));
                    }
                }
            }
        }
    }

    /// SELECT/BACK handling: forward each debounced edge as a [`Button`] Down/Up — the
    /// shared `Gestures` layer derives Press/Hold (or Back/BackHold) from these.
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
    fn up_and_down_emit_signed_steps() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.down.set(true); // DOWN → +1
        bi.update(0);
        bi.update(8);
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));
        pins.down.set(false);
        bi.update(20);
        bi.update(28); // release is silent (no event)
        assert!(bi.poll().is_none());

        pins.up.set(true); // UP → -1
        bi.update(40);
        bi.update(48);
        assert_eq!(bi.poll(), Some(InputEvent::Step(-1)));
    }

    #[test]
    fn holding_down_auto_repeats_then_stops_on_release() {
        let pins = Pins::new();
        let mut bi = pins.input(); // defaults: delay 350, interval 120

        pins.down.set(true);
        bi.update(0);
        bi.update(8); // press → first step, repeat armed for 8 + 350 = 358
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));

        bi.update(100); // before the repeat delay
        assert!(bi.poll().is_none());
        bi.update(358); // first auto-repeat; next due 358 + 120 = 478
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));
        bi.update(478); // second auto-repeat
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));

        pins.down.set(false); // release
        bi.update(490);
        bi.update(498); // release commits → repeats disarmed
        bi.update(700); // long after: no more steps
        assert!(bi.poll().is_none());
    }

    #[test]
    fn auto_repeat_can_be_disabled() {
        let pins = Pins::new();
        let timing = Timing { auto_repeat: false, ..Timing::default() };
        let mut bi = ButtonInput::with_timing(
            MockPin { low: &pins.up },
            MockPin { low: &pins.down },
            MockPin { low: &pins.select },
            MockPin { low: &pins.back },
            timing,
        );
        pins.down.set(true);
        bi.update(0);
        bi.update(8); // one step on press
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));
        bi.update(1000); // still held, much later: no repeat
        assert!(bi.poll().is_none());
    }

    /// A stalled loop that only re-`update`s long after several repeat intervals must emit exactly
    /// ONE catch-up step (not one per missed interval) and rebase the next due time to `now` —
    /// else a long stall would dump a burst and the menu would jump wildly.
    #[test]
    fn a_stalled_loop_emits_one_catch_up_step_then_rearms() {
        let pins = Pins::new();
        let mut bi = pins.input(); // delay 350, interval 120

        pins.down.set(true);
        bi.update(0);
        bi.update(8); // press → first step, repeat armed for 8 + 350 = 358
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));

        // Loop stalls, then resumes at 10_000 — past `due` (358) by ~80 intervals.
        bi.update(10_000);
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)), "exactly one catch-up step");
        assert!(bi.poll().is_none(), "not a burst — only one step for the whole stall");

        // Rearmed relative to `now`: nothing is due before 10_000 + 120 = 10_120.
        bi.update(10_119);
        assert!(bi.poll().is_none(), "next step rebased to now + interval, not the old due");
        bi.update(10_120);
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)), "next interval fires off the rebased due");
    }

    /// The due-time comparison `now.wrapping_sub(due) < u32::MAX / 2` keeps auto-repeat firing
    /// across a u32-millis rollover (~49.7 days). A naive `now >= due` would suppress it forever
    /// after the wrap.
    #[test]
    fn auto_repeat_survives_a_millis_wrap() {
        let pins = Pins::new();
        let mut bi = pins.input(); // delay 350, interval 120

        let t0 = u32::MAX - 100; // press 100 ms before the rollover
        pins.down.set(true);
        bi.update(t0);
        bi.update(t0.wrapping_add(8)); // committed press; repeat armed for ~(t0+8)+350, which wraps
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));

        // `due` = (u32::MAX - 92).wrapping_add(350) = 257. After the wrap, now = 300 is
        // past due by 43 ms; wrapping_sub keeps that small and positive → step fires.
        bi.update(300);
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)), "repeat fires across the millis rollover");
    }

    /// SELECT held while DOWN taps must not block or swallow the DOWN steps — each button
    /// debounces independently. SELECT-down stays latched (no spurious repeat) while DOWN emits its
    /// own step.
    #[test]
    fn select_held_while_down_taps_keeps_both_independent() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.select.set(true); // hold SELECT down
        bi.update(0);
        bi.update(8); // SELECT commits → Down(Select)
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Select))));
        assert!(bi.poll().is_none());

        // DOWN taps while SELECT is still held — independent debounce, independent step.
        pins.down.set(true);
        bi.update(20);
        bi.update(28); // DOWN commits → Step(1); SELECT already latched, emits nothing
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)));
        assert!(bi.poll().is_none(), "held SELECT does not re-emit while DOWN taps");
    }

    /// UP and DOWN committed on the *same* `update(now)` both enqueue, in UP-then-DOWN order
    /// (the order `update` calls `step`), and drain intact.
    #[test]
    fn up_and_down_both_pressed_enqueue_in_call_order() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.up.set(true);
        pins.down.set(true);
        bi.update(0); // both candidates flip
        bi.update(8); // both commit in one update: UP (-1) then DOWN (+1)
        assert_eq!(bi.poll(), Some(InputEvent::Step(-1)), "UP is sampled first");
        assert_eq!(bi.poll(), Some(InputEvent::Step(1)), "DOWN second");
        assert!(bi.poll().is_none());
    }

    /// Force the "can't happen" queue overflow — never drain, hold all four buttons, pump
    /// auto-repeat past QUEUE_LEN=8 — and prove the queue caps at QUEUE_LEN, dropping the overflow
    /// rather than panicking or wrapping.
    #[test]
    fn queue_caps_at_eight_and_drops_overflow() {
        let pins = Pins::new();
        let mut bi = pins.input();

        // Hold all four down; never poll, so nothing drains.
        pins.up.set(true);
        pins.down.set(true);
        pins.select.set(true);
        pins.back.set(true);
        bi.update(0);
        bi.update(8); // 4 events queued (UP, DOWN, SELECT-down, BACK-down) — queue at 4 of 8

        // Pump UP/DOWN auto-repeat many times to push well past QUEUE_LEN=8; every push
        // beyond 8 hits the `let _ = push_back` drop path.
        for f in 1..20 {
            bi.update(8 + 350 + 120 * f); // repeated catch-up steps, never drained
        }

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
