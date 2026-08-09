//! The rain-overlay adapter (WX10): the production bridge from the WX7 OBCW reader/cache onto
//! `obc-render`'s format-agnostic [`RainOverlaySource`] seam.
//!
//! One adapter for every host — the board and the simulator construct it the same way, per frame,
//! from whatever weather store they mount — so firmware and `obc-sim` render precipitation through
//! the identical code path. The freshness gate lives in `obc-weather`
//! ([`WeatherReader::current_frame`]): the adapter simply refuses to exist when no frame may
//! render as current, and a screen holding no adapter draws a byte-identical rain-free map.

use obc_formats::io::ByteSource;
use obc_render::{RainGrid, RainOverlaySource, RAIN_TILE_CELLS, RAIN_TILE_EDGE};
use obc_weather::{WeatherCache, WeatherReader};

// The render seam's tile shape is a mirror of OBCW §5's; neither may drift.
const _: () = assert!(RAIN_TILE_EDGE == obc_formats::obcw::TILE_EDGE);
const _: () = assert!(RAIN_TILE_CELLS == obc_formats::obcw::TILE_CELLS);

/// A per-frame lease of the active weather bundle's **current** rain frame, drawable through the
/// renderer's seam. Construct with [`RainOverlayAdapter::current`] each frame; holding one across
/// frames would let the "current" decision go stale, which is exactly what WX10 forbids.
pub struct RainOverlayAdapter<'a, S: ByteSource + ?Sized> {
    reader: &'a WeatherReader<'a, S>,
    cache: &'a mut WeatherCache,
    frame_index: usize,
    grid: RainGrid,
}

impl<'a, S: ByteSource + ?Sized> RainOverlayAdapter<'a, S> {
    /// The adapter for the frame that is current at `now_unix` (UTC seconds), or `None` when
    /// nothing may render — no frame yet, frame outrun, or bundle expired
    /// ([`WeatherReader::current_frame`] is the one authority). The grid mirrors the OBCW header
    /// bounds and the frame's cell dimensions, so the renderer's fixed-point sampler and
    /// `obc-weather`'s own `cell_index` name the same provider cell for the same coordinate.
    pub fn current(reader: &'a WeatherReader<'a, S>, cache: &'a mut WeatherCache, now_unix: i64) -> Option<Self> {
        Self::at_step(reader, cache, now_unix, 0)
    }

    /// The adapter for the `step`-th **future** frame past the one current at `now_unix` — the
    /// rain map's time-step navigation (WX11). `step == 0` is exactly [`current`](Self::current);
    /// a step past the table clamps to the last frame (the timeline's end is an end). Forecast
    /// frames carry their real future timestamps, so stepping forward is not a freshness
    /// violation — but the *anchor* stays the freshness-gated current frame: with nothing current
    /// there is nothing to step from, and no adapter exists at any step.
    pub fn at_step(
        reader: &'a WeatherReader<'a, S>,
        cache: &'a mut WeatherCache,
        now_unix: i64,
        step: u8,
    ) -> Option<Self> {
        let (current_index, current) = reader.current_frame(now_unix, cache).ok().flatten()?;
        let header = reader.header();
        let frame_index = (current_index + step as usize).min(header.frame_count.saturating_sub(1) as usize);
        let frame = if frame_index == current_index { current } else { reader.frame(frame_index).ok()? };
        Some(Self {
            reader,
            cache,
            frame_index,
            grid: RainGrid {
                west_udeg: header.west_lon_udeg,
                south_udeg: header.south_lat_udeg,
                east_udeg: header.east_lon_udeg,
                north_udeg: header.north_lat_udeg,
                width_cells: frame.width,
                height_cells: frame.height,
            },
        })
    }
}

