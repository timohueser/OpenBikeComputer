//! Fixed, generation-aware weather lookup cache.

use obc_formats::io::{ByteSource, SliceSource};
use obc_formats::obcw::{
    self, FrameDescriptor, Header, HourlyRecord, TileEntry, HOURLY_COUNT, HOURLY_INTERVAL_SECONDS, RAW4_LEN,
    TILE_CELLS, TILE_DIRECTORY_ENTRY_LEN, TILE_EDGE,
};

use crate::{checked_add, checked_mul, Error, FormatError, WeatherReader};

/// Four adjacent entries fit in 48 bytes and match the validated reader's directory window.
pub const DIRECTORY_WINDOW_ENTRIES: usize = 4;

const EMPTY_FRAME: FrameDescriptor = FrameDescriptor {
    valid_at: 0,
    width: 0,
    height: 0,
    cell_size_m: 0,
    tile_directory_offset: 0,
    tile_count: 0,
    tile_data_offset: 0,
    tile_data_len: 0,
    quality_flags: 0,
};
const EMPTY_ENTRY: TileEntry = TileEntry { data_offset: 0, encoded_len: 0, decoded_cells: 0, codec: 0 };

/// Row/column and packed tile address for one in-bounds geographic query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellIndex {
    pub row: u16,
    pub column: u16,
    pub tile_index: u32,
    pub cell_in_tile: u16,
}

/// The fixed cache shared across one active weather generation at a time.
///
/// Every key includes `Header::generation` and the validated bundle CRC; reusing this value after
/// an A/B swap cannot return a tile or directory entry from the previous object, including an
/// equal-generation bundle with a later producer timestamp. A cold random tile lookup performs at most
/// three `ByteSource::read_at` calls: descriptor, four-entry directory window, payload. A tile hit
/// performs none, and another tile in the same window performs only its payload read.
pub struct WeatherCache {
    frame_valid: bool,
    frame_generation: u32,
    frame_bundle_crc: u32,
    frame_index: u16,
    frame: FrameDescriptor,
    directory_valid: bool,
    directory_generation: u32,
    directory_bundle_crc: u32,
    directory_frame: u16,
    directory_first: u32,
    directory_len: u8,
    directory: [TileEntry; DIRECTORY_WINDOW_ENTRIES],
    tile_valid: bool,
    tile_generation: u32,
    tile_bundle_crc: u32,
    tile_frame: u16,
    tile_index: u32,
    tile: [u8; TILE_CELLS],
}

impl WeatherCache {
    pub const fn new() -> Self {
        Self {
            frame_valid: false,
            frame_generation: 0,
            frame_bundle_crc: 0,
            frame_index: 0,
            frame: EMPTY_FRAME,
            directory_valid: false,
            directory_generation: 0,
            directory_bundle_crc: 0,
            directory_frame: 0,
            directory_first: 0,
            directory_len: 0,
            directory: [EMPTY_ENTRY; DIRECTORY_WINDOW_ENTRIES],
            tile_valid: false,
            tile_generation: 0,
            tile_bundle_crc: 0,
            tile_frame: 0,
            tile_index: 0,
            tile: [0; TILE_CELLS],
        }
    }

    pub fn clear(&mut self) {
        self.frame_valid = false;
        self.directory_valid = false;
        self.tile_valid = false;
    }
}

impl Default for WeatherCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Exact resident state of a representative sized reader plus its fixed cache on the current
/// target. The nRF resource-report table records this constant using the actual Thumb ABI.
pub const READER_CACHE_RESIDENT_BYTES: usize =
    core::mem::size_of::<WeatherReader<'static, SliceSource<'static>>>() + core::mem::size_of::<WeatherCache>();

impl<'a, S: ByteSource + ?Sized> WeatherReader<'a, S> {
    /// The hourly record covering `timestamp`, or `None` outside the 24 represented intervals.
    pub fn hourly_at(&self, timestamp: i64) -> Result<Option<(usize, i64, HourlyRecord)>, Error> {
        let delta = match timestamp.checked_sub(self.header.valid_from) {
            Some(delta) if delta >= 0 => delta,
            _ => return Ok(None),
        };
        let represented =
            (HOURLY_COUNT as i64).checked_mul(HOURLY_INTERVAL_SECONDS as i64).ok_or(FormatError::Timestamp)?;
        if delta >= represented {
            return Ok(None);
        }
        let index = usize::try_from(delta / HOURLY_INTERVAL_SECONDS as i64).map_err(|_| FormatError::Timestamp)?;
        let valid_at = self
            .header
            .valid_from
            .checked_add((index as i64) * HOURLY_INTERVAL_SECONDS as i64)
            .ok_or(FormatError::Timestamp)?;
        Ok(Some((index, valid_at, self.hourly(index)?)))
    }

