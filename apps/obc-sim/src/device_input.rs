//! Host-side device-input emulation — the four on-housing buttons (Up / Down / Select / Back)
//! and the keyboard, turned into raw [`InputEvent`]s for the app.
//!
//! [`crate::gui`] pushes raw events here each frame. [`DeviceInput`] implements
//! [`InputSource`], so it drops straight into [`obc_app::App::handle_input`] and its
//! *shared* gesture recognizer — the exact path the firmware uses with real GPIO. All four
//! controls arrive here as held state and leave as edges, so a held arrow key auto-repeats through
//! the recognizer's own cadence rather than the host's. It also owns the millis clock, and it fills
//! in the one thing a host has that the device doesn't: a mouse wheel, folded into a directly
//! injected [`InputEvent::Step`].

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
    /// Held state of each of the four controls, so we only emit edges on transitions.
    up_down: bool,
    down_down: bool,
    select_down: bool,
    back_down: bool,
}

impl DeviceInput {
    pub fn new() -> Self {
        DeviceInput {
            start: Instant::now(),
            pending: VecDeque::new(),
            accum: 0.0,
            up_down: false,
            down_down: false,
            select_down: false,
            back_down: false,
        }
    }

    /// Millis since construction — the clock passed to [`obc_app::App::handle_input`].
    pub fn now_ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    /// Queue `steps` of selection movement as a directly injected [`InputEvent::Step`] event
    /// (negative = Up / "previous", positive = Down / "next") — the wheel and the one-shot
    /// keyboard aliases, which model no button.
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
            Button::Up => &mut self.up_down,
            Button::Down => &mut self.down_down,
            Button::Select => &mut self.select_down,
            Button::Back => &mut self.back_down,
        };
        if *cur != down {
            *cur = down;
            let edge = if down { ButtonEvent::Down(b) } else { ButtonEvent::Up(b) };
            self.pending.push_back(InputEvent::Button(edge));
        }
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

    /// Each of the four controls carries its own held flag: a repeated `true` emits no second
    /// edge, and driving one never disturbs the other three (the four-arm match is where a
    /// swapped field would hide).
    #[test]
    fn set_button_only_emits_on_edges() {
        for b in [Button::Up, Button::Down, Button::Select, Button::Back] {
            let mut d = DeviceInput::new();
            d.set_button(b, true);
            d.set_button(b, true); // no transition → no second edge
            d.set_button(b, false);
            assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Down(b))));
            assert_eq!(d.poll(), Some(InputEvent::Button(ButtonEvent::Up(b))));
            assert_eq!(d.poll(), None, "{b:?} emitted an edge for another button");
        }
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
