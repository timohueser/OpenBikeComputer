//! Shared **host-side** glue for the simulator shells — the code both hosts drive the shared
//! `no_std` core with, factored out of `obc-sim`'s binary (epic #624, S6) so the desktop simulator
//! (`obc-sim`) and the landing page's thin wasm host (`obc-web-demo`) reuse it instead of
//! copy-pasting:
//!
//! - [`HostLoop`] / the [`RouteRepository`] · [`RideRepository`] · [`TrackRepository`] ·
//!   [`TripCatalog`] traits — the shared typed executor the frame-stepped hosts (sim GUI, sim
//!   headless, web demo) run `App::run_pass` behind, so the delete/rescan/nav/track sequencing
//!   lives once here instead of once per shell.
//! - [`ActiveRouteSession`] / [`fill_nav_preview`] — the resident parsed active route (no per-frame
//!   `RouteIndex` reparse) and the shared overview-preview fill.
//! - [`replay_step`] — advance a GPX replay and tick the app on the **playback** clock.
//! - [`NavPlan`] / [`commit_nav_plan`] — the resumable route planner held across frames (one
//!   bounded step per frame, the board's one-step-per-pass shape) and the shared commit tail,
//!   generic over a host's route store via [`RouteRepository`].
//! - [`flat_map`] / [`flat_store`] — map objects and revision-pinned readers on shared
//!   memory, temporary-file, or explicitly opened persistent Unix card media.
//! - [`terrain`] — bounded elevation sampling from the exact retained map on the shared card.
//! - [`trace`] — typed, normalized in-memory behavior traces and policy-free immediate/delayed
//!   outcome scheduling, which the DeviceCore conformance matrix is built on.
//! - [`DeviceInput`] / [`FileSettingsStore`] / [`TrackStore`] — the four button edges a host turns
//!   into raw input events, the persisted-settings file, and the card ride recorder that also
//!   exports a committed ride as GPX.
//! - [`peak_view`] — the cooperative panorama runtime the frame-stepped hosts build terrain with.
//! - [`convert_gpx`] — a GPX file as OBCR bytes, attributed against the host's map.
//! - [`VecSink`] — the in-memory [`ByteSink`](obc_formats::io::ByteSink) OBCR/GPX output collects into.
//! - [`frame`] — the whole-frame draw every rendering host uses ([`frame::render`], the
//!   [`frame::active_route`] re-open before it, [`frame::device_rgb888`]) and [`RgbaFrame`], the
//!   in-memory RGBA8888 `DrawTarget` the browser hosts blit to a `<canvas>`.
//! - [`FlatRouteStore`] — routes on the shared host card, including a card that also owns maps.
//! - [`MemRideStore`] / [`MemTrackStore`] — memory stores for browser hosts and tests.
//!
//! Deliberately **GUI-free**: no egui/eframe/winit here (that's the whole point — the web host's
//! dependency tree must stay framework-free). Simulator maps use native temporary files; browser
//! maps use memory. Persistent card APIs are available for host composition.

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
