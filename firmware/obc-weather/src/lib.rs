//! Allocation-free OBCW bundle traversal.
//!
//! [`WeatherReader`] validates a random-access [`ByteSource`] and fetches hourly records, frame
//! descriptors and one 16 x 16 rain tile at a time. It owns no storage, A/B cache, scheduling,
//! alert or rendering policy; WX7 can compose those around this seam without moving byte rules
//! out of `obc-formats`.

#![no_std]
#![forbid(unsafe_code)]

use obc_crc::Crc32;
use obc_formats::io::{ByteSource, Error as SourceError};
use obc_formats::obcw::{
    self, DecodeError as FormatError, FrameDescriptor, Header, HourlyRecord, TileEntry, FRAME_DESCRIPTOR_LEN,
    HEADER_LEN, HOURLY_COUNT, HOURLY_RECORD_LEN, RAW4_LEN, TILE_CELLS, TILE_DIRECTORY_ENTRY_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Source(SourceError),
    Format(FormatError),
}

impl From<SourceError> for Error {
    fn from(value: SourceError) -> Self {
        Self::Source(value)
    }
}

impl From<FormatError> for Error {
    fn from(value: FormatError) -> Self {
        Self::Format(value)
    }
}

/// Validated, allocation-free view of one stable OBCW byte source.
///
/// Resident state is only the source reference plus a parsed fixed header. Tile decode uses a
/// 128-byte stack buffer and writes into a caller-owned 256-byte output.
pub struct WeatherReader<'a, S: ByteSource + ?Sized> {
    source: &'a S,
    header: Header,
}

impl<'a, S: ByteSource + ?Sized> WeatherReader<'a, S> {
    pub fn open(source: &'a S) -> Result<Self, Error> {
        let mut bytes = [0u8; HEADER_LEN];
        source.read_at(0, &mut bytes)?;
        let header = obcw::decode_header(&bytes)?;
        if header.total_len != source.len() {
            return Err(FormatError::TotalLength.into());
        }
        let reader = Self { source, header };
        if reader.bundle_crc32()? != header.crc32 {
            return Err(FormatError::Crc.into());
        }
        reader.validate_sections()?;
        Ok(reader)
    }

    pub const fn header(&self) -> Header {
        self.header
    }

    pub fn hourly(&self, index: usize) -> Result<HourlyRecord, Error> {
        if index >= HOURLY_COUNT {
            return Err(FormatError::Bounds.into());
        }
        let offset = checked_add(HEADER_LEN as u32, checked_mul(index as u32, HOURLY_RECORD_LEN as u32)?)?;
        let mut bytes = [0u8; HOURLY_RECORD_LEN];
        self.source.read_at(offset, &mut bytes)?;
        Ok(obcw::decode_hourly_record(&bytes)?)
    }

    pub fn frame(&self, index: usize) -> Result<FrameDescriptor, Error> {
        if index >= self.header.frame_count as usize {
            return Err(FormatError::Bounds.into());
        }
        let base = HEADER_LEN as u32 + (HOURLY_COUNT * HOURLY_RECORD_LEN) as u32;
        let offset = checked_add(base, checked_mul(index as u32, FRAME_DESCRIPTOR_LEN as u32)?)?;
        let mut bytes = [0u8; FRAME_DESCRIPTOR_LEN];
        self.source.read_at(offset, &mut bytes)?;
        Ok(obcw::decode_frame_descriptor(&bytes)?)
    }

    pub fn tile_entry(&self, frame: FrameDescriptor, index: u32) -> Result<TileEntry, Error> {
        if index >= frame.tile_count {
            return Err(FormatError::Bounds.into());
        }
        let offset = checked_add(frame.tile_directory_offset, checked_mul(index, TILE_DIRECTORY_ENTRY_LEN as u32)?)?;
        let mut bytes = [0u8; TILE_DIRECTORY_ENTRY_LEN];
        self.source.read_at(offset, &mut bytes)?;
        Ok(obcw::decode_tile_entry(&bytes)?)
    }

