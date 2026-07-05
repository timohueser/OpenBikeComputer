//! OBC device application layer + hardware-abstraction traits.
//!
//! `no_std`, so the **same** logic runs in the desktop simulator and on the nRF54L
//! firmware. It owns *what the device is doing* and leaves *how pixels reach a screen* to
//! the host. It adds no allocations of its own (the only heap use is the
//! [`MapRenderer`](obc_render::MapRenderer) scratch, which clears-not-frees each frame).
//!
//! The boundary is a small hardware-abstraction layer (HAL): the app reads position from a
//! [`LocationSource`] and buttons from an [`InputSource`], oblivious to whether those are a
//! real GPS chip + GPIO or the simulator's control panel + GPX replay. The host injects an
//! implementation; the app stays identical.

#![no_std]

pub mod activity;
pub mod app;
pub mod ble;
pub mod breadcrumb;
pub mod dirty;
pub mod hal;
pub mod hold_hint;
pub mod input;
pub mod input_plane;
pub mod route;
pub mod screen;
pub mod settings;
pub mod stat_fields;
pub mod wall_clock;

pub use activity::{Activity, Mode, TrackAction};
pub use app::{App, AppState, CameraMode, Pan, PanAxis};
pub use ble::BleStatus;
pub use breadcrumb::Breadcrumb;
pub use dirty::Dirty;
pub use hal::{
    AltimeterSource, Button, ButtonEvent, ClockSource, CompassSource, Fix, FuelGauge, GpsTime, InputClock, InputEvent,
    InputSource, LocationSource, RideClock, Sensors, SettingsStore, TemperatureSource, TrackSink,
};
pub use hold_hint::HoldHints;
pub use input::{Gesture, Gestures, DEFAULT_HOLD_MS, DEFAULT_TAP_MS};
pub use input_plane::InputPlane;
pub use route::{Catalog, RouteSummary, MAX_ROUTES};
pub use screen::{Screen, ScreenKind, Transition};
pub use settings::{DateTime, Settings, Units};
pub use stat_fields::{StatField, StatFieldList};
pub use wall_clock::{MinuteTicker, WallClock};
