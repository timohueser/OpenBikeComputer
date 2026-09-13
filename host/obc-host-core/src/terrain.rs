//! Optional elevation from the same retained card map as rendering and planning.

use crate::{flat_map::FlatMap, flat_store::ObjectSource};
use obc_elevation::{ElevationSource, TerrainTables, TileCache, DEFAULT_TILE_SLOTS};
use obc_formats::io::{Error, WindowSource};
use obc_reader::TerrainRegion;

/// A bounded terrain cache bound to one exact map source. The shared read hold keeps that
/// revision and its card alive even after the map frontend is dropped or replaced.
pub struct FlatElevation {
    source: ObjectSource,
    region: TerrainRegion,
    tables: TerrainTables,
    cache: TileCache<DEFAULT_TILE_SLOTS>,
}

impl FlatElevation {
    /// Absence is optional; malformed, unsupported or unreadable terrain is an explicit error.
    /// Descriptor, byte window and cache can only be created together from this map.
    pub fn open(map: &FlatMap) -> Result<Option<Box<Self>>, Error> {
        let Some(region) = map.tables().terrain() else { return Ok(None) };
        let source = map.source();
        let window = WindowSource::new(&source, region.offset, region.len).ok_or(Error::BadOffset)?;
        let tables = TerrainTables::parse(&window)?;
        Ok(Some(Box::new(Self { source, region, tables, cache: TileCache::new() })))
    }
}

impl ElevationSource for FlatElevation {
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        let window = WindowSource::new(&self.source, self.region.offset, self.region.len)?;
        self.tables.reader(&window).sample(&mut self.cache, lat_udeg, lon_udeg)
    }
}

#[cfg(test)]
mod tests;
