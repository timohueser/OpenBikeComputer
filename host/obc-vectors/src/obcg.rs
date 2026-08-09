//! Deterministic OBCG v1 fixtures for `specs/vectors/`.
//!
//! Inputs are semantic cell grids. Encoding goes through the Rust byte authority; Swift
//! independently decodes the same objects to the same cells and rejects every negative. Except
//! for truncation and the deliberate CRC mismatches, negatives recompute every CRC covering the
//! corrupted bytes so structural validation can never hide behind an integrity check.

use obc_crc::Crc32;
use obc_formats::obcg::{
    self, FrameInput, FLAG_FORECAST, FLAG_OBSERVED, HEADER_LEN, PRODUCT_DWD_RV, PRODUCT_ICON_EU, TIER_MODEL, TIER_RADAR,
};
use obc_formats::precip4::INTENSITY_NODATA;

/// Shared semantic seed: 2027-01-15T08:00:00Z, a run five minutes earlier.
pub const VALID_AT: i64 = 1_800_000_000;
pub const REFERENCE_TIME: i64 = VALID_AT - 300;
pub const SOUTH: i32 = 47_000_000;
pub const WEST: i32 = 7_000_000;
pub const CELL_LAT: u32 = 9_000;
pub const CELL_LON: u32 = 14_000;

fn encode(input: &FrameInput<'_>) -> Vec<u8> {
    let mut scratch = vec![0u8; usize::from(input.tile_edge) * usize::from(input.tile_edge)];
    let len = obcg::encoded_len(input, &mut scratch).expect("fixture length") as usize;
    let mut bytes = vec![0u8; len];
    let written = obcg::encode_format(input, &mut scratch, &mut bytes).expect("fixture encode");
    assert_eq!(written, len);
    bytes
}

fn radar_frame(width: u32, height: u32, tile_edge: u16, entries_per_page: u16, cells: &[u8]) -> Vec<u8> {
    encode(&FrameInput {
        product_id: PRODUCT_DWD_RV,
        tier: TIER_RADAR,
        flags: FLAG_OBSERVED,
        valid_at: VALID_AT,
        reference_time: REFERENCE_TIME,
        south_lat_udeg: SOUTH,
        west_lon_udeg: WEST,
        cell_lat_udeg: CELL_LAT,
        cell_lon_udeg: CELL_LON,
        width,
        height,
        cell_size_m: 1_000,
        tile_edge,
        entries_per_page,
        cells,
    })
}

/// A 32 x 32 all-dry frame at tile edge 32: one all-zero sentinel entry, zero payload bytes.
pub fn minimal_dry() -> Vec<u8> {
    radar_frame(32, 32, 32, 8, &vec![0u8; 32 * 32])
}

/// One 16 x 16 incompressible tile: the raw4 codec under a model/forecast header.
pub fn raw_tile() -> Vec<u8> {
    let cells: Vec<u8> = (0..256).map(|index| (index % 13) as u8).collect();
    encode(&FrameInput {
        product_id: PRODUCT_ICON_EU,
        tier: TIER_MODEL,
        flags: FLAG_FORECAST,
        valid_at: VALID_AT + 3_600,
        reference_time: REFERENCE_TIME,
        south_lat_udeg: SOUTH,
        west_lon_udeg: WEST,
        cell_lat_udeg: 62_500,
        cell_lon_udeg: 62_500,
        width: 16,
        height: 16,
        cell_size_m: 6_500,
        tile_edge: 16,
        entries_per_page: 4,
        cells: &cells,
    })
}

/// One uniform moderate-heavy tile: a 16-byte RLE4 payload.
pub fn rle_tile() -> Vec<u8> {
    radar_frame(16, 16, 16, 4, &[6u8; 256])
}

/// One all-no-data tile. Unavailable is encoded, never the dry sentinel.
pub fn nodata_tile() -> Vec<u8> {
    radar_frame(16, 16, 16, 4, &[INTENSITY_NODATA; 256])
}

/// The 40 x 40 grid at tile edge 16 and two entries per page: 3 x 3 tiles over five directory
/// pages with last-page padding, wet south-west and north-east corners, everything else dry.
/// This is the corridor request-accounting target and the paging-arithmetic pin.
pub fn multipage() -> Vec<u8> {
    let mut cells = vec![0u8; 40 * 40];
    cells[0] = 6; // (col 0, row 0): south-west corner
    cells[39 * 40 + 39] = 9; // (col 39, row 39): north-east corner, inside an edge-padded tile
    radar_frame(40, 40, 16, 2, &cells)
}

