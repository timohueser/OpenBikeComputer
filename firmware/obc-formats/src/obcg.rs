//! OBCG v1 published precipitation grid object: byte authority for `specs/OBCG_Spec.md`.
//!
//! One OBCG object is exactly one grid frame — one product, one real UTC valid time, one regular
//! latitude/longitude window. The multi-frame table lives in the service manifest, never inside
//! an object, so heterogeneous per-frame geometry composes with no resampling by construction.
//!
//! The layout is built for HTTP Range consumers: a fixed self-CRC'd header, then a paged tile
//! directory whose pages verify independently, then tightly packed canonical raw4/RLE4 tile
//! payloads (the WX2 codec from [`crate::precip4`], generalized over the per-product tile size).
//! Corridor extraction is: read the header, compute covering directory pages arithmetically,
//! read those pages, read the needed tiles — every piece independently CRC-verified. The
//! whole-object CRC stays for full-object consumers such as the baker's self-check.

use crate::precip4::{self, INTENSITY_DRY, INTENSITY_NODATA};
use obc_crc::Crc32;

pub const MAGIC: [u8; 4] = *b"OBCG";
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 128;
pub const DIRECTORY_ENTRY_LEN: usize = 12;
pub const PAGE_CRC_LEN: usize = 4;
/// Every directory page must be readable in one small Range request.
pub const MAX_PAGE_BYTES: usize = 16 * 1024;
/// `(MAX_PAGE_BYTES - PAGE_CRC_LEN) / DIRECTORY_ENTRY_LEN`.
pub const MAX_ENTRIES_PER_PAGE: u16 = 1_365;
pub const MIN_TILE_EDGE: u16 = 16;
pub const MAX_TILE_EDGE: u16 = 256;
/// Grid dimension ceiling; keeps every derived count and offset comfortably inside `u32`.
pub const MAX_GRID_DIM: u32 = 100_000;
/// Frame cell-count ceiling, matching the WX1 decode bound the baker inherits.
pub const MAX_GRID_CELLS: u64 = 30_000_000;

pub const HDR_MAGIC: usize = 0;
pub const HDR_VERSION: usize = 4;
pub const HDR_HEADER_LEN: usize = 6;
pub const HDR_TOTAL_LEN: usize = 8;
pub const HDR_PRODUCT_ID: usize = 12;
pub const HDR_TIER: usize = 13;
pub const HDR_FLAGS: usize = 14;
pub const HDR_VALID_AT: usize = 16;
pub const HDR_REFERENCE_TIME: usize = 24;
pub const HDR_SOUTH_LAT: usize = 32;
pub const HDR_WEST_LON: usize = 36;
pub const HDR_CELL_LAT: usize = 40;
pub const HDR_CELL_LON: usize = 44;
pub const HDR_WIDTH: usize = 48;
pub const HDR_HEIGHT: usize = 52;
pub const HDR_CELL_SIZE_M: usize = 56;
pub const HDR_TILE_EDGE: usize = 58;
pub const HDR_ENTRIES_PER_PAGE: usize = 60;
pub const HDR_RESERVED0: usize = 62;
pub const HDR_DIRECTORY_OFFSET: usize = 64;
pub const HDR_DATA_OFFSET: usize = 68;
pub const HDR_DATA_LEN: usize = 72;
pub const HDR_OBJECT_CRC32: usize = 76;
pub const HDR_HEADER_CRC32: usize = 80;
pub const HDR_RESERVED: usize = 84;

pub const ENTRY_DATA_OFFSET: usize = 0;
pub const ENTRY_ENCODED_LEN: usize = 4;
pub const ENTRY_CODEC: usize = 6;
pub const ENTRY_RESERVED: usize = 7;
pub const ENTRY_CRC32: usize = 8;

/// Exactly one of [`FLAG_OBSERVED`] and [`FLAG_FORECAST`] must be set.
pub const FLAG_OBSERVED: u16 = 1 << 0;
pub const FLAG_FORECAST: u16 = 1 << 1;
pub const FLAG_KNOWN_MASK: u16 = FLAG_OBSERVED | FLAG_FORECAST;

/// Product registry. Appending an id is a spec-table addition, not a version bump; a consumer
/// MUST NOT reject an unknown nonzero id — selection policy is manifest data, and this field is
/// provenance.
pub const PRODUCT_DWD_RV: u8 = 1;
pub const PRODUCT_ICON_EU: u8 = 2;
pub const PRODUCT_MRMS: u8 = 3;
pub const PRODUCT_HRRR: u8 = 4;
pub const PRODUCT_GFS: u8 = 5;
pub const PRODUCT_EXPERIMENTAL: u8 = 255;

pub const TIER_RADAR: u8 = 1;
pub const TIER_MODEL: u8 = 2;
pub const TIER_FLOOR: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    Bounds,
    Magic,
    Version,
    HeaderLength,
    TotalLength,
    HeaderCrc,
    ObjectCrc,
    PageCrc,
    TileCrc,
    Reserved,
    Flags,
    Product,
    Timestamp,
    Geography,
    Paging,
    SectionLayout,
    Directory,
    TileCodec,
    Padding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    InvalidInput,
    LengthOverflow,
    OutputTooSmall,
    Internal,
}