    pub fn hourly_iter(&self) -> HourlyIter<'_, 'a, S> {
        HourlyIter { reader: self, next: 0 }
    }

    /// Find an exact native rain-frame timestamp with at most 17 descriptor reads.
    pub fn frame_at(
        &self,
        timestamp: i64,
        cache: &mut WeatherCache,
    ) -> Result<Option<(usize, FrameDescriptor)>, Error> {
        let Some((index, frame)) = self.frame_at_or_before(timestamp, cache)? else { return Ok(None) };
        Ok((frame.valid_at == timestamp).then_some((index, frame)))
    }

    /// Find the newest genuine frame at or before `timestamp`; no synthetic interval is invented.
    pub fn frame_at_or_before(
        &self,
        timestamp: i64,
        cache: &mut WeatherCache,
    ) -> Result<Option<(usize, FrameDescriptor)>, Error> {
        let mut low = 0usize;
        let mut high = self.header.frame_count as usize;
        while low < high {
            let mid = low + (high - low) / 2;
            let frame = self.cached_frame(mid, cache)?;
            if frame.valid_at <= timestamp {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        if low == 0 {
            return Ok(None);
        }
        let index = low - 1;
        Ok(Some((index, self.cached_frame(index, cache)?)))
    }

    /// Map a half-open geographic coordinate to the exact provider cell and tile address.
    pub fn cell_index(&self, frame: FrameDescriptor, lat_udeg: i32, lon_udeg: i32) -> Result<Option<CellIndex>, Error> {
        let south = self.header.south_lat_udeg as i64;
        let west = self.header.west_lon_udeg as i64;
        let north = self.header.north_lat_udeg as i64;
        let east = self.header.east_lon_udeg as i64;
        let lat = lat_udeg as i64;
        let lon = lon_udeg as i64;
        if lat < south || lat >= north || lon < west || lon >= east {
            return Ok(None);
        }
        let row = lat
            .checked_sub(south)
            .and_then(|delta| delta.checked_mul(frame.height as i64))
            .and_then(|value| value.checked_div(north - south))
            .ok_or(FormatError::Bounds)?;
        let column = lon
            .checked_sub(west)
            .and_then(|delta| delta.checked_mul(frame.width as i64))
            .and_then(|value| value.checked_div(east - west))
            .ok_or(FormatError::Bounds)?;
        let row = u16::try_from(row).map_err(|_| FormatError::Bounds)?;
        let column = u16::try_from(column).map_err(|_| FormatError::Bounds)?;
        let tile_columns = (frame.width as u32).div_ceil(TILE_EDGE as u32);
        let tile_index =
            checked_add(checked_mul(row as u32 / TILE_EDGE as u32, tile_columns)?, column as u32 / TILE_EDGE as u32)?;
        let cell_in_tile = (row as usize % TILE_EDGE) * TILE_EDGE + column as usize % TILE_EDGE;
        Ok(Some(CellIndex {
            row,
            column,
            tile_index,
            cell_in_tile: u16::try_from(cell_in_tile).map_err(|_| FormatError::Bounds)?,
        }))
    }

    /// Fetch the exact nearest-neighbour intensity for an in-bounds coordinate.
    pub fn intensity_at(
        &self,
        frame_index: usize,
        lat_udeg: i32,
        lon_udeg: i32,
        cache: &mut WeatherCache,
    ) -> Result<Option<u8>, Error> {
        let frame = self.cached_frame(frame_index, cache)?;
        let Some(cell) = self.cell_index(frame, lat_udeg, lon_udeg)? else { return Ok(None) };
        let tile = self.decode_tile_cached(frame_index, cell.tile_index, cache)?;
        Ok(Some(tile[cell.cell_in_tile as usize]))
    }

    /// Decode one tile through the fixed cache.
    pub fn decode_tile_cached<'cache>(
        &self,
        frame_index: usize,
        tile_index: u32,
        cache: &'cache mut WeatherCache,
    ) -> Result<&'cache [u8; TILE_CELLS], Error> {
        let generation = self.header.generation;
        if cache.tile_valid
            && cache.tile_generation == generation
            && cache.tile_bundle_crc == self.header.crc32
            && cache.tile_frame as usize == frame_index
            && cache.tile_index == tile_index
        {
            return Ok(&cache.tile);
        }

        let frame = self.cached_frame(frame_index, cache)?;
        let entry = self.cached_tile_entry(frame_index, frame, tile_index, cache)?;
        let len = entry.encoded_len as usize;
        if len == 0 || len > RAW4_LEN {
            return Err(FormatError::TileCodec.into());
        }
        let mut encoded = [0u8; RAW4_LEN];
        self.source.read_at(entry.data_offset, &mut encoded[..len])?;
        obcw::decode_tile_payload(entry, &encoded[..len], &mut cache.tile)?;
        cache.tile_generation = generation;
        cache.tile_bundle_crc = self.header.crc32;
        cache.tile_frame = u16::try_from(frame_index).map_err(|_| FormatError::Bounds)?;
        cache.tile_index = tile_index;
        cache.tile_valid = true;
        Ok(&cache.tile)
    }

