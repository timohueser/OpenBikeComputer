//! [`InputPlane`] — the input + overlay half of the two-plane device architecture.
//!
//! The device has two cooperating planes (issue #48). The **map plane** ([`App`]) owns
//! the screen stack, the camera, the sensors and the expensive base-map render. This
//! **input plane** owns everything that must stay responsive *while a map frame is
//! rendering*: the shared [`Gestures`] recogniser, the long-press [`HoldHints`] overlay,
//! the live hold-progress, and the Layer-2 overlay render. The two are coupled only by a
//! one-way flow of recognised [`Gesture`]s — there is no shared lock the long map render
//! can hold against input.
//!
//! On the firmware the input plane runs on a **high-priority interrupt executor** that
//! preempts the CPU-bound map render every few milliseconds: it samples the buttons,
//! recognises gestures (pushing each into a channel the map plane drains), and animates +
//! repaints the hold bulge on its own LTDC layer — so the press-to-feedback latency and
//! the auto-repeat cadence stay bounded regardless of how long a map frame takes. On the
//! simulator (and the firmware's single-executor fallback) the same plane is driven inline
//! by [`App::handle_input`](crate::App::handle_input), which recognises and applies in one
//! place. Either way the recognition + overlay logic is *this one struct*, so host and
//! device behave identically.
//!
//! The split is deliberately a behaviour-preserving relocation of fields that used to live
//! on [`App`]: the recogniser, the hint overlay, `enc`/`back` hold-progress, the
//! last-recognised gesture, and the overlay's trailing-edge bookkeeping. [`App`] keeps one
//! [`InputPlane`] for the convenience path; the firmware's high-priority plane owns a
//! second, standalone one.
//!
//! [`App`]: crate::App

use embedded_graphics::draw_target::DrawTarget;

use crate::hal::{InputClock, InputSource};
use crate::hold_hint::HoldHints;
use crate::input::{Gesture, Gestures, DEFAULT_HOLD_MS};

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
    /// The global long-press hint overlay (the charge-in-place bulge at the encoder / Back
    /// edges), drawn above every screen on the dedicated overlay layer.
    hold_hints: HoldHints,
    /// In-flight encoder / Back hold-progress (0.0–1.0) for the confirm ring.
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

    /// Drain this frame's raw input + advance hold timing at `clock`, invoking `on_gesture`
    /// for each recognised gesture **in order**, then fold the frame's hold-progress into the
    /// bulge overlay.
    ///
    /// The caller decides what to do with each gesture: apply it straight away
    /// ([`App::handle_input`](crate::App::handle_input)) or push it into the cross-executor
    /// channel the map plane drains (the firmware's high-priority plane). Recognition depends
    /// only on the raw events + the clock — never on app state — so buffering the gestures and
    /// applying them after this returns is identical to applying them inline.
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
        self.enc_progress = self.gestures.encoder_progress(now_ms);
        self.back_progress = self.gestures.back_progress(now_ms);
        self.hold_hints.update(now_ms, self.enc_progress, self.back_progress, enc_fired, back_fired);
    }

    /// Render **only the overlay plane** — the transient hold bulge / confirm ring — over
    /// whatever is already in `target`, at the plane's own clock. Paints *only* its own
    /// pixels and never clears the rest of the target, so it is valid over an unchanged map
    /// (the compositing contract spelled out on
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

    /// Whether the overlay layer must be repainted this frame: while the bulge is live, plus
    /// exactly one trailing frame after it goes quiet so the last bulge can be cleared off the
    /// layer. The trailing edge is tracked across calls, so call this **once per frame**.
    pub fn take_overlay_dirty(&mut self) -> bool {
        let now = self.overlay_active();
        let dirty = now || self.overlay_was_active;
        self.overlay_was_active = now;
        dirty
    }

    /// The most recently recognized gesture (host input readout), if any.
    pub fn last_gesture(&self) -> Option<Gesture> {
        self.last_gesture
    }

    /// In-flight encoder hold-progress (0.0–1.0) for the confirm-ring readout.
    pub fn encoder_hold_progress(&self) -> f32 {
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
