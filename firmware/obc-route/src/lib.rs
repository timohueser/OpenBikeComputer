//! OBCR route format: reader + GPX converter.
//!
//! `no_std` so the exact same code runs in the desktop simulator and in the nRF
//! firmware. A route is a single ordered polyline with per-point elevation plus
//! precomputed ride stats — the route-planning sibling of the [`obc_reader`] map
//! format. The on-disk layout is specified in `OBCR_Spec.md`.
//!
//! Modules:
//! - [`reader`] — header / chunk-index parsing and on-demand chunk decode
//!   ([`RouteReader`], [`RouteSummary`], [`ChunkMeta`], [`RoutePoint`]).
//! - [`profile`] — a route's elevation sampled to a fixed-width [`Profile`] for the
//!   Elevation screen's band + cursor + peak label.
//! - [`climb`] — offline segmentation of that elevation signal into a resident list of
//!   [`Climbs`] (a hysteresis state machine over the same chunk sweep as the profile).
//! - [`climb_profile`] — one detected climb re-bucketed into a small [`ClimbProfile`] detail
//!   buffer (reading only the chunks overlapping the climb) for the ClimbPro-style Climb screen.
//! - [`nav`] — the on-device A* router over the map's §8 nav graph ([`plan_route`]),
//!   emitting its result as a normal OBCR through the shared converter internals.
//!
//! Coordinates are integer microdegrees (1e-6 degrees) like the map; distances and
//! elevations are whole meters. [`obc_reader::BBox`] is reused for bounding boxes so
//! the renderer can compare a route chunk against the map [`Viewport`]'s bbox without
//! conversion.
//!
//! [`Viewport`]: obc_render

#![no_std]

// `alloc` is opt-in (off on the device, which places every nav buffer in `.bss`): it only
// backs the heap-boxed constructors a std host — the simulator — uses to keep a large
// zero-initialised `NavScratch` off its (small, on wasm) stack. See `NavScratch::new_boxed`.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod climb;
pub mod climb_profile;
pub mod convert;
pub mod corridor;
pub mod deadband;
mod geo;
pub mod gpx;
pub mod matcher;
pub mod nav;
pub mod profile;
pub mod reader;
pub mod ride;
pub mod splice;
pub mod track;
pub mod trip;

pub use climb::{
    segment_climbs, ClimbSeg, Climbs, ElePt, MAX_CLIMBS, MAX_DROP, MAX_FLAT, MIN_AVG_GRADE, MIN_GAIN, MIN_LEN,
};
pub use climb_profile::{ClimbProfile, COLS as CLIMB_PROFILE_COLS};
pub use convert::{gpx_to_obcr, RouteStats};
pub use corridor::{Corridor, CORRIDOR_WIDTH_M, MIN_DETOUR_SPAN_M};
pub use deadband::{DeadBand, Elev, ELE_DEADBAND_M};
pub use geo::{cos_lat, ground_dist_m, ground_dist_m_cl, tri_area_m2, tri_area_m2_cl};
pub use gpx::{GpxScanner, RawPoint, RawWaypoint, WptScanner};
pub use matcher::{Match, RouteMatch};
pub use nav::{plan_detour, plan_route, NavError, NavPhase, NavPlanner, NavScratch, Step, NAV_MAX_NODES};
// `obc-formats` owns the byte-I/O seam. `Error` is re-exported here **solely** so obc-route's own
// public GPX/OBCR writer signatures (`track_to_gpx` and the `ByteSink::{write, patch_at}` helpers)
// can name it as `obc_route::Error` — it is not a downstream byte-I/O path. Every consumer, obc-route
// included, imports the seam (`ByteSource` / `ByteSink` / `SliceSource` / `Error`) from
// `obc_formats::io` directly.
pub use obc_formats::io::Error;
pub use profile::{
    elevation_sparkline, ride_elevation_profile, ride_preview_polyline, Profile, Window, PROFILE_COLS,
    SPARKLINE_BUCKETS,
};
pub use reader::{
    for_each_waypoint, ChunkMeta, RouteCache, RouteIndex, RouteObjectInfo, RoutePoint, RoutePosition, RouteReader,
    RouteSummary, Waypoint, Waypoints, WptEntry, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS, MAX_WAYPOINTS,
};
pub use ride::{track_to_ride, RideInfo, RideStats};
pub use splice::{splice_detour, SpliceStep, Splicer};
pub use track::{decode_record, encode_record, track_to_gpx, TrackPoint};
pub use trip::{trip_object_len, write_trip, TripMeta, TripSummary, MAX_TRIP_STAGES, TRIP_HEADER_LEN, TRIP_VERSION};

pub use obc_reader::BBox;