impl<S: ByteSource + ?Sized> RainOverlaySource for RainOverlayAdapter<'_, S> {
    fn grid(&self) -> Option<RainGrid> {
        Some(self.grid)
    }

    fn tile(&mut self, tile_index: u32, out: &mut [u8; RAIN_TILE_CELLS]) -> bool {
        // Through WX7's fixed cache: a cold random tile is at most three bounded SD reads
        // (descriptor / directory window / payload), a warm one zero. Any failure — SD fault,
        // malformed payload, out-of-range index — renders transparent, never fabricated weather.
        match self.reader.decode_tile_cached(self.frame_index, tile_index, self.cache) {
            Ok(tile) => {
                out.copy_from_slice(tile);
                true
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::SliceSource;
    use obc_formats::obcw::TILE_EDGE;

    const DWD: &[u8] = include_bytes!("../../../specs/vectors/weather-dwd-96x96-9f.obcw");

    /// The adapter's grid and tiles agree with the reader's own geographic lookup: for a sweep of
    /// in-bounds coordinates, sampling the adapter's tile at the renderer's (row, col) → tile/cell
    /// arithmetic returns exactly `intensity_at`'s nearest-neighbour answer.
    #[test]
    fn adapter_tiles_agree_with_the_readers_nearest_neighbour_lookup() {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let header = reader.header();
        let now = reader.frame(0).unwrap().valid_at;
        let mut cache = WeatherCache::new();
        let mut adapter = RainOverlayAdapter::current(&reader, &mut cache, now).unwrap();
        let grid = adapter.grid().unwrap();
        assert_eq!(
            (grid.west_udeg, grid.south_udeg, grid.east_udeg, grid.north_udeg),
            (header.west_lon_udeg, header.south_lat_udeg, header.east_lon_udeg, header.north_lat_udeg)
        );

        let tile_cols = (grid.width_cells as u32).div_ceil(TILE_EDGE as u32);
        let mut tile = [0u8; RAIN_TILE_CELLS];
        for step in 0..64u32 {
            let lat = header.south_lat_udeg + ((step as i64 * 37 + 11) % 1000) as i32 * 863;
            let lon = header.west_lon_udeg + ((step as i64 * 59 + 7) % 1000) as i32 * 1289;
            let frame = reader.frame(0).unwrap();
            let Some(cell) = reader.cell_index(frame, lat, lon).unwrap() else { continue };
            let tile_index = (cell.row as u32 / TILE_EDGE as u32) * tile_cols + cell.column as u32 / TILE_EDGE as u32;
            let cell_in_tile = (cell.row as usize % TILE_EDGE) * TILE_EDGE + cell.column as usize % TILE_EDGE;
            assert_eq!((tile_index, cell_in_tile as u16), (cell.tile_index, cell.cell_in_tile));
            assert!(adapter.tile(tile_index, &mut tile));
            let mut check_cache = WeatherCache::new();
            let expect = reader.intensity_at(0, lat, lon, &mut check_cache).unwrap().unwrap();
            assert_eq!(tile[cell_in_tile], expect, "coordinate ({lat},{lon})");
        }
    }

    /// Expired and future instants construct no adapter at all — the map draws rain-free rather
    /// than rendering stale rain as current.
    #[test]
    fn no_adapter_exists_outside_the_current_window() {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let header = reader.header();
        let last = reader.frame(header.frame_count as usize - 1).unwrap().valid_at;
        let mut cache = WeatherCache::new();
        assert!(RainOverlayAdapter::current(&reader, &mut cache, header.valid_from - 1).is_none());
        assert!(RainOverlayAdapter::current(&reader, &mut cache, last + 10_000).is_none());
        assert!(RainOverlayAdapter::current(&reader, &mut cache, header.valid_until + 1).is_none());
    }

    /// A hostile tile index is a transparent tile, not a panic or a lie.
    #[test]
    fn out_of_range_tiles_fail_closed() {
        let source = SliceSource(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        let now = reader.frame(0).unwrap().valid_at;
        let mut cache = WeatherCache::new();
        let mut adapter = RainOverlayAdapter::current(&reader, &mut cache, now).unwrap();
        let mut tile = [0u8; RAIN_TILE_CELLS];
        assert!(!adapter.tile(u32::MAX, &mut tile));
    }
}
