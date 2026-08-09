//! Provider-neutral 4-bit precipitation intensity and canonical tile codec.
//!
//! OBCW and OBCG share these exact thresholds and raw4/RLE4 bytes. Keeping them below either
//! container prevents the baker, phone, and firmware from growing subtly different
//! precipitation contracts. OBCW always uses the fixed 16 x 16 tile; OBCG chooses a per-product
//! power-of-two tile edge, so the codec is generalized over the decoded cell count with the
//! 256-cell entry points kept as exact wrappers.

pub const TILE_EDGE: usize = 16;
pub const TILE_CELLS: usize = TILE_EDGE * TILE_EDGE;
pub const RAW4_LEN: usize = TILE_CELLS / 2;
/// Largest cell count the generalized codec accepts: a 256 x 256-cell OBCG tile. Its raw4
/// payload is 32,768 bytes, so every canonical encoded length still fits `u16`.
pub const MAX_CELLS: usize = 256 * 256;

pub const CODEC_RAW4: u8 = 0;
pub const CODEC_RLE4: u8 = 1;

/// Dry/transparent. This is real zero precipitation, never missing data.
pub const INTENSITY_DRY: u8 = 0;
/// Highest defined precipitation band (`>= 50 mm/h`).
pub const INTENSITY_MAX: u8 = 12;
/// `13` and `14` are reserved so corrupt nibbles can be rejected.
pub const INTENSITY_NODATA: u8 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    OutputTooSmall,
    EncodedLength,
    Codec,
    Intensity,
    Rle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEncoding {
    pub codec: u8,
    pub encoded_len: u16,
}

/// Quantize a finite non-negative instantaneous precipitation rate in mm/h.
///
/// Negative and non-finite source values are unavailable, not dry. Exact lower bounds belong to
/// their new band, matching the normative OBCW table and the future OBCG contract.
pub fn quantize_rate_mm_per_hour(rate: f64) -> u8 {
    if !rate.is_finite() || rate < 0.0 {
        INTENSITY_NODATA
    } else if rate == 0.0 {
        INTENSITY_DRY
    } else if rate < 0.10 {
        1
    } else if rate < 0.25 {
        2
    } else if rate < 0.50 {
        3
    } else if rate < 1.00 {
        4
    } else if rate < 2.00 {
        5
    } else if rate < 4.00 {
        6
    } else if rate < 6.00 {
        7
    } else if rate < 10.00 {
        8
    } else if rate < 16.00 {
        9
    } else if rate < 25.00 {
        10
    } else if rate < 50.00 {
        11
    } else {
        INTENSITY_MAX
    }
}

pub const fn valid_intensity(value: u8) -> bool {
    value <= INTENSITY_MAX || value == INTENSITY_NODATA
}

/// True when `count` is a legal generalized cell count: even (raw4 packs two cells per byte),
/// nonzero, and no larger than [`MAX_CELLS`].
pub const fn valid_cell_count(count: usize) -> bool {
    count != 0 && count % 2 == 0 && count <= MAX_CELLS
}

/// Return the deterministic encoded length after validating every intensity code.
///
/// `cells.len()` is the decoded cell count and must satisfy [`valid_cell_count`]. OBCW always
/// passes 256; OBCG passes `tile_edge^2` for its per-product tile size.
pub fn encoded_cells_len(cells: &[u8]) -> Result<usize, Error> {
    if !valid_cell_count(cells.len()) {
        return Err(Error::EncodedLength);
    }
    if cells.iter().any(|&value| !valid_intensity(value)) {
        return Err(Error::Intensity);
    }
    Ok(rle4_len(cells).min(cells.len() / 2))
}

/// Return the deterministic encoded length of one 16 x 16 tile.
pub fn encoded_tile_len(tile: &[u8; TILE_CELLS]) -> Result<usize, Error> {
    encoded_cells_len(tile)
}

/// Encode one row-major cell block using canonical raw4/RLE4 selection.
///
/// `out` may be a `cells.len() / 2`-byte scratch buffer or an exact-size destination obtained
/// from [`encoded_cells_len`]. Only the returned prefix is written.
pub fn encode_cells(cells: &[u8], out: &mut [u8]) -> Result<TileEncoding, Error> {
    let encoded_len = encoded_cells_len(cells)?;
    if out.len() < encoded_len {
        return Err(Error::OutputTooSmall);
    }
    let raw4_len = cells.len() / 2;
    let codec = if encoded_len < raw4_len { CODEC_RLE4 } else { CODEC_RAW4 };
    if codec == CODEC_RLE4 {
        encode_rle4(cells, &mut out[..encoded_len]);
    } else {
        encode_raw4(cells, &mut out[..raw4_len]);
    }
    Ok(TileEncoding { codec, encoded_len: encoded_len as u16 })
}

