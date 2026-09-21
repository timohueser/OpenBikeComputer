//! [`InputPlane`] — the input + overlay half of the two-plane device architecture.
//!
//! The map plane ([`App`]) owns the screen stack, camera, sensors and the expensive base-map
//! render. This plane owns everything that must stay responsive while a map frame is rendering: the
//! shared [`Gestures`] recogniser, the long-press [`HoldHints`] overlay, the live hold-progress and
//! the overlay render. The two couple only by a one-way flow of recognised [`Gesture`]s and
//! [`Chord`]s, so no lock the long map render holds can block input.
//!
//! On the firmware this plane runs on a high-priority interrupt executor that preempts the map
//! render every few milliseconds, so press-to-feedback latency stays bounded whatever a frame
//! costs. On the single-loop hosts it runs inline through
//! [`App::handle_input`](crate::App::handle_input). Either way the logic is this one struct.
//!
//! [`App`]: crate::App

use embedded_graphics::draw_target::DrawTarget;

use crate::hold_hint::HoldHints;
use crate::input::{Chord, Gesture, Gestures, DEFAULT_HOLD_MS};
use obc_ports::{InputClock, InputSource};

/// The high-priority input + overlay plane: gesture recognition, the long-press hint overlay, and
/// the live hold-progress readout.
///
/// Feed it raw input each frame with [`recognize`](InputPlane::recognize) and repaint the bulge
/// with [`render_overlay`](InputPlane::render_overlay). The plane holds no repaint mirror of its
/// own: the trailing-clear rule is one half of the pass's `OverlayKey`. It touches nothing the map
/// plane owns, so it is safe to run preemptively against a long map render.
pub struct InputPlane {
    gestures: Gestures,
    /// The long-press hint overlay, drawn above every screen on the dedicated overlay layer.
    hold_hints: HoldHints,
    /// In-flight Select / Back hold-progress (0.0–1.0) for the confirm ring.
    enc_progress: f32,
    back_progress: f32,
    last_gesture: Option<Gesture>,
    /// Millis at the last [`recognize`](InputPlane::recognize): the overlay's own clock, which is
    /// distinct from the map plane's.
    now_ms: u32,
}

impl InputPlane {
    pub fn new() -> Self {
        InputPlane {
            gestures: Gestures::new(DEFAULT_HOLD_MS),
            hold_hints: HoldHints::new(),
            enc_progress: 0.0,
            back_progress: 0.0,
            last_gesture: None,
            now_ms: 0,
        }
    }

    /// Drain this frame's raw input and advance hold timing at `clock`, invoking `on_gesture` for
    /// each recognised gesture in order, then fold the frame's hold-progress into the bulge.
    /// Recognition depends only on the raw events and the clock, never on app state, so the caller
    /// may apply each gesture inline or buffer them.
    ///
    /// Call it once per frame even with no pending events: that is how a held button's long-press
    /// fires at its threshold, how a deferred first step arrives, and how the bulge animates.
    ///
    /// Returns any device-wide [`Chord`] this frame's events completed. A chord is not a gesture:
    /// it never reaches a screen, so it comes back beside the callback rather than through it.
    pub fn recognize(
        &mut self,
        clock: InputClock,
        input: &mut dyn InputSource,
        mut on_gesture: impl FnMut(Gesture),
    ) -> Option<Chord> {
        let now_ms = clock.0;
        self.now_ms = now_ms;
        while let Some(ev) = input.poll() {
            if let Some(g) = self.gestures.on_event(ev, now_ms) {
                self.last_gesture = Some(g);
                on_gesture(g);
            }
        }
        // `tick` is the only source of Hold/BackHold — note which fired this frame so the hint
        // overlay pops the matching pill the instant the threshold crosses.
        let (mut enc_fired, mut back_fired) = (false, false);
        if let Some(g) = self.gestures.tick(now_ms) {
            match g {
                Gesture::Hold => enc_fired = true,
                Gesture::BackHold => back_fired = true,
                _ => {}
            }
            self.last_gesture = Some(g);
            on_gesture(g);
        }
        self.enc_progress = self.gestures.select_progress(now_ms);
        self.back_progress = self.gestures.back_progress(now_ms);
        let chord = self.gestures.take_chord();
        let assistant_progress = self
            .gestures
            .chord_remaining_ms(now_ms)
            .map_or(0.0, |remaining| 1.0 - remaining as f32 / DEFAULT_HOLD_MS as f32);
        self.hold_hints.update(
            now_ms,
            (self.enc_progress, enc_fired),
            (self.back_progress, back_fired),
            (assistant_progress, chord == Some(Chord::Assistant)),
        );
        chord
    }

    /// Render only the overlay plane, the transient hold bulge, over whatever is already in
    /// `target`. It paints only its own pixels and never clears the rest, so it is valid over an
    /// unchanged map.
    pub fn render_overlay<D, F>(&self, target: &mut D, w: f32, h: f32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.hold_hints.draw(target, &color_fn, w as i32, h as i32, self.now_ms);
    }

    /// Whether the overlay has live content right now. `false` exactly when
    /// [`render_overlay`](InputPlane::render_overlay) would paint nothing.
    pub fn overlay_active(&self) -> bool {
        self.hold_hints.active(self.now_ms)
    }

    /// The bounding rows `[y0, y0 + rows)` of the live hold bulge: the dirty region a
    /// partial-overlay host re-presents. `Some` exactly when
    /// [`overlay_active`](InputPlane::overlay_active) is `true`. `w` and `h` size the frame.
    pub fn overlay_rows(&self, w: i32, h: i32) -> Option<(u16, u16)> {
        self.hold_hints.active_rows(self.now_ms, w, h)
    }

    /// Cancel any in-flight hold. The map plane rings this after a gesture transitioned the screen
    /// stack, so a long-press charging over the old top cannot complete onto the new one.
    pub fn cancel_holds(&mut self) {
        self.gestures.cancel_holds();
    }

    /// Whether an ordinary hold or the Assistant chord is waiting for its threshold.
    pub fn hold_charging(&self) -> bool {
        self.enc_progress > 0.0 || self.back_progress > 0.0 || self.gestures.chord_remaining_ms(self.now_ms).is_some()
    }

    /// Time until a pending Assistant chord reaches its hold threshold.
    pub fn chord_remaining_ms(&self, now_ms: u32) -> Option<u32> {
        self.gestures.chord_remaining_ms(now_ms)
    }

    pub fn last_gesture(&self) -> Option<Gesture> {
        self.last_gesture
    }

    pub fn select_hold_progress(&self) -> f32 {
        self.enc_progress
    }

    pub fn back_hold_progress(&self) -> f32 {
        self.back_progress
    }
}

impl Default for InputPlane {
    fn default() -> Self {
        Self::new()
    }
}