/// Decoded fixed header. Every derived count below is checked arithmetic over these fields, so a
/// corridor consumer can compute directory-page and tile byte ranges from the header alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub total_len: u32,
    pub product_id: u8,
    pub tier: u8,
    pub flags: u16,
    /// Real upstream UTC frame validity timestamp; never an ordinal or a re-stamped fetch time.
    pub valid_at: i64,
    /// Upstream run/reference UTC timestamp the frame was derived from.
    pub reference_time: i64,
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub cell_size_m: u16,
    pub tile_edge: u16,
    pub entries_per_page: u16,
    pub data_offset: u32,
    pub data_len: u32,
    pub object_crc32: u32,
    pub header_crc32: u32,
}

impl Header {
    /// North edge in microdegrees (checked when the header was validated).
    pub fn north_lat_udeg(&self) -> i64 {
        i64::from(self.south_lat_udeg) + i64::from(self.height) * i64::from(self.cell_lat_udeg)
    }

    /// East edge in microdegrees (checked when the header was validated).
    pub fn east_lon_udeg(&self) -> i64 {
        i64::from(self.west_lon_udeg) + i64::from(self.width) * i64::from(self.cell_lon_udeg)
    }

    pub fn tile_cols(&self) -> u32 {
        self.width.div_ceil(u32::from(self.tile_edge))
    }

    pub fn tile_rows(&self) -> u32 {
        self.height.div_ceil(u32::from(self.tile_edge))
    }

    pub fn tile_count(&self) -> u32 {
        self.tile_cols() * self.tile_rows()
    }

    /// Row-major directory index of tile `(tile_col, tile_row)`; row 0 is the southernmost row.
    pub fn tile_index(&self, tile_col: u32, tile_row: u32) -> Option<u32> {
        if tile_col >= self.tile_cols() || tile_row >= self.tile_rows() {
            return None;
        }
        Some(tile_row * self.tile_cols() + tile_col)
    }

    /// Fixed byte length of one directory page including its trailing CRC-32.
    pub fn page_bytes(&self) -> u32 {
        u32::from(self.entries_per_page) * DIRECTORY_ENTRY_LEN as u32 + PAGE_CRC_LEN as u32
    }

    pub fn page_count(&self) -> u32 {
        self.tile_count().div_ceil(u32::from(self.entries_per_page))
    }

    pub fn page_of_entry(&self, tile_index: u32) -> u32 {
        tile_index / u32::from(self.entries_per_page)
    }

    /// Absolute byte offset of directory page `page`.
    pub fn page_offset(&self, page: u32) -> Option<u32> {
        if page >= self.page_count() {
            return None;
        }
        Some(HEADER_LEN as u32 + page * self.page_bytes())
    }

    /// Absolute byte offset of `tile_index`'s 12-byte directory entry.
    pub fn entry_offset(&self, tile_index: u32) -> Option<u32> {
        if tile_index >= self.tile_count() {
            return None;
        }
        let page = self.page_of_entry(tile_index);
        let within = tile_index - page * u32::from(self.entries_per_page);
        Some(self.page_offset(page)? + within * DIRECTORY_ENTRY_LEN as u32)
    }

    /// The tile grid coordinates covering the in-bounds cell `(col, row)`; row 0 is south.
    pub fn tile_of_cell(&self, col: u32, row: u32) -> Option<(u32, u32)> {
        if col >= self.width || row >= self.height {
            return None;
        }
        Some((col / u32::from(self.tile_edge), row / u32::from(self.tile_edge)))
    }

    /// Row-major index of cell `(col, row)` inside its (nodata-padded) tile.
    pub fn cell_index_in_tile(&self, col: u32, row: u32) -> Option<usize> {
        self.tile_of_cell(col, row)?;
        let edge = u32::from(self.tile_edge);
        Some(((row % edge) * edge + (col % edge)) as usize)
    }

    /// Decoded cell count of every tile (edge tiles are nodata-padded to the full square).
    pub fn tile_cells(&self) -> usize {
        usize::from(self.tile_edge) * usize::from(self.tile_edge)
    }
}

/// One decoded directory entry. `encoded_len == 0` is the all-dry sentinel: no payload bytes
/// exist and every other field must be zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEntry {
    pub data_offset: u32,
    pub encoded_len: u16,
    pub codec: u8,
    pub crc32: u32,
}

impl TileEntry {
    pub fn is_dry(&self) -> bool {
        self.encoded_len == 0
    }
}

