//! OBCG v1 published precipitation grid object: byte authority for `specs/OBCG_Spec.md`.
//!
//! One OBCG object is exactly one grid frame — one product, one real UTC valid time, one regular
//! latitude/longitude window. The multi-frame table lives in the service manifest, never inside
//! an object, so heterogeneous per-frame geometry composes with no resampling by construction.
//!
//! The layout is built for HTTP Range consumers: a fixed self-CRC'd header, then a paged tile
//! directory whose pages verify independently, then tightly packed tile payloads. Corridor
//! extraction is: read the header, compute covering directory pages arithmetically, read those
//! pages, read the needed tiles — every piece independently CRC-verified. The whole-object CRC
//! stays for full-object consumers such as the baker's self-check.
//!
//! **Where the codecs live.** Codecs 0 (raw4) and 1 (RLE4) are the WX2 pair from
//! [`crate::precip4`], the one authority OBCG and OBCW share; this module delegates them
//! unchanged. Codec 2 (deflate over the raw4 nibbles) is **OBCG's own**, implemented here behind
//! the non-default `obcg-deflate` feature and never reachable from [`crate::obcw`]. That
//! placement is the whole point: the phone inflates OBCG and re-encodes the corridor as OBCW
//! RLE4, so the device — which links this crate with default features — contains no LZ decoder
//! and an OBCW tile can never claim a codec the firmware cannot decode.

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
/// The one canonical mosaic dataset: every source normalised onto the global 0.01 degree lattice
/// by the baker, best available in every cell, no provenance carried (#1242). The five codes
/// above and the two below are the per-source products it replaces.
pub const PRODUCT_MOSAIC: u8 = 6;
/// EUMETNET OPERA, the European radar pair (#1245). Source provenance only: these never reach a
/// published object once the mosaic is the dataset, and WXR7 #1246 deletes the whole registry.
pub const PRODUCT_OPERA_CIRRUS: u8 = 7;
pub const PRODUCT_OPERA_NIMBUS: u8 = 8;
pub const PRODUCT_EXPERIMENTAL: u8 = 255;

pub const TIER_RADAR: u8 = 1;
pub const TIER_MODEL: u8 = 2;
pub const TIER_FLOOR: u8 = 3;
/// The canonical mosaic's tier, and the honest answer to "which tier is a global mosaic?" —
/// **none of them** (#1243). A mosaic frame is 1 km radar over Germany and 27.75 km model over the
/// Pacific in the same object, so `TIER_RADAR` would be the same category of untruth `cell_size_m`
/// was retired for in #1242. The header slot is fixed and must be nonzero, so the field gets a code
/// that means "this object is not a member of any tier"; nothing may branch on it, and manifest v2
/// carries no tier at all.
pub const TIER_MOSAIC: u8 = 4;