/// Encode one 16 x 16 tile using canonical raw4/RLE4 selection.
pub fn encode_tile(tile: &[u8; TILE_CELLS], out: &mut [u8]) -> Result<TileEncoding, Error> {
    encode_cells(tile, out)
}

/// Validate a canonical encoded cell block without expanding it.
pub fn validate_cells(codec: u8, encoded: &[u8], cell_count: usize) -> Result<(), Error> {
    if !valid_cell_count(cell_count) {
        return Err(Error::EncodedLength);
    }
    let raw4_len = cell_count / 2;
    let len = encoded.len();
    if len == 0 || len > raw4_len {
        return Err(Error::EncodedLength);
    }
    match codec {
        CODEC_RAW4 if len == raw4_len => {
            if encoded.iter().any(|byte| !valid_intensity(byte & 0x0F) || !valid_intensity(byte >> 4)) {
                return Err(Error::Intensity);
            }
            if raw4_canonical_rle_len(encoded, cell_count) < raw4_len {
                return Err(Error::Codec);
            }
        }
        CODEC_RLE4 if len < raw4_len => {
            let mut count = 0usize;
            let mut previous: Option<(u8, usize)> = None;
            for &byte in encoded {
                let value = byte & 0x0F;
                let run = (byte >> 4) as usize + 1;
                if !valid_intensity(value) {
                    return Err(Error::Intensity);
                }
                if previous.is_some_and(|(previous_value, previous_run)| value == previous_value && previous_run != 16)
                {
                    return Err(Error::Rle);
                }
                count = count.checked_add(run).ok_or(Error::Rle)?;
                if count > cell_count {
                    return Err(Error::Rle);
                }
                previous = Some((value, run));
            }
            if count != cell_count {
                return Err(Error::Rle);
            }
        }
        _ => return Err(Error::Codec),
    }
    Ok(())
}

/// Validate a canonical encoded 16 x 16 tile without expanding it.
pub fn validate_tile(codec: u8, encoded: &[u8]) -> Result<(), Error> {
    validate_cells(codec, encoded, TILE_CELLS)
}