/// Producer input: one frame as a full south-up row-major cell grid of canonical intensity
/// codes. The encoder tiles, pads, chooses canonical codecs, and emits the sentinel for all-dry
/// tiles; handing it raw cells keeps every canonicality decision inside the byte authority.
#[derive(Debug)]
pub struct FrameInput<'a> {
    pub product_id: u8,
    pub tier: u8,
    pub flags: u16,
    pub valid_at: i64,
    pub reference_time: i64,
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub cell_lat_udeg: u32,
    pub cell_lon_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub cell_size_m: u16,
    pub tile_edge: u16,
    pub entries_per_page: u16,
    /// `width * height` intensity codes, row-major, row 0 = south edge.
    pub cells: &'a [u8],
}

fn rd_u16(bytes: &[u8], offset: usize) -> Result<u16, DecodeError> {
    let slice = bytes.get(offset..offset + 2).ok_or(DecodeError::Bounds)?;
    Ok(u16::from_le_bytes(slice.try_into().map_err(|_| DecodeError::Bounds)?))
}

fn rd_u32(bytes: &[u8], offset: usize) -> Result<u32, DecodeError> {
    let slice = bytes.get(offset..offset + 4).ok_or(DecodeError::Bounds)?;
    Ok(u32::from_le_bytes(slice.try_into().map_err(|_| DecodeError::Bounds)?))
}

fn rd_i32(bytes: &[u8], offset: usize) -> Result<i32, DecodeError> {
    Ok(rd_u32(bytes, offset)? as i32)
}

