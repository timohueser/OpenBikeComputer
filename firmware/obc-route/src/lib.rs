//! OBCR route format: reader + GPX converter.
//!
//! `no_std` so the exact same code runs in the desktop simulator and in the nRF
//! firmware. A route is a single ordered polyline with per-point elevation plus
//! precomputed ride stats — the route-planning sibling of the [`obc_reader`] map
//! format. The on-disk layout is specified in `OBCR_Spec.md`.
//!
//! Modules:
//! - [`byte_io`] — the [`ByteSource`]/[`ByteSink`] abstractions over file / SD / USB
//!   bytes, plus a [`SliceSource`](byte_io::SliceSource) for in-memory bytes.
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

pub mod byte_io;
pub mod climb;
pub mod climb_profile;
pub mod convert;
pub mod deadband;
mod geo;
pub mod gpx;
pub mod matcher;
pub mod nav;
pub mod profile;
pub mod reader;
pub mod ride;
pub mod track;

pub use byte_io::{ByteSink, ByteSource, Error, SliceSource};
pub use climb::{
    segment_climbs, ClimbSeg, Climbs, ElePt, MAX_CLIMBS, MAX_DROP, MAX_FLAT, MIN_AVG_GRADE, MIN_GAIN, MIN_LEN,
};
pub use climb_profile::{ClimbProfile, COLS as CLIMB_PROFILE_COLS};
pub use convert::{gpx_to_obcr, RouteStats};
pub use deadband::{DeadBand, Elev, ELE_DEADBAND_M};
pub use geo::{cos_lat, ground_dist_m, ground_dist_m_cl, tri_area_m2, tri_area_m2_cl};
pub use gpx::{GpxScanner, RawPoint, RawWaypoint, WptScanner};
pub use matcher::{Match, RouteMatch};
pub use nav::{plan_route, NavError, NavPhase, NavPlanner, NavScratch, Step, NAV_MAX_NODES};
pub use profile::{
    elevation_sparkline, ride_elevation_profile, ride_preview_polyline, Profile, Window, PROFILE_COLS,
    SPARKLINE_BUCKETS,
};
pub use reader::{
    for_each_waypoint, ChunkMeta, RouteCache, RouteIndex, RouteObjectInfo, RoutePoint, RouteReader, RouteSummary,
    Waypoint, Waypoints, WptEntry, CHUNK_META_LEN, HEADER_LEN, HEADER_V2_LEN, MAX_POINTS_PER_CHUNK, MAX_ROUTE_CHUNKS,
    MAX_WAYPOINTS, NAME_CAP, WAYPOINT_ELE_NONE, WAYPOINT_LEN, WAYPOINT_NAME_CAP,
};
pub use ride::{
    ride_header_len, ride_object_len, ride_point_len, track_to_ride, RideInfo, RideStats, RIDE_CAD_NONE, RIDE_ELE_NONE,
    RIDE_HEADER_LEN_V1, RIDE_HEADER_LEN_V2, RIDE_HR_NONE, RIDE_POINT_LEN_V1, RIDE_POINT_LEN_V2, RIDE_PWR_NONE,
    RIDE_VERSION,
};
pub use track::{
    decode_record, encode_record, track_to_gpx, TrackPoint, TRACK_CAD_NONE, TRACK_HR_NONE, TRACK_PWR_NONE,
    TRACK_RECORD_LEN,
};

pub use obc_reader::BBox;
