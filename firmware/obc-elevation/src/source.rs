//! The elevation seam: one trait every consumer wires through, and the two implementations that
//! make "the device has no terrain file" and "the device has one" the same code path.

use obc_formats::io::ByteSource;

use crate::{TerrainReader, TileCache};

/// Where a height comes from. **This trait is the seam** — `obc-route`, `obc-pack` and `obc-app`
/// take one of these and never learn whether a raster exists.
///
/// `&mut self` because the real implementation caches tiles behind the call; a sample is a *read*
/// conceptually, but pretending it is one would push the cache behind a `RefCell` for no gain (the
/// callers are all single-threaded sweeps that own their source for the duration).
///
/// `None` means "no height here" and covers every reason at once — outside coverage, a `NODATA`
/// corner, no terrain file at all, a failed read. Consumers must already behave sanely without
/// elevation ([`NullElevation`] is the pin that they do), so a richer answer would only buy them
/// branches they have no different action for.
pub trait ElevationSource {
    /// The height at `(lat, lon)` in whole metres, or `None`.
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16>;
}

/// The no-terrain source: `None` everywhere.
///
/// Not a placeholder — it is the **contract that the epic's "zero changes downstream" claim rests
/// on**. Wiring a consumer through `ElevationSource` must leave its behaviour bit-for-bit identical
/// while this is the implementation, which is what makes the terrain file removable: delete it and
/// the map still renders, routing still works, profiles degrade to flat.
#[derive(Debug, Clone, Copy, Default)]
pub struct NullElevation;

impl ElevationSource for NullElevation {
    #[inline]
    fn sample(&mut self, _lat_udeg: i32, _lon_udeg: i32) -> Option<i16> {
        None
    }
}

/// A [`TerrainReader`] and its [`TileCache`] bound together as one [`ElevationSource`].
///
/// **Never build one on a stack**: it embeds the cache, which is ≈ 2.1 KB at `N = 4` (see
/// [`TileCache`]). Place it where the `App` lives — a `static`, or the device's reserved region —
/// and hand out `&mut`.
pub struct TerrainElevation<'a, const N: usize> {
    reader: TerrainReader<'a>,
    cache: TileCache<N>,
}

impl<'a, const N: usize> TerrainElevation<'a, N> {
    /// Parse `src` as an OBCT container and bind a fresh cache to it.
    pub fn parse(src: &'a dyn ByteSource) -> Result<Self, obc_formats::io::Error> {
        Ok(TerrainElevation { reader: TerrainReader::parse(src)?, cache: TileCache::new() })
    }

    /// The reader underneath — for the header (coverage bbox, posting) a caller wants to show or
    /// check without going through a sample.
    #[inline]
    pub fn reader(&self) -> &TerrainReader<'a> {
        &self.reader
    }

    /// Resident-tile hits and misses, forwarded from the cache.
    #[inline]
    pub fn stats(&self) -> (u32, u32) {
        self.cache.stats()
    }
}

impl<const N: usize> ElevationSource for TerrainElevation<'_, N> {
    #[inline]
    fn sample(&mut self, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        self.reader.sample(&mut self.cache, lat_udeg, lon_udeg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The null source answers `None` for everything, including the coordinates a real one would
    /// answer — that is the whole point of it.
    #[test]
    fn the_null_source_has_no_height_anywhere() {
        let mut null = NullElevation;
        for (lat, lon) in [(0, 0), (47_000_000, 8_000_000), (i32::MIN, i32::MAX)] {
            assert_eq!(null.sample(lat, lon), None);
        }
    }

    /// It is usable through the trait object / generic seam a consumer will hold it behind.
    #[test]
    fn a_consumer_can_hold_the_seam_generically() {
        fn total<E: ElevationSource>(source: &mut E, points: &[(i32, i32)]) -> i32 {
            points.iter().filter_map(|&(lat, lon)| source.sample(lat, lon)).map(i32::from).sum()
        }
        assert_eq!(total(&mut NullElevation, &[(47_000_000, 8_000_000)]), 0);
    }
}