    /// Decode one independently addressable tile. The output is always exactly 256 intensity
    /// cells; malformed RLE is rejected before any out-of-bounds write.
    pub fn decode_tile(&self, frame_index: usize, tile_index: u32, out: &mut [u8; TILE_CELLS]) -> Result<(), Error> {
        let frame = self.frame(frame_index)?;
        let entry = self.tile_entry(frame, tile_index)?;
        let mut encoded = [0u8; RAW4_LEN];
        let len = entry.encoded_len as usize;
        if len == 0 || len > RAW4_LEN {
            return Err(FormatError::TileCodec.into());
        }
        self.source.read_at(entry.data_offset, &mut encoded[..len])?;
        Ok(obcw::decode_tile_payload(entry, &encoded[..len], out)?)
    }

    fn validate_sections(&self) -> Result<(), Error> {
        let mut previous_hour = None;
        for index in 0..HOURLY_COUNT {
            let record = self.hourly(index)?;
            obcw::validate_hourly(&record)?;
            let valid_at =
                self.header.valid_from.checked_add(record.valid_time_offset_s as i64).ok_or(FormatError::Timestamp)?;
            if valid_at > self.header.valid_until || previous_hour.is_some_and(|prior| valid_at <= prior) {
                return Err(FormatError::Timestamp.into());
            }
            previous_hour = Some(valid_at);
        }

        let descriptor_bytes = checked_mul(self.header.frame_count as u32, FRAME_DESCRIPTOR_LEN as u32)?;
        let mut cursor = checked_add(HEADER_LEN as u32 + (HOURLY_COUNT * HOURLY_RECORD_LEN) as u32, descriptor_bytes)?;
        let mut previous_frame = None;
        for frame_index in 0..self.header.frame_count as usize {
            let frame = self.frame(frame_index)?;
            obcw::validate_frame(&frame, &self.header)?;
            if previous_frame.is_some_and(|prior| frame.valid_at <= prior) {
                return Err(FormatError::Timestamp.into());
            }
            previous_frame = Some(frame.valid_at);
            if frame.tile_directory_offset != cursor {
                return Err(FormatError::SectionLayout.into());
            }
            let directory_len = checked_mul(frame.tile_count, TILE_DIRECTORY_ENTRY_LEN as u32)?;
            let data_start = checked_add(cursor, directory_len)?;
            if frame.tile_data_offset != data_start {
                return Err(FormatError::SectionLayout.into());
            }
            let mut payload_cursor = data_start;
            for tile_index in 0..frame.tile_count {
                let entry = self.tile_entry(frame, tile_index)?;
                if entry.data_offset != payload_cursor || entry.decoded_cells as usize != TILE_CELLS {
                    return Err(FormatError::TileDirectory.into());
                }
                let mut encoded = [0u8; RAW4_LEN];
                let len = entry.encoded_len as usize;
                if len == 0 || len > RAW4_LEN {
                    return Err(FormatError::TileCodec.into());
                }
                self.source.read_at(entry.data_offset, &mut encoded[..len])?;
                obcw::validate_tile_payload(entry, &encoded[..len])?;
                let mut decoded = [0u8; TILE_CELLS];
                obcw::decode_tile_payload(entry, &encoded[..len], &mut decoded)?;
                obcw::validate_tile_padding(frame.width, frame.height, tile_index, &decoded)?;
                payload_cursor = checked_add(payload_cursor, entry.encoded_len as u32)?;
            }
            if payload_cursor != checked_add(frame.tile_data_offset, frame.tile_data_len)? {
                return Err(FormatError::TileDirectory.into());
            }
            cursor = payload_cursor;
        }
        if cursor != self.header.total_len {
            return Err(FormatError::SectionLayout.into());
        }
        Ok(())
    }

    fn bundle_crc32(&self) -> Result<u32, Error> {
        let mut hasher = Crc32::new();
        let mut offset = 0u32;
        let mut buffer = [0u8; 128];
        while offset < self.header.total_len {
            let take = (self.header.total_len - offset).min(buffer.len() as u32) as usize;
            self.source.read_at(offset, &mut buffer[..take])?;
            for (index, byte) in buffer[..take].iter_mut().enumerate() {
                let absolute = offset + index as u32;
                if (obcw::HDR_CRC32 as u32..(obcw::HDR_CRC32 + 4) as u32).contains(&absolute) {
                    *byte = 0;
                }
            }
            hasher.update(&buffer[..take]);
            offset = checked_add(offset, take as u32)?;
        }
        Ok(hasher.finalize())
    }
}

