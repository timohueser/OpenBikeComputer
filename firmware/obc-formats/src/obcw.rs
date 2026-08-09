//! OBCW v1 weather-bundle byte authority.
//!
//! The normative contract is `specs/OBCW_Spec.md`. This module deliberately owns every fixed
//! size, field offset, sentinel, checked slice codec and layout primitive used by Rust. The
//! allocation-free `ByteSource` traversal lives in `obc-weather`; storage/cache policy does not.

use obc_crc::Crc32;

pub const MAGIC: [u8; 4] = *b"OBCW";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 112;
pub const HOURLY_COUNT: usize = 24;
pub const HOURLY_RECORD_LEN: usize = 24;
pub const HOURLY_INTERVAL_SECONDS: u32 = 3_600;
pub const FRAME_DESCRIPTOR_LEN: usize = 48;
pub const TILE_DIRECTORY_ENTRY_LEN: usize = 12;
pub const TILE_EDGE: usize = 16;
pub const TILE_CELLS: usize = TILE_EDGE * TILE_EDGE;
pub const RAW4_LEN: usize = TILE_CELLS / 2;

pub const HDR_MAGIC: usize = 0;
pub const HDR_VERSION: usize = 4;
pub const HDR_HEADER_LEN: usize = 6;
pub const HDR_TOTAL_LEN: usize = 8;
pub const HDR_GENERATION: usize = 12;
pub const HDR_REQUEST_ID: usize = 16;
pub const HDR_GENERATED_AT: usize = 20;
pub const HDR_VALID_FROM: usize = 28;
pub const HDR_VALID_UNTIL: usize = 36;
pub const HDR_SOUTH_LAT: usize = 44;
pub const HDR_WEST_LON: usize = 48;
pub const HDR_NORTH_LAT: usize = 52;
pub const HDR_EAST_LON: usize = 56;
pub const HDR_GRID_ORIGIN_LAT: usize = 60;
pub const HDR_GRID_ORIGIN_LON: usize = 64;
pub const HDR_HOURLY_OFFSET: usize = 68;
pub const HDR_HOURLY_COUNT: usize = 72;
pub const HDR_HOURLY_RECORD_LEN: usize = 74;
pub const HDR_FRAME_DIRECTORY_OFFSET: usize = 76;
pub const HDR_FRAME_COUNT: usize = 80;
pub const HDR_FRAME_DESCRIPTOR_LEN: usize = 82;
pub const HDR_FLAGS: usize = 84;
pub const HDR_CRC32: usize = 88;
pub const HDR_RESERVED: usize = 92;

pub const FRAME_VALID_AT: usize = 0;
pub const FRAME_WIDTH: usize = 8;
pub const FRAME_HEIGHT: usize = 10;
pub const FRAME_CELL_SIZE_M: usize = 12;
pub const FRAME_TILE_EDGE: usize = 14;
pub const FRAME_RESERVED0: usize = 15;
pub const FRAME_TILE_DIRECTORY_OFFSET: usize = 16;
pub const FRAME_TILE_COUNT: usize = 20;
pub const FRAME_TILE_DATA_OFFSET: usize = 24;
pub const FRAME_TILE_DATA_LEN: usize = 28;
pub const FRAME_QUALITY_FLAGS: usize = 32;
pub const FRAME_RESERVED: usize = 36;

pub const TILE_DATA_OFFSET: usize = 0;
pub const TILE_ENCODED_LEN: usize = 4;
pub const TILE_DECODED_CELLS: usize = 6;
pub const TILE_CODEC: usize = 8;
pub const TILE_FLAGS: usize = 9;
pub const TILE_RESERVED: usize = 10;

pub const TILE_CODEC_RAW4: u8 = 0;
pub const TILE_CODEC_RLE4: u8 = 1;

/// Dry/transparent. This is real zero precipitation, never missing data.
pub const INTENSITY_DRY: u8 = 0;
/// Highest defined precipitation band (`>= 50 mm/h`).
pub const INTENSITY_MAX: u8 = 12;
/// `13` and `14` are reserved so corrupt nibbles can be rejected.
pub const INTENSITY_NODATA: u8 = 15;

pub const QUALITY_OBSERVED: u32 = 1 << 0;
pub const QUALITY_FORECAST: u32 = 1 << 1;
pub const QUALITY_PARTIAL_COVERAGE: u32 = 1 << 2;
pub const QUALITY_DEGRADED: u32 = 1 << 3;
pub const QUALITY_KNOWN_MASK: u32 = QUALITY_OBSERVED | QUALITY_FORECAST | QUALITY_PARTIAL_COVERAGE | QUALITY_DEGRADED;

pub const CONDITION_CLEAR: u8 = 0;
pub const CONDITION_MOSTLY_CLEAR: u8 = 1;
pub const CONDITION_PARTLY_CLOUDY: u8 = 2;
pub const CONDITION_OVERCAST: u8 = 3;
pub const CONDITION_FOG: u8 = 4;
pub const CONDITION_DRIZZLE: u8 = 5;
pub const CONDITION_RAIN: u8 = 6;
pub const CONDITION_SLEET: u8 = 7;
pub const CONDITION_SNOW: u8 = 8;
pub const CONDITION_SHOWERS: u8 = 9;
pub const CONDITION_THUNDERSTORM: u8 = 10;
pub const CONDITION_HAIL: u8 = 11;
pub const CONDITION_WIND: u8 = 12;
pub const CONDITION_UNAVAILABLE: u8 = 0xFF;

