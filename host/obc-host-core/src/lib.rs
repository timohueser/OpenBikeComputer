//! Shared host-side glue for the simulator shells: the code both hosts drive the shared `no_std`
//! core with, so the desktop simulator and the landing page's thin wasm host reuse it.
//!
//! - [`HostLoop`] and the [`RouteRepository`], [`RideRepository`], [`TrackRepository`] and
//!   [`TripCatalog`] traits — the shared typed executor the frame-stepped hosts run
//!   `App::run_pass` behind, so the delete, rescan, nav and track sequencing lives once here.
//! - [`ActiveRouteSession`] and [`fill_nav_preview`] — the resident parsed active route and the
//!   shared overview-preview fill.
//! - [`replay_step`] — advance a GPX replay and tick the app on the playback clock.
//! - [`NavPlan`] and [`commit_nav_plan`] — the resumable route planner held across frames, one
//!   bounded step per frame, and the shared commit tail.
//! - [`flat_map`] and [`flat_store`] — map objects and revision-pinned readers on shared memory,
//!   temporary-file, or explicitly opened persistent Unix card media.
//! - [`terrain`] — bounded elevation sampling from the exact retained map on the shared card.
//! - [`trace`] — typed, normalized in-memory behavior traces and policy-free outcome scheduling,
//!   which the DeviceCore conformance matrix is built on.
//! - [`DeviceInput`], [`FileSettingsStore`] and [`TrackStore`] — the four button edges a host turns
//!   into raw input events, the persisted-settings file, and the card ride recorder that also
//!   exports a committed ride as GPX.
//! - [`peak_view`] — the cooperative panorama runtime the frame-stepped hosts build terrain with.
//! - [`convert_gpx`] — a GPX file as OBCR bytes, attributed against the host's map.
//! - [`VecSink`] — the in-memory [`ByteSink`](obc_formats::io::ByteSink) OBCR and GPX output
//!   collects into.
//! - [`frame`] — the whole-frame draw every rendering host uses, and [`RgbaFrame`], the in-memory
//!   RGBA8888 `DrawTarget` the browser hosts blit to a `<canvas>`.
//! - [`FlatRouteStore`] — routes on the shared host card, including a card that also owns maps.
//! - [`MemRideStore`] and [`MemTrackStore`] — memory stores for browser hosts and tests.
//!
//! Deliberately GUI-free: no egui, eframe or winit here, because the web host's dependency tree
//! must stay framework-free. Simulator maps use native temporary files; browser maps use memory.

pub mod conformance;
mod device_input;
mod dispatch;
pub mod flat_map;
mod flat_recorder;
mod flat_rides;
pub use flat_recorder::FlatRideRecorder;
mod flat_routes;
mod flat_trips;

pub use flat_rides::FlatRideStore;
pub use flat_trips::FlatTripStore;
pub mod flat_store;
pub use flat_routes::FlatRouteStore;
pub mod frame;
mod gpx;
mod nav;
mod nav_visit;
pub mod peak_view;
pub mod photo;
mod replay;
mod repo;
mod session;
mod settings_store;
mod sink;
mod stores;
pub mod terrain;
/// Test-only oracles, for this crate's suites and for dependents that enable `test-support`.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod trace;
mod track_store;

pub use device_input::DeviceInput;
pub use dispatch::{HostLoop, HostPlatform, InflightPlan, PlanHold};
pub use frame::RgbaFrame;
pub use gpx::convert_gpx;
pub use nav::{commit_detour, commit_nav_plan, plan_detour_preview, DetourPlan, DetourReady, NavPlan};
pub use replay::{initial_camera, replay_advance, ReplaySensors};
pub use repo::{
    AppendStatus, RideRepository, RouteLease, RoutePublication, RouteRepository, TrackRepository, TripCatalog,
};
pub use session::{fill_nav_preview, ActiveRouteSession};
pub use settings_store::FileSettingsStore;
pub use sink::VecSink;
pub use stores::{MemRideStore, MemTrackStore};
pub use track_store::TrackStore;

/// Session ID band retained by legacy folder and summary-only ride fixtures.
pub const RIDE_ID_BASE: obc_app::CatalogObjectId = 1 << 32;

/// Session ID band retained by legacy trip fixtures. Physical deletion also carries its kind.
pub const TRIP_ID_BASE: obc_app::CatalogObjectId = 1 << 48;
