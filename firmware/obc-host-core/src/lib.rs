//! Shared **host-side** glue for the simulator shells — the code both hosts drive the shared
//! `no_std` core with, factored out of `obc-sim`'s binary (epic #624, S6) so the desktop simulator
//! (`obc-sim`) and the landing page's thin wasm host (`obc-web-demo`) reuse it instead of
//! copy-pasting:
//!
//! - [`replay_step`] — advance a GPX replay and tick the app on the **playback** clock.
//! - [`NavPlan`] / [`finish_nav_plan`] — the resumable route planner held across frames (one
//!   bounded step per frame, the board's one-step-per-pass shape) and the shared commit/answer
//!   tail, generic over a host's route store via [`NavRouteStore`].
//! - [`VecSink`] — the in-memory [`ByteSink`](obc_route::ByteSink) OBCR/GPX output collects into.
//! - [`MemRouteStore`] / [`MemRideStore`] / [`MemTrackStore`] — the in-memory store family for a
//!   host without a filesystem (the web demo; also handy in tests). Same surfaces as `obc-sim`'s
//!   folder-backed stores, so host code drives either shape identically.
//!
//! Deliberately **GUI-free**: no egui/eframe/winit here (that's the whole point — the web host's
//! dependency tree must stay framework-free), and no wasm-specific code either. Everything
//! compiles and is tested on the native host.

mod nav;
mod replay;
mod sink;
mod stores;

pub use nav::{finish_nav_plan, NavPlan, NavRouteStore};
pub use replay::{initial_camera, replay_step, ReplaySensors};
pub use sink::VecSink;
pub use stores::{MemRideStore, MemRouteStore, MemTrackStore, MEM_NAV_ID};
