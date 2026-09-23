//! OBCR route format: reader and GPX converter.
//!
//! `no_std`, so the same code runs in the desktop simulator and in the nRF firmware. A route is a
//! single ordered polyline with per-point elevation plus precomputed ride stats. The on-disk
//! layout is specified in `OBCR_Spec.md`.
//!
//! The elevation dead-band that every module here integrates through lives in [`obc_elevation`],
//! not in this crate. One home for the hysteresis keeps the packer's ascent, the profile's climb
//! and the ridden barometric total from drifting apart.
//!
//! Coordinates are integer microdegrees like the map, and distances and elevations are whole
//! metres. [`obc_map_scene::BBox`] is reused for bounding boxes, so the renderer can compare a
//! route chunk against the map viewport's bbox without conversion.

#![no_std]

// `alloc` is opt-in and off on the device, which places every nav buffer in `.bss`. It only backs
// the heap-boxed constructors a std host uses to keep a large `NavScratch` off its stack.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod attribution;
pub mod climb;
pub mod climb_profile;
pub mod convert;
pub mod corridor;
pub mod easier;
mod eta;
pub mod facts;
mod geo;
pub mod gpx;
pub mod matcher;
pub mod nav;
pub mod profile;
pub mod reader;
pub mod ride;
pub mod splice;
pub mod symbol;
pub mod track;
mod trim;
pub mod trip;
pub mod visit;
pub mod window;

pub use climb::{
    segment_climbs, ClimbSeg, Climbs, ElePt, MAX_CLIMBS, MAX_DROP, MAX_FLAT, MIN_AVG_GRADE, MIN_GAIN, MIN_LEN,
};
pub use climb_profile::{ClimbProfile, COLS as CLIMB_PROFILE_COLS};
pub use convert::{gpx_to_obcr, gpx_to_obcr_attributed, RouteStats};
pub use corridor::{Corridor, MIN_DETOUR_SPAN_M};
pub use eta::time_to_go_s;
pub use facts::{GradeSample, IntervalFacts};
pub use geo::tri_area_m2_cl;
pub use gpx::{GpxScanner, RawPoint, RawWaypoint, WptScanner, WAYPOINT_SYMBOL_CAP};
pub use matcher::{Match, RouteMatch};
pub use obc_formats::bike::BikeType;
// The emit-time elevation seam, re-exported so a caller of `plan_route` names the source it must
// hand in without depending on `obc-elevation` directly.
pub use nav::{plan_detour, plan_route, NavError, NavPhase, NavPlanner, NavScratch, Step, NAV_MAX_NODES};
pub use obc_elevation::{ElevationSource, NullElevation};
pub use profile::{elevation_sparkline, ride_track_into, Profile, Window, PROFILE_COLS, SPARKLINE_BUCKETS};
pub use reader::{
    for_each_waypoint, route_end, ChunkMeta, RouteCache, RouteIndex, RouteObjectInfo, RoutePoint, RoutePosition,
    RouteReader, RouteSummary, Waypoint, WaypointCursor, Waypoints, WptEntry, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS,
    MAX_WAYPOINTS,
};
pub use ride::{encode_summary_footer, RideInfo, RideStats};
pub use splice::{original_name, splice_detour, Leg, SpliceStep, Splicer};
pub use track::track_to_gpx;
pub use trim::{trim_detour_to_tail, TrimOutcome, TrimStep, Trimmer};
pub use trip::{
    read_trip_day, trip_object_len, write_trip, TripDay, TripMeta, TripSummary, MAX_TRIP_DAYS, TRIP_DAY_LEN,
    TRIP_HEADER_LEN, TRIP_VERSION,
};
