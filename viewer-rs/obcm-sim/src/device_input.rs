//! Host-side device-input emulation — the control panel's knob / PUSH / BACK
//! widgets and keyboard, turned into raw [`InputEvent`]s for the app.
//!
//! The egui widgets in [`crate::gui`] push raw events here each frame (knob
//! drag/scroll → [`InputEvent::Turn`] detents; PUSH/BACK press-hold and the
//! Enter/Backspace keys → button edges). [`DeviceInput`] implements
//! [`InputSource`], so it drops straight into [`obcm_app::App::handle_input`],
//! which runs the *shared* gesture recognizer and dispatches the gestures to the
//! screen stack — the exact path the firmware uses with real GPIO. This is the
//! brief's "real device-input emulation path"; the existing GPS/camera widgets
//! stay a separate dev tool. It also owns the millis clock (since construction)
//! and the visual knob angle for drawing.

use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};
use std::time::Instant;

use obcm_app::{Button, ButtonEvent, InputEvent, InputSource};

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
    /// Pointer angle (rad) at the previous drag sample while turning the knob.
    drag_angle: Option<f32>,
    /// Sub-detent rotation accumulator (drag + scroll), in radians.
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
            drag_angle: None,
            accum: 0.0,
            knob_angle: 0.0,
            enc_down: false,
            back_down: false,
        }
    }

    /// Millis since construction — the clock passed to [`obcm_app::App::handle_input`].
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

    /// Feed a knob drag sample: `angle` is the pointer's angle (rad) about the
    /// knob center. Accumulates rotation and emits whole detents.
    pub fn drag_to(&mut self, angle: f32) {
        if let Some(prev) = self.drag_angle {
            let mut d = angle - prev;
            while d > PI {
                d -= TAU;
            }
            while d < -PI {
                d += TAU;
            }
            self.accum += d;
            let n = self.take_detents();
            self.turn(n);
        }
        self.drag_angle = Some(angle);
    }

    /// End a knob drag (pointer released or left the knob).
    pub fn end_drag(&mut self) {
        self.drag_angle = None;
        self.accum = 0.0;
    }

    /// Feed a scroll delta over the knob (egui `smooth_scroll_delta.y`); scroll up
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

    /// The visual knob angle (rad) for drawing the pointer notch.
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
    fn drag_emits_detents_in_the_turn_direction() {
        let mut d = DeviceInput::new();
        d.drag_to(0.0); // baseline, no detent
        d.drag_to(PI); // +180° ⇒ ~12 detents clockwise
        assert!((11..=12).contains(&drain_turns(&mut d)), "half turn ≈ 12 detents");

        let mut d = DeviceInput::new();
        d.drag_to(0.0);
        d.drag_to(-PI); // the opposite direction is negative
        assert!((-12..=-11).contains(&drain_turns(&mut d)));
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
}
