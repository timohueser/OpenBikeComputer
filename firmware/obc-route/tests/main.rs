//! `obc-route`'s integration tests: one binary, one module per area, each keeping its own
//! helpers and assertions.

mod common;

#[path = "cases/climb.rs"]
mod climb;
#[path = "cases/climb_profile.rs"]
mod climb_profile;
#[path = "cases/convert.rs"]
mod convert;
#[path = "cases/corridor.rs"]
mod corridor;
#[path = "cases/detour.rs"]
mod detour;
#[path = "cases/eta.rs"]
mod eta;
#[path = "cases/facts.rs"]
mod facts;
#[path = "cases/format.rs"]
mod format;
#[path = "cases/gpx.rs"]
mod gpx;
#[path = "cases/matcher.rs"]
mod matcher;
#[path = "cases/nav.rs"]
mod nav;
#[path = "cases/profile.rs"]
mod profile;
#[path = "cases/ride.rs"]
mod ride;
#[path = "cases/track.rs"]
mod track;
#[path = "cases/transform.rs"]
mod transform;
#[path = "cases/trip.rs"]
mod trip;
#[path = "cases/visit.rs"]
mod visit;
#[path = "cases/waypoints.rs"]
mod waypoints;
