//! OBC device application layer.
//!
//! `no_std`, so the same logic runs in the desktop simulator and on the nRF54L firmware. It owns
//! what the device is doing and leaves how pixels reach a screen to the host. It adds no
//! allocations, and it does not own the render path's working memory: the host lends a
//! [`RenderScratch`](obc_render::RenderScratch) to each render call.
//!
//! The boundary is the dependency-light [`obc_ports`] layer, so the app reads position from a
//! [`LocationSource`] and buttons from an [`InputSource`] whatever the host provides.

// `no_std` for every real target; the crate's own in-crate test build re-enables `std` so the
// staging harnesses can use `Vec`/`Box`. Production code paths are `not(test)` and stay `no_std`.
#![cfg_attr(not(test), no_std)]

// The shared test-support module is compiled twice: once in-crate, once by `tests/common/mod.rs`
// as an integration-test module. Its one crate-relative name is `obc_app::App`, and this alias
// makes that resolve in-crate too, so the same source compiles on both sides.
#[cfg(test)]
extern crate self as obc_app;

pub mod activity;
mod alert;
pub mod altitude;
pub mod app;
pub mod arena_gate;
pub mod ble;
pub mod breadcrumb;
pub(crate) mod card_scheduler;
pub mod catalog_state;
pub mod corridor;
mod crc16;
mod cues;
pub mod device_core;
mod device_status;
pub mod dfu;
pub mod dirty;
mod easier;
pub mod effort;
pub mod fault;
pub mod find_place;
#[cfg(test)]
mod harness;
pub mod hold_hint;
pub mod host;
pub mod i18n;
pub mod input;
pub mod input_plane;
pub mod landmarks;
pub mod map_catalog;
mod map_icons;
pub mod metadata;
pub mod navigator;
pub mod next_ahead;
pub mod peak_view;
pub mod photo;
pub(crate) mod placement;
pub mod recorder;
pub(crate) mod render_key;
pub mod ride;
pub mod route;
pub mod screen;
pub mod sensors;
pub mod settings;
mod settings_enum;
mod settings_table;
mod settlements;
pub mod stat_fields;
pub mod trip;
pub(crate) mod ui_runtime;
pub mod upload_facts;
pub mod wall_clock;
pub mod whats_next;

pub use activity::{Activity, DetourRequest, DfuAction, Mode, NavRequest};
pub use alert::{Alert, Alerts};
pub use altitude::AltitudeFusion;
pub use app::{App, AppState, CameraMode, ClockTrust, Pan, PanBasis, PanTool, GESTURE_BUF, NAV_PREVIEW_MAX};
pub use arena_gate::{ArenaError, ArenaGate, ArenaInit, ArenaOwner, MapQuiesced, TransferReady};
pub use ble::{BleLink, BleStatus};
pub use breadcrumb::Breadcrumb;
pub use corridor::{CorridorKey, CorridorScratch};
pub use device_status::DeviceStatus;
pub use dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
pub use dirty::Dirty;
pub use fault::{draw_boot_fault, BootFault};
pub use hold_hint::HoldHints;
pub use host::DetourPreview;
pub use i18n::{t, Msg};
pub use input::{Chord, Gesture, Gestures, DEFAULT_CHORD_MS, DEFAULT_HOLD_MS, DEFAULT_TAP_MS};
pub use input_plane::InputPlane;
pub use map_catalog::flat_boot_fault;
pub use next_ahead::{NextAhead, NextPoi, REFRESH_STEP_M};
pub use peak_view::{PeakName, PeakViewPeak, PeakViewProfile};
pub use recorder::{RecorderIntent, RecorderMachine, RideContinuation, RideDamage, RideOrigin};
pub use ride::{RideCatalog, RideEntry, RideSummary, RideTrip, RideTrips, MAX_RIDES, UI_RIDES_CAP};
pub use route::{Catalog, RouteSummary, MAX_ROUTES};
pub use screen::{Screen, ScreenKind, Transition, WarningScreen};
pub use sensors::{SensorPhase, SensorScanHit, SensorScanHits, SensorStatus};
pub use settings::{
    ClimbMode, DateTimeEditorExt, IdleReturn, SavedSensor, Settings, Theme, Units, WaypointMode, DATETIME_MAX_YEAR,
    DATETIME_MIN_YEAR, SENSOR_SLOTS,
};
pub use stat_fields::{StatField, StatFieldList};
pub use trip::{RouteVersion, TripInput, TripProgress, TripSummary, Trips, MAX_TRIPS};
pub use upload_facts::{CatalogUpload, CatalogUploadKind, UploadFacts};

/// Durable identity of a catalog object. This is the flat store's `ObjectId` width; UI code keeps
/// the primitive alias so `obc-app` does not depend on a storage implementation.
pub type CatalogObjectId = u64;
pub use wall_clock::{MinuteTicker, WallClock};
