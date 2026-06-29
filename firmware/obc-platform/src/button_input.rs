//! Board-agnostic pushbutton input → the shared gesture recognizer.
//!
//! The on-device counterpart of the simulator's `obc-sim/src/device_input.rs`: it
//! turns raw GPIO levels into [`obc_app::InputEvent`]s and implements
//! [`InputSource`], so a board drops it straight into
//! [`App::handle_input`](obc_app), which runs the *shared*
//! [`Gestures`](obc_app::Gestures) layer — the exact path the host uses with its
//! knob/keyboard. The recognizer (and thus the five UI gestures, long-press timing
//! and the confirm ring/bulge) is reused **verbatim**; this only manufactures the
//! same raw events from four buttons instead of an encoder.
//!
//! The hardware is four pushbuttons sharing one common pin (no rotary encoder), so:
//! - **PREV** / **NEXT** synthesize encoder detents — [`InputEvent::Turn(-1)`] /
//!   [`InputEvent::Turn(+1)`] — with auto-repeat while held, for fast menu scrolling.
//! - **SELECT** forwards encoder [`Button`] edges → `Gestures` yields `Press` (short)
//!   / `Hold` (long).
//! - **BACK** forwards Back button edges → `Back` / `BackHold`.
//!
//! ## Wiring convention — active-low
//! Each switch connects its GPIO to the shared **GND** common pin, and the input uses
//! its **internal pull-up** (no external parts). So a released line reads high and a
//! press pulls it low: pressed ≡ [`InputPin::is_low`]. (This is also the natural
//! convention for the future nRF board, where this debouncer is reused.)
//!
//! ## Time
//! Debounce and auto-repeat need a clock, but [`InputSource::poll`] is clockless, so
//! the board calls [`ButtonInput::update`] with the current wall-clock millis (an
//! embassy `Instant` on the F429) once per loop *before* `handle_input`; `update`
//! samples the pins and queues events, and `poll` drains the queue. Injecting the
//! clock (rather than reaching for a timer here) keeps the crate board-agnostic and
//! host-testable.

use embedded_hal::digital::InputPin;
use heapless::Deque;
use obc_app::{Button, ButtonEvent, InputEvent, InputSource};

/// Default contact-settle window (ms): a level must hold for this long before its
/// edge is reported. The issue's 5–10 ms band; 8 ms rejects switch bounce without a
/// perceptible press delay.
pub const DEFAULT_DEBOUNCE_MS: u32 = 8;
/// Default delay before a held PREV/NEXT starts auto-repeating (ms) — long enough
/// that a single tap never double-fires, short enough to feel responsive on a hold.
pub const DEFAULT_REPEAT_DELAY_MS: u32 = 350;
/// Default interval between auto-repeat detents while PREV/NEXT stays held (ms) —
/// ~8 detents/s, smooth for scrolling a long menu.
pub const DEFAULT_REPEAT_INTERVAL_MS: u32 = 120;

/// Capacity of the event ring buffered between [`ButtonInput::update`] and the app's
/// drain. One `update` queues at most one event per button (four), and the app drains
/// to empty every frame, so this never fills — a couple of frames' slack is plenty.
const QUEUE_LEN: usize = 8;

/// Debounce + auto-repeat timing. [`Timing::default`] uses the `DEFAULT_*` constants;
/// set [`auto_repeat`](Timing::auto_repeat) `false` to make PREV/NEXT one detent per
/// press.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Contact-settle window for every button (ms).
    pub debounce_ms: u32,
    /// Whether holding PREV/NEXT repeats the detent (vs. one per press).
    pub auto_repeat: bool,
    /// Delay from press to the first auto-repeat detent (ms).
    pub repeat_delay_ms: u32,
    /// Interval between subsequent auto-repeat detents while held (ms).
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

/// One debounced active-low input. Tracks the committed (debounced) level plus the
/// last raw sample and when it appeared, so a level is committed — and its edge
/// reported — only once it has held steady for the debounce window.
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
        // Active-low: the switch shorts the line to the common GND, the internal
        // pull-up holds it high when released, so a *low* read is a press. A read
        // error is impossible on real GPIO (`Infallible`); treat any as "released".
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
}

