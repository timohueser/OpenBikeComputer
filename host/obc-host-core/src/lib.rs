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
//! - [`terrain`] — the one place a host resolves "the elevation source for this map" (EL7): the
//!   `.obcd` sidecar mounted into an [`ElevationSource`](obc_route::ElevationSource), or the null
//!   source when there is none.
//! - [`trace`] — typed, normalized in-memory behavior traces and policy-free immediate/delayed
//!   outcome scheduling, which the DeviceCore conformance matrix is built on.
//! - [`VecSink`] — the in-memory [`ByteSink`](obc_formats::io::ByteSink) OBCR/GPX output collects into.
//! - [`RgbaFrame`] — the in-memory RGBA8888 `DrawTarget` the browser hosts blit to a `<canvas>`
//!   (the app demo and the builder's preset previews both draw into it).
//! - [`MemRouteStore`] / [`MemRideStore`] / [`MemTrackStore`] — the in-memory store family for a
//!   host without a filesystem (the web demo; also handy in tests). Same surfaces as `obc-sim`'s
//!   folder-backed stores, so host code drives either shape identically.
//!
//! Deliberately **GUI-free**: no egui/eframe/winit here (that's the whole point — the web host's
//! dependency tree must stay framework-free), and no wasm-specific code either. Everything
//! compiles and is tested on the native host.

pub mod conformance;
mod dispatch;
mod frame;
mod nav;
mod replay;
mod repo;
mod session;
mod sink;
mod stores;
pub mod terrain;
pub mod trace;

pub use dispatch::{HostLoop, HostPlatform, InflightPlan, PlanHold};
pub use frame::RgbaFrame;
pub use nav::{commit_detour, commit_nav_plan, plan_detour_preview, DetourPlan, DetourReady, NavPlan};
pub use replay::{initial_camera, replay_advance, ReplaySensors};
pub use repo::{RideRepository, RouteRepository, TrackRepository, TripCatalog};
pub use session::{fill_nav_preview, ActiveRouteSession};
pub use sink::VecSink;
pub use stores::{MemRideStore, MemRouteStore, MemTrackStore};

/// The id band a host's **ride** objects live in.
///
/// [`CatalogEffect::RemoveObject`](obc_app::catalog_state::CatalogEffect) names an object by
/// identity and never by namespace, because the flat store the board runs numbers every object out
/// of one space (FS7 #1389). The simulator's folder stores and the in-memory family below number
/// each family from zero (routes) or from a `TP{id}.OBT` filename (trips), so two families could
/// share an id and a removal would take the wrong one. Carving each non-route family out of a high
/// band gives the executor the same one-space identity without renaming a file on disk: the band is
/// added when a store *reads* an id and stripped when it builds a path.
///
/// Routes keep `[0, RIDE_ID_BASE)`, rides `[RIDE_ID_BASE, TRIP_ID_BASE)`, trips
/// `[TRIP_ID_BASE, ..)`.
pub const RIDE_ID_BASE: obc_app::CatalogObjectId = 1 << 32;

/// The id band a host's **trip** objects live in — the twin of [`RIDE_ID_BASE`], and what lets the
/// trip cascade's last step name the folder through the same namespace-free removal its member
/// steps use (#1491).
pub const TRIP_ID_BASE: obc_app::CatalogObjectId = 1 << 48;
