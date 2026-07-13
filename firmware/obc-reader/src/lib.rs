//! OBCM map format reader and renderer.
//!
//! `no_std`, zero-alloc (heapless) so the exact same code runs in the desktop
//! simulator and in the nRF54L firmware. Parses format **v10** (the LOD pyramid,
//! a header marker color, a per-style priority level, the POI directory + hours
//! pool, and the trailing nav-graph section with its profile table — see
//! OBCM_Spec.md): a file holds N
//! levels of detail, each its own quadtree + chunk set, selected at render time
//! from the current meters-per-pixel, plus a per-category POI index, a
//! deduplicated hours pool ([`Reader::poi_hours`](crate::Reader::poi_hours)
//! resolves a POI's schedule on demand — #443), and a tiled routable graph
//! ([`Reader::for_each_nav_node`](crate::Reader::for_each_nav_node) /
//! [`Reader::nav_edge`](crate::Reader::nav_edge) — parse/decode only; the A* is
//! R3, #465).
//!
//! Modules:
//! - [`byte_io`] — compatibility paths for the `obc-formats` byte-I/O seam.
//! - [`reader`] — header / style / LOD-table parsing and per-LOD query + decode.
//! - [`hours`] — the device-side view over a hours-pool blob (spec §7.5): decode a pooled
//!   [`WeeklySchedule`], select today's intervals, answer *open now*, and the Zeller weekday
//!   helper the app maps its local clock through.
//! - [`color`] — RGB565 → display color conversions.
//! - [`codec`] / [`format`] — compatibility paths for `obc-formats` primitive codecs and OBCM
//!   constants.
//! - [`geo`] — the shared Earth-model distance core ([`M_PER_DEG`] in `f32` clothing):
//!   microdegrees → ground meters, used identically by the route format's stored distances
//!   and the layers that render or match against them.
//! - [`poi_table`] — compatibility paths for the canonical `obc-formats` POI category/subtype
//!   table (spec §7.4).
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.

// `no_std` for every real target; the host test harness needs `std`, so allow it under `cfg(test)`.
#![cfg_attr(not(test), no_std)]

// `alloc` is opt-in (off on the device, which `ptr::write`s its cache into a reserved region):
// it only backs the heap-boxed constructor a small-stack host — the wasm web demo — uses to
// keep the ≈277 KB zero-initialised `MapCache` off its stack. See `MapCache::new_boxed`.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod byte_io;
pub mod codec;
pub mod color;
pub mod format;
pub mod geo;
pub mod hours;
pub mod poi_table;
pub mod reader;
mod scene;

// The byte-I/O traits are re-exported at the crate root for convenience; its `Error` is **not**
// (it would shadow the map-parse [`Error`] below) — reach it via `byte_io::Error`, as `obc-route`
// does when it re-exports it as `obc_route::Error`.
pub use byte_io::{ByteSink, ByteSource, SliceSource};
pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use geo::{cos_lat, delta_m, ground_dist_m, ground_dist_m_cl, seg_dist_m, seg_dist_m_cl};
pub use hours::{
    weekday_from_ymd, Interval, WeeklySchedule, HOURS_FLAG_SEASONAL, HOURS_FLAG_TRUNCATED, MINUTES_PER_DAY,
};
pub use poi_table::{category_of, label_of, subtype_row, PoiCategory, PoiSubtype, SUBTYPES};
pub use reader::{
    read_header, CacheError, CacheStats, CapacityError, DecodeStatus, FeatureDecodeError, FeatureReadError, FeatureRef,
    Lod, MapCache, MapHeader, MapProfile, MapReadError, MapTables, NavCacheStats, NavDirectory, NavNeighbor,
    NavNodeRef, NavTileCache, Poi, PoiCatEntry, PoiDirectory, Reader, HEADER_LEN, MAX_CHUNK_BYTES, MAX_FEAT_PTS,
    MAX_FEAT_RINGS, MAX_POI_RESULTS, NAV_CHUNK_SIZE, NAV_EDGE_FIXED_LEN, NAV_MAX_CHUNK_BYTES, NAV_MAX_PROFILES,
    NAV_NEIGHBOR_LEN, NAV_NODE_FIXED_LEN, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN, NAV_TILE_SLOTS, POI_HOURS_BLOB_LEN,
    POI_MAX_CATEGORIES, POI_MAX_CHUNK_BYTES, POI_NAME_MAX,
};

// Compatibility paths: neutral scene/geometry primitives now live below the concrete OBCM reader.
pub use obc_map_scene::{BBox, Kind, Style, M_PER_DEG};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadMagic,
    BadVersion,
    BadOffset,
    /// The requested bytes were validly addressed, but the backing medium failed.
    Source(obc_formats::io::Error),
    /// A safe cache-backed call was re-entered while the cache was already borrowed.
    CacheBusy,
}