fn checked_add(left: u32, right: u32) -> Result<u32, Error> {
    left.checked_add(right).ok_or_else(|| FormatError::Bounds.into())
}

fn checked_mul(left: u32, right: u32) -> Result<u32, Error> {
    left.checked_mul(right).ok_or_else(|| FormatError::Bounds.into())
}

const _: () = assert!(core::mem::size_of::<Header>() == 72);

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::SliceSource;
    use obc_formats::obcw::{
        encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, CONDITION_CLEAR, INTENSITY_DRY,
        QUALITY_FORECAST,
    };
    use std::vec;
    use std::vec::Vec;

    fn valid_bundle() -> Vec<u8> {
        let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|index| HourlyRecord {
            valid_time_offset_s: index as u32 * 3_600,
            temperature_deci_c: 120,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: CONDITION_CLEAR,
            wind_from_deg: 225,
            wind_speed_deci_ms: 40,
            wind_gust_deci_ms: 60,
            flags: 0,
        });
        let tiles = [[INTENSITY_DRY; TILE_CELLS]];
        let frames = [RainFrameInput {
            valid_at: 1_800_000_900,
            width: 16,
            height: 16,
            cell_size_m: 1_000,
            quality_flags: QUALITY_FORECAST,
            tiles: &tiles,
        }];
        let input = BundleInput {
            generation: 1,
            request_id: 7,
            generated_at: 1_800_000_000,
            valid_from: 1_800_000_000,
            valid_until: 1_800_100_000,
            south_lat_udeg: 47_000_000,
            west_lon_udeg: 7_000_000,
            north_lat_udeg: 48_000_000,
            east_lon_udeg: 8_000_000,
            grid_origin_lat_udeg: 47_000_000,
            grid_origin_lon_udeg: 7_000_000,
            flags: 0,
            hourly: &hourly,
            frames: &frames,
        };
        let mut bytes = vec![0; encoded_len(&input).unwrap() as usize];
        encode_format(&input, &mut bytes).unwrap();
        bytes
    }

    #[test]
    fn validates_and_decodes_one_tile_with_bounded_resident_state() {
        let bytes = valid_bundle();
        let source = SliceSource(&bytes);
        let reader = WeatherReader::open(&source).unwrap();
        assert_eq!(reader.header().frame_count, 1);
        let mut tile = [9u8; TILE_CELLS];
        reader.decode_tile(0, 0, &mut tile).unwrap();
        assert_eq!(tile, [INTENSITY_DRY; TILE_CELLS]);
        assert_eq!(core::mem::size_of::<WeatherReader<'_, SliceSource<'_>>>(), 80);
        assert_eq!(RAW4_LEN + TILE_CELLS, 384, "largest explicit tile-validation scratch");
    }

    #[test]
    fn reader_never_panics_on_arbitrary_slices() {
        let mut state = 0xC0DE_1187u32;
        for len in 0..1_024usize {
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                bytes.push(state as u8);
            }
            let result = std::panic::catch_unwind(|| {
                let source = SliceSource(&bytes);
                let _ = WeatherReader::open(&source);
            });
            assert!(result.is_ok(), "reader panicked for {len} bytes");
        }
    }

    #[test]
    fn rejects_impossible_dimensions_and_non_nodata_edge_padding() {
        fn refresh_crc(bytes: &mut [u8]) {
            bytes[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].fill(0);
            let crc = Crc32::checksum(bytes);
            bytes[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].copy_from_slice(&crc.to_le_bytes());
        }

        let descriptor = HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN;
        let mut impossible = valid_bundle();
        impossible[descriptor + obcw::FRAME_WIDTH..descriptor + obcw::FRAME_WIDTH + 2]
            .copy_from_slice(&17u16.to_le_bytes());
        refresh_crc(&mut impossible);
        let source = SliceSource(&impossible);
        assert!(WeatherReader::open(&source).is_err());

        let mut bad_padding = valid_bundle();
        bad_padding[descriptor + obcw::FRAME_WIDTH..descriptor + obcw::FRAME_WIDTH + 2]
            .copy_from_slice(&15u16.to_le_bytes());
        refresh_crc(&mut bad_padding);
        let source = SliceSource(&bad_padding);
        assert!(WeatherReader::open(&source).is_err());
    }
}
