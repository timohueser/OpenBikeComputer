//! Host-side device-input emulation — the on-housing encoder / Back controls and the
//! keyboard, turned into raw [`InputEvent`]s for the app.
//!
//! [`crate::gui`] pushes raw events here each frame. [`DeviceInput`] implements
//! [`InputSource`], so it drops straight into [`obc_app::App::handle_input`] and its
//! *shared* gesture recognizer — the exact path the firmware uses with real GPIO. It also
//! owns the millis clock and the visual knob angle for drawing the wheel.

use std::collections::VecDeque;
use std::f32::consts::TAU;

// web_time::Instant is std's on native and a JS-clock shim on wasm (std's panics
// in the browser), so the device millis clock works in the web build unchanged.
use web_time::Instant;

use obc_app::{Button, ButtonEvent, InputEvent, InputSource};

/// Radians of knob rotation per emitted detent (~15° ⇒ ~24 detents per turn,
/// a typical encoder). Shared by drag and scroll so both feel the same.
const DETENT_RADS: f32 = TAU / 24.0;
/// Scroll pixels per emitted detent.
const SCROLL_PER_DETENT: f32 = 24.0;

/// Accumulates raw control input into a queue the app drains via [`InputSource`].
/// Owns the millis clock and the visual knob angle.
pub struct DeviceInput {
    start: Instant,
    /// Raw events queued by the widgets this frame, drained by [`poll`](InputSource::poll).
    pending: VecDeque<InputEvent>,
    /// Sub-detent rotation accumulator (from scroll), in radians.
    accum: f32,
    /// Visual knob angle (rad), stepped one detent per emitted detent.
    knob_angle: f32,
    /// Debounced button-held state, so we only emit edges on transitions.
    enc_down: bool,
    back_down: bool,
}

impl DeviceInput {
    pub fn new() -> Self {
        DeviceInput {
            start: Instant::now(),
            pending: VecDeque::new(),
            accum: 0.0,
            knob_angle: 0.0,
            enc_down: false,
            back_down: false,
        }
    }

    /// Millis since construction — the clock passed to [`obc_app::App::handle_input`].
    pub fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// Queue `detents` of rotation as a `Turn` event and step the visual knob
    /// (positive = clockwise / "next").
    pub fn turn(&mut self, detents: i32) {
        if detents != 0 {
            self.knob_angle += detents as f32 * DETENT_RADS;
            self.pending.push_back(InputEvent::Turn(detents));
        }
    }

    /// Feed a scroll delta over the wheel (egui `smooth_scroll_delta.y`); scroll up
    /// is "next" (clockwise). Emits whole detents.
    pub fn scroll(&mut self, dy: f32) {
        self.accum += dy / SCROLL_PER_DETENT * DETENT_RADS;
        let n = self.take_detents();
        self.turn(n);
    }

    /// Set a button's held state from the widget/keyboard; emits a Down/Up edge
    /// only on a transition, so holding produces exactly one Down.
    pub fn set_button(&mut self, b: Button, down: bool) {
        let cur = match b {
            Button::Encoder => &mut self.enc_down,
            Button::Back => &mut self.back_down,
        };
        if *cur != down {
            *cur = down;
            let edge = if down { ButtonEvent::Down(b) } else { ButtonEvent::Up(b) };
            self.pending.push_back(InputEvent::Button(edge));
        }
    }

    /// Pull whole detents out of the rotation accumulator, keeping the remainder.
    fn take_detents(&mut self) -> i32 {
        let mut n = 0;
        while self.accum >= DETENT_RADS {
            self.accum -= DETENT_RADS;
            n += 1;
        }
        while self.accum <= -DETENT_RADS {
            self.accum += DETENT_RADS;
            n -= 1;
        }
        n
    }

    /// The visual knob angle (rad) for drawing the wheel's knurl scroll.
    pub fn knob_angle(&self) -> f32 {
        self.knob_angle
    }
}

