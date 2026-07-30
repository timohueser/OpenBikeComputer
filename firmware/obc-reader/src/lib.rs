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
//! - [`reader`] — header / style / LOD-table parsing and per-LOD query + decode.
//! - [`hours`] — the device-side view over a hours-pool blob (spec §7.5): decode a pooled
//!   [`WeeklySchedule`], select today's intervals, answer *open now*, and the Zeller weekday
//!   helper the app maps its local clock through.
//! - [`color`] — RGB565 → display color conversions.
//! - [`geo`] — the shared Earth-model distance core ([`M_PER_DEG`] in `f32` clothing):
//!   microdegrees → ground meters, used identically by the route format's stored distances
//!   and the layers that render or match against them.
//!
//! The persistent-format authority — the byte-I/O seam, primitive codecs, OBCM flags/constants,
//! and the POI category/subtype table — lives in [`obc_formats`]; consumers import it from there.
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

pub mod color;
pub mod corridor;
pub mod geo;
pub mod hours;
pub mod reader;
mod scene;
pub mod volume;

pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use corridor::{
    project_onto_chunk, CorridorPoi, PathProjection, PoiCategorySet, RoutePath, CORRIDOR_HALF_WIDTH_M,
    MAX_CORRIDOR_RESULTS,
};
pub use geo::{cos_lat, delta_m, ground_dist_m, ground_dist_m_cl, seg_dist_m, seg_dist_m_cl};
pub use hours::{weekday_from_ymd, Interval, WeeklySchedule, MINUTES_PER_DAY};
// The byte-I/O seam is owned by `obc-formats`; re-exported here at the crate root for convenience
// because the reader's public API traffics in it (`Reader::new(&dyn ByteSource)`). Its `Error` is
// **not** re-exported (it would shadow the map-parse [`Error`] below) — reach it via
// `obc_formats::io::Error`, as `obc-route` does when it re-exports it as `obc_route::Error`.
pub use obc_formats::io::{ByteSink, ByteSource, SliceSource};
// The POI category/subtype types the reader's `Poi` surfaces; the normative table + its
// lookups (`poi_category_of` / `poi_label_of` / `poi_subtype_row`) are imported from `obc_formats`.
pub use obc_formats::obcm::{PoiCategory, PoiSubtype};
pub use reader::{
    read_header, CacheError, CacheStats, CapacityError, DecodeStatus, FeatureDecodeError, FeatureReadError, FeatureRef,
    Lod, MapCache, MapHeader, MapProfile, MapReadError, MapTables, NavCacheStats, NavDirectory, NavNeighbor,
    NavNodeRef, NavTileCache, Poi, PoiCatEntry, PoiDirectory, Reader, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS,
    MAX_POI_RESULTS, NAV_MAX_CHUNK_BYTES, NAV_TILE_SLOTS, POI_MAX_CATEGORIES, POI_MAX_CHUNK_BYTES,
};
pub use volume::{MountError, MountedSet, ShardTables};

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
