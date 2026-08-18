//! Allocation-free OBCW bundle traversal.
//!
//! [`WeatherReader`] validates a random-access [`ByteSource`] and fetches hourly records, frame
//! descriptors and one 16 x 16 rain tile at a time. The fixed cache and pure A/B generation policy
//! live beside the reader; filesystem adapters, scheduling, alerts and rendering do not.

#![no_std]
#![forbid(unsafe_code)]

use obc_crc::Crc32;
use obc_formats::io::{ByteSource, Error as SourceError};
use obc_formats::obcw::{
    self, DecodeError as FormatError, FrameDescriptor, Header, HourlyRecord, TileEntry, FRAME_DESCRIPTOR_LEN,
    HEADER_LEN, HOURLY_COUNT, HOURLY_RECORD_LEN, RAW4_LEN, TILE_CELLS, TILE_DIRECTORY_ENTRY_LEN,
};

mod cache;
mod slots;

pub use cache::{CellIndex, HourlyIter, WeatherCache, FRAME_CURRENT_CAP_S, READER_CACHE_RESIDENT_BYTES};
pub use slots::{
    candidate_is_newer, select_slots, validate_slot, validate_slot_with_magic, Candidate, SelectionReason, Slot,
    SlotSelection, SlotValidation, WEATHER_A_FILE, WEATHER_B_FILE,
};

/// CRC reads are deliberately large enough to avoid one SD seek per tile-sized block while
/// keeping open-time scratch comfortably below 2 KiB.
pub const OPEN_CRC_CHUNK_BYTES: usize = 512;
/// Fixed records decoded per validation read.
pub const OPEN_HOURLY_WINDOW_RECORDS: usize = 4;
/// Tile entries and their contiguous payloads decoded per validation window.
pub const OPEN_TILE_WINDOW_ENTRIES: usize = 4;
/// Largest explicit simultaneously-live validation scratch: directory bytes + parsed entries +
/// encoded payload window + one decoded tile.
pub const OPEN_VALIDATION_SCRATCH_BYTES: usize = OPEN_TILE_WINDOW_ENTRIES * TILE_DIRECTORY_ENTRY_LEN
    + OPEN_TILE_WINDOW_ENTRIES * core::mem::size_of::<TileEntry>()
    + OPEN_TILE_WINDOW_ENTRIES * RAW4_LEN
    + TILE_CELLS;

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

/// Proof that one **stable** byte source was fully validated as canonical OBCW.
///
/// `WeatherReader::open` performs the expensive whole-object CRC and tile/layout validation once,
/// then [`ValidatedBundle::reader`] re-borrows the same session-stable object using only its stored
/// header and one matching-header read. This is the mount token filesystem hosts retain beside an open file;
/// its fields are private, so callers cannot turn an unvalidated header into a fast reader.
///
/// The source must remain byte-stable while the token is used. The device guarantees that by
/// holding the active A/B slot read-only and writing only the inactive slot; the simulator replaces
/// the token whenever it replaces/re-anchors its byte vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedBundle {
    header: Header,
}

impl ValidatedBundle {
    pub const fn header(self) -> Header {
        self.header
    }

    /// Open a cheap reader over the stable source this token belongs to. Length plus the complete
    /// decoded header (including the bundle CRC) must match the validated proof, preventing a token
    /// from being paired with another equal-length OBCW object. This is one header read, never the
    /// whole-object CRC/tile walk.
    pub fn reader<'a, S: ByteSource + ?Sized>(self, source: &'a S) -> Result<WeatherReader<'a, S>, Error> {
        if source.len() != u64::from(self.header.total_len) {
            return Err(FormatError::TotalLength.into());
        }
        let mut bytes = [0u8; HEADER_LEN];
        source.read_at(0, &mut bytes)?;
        if obcw::decode_header(&bytes)? != self.header {
            return Err(FormatError::Crc.into());
        }
        Ok(WeatherReader { source, header: self.header })
    }
}