    fn cached_frame(&self, frame_index: usize, cache: &mut WeatherCache) -> Result<FrameDescriptor, Error> {
        if frame_index >= self.header.frame_count as usize {
            return Err(FormatError::Bounds.into());
        }
        let frame_index_u16 = u16::try_from(frame_index).map_err(|_| FormatError::Bounds)?;
        if cache.frame_valid
            && cache.frame_generation == self.header.generation
            && cache.frame_bundle_crc == self.header.crc32
            && cache.frame_index == frame_index_u16
        {
            return Ok(cache.frame);
        }
        let frame = self.frame(frame_index)?;
        cache.frame_generation = self.header.generation;
        cache.frame_bundle_crc = self.header.crc32;
        cache.frame_index = frame_index_u16;
        cache.frame = frame;
        cache.frame_valid = true;
        Ok(frame)
    }

    fn cached_tile_entry(
        &self,
        frame_index: usize,
        frame: FrameDescriptor,
        tile_index: u32,
        cache: &mut WeatherCache,
    ) -> Result<TileEntry, Error> {
        if tile_index >= frame.tile_count {
            return Err(FormatError::Bounds.into());
        }
        let frame_index_u16 = u16::try_from(frame_index).map_err(|_| FormatError::Bounds)?;
        let first = (tile_index / DIRECTORY_WINDOW_ENTRIES as u32) * DIRECTORY_WINDOW_ENTRIES as u32;
        let hit = cache.directory_valid
            && cache.directory_generation == self.header.generation
            && cache.directory_bundle_crc == self.header.crc32
            && cache.directory_frame == frame_index_u16
            && cache.directory_first == first
            && tile_index - first < cache.directory_len as u32;
        if !hit {
            let count = (frame.tile_count - first).min(DIRECTORY_WINDOW_ENTRIES as u32) as usize;
            let mut bytes = [0u8; DIRECTORY_WINDOW_ENTRIES * TILE_DIRECTORY_ENTRY_LEN];
            let offset =
                checked_add(frame.tile_directory_offset, checked_mul(first, TILE_DIRECTORY_ENTRY_LEN as u32)?)?;
            self.source.read_at(offset, &mut bytes[..count * TILE_DIRECTORY_ENTRY_LEN])?;
            for (index, entry) in cache.directory[..count].iter_mut().enumerate() {
                let start = index * TILE_DIRECTORY_ENTRY_LEN;
                *entry = obcw::decode_tile_entry(
                    bytes[start..start + TILE_DIRECTORY_ENTRY_LEN].try_into().map_err(|_| FormatError::Bounds)?,
                )?;
            }
            cache.directory_generation = self.header.generation;
            cache.directory_bundle_crc = self.header.crc32;
            cache.directory_frame = frame_index_u16;
            cache.directory_first = first;
            cache.directory_len = count as u8;
            cache.directory_valid = true;
        }
        Ok(cache.directory[(tile_index - first) as usize])
    }
}

/// Exact-size, fallible iterator over the fixed 24 hourly records.
pub struct HourlyIter<'reader, 'source, S: ByteSource + ?Sized> {
    reader: &'reader WeatherReader<'source, S>,
    next: usize,
}

