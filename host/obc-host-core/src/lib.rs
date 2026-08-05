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

pub use dispatch::HostLoop;
pub use frame::RgbaFrame;
pub use nav::{finish_detour_commit, finish_detour_plan, finish_nav_plan, DetourPlan, DetourReady, NavPlan};
pub use replay::{initial_camera, replay_step, ReplaySensors};
pub use repo::{RideRepository, RouteRepository, TrackRepository, TripCatalog};
pub use session::{fill_nav_preview, ActiveRouteSession};
pub use sink::VecSink;
pub use stores::{MemRideStore, MemRouteStore, MemTrackStore};
