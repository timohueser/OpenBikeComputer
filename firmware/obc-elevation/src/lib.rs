//! Elevation: the OBCT terrain reader, the sampling rules and the shared ascent integrator.
//!
//! `no_std` and allocation-free, so the same code runs in the packer, the desktop simulator and
//! the nRF firmware. That is the point: one sampling truth, so the packer's per-edge ascent, the
//! device's route elevation and the drawn profile agree by construction.
//!
//! A strict leaf: the only dependency is `obc-formats`, and the crate knows nothing about maps,
//! routes or the UI.
//!
//! [`grid`] holds the integer lattice arithmetic, [`reader`] the container parse and the
//! normative bilinear sample, [`cache`] the resident tiles (never on a stack), [`source`] the
//! [`ElevationSource`] seam with its no-terrain implementation, [`deadband`] the shared hysteresis
//! integrator, and [`integrator`] the dead-banded ascent over a `(distance, elevation)` stream.
//!
//! Units: coordinates are integer microdegrees, heights whole orthometric metres, distances metres.

#![no_std]

pub mod cache;
pub mod deadband;
pub mod grid;
pub mod integrator;
pub mod reader;
pub mod source;
pub mod surface;

pub use cache::TileCache;
pub use deadband::{DeadBand, Elev, ELE_DEADBAND_M};
pub use integrator::ProfileIntegrator;
pub use reader::{TerrainHeader, TerrainReader, TerrainTables};
pub use source::{ElevationSource, NullElevation, TerrainElevation};

/// The credit the Copernicus DEM licence requires on any product derived from the dataset,
/// verbatim. The licence requires this exact notice wherever the data have been adapted, which a
/// resample certainly is, so it is not paraphrasable. It lives in this leaf crate because every
/// consumer that must show it already depends on it.
pub const COPERNICUS_ATTRIBUTION: &str = "produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 \
and © Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union and \
ESA; all rights reserved";

/// The dataset the terrain tiles are derived from, as the catalog names it.
pub const SOURCE_DATASET: &str = "Copernicus DEM GLO-30";

/// The v1 tile-cache depth: four 512 B tiles, about 2.1 KB. Four, because a single bilinear query
/// can straddle a tile corner and touch exactly four tiles.
pub const DEFAULT_TILE_SLOTS: usize = 4;

#[cfg(test)]
mod tests {
    use super::COPERNICUS_ATTRIBUTION;

    #[test]
    fn the_attribution_is_the_wording_the_licence_names() {
        // Pinned as one line: the `const` is written with continuations, and a stray newline would
        // travel into the catalog and the builder.
        assert_eq!(
            COPERNICUS_ATTRIBUTION,
            "produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus Defence and \
             Space GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all \
             rights reserved"
        );
        assert!(!COPERNICUS_ATTRIBUTION.contains('\n'));
    }
}
