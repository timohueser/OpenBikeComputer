//! Irregular-frame-table freshness (WX10) — adopted from PR #1213's adversarial review, which
//! demonstrated the earlier last-frame-only cadence rule fail-open on two shapes: a mid-table bake
//! gap serving a 10.9 h-stale frame as current, and a gap-inflated last-two spacing extending the
//! last frame's life 54×. `current_frame` now bounds **every** frame by
//! `min(minimum inter-frame spacing, FRAME_CURRENT_CAP_S)`; these tests pin the closed behavior.

use obc_formats::io::SliceSource;
use obc_formats::obcw::{
    encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, CONDITION_RAIN, HOURLY_COUNT,
    HOURLY_INTERVAL_SECONDS, QUALITY_FORECAST, TILE_CELLS,
};
use obc_weather::{WeatherCache, WeatherReader, FRAME_CURRENT_CAP_S};

const T0: i64 = 1_800_000_000;

fn bundle(frame_times: &[i64], valid_from: i64, valid_until: i64) -> Vec<u8> {
    let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|i| HourlyRecord {
        valid_time_offset_s: i as u32 * HOURLY_INTERVAL_SECONDS,
        temperature_deci_c: 140,
        precipitation_tenth_mm: 12,
        precipitation_probability_pct: 60,
        condition: CONDITION_RAIN,
        wind_from_deg: 240,
        wind_speed_deci_ms: 40,
        wind_gust_deci_ms: 70,
        flags: 0,
    });
    let tiles = vec![[7u8; TILE_CELLS]; 1];
    let frames: Vec<RainFrameInput<'_>> = frame_times
        .iter()
        .map(|&t| RainFrameInput {
            valid_at: t,
            width: 16,
            height: 16,
            cell_size_m: 1_000,
            quality_flags: QUALITY_FORECAST,
            tiles: &tiles,
        })
        .collect();
    let input = BundleInput {
        generation: 1,
        request_id: 1,
        generated_at: T0,
        valid_from,
        valid_until,
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
    let mut bytes = vec![0u8; encoded_len(&input).unwrap() as usize];
    let len = encode_format(&input, &mut bytes).unwrap();
    bytes.truncate(len);
    bytes
}

