//! OBCM map format reader and renderer.
//!
//! `no_std`, zero-alloc (heapless) so the exact same code runs in the desktop
//! simulator and in the nRF54L firmware. Parses format **v6** (the LOD pyramid,
//! a header marker color, a per-style priority level, and the trailing POI
//! directory — see OBCM_Spec.md): a file holds N levels of detail, each its own
//! quadtree + chunk set, selected at render time from the current
//! meters-per-pixel, plus a per-category POI index (parse-only in v6).
//!
//! Modules:
//! - [`byte_io`] — the [`ByteSource`]/[`ByteSink`] seam (+ [`SliceSource`]) the map and route
//!   formats both stream through, so neither needs the whole file resident.
//! - [`reader`] — header / style / LOD-table parsing and per-LOD query + decode.
//! - [`color`] — RGB565 → display color conversions.
//! - [`codec`] — little-endian field readers/writers shared with the route format.
//! - [`format`] — the OBCM flag/sentinel bit constants, shared by the reader and the packer.
//! - [`geo`] — the shared Earth-model distance core ([`M_PER_DEG`] in `f32` clothing):
//!   microdegrees → ground meters, used identically by the route format's stored distances
//!   and the layers that render or match against them.
//! - [`poi_table`] — the canonical POI category/subtype table (spec §7.4), the single firmware
//!   source of truth the packer and the app both mirror.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.

// `no_std` for every real target; the host test harness needs `std`, so allow it under `cfg(test)`.
#![cfg_attr(not(test), no_std)]

pub mod byte_io;
pub mod codec;
pub mod color;
pub mod format;
pub mod geo;
pub mod poi_table;
pub mod reader;

// The byte-I/O traits are re-exported at the crate root for convenience; its `Error` is **not**
// (it would shadow the map-parse [`Error`] below) — reach it via `byte_io::Error`, as `obc-route`
// does when it re-exports it as `obc_route::Error`.
pub use byte_io::{ByteSink, ByteSource, SliceSource};
pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use geo::{cos_lat, ground_dist_m, ground_dist_m_cl};
pub use poi_table::{category_of, label_of, subtype_row, PoiCategory, PoiSubtype, SUBTYPES};
pub use reader::{
    read_header, CacheStats, FeatureRef, Kind, Lod, MapCache, MapHeader, MapTables, PoiCatEntry, PoiDirectory, Reader,
    Style, HEADER_LEN, MAX_CHUNK_BYTES, MAX_FEAT_PTS, MAX_FEAT_RINGS, POI_MAX_CATEGORIES, POI_MAX_CHUNK_BYTES,
};

/// Meters of ground per degree of latitude (and of longitude at the equator) — the
/// local-equirectangular Earth model. Every crate that turns microdegrees into ground distance or
/// screen scale derives from this one number, so an Earth-model refinement lands everywhere at once.
pub const M_PER_DEG: f64 = 111_320.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    TooShort,
    BadMagic,
    BadVersion,
    BadOffset,
}

/// Axis-aligned bounding box in microdegrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BBox {
    pub min_lon: i32,
    pub min_lat: i32,
    pub max_lon: i32,
    pub max_lat: i32,
}

impl BBox {
    #[inline]
    pub fn intersects(&self, o: &BBox) -> bool {
        !(self.max_lon < o.min_lon || self.min_lon > o.max_lon || self.max_lat < o.min_lat || self.min_lat > o.max_lat)
    }
}