/// A 24 x 24 grid at tile edge 16: every tile is a partial edge tile whose padding must decode
/// as no-data, with a wet cell in each quadrant.
pub fn edge_padding() -> Vec<u8> {
    let mut cells = vec![0u8; 24 * 24];
    for (col, row, value) in [(3u32, 4u32, 2u8), (20, 4, 5), (3, 20, 8), (20, 20, 12)] {
        cells[(row * 24 + col) as usize] = value;
    }
    radar_frame(24, 24, 16, 8, &cells)
}

fn header(bytes: &[u8]) -> obcg::Header {
    let header_bytes: &[u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().unwrap();
    obcg::decode_header(header_bytes).expect("fixture header")
}

fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn rd_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn rd_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}

/// Recompute one directory page's trailing CRC after entry bytes changed.
fn refresh_page_crc(bytes: &mut [u8], header: &obcg::Header, page: u32) {
    let start = header.page_offset(page).unwrap() as usize;
    let page_bytes = header.page_bytes() as usize;
    let crc = Crc32::checksum(&bytes[start..start + page_bytes - obcg::PAGE_CRC_LEN]);
    put_u32(bytes, start + page_bytes - obcg::PAGE_CRC_LEN, crc);
}

/// Recompute the whole-object CRC then the header CRC (writer order, spec §8).
fn refresh_object_and_header_crc(bytes: &mut [u8]) {
    let object = obcg::object_crc(bytes);
    put_u32(bytes, obcg::HDR_OBJECT_CRC32, object);
    let header_bytes: &[u8; HEADER_LEN] = bytes[..HEADER_LEN].try_into().unwrap();
    let crc = obcg::header_crc(header_bytes);
    put_u32(bytes, obcg::HDR_HEADER_CRC32, crc);
}

/// Absolute offset of tile `index`'s directory entry, straight from the header arithmetic.
fn entry_offset(header: &obcg::Header, index: u32) -> usize {
    header.entry_offset(index).unwrap() as usize
}

pub fn invalid_truncated() -> Vec<u8> {
    let mut bytes = multipage();
    bytes.pop();
    bytes
}

/// A payload byte changed while every CRC covering it is left stale.
pub fn invalid_object_crc() -> Vec<u8> {
    let mut bytes = multipage();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    bytes
}

/// A header field changed without recomputing the header CRC.
pub fn invalid_header_crc() -> Vec<u8> {
    let mut bytes = multipage();
    bytes[obcg::HDR_TIER] = TIER_MODEL;
    bytes
}

/// An entry byte changed; object and header CRCs are refreshed, the page CRC deliberately not.
pub fn invalid_page_crc() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    bytes[entry + obcg::ENTRY_CRC32] ^= 0x01;
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// The first non-dry payload offset is shifted by one; every CRC is honest.
pub fn invalid_bad_offset() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET);
    put_u32(&mut bytes, entry + obcg::ENTRY_DATA_OFFSET, offset + 1);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// The second wet tile aliases the first tile's payload bytes: non-canonical packing/overlap.