/// Four pushbuttons → raw [`InputEvent`]s for the shared app. Generic over any
/// [`InputPin`], so the same type serves the nRF board (`embassy_nrf::gpio::Input`)
/// and the host test mock. Drive it as: [`update`](Self::update) once per loop with the
/// current millis, then hand `&mut self` to
/// [`App::handle_input`](obc_app) — which drains it through [`InputSource`].
pub struct ButtonInput<P> {
    prev: Debounced<P>,
    next: Debounced<P>,
    select: Debounced<P>,
    back: Debounced<P>,
    timing: Timing,
    /// Millis the next auto-repeat detent is due for PREV / NEXT, or `None` while up
    /// (or when [`Timing::auto_repeat`] is off).
    prev_repeat: Option<u32>,
    next_repeat: Option<u32>,
    queue: Deque<InputEvent, QUEUE_LEN>,
}

impl<P: InputPin> ButtonInput<P> {
    /// Build from the four pins (PREV, NEXT, SELECT, BACK) with [`Timing::default`].
    pub fn new(prev: P, next: P, select: P, back: P) -> Self {
        Self::with_timing(prev, next, select, back, Timing::default())
    }

    /// Build from the four pins with explicit [`Timing`].
    pub fn with_timing(prev: P, next: P, select: P, back: P, timing: Timing) -> Self {
        ButtonInput {
            prev: Debounced::new(prev),
            next: Debounced::new(next),
            select: Debounced::new(select),
            back: Debounced::new(back),
            timing,
            prev_repeat: None,
            next_repeat: None,
            queue: Deque::new(),
        }
    }

    /// Sample all four pins at wall-clock `now_ms` and queue any resulting events.
    /// Call once per loop, before [`App::handle_input`](obc_app); the app then drains
    /// the queue via [`InputSource::poll`].
    pub fn update(&mut self, now_ms: u32) {
        let t = self.timing;
        // PREV / NEXT synthesize encoder detents, with auto-repeat while held.
        Self::turn(&mut self.prev, &mut self.prev_repeat, -1, now_ms, &t, &mut self.queue);
        Self::turn(&mut self.next, &mut self.next_repeat, 1, now_ms, &t, &mut self.queue);
        // SELECT / BACK forward debounced button edges; the shared Gestures layer
        // turns those + the clock into Press/Hold and Back/BackHold.
        Self::edge(&mut self.select, Button::Encoder, now_ms, t.debounce_ms, &mut self.queue);
        Self::edge(&mut self.back, Button::Back, now_ms, t.debounce_ms, &mut self.queue);
    }

    /// Whether [`update`](Self::update) queued any events this sample (i.e. before the
    /// app drains them). A board's render loop can use this to redraw only on input.
    pub fn has_pending(&self) -> bool {
        !self.queue.is_empty()
    }