impl InputSource for DeviceInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.pending.pop_front()
    }
}

impl Default for DeviceInput {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drain every pending event, summing the `Turn` detents.
    fn drain_turns(d: &mut DeviceInput) -> i32 {
        let mut total = 0;
        while let Some(ev) = d.poll() {
            if let InputEvent::Turn(n) = ev {
                total += n;
            }
        }
        total
    }

    #[test]
    fn scroll_accumulates_into_whole_detents() {
        let mut d = DeviceInput::new();
        d.scroll(SCROLL_PER_DETENT * 2.5); // 2 detents now, 0.5 carried
        assert_eq!(d.poll(), Some(InputEvent::Turn(2)));
        assert_eq!(d.poll(), None);
        d.scroll(SCROLL_PER_DETENT * 0.6); // 0.5 + 0.6 = 1.1 → 1 detent
        assert_eq!(d.poll(), Some(InputEvent::Turn(1)));
    }

    #[test]
    fn scroll_direction_sets_turn_sign() {
        let mut d = DeviceInput::new();
        d.scroll(SCROLL_PER_DETENT * 3.0);
        assert_eq!(drain_turns(&mut d), 3, "scroll up ⇒ positive detents");
        d.scroll(-SCROLL_PER_DETENT * 2.0);
        assert_eq!(drain_turns(&mut d), -2, "scroll down ⇒ negative detents");
    }

    #[test]
    fn set_button_only_emits_on_edges() {
        let mut d = DeviceInput::new();
        d.set_button(Button::Encoder, true);
        d.set_button(Button::Encoder, true); // no transition → no second edge
        d.set_button(Button::Encoder, false);
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Encoder))));
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Encoder))));
        assert_eq!(d.poll(), None);
    }

    /// A zero-detent turn must queue nothing and leave the knob angle untouched — a frame with
    /// no rotation mustn't emit a spurious `Turn(0)` (which would still wake the app / redraw).
    #[test]
    fn turn_zero_is_a_no_op() {
        let mut d = DeviceInput::new();
        let before = d.knob_angle();
        d.turn(0);
        assert_eq!(d.poll(), None, "zero detents queues no event");
        assert_eq!(d.knob_angle(), before, "knob angle unchanged by a zero turn");
    }

    /// The accumulator must carry its *negative* remainder across `scroll` calls, as the
    /// positive path does — else partial scrolls down a list silently lose motion.
    #[test]
    fn negative_scroll_carries_the_remainder() {
        let mut d = DeviceInput::new();
        d.scroll(-SCROLL_PER_DETENT * 1.5); // -1.5 detents → one -1 now, -0.5 carried
        assert_eq!(d.poll(), Some(InputEvent::Turn(-1)));
        assert_eq!(d.poll(), None, "only one whole detent so far");
        d.scroll(-SCROLL_PER_DETENT * 0.6); // -0.5 + -0.6 = -1.1 → one more -1
        assert_eq!(d.poll(), Some(InputEvent::Turn(-1)), "carried remainder completes the next detent");
        assert_eq!(d.poll(), None);
    }

    /// Driving *only* Back must toggle the Back field and emit Back edges, leaving Encoder
    /// untouched — a swapped-field bug (Back writing `enc_down`) would surface as a wrong or
    /// missing edge.
    #[test]
    fn back_button_is_independent_of_encoder() {
        let mut d = DeviceInput::new();

        // Back down then up — emits Back edges, never Encoder.
        d.set_button(Button::Back, true);
        d.set_button(Button::Back, true); // no transition
        d.set_button(Button::Back, false);
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Back))));
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Back))));
        assert_eq!(d.poll(), None, "Back edges only — Encoder field never touched");

        // Encoder still starts from 'up': its first set is a fresh Down edge, proving the
        // earlier Back activity did not flip enc_down.
        d.set_button(Button::Encoder, true);
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Encoder))));
        assert_eq!(d.poll(), None);
    }
}
