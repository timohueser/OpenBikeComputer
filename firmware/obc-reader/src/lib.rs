//! OBCM map format reader.
//!
//! `no_std` and zero-alloc, so the same code runs in the desktop simulator and in the nRF54L
//! firmware. It parses the one OBCM version named by `obc_formats::obcm::VERSION`; earlier maps
//! get repacked. A file holds N levels of detail, each its own quadtree and chunk set, selected at
//! render time from the current metres per pixel, plus a per-category POI index with a
//! deduplicated hours pool and a tiled routable graph.
//!
//! [`reader`] parses the header, styles and LOD table and serves the per-LOD query and decode,
//! [`hours`] decodes a pooled weekly schedule and answers "open now", and [`color`] converts
//! RGB565 to display colours. The persistent-format authority, the byte-I/O seam and the OBCM
//! constants, lives in [`obc_formats`].
//!
//! All coordinates are integer microdegrees, as stored in the file.

// `no_std` for every real target; the host test harness needs `std` under `cfg(test)`.
#![cfg_attr(not(test), no_std)]

// `alloc` is opt-in and off on the device, which `ptr::write`s its cache into a reserved region.
// It backs only the heap-boxed constructor a small-stack host uses to keep the zero-initialised
// `MapCache` off its stack.
#[cfg(feature = "alloc")]
extern crate alloc;

pub mod articles;
pub mod color;
pub mod corridor;
pub mod hours;
pub mod landmarks;
pub mod peaks;
pub mod photo;
pub mod reader;
mod scene;

pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use corridor::{CorridorPoi, PoiCategorySet, RoutePath, MAX_CORRIDOR_RESULTS};
pub use hours::{weekday_from_ymd, Interval, WeeklySchedule};
// The byte-I/O seam is owned by `obc-formats`, re-exported here because the reader's public API
// traffics in it. Its `Error` is not re-exported, because it would shadow the map-parse [`Error`].
pub use obc_formats::io::{ByteSink, ByteSource, SliceSource};
// The POI category and subtype types the reader's `Poi` surfaces; the normative table and its
// lookups are imported from `obc_formats`.
pub use obc_formats::obcm::{PoiCategory, PoiSubtype};
pub use reader::{
    CacheError, CacheStats, CapacityError, DecodeStatus, FeatureDecodeError, FeatureReadError, FeatureRef, Lod,
    MapCache, MapProfile, MapReadError, MapTables, NavCacheStats, NavDirectory, NavEdgeCandidate, NavEdgeEndpoint,
    NavEdgePosition, NavEdgeSnap, NavNeighbor, NavNodeRef, NavTileCache, Poi, PoiCatEntry, PoiDirectory, Reader,
    Settlement, Summit, TerrainRegion, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS, MAX_POI_RESULTS,
    MAX_SUMMIT_RADIUS_M, NAV_MAX_CHUNK_BYTES, POI_MAX_CATEGORIES, POI_MAX_CHUNK_BYTES,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadMagic,
    BadVersion,
    /// The header's `Offset Scale` byte is outside `0..=9`. Deliberately distinct from
    /// [`Error::BadVersion`]: a scale this reader cannot resolve is an unreadable file, not an old
    /// one.
    BadScale,
    BadOffset,
    /// The requested bytes were validly addressed, but the backing medium failed.
    Source(obc_formats::io::Error),
    /// A safe cache-backed call was re-entered while the cache was already borrowed.
    CacheBusy,
}