pub const TEMP_UNAVAILABLE: i16 = i16::MIN;
pub const PRECIP_UNAVAILABLE: u16 = u16::MAX;
pub const PROBABILITY_UNAVAILABLE: u8 = u8::MAX;
pub const WIND_DIRECTION_UNAVAILABLE: u16 = u16::MAX;
pub const WIND_SPEED_UNAVAILABLE: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Bounds,
    Magic,
    Version,
    HeaderLength,
    TotalLength,
    Crc,
    Reserved,
    SectionLayout,
    Count,
    Timestamp,
    Geography,
    Hourly,
    Frame,
    TileDirectory,
    TileCodec,
    Intensity,
    Rle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    InvalidInput,
    LengthOverflow,
    OutputTooSmall,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub total_len: u32,
    pub generation: u32,
    pub request_id: u32,
    pub generated_at: i64,
    pub valid_from: i64,
    pub valid_until: i64,
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub north_lat_udeg: i32,
    pub east_lon_udeg: i32,
    pub grid_origin_lat_udeg: i32,
    pub grid_origin_lon_udeg: i32,
    pub frame_count: u16,
    pub flags: u32,
    pub crc32: u32,
}

/// Fixed-width following-hour values. Record `i` begins at `valid_from + i*3600`; amount and
/// probability describe `[valid_at, valid_at+3600)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HourlyRecord {
    pub valid_time_offset_s: u32,
    pub temperature_deci_c: i16,
    pub precipitation_tenth_mm: u16,
    pub precipitation_probability_pct: u8,
    pub condition: u8,
    pub wind_from_deg: u16,
    pub wind_speed_deci_ms: u16,
    pub wind_gust_deci_ms: u16,
    /// Reserved for future semantic qualifiers. V1 writers emit zero and readers reject nonzero.
    pub flags: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameDescriptor {
    pub valid_at: i64,
    pub width: u16,
    pub height: u16,
    pub cell_size_m: u16,
    pub tile_directory_offset: u32,
    pub tile_count: u32,
    pub tile_data_offset: u32,
    pub tile_data_len: u32,
    pub quality_flags: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEntry {
    pub data_offset: u32,
    pub encoded_len: u16,
    pub decoded_cells: u16,
    pub codec: u8,
}

#[derive(Debug)]
pub struct RainFrameInput<'a> {
    pub valid_at: i64,
    pub width: u16,
    pub height: u16,
    pub cell_size_m: u16,
    pub quality_flags: u32,
    /// Row-major tile order. Edge tiles still contain 16 x 16 cells; cells outside the declared
    /// grid are `INTENSITY_NODATA`.
    pub tiles: &'a [[u8; TILE_CELLS]],
}

#[derive(Debug)]
pub struct BundleInput<'a> {
    pub generation: u32,
    pub request_id: u32,
    pub generated_at: i64,
    pub valid_from: i64,
    pub valid_until: i64,
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub north_lat_udeg: i32,
    pub east_lon_udeg: i32,
    pub grid_origin_lat_udeg: i32,
    pub grid_origin_lon_udeg: i32,
    pub flags: u32,
    pub hourly: &'a [HourlyRecord; HOURLY_COUNT],
    pub frames: &'a [RainFrameInput<'a>],
}

/// Decode and validate the fixed header. Object length, CRC and pointed-to sections are reader
/// concerns; this function owns only the bytes and invariants stated by the header itself.
pub fn decode_header(bytes: &[u8; HEADER_LEN]) -> Result<Header, DecodeError> {
    if bytes[HDR_MAGIC..HDR_MAGIC + 4] != MAGIC {
        return Err(DecodeError::Magic);
    }
    if rd_u16(bytes, HDR_VERSION)? != VERSION {
        return Err(DecodeError::Version);
    }
    if rd_u16(bytes, HDR_HEADER_LEN)? as usize != HEADER_LEN {
        return Err(DecodeError::HeaderLength);
    }
    if bytes[HDR_RESERVED..].iter().any(|&byte| byte != 0) {
        return Err(DecodeError::Reserved);
    }
    let header = Header {
        total_len: rd_u32(bytes, HDR_TOTAL_LEN)?,
        generation: rd_u32(bytes, HDR_GENERATION)?,
        request_id: rd_u32(bytes, HDR_REQUEST_ID)?,
        generated_at: rd_i64(bytes, HDR_GENERATED_AT)?,
        valid_from: rd_i64(bytes, HDR_VALID_FROM)?,
        valid_until: rd_i64(bytes, HDR_VALID_UNTIL)?,
        south_lat_udeg: rd_i32(bytes, HDR_SOUTH_LAT)?,
        west_lon_udeg: rd_i32(bytes, HDR_WEST_LON)?,
        north_lat_udeg: rd_i32(bytes, HDR_NORTH_LAT)?,
        east_lon_udeg: rd_i32(bytes, HDR_EAST_LON)?,
        grid_origin_lat_udeg: rd_i32(bytes, HDR_GRID_ORIGIN_LAT)?,
        grid_origin_lon_udeg: rd_i32(bytes, HDR_GRID_ORIGIN_LON)?,
        frame_count: rd_u16(bytes, HDR_FRAME_COUNT)?,
        flags: rd_u32(bytes, HDR_FLAGS)?,
        crc32: rd_u32(bytes, HDR_CRC32)?,
    };
    if header.total_len < HEADER_LEN as u32 {
        return Err(DecodeError::TotalLength);
    }
    if rd_u32(bytes, HDR_HOURLY_OFFSET)? != HEADER_LEN as u32
        || rd_u16(bytes, HDR_HOURLY_COUNT)? as usize != HOURLY_COUNT
        || rd_u16(bytes, HDR_HOURLY_RECORD_LEN)? as usize != HOURLY_RECORD_LEN
        || rd_u32(bytes, HDR_FRAME_DIRECTORY_OFFSET)? != (HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN) as u32
        || rd_u16(bytes, HDR_FRAME_DESCRIPTOR_LEN)? as usize != FRAME_DESCRIPTOR_LEN
    {
        return Err(DecodeError::SectionLayout);
    }
    validate_header_semantics(&header)?;
    Ok(header)
}