fn current_at(bytes: &[u8], now: i64) -> Option<(usize, i64)> {
    let source = SliceSource(bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    reader.current_frame(now, &mut cache).unwrap().map(|(i, f)| (i, f.valid_at))
}

/// A mid-sequence cadence gap: frames at T0, T0+900, T0+50_000 (an interrupted bake), bundle
/// window open until T0+86_400. Inside the gap the newest at-or-before frame is the T0+900 one —
/// the earlier rule served it 39,100 s stale; the per-frame cap (minimum spacing = 900 s) now
/// closes it at T0+900+900 exactly.
#[test]
fn mid_sequence_gap_must_not_serve_a_stale_frame_as_current() {
    let bytes = bundle(&[T0, T0 + 900, T0 + 50_000], T0, T0 + 86_400);
    assert_eq!(current_at(&bytes, T0 + 1_800), Some((1, T0 + 900)), "inside its cap the frame is current");
    assert!(current_at(&bytes, T0 + 1_801).is_none(), "one second past the cap the gap goes dark");
    assert!(current_at(&bytes, T0 + 40_000).is_none(), "deep inside the gap nothing is current");
    assert_eq!(current_at(&bytes, T0 + 50_000), Some((2, T0 + 50_000)), "the late frame resumes at its own time");
}

/// The gap-inflated last-two spacing must not extend the last frame's life: with frames at T0,
/// T0+900, T0+50_000, the product's cadence is 900 s (the minimum spacing), so the last frame goes
/// dark 900 s after its timestamp — not the 49,100 s the last-two spacing would have granted.
#[test]
fn last_frame_cadence_measured_from_a_gap_must_not_extend_staleness() {
    let bytes = bundle(&[T0, T0 + 900, T0 + 50_000], T0, T0 + 200_000);
    assert_eq!(current_at(&bytes, T0 + 50_900), Some((2, T0 + 50_000)));
    assert!(current_at(&bytes, T0 + 50_901).is_none(), "the minimum spacing bounds the last frame");
    assert!(current_at(&bytes, T0 + 50_000 + 49_100).is_none(), "the gap-derived spacing grants nothing");
}

/// A coarse product (spacing above the ceiling) is bounded by [`FRAME_CURRENT_CAP_S`]: every frame
/// goes dark `FRAME_CURRENT_CAP_S` after its own timestamp, mid-table and last alike — honest
/// dark windows rather than a synthetic hour of "current" radar.
#[test]
fn coarse_products_are_bounded_by_the_hard_cap() {
    let bytes = bundle(&[T0, T0 + 3_600, T0 + 7_200], T0, T0 + 86_400);
    assert_eq!(current_at(&bytes, T0 + FRAME_CURRENT_CAP_S), Some((0, T0)));
    assert!(current_at(&bytes, T0 + FRAME_CURRENT_CAP_S + 1).is_none(), "mid-table dark window");
    assert_eq!(current_at(&bytes, T0 + 3_600), Some((1, T0 + 3_600)), "the next frame resumes");
    assert!(current_at(&bytes, T0 + 7_200 + FRAME_CURRENT_CAP_S + 1).is_none(), "last frame capped too");
}

/// Window boundary sanity: before the first frame nothing is current even inside the bundle
/// window; frames strictly in the future are never served early.
#[test]
fn future_frames_and_pre_first_instants_stay_dark() {
    let bytes = bundle(&[T0 + 10_000, T0 + 10_900], T0, T0 + 86_400);
    assert!(current_at(&bytes, T0).is_none(), "window open, first frame future");
    assert!(current_at(&bytes, T0 + 9_999).is_none());
    assert_eq!(current_at(&bytes, T0 + 10_000), Some((0, T0 + 10_000)));
}

/// Zero/negative spacing is unreachable through validated bytes — the encoder refuses
/// non-increasing frame timestamps, so `current_frame`'s "every spacing is positive" premise is a
/// checked invariant, not an assumption.
#[test]
fn encoder_rejects_non_increasing_frames() {
    let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|i| HourlyRecord {
        valid_time_offset_s: i as u32 * HOURLY_INTERVAL_SECONDS,
        temperature_deci_c: 140,
        precipitation_tenth_mm: 12,
        precipitation_probability_pct: 60,
        condition: CONDITION_RAIN,
        wind_from_deg: 240,
        wind_speed_deci_ms: 40,
        wind_gust_deci_ms: 70,
        flags: 0,
    });
    let tiles = vec![[7u8; TILE_CELLS]; 1];
    let frame = |t: i64| RainFrameInput {
        valid_at: t,
        width: 16,
        height: 16,
        cell_size_m: 1_000,
        quality_flags: QUALITY_FORECAST,
        tiles: &tiles,
    };
    let frames = [frame(T0 + 900), frame(T0 + 900)];
    let input = BundleInput {
        generation: 1,
        request_id: 1,
        generated_at: T0,
        valid_from: T0,
        valid_until: T0 + 86_400,
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
    let rejected_by_encoder = encoded_len(&input).is_err() || {
        let mut bytes = vec![0u8; encoded_len(&input).unwrap() as usize];
        encode_format(&input, &mut bytes).is_err()
    };
    if !rejected_by_encoder {
        // Encoder permissive? Then the *reader* must refuse the bytes (validation is the gate the
        // device actually runs).
        let mut bytes = vec![0u8; encoded_len(&input).unwrap() as usize];
        let len = encode_format(&input, &mut bytes).unwrap();
        bytes.truncate(len);
        assert!(WeatherReader::open(&SliceSource(&bytes)).is_err(), "non-increasing frames must not validate");
    }
}
