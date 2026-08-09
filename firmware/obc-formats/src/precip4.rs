//! Provider-neutral 4-bit precipitation intensity and canonical tile codec.
//!
//! OBCW and the future OBCG format share these exact thresholds and raw4/RLE4 bytes. Keeping
//! them below either container prevents the baker, phone, and firmware from growing subtly
//! different precipitation contracts.

pub const TILE_EDGE: usize = 16;
pub const TILE_CELLS: usize = TILE_EDGE * TILE_EDGE;
pub const RAW4_LEN: usize = TILE_CELLS / 2;

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

/// Return the deterministic encoded length after validating every intensity code.
pub fn encoded_tile_len(tile: &[u8; TILE_CELLS]) -> Result<usize, Error> {
    if tile.iter().any(|&value| !valid_intensity(value)) {
        return Err(Error::Intensity);
    }
    Ok(rle4_len(tile).min(RAW4_LEN))
}

/// Encode one tile using canonical raw4/RLE4 selection.
///
/// `out` may be a 128-byte scratch buffer or an exact-size destination obtained from
/// [`encoded_tile_len`]. Only the returned prefix is written.
pub fn encode_tile(tile: &[u8; TILE_CELLS], out: &mut [u8]) -> Result<TileEncoding, Error> {
    let encoded_len = encoded_tile_len(tile)?;
    if out.len() < encoded_len {
        return Err(Error::OutputTooSmall);
    }
    let codec = if encoded_len < RAW4_LEN { CODEC_RLE4 } else { CODEC_RAW4 };
    if codec == CODEC_RLE4 {
        encode_rle4(tile, &mut out[..encoded_len]);
    } else {
        encode_raw4(tile, &mut out[..RAW4_LEN]);
    }
    Ok(TileEncoding { codec, encoded_len: encoded_len as u16 })
}

/// Validate a canonical encoded tile without expanding it.
pub fn validate_tile(codec: u8, encoded: &[u8]) -> Result<(), Error> {
    let len = encoded.len();
    if len == 0 || len > RAW4_LEN {
        return Err(Error::EncodedLength);
    }
    match codec {
        CODEC_RAW4 if len == RAW4_LEN => {
            if encoded.iter().any(|byte| !valid_intensity(byte & 0x0F) || !valid_intensity(byte >> 4)) {
                return Err(Error::Intensity);
            }
            if raw4_canonical_rle_len(encoded) < RAW4_LEN {
                return Err(Error::Codec);
            }
        }
        CODEC_RLE4 if len < RAW4_LEN => {
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
                if count > TILE_CELLS {
                    return Err(Error::Rle);
                }
                previous = Some((value, run));
            }
            if count != TILE_CELLS {
                return Err(Error::Rle);
            }
        }
        _ => return Err(Error::Codec),
    }
    Ok(())
}

/// Decode exactly one canonical tile into caller-owned storage.
pub fn decode_tile(codec: u8, encoded: &[u8], out: &mut [u8; TILE_CELLS]) -> Result<(), Error> {
    validate_tile(codec, encoded)?;
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

/// Count the maximal, 16-cell-capped runs represented by a valid raw4 payload without expanding
/// the tile. The result is the canonical RLE4 byte length.
fn raw4_canonical_rle_len(encoded: &[u8]) -> usize {
    let mut runs = 0usize;
    let mut previous = None;
    let mut run_len = 0usize;
    for cell_index in 0..TILE_CELLS {
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