/// Decode exactly one canonical cell block into caller-owned storage. `out.len()` is the decoded
/// cell count.
pub fn decode_cells(codec: u8, encoded: &[u8], out: &mut [u8]) -> Result<(), Error> {
    validate_cells(codec, encoded, out.len())?;
    if codec == CODEC_RAW4 {
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

/// Decode exactly one canonical 16 x 16 tile into caller-owned storage.
pub fn decode_tile(codec: u8, encoded: &[u8], out: &mut [u8; TILE_CELLS]) -> Result<(), Error> {
    decode_cells(codec, encoded, out)
}

/// Count the maximal, 16-cell-capped runs represented by a valid raw4 payload without expanding
/// the block. The result is the canonical RLE4 byte length.
fn raw4_canonical_rle_len(encoded: &[u8], cell_count: usize) -> usize {
    let mut runs = 0usize;
    let mut previous = None;
    let mut run_len = 0usize;
    for cell_index in 0..cell_count {
        let byte = encoded[cell_index / 2];
        let value = if cell_index % 2 == 0 { byte & 0x0F } else { byte >> 4 };
        if previous == Some(value) && run_len < 16 {
            run_len += 1;
        } else {
            runs += 1;
            previous = Some(value);
            run_len = 1;
        }
    }
    runs
}

fn rle4_len(cells: &[u8]) -> usize {
    let mut runs = 0usize;
    let mut index = 0usize;
    while index < cells.len() {
        let value = cells[index];
        let mut run = 1usize;
        while index + run < cells.len() && run < 16 && cells[index + run] == value {
            run += 1;
        }
        runs += 1;
        index += run;
    }
    runs
}

fn encode_raw4(cells: &[u8], out: &mut [u8]) {
    for index in 0..cells.len() / 2 {
        out[index] = cells[index * 2] | (cells[index * 2 + 1] << 4);
    }
}

fn encode_rle4(cells: &[u8], out: &mut [u8]) {
    let mut input = 0usize;
    let mut output = 0usize;
    while input < cells.len() {
        let value = cells[input];
        let mut run = 1usize;
        while input + run < cells.len() && run < 16 && cells[input + run] == value {
            run += 1;
        }
        out[output] = ((run as u8 - 1) << 4) | value;
        input += run;
        output += 1;
    }
}

const _: () = assert!(TILE_CELLS == 256);
const _: () = assert!(RAW4_LEN == 128);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantization_boundaries_are_exact_and_missing_never_becomes_dry() {
        for (rate, expected) in [
            (0.0, 0),
            (f64::from_bits(1), 1),
            (0.099_999, 1),
            (0.10, 2),
            (0.249_999, 2),
            (0.25, 3),
            (0.50, 4),
            (1.0, 5),
            (2.0, 6),
            (4.0, 7),
            (6.0, 8),
            (10.0, 9),
            (16.0, 10),
            (25.0, 11),
            (50.0, 12),
            (500.0, 12),
        ] {
            assert_eq!(quantize_rate_mm_per_hour(rate), expected, "rate={rate}");
        }
        for rate in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(quantize_rate_mm_per_hour(rate), INTENSITY_NODATA);
        }
    }

    #[test]
    fn canonical_raw_and_rle_round_trip() {
        let raw: [u8; TILE_CELLS] = core::array::from_fn(|index| (index % 13) as u8);
        let compressed = [6u8; TILE_CELLS];
        for (tile, expected_codec, expected_len) in [(&raw, CODEC_RAW4, RAW4_LEN), (&compressed, CODEC_RLE4, 16)] {
            let mut encoded = [0u8; RAW4_LEN];
            let encoding = encode_tile(tile, &mut encoded).unwrap();
            assert_eq!(encoding.codec, expected_codec);
            assert_eq!(encoding.encoded_len as usize, expected_len);
            let payload = &encoded[..expected_len];
            assert_eq!(validate_tile(expected_codec, payload), Ok(()));
            let mut decoded = [0u8; TILE_CELLS];
            decode_tile(expected_codec, payload, &mut decoded).unwrap();
            assert_eq!(&decoded, tile);
        }
    }

    #[test]
    fn malformed_tiles_fail_closed() {
        assert_eq!(encoded_tile_len(&[13u8; TILE_CELLS]), Err(Error::Intensity));
        assert_eq!(validate_tile(CODEC_RAW4, &[0u8; RAW4_LEN]), Err(Error::Codec));
        assert_eq!(validate_tile(CODEC_RLE4, &[0xF6; 15]), Err(Error::Rle));
        assert_eq!(validate_tile(CODEC_RLE4, &[0xF6; 17]), Err(Error::Rle));
        assert_eq!(validate_tile(2, &[0; RAW4_LEN]), Err(Error::Codec));
    }

    #[test]
    fn generalized_cell_counts_round_trip_and_reject_bad_sizes() {
        // OBCG's per-product tile sizes: 32 x 32 and 64 x 64 blocks through the same authority.
        for cell_count in [1_024usize, 4_096] {
            let raw: std::vec::Vec<u8> = (0..cell_count).map(|index| (index % 13) as u8).collect();
            let uniform = std::vec![6u8; cell_count];
            for (cells, expected_codec, expected_len) in
                [(&raw, CODEC_RAW4, cell_count / 2), (&uniform, CODEC_RLE4, cell_count / 16)]
            {
                let mut encoded = std::vec![0u8; cell_count / 2];
                let encoding = encode_cells(cells, &mut encoded).unwrap();
                assert_eq!(encoding.codec, expected_codec);
                assert_eq!(encoding.encoded_len as usize, expected_len);
                let payload = &encoded[..expected_len];
                assert_eq!(validate_cells(expected_codec, payload, cell_count), Ok(()));
                let mut decoded = std::vec![0u8; cell_count];
                decode_cells(expected_codec, payload, &mut decoded).unwrap();
                assert_eq!(&decoded, cells);
            }
        }
        // A 256-cell payload is not valid against a different declared cell count.
        let uniform = [6u8; TILE_CELLS];
        let mut encoded = [0u8; RAW4_LEN];
        let encoding = encode_cells(&uniform, &mut encoded).unwrap();
        let payload = &encoded[..encoding.encoded_len as usize];
        assert_eq!(validate_cells(encoding.codec, payload, 1_024), Err(Error::Rle));
        // Odd, zero, and oversized cell counts fail closed.
        assert_eq!(encoded_cells_len(&[0u8; 15]), Err(Error::EncodedLength));
        assert_eq!(encoded_cells_len(&[]), Err(Error::EncodedLength));
        assert_eq!(validate_cells(CODEC_RAW4, &[0u8; 4], 0), Err(Error::EncodedLength));
        assert!(!valid_cell_count(MAX_CELLS + 2));
        assert!(valid_cell_count(MAX_CELLS));
    }

    #[test]
    fn deterministic_valid_tiles_always_round_trip() {
        let mut state = 0x1187_0BC5u32;
        for _ in 0..1_024 {
            let mut tile = [0u8; TILE_CELLS];
            for value in &mut tile {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let candidate = (state & 0x0F) as u8;
                *value = if valid_intensity(candidate) { candidate } else { INTENSITY_NODATA };
            }
            let mut encoded = [0u8; RAW4_LEN];
            let encoding = encode_tile(&tile, &mut encoded).unwrap();
            let mut decoded = [0u8; TILE_CELLS];
            decode_tile(encoding.codec, &encoded[..encoding.encoded_len as usize], &mut decoded).unwrap();
            assert_eq!(decoded, tile);
        }
    }
}
