//! [`InputPlane`] — the input + overlay half of the two-plane device architecture.
//!
//! The **map plane** ([`App`]) owns the screen stack, camera, sensors and the expensive base-map
//! render. This **input plane** owns everything that must stay responsive *while a map frame is
//! rendering*: the shared [`Gestures`] recogniser, the long-press [`HoldHints`] overlay, the live
//! hold-progress, and the Layer-2 overlay render. The two couple only by a one-way flow of
//! recognised [`Gesture`]s — no shared lock the long map render can hold against input.
//!
//! On the firmware this plane runs on a **high-priority interrupt executor** that preempts the
//! CPU-bound map render every few milliseconds: it samples the buttons, recognises gestures (into a
//! channel the map plane drains), and animates the hold bulge on its own overlay layer — so
//! press-to-feedback latency stays bounded regardless of map-frame length. On the simulator (and
//! the firmware's single-executor fallback) the same plane runs inline via
//! [`App::handle_input`](crate::App::handle_input). Either way the logic is *this one struct*, so
//! host and device behave identically.
//!
//! [`App`]: crate::App

use embedded_graphics::draw_target::DrawTarget;

use crate::hold_hint::HoldHints;
use crate::input::{Gesture, Gestures, DEFAULT_HOLD_MS};
use obc_ports::{InputClock, InputSource};

/// The high-priority input + overlay plane: gesture recognition, the long-press hint
/// overlay, and the live hold-progress readout.
///
/// Feed it raw input each frame with [`recognize`](InputPlane::recognize) (it emits
/// [`Gesture`]s and advances the bulge), repaint the bulge with
/// [`render_overlay`](InputPlane::render_overlay) whenever
/// [`take_overlay_dirty`](InputPlane::take_overlay_dirty) reports a change, and read the
/// confirm-ring progress / last gesture for a host readout. It touches **nothing** the map
/// plane owns, so it is safe to run preemptively against a long map render.
pub struct InputPlane {
    /// The shared gesture recognizer (raw events + clock → the five gestures).
    gestures: Gestures,
    /// The global long-press hint overlay (the charge-in-place bulge at the Select / Back
    /// edges), drawn above every screen on the dedicated overlay layer.
    hold_hints: HoldHints,
    /// In-flight Select / Back hold-progress (0.0–1.0) for the confirm ring.
    enc_progress: f32,
    back_progress: f32,
    /// The most recently recognized gesture, for the host's input readout.
    last_gesture: Option<Gesture>,
    /// Millis at the last [`recognize`](InputPlane::recognize) — the overlay's own clock
    /// (the input/wall clock), distinct from the map plane's [`App`](crate::App) clock.
    now_ms: u32,
    /// The overlay's live state at the previous [`take_overlay_dirty`](InputPlane::take_overlay_dirty),
    /// so a bulge going quiet yields exactly one trailing repaint (clearing the last frame off
    /// the overlay layer).
    overlay_was_active: bool,
}

impl InputPlane {
    /// A fresh plane with the [`DEFAULT_HOLD_MS`] long-press threshold and nothing charging.
    pub fn new() -> Self {
        InputPlane {
            gestures: Gestures::new(DEFAULT_HOLD_MS),
            hold_hints: HoldHints::new(),
            enc_progress: 0.0,
            back_progress: 0.0,
            last_gesture: None,
            now_ms: 0,
            overlay_was_active: false,
        }
    }

    /// Drain this frame's raw input + advance hold timing at `clock`, invoking `on_gesture` for
    /// each recognised gesture **in order**, then fold the frame's hold-progress into the bulge.
    ///
    /// Recognition depends only on the raw events + the clock — never on app state — so the caller
    /// may apply each gesture inline or buffer them into a channel; both are identical.
    ///
    /// Call once per frame even with no pending events: that is how a held button's long-press
    /// fires at its threshold and how the bulge animates while charging.
    pub fn recognize(&mut self, clock: InputClock, input: &mut dyn InputSource, mut on_gesture: impl FnMut(Gesture)) {
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
        self.hold_hints.update(now_ms, self.enc_progress, self.back_progress, enc_fired, back_fired);
    }

    /// Render **only the overlay plane** — the transient hold bulge / confirm ring — over whatever
    /// is already in `target`, at the plane's own clock. Paints *only* its own pixels and never
    /// clears the rest, so it is valid over an unchanged map (the compositing contract on
    /// [`App::render_overlay`](crate::App::render_overlay)).
    pub fn render_overlay<D, F>(&self, target: &mut D, w: f32, h: f32, color_fn: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        self.hold_hints.draw(target, &color_fn, w as i32, h as i32, self.now_ms);
    }

    /// Whether the overlay has live content right now — a bulge charging, popping, or
    /// retracting. `false` exactly when [`render_overlay`](InputPlane::render_overlay) would
    /// paint nothing, so the overlay layer can stay idle.
    pub fn overlay_active(&self) -> bool {
        self.hold_hints.active(self.now_ms)
    }

    /// The bounding rows `[y0, y0 + rows)` of the live hold bulge — the dirty region a
    /// partial-overlay host re-presents, so it can re-push only the active bulge's rows. `Some`
    /// exactly when [`overlay_active`](InputPlane::overlay_active) is `true`. `w`/`h` size the frame
    /// the bulge is anchored in.
    pub fn overlay_rows(&self, w: i32, h: i32) -> Option<(u16, u16)> {
        self.hold_hints.active_rows(self.now_ms, w, h)
    }

    /// Whether the overlay layer must be repainted this frame: while the bulge is live, plus
    /// exactly one trailing frame after it goes quiet so the last bulge can be cleared off the
    /// layer. The trailing edge is tracked across calls, so call this **once per frame**.
    pub fn take_overlay_dirty(&mut self) -> bool {
        let now = self.overlay_active();
        let dirty = now || self.overlay_was_active;
        self.overlay_was_active = now;
        dirty
    }

    /// Cancel any in-flight hold (see [`Gestures::cancel_holds`]). The map plane rings this after
    /// a gesture **transitioned the screen stack** ([`App::take_hold_cancel`](crate::App::take_hold_cancel)),
    /// so a long-press that was charging over the old top can't complete onto the new one. The
    /// bulge retracts on the next [`recognize`](InputPlane::recognize) — a cancelled hold's
    /// progress reads 0.
    pub fn cancel_holds(&mut self) {
        self.gestures.cancel_holds();
    }

    /// The most recently recognized gesture (host input readout), if any.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.last_gesture
    }

    /// In-flight Select hold-progress (0.0–1.0) for the confirm-ring readout.
    pub fn select_hold_progress(&self) -> f32 {
        self.enc_progress
    }

    /// In-flight Back hold-progress (0.0–1.0).
    pub fn back_hold_progress(&self) -> f32 {
        self.back_progress
    }
}

impl Default for InputPlane {
    fn default() -> Self {
        Self::new()
    }
}
