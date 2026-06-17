//! Host-side device-input emulation — the bridge from the control panel's
//! knob / PUSH / BACK widgets and keyboard to the shared gesture recognizer.
//!
//! The egui widgets in [`crate::gui`] push raw [`InputEvent`]s here each frame
//! (knob drag/scroll → [`InputEvent::Turn`] detents; PUSH/BACK press-hold and the
//! Enter/Backspace keys → button edges); [`DeviceInput::pump`] then drains them
//! through [`obcm_app::Gestures`] with a millis clock, producing the five
//! [`Gesture`]s. This is the brief's "real device-input emulation path" — the
//! existing GPS/camera widgets stay a separate dev tool. Screens (a later slice)
//! will consume the gestures; for now they drive the panel's readout so the
//! controls are visibly working.

use std::collections::VecDeque;
use std::f32::consts::{PI, TAU};
use std::time::Instant;

use obcm_app::{Button, ButtonEvent, Gesture, Gestures, InputEvent};

/// Radians of knob rotation per emitted detent (~15° ⇒ ~24 detents per turn,
/// a typical encoder). Shared by drag and scroll so both feel the same.
const DETENT_RADS: f32 = TAU / 24.0;
/// Scroll pixels per emitted detent.
const SCROLL_PER_DETENT: f32 = 24.0;

/// Accumulates raw control input and recognizes gestures from it. Owns the millis
/// clock (since construction) the shared recognizer needs.
pub struct DeviceInput {
    gestures: Gestures,
    start: Instant,
    /// Raw events queued by the widgets this frame, drained by [`pump`](Self::pump).
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
    /// The most recently recognized gesture, for the readout.
    last: Option<String>,
    /// Latest (encoder, back) hold-progress for the confirm-ring preview.
    progress: (f32, f32),
}

impl DeviceInput {
    pub fn new() -> Self {
        DeviceInput {
            gestures: Gestures::with_defaults(),
            start: Instant::now(),
            pending: VecDeque::new(),
            drag_angle: None,
            accum: 0.0,
            knob_angle: 0.0,
            enc_down: false,
            back_down: false,
            last: None,
            progress: (0.0, 0.0),
        }
    }

    /// Millis since construction — the shared recognizer's clock.
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

    /// Drain this frame's queued raw events through the recognizer, then fire any
    /// long-press that crossed its threshold and refresh hold-progress. Call once
    /// per frame (even with no new events — that's how a held button's long-press
    /// fires). Returns the gestures produced.
    pub fn pump(&mut self) -> Vec<Gesture> {
        let now = self.now_ms();
        let mut out = Vec::new();
        while let Some(ev) = self.pending.pop_front() {
            if let Some(g) = self.gestures.on_event(ev, now) {
                out.push(g);
            }
        }
        if let Some(g) = self.gestures.tick(now) {
            out.push(g);
        }
        if let Some(&g) = out.last() {
            self.last = Some(label(g));
        }
        self.progress = (self.gestures.encoder_progress(now), self.gestures.back_progress(now));
        out
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

    pub fn knob_angle(&self) -> f32 {
        self.knob_angle
    }
    pub fn encoder_progress(&self) -> f32 {
        self.progress.0
    }
    pub fn back_progress(&self) -> f32 {
        self.progress.1
    }
    pub fn last_gesture(&self) -> Option<&str> {
        self.last.as_deref()
    }
}

impl Default for DeviceInput {
    fn default() -> Self {
        Self::new()
    }
}

/// A short human label for a gesture (the on-screen readout / log).
pub fn label(g: Gesture) -> String {
    match g {
        Gesture::Turn(n) => format!("Turn {n:+}"),
        Gesture::Press => "Press".into(),
        Gesture::Hold => "Hold".into(),
        Gesture::Back => "Back".into(),
        Gesture::BackHold => "Back-hold".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_accumulates_into_whole_detents() {
        let mut d = DeviceInput::new();
        d.scroll(SCROLL_PER_DETENT * 2.5); // 2 detents now, 0.5 carried
        assert_eq!(d.pump(), vec![Gesture::Turn(2)]);
        d.scroll(SCROLL_PER_DETENT * 0.6); // 0.5 + 0.6 = 1.1 → 1 detent
        assert_eq!(d.pump(), vec![Gesture::Turn(1)]);
    }

    #[test]
    fn drag_emits_detents_in_the_turn_direction() {
        let mut d = DeviceInput::new();
        d.drag_to(0.0); // baseline, no detent
        d.drag_to(PI); // +180° ⇒ ~12 detents clockwise
        let total: i32 = d.pump().iter().map(|g| if let Gesture::Turn(n) = g { *n } else { 0 }).sum();
        assert!((11..=12).contains(&total), "half turn ≈ 12 detents, got {total}");
        // The opposite direction is negative.
        let mut d = DeviceInput::new();
        d.drag_to(0.0);
        d.drag_to(-PI);
        let total: i32 = d.pump().iter().map(|g| if let Gesture::Turn(n) = g { *n } else { 0 }).sum();
        assert!((-12..=-11).contains(&total), "got {total}");
    }

    #[test]
    fn set_button_only_emits_on_edges() {
        let mut d = DeviceInput::new();
        d.set_button(Button::Encoder, true);
        d.set_button(Button::Encoder, true); // no new edge
        d.set_button(Button::Encoder, false);
        // Down then Up within the same frame (< hold threshold) ⇒ one Press.
        assert_eq!(d.pump(), vec![Gesture::Press]);
    }
}