pub fn encoded_len(input: &BundleInput<'_>) -> Result<u32, EncodeError> {
    validate_input(input)?;
    let mut total = (HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN)
        .checked_add(input.frames.len().checked_mul(FRAME_DESCRIPTOR_LEN).ok_or(EncodeError::LengthOverflow)?)
        .ok_or(EncodeError::LengthOverflow)?;
    for frame in input.frames {
        total = total
            .checked_add(frame.tiles.len().checked_mul(TILE_DIRECTORY_ENTRY_LEN).ok_or(EncodeError::LengthOverflow)?)
            .ok_or(EncodeError::LengthOverflow)?;
        for tile in frame.tiles {
            total = total.checked_add(encoded_tile_len(tile)).ok_or(EncodeError::LengthOverflow)?;
        }
    }
    u32::try_from(total).map_err(|_| EncodeError::LengthOverflow)
}

/// Encode the full `u32`-capacity format. Producer size policy deliberately lives outside this
/// byte authority.
pub fn encode_format(input: &BundleInput<'_>, out: &mut [u8]) -> Result<usize, EncodeError> {
    let total = encoded_len(input)? as usize;
    let bytes = out.get_mut(..total).ok_or(EncodeError::OutputTooSmall)?;
    bytes.fill(0);

    bytes[HDR_MAGIC..HDR_MAGIC + 4].copy_from_slice(&MAGIC);
    put_u16(bytes, HDR_VERSION, VERSION)?;
    put_u16(bytes, HDR_HEADER_LEN, HEADER_LEN as u16)?;
    put_u32(bytes, HDR_TOTAL_LEN, total as u32)?;
    put_u32(bytes, HDR_GENERATION, input.generation)?;
    put_u32(bytes, HDR_REQUEST_ID, input.request_id)?;
    put_i64(bytes, HDR_GENERATED_AT, input.generated_at)?;
    put_i64(bytes, HDR_VALID_FROM, input.valid_from)?;
    put_i64(bytes, HDR_VALID_UNTIL, input.valid_until)?;
    put_i32(bytes, HDR_SOUTH_LAT, input.south_lat_udeg)?;
    put_i32(bytes, HDR_WEST_LON, input.west_lon_udeg)?;
    put_i32(bytes, HDR_NORTH_LAT, input.north_lat_udeg)?;
    put_i32(bytes, HDR_EAST_LON, input.east_lon_udeg)?;
    put_i32(bytes, HDR_GRID_ORIGIN_LAT, input.grid_origin_lat_udeg)?;
    put_i32(bytes, HDR_GRID_ORIGIN_LON, input.grid_origin_lon_udeg)?;
    put_u32(bytes, HDR_HOURLY_OFFSET, HEADER_LEN as u32)?;
    put_u16(bytes, HDR_HOURLY_COUNT, HOURLY_COUNT as u16)?;
    put_u16(bytes, HDR_HOURLY_RECORD_LEN, HOURLY_RECORD_LEN as u16)?;
    let frame_base = HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN;
    put_u32(bytes, HDR_FRAME_DIRECTORY_OFFSET, frame_base as u32)?;
    put_u16(bytes, HDR_FRAME_COUNT, u16::try_from(input.frames.len()).map_err(|_| EncodeError::LengthOverflow)?)?;
    put_u16(bytes, HDR_FRAME_DESCRIPTOR_LEN, FRAME_DESCRIPTOR_LEN as u16)?;
    put_u32(bytes, HDR_FLAGS, input.flags)?;

    for (index, hourly) in input.hourly.iter().enumerate() {
        let offset = HEADER_LEN + index * HOURLY_RECORD_LEN;
        encode_hourly(hourly, &mut bytes[offset..offset + HOURLY_RECORD_LEN])?;
    }

    let mut tail = frame_base + input.frames.len() * FRAME_DESCRIPTOR_LEN;
    for (frame_index, frame) in input.frames.iter().enumerate() {
        let directory_offset = tail;
        let directory_len = frame.tiles.len() * TILE_DIRECTORY_ENTRY_LEN;
        let data_offset = directory_offset + directory_len;
        let data_len: usize = frame.tiles.iter().map(encoded_tile_len).sum();
        let descriptor_offset = frame_base + frame_index * FRAME_DESCRIPTOR_LEN;
        encode_frame(
            frame,
            directory_offset as u32,
            data_offset as u32,
            data_len as u32,
            &mut bytes[descriptor_offset..descriptor_offset + FRAME_DESCRIPTOR_LEN],
        )?;
        let mut payload = data_offset;
        for (tile_index, tile) in frame.tiles.iter().enumerate() {
            let encoded_len = encoded_tile_len(tile);
            let codec = if encoded_len < RAW4_LEN { TILE_CODEC_RLE4 } else { TILE_CODEC_RAW4 };
            let entry_offset = directory_offset + tile_index * TILE_DIRECTORY_ENTRY_LEN;
            encode_tile_entry(
                payload as u32,
                encoded_len as u16,
                codec,
                &mut bytes[entry_offset..entry_offset + TILE_DIRECTORY_ENTRY_LEN],
            )?;
            if codec == TILE_CODEC_RLE4 {
                encode_rle4(tile, &mut bytes[payload..payload + encoded_len]);
            } else {
                encode_raw4(tile, &mut bytes[payload..payload + RAW4_LEN]);
            }
            payload += encoded_len;
        }
        tail = data_offset + data_len;
    }
    debug_assert_eq!(tail, total);
    put_u32(bytes, HDR_CRC32, 0)?;
    let crc = Crc32::checksum(bytes);
    put_u32(bytes, HDR_CRC32, crc)?;
    Ok(total)
}