/// Tile codec ids (spec §4.1/§5). `0` and `1` are [`crate::precip4`]'s shared raw4/RLE4 pair —
/// the identical two bytes OBCW uses. `2` is OBCG-only: raw DEFLATE (RFC 1951, no wrapper) over
/// the tile's raw4 nibble image, decoded by this module and by nothing on the device.
pub const CODEC_RAW4: u8 = precip4::CODEC_RAW4;
pub const CODEC_RLE4: u8 = precip4::CODEC_RLE4;
pub const CODEC_DEFLATE4: u8 = 2;

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

    /// True when `tile_index` names a partial tile at the north or east grid edge. Such a tile
    /// contains no-data padding and may therefore never be a dry sentinel (spec §4.1).
    pub fn tile_is_partial(&self, tile_index: u32) -> bool {
        let edge = u32::from(self.tile_edge);
        let tile_col = tile_index % self.tile_cols();
        let tile_row = tile_index / self.tile_cols();
        self.width < (tile_col + 1) * edge || self.height < (tile_row + 1) * edge
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

/// OBCG codec 2: raw DEFLATE over the tile's raw4 nibble image (spec §5).
///
/// Host/phone only. The stream carries no zlib or gzip wrapper — the directory entry's CRC-32
/// already covers the stored bytes, so a wrapper would only add duplicate integrity and per-tile
/// bytes — and the decompressed image is exactly the tile's `N / 2` raw4 bytes, which is what
/// bounds the output before a single byte is allocated.
#[cfg(feature = "obcg-deflate")]
mod deflate4 {
    use super::{precip4, DecodeError};
    use alloc::vec;
    use alloc::vec::Vec;
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;

    /// Producer knob, not a format parameter: a decoder accepts any conforming stream. Level 6 is
    /// what WXR1 #1240 measured the published sizes at.
    ///
    /// Because the format does not pin compressed bytes, *this compressor at this level* is what
    /// every byte-for-byte pin downstream actually rests on — the checked-in vectors, the event
    /// pack, and the bakery's own re-bake self-check. `miniz_oxide` is therefore pinned to an
    /// exact version in `Cargo.toml`; changing either it or this constant is a fixture
    /// regeneration, not a tidy-up.
    pub const LEVEL: u8 = 6;

    /// The raw4 nibble image the codec compresses: two row-major cells per byte, earlier cell in
    /// the low nibble — codec 0's bytes, without codec 0's canonicality restriction.
    fn pack_raw4(cells: &[u8]) -> Vec<u8> {
        let mut packed = vec![0u8; cells.len() / 2];
        for (index, byte) in packed.iter_mut().enumerate() {
            *byte = cells[index * 2] | (cells[index * 2 + 1] << 4);
        }
        packed
    }

    /// Compress one tile. `cells` must already have been intensity-validated.
    pub fn compress(cells: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec(&pack_raw4(cells), LEVEL)
    }

    /// Inflate one payload into `out` (`out.len()` = the tile's decoded cell count) and apply
    /// every §5 rule the bytes have to satisfy.
    ///
    /// The output buffer is exactly the tile's raw4 length, sized from the *header* rather than
    /// from anything the payload claims, so an over-inflating stream is refused by construction:
    /// there is no allocation a bomb can grow.
    pub fn decode(payload: &[u8], out: &mut [u8]) -> Result<(), DecodeError> {
        if !precip4::valid_cell_count(out.len()) {
            return Err(DecodeError::Bounds);
        }
        let raw4_len = out.len() / 2;
        // Codec 2 exists only where it beats raw4; the strictly-smaller-than-canonical rule below
        // subsumes this, but checking it first keeps an oversized payload from ever being inflated.
        if payload.is_empty() || payload.len() >= raw4_len {
            return Err(DecodeError::TileCodec);
        }
        let mut packed = vec![0u8; raw4_len];
        let mut state = DecompressorOxide::new();
        let (status, consumed, written) =
            decompress(&mut state, payload, &mut packed, 0, inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
        // Exactly one complete stream, exactly the tile's raw4 image: a truncated stream, trailing
        // bytes after it, a short output and an over-inflating one are all the same verdict.
        if status != TINFLStatus::Done || consumed != payload.len() || written != raw4_len {
            return Err(DecodeError::TileCodec);
        }
        for (index, &byte) in packed.iter().enumerate() {
            out[index * 2] = byte & 0x0F;
            out[index * 2 + 1] = byte >> 4;
        }
        // §5 canonical choice: codec 2 is legal only where it is strictly smaller than the shared
        // raw4/RLE4 authority's canonical length for the same cells. `encoded_cells_len` validates
        // every intensity code on the way, so reserved nibbles are rejected here too.
        let canonical = precip4::encoded_cells_len(out).map_err(|_| DecodeError::TileCodec)?;
        if payload.len() >= canonical {
            return Err(DecodeError::TileCodec);
        }
        Ok(())
    }
}

/// Verify one non-dry tile payload against its directory entry: CRC over the **encoded** bytes
/// first — so a corrupt payload is refused before any decompression work — then the §5 codec
/// rules for this product's `tile_edge^2` cells.
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
    if entry.codec == CODEC_DEFLATE4 {
        // Codec 2 cannot be validated without expanding it; without the host feature this crate
        // has no inflate at all and the tile is simply undecodable, which is the honest verdict.
        #[cfg(feature = "obcg-deflate")]
        {
            let mut cells = alloc::vec![0u8; header.tile_cells()];
            return deflate4::decode(payload, &mut cells);
        }
        #[cfg(not(feature = "obcg-deflate"))]
        return Err(DecodeError::TileCodec);
    }
    precip4::validate_cells(entry.codec, payload, header.tile_cells()).map_err(|_| DecodeError::TileCodec)
}

/// Decode one verified tile into `out` (`header.tile_cells()` bytes). A dry entry has no
/// payload bytes — offering any is rejected (the Swift decoder gives the identical verdict) —
/// and fills the tile with [`INTENSITY_DRY`].
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
        if !payload.is_empty() {
            return Err(DecodeError::Directory);
        }
        out.fill(INTENSITY_DRY);
        return Ok(());
    }
    if entry.codec == CODEC_DEFLATE4 {
        if payload.len() != usize::from(entry.encoded_len) {
            return Err(DecodeError::Bounds);
        }
        if Crc32::checksum(payload) != entry.crc32 {
            return Err(DecodeError::TileCrc);
        }
        #[cfg(feature = "obcg-deflate")]
        return deflate4::decode(payload, out);
        #[cfg(not(feature = "obcg-deflate"))]
        return Err(DecodeError::TileCodec);
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

/// The encoding §5 selects for one gathered tile. `encoded_len == 0` is the all-dry sentinel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TilePlan {
    codec: u8,
    encoded_len: usize,
}