impl<'a, S: ByteSource + ?Sized> WeatherReader<'a, S> {
    #[inline(never)]
    pub fn open(source: &'a S) -> Result<Self, Error> {
        let mut bytes = [0u8; HEADER_LEN];
        source.read_at(0, &mut bytes)?;
        let header = obcw::decode_header(&bytes)?;
        if u64::from(header.total_len) != source.len() {
            return Err(FormatError::TotalLength.into());
        }
        let reader = Self { source, header };
        if reader.bundle_crc32(&bytes)? != header.crc32 {
            return Err(FormatError::Crc.into());
        }
        reader.validate_sections()?;
        Ok(reader)
    }

    /// Capture the private fast-reopen proof after this reader completed full validation.
    pub const fn validated(&self) -> ValidatedBundle {
        ValidatedBundle { header: self.header }
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
        self.source.read_at(offset.into(), &mut bytes)?;
        Ok(obcw::decode_hourly_record(&bytes)?)
    }

    /// Read the fixed hourly section in the same four-record windows used by validation.
    ///
    /// A snapshot always needs all 24 records. Calling [`Self::hourly`] for each one turns that
    /// contiguous 576-byte section into 24 small SD transactions; six bounded 96-byte reads carry
    /// the exact same decoded records without growing the reader or allocating.
    pub fn hourly_records(&self) -> Result<[HourlyRecord; HOURLY_COUNT], Error> {
        let mut records = [HourlyRecord {
            valid_time_offset_s: 0,
            temperature_deci_c: 0,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: 0,
            wind_from_deg: 0,
            wind_speed_deci_ms: 0,
            wind_gust_deci_ms: 0,
            flags: 0,
        }; HOURLY_COUNT];
        let mut bytes = [0u8; OPEN_HOURLY_WINDOW_RECORDS * HOURLY_RECORD_LEN];
        for first in (0..HOURLY_COUNT).step_by(OPEN_HOURLY_WINDOW_RECORDS) {
            let count = (HOURLY_COUNT - first).min(OPEN_HOURLY_WINDOW_RECORDS);
            let byte_len = count * HOURLY_RECORD_LEN;
            let offset = checked_add(HEADER_LEN as u32, checked_mul(first as u32, HOURLY_RECORD_LEN as u32)?)?;
            self.source.read_at(offset.into(), &mut bytes[..byte_len])?;
            for local in 0..count {
                let start = local * HOURLY_RECORD_LEN;
                records[first + local] = obcw::decode_hourly_record(
                    bytes[start..start + HOURLY_RECORD_LEN].try_into().map_err(|_| FormatError::Bounds)?,
                )?;
            }
        }
        Ok(records)
    }

    pub fn frame(&self, index: usize) -> Result<FrameDescriptor, Error> {
        if index >= self.header.frame_count as usize {
            return Err(FormatError::Bounds.into());
        }
        let base = HEADER_LEN as u32 + (HOURLY_COUNT * HOURLY_RECORD_LEN) as u32;
        let offset = checked_add(base, checked_mul(index as u32, FRAME_DESCRIPTOR_LEN as u32)?)?;
        let mut bytes = [0u8; FRAME_DESCRIPTOR_LEN];
        self.source.read_at(offset.into(), &mut bytes)?;
        Ok(obcw::decode_frame_descriptor(&bytes)?)
    }

    pub fn tile_entry(&self, frame: FrameDescriptor, index: u32) -> Result<TileEntry, Error> {
        if index >= frame.tile_count {
            return Err(FormatError::Bounds.into());
        }
        let offset = checked_add(frame.tile_directory_offset, checked_mul(index, TILE_DIRECTORY_ENTRY_LEN as u32)?)?;
        let mut bytes = [0u8; TILE_DIRECTORY_ENTRY_LEN];
        self.source.read_at(offset.into(), &mut bytes)?;
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
        self.source.read_at(entry.data_offset.into(), &mut encoded[..len])?;
        Ok(obcw::decode_tile_payload(entry, &encoded[..len], out)?)
    }

