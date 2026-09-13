//! Hostile-shape pins for `rain_outlook` / `WeatherSnapshot` — adopted from the #1224
//! adversarial review's probe suite (all nine shapes held; committed so they keep holding):
//! expired-but-valid bundles, a 60-second correction frame collapsing the cap, exact-second
//! coverage boundaries, horizon-inclusive wet frames, NOW-vs-minutes rounding, a no-data current
//! cell with wet future frames, and snapshot↔reader currency parity on irregular spacing.

use obc_app::weather::{rain_outlook, FrameSample, RainOutlook, WeatherSnapshot};
use obc_formats::io::SliceSource;
use obc_formats::obcw::{
    encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, HOURLY_COUNT, INTENSITY_NODATA,
    QUALITY_FORECAST, TILE_CELLS,
};
use obc_weather::{WeatherCache, WeatherReader};

const T0: i64 = 1_800_000_000;

fn base() -> WeatherSnapshot {
    WeatherSnapshot {
        generated_at: T0,
        valid_from: T0 - 3_600,
        valid_until: T0 + 24 * 3_600,
        hourly: [HourlyRecord {
            valid_time_offset_s: 0,
            temperature_deci_c: 0,
            precipitation_tenth_mm: 0,
            precipitation_probability_pct: 0,
            condition: 0,
            wind_from_deg: 0,
            wind_speed_deci_ms: 0,
            wind_gust_deci_ms: 0,
            flags: 0,
        }; HOURLY_COUNT],
        frames: heapless::Vec::new(),
        frame_cap_s: 900,
        sampled_at: Some((0, 0)),
        pos_in_grid: true,
        current_pos_in_grid: true,
        projected: false,
        frames_truncated: false,
        rain_grid: None,
    }
}

fn with_frames(spacing_intensity: &[(i64, u8)], cap: i64) -> WeatherSnapshot {
    let mut s = base();
    for &(at, i) in spacing_intensity {
        s.frames
            .push(FrameSample {
                valid_at: T0 + at,
                intensity: i,
                lat: 0,
                lon: 0,
                past_route_end: false,
                spread_uncertain: false,
            })
            .unwrap();
    }
    s.frame_cap_s = cap;
    s
}

#[test]
fn expired_rain_keeps_valid_hourly_forecast() {
    let s = with_frames(&[(0, 0), (900, 0), (1800, 0)], 900);
    // now far past the last frame's cap but well inside valid_until
    let now = T0 + 10_000;
    assert!(now < s.valid_until);
    assert_eq!(rain_outlook(&s, now), RainOutlook::HourlyOnly);
    // Even if a frame was wet, expired wet frames must not claim rain either.
    let s = with_frames(&[(0, 9), (900, 9), (1800, 9)], 900);
    assert_eq!(rain_outlook(&s, now), RainOutlook::HourlyOnly, "expired storm never alerts");
}

#[test]
fn sixty_second_correction_frame_shrinks_the_cap_and_kills_dry() {
    // Frames every 900s but one 60s-spaced correction pair: global min spacing = 60.
    let intens: Vec<(i64, u8)> = vec![
        (0, 0),
        (60, 0), // the correction frame
        (900, 0),
        (1800, 0),
        (2700, 0),
        (3600, 0),
        (4500, 0),
        (5400, 0),
        (6300, 0),
        (7200, 0),
    ];
    let s = with_frames(&intens, 60);
    // With cap 60 the windows are 60s wide: coverage is full of holes -> never Dry.
    assert_eq!(rain_outlook(&s, T0), RainOutlook::Dry { minutes: 2 });
}

#[test]
fn dry_claim_boundary_at_the_last_covered_second() {
    // Nine 900s frames: window_end(last) = T0+8100. Dry only while now+7200 <= 8100.
    let dry: Vec<(i64, u8)> = (0..9).map(|i| (i * 900, 0)).collect();
    let s = with_frames(&dry, 900);
    assert_eq!(rain_outlook(&s, T0 + 900), RainOutlook::Dry { minutes: 120 }, "exactly 2h coverage left");
    assert_eq!(rain_outlook(&s, T0 + 901), RainOutlook::Dry { minutes: 119 }, "round down the remaining coverage");
}

#[test]
fn gap_exactly_at_the_two_hour_boundary() {
    // Coverage chain ends at T0+7100 (cap 500 on the last frame via spacing), horizon = T0+7200.
    let mut s = with_frames(&[(0, 0), (500, 0), (1000, 0)], 500);
    // Make a long chain of 500s-spaced dry frames up to 6600, window_end(last)=7100 < 7200.
    s.frames.clear();
    let mut at = 0;
    while at <= 6_600 {
        s.frames
            .push(FrameSample {
                valid_at: T0 + at,
                intensity: 0,
                lat: 0,
                lon: 0,
                past_route_end: false,
                spread_uncertain: false,
            })
            .unwrap();
        at += 500;
    }
    s.frame_cap_s = 500;
    assert_eq!(
        rain_outlook(&s, T0),
        RainOutlook::Dry { minutes: 116 },
        "the 7000-second chain ends before the horizon"
    );
}

#[test]
fn wet_exactly_at_the_horizon_counts_and_past_it_does_not() {
    // Frame at exactly now+7200 wet.
    let mut frames: Vec<(i64, u8)> = (0..8).map(|i| (i * 900, 0)).collect();
    frames.push((7200, 5));
    let s = with_frames(&frames, 900);
    assert_eq!(rain_outlook(&s, T0), RainOutlook::RainIn { minutes: 120 }, "horizon-inclusive");
    // Shift now back one second: the wet frame is past the horizon; coverage now ends at 8100 >= horizon.
    assert!(matches!(rain_outlook(&s, T0 - 1), RainOutlook::HourlyOnly));
}