fn validate_input(input: &BundleInput<'_>) -> Result<(), EncodeError> {
    if input.frames.len() > u16::MAX as usize {
        return Err(EncodeError::LengthOverflow);
    }
    let header = Header {
        total_len: 0,
        generation: input.generation,
        request_id: input.request_id,
        generated_at: input.generated_at,
        valid_from: input.valid_from,
        valid_until: input.valid_until,
        south_lat_udeg: input.south_lat_udeg,
        west_lon_udeg: input.west_lon_udeg,
        north_lat_udeg: input.north_lat_udeg,
        east_lon_udeg: input.east_lon_udeg,
        grid_origin_lat_udeg: input.grid_origin_lat_udeg,
        grid_origin_lon_udeg: input.grid_origin_lon_udeg,
        frame_count: input.frames.len() as u16,
        flags: input.flags,
        crc32: 0,
    };
    validate_header_semantics(&header).map_err(|_| EncodeError::InvalidInput)?;
    for (index, hourly) in input.hourly.iter().enumerate() {
        validate_hourly(hourly).map_err(|_| EncodeError::InvalidInput)?;
        validate_hourly_time(index, hourly, input.valid_from, input.valid_until)
            .map_err(|_| EncodeError::InvalidInput)?;
    }
    let mut previous_frame = None;
    for frame in input.frames {
        let tile_count = expected_tile_count(frame.width, frame.height).ok_or(EncodeError::InvalidInput)?;
        if frame.tiles.len() != tile_count as usize
            || frame.cell_size_m == 0
            || frame.valid_at < input.valid_from
            || frame.valid_at > input.valid_until
            || previous_frame.is_some_and(|p| frame.valid_at <= p)
            || !valid_quality_flags(frame.quality_flags)
        {
            return Err(EncodeError::InvalidInput);
        }
        previous_frame = Some(frame.valid_at);
        for (tile_index, tile) in frame.tiles.iter().enumerate() {
            if tile.iter().any(|&v| !valid_intensity(v)) {
                return Err(EncodeError::InvalidInput);
            }
            validate_tile_padding(frame.width, frame.height, tile_index as u32, tile)
                .map_err(|_| EncodeError::InvalidInput)?;
        }
    }
    Ok(())
}

pub fn validate_header_semantics(header: &Header) -> Result<(), DecodeError> {
    if header.flags != 0 || header.generated_at <= 0 || header.valid_from > header.valid_until {
        return Err(DecodeError::Timestamp);
    }
    if !(-90_000_000..=90_000_000).contains(&header.south_lat_udeg)
        || !(-90_000_000..=90_000_000).contains(&header.north_lat_udeg)
        || !(-180_000_000..=180_000_000).contains(&header.west_lon_udeg)
        || !(-180_000_000..=180_000_000).contains(&header.east_lon_udeg)
        || header.south_lat_udeg >= header.north_lat_udeg
        || header.west_lon_udeg >= header.east_lon_udeg
        || header.grid_origin_lat_udeg != header.south_lat_udeg
        || header.grid_origin_lon_udeg != header.west_lon_udeg
    {
        return Err(DecodeError::Geography);
    }
    Ok(())
}