/// A full interior tile of dry cells is the §4.1 sentinel, never a payload.
fn tile_is_all_dry(scratch: &[u8]) -> bool {
    scratch.iter().all(|&cell| cell == INTENSITY_DRY)
}

/// Choose the tile's codec by measured size (spec §5): the smallest payload wins and ties go to
/// the lower codec id, so a producer's choice is determined by the cells rather than by taste.
/// Codec 2's length is only knowable by compressing, which is why the sizing pass compresses too;
/// both passes are the same deterministic function of the same bytes.
fn plan_tile(scratch: &[u8]) -> Result<TilePlan, EncodeError> {
    if tile_is_all_dry(scratch) {
        return Ok(TilePlan { codec: CODEC_RAW4, encoded_len: 0 });
    }
    let canonical = precip4::encoded_cells_len(scratch).map_err(|_| EncodeError::InvalidInput)?;
    let paired = if canonical < scratch.len() / 2 { CODEC_RLE4 } else { CODEC_RAW4 };
    #[cfg(feature = "obcg-deflate")]
    {
        let compressed = deflate4::compress(scratch).len();
        if compressed < canonical {
            return Ok(TilePlan { codec: CODEC_DEFLATE4, encoded_len: compressed });
        }
    }
    Ok(TilePlan { codec: paired, encoded_len: canonical })
}

/// Write one tile's payload into the front of `out`, returning the same plan [`plan_tile`] would.
fn encode_tile_payload(scratch: &[u8], out: &mut [u8]) -> Result<TilePlan, EncodeError> {
    #[cfg(feature = "obcg-deflate")]
    {
        let canonical = precip4::encoded_cells_len(scratch).map_err(|_| EncodeError::InvalidInput)?;
        let compressed = deflate4::compress(scratch);
        if compressed.len() < canonical {
            let destination = out.get_mut(..compressed.len()).ok_or(EncodeError::OutputTooSmall)?;
            destination.copy_from_slice(&compressed);
            return Ok(TilePlan { codec: CODEC_DEFLATE4, encoded_len: compressed.len() });
        }
    }
    let encoding = precip4::encode_cells(scratch, out).map_err(|error| match error {
        precip4::Error::OutputTooSmall => EncodeError::OutputTooSmall,
        _ => EncodeError::InvalidInput,
    })?;
    Ok(TilePlan { codec: encoding.codec, encoded_len: usize::from(encoding.encoded_len) })
}

/// An upper bound on the encoded object length, computed from the geometry alone — no cells are
/// read and nothing is compressed.
///
/// This is what a producer sizing an output buffer should use. [`encoded_len`] is exact but has
/// to run the whole per-tile codec choice to get there (codec 2's length is only knowable by
/// compressing), so a caller that sizes with it and then calls [`encode_format`] compresses every
/// tile twice. Sizing with the bound and truncating to `encode_format`'s return value costs one
/// pass and at most `width * height / 2` bytes of slack — the raw4 image the frame could not
/// exceed anyway.
pub fn max_encoded_len(input: &FrameInput<'_>) -> Result<u32, EncodeError> {
    let header = frame_header(input)?;
    let directory_len = u64::from(header.page_count())
        .checked_mul(u64::from(header.page_bytes()))
        .ok_or(EncodeError::LengthOverflow)?;
    let payload_bound = u64::from(header.tile_count())
        .checked_mul(header.tile_cells() as u64 / 2)
        .ok_or(EncodeError::LengthOverflow)?;
    let total = (HEADER_LEN as u64)
        .checked_add(directory_len)
        .and_then(|value| value.checked_add(payload_bound))
        .ok_or(EncodeError::LengthOverflow)?;
    u32::try_from(total).map_err(|_| EncodeError::LengthOverflow)
}

