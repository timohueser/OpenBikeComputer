//! WX10 wiring test: the rain overlay lease flows host → `App::render_frame_with_rain` → the Map
//! screen → `render_rain_timed`, through the production [`RainOverlayAdapter`] over real OBCW
//! bytes. Pins the acceptance guarantees at the app level: rain renders when (and only when) a
//! frame is current, the rider marker stays above it, and an absent / expired / dry lease is
//! byte-identical to the plain rain-free frame.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState, RainOverlayAdapter};
use obc_formats::io::SliceSource;
use obc_formats::obcw::{
    self, encode_format, encoded_len, BundleInput, HourlyRecord, RainFrameInput, CONDITION_CLEAR, HOURLY_COUNT,
    QUALITY_FORECAST, TILE_CELLS,
};
use obc_ports::{Fix, RideClock, Sensors};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource as MapSliceSource};
use obc_render::{rain_style, RainOverlaySource};
use obc_weather::{WeatherCache, WeatherReader};

mod common;
use common::{build_min_obcm, Buf, ReplayFix};

/// The fixture's one frame timestamp.
const FRAME_AT: i64 = 1_800_000_900;

/// A valid single-frame OBCW bundle whose 16 × 16-cell grid covers the test map's origin, every
/// cell at `intensity`.
fn bundle(intensity: u8) -> Vec<u8> {
    let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|index| HourlyRecord {
        valid_time_offset_s: index as u32 * obcw::HOURLY_INTERVAL_SECONDS,
        temperature_deci_c: 120,
        precipitation_tenth_mm: 0,
        precipitation_probability_pct: 0,
        condition: CONDITION_CLEAR,
        wind_from_deg: 225,
        wind_speed_deci_ms: 40,
        wind_gust_deci_ms: 60,
        flags: 0,
    });
    let tiles = [[intensity; TILE_CELLS]];
    let frames = [RainFrameInput {
        valid_at: FRAME_AT,
        width: 16,
        height: 16,
        cell_size_m: 1_000,
        quality_flags: QUALITY_FORECAST,
        tiles: &tiles,
    }];
    let input = BundleInput {
        generation: 1,
        request_id: 7,
        generated_at: 1_800_000_000,
        valid_from: 1_800_000_000,
        valid_until: 1_800_100_000,
        south_lat_udeg: -500_000,
        west_lon_udeg: -500_000,
        north_lat_udeg: 500_000,
        east_lon_udeg: 500_000,
        grid_origin_lat_udeg: -500_000,
        grid_origin_lon_udeg: -500_000,
        flags: 0,
        hourly: &hourly,
        frames: &frames,
    };
    let mut bytes = vec![0; encoded_len(&input).unwrap() as usize];
    encode_format(&input, &mut bytes).unwrap();
    bytes
}

fn rgb888(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// Render one frame with an optional rain lease against the minimal sea-backdrop map.
fn render(app: &mut App, map: &[u8], rain: Option<&mut dyn RainOverlaySource>) -> Buf {
    let cache = MapCache::new();
    let src = MapSliceSource(map);
    let tables = MapTables::parse(&src).expect("valid map");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(120, 120);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame_with_rain(Some(&mut scratch), &mut buf, &reader, None, rain, 120.0, 120.0, rgb888);
    buf
}

#[test]
fn current_rain_renders_and_the_rider_stays_above_it() {
    const MARKER_565: u16 = 0xF800;
    let map = build_min_obcm(MARKER_565);
    let obcw = bundle(12); // torrential: coverage 16/16, so the whole in-grid view is painted
    let source = SliceSource(&obcw);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();

    let mut app = App::new(AppState::new(0, 0, 0.05));
    app.tick(RideClock(0), Sensors::new(&mut ReplayFix(Some(Fix::at(0, 0)))), None);

    let dry = render(&mut app, &map, None);
    let mut adapter = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).expect("frame is current");
    let rained = render(&mut app, &map, Some(&mut adapter));

    let rain_color = rgb888(rain_style(12).0);
    let sea = rgb888(0x001F);
    assert!(dry.count(rain_color) == 0 && dry.count(sea) > 0);
    assert!(rained.count(rain_color) > 0, "current rain must render");
    assert_eq!(rained.count(sea), 0, "full coverage covers the whole in-grid backdrop");
    // The rider marker draws above rain: same marker pixels as the dry frame.
    let marker = rgb888(MARKER_565);
    assert!(dry.count(marker) > 0, "the fix draws a marker");
    assert_eq!(rained.count(marker), dry.count(marker), "the marker never loses pixels to rain");
}

#[test]
fn expired_dry_and_absent_leases_are_byte_identical() {
    let map = build_min_obcm(0);
    let mut app = App::new(AppState::new(0, 0, 0.05));
    let plain = render(&mut app, &map, None);

    // Expired (or not-yet-valid) bundle: no adapter exists at all, so the host passes `None`; the
    // gate is `current_frame`, exercised here through the adapter's constructor.
    let obcw = bundle(12);
    let source = SliceSource(&obcw);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    assert!(RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT + 100_000).is_none());
    assert!(RainOverlayAdapter::current(&reader, &mut cache, 1_799_999_999).is_none());

    // A genuinely dry current frame: the lease exists and changes nothing.
    let dry_obcw = bundle(0);
    let dry_source = SliceSource(&dry_obcw);
    let dry_reader = WeatherReader::open(&dry_source).unwrap();
    let mut dry_cache = WeatherCache::new();
    let mut adapter = RainOverlayAdapter::current(&dry_reader, &mut dry_cache, FRAME_AT).unwrap();
    let rained = render(&mut app, &map, Some(&mut adapter));
    assert_eq!(plain.px, rained.px, "a dry frame is byte-identical to no lease at all");
}

#[test]
fn rendering_with_a_lease_is_deterministic() {
    let map = build_min_obcm(0);
    let obcw = bundle(5);
    let source = SliceSource(&obcw);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let mut app = App::new(AppState::new(0, 0, 0.05));

    let mut adapter_a = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).unwrap();
    let a = render(&mut app, &map, Some(&mut adapter_a));
    let mut adapter_b = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).unwrap();
    let b = render(&mut app, &map, Some(&mut adapter_b));
    assert_eq!(a.px, b.px, "same bundle, same instant, same bytes");
    let rain_color = rgb888(rain_style(5).0);
    assert!(a.count(rain_color) > 0, "moderate rain paints");
    // Coverage 10/16, verified exactly on a chrome-free Bayer-aligned 4×4 block (map chrome — the
    // start hint, clock, scale bar — legitimately paints over rain elsewhere on the frame).
    let block_hits =
        (40..44).flat_map(|y| (100..104).map(move |x| (x, y))).filter(|&(x, y)| a.get(x, y) == rain_color).count();
    assert_eq!(block_hits, 10, "intensity 5 paints 10 of every 16 Bayer cells");
}