pub fn validate_hourly(record: &HourlyRecord) -> Result<(), DecodeError> {
    if record.flags != 0
        || (record.temperature_deci_c != TEMP_UNAVAILABLE && !(-1_000..=700).contains(&record.temperature_deci_c))
        || (record.precipitation_probability_pct != PROBABILITY_UNAVAILABLE
            && record.precipitation_probability_pct > 100)
        || !valid_condition(record.condition)
        || (record.wind_from_deg != WIND_DIRECTION_UNAVAILABLE && record.wind_from_deg > 359)
        || (record.wind_speed_deci_ms != WIND_SPEED_UNAVAILABLE && record.wind_speed_deci_ms > 2_000)
        || (record.wind_gust_deci_ms != WIND_SPEED_UNAVAILABLE && record.wind_gust_deci_ms > 2_000)
    {
        return Err(DecodeError::Hourly);
    }
    Ok(())
}

/// Validate v1's fixed following-hour schedule.
///
/// Record `i` represents `[valid_from + i*1h, valid_from + (i+1)*1h)`, so its
/// offset is not a provider-defined sample timestamp and the final interval end must remain
/// inside `valid_until`.
pub fn validate_hourly_time(
    index: usize,
    record: &HourlyRecord,
    valid_from: i64,
    valid_until: i64,
) -> Result<(), DecodeError> {
    if index >= HOURLY_COUNT || record.valid_time_offset_s != index as u32 * HOURLY_INTERVAL_SECONDS {
        return Err(DecodeError::Timestamp);
    }
    let valid_at = valid_from.checked_add(record.valid_time_offset_s as i64).ok_or(DecodeError::Timestamp)?;
    let interval_end = valid_at.checked_add(HOURLY_INTERVAL_SECONDS as i64).ok_or(DecodeError::Timestamp)?;
    if interval_end > valid_until {
        return Err(DecodeError::Timestamp);
    }
    Ok(())
}

pub fn validate_frame(frame: &FrameDescriptor, header: &Header) -> Result<(), DecodeError> {
    if frame.valid_at < header.valid_from
        || frame.valid_at > header.valid_until
        || frame.cell_size_m == 0
        || !valid_quality_flags(frame.quality_flags)
        || expected_tile_count(frame.width, frame.height) != Some(frame.tile_count)
    {
        return Err(DecodeError::Frame);
    }
    Ok(())
}

pub fn valid_quality_flags(flags: u32) -> bool {
    let source = flags & (QUALITY_OBSERVED | QUALITY_FORECAST);
    flags & !QUALITY_KNOWN_MASK == 0 && (source == QUALITY_OBSERVED || source == QUALITY_FORECAST)
}

pub const fn valid_intensity(value: u8) -> bool {
    value <= INTENSITY_MAX || value == INTENSITY_NODATA
}

pub const fn valid_condition(value: u8) -> bool {
    value <= CONDITION_WIND || value == CONDITION_UNAVAILABLE
}

pub const fn expected_tile_count(width: u16, height: u16) -> Option<u32> {
    if width == 0 || height == 0 {
        return None;
    }
    let cols = (width as u32).div_ceil(TILE_EDGE as u32);
    let rows = (height as u32).div_ceil(TILE_EDGE as u32);
    rows.checked_mul(cols)
}

/// Edge tiles are physically full-sized, but cells beyond the declared grid are always no-data.
pub fn validate_tile_padding(
    width: u16,
    height: u16,
    tile_index: u32,
    cells: &[u8; TILE_CELLS],
) -> Result<(), DecodeError> {
    let tile_count = expected_tile_count(width, height).ok_or(DecodeError::Frame)?;
    if tile_index >= tile_count {
        return Err(DecodeError::Bounds);
    }
    let tile_columns = (width as usize).div_ceil(TILE_EDGE);
    let tile_row = tile_index as usize / tile_columns;
    let tile_column = tile_index as usize % tile_columns;
    let valid_rows = (height as usize).saturating_sub(tile_row * TILE_EDGE).min(TILE_EDGE);
    let valid_columns = (width as usize).saturating_sub(tile_column * TILE_EDGE).min(TILE_EDGE);
    for row in 0..TILE_EDGE {
        for column in 0..TILE_EDGE {
            if (row >= valid_rows || column >= valid_columns) && cells[row * TILE_EDGE + column] != INTENSITY_NODATA {
                return Err(DecodeError::Intensity);
            }
        }
    }
    Ok(())
}

pub fn decode_hourly_record(bytes: &[u8; HOURLY_RECORD_LEN]) -> Result<HourlyRecord, DecodeError> {
    if bytes[18..].iter().any(|&b| b != 0) {
        return Err(DecodeError::Reserved);
    }
    Ok(HourlyRecord {
        valid_time_offset_s: rd_u32(bytes, 0)?,
        temperature_deci_c: rd_i16(bytes, 4)?,
        precipitation_tenth_mm: rd_u16(bytes, 6)?,
        precipitation_probability_pct: bytes[8],
        condition: bytes[9],
        wind_from_deg: rd_u16(bytes, 10)?,
        wind_speed_deci_ms: rd_u16(bytes, 12)?,
        wind_gust_deci_ms: rd_u16(bytes, 14)?,
        flags: rd_u16(bytes, 16)?,
    })
}

