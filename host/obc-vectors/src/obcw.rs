//! Deterministic OBCW v1 fixtures for `specs/vectors/`.
//!
//! Inputs are provider-neutral semantic grids. Encoding goes through the Rust byte authority;
//! Swift independently re-encodes the same decoded values and pins every resulting byte.

use obc_formats::obcw::{
    self, BundleInput, HourlyRecord, RainFrameInput, CONDITION_CLEAR, CONDITION_OVERCAST, CONDITION_RAIN,
    CONDITION_THUNDERSTORM, HOURLY_COUNT, INTENSITY_NODATA, QUALITY_FORECAST, QUALITY_OBSERVED, TILE_CELLS,
};

const GENERATED_AT: i64 = 1_800_000_000;
const VALID_UNTIL: i64 = GENERATED_AT + 24 * 3_600;
const SOUTH: i32 = 47_000_000;
const WEST: i32 = 7_000_000;
const NORTH: i32 = 48_000_000;
const EAST: i32 = 8_500_000;
/// Phone producer policy from OBCW_Spec.md §1, intentionally outside the format authority.
pub const PRODUCER_POLICY_MAX_LEN: usize = 64 * 1024;

fn hours(dry: bool) -> [HourlyRecord; HOURLY_COUNT] {
    core::array::from_fn(|i| HourlyRecord {
        valid_time_offset_s: i as u32 * 3_600,
        temperature_deci_c: 80 + i as i16 * 3,
        precipitation_tenth_mm: if dry { 0 } else { (i % 7) as u16 },
        precipitation_probability_pct: if dry { 0 } else { ((i * 9) % 101) as u8 },
        condition: if dry {
            CONDITION_CLEAR
        } else if i == 8 {
            CONDITION_THUNDERSTORM
        } else if i % 3 == 0 {
            CONDITION_RAIN
        } else {
            CONDITION_OVERCAST
        },
        wind_from_deg: ((i * 17) % 360) as u16,
        wind_speed_deci_ms: 25 + i as u16,
        wind_gust_deci_ms: 45 + i as u16,
        flags: 0,
    })
}

