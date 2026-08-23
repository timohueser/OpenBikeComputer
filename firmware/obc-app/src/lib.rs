//! OBC device application layer.
//!
//! `no_std`, so the **same** logic runs in the desktop simulator and on the nRF54L
//! firmware. It owns *what the device is doing* and leaves *how pixels reach a screen* to
//! the host. It adds no allocations of its own — and since #1146 it does not even own the render
//! path's working memory: the host lends a [`RenderScratch`](obc_render::RenderScratch) to each
//! render call, and that scratch clears-not-frees its buffers per frame.
//!
//! The boundary is the dependency-light [`obc_ports`] layer: the app reads position from a
//! [`LocationSource`] and buttons from an [`InputSource`], oblivious to whether those are a
//! real GPS chip + GPIO or the simulator's control panel + GPX replay. The host injects an
//! implementation; the app stays identical.

// `no_std` for every real target (board + sim build `not(test)`); the crate's own in-crate test
// build re-enables `std` so the relocated screen/upload staging harnesses (FAR-19, #812) — which
// reach `Activity`'s `pub(crate)` fields directly — can use `Vec`/`Box`, the same pattern
// `obc-reader` uses. Production code paths are `not(test)` and stay strictly `no_std`.
#![cfg_attr(not(test), no_std)]

// The shared test-support module ([`harness::support`]) is compiled **twice**: once in-crate for
// the staging harnesses, once by `tests/common/mod.rs` as an integration-test module (a `#[path]`
// include, so there is exactly one copy on disk). Its one crate-relative name is `obc_app::App` —
// which the integration side resolves as an extern crate; this alias makes it resolve in-crate too,
// so the same source compiles on both sides.
#[cfg(test)]
extern crate self as obc_app;

pub mod activity;
pub mod altitude;
pub mod app;
pub mod arena_gate;
pub mod ble;
pub mod breadcrumb;
pub mod catalog_state;
pub mod corridor;
pub mod device_core;
mod device_status;
pub mod dfu;
pub mod dirty;
pub mod fault;
#[cfg(test)]
mod harness;
pub mod hold_hint;
pub mod host;
pub mod i18n;
pub mod input;
pub mod input_plane;
pub mod link_gate;
pub mod map_catalog;
pub mod nav_profiles;
pub mod next_ahead;
pub mod reroute_freeze;
pub mod retention;
pub mod ride;
pub(crate) mod ride_engine;
pub mod route;
pub mod screen;
pub mod sensors;
pub mod settings;
mod settings_enum;
pub mod stat_fields;
pub mod store_meta;
pub mod trip;
pub(crate) mod ui_runtime;
pub mod wall_clock;
pub mod weather;
pub mod weather_alerts;
pub mod weather_rain;

pub use activity::{Activity, DetourRequest, DfuAction, Mode, NavRequest, RideContinuation, TrackAction};
pub use altitude::AltitudeFusion;
pub use app::{App, AppState, CameraMode, ClockTrust, Pan, PanBasis, PanTool, NAV_PREVIEW_MAX};
pub use arena_gate::{ArenaError, ArenaGate, ArenaInit, ArenaOwner, MapQuiesced, TransferReady};
// `ble::WeatherSnapshot` (WX8's request-context inputs) deliberately keeps its module-qualified
// name: the crate-root `WeatherSnapshot` is the resident *forecast* snapshot the screens render
// (WX11's `weather::WeatherSnapshot`) — two different objects that merged with the same name.
pub use ble::{BleLink, BleStatus, WeatherFix};
pub use breadcrumb::Breadcrumb;
pub use corridor::{CorridorKey, CorridorScratch};
pub use device_status::DeviceStatus;
pub use dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
pub use dirty::Dirty;
pub use fault::{draw_boot_fault, BootFault};
pub use hold_hint::HoldHints;
pub use host::{DetourPreview, DrainStatus, HostCommand, HostEvent, HostMailbox, HOST_COMMAND_CLASSES};
pub use i18n::{t, Msg};
pub use input::{Gesture, Gestures, DEFAULT_HOLD_MS, DEFAULT_TAP_MS};
pub use input_plane::InputPlane;
pub use link_gate::{GateOwner, TransferGate};
pub use map_catalog::{
    boot_fault, choose_map, classify_map_entry, flat_boot_fault, is_superseded_upload, newest_set,
    set_retirement_keeper, MapChoice, MapEntry,
};
pub use nav_profiles::NavProfiles;
pub use next_ahead::{NextAhead, NextPoi, REFRESH_STEP_M};
pub use retention::{
    decode_route_retention, encode_route_retention, Retention, RideRetention, RideRetentionRecord, RouteRetentionMeta,
    RouteRetentionStore, ROUTE_RETENTION_MAX_LEN,
};
pub use ride::{RideCatalog, RideSummary, MAX_RIDES, UI_RIDES_CAP};
pub use route::{Catalog, RouteSummary, MAX_ROUTES};
pub use screen::{Screen, ScreenKind, Transition, WarningFlags, WarningScreen, WeatherAlertKind};
pub use sensors::{SensorPhase, SensorScanHit, SensorScanHits, SensorStatus};
pub use settings::{
    ClimbMode, DateTimeEditorExt, IdleReturn, SavedSensor, Settings, Units, WaypointMode, WeatherRefresh,
    DATETIME_MAX_YEAR, DATETIME_MIN_YEAR, SENSOR_SLOTS,
};
pub use stat_fields::{StatField, StatFieldList};
pub use trip::{TripInput, TripSummary, Trips, MAX_TRIPS};

/// Durable identity of a catalog object. This is the flat store's `ObjectId` width; UI code keeps
/// the primitive alias so `obc-app` does not depend on a storage implementation.
pub type CatalogObjectId = u64;
pub use wall_clock::{MinuteTicker, WallClock};
pub use weather::{rain_outlook, RainOutlook, RideProjection, WeatherFeed, WeatherSnapshot};
pub use weather_alerts::{AlertCandidate, AlertClass, AlertMark};
pub use weather_rain::RainOverlayAdapter;