pub fn decode_frame_descriptor(bytes: &[u8; FRAME_DESCRIPTOR_LEN]) -> Result<FrameDescriptor, DecodeError> {
    if bytes[FRAME_RESERVED0] != 0 || bytes[FRAME_RESERVED..].iter().any(|&b| b != 0) {
        return Err(DecodeError::Reserved);
    }
    if bytes[FRAME_TILE_EDGE] as usize != TILE_EDGE {
        return Err(DecodeError::Frame);
    }
    Ok(FrameDescriptor {
        valid_at: rd_i64(bytes, FRAME_VALID_AT)?,
        width: rd_u16(bytes, FRAME_WIDTH)?,
        height: rd_u16(bytes, FRAME_HEIGHT)?,
        cell_size_m: rd_u16(bytes, FRAME_CELL_SIZE_M)?,
        tile_directory_offset: rd_u32(bytes, FRAME_TILE_DIRECTORY_OFFSET)?,
        tile_count: rd_u32(bytes, FRAME_TILE_COUNT)?,
        tile_data_offset: rd_u32(bytes, FRAME_TILE_DATA_OFFSET)?,
        tile_data_len: rd_u32(bytes, FRAME_TILE_DATA_LEN)?,
        quality_flags: rd_u32(bytes, FRAME_QUALITY_FLAGS)?,
    })
}

pub fn decode_tile_entry(bytes: &[u8; TILE_DIRECTORY_ENTRY_LEN]) -> Result<TileEntry, DecodeError> {
    if bytes[TILE_FLAGS] != 0 || rd_u16(bytes, TILE_RESERVED)? != 0 {
        return Err(DecodeError::Reserved);
    }
    Ok(TileEntry {
        data_offset: rd_u32(bytes, TILE_DATA_OFFSET)?,
        encoded_len: rd_u16(bytes, TILE_ENCODED_LEN)?,
        decoded_cells: rd_u16(bytes, TILE_DECODED_CELLS)?,
        codec: bytes[TILE_CODEC],
    })
}

pub fn validate_tile_payload(entry: TileEntry, encoded: &[u8]) -> Result<(), DecodeError> {
    let len = entry.encoded_len as usize;
    if len == 0 || len > RAW4_LEN || encoded.len() != len || entry.decoded_cells as usize != TILE_CELLS {
        return Err(DecodeError::TileCodec);
    }
    match entry.codec {
        TILE_CODEC_RAW4 if len == RAW4_LEN => {
            if encoded.iter().any(|b| !valid_intensity(b & 0x0F) || !valid_intensity(b >> 4)) {
                return Err(DecodeError::Intensity);
            }
        }
        TILE_CODEC_RLE4 if len < RAW4_LEN => {
            let mut count = 0usize;
            let mut previous: Option<(u8, usize)> = None;
            for &byte in encoded {
                let value = byte & 0x0F;
                let run = (byte >> 4) as usize + 1;
                if !valid_intensity(value) {
                    return Err(DecodeError::Intensity);
                }
                if previous.is_some_and(|(previous_value, previous_run)| value == previous_value && previous_run != 16)
                {
                    return Err(DecodeError::Rle);
                }
                count = count.checked_add(run).ok_or(DecodeError::Rle)?;
                if count > TILE_CELLS {
                    return Err(DecodeError::Rle);
                }
                previous = Some((value, run));
            }
            if count != TILE_CELLS {
                return Err(DecodeError::Rle);
            }
        }
        _ => return Err(DecodeError::TileCodec),
    }
    Ok(())
}

pub fn decode_tile_payload(entry: TileEntry, encoded: &[u8], out: &mut [u8; TILE_CELLS]) -> Result<(), DecodeError> {
    validate_tile_payload(entry, encoded)?;
    if entry.codec == TILE_CODEC_RAW4 {
        for (index, &byte) in encoded.iter().enumerate() {
            out[index * 2] = byte & 0x0F;
            out[index * 2 + 1] = byte >> 4;
        }
    } else {
        let mut index = 0usize;
        for &byte in encoded {
            let run = (byte >> 4) as usize + 1;
            out[index..index + run].fill(byte & 0x0F);
            index += run;
        }
    }
    Ok(())
}

fn encoded_tile_len(tile: &[u8; TILE_CELLS]) -> usize {
    rle4_len(tile).min(RAW4_LEN)
}

fn rle4_len(tile: &[u8; TILE_CELLS]) -> usize {
    let mut runs = 0usize;
    let mut index = 0usize;
    while index < TILE_CELLS {
        let value = tile[index];
        let mut run = 1usize;
        while index + run < TILE_CELLS && run < 16 && tile[index + run] == value {
            run += 1;
        }
        runs += 1;
        index += run;
    }
    runs
}

fn encode_raw4(tile: &[u8; TILE_CELLS], out: &mut [u8]) {
    for index in 0..RAW4_LEN {
        out[index] = tile[index * 2] | (tile[index * 2 + 1] << 4);
    }
}

fn encode_rle4(tile: &[u8; TILE_CELLS], out: &mut [u8]) {
    let mut input = 0usize;
    let mut output = 0usize;
    while input < TILE_CELLS {
        let value = tile[input];
        let mut run = 1usize;
        while input + run < TILE_CELLS && run < 16 && tile[input + run] == value {
            run += 1;
        }
        out[output] = ((run as u8 - 1) << 4) | value;
        input += run;
        output += 1;
    }
}

