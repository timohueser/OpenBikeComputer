//! Elevation: the OBCT terrain reader, the sampling rules, and the shared ascent integrator.
//!
//! `no_std` and allocation-free, so the same code runs in the packer, the desktop simulator and the
//! nRF firmware — which is the whole architectural point of epic #1068: **one sampling truth**. The
//! packer integrates per-edge ascent from baked OBCT tiles, the device fills a planned route's
//! elevation from the same tiles, and the drawn profile reads the same numbers. They agree by
//! construction rather than by luck, because there is one implementation of the arithmetic and the
//! bytes it reads are normative in [`OBCT_Spec.md`](../../../specs/OBCT_Spec.md).
//!
//! A **strict leaf**: the only dependency is `obc-formats`, and the crate knows nothing about maps,
//! routes or the UI. Everything flows the other way — `obc-route`, `obc-pack` and `obc-app` depend
//! on this.
//!
//! Modules:
//! - [`grid`] — lattice arithmetic: µdeg → sample index → cell → tile, integer-only (spec §1, §3).
//! - [`reader`] — [`TerrainReader`]: parse + validate a container, and the normative bilinear
//!   [`sample`](TerrainReader::sample) (spec §4, §5).
//! - [`cache`] — [`TileCache`]: `N` × 512 B of resident terrain. **Never on a stack** (#419/#501).
//! - [`source`] — [`ElevationSource`], the one seam consumers wire through, and [`NullElevation`],
//!   the no-terrain implementation that pins "removing terrain changes nothing else".
//! - [`deadband`] — the shared hysteresis integrator (moved here from `obc-route` in #1069: this is
//!   its single home, per the #14 rule).
//! - [`integrator`] — [`ProfileIntegrator`], dead-banded ascent/descent over a
//!   `(distance, elevation)` stream — the packer's per-edge ascent and the device's route profile.
//!
//! Units throughout: coordinates are integer microdegrees (1e-6°), heights whole metres
//! (orthometric, EGM2008 — what the source DEM ships and what a rider reads off a signpost),
//! distances metres.

#![no_std]

pub mod cache;
pub mod deadband;
pub mod grid;
pub mod integrator;
pub mod reader;
pub mod source;

pub use cache::TileCache;
pub use deadband::{DeadBand, Elev, ELE_DEADBAND_M};
pub use integrator::ProfileIntegrator;
pub use reader::{TerrainHeader, TerrainReader};
pub use source::{ElevationSource, NullElevation, TerrainElevation};

/// The v1 tile-cache depth: four 512 B tiles ≈ 2.1 KB. Four because a single bilinear query can
/// straddle a tile corner and touch exactly four tiles — anything less would thrash on the one
/// access pattern the sampler is guaranteed to make.
pub const DEFAULT_TILE_SLOTS: usize = 4;
