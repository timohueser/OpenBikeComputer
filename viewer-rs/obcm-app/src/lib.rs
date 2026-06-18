//! OBCM device application layer + hardware-abstraction traits.
//!
//! `no_std` so the **same** application logic runs in the desktop simulator and
//! on the nRF5340 firmware, exactly like [`obcm`]'s reader and renderer already
//! do. This crate adds no allocations of its own; the only heap use is inside the
//! [`MapRenderer`](obcm_render::MapRenderer) scratch that [`App`] drives (see [`obcm_reader`]),
//! which clears-not-frees each frame. This crate sits one level above the
//! renderer: it owns *what the device is doing* — where the user is, where the
//! camera looks, which mode it's in — and leaves *how pixels reach a screen* to
//! the host.
//!
//! The boundary is a small hardware-abstraction layer (HAL): the app reads the
//! user's position from a [`LocationSource`] and (later) buttons from an
//! [`InputSource`], never caring whether those come from a real GPS chip and
//! GPIO pins (firmware) or from the simulator's control panel and a GPX replay
//! (host). The host injects an implementation; the app stays identical.
//!
//! Modules:
//! - [`hal`] — the injected-hardware traits ([`LocationSource`], [`InputSource`])
//!   and their data types ([`Fix`], [`Button`], [`ButtonEvent`], [`InputEvent`]).
//! - [`input`] — the shared gesture recognizer ([`Gestures`]) turning raw
//!   [`InputEvent`]s + a millis clock into the five UI [`Gesture`]s.
//! - [`screen`] — the modular screen system: the [`Screen`] enum, the
//!   [`Transition`] navigation stack, and the per-screen `handle`/`draw`.
//! - [`activity`] — the ride/tracking model ([`Activity`] + [`Mode`]).
//! - [`app`] — [`App`]: owns the screen stack, the gesture recognizer, the camera
//!   [`AppState`] and the renderer, and drives a frame; [`AppState`] is the camera
//!   core projected into an [`obcm_render::Viewport`].

#![no_std]

pub mod activity;
pub mod app;
pub mod hal;
pub mod input;
pub mod route;
pub mod screen;

pub use activity::{Activity, Mode};
pub use route::{Catalog, RouteSummary, MAX_ROUTES};
pub use app::{App, AppState, CameraMode};
pub use hal::{
    AltimeterSource, Button, ButtonEvent, Fix, InputClock, InputEvent, InputSource, LocationSource,
    RideClock, Sensors,
};
pub use input::{Gesture, Gestures, DEFAULT_HOLD_MS};
pub use screen::{Screen, Transition};
