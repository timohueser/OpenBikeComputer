//! OBCM map format reader and renderer.
//!
//! `no_std`, zero-alloc (heapless) so the exact same code runs in the desktop
//! simulator and in the nRF54L firmware. Parses format **v5** (the LOD pyramid,
//! a header marker color, and a per-style priority level — see OBCM_Spec.md): a
//! file holds N levels of detail, each its own quadtree + chunk set, selected at
//! render time from the current meters-per-pixel.
//!
//! Modules:
//! - [`byte_io`] — the [`ByteSource`]/[`ByteSink`] seam (+ [`SliceSource`]) the map and route
//!   formats both stream through, so neither needs the whole file resident (issue #37).
//! - [`reader`] — header / style / LOD-table parsing and per-LOD query + decode.
//! - [`color`] — RGB565 → display color conversions.
//! - [`codec`] — little-endian field readers/writers shared with the route format.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.

// `no_std` for every real target; the host test harness needs `std`, so allow it under `cfg(test)`
// (the unit tests in `reader` exercise the chunk cache against a flaky `ByteSource`, issue #64).
#![cfg_attr(not(test), no_std)]

pub mod byte_io;
pub mod codec;
pub mod color;
pub mod reader;

// The byte-I/O traits are re-exported at the crate root for convenience; its `Error` is **not**
// (it would shadow the map-parse [`Error`] below) — reach it via `byte_io::Error`, as `obc-route`
// does when it re-exports it as `obc_route::Error`.
pub use byte_io::{ByteSink, ByteSource, SliceSource};
pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use reader::{
    CacheStats, FeatureRef, Kind, Lod, MapCache, Reader, Style, HEADER_LEN, MAX_CHUNK_BYTES, MAX_FEAT_PTS,
    MAX_FEAT_RINGS,
};

/// Meters of ground per degree of latitude (and of longitude at the equator) — the
/// local-equirectangular Earth model. The single source of truth for every crate that
/// turns microdegree coordinates into ground distance (the route converter and its
/// elevation profile, the packer's simplify tolerance) or into screen scale (the
/// renderer's zoom ↔ meters-per-pixel): they all derive from this one number, so a
/// refinement to the Earth model lands everywhere at once.
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