fn encode_hourly(record: &HourlyRecord, out: &mut [u8]) -> Result<(), EncodeError> {
    put_u32(out, 0, record.valid_time_offset_s)?;
    put_i16(out, 4, record.temperature_deci_c)?;
    put_u16(out, 6, record.precipitation_tenth_mm)?;
    out[8] = record.precipitation_probability_pct;
    out[9] = record.condition;
    put_u16(out, 10, record.wind_from_deg)?;
    put_u16(out, 12, record.wind_speed_deci_ms)?;
    put_u16(out, 14, record.wind_gust_deci_ms)?;
    put_u16(out, 16, record.flags)?;
    Ok(())
}

fn encode_frame(
    frame: &RainFrameInput<'_>,
    directory_offset: u32,
    data_offset: u32,
    data_len: u32,
    out: &mut [u8],
) -> Result<(), EncodeError> {
    put_i64(out, FRAME_VALID_AT, frame.valid_at)?;
    put_u16(out, FRAME_WIDTH, frame.width)?;
    put_u16(out, FRAME_HEIGHT, frame.height)?;
    put_u16(out, FRAME_CELL_SIZE_M, frame.cell_size_m)?;
    out[FRAME_TILE_EDGE] = TILE_EDGE as u8;
    put_u32(out, FRAME_TILE_DIRECTORY_OFFSET, directory_offset)?;
    put_u32(out, FRAME_TILE_COUNT, frame.tiles.len() as u32)?;
    put_u32(out, FRAME_TILE_DATA_OFFSET, data_offset)?;
    put_u32(out, FRAME_TILE_DATA_LEN, data_len)?;
    put_u32(out, FRAME_QUALITY_FLAGS, frame.quality_flags)?;
    Ok(())
}

fn encode_tile_entry(offset: u32, len: u16, codec: u8, out: &mut [u8]) -> Result<(), EncodeError> {
    put_u32(out, TILE_DATA_OFFSET, offset)?;
    put_u16(out, TILE_ENCODED_LEN, len)?;
    put_u16(out, TILE_DECODED_CELLS, TILE_CELLS as u16)?;
    out[TILE_CODEC] = codec;
    Ok(())
}

fn range(bytes: &[u8], offset: usize, len: usize) -> Result<&[u8], DecodeError> {
    bytes.get(offset..offset.checked_add(len).ok_or(DecodeError::Bounds)?).ok_or(DecodeError::Bounds)
}

fn range_mut(bytes: &mut [u8], offset: usize, len: usize) -> Result<&mut [u8], EncodeError> {
    let end = offset.checked_add(len).ok_or(EncodeError::LengthOverflow)?;
    bytes.get_mut(offset..end).ok_or(EncodeError::Internal)
}