impl<S: ByteSource + ?Sized> Iterator for HourlyIter<'_, '_, S> {
    type Item = Result<(i64, HourlyRecord), Error>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= HOURLY_COUNT {
            return None;
        }
        let index = self.next;
        self.next += 1;
        let valid_at = self
            .reader
            .header
            .valid_from
            .checked_add(index as i64 * HOURLY_INTERVAL_SECONDS as i64)
            .ok_or(Error::Format(FormatError::Timestamp));
        Some(valid_at.and_then(|timestamp| self.reader.hourly(index).map(|record| (timestamp, record))))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = HOURLY_COUNT - self.next;
        (remaining, Some(remaining))
    }
}

impl<S: ByteSource + ?Sized> ExactSizeIterator for HourlyIter<'_, '_, S> {}

const _: () = assert!(core::mem::size_of::<Header>() == 72);
const _: () = assert!(READER_CACHE_RESIDENT_BYTES < 2 * 1024, "reader + weather cache exceeds the 2 KiB target");
const _: () = assert!(READER_CACHE_RESIDENT_BYTES <= 4 * 1024, "reader + weather cache exceeds the 4 KiB hard ceiling");
const _: () = assert!(core::mem::size_of::<WeatherCache>() < 1024);

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use obc_crc::Crc32;
    use obc_formats::io::{Error as SourceError, SliceSource};
    use std::vec::Vec;

    const DWD: &[u8] = include_bytes!("../../../specs/vectors/weather-dwd-96x96-9f.obcw");

    struct CountingSource<'a> {
        bytes: &'a [u8],
        calls: Cell<usize>,
        bytes_read: Cell<usize>,
        blocks_touched: Cell<usize>,
    }

    impl<'a> CountingSource<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, calls: Cell::new(0), bytes_read: Cell::new(0), blocks_touched: Cell::new(0) }
        }
    }

    impl ByteSource for CountingSource<'_> {
        fn read_at(&self, offset: u32, out: &mut [u8]) -> Result<(), SourceError> {
            let start = offset as usize;
            let end = start.checked_add(out.len()).ok_or(SourceError::BadOffset)?;
            out.copy_from_slice(self.bytes.get(start..end).ok_or(SourceError::BadOffset)?);
            self.calls.set(self.calls.get() + 1);
            self.bytes_read.set(self.bytes_read.get() + out.len());
            if !out.is_empty() {
                let first_block = start / 512;
                let last_block = (end - 1) / 512;
                self.blocks_touched.set(self.blocks_touched.get() + last_block - first_block + 1);
            }
            Ok(())
        }

        fn len(&self) -> u32 {
            self.bytes.len() as u32
        }
    }

    #[test]
    fn random_tile_io_is_bounded_and_cache_hits_are_measured() {
        let source = CountingSource::new(DWD);
        let reader = WeatherReader::open(&source).unwrap();
        source.calls.set(0);
        source.bytes_read.set(0);
        source.blocks_touched.set(0);
        let mut cache = WeatherCache::new();

        reader.decode_tile_cached(0, 0, &mut cache).unwrap();
        assert_eq!((source.calls.get(), source.bytes_read.get()), (3, obcw::FRAME_DESCRIPTOR_LEN + 48 + RAW4_LEN));
        reader.decode_tile_cached(0, 0, &mut cache).unwrap();
        assert_eq!(source.calls.get(), 3, "tile hit performs zero reads");
        reader.decode_tile_cached(0, 1, &mut cache).unwrap();
        assert_eq!(source.calls.get(), 4, "same directory window reads only one payload");
        reader.decode_tile_cached(0, 8, &mut cache).unwrap();
        assert_eq!(source.calls.get(), 6, "same frame, cold directory window is two reads");
        reader.decode_tile_cached(1, 8, &mut cache).unwrap();
        assert_eq!(source.calls.get(), 9, "cold random frame/tile is capped at three reads");

        let mut max_calls = 0;
        let mut max_blocks = 0;
        for frame in 0..reader.header().frame_count as usize {
            let descriptor = reader.frame(frame).unwrap();
            for tile in 0..descriptor.tile_count {
                cache.clear();
                source.calls.set(0);
                source.blocks_touched.set(0);
                reader.decode_tile_cached(frame, tile, &mut cache).unwrap();
                max_calls = max_calls.max(source.calls.get());
                max_blocks = max_blocks.max(source.blocks_touched.get());
            }
        }
        assert_eq!(max_calls, 3, "all 324 DWD-shaped random lookups stay at the three-call ceiling");
        assert_eq!(max_blocks, 5, "measured 512-byte block-touch ceiling across all random DWD lookups");
    }

    #[test]
    fn cache_keys_include_generation_and_bundle_identity() {
        let first = DWD.to_vec();
        let mut second = first.clone();
        second[obcw::HDR_GENERATION..obcw::HDR_GENERATION + 4].copy_from_slice(&99u32.to_le_bytes());
        second[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].fill(0);
        let crc = Crc32::checksum(&second);
        second[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].copy_from_slice(&crc.to_le_bytes());
        let source_a = CountingSource::new(&first);
        let source_b = CountingSource::new(&second);
        let reader_a = WeatherReader::open(&source_a).unwrap();
        let reader_b = WeatherReader::open(&source_b).unwrap();
        let mut cache = WeatherCache::new();
        reader_a.decode_tile_cached(0, 0, &mut cache).unwrap();
        source_b.calls.set(0);
        reader_b.decode_tile_cached(0, 0, &mut cache).unwrap();
        assert_eq!(source_b.calls.get(), 3, "a new generation cannot hit old frame/directory/tile state");

        let mut same_generation = first.clone();
        same_generation[obcw::HDR_REQUEST_ID..obcw::HDR_REQUEST_ID + 4].copy_from_slice(&123u32.to_le_bytes());
        same_generation[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].fill(0);
        let crc = Crc32::checksum(&same_generation);
        same_generation[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].copy_from_slice(&crc.to_le_bytes());
        let source_c = CountingSource::new(&same_generation);
        let reader_c = WeatherReader::open(&source_c).unwrap();
        reader_a.decode_tile_cached(0, 0, &mut cache).unwrap();
        source_c.calls.set(0);
        reader_c.decode_tile_cached(0, 0, &mut cache).unwrap();
        assert_eq!(source_c.calls.get(), 3, "equal-generation replacement is separated by bundle CRC");
    }

    #[test]
    fn timestamp_iteration_and_half_open_geography_are_exact() {
        let reader = WeatherReader::open(&SliceSource(DWD)).unwrap();
        let mut cache = WeatherCache::new();
        let header = reader.header();
        let (index, valid_at, _) = reader.hourly_at(header.valid_from + 3_601).unwrap().unwrap();
        assert_eq!((index, valid_at), (1, header.valid_from + 3_600));
        assert!(reader.hourly_at(header.valid_from - 1).unwrap().is_none());
        assert!(reader.hourly_at(header.valid_from + 24 * 3_600).unwrap().is_none());
        assert_eq!(reader.hourly_iter().count(), HOURLY_COUNT);

        let (frame_index, frame) = reader.frame_at_or_before(header.valid_from + 901, &mut cache).unwrap().unwrap();
        assert_eq!(frame_index, 1);
        assert!(reader.frame_at(frame.valid_at, &mut cache).unwrap().is_some());
        let south_west = reader.cell_index(frame, header.south_lat_udeg, header.west_lon_udeg).unwrap().unwrap();
        assert_eq!((south_west.row, south_west.column, south_west.tile_index, south_west.cell_in_tile), (0, 0, 0, 0));
        let north_east =
            reader.cell_index(frame, header.north_lat_udeg - 1, header.east_lon_udeg - 1).unwrap().unwrap();
        assert_eq!((north_east.row, north_east.column), (frame.height - 1, frame.width - 1));
        assert!(reader.cell_index(frame, header.north_lat_udeg, header.west_lon_udeg).unwrap().is_none());
        assert!(reader.cell_index(frame, header.south_lat_udeg, header.east_lon_udeg).unwrap().is_none());
    }

    #[test]
    fn resident_budget_is_far_below_both_gates() {
        let resident = std::hint::black_box(READER_CACHE_RESIDENT_BYTES);
        assert!(core::mem::size_of::<WeatherCache>() < 1024);
        assert!(resident < 2 * 1024);
        assert!(resident <= 4 * 1024);
        assert!(core::mem::size_of::<Vec<u8>>() < resident, "sanity: reports a concrete type size");
    }
}
