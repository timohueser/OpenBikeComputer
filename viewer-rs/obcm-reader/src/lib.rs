//! OBCM map format reader and renderer.
//!
//! `no_std + alloc` so the exact same code runs in the desktop simulator and in
//! the nRF5340 firmware. Parses format **v4** (the LOD pyramid plus a header
//! marker color — see docs/superpowers/specs/2026-06-16-obcm-lod-design.md and
//! OBCM_Spec.md): a file holds N levels of detail, each its own quadtree + chunk
//! set, selected at render time from the current meters-per-pixel.
//!
//! Modules:
//! - [`reader`] — header / style / LOD-table parsing and per-LOD query + decode.
//! - [`color`] — RGB565 → display color conversions.
//!
//! All coordinates are integer microdegrees (1e-6 degrees), as stored in the
//! file. Projection to screen space is the renderer's job.

#![no_std]

pub mod color;
pub mod reader;

pub use color::rgb565_to_device64;
pub use color::rgb565_to_rgb888;
pub use reader::{FeatureRef, Kind, Lod, Reader, Style, MAX_FEAT_PTS, MAX_FEAT_RINGS, HEADER_LEN};

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
