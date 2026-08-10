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

/// Deterministic pseudo-random intensities over the valid alphabet (0...12 plus no-data). Any
/// *structured* fill compresses, so this is what keeps a genuine raw4 tile in the vector set:
/// 169 distinct nibble-pair byte values defeat deflate's Huffman table on a 128-byte tile.
fn incompressible_cells(count: usize) -> Vec<u8> {
    let mut state = 0x0BC5_1190u32;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let candidate = (state & 0x0F) as u8;
            if candidate <= 12 {
                candidate
            } else {
                INTENSITY_NODATA
            }
        })
        .collect()
}

/// One 16 x 16 incompressible tile: the raw4 codec under a model/forecast header.
pub fn raw_tile() -> Vec<u8> {
    let cells = incompressible_cells(256);
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

/// One uniform moderate-heavy tile: a 16-byte RLE4 payload. Deflate4 also reaches 16 bytes here,
/// so this vector pins §5's tie-break — equal lengths go to the **lower codec id**.
pub fn rle_tile() -> Vec<u8> {
    radar_frame(16, 16, 16, 4, &[6u8; 256])
}

/// Sixteen varied 8-cell runs: RLE4 is 32 bytes and deflate4 is 46, so RLE4 wins outright. This
/// is the vector that proves codec 1 is still load-bearing rather than legacy — and the base for
/// the overlong and noncanonical RLE negatives.
pub fn rle_wins() -> Vec<u8> {
    let cells: Vec<u8> = (0..256).map(|index| ((index / 8) % 13) as u8).collect();
    radar_frame(16, 16, 16, 4, &cells)
}

/// A second legal byte image of [`deflate_tile`], differing only in the **padding bits** of its
/// final payload byte (§5). A DEFLATE stream ends on a bit boundary and the leftover bits of the
/// last byte are not data; a decoder that rejected this object — or that treated the bits as
/// signal — would be wrong, so this is a *positive* whose whole job is to be accepted and to
/// decode to `deflate_tile`'s cells exactly.
pub fn deflate_padding_bits() -> Vec<u8> {
    let mut bytes = deflate_tile();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    assert_eq!(bytes[entry + obcg::ENTRY_CODEC], obcg::CODEC_DEFLATE4);
    // Bit 7 of this stream's last byte is padding; bits 0 and 1 are the tail of the final block.
    // The assertion below is the guard: if a compressor change moves the boundary, the fixture
    // fails to build rather than shipping a vector that pins the wrong claim.
    let last = offset + len - 1;
    let decode = |payload: &[u8]| -> Vec<u8> {
        let probe = obcg::TileEntry {
            data_offset: offset as u32,
            encoded_len: len as u16,
            codec: obcg::CODEC_DEFLATE4,
            crc32: Crc32::checksum(payload),
        };
        let mut cells = vec![0u8; h.tile_cells()];
        obcg::decode_tile_cells(&h, &probe, payload, &mut cells).expect("a legal deflate4 stream");
        cells
    };
    let original = decode(&bytes[offset..offset + len]);
    bytes[last] ^= 0x80;
    let payload = bytes[offset..offset + len].to_vec();
    // The guard, and it has to be this strict: a flipped bit that still *decoded* but decoded to
    // different cells would ship a vector claiming the opposite of what §5 says. Accepting is not
    // enough — the two byte images must be the same object.
    assert_eq!(decode(&payload), original, "the padding-bit flip must decode to identical cells");
    let crc = Crc32::checksum(&payload);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// The production geometry WXR1 recommends: `tile_edge = 256`, 65,536 cells in one tile. It is
/// the only vector where a payload can exceed 255 bytes (so the directory's `uint16` length is
/// exercised) and where §5's pre-inflate ceiling reaches 32,767. The field is 16 x 16 blocks of
/// pseudo-random intensity — coarse enough for deflate4 to win, varied enough not to collapse to
/// nothing.
pub fn deflate_edge256() -> Vec<u8> {
    let mut state = 0x0BC5_1256u32;
    let blocks: Vec<u8> = (0..16 * 16)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let candidate = (state & 0x0F) as u8;
            if candidate <= 12 {
                candidate
            } else {
                INTENSITY_NODATA
            }
        })
        .collect();
    let cells: Vec<u8> = (0..256 * 256)
        .map(|index| {
            let (row, col) = (index / 256, index % 256);
            blocks[(row / 16) * 16 + col / 16]
        })
        .collect();
    radar_frame(256, 256, 256, 128, &cells)
}