fn rd_u16(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    Ok(u16::from_le_bytes(range(bytes, offset, 2)?.try_into().map_err(|_| DecodeError::Bounds)?))
}
fn rd_i16(bytes: &[u8], offset: usize) -> Result<i16, DecodeError> {
    Ok(i16::from_le_bytes(range(bytes, offset, 2)?.try_into().map_err(|_| DecodeError::Bounds)?))
}
fn rd_u32(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    Ok(u32::from_le_bytes(range(bytes, offset, 4)?.try_into().map_err(|_| DecodeError::Bounds)?))
}
fn rd_i32(bytes: &[u8], offset: usize) -> Result<i32, DecodeError> {
    Ok(i32::from_le_bytes(range(bytes, offset, 4)?.try_into().map_err(|_| DecodeError::Bounds)?))
}
fn rd_i64(bytes: &[u8], offset: usize) -> Result<i64, DecodeError> {
    Ok(i64::from_le_bytes(range(bytes, offset, 8)?.try_into().map_err(|_| DecodeError::Bounds)?))
}
fn put_u16(bytes: &mut [u8], offset: usize, value: u16) -> Result<(), EncodeError> {
    range_mut(bytes, offset, 2)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn put_i16(bytes: &mut [u8], offset: usize, value: i16) -> Result<(), EncodeError> {
    range_mut(bytes, offset, 2)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) -> Result<(), EncodeError> {
    range_mut(bytes, offset, 4)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn put_i32(bytes: &mut [u8], offset: usize, value: i32) -> Result<(), EncodeError> {
    range_mut(bytes, offset, 4)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}
fn put_i64(bytes: &mut [u8], offset: usize, value: i64) -> Result<(), EncodeError> {
    range_mut(bytes, offset, 8)?.copy_from_slice(&value.to_le_bytes());
    Ok(())
}

const _: () = assert!(HEADER_LEN == HDR_RESERVED + 20);
const _: () = assert!(HOURLY_RECORD_LEN == 24);
const _: () = assert!(FRAME_DESCRIPTOR_LEN == 48);
const _: () = assert!(TILE_DIRECTORY_ENTRY_LEN == 12);
const _: () = assert!(RAW4_LEN == 128);

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    fn hourly() -> [HourlyRecord; HOURLY_COUNT] {
        core::array::from_fn(|i| HourlyRecord {
            valid_time_offset_s: i as u32 * 3_600,
            temperature_deci_c: 123,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: CONDITION_CLEAR,
            wind_from_deg: 225,
            wind_speed_deci_ms: 40,
            wind_gust_deci_ms: 60,
            flags: 0,
        })
    }

    #[test]
    fn raw_and_rle_tiles_round_trip_without_frame_allocation() {
        let raw: [u8; TILE_CELLS] = core::array::from_fn(|i| (i % 13) as u8);
        let dry = [INTENSITY_DRY; TILE_CELLS];
        let tiles = [raw, dry];
        let frames = [RainFrameInput {
            valid_at: 1_800_000_000,
            width: 32,
            height: 16,
            cell_size_m: 1_000,
            quality_flags: QUALITY_FORECAST,
            tiles: &tiles,
        }];
        let hours = hourly();
        let input = BundleInput {
            generation: 7,
            request_id: 9,
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
            hourly: &hours,
            frames: &frames,
        };
        let mut bytes = vec![0u8; encoded_len(&input).unwrap() as usize];
        let len = encode_format(&input, &mut bytes).unwrap();
        let bytes = &bytes[..len];
        let header = decode_header(bytes[..HEADER_LEN].try_into().unwrap()).unwrap();
        assert_eq!(header.generation, 7);
        let frame_base = HEADER_LEN + HOURLY_COUNT * HOURLY_RECORD_LEN;
        let frame =
            decode_frame_descriptor(bytes[frame_base..frame_base + FRAME_DESCRIPTOR_LEN].try_into().unwrap()).unwrap();
        assert_eq!(frame.tile_count, 2);
        let raw_entry = decode_tile_entry(
            bytes
                [frame.tile_directory_offset as usize..frame.tile_directory_offset as usize + TILE_DIRECTORY_ENTRY_LEN]
                .try_into()
                .unwrap(),
        )
        .unwrap();
        let rle_entry_offset = frame.tile_directory_offset as usize + TILE_DIRECTORY_ENTRY_LEN;
        let rle_entry =
            decode_tile_entry(bytes[rle_entry_offset..rle_entry_offset + TILE_DIRECTORY_ENTRY_LEN].try_into().unwrap())
                .unwrap();
        assert_eq!(raw_entry.codec, TILE_CODEC_RAW4);
        assert_eq!(rle_entry.codec, TILE_CODEC_RLE4);
        let mut decoded = [0u8; TILE_CELLS];
        decode_tile_payload(
            raw_entry,
            &bytes[raw_entry.data_offset as usize..raw_entry.data_offset as usize + raw_entry.encoded_len as usize],
            &mut decoded,
        )
        .unwrap();
        assert_eq!(decoded, raw);
        decode_tile_payload(
            rle_entry,
            &bytes[rle_entry.data_offset as usize..rle_entry.data_offset as usize + rle_entry.encoded_len as usize],
            &mut decoded,
        )
        .unwrap();
        assert_eq!(decoded, dry);
    }

    #[test]
    fn hourly_intervals_and_maximal_rle_runs_are_canonical() {
        let hours = hourly();
        assert_eq!(validate_hourly_time(0, &hours[0], 1_800_000_000, 1_800_086_400), Ok(()));
        assert_eq!(validate_hourly_time(23, &hours[23], 1_800_000_000, 1_800_086_400), Ok(()));
        assert_eq!(validate_hourly_time(23, &hours[23], 1_800_000_000, 1_800_086_399), Err(DecodeError::Timestamp));
        let mut shifted = hours[0];
        shifted.valid_time_offset_s = HOURLY_INTERVAL_SECONDS;
        assert_eq!(validate_hourly_time(0, &shifted, 1_800_000_000, 1_800_086_400), Err(DecodeError::Timestamp));

        let canonical = [0xF6; 16];
        let canonical_entry = TileEntry {
            data_offset: 0,
            encoded_len: canonical.len() as u16,
            decoded_cells: TILE_CELLS as u16,
            codec: TILE_CODEC_RLE4,
        };
        assert_eq!(validate_tile_payload(canonical_entry, &canonical), Ok(()));

        let mut split = [0xF6; 17];
        split[0] = 0x76;
        split[1] = 0x76;
        let split_entry = TileEntry { encoded_len: split.len() as u16, ..canonical_entry };
        assert_eq!(validate_tile_payload(split_entry, &split), Err(DecodeError::Rle));
    }

    #[test]
    fn fixed_slice_decoders_never_panic_on_arbitrary_bytes() {
        let mut state = 0xC0DE_1187u32;
        for _ in 0..1_024usize {
            let mut header = [0u8; HEADER_LEN];
            for byte in &mut header {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *byte = state as u8;
            }
            let result = std::panic::catch_unwind(|| {
                let _ = decode_header(&header);
                let _ = decode_hourly_record((&header[..HOURLY_RECORD_LEN]).try_into().unwrap());
                let _ = decode_frame_descriptor((&header[..FRAME_DESCRIPTOR_LEN]).try_into().unwrap());
                let _ = decode_tile_entry((&header[..TILE_DIRECTORY_ENTRY_LEN]).try_into().unwrap());
            });
            assert!(result.is_ok());
        }
    }

    #[test]
    fn wire_structs_are_small_and_explicit() {
        assert!(core::mem::size_of::<Header>() <= 80);
        assert!(core::mem::size_of::<HourlyRecord>() <= 24);
        assert!(core::mem::size_of::<FrameDescriptor>() <= 48);
        assert!(core::mem::size_of::<TileEntry>() <= 12);
    }
}