pub fn invalid_overlap() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let first = entry_offset(&h, 0);
    let second = entry_offset(&h, 8);
    let (offset, len, codec, crc) = (
        rd_u32(&bytes, first + obcg::ENTRY_DATA_OFFSET),
        rd_u16(&bytes, first + obcg::ENTRY_ENCODED_LEN),
        bytes[first + obcg::ENTRY_CODEC],
        rd_u32(&bytes, first + obcg::ENTRY_CRC32),
    );
    put_u32(&mut bytes, second + obcg::ENTRY_DATA_OFFSET, offset);
    put_u16(&mut bytes, second + obcg::ENTRY_ENCODED_LEN, len);
    bytes[second + obcg::ENTRY_CODEC] = codec;
    put_u32(&mut bytes, second + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, h.page_of_entry(8));
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// Width zero: impossible dimensions with honest CRCs.
pub fn invalid_impossible_dims() -> Vec<u8> {
    let mut bytes = multipage();
    put_u32(&mut bytes, obcg::HDR_WIDTH, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// Tile edge 24 is not a power of two.
pub fn invalid_tile_edge() -> Vec<u8> {
    let mut bytes = multipage();
    put_u16(&mut bytes, obcg::HDR_TILE_EDGE, 24);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// Zero entries per page: paging arithmetic must fail closed, not divide by zero.
pub fn invalid_paging() -> Vec<u8> {
    let mut bytes = multipage();
    put_u16(&mut bytes, obcg::HDR_ENTRIES_PER_PAGE, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// The multipage fixture's south-west tile payload starts with a one-cell run of intensity 6
/// (`0x06`). Turning it into a full 16-cell run (`0xF6`) keeps the byte length but expands the
/// decoded sum to 271 cells. Tile, page, object and header CRCs are all honest.
pub fn invalid_rle_overlong() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    assert_eq!(bytes[offset], 0x06, "the fixture's first run is one cell of intensity 6");
    bytes[offset] = 0xF6;
    let crc = Crc32::checksum(&bytes[offset..offset + len]);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// Two adjacent equal runs where the first is shorter than 16 cells: noncanonical RLE.
pub fn invalid_rle_noncanonical() -> Vec<u8> {
    let mut bytes = nodata_tile();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    // The first full 16-cell no-data run becomes 15 + 1: the same 256-cell total and the same
    // 16-byte length, but the equal adjacent runs no longer follow a full run.
    assert_eq!(len, 16);
    bytes[offset] = 0xEF;
    bytes[offset + 1] = 0x0F;
    let crc = Crc32::checksum(&bytes[offset..offset + len]);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// A compressible payload labeled raw4: the all-no-data tile padded out to 128 raw bytes.
pub fn invalid_raw_compressible() -> Vec<u8> {
    let mut bytes = raw_tile();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    assert_eq!(len, 128, "raw_tile fixture must carry a raw4 payload");
    bytes[offset..offset + len].fill(0xFF);
    let crc = Crc32::checksum(&bytes[offset..offset + len]);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// An all-dry tile carried as an encoded RLE payload instead of the len-0 sentinel.
pub fn invalid_dry_encoded() -> Vec<u8> {
    let mut bytes = rle_tile();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    for byte in &mut bytes[offset..offset + len] {
        *byte = 0xF0; // sixteen full dry runs: canonically encoded, canonically forbidden
    }
    let crc = Crc32::checksum(&bytes[offset..offset + len]);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// A dry sentinel whose CRC field is nonzero: the len-0 entry must be exactly twelve zero bytes.
pub fn invalid_dry_sentinel_nonzero() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let entry = entry_offset(&h, 1); // tile 1 is dry in the multipage fixture
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, 0xDEAD_BEEF);
    refresh_page_crc(&mut bytes, &h, h.page_of_entry(1));
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// A payload byte changed with object and header CRCs refreshed but the tile CRC left stale.
pub fn invalid_tile_crc() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    bytes[offset] ^= 0x10;
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// A nonzero reserved header byte with honest CRCs.
pub fn invalid_reserved() -> Vec<u8> {
    let mut bytes = multipage();
    bytes[obcg::HDR_RESERVED] = 1;
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// Observed and Forecast both set: the source class must be exactly one of them.
pub fn invalid_flags() -> Vec<u8> {
    let mut bytes = multipage();
    put_u16(&mut bytes, obcg::HDR_FLAGS, FLAG_OBSERVED | FLAG_FORECAST);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

pub fn positives() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("grid-minimal-dry.obcg", minimal_dry()),
        ("grid-raw-tile.obcg", raw_tile()),
        ("grid-rle-tile.obcg", rle_tile()),
        ("grid-nodata-tile.obcg", nodata_tile()),
        ("grid-multipage.obcg", multipage()),
        ("grid-edge-padding.obcg", edge_padding()),
    ]
}

pub fn negatives() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("grid-invalid-truncated.obcg", invalid_truncated()),
        ("grid-invalid-object-crc.obcg", invalid_object_crc()),
        ("grid-invalid-header-crc.obcg", invalid_header_crc()),
        ("grid-invalid-page-crc.obcg", invalid_page_crc()),
        ("grid-invalid-bad-offset.obcg", invalid_bad_offset()),
        ("grid-invalid-overlap.obcg", invalid_overlap()),
        ("grid-invalid-impossible-dims.obcg", invalid_impossible_dims()),
        ("grid-invalid-tile-edge.obcg", invalid_tile_edge()),
        ("grid-invalid-paging.obcg", invalid_paging()),
        ("grid-invalid-rle-overlong.obcg", invalid_rle_overlong()),
        ("grid-invalid-rle-noncanonical.obcg", invalid_rle_noncanonical()),
        ("grid-invalid-raw-compressible.obcg", invalid_raw_compressible()),
        ("grid-invalid-dry-encoded.obcg", invalid_dry_encoded()),
        ("grid-invalid-dry-sentinel-nonzero.obcg", invalid_dry_sentinel_nonzero()),
        ("grid-invalid-tile-crc.obcg", invalid_tile_crc()),
        ("grid-invalid-reserved.obcg", invalid_reserved()),
        ("grid-invalid-flags.obcg", invalid_flags()),
    ]
}

pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    let mut fixtures = positives();
    fixtures.extend(negatives());
    fixtures
}
