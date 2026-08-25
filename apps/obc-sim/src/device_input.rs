//! Host-side device-input emulation — the four on-housing buttons (Up / Down / Select / Back)
//! and the keyboard, turned into raw [`InputEvent`]s for the app.
//!
//! [`crate::gui`] pushes raw events here each frame. [`DeviceInput`] implements
//! [`InputSource`], so it drops straight into [`obc_app::App::handle_input`] and its
//! *shared* gesture recognizer — the exact path the firmware uses with real GPIO. It also
//! owns the millis clock, and it fills in the one thing a host has that the device doesn't: a
//! mouse wheel, folded into the same signed [`InputEvent::Step`]s the Up/Down buttons emit.

use std::collections::VecDeque;

use std::time::Instant;

use obc_ports::{Button, ButtonEvent, InputEvent, InputSource};

/// Scroll pixels per emitted selection step — the wheel over the screen stands in for tapping
/// Up/Down, so one notch of a typical mouse wheel is one step.
const SCROLL_PER_STEP: f32 = 24.0;

/// Accumulates raw control input into a queue the app drains via [`InputSource`].
/// Owns the millis clock.
pub struct DeviceInput {
    start: Instant,
    /// Raw events queued by the widgets this frame, drained by [`poll`](InputSource::poll).
    pending: VecDeque<InputEvent>,
    /// Sub-step scroll accumulator, in fractional steps.
    accum: f32,
    /// Debounced button-held state, so we only emit edges on transitions.
    select_down: bool,
    back_down: bool,
}

impl DeviceInput {
    pub fn new() -> Self {
        DeviceInput {
            start: Instant::now(),
            pending: VecDeque::new(),
            accum: 0.0,
            select_down: false,
            back_down: false,
        }
    }

    /// Millis since construction — the clock passed to [`obc_app::App::handle_input`].
    pub fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// Queue `steps` of selection movement as a [`InputEvent::Step`] event
    /// (negative = Up / "previous", positive = Down / "next").
    pub fn step(&mut self, steps: i32) {
        if steps != 0 {
            self.pending.push_back(InputEvent::Step(steps));
        }
    }

    /// Feed a mouse-wheel delta (egui `smooth_scroll_delta.y`); scrolling **down** the list is
    /// "next", matching the Down button. Emits whole steps, carrying the remainder.
    pub fn scroll(&mut self, dy: f32) {
        self.accum += dy / SCROLL_PER_STEP;
        let n = self.take_steps();
        self.step(n);
    }

    /// Set a button's held state from the widget/keyboard; emits a Down/Up edge
    /// only on a transition, so holding produces exactly one Down.
    pub fn set_button(&mut self, b: Button, down: bool) {
        let cur = match b {
            Button::Select => &mut self.select_down,
            Button::Back => &mut self.back_down,
        };
        if *cur != down {
            *cur = down;
            let edge = if down { ButtonEvent::Down(b) } else { ButtonEvent::Up(b) };
            self.pending.push_back(InputEvent::Button(edge));
        }
    }

    /// Drop queued constituent input when a two-button drawer chord wins. The shared recognizer is
    /// cancelled by the App hook at the same edge; suppressing releases here prevents a chord from
    /// turning into a stray Select/Back tap on the newly opened drawer.
    pub fn cancel_buttons(&mut self) {
        self.pending.clear();
        self.select_down = false;
        self.back_down = false;
    }

    /// Pull whole steps out of the scroll accumulator, keeping the remainder.
    fn take_steps(&mut self) -> i32 {
        let n = self.accum.trunc();
        self.accum -= n;
        n as i32
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

    /// Drain every pending event, summing the `Step` counts.
    fn drain_steps(d: &mut DeviceInput) -> i32 {
        let mut total = 0;
        while let Some(ev) = d.poll() {
            if let InputEvent::Step(n) = ev {
                total += n;
            }
        }
        total
    }

    #[test]
    fn scroll_accumulates_into_whole_steps() {
        let mut d = DeviceInput::new();
        d.scroll(SCROLL_PER_STEP * 2.5); // 2 steps now, 0.5 carried
        assert_eq!(d.poll(), Some(InputEvent::Step(2)));
        assert_eq!(d.poll(), None);
        d.scroll(SCROLL_PER_STEP * 0.6); // 0.5 + 0.6 = 1.1 → 1 step
        assert_eq!(d.poll(), Some(InputEvent::Step(1)));
    }

    #[test]
    fn scroll_direction_sets_the_step_sign() {
        let mut d = DeviceInput::new();
        d.scroll(SCROLL_PER_STEP * 3.0);
        assert_eq!(drain_steps(&mut d), 3, "scroll up ⇒ positive steps");
        d.scroll(-SCROLL_PER_STEP * 2.0);
        assert_eq!(drain_steps(&mut d), -2, "scroll down ⇒ negative steps");
    }

    #[test]
    fn set_button_only_emits_on_edges() {
        let mut d = DeviceInput::new();
        d.set_button(Button::Select, true);
        d.set_button(Button::Select, true); // no transition → no second edge
        d.set_button(Button::Select, false);
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Select))));
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Select))));
        assert_eq!(d.poll(), None);
    }

    #[test]
    fn cancelling_a_chord_drops_its_edges_and_release() {
        let mut d = DeviceInput::new();
        d.set_button(Button::Select, true);
        d.cancel_buttons();
        assert_eq!(d.poll(), None);

        d.set_button(Button::Select, false);
        assert_eq!(d.poll(), None, "the physical release is not a tap on the opened drawer");
    }

    /// A zero-count step must queue nothing — a frame with no Up/Down activity mustn't emit a
    /// spurious `Step(0)` (which would still wake the app / redraw).
    #[test]
    fn step_zero_is_a_no_op() {
        let mut d = DeviceInput::new();
        d.step(0);
        assert_eq!(d.poll(), None, "zero steps queues no event");
    }

    /// The accumulator must carry its *negative* remainder across `scroll` calls, as the
    /// positive path does — else partial scrolls down a list silently lose motion.
    #[test]
    fn negative_scroll_carries_the_remainder() {
        let mut d = DeviceInput::new();
        d.scroll(-SCROLL_PER_STEP * 1.5); // -1.5 steps → one -1 now, -0.5 carried
        assert_eq!(d.poll(), Some(InputEvent::Step(-1)));
        assert_eq!(d.poll(), None, "only one whole step so far");
        d.scroll(-SCROLL_PER_STEP * 0.6); // -0.5 + -0.6 = -1.1 → one more -1
        assert_eq!(d.poll(), Some(InputEvent::Step(-1)), "carried remainder completes the next step");
        assert_eq!(d.poll(), None);
    }

    /// Driving *only* Back must toggle the Back field and emit Back edges, leaving Select
    /// untouched — a swapped-field bug (Back writing `select_down`) would surface as a wrong or
    /// missing edge.
    #[test]
    fn back_button_is_independent_of_select() {
        let mut d = DeviceInput::new();

        // Back down then up — emits Back edges, never Select.
        d.set_button(Button::Back, true);
        d.set_button(Button::Back, true); // no transition
        d.set_button(Button::Back, false);
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Back))));
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Up(Button::Back))));
        assert_eq!(d.poll(), None, "Back edges only — the Select field is never touched");

        // Select still starts from 'up': its first set is a fresh Down edge, proving the
        // earlier Back activity did not flip `select_down`.
        d.set_button(Button::Select, true);
        assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(Button::Select))));
        assert_eq!(d.poll(), None);
    }
}