fn encode(hours: &[HourlyRecord; HOURLY_COUNT], frames: &[RainFrameInput<'_>], generation: u32) -> Vec<u8> {
    let input = BundleInput {
        generation,
        request_id: 0x1187_0000 | generation,
        generated_at: GENERATED_AT,
        valid_from: GENERATED_AT,
        valid_until: VALID_UNTIL,
        south_lat_udeg: SOUTH,
        west_lon_udeg: WEST,
        north_lat_udeg: NORTH,
        east_lon_udeg: EAST,
        grid_origin_lat_udeg: SOUTH,
        grid_origin_lon_udeg: WEST,
        flags: 0,
        hourly: hours,
        frames,
    };
    let mut bytes = vec![0u8; obcw::encoded_len(&input).expect("fixture length") as usize];
    let len = obcw::encode_format(&input, &mut bytes).expect("fixture encode");
    bytes.truncate(len);
    bytes
}

fn raw_tile(phase: usize) -> [u8; TILE_CELLS] {
    core::array::from_fn(|i| ((i + phase) % 13) as u8)
}

fn runs_tile(runs: usize, first: u8) -> [u8; TILE_CELLS] {
    assert!((16..=127).contains(&runs));
    let base = TILE_CELLS / runs;
    let longer = TILE_CELLS % runs;
    let mut cells = [0u8; TILE_CELLS];
    let mut cursor = 0usize;
    for run in 0..runs {
        let len = base + usize::from(run < longer);
        cells[cursor..cursor + len].fill((first + run as u8) % 13);
        cursor += len;
    }
    assert_eq!(cursor, TILE_CELLS);
    cells
}

pub fn minimal_dry() -> Vec<u8> {
    encode(&hours(true), &[], 1)
}

pub fn raw_tile_bundle() -> Vec<u8> {
    let tiles = [raw_tile(0)];
    let frames = [RainFrameInput {
        valid_at: GENERATED_AT,
        width: 16,
        height: 16,
        cell_size_m: 1_000,
        quality_flags: QUALITY_OBSERVED,
        tiles: &tiles,
    }];
    encode(&hours(false), &frames, 2)
}

pub fn rle_tile_bundle() -> Vec<u8> {
    let tiles = [[6u8; TILE_CELLS]];
    let frames = [RainFrameInput {
        valid_at: GENERATED_AT + 900,
        width: 16,
        height: 16,
        cell_size_m: 1_000,
        quality_flags: QUALITY_FORECAST,
        tiles: &tiles,
    }];
    encode(&hours(false), &frames, 3)
}

pub fn nodata_tile_bundle() -> Vec<u8> {
    let tiles = [[INTENSITY_NODATA; TILE_CELLS]];
    let frames = [RainFrameInput {
        valid_at: GENERATED_AT,
        width: 16,
        height: 16,
        cell_size_m: 1_000,
        quality_flags: QUALITY_OBSERVED | obcw::QUALITY_PARTIAL_COVERAGE,
        tiles: &tiles,
    }];
    encode(&hours(false), &frames, 4)
}

pub fn coarse_model() -> Vec<u8> {
    let tiles0 = [[0u8; TILE_CELLS], [1u8; TILE_CELLS], [2u8; TILE_CELLS], [3u8; TILE_CELLS]];
    let tiles1 = [[2u8; TILE_CELLS], [3u8; TILE_CELLS], [4u8; TILE_CELLS], [5u8; TILE_CELLS]];
    let frames = [
        RainFrameInput {
            valid_at: GENERATED_AT + 3_600,
            width: 32,
            height: 32,
            cell_size_m: 25_000,
            quality_flags: QUALITY_FORECAST | obcw::QUALITY_DEGRADED,
            tiles: &tiles0,
        },
        RainFrameInput {
            valid_at: GENERATED_AT + 10_800,
            width: 32,
            height: 32,
            cell_size_m: 25_000,
            quality_flags: QUALITY_FORECAST | obcw::QUALITY_DEGRADED,
            tiles: &tiles1,
        },
    ];
    encode(&hours(false), &frames, 5)
}

pub fn dwd_shaped() -> Vec<u8> {
    let tiles: Vec<[u8; TILE_CELLS]> = (0..36).map(raw_tile).collect();
    let frames: Vec<RainFrameInput<'_>> = (0..9)
        .map(|i| RainFrameInput {
            valid_at: GENERATED_AT + i * 900,
            width: 96,
            height: 96,
            cell_size_m: 1_000,
            quality_flags: if i == 0 { QUALITY_OBSERVED } else { QUALITY_FORECAST },
            tiles: &tiles,
        })
        .collect();
    let bytes = encode(&hours(false), &frames, 6);
    assert_eq!(bytes.len(), 46_480, "locked DWD raw4 budget");
    bytes
}

pub fn maximum_policy() -> Vec<u8> {
    let mut tiles: Vec<[u8; TILE_CELLS]> = (0..462).map(raw_tile).collect();
    tiles.push(runs_tile(108, 0));
    let frames = [RainFrameInput {
        valid_at: GENERATED_AT + 900,
        width: 463 * 16,
        height: 16,
        cell_size_m: 1_000,
        quality_flags: QUALITY_FORECAST,
        tiles: &tiles,
    }];
    let bytes = encode(&hours(false), &frames, 7);
    assert_eq!(bytes.len(), PRODUCER_POLICY_MAX_LEN);
    bytes
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_i64(bytes: &mut [u8], offset: usize, value: i64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn rd_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

fn refresh_crc(bytes: &mut [u8]) {
    put_u32(bytes, obcw::HDR_CRC32, 0);
    let crc = super::crc32(bytes);
    put_u32(bytes, obcw::HDR_CRC32, crc);
}

pub fn invalid_truncated() -> Vec<u8> {
    let mut bytes = raw_tile_bundle();
    bytes.pop();
    bytes
}

pub fn invalid_bad_offset() -> Vec<u8> {
    let mut bytes = minimal_dry();
    put_u32(&mut bytes, obcw::HDR_HOURLY_OFFSET, (obcw::HEADER_LEN - 1) as u32);
    refresh_crc(&mut bytes);
    bytes
}

pub fn invalid_overlap() -> Vec<u8> {
    let mut bytes = raw_tile_bundle();
    let descriptor = obcw::HEADER_LEN + obcw::HOURLY_COUNT * obcw::HOURLY_RECORD_LEN;
    let directory = rd_u32(&bytes, descriptor + obcw::FRAME_TILE_DIRECTORY_OFFSET);
    put_u32(&mut bytes, descriptor + obcw::FRAME_TILE_DATA_OFFSET, directory);
    refresh_crc(&mut bytes);
    bytes
}

pub fn invalid_nibble() -> Vec<u8> {
    let mut bytes = raw_tile_bundle();
    let descriptor = obcw::HEADER_LEN + obcw::HOURLY_COUNT * obcw::HOURLY_RECORD_LEN;
    let data = rd_u32(&bytes, descriptor + obcw::FRAME_TILE_DATA_OFFSET) as usize;
    bytes[data] = (bytes[data] & 0xF0) | 13;
    refresh_crc(&mut bytes);
    bytes
}

pub fn invalid_rle_overlong() -> Vec<u8> {
    // Use the 108-run max fixture: its fixed payload can be made 108 x 16 = 1,728 decoded cells
    // without changing any layout field, isolating the zip-bomb-style expansion error.
    let mut bytes = maximum_policy();
    let descriptor = obcw::HEADER_LEN + obcw::HOURLY_COUNT * obcw::HOURLY_RECORD_LEN;
    let directory = rd_u32(&bytes, descriptor + obcw::FRAME_TILE_DIRECTORY_OFFSET) as usize;
    let last_entry = directory + 462 * obcw::TILE_DIRECTORY_ENTRY_LEN;
    let data = rd_u32(&bytes, last_entry + obcw::TILE_DATA_OFFSET) as usize;
    let len = u16::from_le_bytes(bytes[last_entry + 4..last_entry + 6].try_into().unwrap()) as usize;
    for byte in &mut bytes[data..data + len] {
        *byte = 0xF6;
    }
    refresh_crc(&mut bytes);
    bytes
}

pub fn invalid_crc() -> Vec<u8> {
    let mut bytes = raw_tile_bundle();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    bytes
}

pub fn invalid_time_order() -> Vec<u8> {
    let mut bytes = dwd_shaped();
    let descriptors = obcw::HEADER_LEN + obcw::HOURLY_COUNT * obcw::HOURLY_RECORD_LEN;
    let first = i64::from_le_bytes(bytes[descriptors..descriptors + 8].try_into().unwrap());
    put_i64(&mut bytes, descriptors + obcw::FRAME_DESCRIPTOR_LEN, first);
    refresh_crc(&mut bytes);
    bytes
}

pub fn positives() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("weather-minimal-dry.obcw", minimal_dry()),
        ("weather-dwd-96x96-9f.obcw", dwd_shaped()),
        ("weather-coarse-model.obcw", coarse_model()),
        ("weather-nodata-tile.obcw", nodata_tile_bundle()),
        ("weather-raw-tile.obcw", raw_tile_bundle()),
        ("weather-rle-tile.obcw", rle_tile_bundle()),
        ("weather-max-policy.obcw", maximum_policy()),
    ]
}

pub fn negatives() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("weather-invalid-truncated.obcw", invalid_truncated()),
        ("weather-invalid-bad-offset.obcw", invalid_bad_offset()),
        ("weather-invalid-overlap.obcw", invalid_overlap()),
        ("weather-invalid-nibble.obcw", invalid_nibble()),
        ("weather-invalid-rle-overlong.obcw", invalid_rle_overlong()),
        ("weather-invalid-crc.obcw", invalid_crc()),
        ("weather-invalid-time-order.obcw", invalid_time_order()),
    ]
}

pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    let mut fixtures = positives();
    fixtures.extend(negatives());
    fixtures
}