/// A 64 x 64 tile of coarse data upsampled onto a fine lattice — 8 x 8 blocks of one value, the
/// exact shape the baker publishes. raw4 is 2,048 bytes and RLE4 512; deflate4 is 77, because it
/// has the back-reference RLE4 lacks. Paged at WXR1's recommended `entries_per_page = 128`, like
/// [`deflate_edge256`].
pub fn deflate_tile() -> Vec<u8> {
    let cells: Vec<u8> = (0..64 * 64)
        .map(|index| {
            let (row, col) = (index / 64, index % 64);
            (((row / 8) * 8 + (col / 8)) % 13) as u8
        })
        .collect();
    radar_frame(64, 64, 64, 128, &cells)
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

/// Raw DEFLATE (RFC 1951, no wrapper) at the producer's level — spec §5's codec 2 stream.
fn deflate(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec(bytes, 6)
}

/// The raw4 nibble image codec 2 compresses: two row-major cells per byte, earlier cell low.
fn pack_raw4(cells: &[u8]) -> Vec<u8> {
    (0..cells.len() / 2).map(|index| cells[index * 2] | (cells[index * 2 + 1] << 4)).collect()
}

/// Replace a **single-tile** object's only payload, fixing every length and CRC the change
/// touches. The bases are one-tile frames, so the data section *is* that payload and canonical
/// packing survives by construction — which is what leaves the codec rule as the only thing a
/// validator can be rejecting.
fn with_single_tile_payload(mut bytes: Vec<u8>, codec: u8, payload: &[u8]) -> Vec<u8> {
    let h = header(&bytes);
    assert_eq!(h.tile_count(), 1, "the payload-swap bases carry exactly one tile");
    let entry = entry_offset(&h, 0);
    bytes.truncate(h.data_offset as usize);
    bytes.extend_from_slice(payload);
    put_u32(&mut bytes, entry + obcg::ENTRY_DATA_OFFSET, h.data_offset);
    put_u16(&mut bytes, entry + obcg::ENTRY_ENCODED_LEN, u16::try_from(payload.len()).unwrap());
    bytes[entry + obcg::ENTRY_CODEC] = codec;
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, Crc32::checksum(payload));
    put_u32(&mut bytes, obcg::HDR_DATA_LEN, payload.len() as u32);
    let total = bytes.len() as u32;
    put_u32(&mut bytes, obcg::HDR_TOTAL_LEN, total);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
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

/// The `rle_wins` fixture's payload is sixteen 8-cell runs (`0x7v`). Widening the first to a full
/// 16 cells (`0xFv`) keeps the byte length but expands the decoded sum to 264 cells. Tile, page,
/// object and header CRCs are all honest.
pub fn invalid_rle_overlong() -> Vec<u8> {
    let mut bytes = rle_wins();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    assert_eq!(bytes[offset], 0x70, "the fixture's first run is eight cells of intensity 0");
    bytes[offset] = 0xF0;
    let crc = Crc32::checksum(&bytes[offset..offset + len]);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// Two adjacent equal runs where the first is shorter than 16 cells: noncanonical RLE. Giving the
/// `rle_wins` fixture's second run the first run's intensity leaves the length *and* the 256-cell
/// sum intact, so only the maximal-run rule can reject it.
pub fn invalid_rle_noncanonical() -> Vec<u8> {
    let mut bytes = rle_wins();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    assert_eq!((bytes[offset], bytes[offset + 1]), (0x70, 0x71), "eight cells of 0 then eight of 1");
    bytes[offset + 1] = 0x70;
    let crc = Crc32::checksum(&bytes[offset..offset + len]);
    put_u32(&mut bytes, entry + obcg::ENTRY_CRC32, crc);
    refresh_page_crc(&mut bytes, &h, 0);
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// A codec id outside §4.1's closed set `{0, 1, 2}`, with every CRC honest.
pub fn invalid_codec_id() -> Vec<u8> {
    let mut bytes = multipage();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    bytes[entry + obcg::ENTRY_CODEC] = 3;
    refresh_page_crc(&mut bytes, &h, h.page_of_entry(0));
    refresh_object_and_header_crc(&mut bytes);
    bytes
}

/// A codec-2 payload with its last two bytes cut off: the stream no longer terminates.
pub fn invalid_deflate_truncated() -> Vec<u8> {
    let bytes = deflate_tile();
    let h = header(&bytes);
    let entry = entry_offset(&h, 0);
    let offset = rd_u32(&bytes, entry + obcg::ENTRY_DATA_OFFSET) as usize;
    let len = usize::from(rd_u16(&bytes, entry + obcg::ENTRY_ENCODED_LEN));
    assert_eq!(bytes[entry + obcg::ENTRY_CODEC], obcg::CODEC_DEFLATE4);
    let payload = bytes[offset..offset + len - 2].to_vec();
    with_single_tile_payload(bytes, obcg::CODEC_DEFLATE4, &payload)
}

/// A well-formed stream that inflates to 8,192 bytes — four times the tile's 2,048-byte raw4
/// image: the bomb the decoder must refuse *before* it allocates, because the only legal output
/// size is `tile_edge^2 / 2`.
pub fn invalid_deflate_bomb() -> Vec<u8> {
    let bytes = deflate_tile();
    let h = header(&bytes);
    let payload = deflate(&vec![0u8; h.tile_cells() * 2]);
    assert!(payload.len() < h.tile_cells() / 2, "the bomb must clear the pre-inflate ceiling");
    with_single_tile_payload(bytes, obcg::CODEC_DEFLATE4, &payload)
}

/// The mirror image: a stream that stops one byte short of the tile's raw4 image.
pub fn invalid_deflate_short_output() -> Vec<u8> {
    let bytes = deflate_tile();
    let h = header(&bytes);
    let payload = deflate(&vec![0u8; h.tile_cells() / 2 - 1]);
    with_single_tile_payload(bytes, obcg::CODEC_DEFLATE4, &payload)
}

/// A stream whose match distance reaches before the first byte of the tile's raw4 image: one
/// fixed-Huffman literal, then a length-127 match at distance 4 with only one byte of history.
/// RFC 1951 forbids it and §5 says so explicitly, because a decoder that zero-filled the
/// out-of-range distance instead of failing would decode *different cells* from these same bytes
/// — the one class of divergence a two-language format cannot tolerate.
pub fn invalid_deflate_back_reference() -> Vec<u8> {
    with_single_tile_payload(deflate_tile(), obcg::CODEC_DEFLATE4, &[0x63, 0x18, 0x60, 0x0C, 0x00])
}

/// A perfectly valid codec-2 stream of the `rle_wins` cells — 46 bytes against RLE4's 32. §5
/// says codec 2 is legal only where it is strictly smaller than the canonical raw4/RLE4 length,
/// so this is the vector that keeps a producer from compressing tiles it should have run-length
/// encoded.
pub fn invalid_deflate_noncanonical() -> Vec<u8> {
    let cells: Vec<u8> = (0..256).map(|index| ((index / 8) % 13) as u8).collect();
    let payload = deflate(&pack_raw4(&cells));
    assert!(payload.len() > 32, "deflate must lose to RLE4 on these cells");
    with_single_tile_payload(rle_wins(), obcg::CODEC_DEFLATE4, &payload)
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

/// A dry sentinel used for a partial edge tile: forbidden by §4.1 because edge padding is
/// no-data, never dry. Built by rewriting an 8 x 8 single-partial-tile object as if the producer
/// had emitted the sentinel — every CRC is honest, so only the edge rule can reject it.
pub fn invalid_dry_sentinel_edge_tile() -> Vec<u8> {
    let mut bytes = radar_frame(8, 8, 16, 4, &[0u8; 64]);
    let h = header(&bytes);
    bytes.truncate(128 + h.page_bytes() as usize);
    let entry = entry_offset(&h, 0);
    bytes[entry..entry + obcg::DIRECTORY_ENTRY_LEN].fill(0);
    put_u32(&mut bytes, obcg::HDR_DATA_LEN, 0);
    let total_len = bytes.len() as u32;
    put_u32(&mut bytes, obcg::HDR_TOTAL_LEN, total_len);
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
        ("grid-rle-wins.obcg", rle_wins()),
        ("grid-deflate-tile.obcg", deflate_tile()),
        ("grid-deflate-padding-bits.obcg", deflate_padding_bits()),
        ("grid-deflate-edge256.obcg", deflate_edge256()),
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
        ("grid-invalid-codec-id.obcg", invalid_codec_id()),
        ("grid-invalid-rle-overlong.obcg", invalid_rle_overlong()),
        ("grid-invalid-rle-noncanonical.obcg", invalid_rle_noncanonical()),
        ("grid-invalid-raw-compressible.obcg", invalid_raw_compressible()),
        ("grid-invalid-deflate-truncated.obcg", invalid_deflate_truncated()),
        ("grid-invalid-deflate-bomb.obcg", invalid_deflate_bomb()),
        ("grid-invalid-deflate-short-output.obcg", invalid_deflate_short_output()),
        ("grid-invalid-deflate-back-reference.obcg", invalid_deflate_back_reference()),
        ("grid-invalid-deflate-noncanonical.obcg", invalid_deflate_noncanonical()),
        ("grid-invalid-dry-encoded.obcg", invalid_dry_encoded()),
        ("grid-invalid-dry-sentinel-nonzero.obcg", invalid_dry_sentinel_nonzero()),
        ("grid-invalid-dry-sentinel-edge-tile.obcg", invalid_dry_sentinel_edge_tile()),
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