    #[inline(never)]
    fn validate_sections(&self) -> Result<(), Error> {
        let mut hourly_bytes = [0u8; OPEN_HOURLY_WINDOW_RECORDS * HOURLY_RECORD_LEN];
        for first in (0..HOURLY_COUNT).step_by(OPEN_HOURLY_WINDOW_RECORDS) {
            let count = (HOURLY_COUNT - first).min(OPEN_HOURLY_WINDOW_RECORDS);
            let byte_len = count * HOURLY_RECORD_LEN;
            let offset = checked_add(HEADER_LEN as u32, checked_mul(first as u32, HOURLY_RECORD_LEN as u32)?)?;
            self.source.read_at(offset.into(), &mut hourly_bytes[..byte_len])?;
            for local in 0..count {
                let start = local * HOURLY_RECORD_LEN;
                let record = obcw::decode_hourly_record(
                    hourly_bytes[start..start + HOURLY_RECORD_LEN].try_into().map_err(|_| FormatError::Bounds)?,
                )?;
                obcw::validate_hourly(&record)?;
                obcw::validate_hourly_time(first + local, &record, self.header.valid_from, self.header.valid_until)?;
            }
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
            let empty_entry = TileEntry { data_offset: 0, encoded_len: 0, decoded_cells: 0, codec: 0 };
            let mut entries = [empty_entry; OPEN_TILE_WINDOW_ENTRIES];
            let mut directory_bytes = [0u8; OPEN_TILE_WINDOW_ENTRIES * TILE_DIRECTORY_ENTRY_LEN];
            let mut encoded = [0u8; OPEN_TILE_WINDOW_ENTRIES * RAW4_LEN];
            let mut decoded = [0u8; TILE_CELLS];
            let mut first_tile = 0u32;
            while first_tile < frame.tile_count {
                let count = (frame.tile_count - first_tile).min(OPEN_TILE_WINDOW_ENTRIES as u32) as usize;
                let directory_byte_len = count * TILE_DIRECTORY_ENTRY_LEN;
                let directory_offset = checked_add(
                    frame.tile_directory_offset,
                    checked_mul(first_tile, TILE_DIRECTORY_ENTRY_LEN as u32)?,
                )?;
                self.source.read_at(directory_offset.into(), &mut directory_bytes[..directory_byte_len])?;

                let window_payload_start = payload_cursor;
                let mut window_payload_len = 0usize;
                for (local, entry_slot) in entries[..count].iter_mut().enumerate() {
                    let start = local * TILE_DIRECTORY_ENTRY_LEN;
                    let entry = obcw::decode_tile_entry(
                        directory_bytes[start..start + TILE_DIRECTORY_ENTRY_LEN]
                            .try_into()
                            .map_err(|_| FormatError::Bounds)?,
                    )?;
                    let len = entry.encoded_len as usize;
                    if entry.data_offset != payload_cursor || entry.decoded_cells as usize != TILE_CELLS {
                        return Err(FormatError::TileDirectory.into());
                    }
                    if len == 0 || len > RAW4_LEN {
                        return Err(FormatError::TileCodec.into());
                    }
                    *entry_slot = entry;
                    window_payload_len = window_payload_len.checked_add(len).ok_or(FormatError::Bounds)?;
                    payload_cursor = checked_add(payload_cursor, entry.encoded_len as u32)?;
                }
                self.source.read_at(window_payload_start.into(), &mut encoded[..window_payload_len])?;

                let mut encoded_cursor = 0usize;
                for (local, &entry) in entries[..count].iter().enumerate() {
                    let end = encoded_cursor.checked_add(entry.encoded_len as usize).ok_or(FormatError::Bounds)?;
                    let tile_bytes = &encoded[encoded_cursor..end];
                    obcw::decode_tile_payload(entry, tile_bytes, &mut decoded)?;
                    obcw::validate_tile_padding(frame.width, frame.height, first_tile + local as u32, &decoded)?;
                    encoded_cursor = end;
                }
                first_tile = checked_add(first_tile, count as u32)?;
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

    #[inline(never)]
    fn bundle_crc32(&self, header_bytes: &[u8; HEADER_LEN]) -> Result<u32, Error> {
        let mut hasher = Crc32::new();
        let mut crc_header = *header_bytes;
        crc_header[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].fill(0);
        hasher.update(&crc_header);
        let mut offset = HEADER_LEN as u32;
        let mut buffer = [0u8; OPEN_CRC_CHUNK_BYTES];
        while offset < self.header.total_len {
            let take = (self.header.total_len - offset).min(buffer.len() as u32) as usize;
            self.source.read_at(offset.into(), &mut buffer[..take])?;
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
const _: () = assert!(OPEN_VALIDATION_SCRATCH_BYTES == 864);
const _: () = assert!(OPEN_VALIDATION_SCRATCH_BYTES < 2 * 1024);

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::io::{ByteSource, SliceSource};
    use obc_formats::obcw::{
        encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, CONDITION_CLEAR, INTENSITY_DRY,
        QUALITY_FORECAST,
    };
    use std::cell::Cell;
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

    fn dwd_shaped_bundle() -> Vec<u8> {
        let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|index| HourlyRecord {
            valid_time_offset_s: index as u32 * obcw::HOURLY_INTERVAL_SECONDS,
            temperature_deci_c: 120,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: CONDITION_CLEAR,
            wind_from_deg: 225,
            wind_speed_deci_ms: 40,
            wind_gust_deci_ms: 60,
            flags: 0,
        });
        let tiles: Vec<[u8; TILE_CELLS]> =
            (0..36).map(|phase| core::array::from_fn(|index| ((index + phase) % 13) as u8)).collect();
        let frames: Vec<RainFrameInput<'_>> = (0..9)
            .map(|index| RainFrameInput {
                valid_at: 1_800_000_000 + index * 900,
                width: 96,
                height: 96,
                cell_size_m: 1_000,
                quality_flags: QUALITY_FORECAST,
                tiles: &tiles,
            })
            .collect();
        let input = BundleInput {
            generation: 1,
            request_id: 7,
            generated_at: 1_800_000_000,
            valid_from: 1_800_000_000,
            valid_until: 1_800_086_400,
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
        assert_eq!(bytes.len(), 46_480);
        bytes
    }

    fn refresh_crc(bytes: &mut [u8]) {
        bytes[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].fill(0);
        let crc = Crc32::checksum(bytes);
        bytes[obcw::HDR_CRC32..obcw::HDR_CRC32 + 4].copy_from_slice(&crc.to_le_bytes());
    }

    struct CountingSource<'a> {
        bytes: &'a [u8],
        calls: Cell<usize>,
        bytes_read: Cell<usize>,
    }

    impl ByteSource for CountingSource<'_> {
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), SourceError> {
            let start = offset as usize;
            let end = start.checked_add(out.len()).ok_or(SourceError::BadOffset)?;
            out.copy_from_slice(self.bytes.get(start..end).ok_or(SourceError::BadOffset)?);
            self.calls.set(self.calls.get() + 1);
            self.bytes_read.set(self.bytes_read.get() + out.len());
            Ok(())
        }

        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
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
        assert_eq!(OPEN_VALIDATION_SCRATCH_BYTES, 864, "largest open-validation scratch");
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
    fn valid_crc_structured_mutations_and_every_truncation_never_panic() {
        let seed = dwd_shaped_bundle();
        for length in 0..seed.len() {
            let result = std::panic::catch_unwind(|| {
                let source = SliceSource(&seed[..length]);
                WeatherReader::open(&source).is_err()
            });
            assert!(result.is_ok(), "reader panicked on valid-bundle truncation at {length}");
            assert!(result.unwrap(), "reader accepted truncation at {length}");
        }

        let frame_base = HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN;
        let first_directory = u32::from_le_bytes(
            seed[frame_base + obcw::FRAME_TILE_DIRECTORY_OFFSET..frame_base + obcw::FRAME_TILE_DIRECTORY_OFFSET + 4]
                .try_into()
                .unwrap(),
        ) as usize;
        let first_payload = u32::from_le_bytes(
            seed[frame_base + obcw::FRAME_TILE_DATA_OFFSET..frame_base + obcw::FRAME_TILE_DATA_OFFSET + 4]
                .try_into()
                .unwrap(),
        ) as usize;
        let ranges = [
            (HEADER_LEN, frame_base, "hourly"),
            (frame_base, frame_base + 9 * FRAME_DESCRIPTOR_LEN, "frame descriptors"),
            (first_directory, first_payload, "tile directory"),
            (first_payload, first_payload + 36 * RAW4_LEN, "tile payload"),
        ];
        for (start, end, label) in ranges {
            let mut rejected = 0usize;
            for case in 0..128usize {
                let mut mutated = seed.clone();
                let offset = start + (case * 97) % (end - start);
                mutated[offset] ^= 1 << (case % 8);
                refresh_crc(&mut mutated);
                let result = std::panic::catch_unwind(|| {
                    let source = SliceSource(&mutated);
                    WeatherReader::open(&source).is_err()
                });
                assert!(result.is_ok(), "reader panicked in {label} mutation {case} at {offset}");
                rejected += usize::from(result.unwrap());
            }
            assert!(rejected > 0, "structured {label} mutations never exercised a rejection path");
        }
    }

    #[test]
    fn dwd_open_io_budget_is_pinned() {
        let bytes = dwd_shaped_bundle();
        let source = CountingSource { bytes: &bytes, calls: Cell::new(0), bytes_read: Cell::new(0) };
        WeatherReader::open(&source).unwrap();
        assert_eq!(source.calls.get(), 269, "DWD open read_at budget");
        assert_eq!(source.bytes_read.get(), 92_848, "DWD open byte budget");
    }

    #[test]
    fn validated_mount_reopens_without_rereading_the_bundle() {
        let bytes = dwd_shaped_bundle();
        let source = CountingSource { bytes: &bytes, calls: Cell::new(0), bytes_read: Cell::new(0) };
        let reader = WeatherReader::open(&source).unwrap();
        let mount = reader.validated();
        let calls_after_validation = source.calls.get();
        let bytes_after_validation = source.bytes_read.get();

        let reopened = mount.reader(&source).unwrap();
        assert_eq!(reopened.header(), reader.header());
        assert_eq!(source.calls.get(), calls_after_validation + 1, "fast reopen reads only the header");
        assert_eq!(source.bytes_read.get(), bytes_after_validation + HEADER_LEN, "fast reopen reads only the header");

        let shorter = SliceSource(&bytes[..bytes.len() - 1]);
        assert_eq!(mount.reader(&shorter).err(), Some(Error::Format(FormatError::TotalLength)));

        let mut different = bytes.clone();
        different[obcw::HDR_GENERATION] ^= 1;
        let equal_length_other = SliceSource(&different);
        assert_eq!(mount.reader(&equal_length_other).err(), Some(Error::Format(FormatError::Crc)));
    }

    #[test]
    fn complete_hourly_section_uses_six_bounded_reads() {
        let bytes = dwd_shaped_bundle();
        let source = CountingSource { bytes: &bytes, calls: Cell::new(0), bytes_read: Cell::new(0) };
        let reader = WeatherReader::open(&source).unwrap();
        source.calls.set(0);
        source.bytes_read.set(0);

        let records = reader.hourly_records().unwrap();
        assert_eq!(source.calls.get(), 6);
        assert_eq!(source.bytes_read.get(), HOURLY_COUNT * HOURLY_RECORD_LEN);
        assert_eq!(records, core::array::from_fn(|index| reader.hourly(index).unwrap()));
    }

    #[test]
    fn rejects_compressible_raw_tile_with_valid_crc() {
        let mut bytes = dwd_shaped_bundle();
        let frame_base = HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN;
        let data_offset = u32::from_le_bytes(
            bytes[frame_base + obcw::FRAME_TILE_DATA_OFFSET..frame_base + obcw::FRAME_TILE_DATA_OFFSET + 4]
                .try_into()
                .unwrap(),
        ) as usize;
        bytes[data_offset..data_offset + RAW4_LEN].fill(0);
        refresh_crc(&mut bytes);
        let source = SliceSource(&bytes);
        assert!(matches!(WeatherReader::open(&source), Err(Error::Format(FormatError::TileCodec))));
    }

    #[test]
    fn rejects_impossible_dimensions_and_non_nodata_edge_padding() {
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
