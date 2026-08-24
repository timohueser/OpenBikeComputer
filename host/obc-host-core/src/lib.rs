//! Shared **host-side** glue for the simulator shells — the code both hosts drive the shared
//! `no_std` core with, factored out of `obc-sim`'s binary (epic #624, S6) so the desktop simulator
//! (`obc-sim`) and the landing page's thin wasm host (`obc-web-demo`) reuse it instead of
//! copy-pasting:
//!
//! - [`HostLoop`] / the [`RouteRepository`] · [`RideRepository`] · [`TrackRepository`] ·
//!   [`TripCatalog`] traits — the shared command/event dispatcher the frame-stepped hosts (sim GUI,
//!   sim headless, web demo) drive their `drain_host_commands`/`apply_event` protocol through, so
//!   the delete/rescan/nav/track sequencing lives once here instead of once per shell.
//! - [`ActiveRouteSession`] / [`fill_nav_preview`] — the resident parsed active route (no per-frame
//!   `RouteIndex` reparse) and the shared overview-preview fill.
//! - [`replay_step`] — advance a GPX replay and tick the app on the **playback** clock.
//! - [`NavPlan`] / [`finish_nav_plan`] — the resumable route planner held across frames (one
//!   bounded step per frame, the board's one-step-per-pass shape) and the shared commit/answer
//!   tail, generic over a host's route store via [`RouteRepository`].
//! - [`terrain`] — the one place a host resolves "the elevation source for this map" (EL7): the
//!   `.obcd` sidecar mounted into an [`ElevationSource`](obc_route::ElevationSource), or the null
//!   source when there is none.
//! - [`trace`] — typed, normalized in-memory behavior traces and policy-free immediate/delayed
//!   outcome scheduling used to pin the legacy DeviceCore boundary before ownership moves.
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
mod legacy;
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
pub use legacy::LegacyLoop;
pub use nav::{finish_detour_commit, finish_detour_plan, finish_nav_plan, DetourPlan, DetourReady, NavPlan};
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
/// each family from zero, so a route and a ride could share an id and a removal would take the
/// wrong one. Carving the rides out of a high band gives the executor the same one-space identity
/// without renaming a file on disk: the band is added when a store *reads* an id and stripped when
/// it builds a path.
pub const RIDE_ID_BASE: obc_app::CatalogObjectId = 1 << 32;
