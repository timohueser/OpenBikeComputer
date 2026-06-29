//! OBC device application layer + hardware-abstraction traits.
//!
//! `no_std`, so the **same** logic runs in the desktop simulator and on the nRF54L
//! firmware. It owns *what the device is doing* — where the user is, where the camera
//! looks, which mode it's in — and leaves *how pixels reach a screen* to the host. It adds
//! no allocations of its own (the only heap use is the
//! [`MapRenderer`](obc_render::MapRenderer) scratch, which clears-not-frees each frame).
//!
//! The boundary is a small hardware-abstraction layer (HAL): the app reads position from a
//! [`LocationSource`] and buttons from an [`InputSource`], not caring whether those are a real
//! GPS chip + GPIO (firmware) or the simulator's control panel + GPX replay (host). The host
//! injects an implementation; the app stays identical.
//!
//! Modules:
//! - [`hal`] — the injected-hardware traits ([`LocationSource`], [`InputSource`])
//!   and their data types ([`Fix`], [`Button`], [`ButtonEvent`], [`InputEvent`]).
//! - [`input`] — the shared gesture recognizer ([`Gestures`]) turning raw
//!   [`InputEvent`]s + a millis clock into the five UI [`Gesture`]s.
//! - [`input_plane`] — [`InputPlane`]: the input + overlay plane (recogniser + hold-hint
//!   overlay + hold-progress) that the firmware runs preemptively against the map render.
//! - [`screen`] — the modular screen system: the [`Screen`] enum, the
//!   [`Transition`] navigation stack, and the per-screen `handle`/`draw`.
//! - [`activity`] — the ride/tracking model ([`Activity`] + [`Mode`]).
//! - [`app`] — [`App`]: owns the screen stack, the gesture recognizer, the camera
//!   [`AppState`] and the renderer, and drives a frame; [`AppState`] is the camera
//!   core projected into an [`obc_render::Viewport`].
//! - [`dirty`] — [`Dirty`]: the per-frame "which plane changed" signal the
//!   render-on-demand host drains via [`App::take_dirty`](app::App::take_dirty).

#![no_std]

pub mod activity;
pub mod app;
pub mod breadcrumb;
pub mod dirty;
pub mod hal;
pub mod hold_hint;
pub mod input;
pub mod input_plane;
pub mod route;
pub mod screen;
pub mod settings;

pub use activity::{Activity, Mode, TrackAction};
pub use app::{App, AppState, CameraMode, Pan, PanAxis};
pub use breadcrumb::Breadcrumb;
pub use dirty::Dirty;
pub use hal::{
    AltimeterSource, Button, ButtonEvent, CompassSource, Fix, FuelGauge, InputClock, InputEvent, InputSource,
    LocationSource, RideClock, Sensors, SettingsStore, TrackSink,
};
pub use hold_hint::HoldHints;
pub use input::{Gesture, Gestures, DEFAULT_HOLD_MS};
pub use input_plane::InputPlane;
pub use route::{Catalog, RouteSummary, MAX_ROUTES};
pub use screen::{Screen, Transition};
pub use settings::{DateTime, Settings, Units};
