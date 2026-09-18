//! `obc-app`'s integration tests. One binary; each module below was a
//! top-level `tests/*.rs` file and keeps its own name, helpers and assertions.

mod common;

#[path = "cases/app.rs"]
mod app;
#[path = "cases/ble.rs"]
mod ble;
#[path = "cases/breadcrumb.rs"]
mod breadcrumb;
#[path = "cases/corridor.rs"]
mod corridor;
#[path = "cases/detour_flow.rs"]
mod detour_flow;
#[path = "cases/dirty.rs"]
mod dirty;
#[path = "cases/hold_hint.rs"]
mod hold_hint;
#[path = "cases/host_protocol.rs"]
mod host_protocol;
#[path = "cases/i18n.rs"]
mod i18n;
#[path = "cases/input_plane.rs"]
mod input_plane;
#[path = "cases/marker.rs"]
mod marker;
#[path = "cases/overlay_plane.rs"]
mod overlay_plane;
#[path = "cases/palette.rs"]
mod palette;
#[path = "cases/poi.rs"]
mod poi;
#[path = "cases/ride_recovery.rs"]
mod ride_recovery;
#[path = "cases/settlements.rs"]
mod settlements;
#[path = "cases/trips.rs"]
mod trips;
