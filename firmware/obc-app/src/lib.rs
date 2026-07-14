//! OBC device application layer + compatibility re-exports for semantic ports.
//!
//! `no_std`, so the **same** logic runs in the desktop simulator and on the nRF54L
//! firmware. It owns *what the device is doing* and leaves *how pixels reach a screen* to
//! the host. It adds no allocations of its own (the only heap use is the
//! [`MapRenderer`](obc_render::MapRenderer) scratch, which clears-not-frees each frame).
//!
//! The boundary is the dependency-light [`obc_ports`] layer: the app reads position from a
//! [`LocationSource`] and buttons from an [`InputSource`], oblivious to whether those are a
//! real GPS chip + GPIO or the simulator's control panel + GPX replay. The host injects an
//! implementation; the app stays identical. This crate re-exports those names for compatibility.

#![no_std]

pub mod activity;
pub mod app;
pub mod ble;
pub mod breadcrumb;
pub mod catalog_state;
pub mod dfu;
pub mod dirty;
pub mod fault;
pub mod hal;
pub mod hold_hint;
pub mod host;
pub mod i18n;
pub mod input;
pub mod input_plane;
pub mod nav_profiles;
pub mod ride;
pub(crate) mod ride_engine;
pub mod route;
pub mod screen;
pub mod sensors;
pub mod settings;
pub mod stat_fields;
pub mod trip;
pub mod wall_clock;

pub use activity::{Activity, DfuAction, Mode, NavRequest, TrackAction};
pub use app::{App, AppState, CameraMode, Pan, PanAxis, NAV_PREVIEW_MAX};
pub use ble::{BleLink, BleStatus};
pub use breadcrumb::Breadcrumb;
pub use dfu::{DfuFailure, DfuInstallError, DfuScanError, DfuScanReport};
pub use dirty::Dirty;
pub use fault::{draw_boot_fault, BootFault};
pub use hal::{
    AltimeterSource, Button, ButtonEvent, CadenceSource, ClockSource, CompassSource, Fix, FuelGauge, GpsTime,
    HeartRateSource, InputClock, InputEvent, InputSource, LocationSource, PowerSource, RideClock, Sensors,
    SettingsStore, TemperatureSource, TrackError, TrackSink,
};
pub use hold_hint::HoldHints;
pub use host::{DrainStatus, HostCommand, HostEvent, HostMailbox, HOST_COMMAND_CLASSES};
pub use i18n::{t, Msg};
pub use input::{Gesture, Gestures, DEFAULT_HOLD_MS, DEFAULT_TAP_MS};
pub use input_plane::InputPlane;
pub use nav_profiles::NavProfiles;
pub use ride::{RideCatalog, RideSummary, MAX_RIDES, UI_RIDES_CAP};
pub use route::{Catalog, RouteSummary, MAX_ROUTES};
pub use screen::{Screen, ScreenKind, Transition, WarningFlags, WarningScreen};
pub use sensors::{SensorPhase, SensorScanHit, SensorScanHits, SensorStatus};
pub use settings::{
    decode_route_crcs, decode_store_epoch, decode_synced_rides, encode_route_crcs, encode_store_epoch,
    encode_synced_rides, route_crcs_len, synced_rides_len, ClimbMode, DateTime, DateTimeEditorExt, IdleReturn,
    RouteCrcs, SavedSensor, Settings, SyncedRides, Units, WaypointMode, DATETIME_MAX_YEAR, DATETIME_MIN_YEAR,
    ROUTE_CRCS_MAX_LEN, SENSOR_SLOTS, STORE_EPOCH_LEN, SYNCED_RIDES_MAX_LEN,
};
pub use stat_fields::{StatField, StatFieldList};
pub use trip::{TripInput, TripSummary, Trips, MAX_TRIPS};
pub use wall_clock::{MinuteTicker, WallClock};