/// Total encoded object length for `input`, using a caller-provided `tile_cells()`-sized scratch
/// buffer. Exact, but it runs the full per-tile codec choice to get there — see
/// [`max_encoded_len`] for the cheap bound a producer should size with.
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
            total += plan_tile(scratch)?.encoded_len as u64;
        }
    }
    u32::try_from(total).map_err(|_| EncodeError::LengthOverflow)
}

/// Encode one complete OBCG object into `out`, returning the byte length. `out` must be at least
/// [`encoded_len`] bytes; `scratch` is one `tile_cells()`-sized buffer.
pub fn encode_format(input: &FrameInput<'_>, scratch: &mut [u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    let header = frame_header(input)?;
    if scratch.len() != header.tile_cells() {
        return Err(EncodeError::InvalidInput);
    }
    let page_bytes = header.page_bytes() as usize;
    let page_count = header.page_count() as usize;
    let directory_len = page_count.checked_mul(page_bytes).ok_or(EncodeError::LengthOverflow)?;
    let data_offset = HEADER_LEN.checked_add(directory_len).ok_or(EncodeError::LengthOverflow)?;
    out.get_mut(..data_offset).ok_or(EncodeError::OutputTooSmall)?.fill(0);

    // One payload pass: each tile is planned and written in the same step. `encoded_len` runs the
    // same deterministic per-tile choice, so the size it promised and the bytes written here are
    // the same function of the same cells — nothing can drift between a sizing and a writing pass.
    let mut payload = data_offset;
    for tile_row in 0..header.tile_rows() {
        for tile_col in 0..header.tile_cols() {
            gather_tile(input, tile_col, tile_row, scratch);
            if tile_is_all_dry(scratch) {
                // All-dry sentinel: the entry stays twelve zero bytes and costs no payload.
                continue;
            }
            let tile_index = (tile_row * header.tile_cols() + tile_col) as usize;
            let entry_offset = HEADER_LEN
                + (tile_index / usize::from(input.entries_per_page)) * page_bytes
                + (tile_index % usize::from(input.entries_per_page)) * DIRECTORY_ENTRY_LEN;
            let tail = out.get_mut(payload..).ok_or(EncodeError::OutputTooSmall)?;
            let plan = encode_tile_payload(scratch, tail)?;
            let end = payload.checked_add(plan.encoded_len).ok_or(EncodeError::LengthOverflow)?;
            let encoded_len = u16::try_from(plan.encoded_len).map_err(|_| EncodeError::Internal)?;
            let crc = Crc32::checksum(out.get(payload..end).ok_or(EncodeError::OutputTooSmall)?);
            let offset = u32::try_from(payload).map_err(|_| EncodeError::LengthOverflow)?;
            put_u32(out, entry_offset + ENTRY_DATA_OFFSET, offset);
            put_u16(out, entry_offset + ENTRY_ENCODED_LEN, encoded_len);
            out[entry_offset + ENTRY_CODEC] = plan.codec;
            put_u32(out, entry_offset + ENTRY_CRC32, crc);
            payload = end;
        }
    }
    let total = payload;
    let data_len = total - data_offset;
    u32::try_from(total).map_err(|_| EncodeError::LengthOverflow)?;
    let bytes = out.get_mut(..total).ok_or(EncodeError::OutputTooSmall)?;

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
                // §4.1: a partial edge tile contains no-data padding and can never be a dry
                // sentinel — accepting one would let missing edge data decode as dry weather.
                if header.tile_is_partial(tile_index as u32) {
                    return Err(DecodeError::Directory);
                }
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

    /// §4.1: a dry sentinel at a partial edge tile is rejected, and a dry entry offered payload
    /// bytes is rejected — both with the same verdict the Swift decoder gives.
    #[test]
    fn dry_sentinels_are_forbidden_at_partial_edge_tiles_and_never_carry_payload() {
        // An 8 x 8 grid at tile edge 16: the single tile is partial. Rewrite its encoded entry
        // as a dry sentinel with honest CRCs; the validator must refuse it.
        let bytes = frame(8, 8, 16, 4, &[0u8; 64]);
        let header = validated(&bytes).unwrap();
        assert!(header.tile_is_partial(0));
        let mut forged = bytes.clone();
        forged.truncate(header.data_offset as usize);
        let entry_offset = header.entry_offset(0).unwrap() as usize;
        forged[entry_offset..entry_offset + DIRECTORY_ENTRY_LEN].fill(0);
        put_u32(&mut forged, HDR_DATA_LEN, 0);
        let total_len = forged.len() as u32;
        put_u32(&mut forged, HDR_TOTAL_LEN, total_len);
        let page_offset = header.page_offset(0).unwrap() as usize;
        let page_bytes = header.page_bytes() as usize;
        let page_crc = Crc32::checksum(&forged[page_offset..page_offset + page_bytes - PAGE_CRC_LEN]);
        put_u32(&mut forged, page_offset + page_bytes - PAGE_CRC_LEN, page_crc);
        put_u32(&mut forged, HDR_OBJECT_CRC32, 0);
        put_u32(&mut forged, HDR_HEADER_CRC32, 0);
        let object = object_crc(&forged);
        put_u32(&mut forged, HDR_OBJECT_CRC32, object);
        let header_bytes: &[u8; HEADER_LEN] = forged[..HEADER_LEN].try_into().unwrap();
        let crc = header_crc(header_bytes);
        put_u32(&mut forged, HDR_HEADER_CRC32, crc);
        assert_eq!(validated(&forged), Err(DecodeError::Directory));

        // A full-tile dry sentinel decodes only with an empty payload slice.
        let dry = TileEntry { data_offset: 0, encoded_len: 0, codec: 0, crc32: 0 };
        let mut out = vec![0u8; header.tile_cells()];
        assert_eq!(decode_tile_cells(&header, &dry, &[], &mut out), Ok(()));
        assert!(out.iter().all(|&cell| cell == INTENSITY_DRY));
        assert_eq!(decode_tile_cells(&header, &dry, &[0xF0], &mut out), Err(DecodeError::Directory));
    }

    /// Codec 2 is chosen exactly where it strictly beats the shared raw4/RLE4 authority, and the
    /// ties and losses stay with the lower codec ids. Every case round-trips to the same cells.
    #[cfg(feature = "obcg-deflate")]
    #[test]
    fn codec_choice_follows_measured_size_with_the_low_id_tie_break() {
        // (cells, tile edge, expected codec) — the shapes the vectors also pin.
        let upsampled: Vec<u8> = (0..64 * 64)
            .map(|index| {
                let (row, col) = (index / 64, index % 64);
                (((row / 8) * 8 + (col / 8)) % 13) as u8
            })
            .collect();
        let mut random = 0x0BC5_1190u32;
        let incompressible: Vec<u8> = (0..256)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                let candidate = (random & 0x0F) as u8;
                if precip4::valid_intensity(candidate) {
                    candidate
                } else {
                    INTENSITY_NODATA
                }
            })
            .collect();
        let runs: Vec<u8> = (0..256).map(|index| ((index / 8) % 13) as u8).collect();
        for (cells, edge, expected) in [
            (upsampled, 64u16, CODEC_DEFLATE4),
            (incompressible, 16, CODEC_RAW4),
            (runs, 16, CODEC_RLE4),           // deflate4 loses outright on short varied runs
            (vec![6u8; 256], 16, CODEC_RLE4), // deflate4 ties at 16 bytes; the low id wins
            (vec![INTENSITY_NODATA; 256], 16, CODEC_DEFLATE4),
        ] {
            let width = u32::from(edge);
            let bytes = frame(width, width, edge, 8, &cells);
            let header = validated(&bytes).unwrap();
            let page_offset = header.page_offset(0).unwrap() as usize;
            let page = &bytes[page_offset..page_offset + header.page_bytes() as usize];
            let entry = decode_entry(page, 0).unwrap();
            assert_eq!(entry.codec, expected, "codec for a {edge}x{edge} tile");
            let payload =
                &bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)];
            let mut out = vec![0u8; header.tile_cells()];
            decode_tile_cells(&header, &entry, payload, &mut out).unwrap();
            assert_eq!(out, cells, "round trip for codec {expected}");
        }
    }

    /// Every codec-2 failure mode is one verdict, and none of them allocates on the payload's
    /// word: truncation, trailing bytes, an over-inflating bomb, a short output, a payload that
    /// does not beat the canonical raw4/RLE4 length, and an unknown codec id.
    #[cfg(feature = "obcg-deflate")]
    #[test]
    fn deflate_tiles_fail_closed_without_trusting_the_payload() {
        let cells: Vec<u8> = (0..4_096)
            .map(|index| {
                let (row, col) = (index / 64, index % 64);
                (((row / 8) * 8 + (col / 8)) % 13) as u8
            })
            .collect();
        let bytes = frame(64, 64, 64, 8, &cells);
        let header = validated(&bytes).unwrap();
        let page_offset = header.page_offset(0).unwrap() as usize;
        let page = &bytes[page_offset..page_offset + header.page_bytes() as usize];
        let good = decode_entry(page, 0).unwrap();
        let payload =
            bytes[good.data_offset as usize..good.data_offset as usize + usize::from(good.encoded_len)].to_vec();
        let raw4_len = header.tile_cells() / 2;
        let mut out = vec![0u8; header.tile_cells()];

        let entry_for = |payload: &[u8], codec: u8| TileEntry {
            data_offset: good.data_offset,
            encoded_len: payload.len() as u16,
            codec,
            crc32: Crc32::checksum(payload),
        };
        // The honest payload still decodes, so every rejection below is about the mutation.
        assert_eq!(decode_tile_cells(&header, &entry_for(&payload, CODEC_DEFLATE4), &payload, &mut out), Ok(()));

        let truncated = payload[..payload.len() - 2].to_vec();
        let mut trailing = payload.clone();
        trailing.push(0x00);
        // A stream that inflates to four times the tile's raw4 image, and one that stops short.
        let bomb = deflate4::compress(&vec![INTENSITY_DRY; header.tile_cells() * 4]);
        let short = miniz_oxide::deflate::compress_to_vec(&vec![0u8; raw4_len - 1], deflate4::LEVEL);
        for (name, mutated) in
            [("truncated", truncated), ("trailing bytes", trailing), ("bomb", bomb), ("short output", short)]
        {
            let entry = entry_for(&mutated, CODEC_DEFLATE4);
            assert_eq!(
                decode_tile_cells(&header, &entry, &mutated, &mut out),
                Err(DecodeError::TileCodec),
                "{name} must be refused"
            );
        }
        // A stale CRC is caught before the payload is inflated at all, and an unknown codec id
        // never reaches a decoder.
        let stale = TileEntry { crc32: good.crc32 ^ 1, ..entry_for(&payload, CODEC_DEFLATE4) };
        assert_eq!(decode_tile_cells(&header, &stale, &payload, &mut out), Err(DecodeError::TileCrc));
        let unknown = entry_for(&payload, 3);
        assert_eq!(decode_tile_cells(&header, &unknown, &payload, &mut out), Err(DecodeError::TileCodec));

        // §5: the tile's history starts empty. A fixed-Huffman stream that emits one literal and
        // then matches 127 bytes at distance 4 reaches before the start of the raw4 image; a
        // decoder that zero-filled instead of failing would decode different cells from these
        // same bytes, which is the one divergence a two-language format cannot survive.
        let early = [0x63u8, 0x18, 0x60, 0x0C, 0x00];
        assert_eq!(
            decode_tile_cells(&header, &entry_for(&early, CODEC_DEFLATE4), &early, &mut out),
            Err(DecodeError::TileCodec),
            "a match distance before the tile image must be refused"
        );

        // A perfectly valid deflate stream is still refused where it does not beat the canonical
        // raw4/RLE4 length: 16 varied 8-cell runs are 32 RLE4 bytes and 46 deflate bytes.
        let runs: Vec<u8> = (0..256).map(|index| ((index / 8) % 13) as u8).collect();
        let small = validated(&frame(16, 16, 16, 8, &runs)).unwrap();
        let losing = deflate4::compress(&runs);
        assert!(losing.len() > precip4::encoded_cells_len(&runs).unwrap(), "the fixture premise");
        let entry = TileEntry {
            data_offset: small.data_offset,
            encoded_len: losing.len() as u16,
            codec: CODEC_DEFLATE4,
            crc32: Crc32::checksum(&losing),
        };
        let mut small_out = vec![0u8; small.tile_cells()];
        assert_eq!(
            decode_tile_cells(&small, &entry, &losing, &mut small_out),
            Err(DecodeError::TileCodec),
            "codec 2 must beat the canonical raw4/RLE4 length"
        );
    }

    /// §5: a DEFLATE stream ends on a bit boundary, so the leftover bits of the payload's last
    /// byte are padding, not data. A decoder MUST NOT reject on them — six of this stream's eight
    /// last-byte bit patterns are the same object, and a reader that disagreed would refuse
    /// conforming published frames.
    #[cfg(feature = "obcg-deflate")]
    #[test]
    fn padding_bits_of_the_final_byte_are_not_data() {
        let cells: Vec<u8> = (0..4_096)
            .map(|index| {
                let (row, col) = (index / 64, index % 64);
                (((row / 8) * 8 + (col / 8)) % 13) as u8
            })
            .collect();
        let bytes = frame(64, 64, 64, 8, &cells);
        let header = validated(&bytes).unwrap();
        let page_offset = header.page_offset(0).unwrap() as usize;
        let page = &bytes[page_offset..page_offset + header.page_bytes() as usize];
        let good = decode_entry(page, 0).unwrap();
        let payload =
            bytes[good.data_offset as usize..good.data_offset as usize + usize::from(good.encoded_len)].to_vec();

        let mut accepted = 0usize;
        let mut out = vec![0u8; header.tile_cells()];
        for bit in 0..8u32 {
            let mut flipped = payload.clone();
            let last = flipped.len() - 1;
            flipped[last] ^= 1 << bit;
            let entry = TileEntry { crc32: Crc32::checksum(&flipped), encoded_len: flipped.len() as u16, ..good };
            if decode_tile_cells(&header, &entry, &flipped, &mut out).is_ok() {
                assert_eq!(out, cells, "an accepted variant must decode to the same cells");
                accepted += 1;
            }
        }
        // Exactly six: this stream's final block ends two bits into its last byte, so bits 0 and 1
        // are data and bits 2...7 are padding. Pinning the number rather than a floor means a
        // compressor change that moved the boundary shows up here as a fact that moved, not as a
        // test that still passes for a different reason.
        assert_eq!(accepted, 6, "six of the eight last-byte bit patterns are the same object");

        // The cheap producer-side bound is a real bound, and slack enough to hold the object.
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
            width: 64,
            height: 64,
            cell_size_m: 1_000,
            tile_edge: 64,
            entries_per_page: 8,
            cells: &cells,
        };
        let mut scratch = vec![0u8; header.tile_cells()];
        let exact = encoded_len(&input, &mut scratch).unwrap();
        let bound = max_encoded_len(&input).unwrap();
        assert_eq!(exact as usize, bytes.len());
        assert!(bound >= exact, "{bound} must bound {exact}");
        let mut sized = vec![0u8; bound as usize];
        assert_eq!(encode_format(&input, &mut scratch, &mut sized).unwrap(), exact as usize);
    }

    /// Without the host feature this crate has no inflate — the device build's shape. A codec-2
    /// tile is then simply undecodable, which is the honest verdict rather than a silent accept.
    #[cfg(not(feature = "obcg-deflate"))]
    #[test]
    fn the_default_build_carries_no_inflate_and_refuses_codec_two() {
        let bytes = frame(16, 16, 16, 4, &[6u8; 256]);
        let header = validated(&bytes).unwrap();
        let payload = [0x63u8, 0x60, 0x00, 0x00];
        let entry = TileEntry {
            data_offset: header.data_offset,
            encoded_len: payload.len() as u16,
            codec: CODEC_DEFLATE4,
            crc32: Crc32::checksum(&payload),
        };
        let mut out = vec![0u8; header.tile_cells()];
        assert_eq!(validate_tile_payload(&header, &entry, &payload), Err(DecodeError::TileCodec));
        assert_eq!(decode_tile_cells(&header, &entry, &payload, &mut out), Err(DecodeError::TileCodec));
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
            if next().is_multiple_of(4) {
                mutated.truncate(truncate_to);
            }
            let _ = validate(&mutated, &mut scratch);
        }
    }
}