fn rd_i64(bytes: &[u8], offset: usize) -> Result<i64, DecodeError> {
    let slice = bytes.get(offset..offset + 8).ok_or(DecodeError::Bounds)?;
    Ok(i64::from_le_bytes(slice.try_into().map_err(|_| DecodeError::Bounds)?))
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_i32(bytes: &mut [u8], offset: usize, value: i32) {
    put_u32(bytes, offset, value as u32);
}

fn put_i64(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// CRC-32/IEEE over the fixed header with the header-CRC field treated as zero. The object-CRC
/// field participates as stored, so a header-only reader also proves that field's integrity.
pub fn header_crc(header_bytes: &[u8; HEADER_LEN]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(&header_bytes[..HDR_HEADER_CRC32]);
    crc.update(&[0u8; 4]);
    crc.update(&header_bytes[HDR_HEADER_CRC32 + 4..]);
    crc.finalize()
}

/// CRC-32/IEEE over the whole object with both CRC fields treated as zero.
pub fn object_crc(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(&bytes[..HDR_OBJECT_CRC32]);
    crc.update(&[0u8; 8]);
    crc.update(&bytes[HDR_HEADER_CRC32 + 4..]);
    crc.finalize()
}

/// Decode and validate the fixed header, including its own CRC. Object length, whole-object CRC
/// and pointed-to sections are separate reader concerns; every invariant computable from the 128
/// header bytes alone is enforced here.
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
    if rd_u32(bytes, HDR_HEADER_CRC32)? != header_crc(bytes) {
        return Err(DecodeError::HeaderCrc);
    }
    if rd_u16(bytes, HDR_RESERVED0)? != 0 || bytes[HDR_RESERVED..].iter().any(|&byte| byte != 0) {
        return Err(DecodeError::Reserved);
    }
    let header = Header {
        total_len: rd_u32(bytes, HDR_TOTAL_LEN)?,
        product_id: bytes[HDR_PRODUCT_ID],
        tier: bytes[HDR_TIER],
        flags: rd_u16(bytes, HDR_FLAGS)?,
        valid_at: rd_i64(bytes, HDR_VALID_AT)?,
        reference_time: rd_i64(bytes, HDR_REFERENCE_TIME)?,
        south_lat_udeg: rd_i32(bytes, HDR_SOUTH_LAT)?,
        west_lon_udeg: rd_i32(bytes, HDR_WEST_LON)?,
        cell_lat_udeg: rd_u32(bytes, HDR_CELL_LAT)?,
        cell_lon_udeg: rd_u32(bytes, HDR_CELL_LON)?,
        width: rd_u32(bytes, HDR_WIDTH)?,
        height: rd_u32(bytes, HDR_HEIGHT)?,
        cell_size_m: rd_u16(bytes, HDR_CELL_SIZE_M)?,
        tile_edge: rd_u16(bytes, HDR_TILE_EDGE)?,
        entries_per_page: rd_u16(bytes, HDR_ENTRIES_PER_PAGE)?,
        data_offset: rd_u32(bytes, HDR_DATA_OFFSET)?,
        data_len: rd_u32(bytes, HDR_DATA_LEN)?,
        object_crc32: rd_u32(bytes, HDR_OBJECT_CRC32)?,
        header_crc32: rd_u32(bytes, HDR_HEADER_CRC32)?,
    };
    validate_header_semantics(&header)?;
    if rd_u32(bytes, HDR_DIRECTORY_OFFSET)? != HEADER_LEN as u32 {
        return Err(DecodeError::SectionLayout);
    }
    Ok(header)
}

fn validate_header_semantics(header: &Header) -> Result<(), DecodeError> {
    if header.product_id == 0 || header.tier == 0 {
        return Err(DecodeError::Product);
    }
    if header.flags & !FLAG_KNOWN_MASK != 0
        || (header.flags & FLAG_OBSERVED != 0) == (header.flags & FLAG_FORECAST != 0)
    {
        return Err(DecodeError::Flags);
    }
    if header.reference_time <= 0 || header.valid_at < header.reference_time {
        return Err(DecodeError::Timestamp);
    }
    if header.width == 0
        || header.height == 0
        || header.width > MAX_GRID_DIM
        || header.height > MAX_GRID_DIM
        || u64::from(header.width) * u64::from(header.height) > MAX_GRID_CELLS
        || header.cell_lat_udeg == 0
        || header.cell_lon_udeg == 0
        || header.cell_size_m == 0
    {
        return Err(DecodeError::Geography);
    }
    let south = i64::from(header.south_lat_udeg);
    let west = i64::from(header.west_lon_udeg);
    let north = header.north_lat_udeg();
    let east = header.east_lon_udeg();
    if south < -90_000_000 || north > 90_000_000 || west < -180_000_000 || east > 180_000_000 {
        return Err(DecodeError::Geography);
    }
    if !header.tile_edge.is_power_of_two() || !(MIN_TILE_EDGE..=MAX_TILE_EDGE).contains(&header.tile_edge) {
        return Err(DecodeError::Paging);
    }
    if header.entries_per_page == 0 || header.entries_per_page > MAX_ENTRIES_PER_PAGE {
        return Err(DecodeError::Paging);
    }
    let directory_len = u64::from(header.page_count()) * u64::from(header.page_bytes());
    let expected_data_offset = HEADER_LEN as u64 + directory_len;
    if u64::from(header.data_offset) != expected_data_offset {
        return Err(DecodeError::SectionLayout);
    }
    let expected_total = expected_data_offset + u64::from(header.data_len);
    if u64::from(header.total_len) != expected_total || expected_total > u64::from(u32::MAX) {
        return Err(DecodeError::TotalLength);
    }
    Ok(())
}

/// Decode one directory entry from a page's entry area (offsets relative to the page start).
pub fn decode_entry(page: &[u8], index_in_page: usize) -> Result<TileEntry, DecodeError> {
    let base = index_in_page.checked_mul(DIRECTORY_ENTRY_LEN).ok_or(DecodeError::Bounds)?;
    let bytes = page.get(base..base + DIRECTORY_ENTRY_LEN).ok_or(DecodeError::Bounds)?;
    if bytes[ENTRY_RESERVED] != 0 {
        return Err(DecodeError::Reserved);
    }
    let entry = TileEntry {
        data_offset: rd_u32(bytes, ENTRY_DATA_OFFSET)?,
        encoded_len: rd_u16(bytes, ENTRY_ENCODED_LEN)?,
        codec: bytes[ENTRY_CODEC],
        crc32: rd_u32(bytes, ENTRY_CRC32)?,
    };
    if entry.is_dry() && (entry.data_offset != 0 || entry.codec != 0 || entry.crc32 != 0) {
        return Err(DecodeError::Directory);
    }
    Ok(entry)
}

/// Verify one directory page's trailing CRC-32. `page` is the full fixed-size page.
pub fn validate_page(header: &Header, page: &[u8]) -> Result<(), DecodeError> {
    let page_bytes = header.page_bytes() as usize;
    if page.len() != page_bytes {
        return Err(DecodeError::Bounds);
    }
    let entry_area = &page[..page_bytes - PAGE_CRC_LEN];
    let stored = u32::from_le_bytes(page[page_bytes - PAGE_CRC_LEN..].try_into().map_err(|_| DecodeError::Bounds)?);
    if Crc32::checksum(entry_area) != stored {
        return Err(DecodeError::PageCrc);
    }
    Ok(())
}

/// Verify one non-dry tile payload against its directory entry: CRC first, then the canonical
/// codec including the per-product decoded cell count.
pub fn validate_tile_payload(header: &Header, entry: &TileEntry, payload: &[u8]) -> Result<(), DecodeError> {
    if entry.is_dry() {
        return Err(DecodeError::Directory);
    }
    if payload.len() != usize::from(entry.encoded_len) {
        return Err(DecodeError::Bounds);
    }
    if Crc32::checksum(payload) != entry.crc32 {
        return Err(DecodeError::TileCrc);
    }
    precip4::validate_cells(entry.codec, payload, header.tile_cells()).map_err(|_| DecodeError::TileCodec)
}

/// Decode one verified tile into `out` (`header.tile_cells()` bytes). A dry entry fills the tile
/// with [`INTENSITY_DRY`] without touching payload bytes.
pub fn decode_tile_cells(
    header: &Header,
    entry: &TileEntry,
    payload: &[u8],
    out: &mut [u8],
) -> Result<(), DecodeError> {
    if out.len() != header.tile_cells() {
        return Err(DecodeError::Bounds);
    }
    if entry.is_dry() {
        out.fill(INTENSITY_DRY);
        return Ok(());
    }
    validate_tile_payload(header, entry, payload)?;
    precip4::decode_cells(entry.codec, payload, out).map_err(|_| DecodeError::TileCodec)
}

fn frame_header(input: &FrameInput<'_>) -> Result<Header, EncodeError> {
    let header = Header {
        total_len: 0,
        product_id: input.product_id,
        tier: input.tier,
        flags: input.flags,
        valid_at: input.valid_at,
        reference_time: input.reference_time,
        south_lat_udeg: input.south_lat_udeg,
        west_lon_udeg: input.west_lon_udeg,
        cell_lat_udeg: input.cell_lat_udeg,
        cell_lon_udeg: input.cell_lon_udeg,
        width: input.width,
        height: input.height,
        cell_size_m: input.cell_size_m,
        tile_edge: input.tile_edge,
        entries_per_page: input.entries_per_page,
        data_offset: 0,
        data_len: 0,
        object_crc32: 0,
        header_crc32: 0,
    };
    // Semantic validation minus the layout/total fields this function has not derived yet.
    if header.product_id == 0
        || header.tier == 0
        || header.flags & !FLAG_KNOWN_MASK != 0
        || (header.flags & FLAG_OBSERVED != 0) == (header.flags & FLAG_FORECAST != 0)
        || header.reference_time <= 0
        || header.valid_at < header.reference_time
    {
        return Err(EncodeError::InvalidInput);
    }
    if header.width == 0
        || header.height == 0
        || header.width > MAX_GRID_DIM
        || header.height > MAX_GRID_DIM
        || u64::from(header.width) * u64::from(header.height) > MAX_GRID_CELLS
        || header.cell_lat_udeg == 0
        || header.cell_lon_udeg == 0
        || header.cell_size_m == 0
        || i64::from(header.south_lat_udeg) < -90_000_000
        || header.north_lat_udeg() > 90_000_000
        || i64::from(header.west_lon_udeg) < -180_000_000
        || header.east_lon_udeg() > 180_000_000
        || !header.tile_edge.is_power_of_two()
        || !(MIN_TILE_EDGE..=MAX_TILE_EDGE).contains(&header.tile_edge)
        || header.entries_per_page == 0
        || header.entries_per_page > MAX_ENTRIES_PER_PAGE
    {
        return Err(EncodeError::InvalidInput);
    }
    if input.cells.len() as u64 != u64::from(header.width) * u64::from(header.height) {
        return Err(EncodeError::InvalidInput);
    }
    Ok(header)
}

/// Gather one padded tile from the frame grid. Cells outside the declared width/height are
/// [`INTENSITY_NODATA`]; a consumer clips them.
fn gather_tile(input: &FrameInput<'_>, tile_col: u32, tile_row: u32, out: &mut [u8]) {
    let edge = u32::from(input.tile_edge);
    for local_row in 0..edge {
        let row = tile_row * edge + local_row;
        for local_col in 0..edge {
            let col = tile_col * edge + local_col;
            out[(local_row * edge + local_col) as usize] = if row < input.height && col < input.width {
                input.cells[(row as usize) * (input.width as usize) + col as usize]
            } else {
                INTENSITY_NODATA
            };
        }
    }
}

fn tile_encoded_len(scratch: &[u8]) -> Result<usize, EncodeError> {
    if scratch.iter().all(|&cell| cell == INTENSITY_DRY) {
        return Ok(0);
    }
    precip4::encoded_cells_len(scratch).map_err(|_| EncodeError::InvalidInput)
}

/// Total encoded object length for `input`, using a caller-provided `tile_cells()`-sized scratch
/// buffer. Two-pass with [`encode_format`]; both passes are deterministic.
pub fn encoded_len(input: &FrameInput<'_>, scratch: &mut [u8]) -> Result<u32, EncodeError> {
    let header = frame_header(input)?;
    if scratch.len() != header.tile_cells() {
        return Err(EncodeError::InvalidInput);
    }
    let directory_len = u64::from(header.page_count())
        .checked_mul(u64::from(header.page_bytes()))
        .ok_or(EncodeError::LengthOverflow)?;
    let mut total = HEADER_LEN as u64 + directory_len;
    for tile_row in 0..header.tile_rows() {
        for tile_col in 0..header.tile_cols() {
            gather_tile(input, tile_col, tile_row, scratch);
            total += tile_encoded_len(scratch)? as u64;
        }
    }
    u32::try_from(total).map_err(|_| EncodeError::LengthOverflow)
}

/// Encode one complete OBCG object into `out`, returning the byte length. `out` must be at least
/// [`encoded_len`] bytes; `scratch` is one `tile_cells()`-sized buffer.
pub fn encode_format(input: &FrameInput<'_>, scratch: &mut [u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    let total = encoded_len(input, scratch)? as usize;
    let header = frame_header(input)?;
    let bytes = out.get_mut(..total).ok_or(EncodeError::OutputTooSmall)?;
    bytes.fill(0);

    let page_bytes = header.page_bytes() as usize;
    let page_count = header.page_count() as usize;
    let directory_len = page_count * page_bytes;
    let data_offset = HEADER_LEN + directory_len;
    let data_len = total - data_offset;

    bytes[HDR_MAGIC..HDR_MAGIC + 4].copy_from_slice(&MAGIC);
    put_u16(bytes, HDR_VERSION, VERSION);
    put_u16(bytes, HDR_HEADER_LEN, HEADER_LEN as u16);
    put_u32(bytes, HDR_TOTAL_LEN, total as u32);
    bytes[HDR_PRODUCT_ID] = input.product_id;
    bytes[HDR_TIER] = input.tier;
    put_u16(bytes, HDR_FLAGS, input.flags);
    put_i64(bytes, HDR_VALID_AT, input.valid_at);
    put_i64(bytes, HDR_REFERENCE_TIME, input.reference_time);
    put_i32(bytes, HDR_SOUTH_LAT, input.south_lat_udeg);
    put_i32(bytes, HDR_WEST_LON, input.west_lon_udeg);
    put_u32(bytes, HDR_CELL_LAT, input.cell_lat_udeg);
    put_u32(bytes, HDR_CELL_LON, input.cell_lon_udeg);
    put_u32(bytes, HDR_WIDTH, input.width);
    put_u32(bytes, HDR_HEIGHT, input.height);
    put_u16(bytes, HDR_CELL_SIZE_M, input.cell_size_m);
    put_u16(bytes, HDR_TILE_EDGE, input.tile_edge);
    put_u16(bytes, HDR_ENTRIES_PER_PAGE, input.entries_per_page);
    put_u32(bytes, HDR_DIRECTORY_OFFSET, HEADER_LEN as u32);
    put_u32(bytes, HDR_DATA_OFFSET, data_offset as u32);
    put_u32(bytes, HDR_DATA_LEN, data_len as u32);

    let mut payload = data_offset;
    for tile_row in 0..header.tile_rows() {
        for tile_col in 0..header.tile_cols() {
            gather_tile(input, tile_col, tile_row, scratch);
            let tile_index = (tile_row * header.tile_cols() + tile_col) as usize;
            let entry_offset = HEADER_LEN
                + (tile_index / usize::from(input.entries_per_page)) * page_bytes
                + (tile_index % usize::from(input.entries_per_page)) * DIRECTORY_ENTRY_LEN;
            let encoded = tile_encoded_len(scratch)?;
            if encoded == 0 {
                // All-dry sentinel: the entry stays all zero.
                continue;
            }
            let encoding = precip4::encode_cells(scratch, &mut bytes[payload..payload + encoded])
                .map_err(|_| EncodeError::Internal)?;
            if encoding.encoded_len as usize != encoded {
                return Err(EncodeError::Internal);
            }
            let crc = Crc32::checksum(&bytes[payload..payload + encoded]);
            put_u32(bytes, entry_offset + ENTRY_DATA_OFFSET, payload as u32);
            put_u16(bytes, entry_offset + ENTRY_ENCODED_LEN, encoding.encoded_len);
            bytes[entry_offset + ENTRY_CODEC] = encoding.codec;
            put_u32(bytes, entry_offset + ENTRY_CRC32, crc);
            payload += encoded;
        }
    }
    debug_assert_eq!(payload, total);

    for page in 0..page_count {
        let start = HEADER_LEN + page * page_bytes;
        let crc = Crc32::checksum(&bytes[start..start + page_bytes - PAGE_CRC_LEN]);
        put_u32(bytes, start + page_bytes - PAGE_CRC_LEN, crc);
    }

    let whole = object_crc(bytes);
    put_u32(bytes, HDR_OBJECT_CRC32, whole);
    let header_bytes: &[u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().map_err(|_| EncodeError::Internal)?;
    let hcrc = header_crc(header_bytes);
    put_u32(bytes, HDR_HEADER_CRC32, hcrc);
    Ok(total)
}

/// Full-object structural validation: the whole-object consumer's acceptance check.
///
/// Order: header (with its own CRC), whole-object length + CRC, every directory page CRC, every
/// entry's canonical packing (tight row-major payloads, dry sentinels all-zero, last-page padding
/// all-zero), every tile payload CRC + canonical codec, and nodata padding + the all-dry-sentinel
/// canonicality rule on every decoded tile.
///
/// `scratch` is one caller-owned decode buffer of at least `tile_cells()` bytes (at most
/// [`precip4::MAX_CELLS`]); this crate never places a 64 KiB buffer on a device stack.
pub fn validate(bytes: &[u8], scratch: &mut [u8]) -> Result<Header, DecodeError> {
    let header_bytes: &[u8; HEADER_LEN] = bytes.get(..HEADER_LEN).ok_or(DecodeError::Bounds)?.try_into().unwrap();
    let header = decode_header(header_bytes)?;
    if header.total_len as usize != bytes.len() {
        return Err(DecodeError::TotalLength);
    }
    if object_crc(bytes) != header.object_crc32 {
        return Err(DecodeError::ObjectCrc);
    }

    let page_bytes = header.page_bytes() as usize;
    let entries_per_page = usize::from(header.entries_per_page);
    let tile_count = header.tile_count() as usize;
    let mut cursor = header.data_offset;
    let tile_cells = header.tile_cells();
    if scratch.len() < tile_cells {
        return Err(DecodeError::Bounds);
    }
    for page in 0..header.page_count() as usize {
        let start = HEADER_LEN + page * page_bytes;
        let page_slice = bytes.get(start..start + page_bytes).ok_or(DecodeError::Bounds)?;
        validate_page(&header, page_slice)?;
        for index_in_page in 0..entries_per_page {
            let tile_index = page * entries_per_page + index_in_page;
            if tile_index >= tile_count {
                // Padding entries beyond the tile count must be all zero.
                let base = index_in_page * DIRECTORY_ENTRY_LEN;
                if page_slice[base..base + DIRECTORY_ENTRY_LEN].iter().any(|&byte| byte != 0) {
                    return Err(DecodeError::Padding);
                }
                continue;
            }
            let entry = decode_entry(page_slice, index_in_page)?;
            if entry.is_dry() {
                continue;
            }
            if entry.data_offset != cursor {
                return Err(DecodeError::Directory);
            }
            let start = entry.data_offset as usize;
            let end = start.checked_add(usize::from(entry.encoded_len)).ok_or(DecodeError::Directory)?;
            let payload = bytes.get(start..end).ok_or(DecodeError::Directory)?;
            let out = &mut scratch[..tile_cells];
            decode_tile_cells(&header, &entry, payload, out)?;
            if out.iter().all(|&cell| cell == INTENSITY_DRY) {
                // An all-dry tile must use the len-0 sentinel; an encoded copy is noncanonical.
                return Err(DecodeError::Directory);
            }
            validate_tile_padding(&header, tile_index as u32, out)?;
            cursor = end as u32;
        }
    }
    if u64::from(cursor) != u64::from(header.data_offset) + u64::from(header.data_len) {
        return Err(DecodeError::Directory);
    }
    Ok(header)
}

/// Cells outside the declared grid in an edge tile must be the no-data intensity.
fn validate_tile_padding(header: &Header, tile_index: u32, cells: &[u8]) -> Result<(), DecodeError> {
    let edge = u32::from(header.tile_edge);
    let tile_col = tile_index % header.tile_cols();
    let tile_row = tile_index / header.tile_cols();
    let full_cols = header.width >= (tile_col + 1) * edge;
    let full_rows = header.height >= (tile_row + 1) * edge;
    if full_cols && full_rows {
        return Ok(());
    }
    for local_row in 0..edge {
        for local_col in 0..edge {
            let row = tile_row * edge + local_row;
            let col = tile_col * edge + local_col;
            if (row >= header.height || col >= header.width)
                && cells[(local_row * edge + local_col) as usize] != INTENSITY_NODATA
            {
                return Err(DecodeError::Padding);
            }
        }
    }
    Ok(())
}

const _: () = assert!(HEADER_LEN == HDR_RESERVED + 44);
const _: () = assert!(DIRECTORY_ENTRY_LEN == ENTRY_CRC32 + 4);
const _: () = assert!(
    MAX_ENTRIES_PER_PAGE as usize * DIRECTORY_ENTRY_LEN + PAGE_CRC_LEN <= MAX_PAGE_BYTES,
    "every directory page fits one 16 KiB range request"
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    fn frame(width: u32, height: u32, tile_edge: u16, entries_per_page: u16, cells: &[u8]) -> Vec<u8> {
        let input = FrameInput {
            product_id: PRODUCT_DWD_RV,
            tier: TIER_RADAR,
            flags: FLAG_OBSERVED,
            valid_at: 1_800_000_000,
            reference_time: 1_800_000_000,
            south_lat_udeg: 45_000_000,
            west_lon_udeg: 5_000_000,
            cell_lat_udeg: 9_000,
            cell_lon_udeg: 14_000,
            width,
            height,
            cell_size_m: 1_000,
            tile_edge,
            entries_per_page,
            cells,
        };
        let mut scratch = vec![0u8; usize::from(tile_edge) * usize::from(tile_edge)];
        let len = encoded_len(&input, &mut scratch).unwrap() as usize;
        let mut bytes = vec![0u8; len];
        let written = encode_format(&input, &mut scratch, &mut bytes).unwrap();
        assert_eq!(written, len);
        bytes
    }

    fn validated(bytes: &[u8]) -> Result<Header, DecodeError> {
        let mut scratch = vec![0u8; precip4::MAX_CELLS];
        validate(bytes, &mut scratch)
    }

    #[test]
    fn round_trip_with_paging_and_dry_sentinels() {
        // 40 x 40 cells at edge 16 -> 3 x 3 tiles; two entries per page -> 5 pages with padding.
        let mut cells = vec![0u8; 40 * 40];
        cells[0] = 6; // south-west tile wet
        cells[39 * 40 + 39] = 9; // north-east corner wet (edge tile with padding)
        let bytes = frame(40, 40, 16, 2, &cells);
        let header = validated(&bytes).unwrap();
        assert_eq!(header.tile_count(), 9);
        assert_eq!(header.page_count(), 5);
        assert_eq!(header.page_bytes(), 28);
        assert_eq!(header.data_offset, HEADER_LEN as u32 + 5 * 28);

        // Sample the two wet cells and one dry cell through the tile path.
        for (col, row, expected) in [(0u32, 0u32, 6u8), (39, 39, 9), (20, 20, INTENSITY_DRY)] {
            let (tile_col, tile_row) = header.tile_of_cell(col, row).unwrap();
            let tile_index = header.tile_index(tile_col, tile_row).unwrap();
            let page = header.page_of_entry(tile_index);
            let page_offset = header.page_offset(page).unwrap() as usize;
            let page_slice = &bytes[page_offset..page_offset + header.page_bytes() as usize];
            validate_page(&header, page_slice).unwrap();
            let entry =
                decode_entry(page_slice, (tile_index - page * u32::from(header.entries_per_page)) as usize).unwrap();
            let mut out = vec![0u8; header.tile_cells()];
            let payload = if entry.is_dry() {
                &[][..]
            } else {
                &bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)]
            };
            decode_tile_cells(&header, &entry, payload, &mut out).unwrap();
            assert_eq!(out[header.cell_index_in_tile(col, row).unwrap()], expected);
        }
    }

    #[test]
    fn corrupt_objects_fail_closed() {
        let mut cells = vec![0u8; 40 * 40];
        cells[0] = 6;
        let good = frame(40, 40, 16, 2, &cells);
        validated(&good).unwrap();

        // Truncation.
        assert_eq!(validated(&good[..good.len() - 1]), Err(DecodeError::TotalLength));
        // Whole-object CRC.
        let mut corrupt = good.clone();
        *corrupt.last_mut().unwrap() ^= 1;
        assert_eq!(validated(&corrupt), Err(DecodeError::ObjectCrc));
        // Header CRC.
        let mut corrupt = good.clone();
        corrupt[HDR_TIER] = TIER_MODEL;
        assert_eq!(validated(&corrupt), Err(DecodeError::HeaderCrc));
        // Page CRC.
        let mut corrupt = good.clone();
        corrupt[HEADER_LEN] ^= 1; // first entry byte
        let object = object_crc(&corrupt);
        put_u32(&mut corrupt, HDR_OBJECT_CRC32, object);
        let header_bytes: &[u8; HEADER_LEN] = corrupt[..HEADER_LEN].try_into().unwrap();
        let hcrc = header_crc(header_bytes);
        put_u32(&mut corrupt, HDR_HEADER_CRC32, hcrc);
        assert_eq!(validated(&corrupt), Err(DecodeError::PageCrc));
    }

    #[test]
    fn all_dry_grid_has_no_payload_bytes() {
        let cells = vec![0u8; 32 * 32];
        let bytes = frame(32, 32, 32, 8, &cells);
        let header = validated(&bytes).unwrap();
        assert_eq!(header.data_len, 0);
        assert_eq!(bytes.len(), HEADER_LEN + header.page_bytes() as usize);
    }

    /// The obcw fuzz posture, applied here: arbitrary bytes and structured single-bit mutations
    /// of a valid object must decode to an error or a valid header — never a panic, never an
    /// out-of-bounds access.
    #[test]
    fn validator_never_panics_on_arbitrary_or_mutated_bytes() {
        let mut scratch = vec![0u8; precip4::MAX_CELLS];
        let mut state = 0x0BC5_1190u32;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        // Arbitrary garbage at assorted lengths.
        for length in [0usize, 1, 16, HEADER_LEN - 1, HEADER_LEN, HEADER_LEN + 3, 512, 4_096] {
            let bytes: Vec<u8> = (0..length).map(|_| (next() & 0xFF) as u8).collect();
            let _ = validate(&bytes, &mut scratch);
        }
        // Structured mutations: every single-bit flip of a small valid object, plus random
        // multi-byte mutations of a paged one.
        let mut cells = vec![0u8; 40 * 40];
        cells[0] = 6;
        cells[39 * 40 + 39] = 9;
        let good = frame(40, 40, 16, 2, &cells);
        for bit in 0..good.len() * 8 {
            let mut mutated = good.clone();
            mutated[bit / 8] ^= 1 << (bit % 8);
            let _ = validate(&mutated, &mut scratch);
        }
        for _ in 0..512 {
            let mut mutated = good.clone();
            for _ in 0..(next() % 8 + 1) {
                let index = (next() as usize) % mutated.len();
                mutated[index] = (next() & 0xFF) as u8;
            }
            let truncate_to = (next() as usize) % (mutated.len() + 1);
            if next() % 4 == 0 {
                mutated.truncate(truncate_to);
            }
            let _ = validate(&mutated, &mut scratch);
        }
    }
}