    /// PREV/NEXT handling: a debounced press emits one detent and arms auto-repeat; a
    /// release disarms it; while held, a detent is emitted each time the repeat falls
    /// due (rebased to `now`, so a stalled loop emits one catch-up detent, not a burst).
    fn turn(
        btn: &mut Debounced<P>,
        repeat: &mut Option<u32>,
        dir: i32,
        now: u32,
        t: &Timing,
        queue: &mut Deque<InputEvent, QUEUE_LEN>,
    ) {
        match btn.update(now, t.debounce_ms) {
            Some(Edge::Press) => {
                push(queue, InputEvent::Turn(dir));
                *repeat = t.auto_repeat.then(|| now.wrapping_add(t.repeat_delay_ms));
            }
            Some(Edge::Release) => *repeat = None,
            None => {
                if let Some(due) = *repeat {
                    // `due` reached? (wrap-tolerant: now ∈ [due, due + 2^31) ⇒ due).
                    if btn.pressed && now.wrapping_sub(due) < u32::MAX / 2 {
                        push(queue, InputEvent::Turn(dir));
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

    /// The four cells backing a [`ButtonInput`] of [`MockPin`]s, in PREV/NEXT/SELECT/
    /// BACK order — kept alive alongside the input the pins borrow from.
    struct Pins {
        prev: Cell<bool>,
        next: Cell<bool>,
        select: Cell<bool>,
        back: Cell<bool>,
    }
    impl Pins {
        fn new() -> Self {
            Pins { prev: Cell::new(false), next: Cell::new(false), select: Cell::new(false), back: Cell::new(false) }
        }
        fn input(&self) -> ButtonInput<MockPin<'_>> {
            ButtonInput::new(
                MockPin { low: &self.prev },
                MockPin { low: &self.next },
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
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Encoder))));
        assert!(bi.poll().is_none());

        pins.select.set(false); // release
        bi.update(20);
        bi.update(28); // settled → Up
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Encoder))));
        assert!(bi.poll().is_none());
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
    fn prev_and_next_emit_signed_detents() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.next.set(true); // NEXT → +1
        bi.update(0);
        bi.update(8);
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));
        pins.next.set(false);
        bi.update(20);
        bi.update(28); // release is silent (no event)
        assert!(bi.poll().is_none());

        pins.prev.set(true); // PREV → -1
        bi.update(40);
        bi.update(48);
        assert_eq!(bi.poll(), Some(InputEvent::Turn(-1)));
    }

    #[test]
    fn holding_next_auto_repeats_then_stops_on_release() {
        let pins = Pins::new();
        let mut bi = pins.input(); // defaults: delay 350, interval 120

        pins.next.set(true);
        bi.update(0);
        bi.update(8); // press → first detent, repeat armed for 8 + 350 = 358
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));

        bi.update(100); // before the repeat delay
        assert!(bi.poll().is_none());
        bi.update(358); // first auto-repeat; next due 358 + 120 = 478
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));
        bi.update(478); // second auto-repeat
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));

        pins.next.set(false); // release
        bi.update(490);
        bi.update(498); // release commits → repeats disarmed
        bi.update(700); // long after: no more detents
        assert!(bi.poll().is_none());
    }

    #[test]
    fn auto_repeat_can_be_disabled() {
        let pins = Pins::new();
        let timing = Timing { auto_repeat: false, ..Timing::default() };
        let mut bi = ButtonInput::with_timing(
            MockPin { low: &pins.prev },
            MockPin { low: &pins.next },
            MockPin { low: &pins.select },
            MockPin { low: &pins.back },
            timing,
        );
        pins.next.set(true);
        bi.update(0);
        bi.update(8); // one detent on press
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));
        bi.update(1000); // still held, much later: no repeat
        assert!(bi.poll().is_none());
    }

    /// Item 1 (catch-up/rebase, `turn` ~202-208): a render loop that stalls and only
    /// re-`update`s long *after* several repeat intervals have elapsed must emit exactly
    /// ONE catch-up detent (not one per missed interval) and rebase the next due time to
    /// `now`. Guards the `*repeat = Some(now.wrapping_add(interval))` rebase: if the code
    /// instead advanced `due` by `interval`, a long stall would dump a burst on the next
    /// frame and the menu would jump wildly.
    #[test]
    fn a_stalled_loop_emits_one_catch_up_detent_then_rearms() {
        let pins = Pins::new();
        let mut bi = pins.input(); // delay 350, interval 120

        pins.next.set(true);
        bi.update(0);
        bi.update(8); // press → first detent, repeat armed for 8 + 350 = 358
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));

        // Loop stalls, then resumes at 10_000 — past `due` (358) by ~80 intervals.
        bi.update(10_000);
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)), "exactly one catch-up detent");
        assert!(bi.poll().is_none(), "not a burst — only one detent for the whole stall");

        // Rearmed relative to `now`: nothing is due before 10_000 + 120 = 10_120.
        bi.update(10_119);
        assert!(bi.poll().is_none(), "next detent rebased to now + interval, not the old due");
        bi.update(10_120);
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)), "next interval fires off the rebased due");
    }

    /// Item 1 (millis wrap, `turn` ~205): the due-time comparison is
    /// `now.wrapping_sub(due) < u32::MAX / 2`, so auto-repeat must keep firing across a
    /// u32-millis rollover (~49.7 days of uptime). Arm the repeat just below `u32::MAX`,
    /// wrap `now` past 0, and assert the detent still fires — a naive `now >= due` would
    /// wrongly suppress it forever after the wrap.
    #[test]
    fn auto_repeat_survives_a_millis_wrap() {
        let pins = Pins::new();
        let mut bi = pins.input(); // delay 350, interval 120

        let t0 = u32::MAX - 100; // press 100 ms before the rollover
        pins.next.set(true);
        bi.update(t0);
        bi.update(t0.wrapping_add(8)); // committed press; repeat armed for ~(t0+8)+350, which wraps
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));

        // `due` = (u32::MAX - 92).wrapping_add(350) = 257. After the wrap, now = 300 is
        // past due by 43 ms; wrapping_sub keeps that small and positive → detent fires.
        bi.update(300);
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)), "repeat fires across the millis rollover");
    }

    /// Item 2 (overlapping presses): SELECT held while NEXT taps must not block or swallow
    /// the NEXT detents — each button debounces independently, so one `update(now)` can
    /// commit edges for several buttons. Proves the SELECT-down stays latched (no spurious
    /// repeat) while NEXT cleanly emits its own detent on a later frame.
    #[test]
    fn select_held_while_next_taps_keeps_both_independent() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.select.set(true); // hold SELECT down
        bi.update(0);
        bi.update(8); // SELECT commits → Down(Encoder)
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Encoder))));
        assert!(bi.poll().is_none());

        // NEXT taps while SELECT is still held — independent debounce, independent detent.
        pins.next.set(true);
        bi.update(20);
        bi.update(28); // NEXT commits → Turn(1); SELECT already latched, emits nothing
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)));
        assert!(bi.poll().is_none(), "held SELECT does not re-emit while NEXT taps");
    }

    /// Item 2 (simultaneous presses): PREV and NEXT pressed and committed on the *same*
    /// `update(now)` must both enqueue, in PREV-then-NEXT order (the order `update` calls
    /// `turn`), and drain intact. Proves a single frame can queue multiple events and the
    /// queue preserves their order.
    #[test]
    fn prev_and_next_both_down_enqueue_in_call_order() {
        let pins = Pins::new();
        let mut bi = pins.input();

        pins.prev.set(true);
        pins.next.set(true);
        bi.update(0); // both candidates flip
        bi.update(8); // both commit in one update: PREV (-1) then NEXT (+1)
        assert_eq!(bi.poll(), Some(InputEvent::Turn(-1)), "PREV is sampled first");
        assert_eq!(bi.poll(), Some(InputEvent::Turn(1)), "NEXT second");
        assert!(bi.poll().is_none());
    }

    /// Item 3 (overflow drop, `push` ~239, QUEUE_LEN=8): the queue silently drops on
    /// overflow ("can't happen in practice"). Force it to happen — never drain, hold all
    /// four buttons, and pump auto-repeat until well past 8 queued events — then prove the
    /// queue caps at exactly QUEUE_LEN and the overflow is dropped, not a panic or wraparound.
    #[test]
    fn queue_caps_at_eight_and_drops_overflow() {
        let pins = Pins::new();
        let mut bi = pins.input();

        // Hold all four down; never poll, so nothing drains.
        pins.prev.set(true);
        pins.next.set(true);
        pins.select.set(true);
        pins.back.set(true);
        bi.update(0);
        bi.update(8); // 4 events queued (PREV, NEXT, SELECT-down, BACK-down) — queue at 4 of 8

        // Pump PREV/NEXT auto-repeat many times to push well past QUEUE_LEN=8; every push
        // beyond 8 hits the `let _ = push_back` drop path.
        for f in 1..20 {
            bi.update(8 + 350 + 120 * f); // repeated catch-up detents, never drained
        }

        // Drain: exactly QUEUE_LEN events come out, the rest were dropped.
        let mut drained = 0;
        while bi.poll().is_some() {
            drained += 1;
        }
        assert_eq!(drained, QUEUE_LEN, "queue holds at most QUEUE_LEN; overflow is dropped, not panicked");
    }

    /// Item 4 (debounce boundary, `Debounced::update` ~116 uses `>=`): a level that has
    /// held for *exactly* `debounce_ms` commits (`>=`, not `>`). At `now - since == 8` the
    /// edge must fire on this very frame; at `== 7` it must not yet. Pins down the
    /// off-by-one a `>` would introduce (one frame of extra latency, or a press that
    /// never commits if updates always land exactly on the window).
    #[test]
    fn commit_lands_exactly_on_the_debounce_boundary() {
        let pins = Pins::new();
        let mut bi = pins.input(); // debounce 8 ms

        pins.select.set(true);
        bi.update(0); // candidate seen at 0
        bi.update(7); // 7 ms held: one short of the window → no commit
        assert!(bi.poll().is_none(), "7 ms < 8 ms window: not yet committed");
        bi.update(8); // exactly 8 ms: `now - since >= debounce_ms` → commit
        assert_eq!(bi.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Encoder))), "8 ms >= window commits");
    }
}