#[test]
fn rain_now_vs_rain_in_one_second() {
    let mut frames: Vec<(i64, u8)> = (0..9).map(|i| (i * 900, 0)).collect();
    frames[0].1 = 3;
    let s = with_frames(&frames, 900);
    assert_eq!(rain_outlook(&s, T0), RainOutlook::RainIn { minutes: 0 }, "wet current frame = NOW");
    // A wet frame starting 1s from now: minutes truncates to 0 -> also NOW (conservative, earlier).
    let mut frames: Vec<(i64, u8)> = (0..9).map(|i| (i * 900, 0)).collect();
    frames[1].1 = 3;
    let s = with_frames(&frames, 900);
    assert_eq!(rain_outlook(&s, T0 + 899), RainOutlook::RainIn { minutes: 0 });
    assert_eq!(rain_outlook(&s, T0 + 900 - 60), RainOutlook::RainIn { minutes: 1 });
}

#[test]
fn wet_before_now_inside_current_window_reports_now() {
    // Frame wet, started 800s ago, still current (cap 900): 0 minutes.
    let s = with_frames(&[(0, 7), (900, 0), (1800, 0)], 900);
    assert_eq!(rain_outlook(&s, T0 + 800), RainOutlook::RainIn { minutes: 0 });
}

#[test]
fn nodata_current_with_wet_future_still_reports_the_rain() {
    // The sampled cell is NODATA for the current frame (rider under a data hole) but a future
    // frame is wet: the rain must still be reported (gaps suppress dry, never a warning).
    let s = with_frames(
        &[(0, INTENSITY_NODATA), (900, 6), (1800, 0), (2700, 0), (3600, 0), (4500, 0), (5400, 0), (6300, 0), (7200, 0)],
        900,
    );
    assert_eq!(rain_outlook(&s, T0), RainOutlook::RainIn { minutes: 15 });
}

/// Snapshot/reader currency parity on an IRREGULARLY spaced bundle (the committed pin only walks
/// the evenly-spaced DWD vector).
#[test]
fn snapshot_currency_matches_reader_on_irregular_spacing() {
    let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|i| HourlyRecord {
        valid_time_offset_s: i as u32 * 3_600,
        temperature_deci_c: 100,
        precipitation_tenth_mm: 0,
        precipitation_probability_pct: 0,
        condition: 0,
        wind_from_deg: 0,
        wind_speed_deci_ms: 0,
        wind_gust_deci_ms: 0,
        flags: 0,
    });
    // Irregular: 0, 60, 960, 2400, 2460, 4000 — min spacing 60.
    let offsets = [0i64, 60, 960, 2400, 2460, 4000];
    let tiles = vec![[1u8; TILE_CELLS]; 9]; // 48x48 grid = 9 tiles
    let frames: Vec<RainFrameInput<'_>> = offsets
        .iter()
        .map(|&o| RainFrameInput {
            valid_at: T0 + o,
            width: 48,
            height: 48,
            cell_size_m: 1_000,
            quality_flags: QUALITY_FORECAST,
            tiles: &tiles,
        })
        .collect();
    let input = BundleInput {
        generation: 1,
        request_id: 1,
        generated_at: T0,
        valid_from: T0 - 3_600,
        valid_until: T0 - 3_600 + 24 * 3_600,
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
    let n = encode_format(&input, &mut bytes).unwrap();
    bytes.truncate(n);
    let source = SliceSource(&bytes);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let snap = WeatherSnapshot::sample(&reader, &mut cache, Some((47_500_000, 7_500_000))).unwrap();
    assert_eq!(snap.frame_cap_s, 60, "global min spacing");
    for offset in -100..4_600 {
        let now = T0 + offset;
        let expect = reader.current_frame(now, &mut cache).unwrap().map(|(i, _)| i);
        assert_eq!(snap.current_frame_index(now), expect, "offset {offset}");
    }
}

#[test]
fn downloaded_forecast_remains_useful_through_an_offline_day() {
    let frames: Vec<_> = (0..9).map(|i| (i * 900, 0)).collect();
    let mut s = with_frames(&frames, 900);
    s.generated_at = T0 + 16 * 60;
    for (age_minutes, remaining) in [(16, 119), (40, 95), (120, 15), (134, 1)] {
        assert_eq!(rain_outlook(&s, T0 + age_minutes * 60), RainOutlook::Dry { minutes: remaining });
    }
    assert_eq!(rain_outlook(&s, T0 + 135 * 60), RainOutlook::HourlyOnly);
    assert!(s.hourly_at(T0 + 12 * 3600).is_some());
    assert_eq!(rain_outlook(&s, T0 + 12 * 3600), RainOutlook::HourlyOnly);
    // Reassembling the same forecast does not extend either coverage horizon.
    s.generated_at = T0 + 12 * 3600;
    assert_eq!(rain_outlook(&s, T0 + 12 * 3600), RainOutlook::HourlyOnly);
    assert_eq!(rain_outlook(&s, s.valid_from + 24 * 3600), RainOutlook::UpdateNeeded);
    s.frames.clear();
    assert_eq!(rain_outlook(&s, s.valid_until + 1), RainOutlook::UpdateNeeded);
}
