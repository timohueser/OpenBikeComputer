//! OBCM map format reader and renderer.
//!
//! `no_std`, zero-alloc (heapless) so the exact same code runs in the desktop
//! simulator and in the nRF54L firmware. Parses format **v5** (the LOD pyramid,
//! a header marker color, and a per-style priority level — see OBCM_Spec.md): a
//! file holds N levels of detail, each its own quadtree + chunk set, selected at
//! render time from the current meters-per-pixel.
//!
//! Modules:
//! - [`reader`] — header / style / LOD-table parsing and per-LOD query + decode.
//! - [`color`] — RGB565 → display color conversions.
//! - [`codec`] — little-endian field readers/writers shared with the route format.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.

#![no_std]

pub mod codec;
pub mod color;
pub mod reader;

pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use reader::{FeatureRef, Kind, Lod, Reader, Style, HEADER_LEN, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// Meters of ground per degree of latitude (and of longitude at the equator) — the
/// local-equirectangular Earth model. The single source of truth for every crate that
/// turns microdegree coordinates into ground distance (the route converter and its
/// elevation profile, the packer's simplify tolerance) or into screen scale (the
/// renderer's zoom ↔ meters-per-pixel): they all derive from this one number, so a
/// refinement to the Earth model lands everywhere at once instead of in four places
/// under three names.
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
        !(self.max_lon < o.min_lon
            || self.min_lon > o.max_lon
            || self.max_lat < o.min_lat
            || self.min_lat > o.max_lat)
    }
}
