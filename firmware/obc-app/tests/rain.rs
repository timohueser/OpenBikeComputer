//! WX10 wiring test: the rain overlay lease flows host → `App::render_frame_with_rain` → the
//! **rain map** screen (WX11) → `render_rain_timed`, through the production [`RainOverlayAdapter`]
//! over real OBCW bytes. Pins the acceptance guarantees at the app level: rain renders when (and
//! only when) a frame is current, the rider marker stays above it, a dry lease paints nothing —
//! and the raster belongs to the screen that asked for it, so the ordinary Map is rain-free even
//! while the host leases weather every frame.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState, Gesture, RainOverlayAdapter, Screen};
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
use common::{build_min_obcm, weather_pass, weather_snapshot, Buf, ReplayFix};

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

/// Walk the rider's own route from the Map to the **rain map**, through the production gestures:
/// the Map's ride menu → Main menu → the Weather station → the dashboard's RAIN MAP row. The Map
/// stays on the stack underneath, so [`walk_back_to_the_map`] returns to the very same screen the
/// rider left — the sequence the state leak was reported on.
fn walk_to_the_rain_map(app: &mut App) {
    app.apply_gesture(Gesture::BackHold); // Map → ride menu
    app.apply_gesture(Gesture::Step(-1)); // → its Main menu station
    app.apply_gesture(Gesture::Press); // → Menu
    for _ in 0..4 {
        app.apply_gesture(Gesture::Step(1)); // → the Weather station
    }
    app.apply_gesture(Gesture::Press); // → the dashboard
    app.apply_gesture(Gesture::Step(1)); // → its RAIN MAP row
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)), "the dashboard's second action is the rain map");
}

/// Back out of the rain map to the Map the walk started from (rain map → dashboard → Menu → ride
/// menu → Map).
fn walk_back_to_the_map(app: &mut App) {
    for _ in 0..4 {
        app.apply_gesture(Gesture::Back);
    }
    assert!(matches!(app.top_screen(), Screen::Map(_)), "four Backs land on the Map again");
}

/// Render one frame with an optional rain lease against the minimal sea-backdrop map.
fn render(app: &mut App, map: &[u8], rain: Option<&mut dyn RainOverlaySource>) -> Buf {
    let cache = MapCache::new();
    let src = MapSliceSource(map);
    let tables = MapTables::parse(&src).expect("valid map");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(120, 120);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame_with_rain(Some(&mut scratch), &mut buf, &reader, None, rain, None, 120.0, 120.0, rgb888);
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
    walk_to_the_rain_map(&mut app);

    // The rain-free reference is the *same* screen holding a genuinely dry lease — not a leaseless
    // frame, which would honestly declare itself with WX11's "no data" banner and so differ in
    // chrome as well as in rain.
    let dry_obcw = bundle(0);
    let dry_source = SliceSource(&dry_obcw);
    let dry_reader = WeatherReader::open(&dry_source).unwrap();
    let mut dry_cache = WeatherCache::new();
    let mut dry_adapter = RainOverlayAdapter::current(&dry_reader, &mut dry_cache, FRAME_AT).unwrap();
    let dry = render(&mut app, &map, Some(&mut dry_adapter));
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
fn expired_leases_never_exist_and_a_dry_one_paints_nothing() {
    let map = build_min_obcm(0);
    let mut app = App::new(AppState::new(0, 0, 0.05));
    walk_to_the_rain_map(&mut app);

    // Expired (or not-yet-valid) bundle: no adapter exists at all, so the host passes `None`; the
    // gate is `current_frame`, exercised here through the adapter's constructor.
    let obcw = bundle(12);
    let source = SliceSource(&obcw);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    assert!(RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT + 100_000).is_none());
    assert!(RainOverlayAdapter::current(&reader, &mut cache, 1_799_999_999).is_none());

    // A genuinely dry current frame: the lease exists and paints no rain at any intensity, while
    // the same walk with the torrential bundle paints plenty.
    let dry_obcw = bundle(0);
    let dry_source = SliceSource(&dry_obcw);
    let dry_reader = WeatherReader::open(&dry_source).unwrap();
    let mut dry_cache = WeatherCache::new();
    let mut adapter = RainOverlayAdapter::current(&dry_reader, &mut dry_cache, FRAME_AT).unwrap();
    let dry = render(&mut app, &map, Some(&mut adapter));
    for intensity in 1..=15u8 {
        assert_eq!(dry.count(rgb888(rain_style(intensity).0)), 0, "a dry frame paints no intensity-{intensity} cell");
    }
    let mut wet = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).unwrap();
    let rained = render(&mut app, &map, Some(&mut wet));
    assert_ne!(dry.px, rained.px, "the same screen with rain in the bundle is a different frame");
}

#[test]
fn rendering_with_a_lease_is_deterministic() {
    let map = build_min_obcm(0);
    let obcw = bundle(5);
    let source = SliceSource(&obcw);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let mut app = App::new(AppState::new(0, 0, 0.05));
    walk_to_the_rain_map(&mut app);

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

/// The grid the sampled snapshot below carries: dense enough that the zoom floor the domain derives
/// from it sits well above `AppState::new`'s 0.05, so the rain map's entry clamp
/// (`AppState::clamp_rain_zoom`) genuinely moves the shared camera, exactly as it does on the
/// device. A frameless bundle would leave the floor at `0.0` and quietly turn the walk below into a
/// no-op.
const DENSE_GRID: obc_render::RainGrid = obc_render::RainGrid {
    west_udeg: -100_000,
    south_udeg: -100_000,
    east_udeg: 100_000,
    north_udeg: 100_000,
    width_cells: 4_096,
    height_cells: 4_096,
};

/// One pass with a four-frame bundle sampled — the production path (stage 10) for the rain map's
/// step range and zoom floor, which no host derives any more.
fn sample_dense(app: &mut App) {
    let snap = weather_snapshot(app.wall_unix_now() as i64, &[0; 4], Some(DENSE_GRID));
    weather_pass(app, 0, Some(&snap), |_| {});
    assert_eq!(app.weather().steps_ahead(), 3, "four frames, three of them ahead");
    assert!(app.weather().zoom_floor() > 0.05, "the fixture grid must engage the clamp");
}

/// The reported state leak (on-glass, simulator): after a visit to the rain map the precipitation
/// raster kept drawing on the ordinary Map. It cannot any more — the overlay is the base screen's
/// declared capability (`Caps::rain_overlay`), so a host that leases weather on *every* frame (both
/// production hosts do) still hands the Map nothing.
///
/// The walk runs through the **production derivation** (stage 10, once per pass on every host),
/// so the rain map's zoom clamp really fires. That is why the
/// claim pinned here is *rain-freeness*, not frame identity: the returned-to Map legitimately sits
/// at a different zoom, which is its own defect (#1252) and has its own case below. What must hold
/// at any camera is that the offered lease changes **nothing** on the Map — before or after.
#[test]
fn the_map_is_rain_free_before_and_after_a_visit_to_the_rain_map() {
    let map = build_min_obcm(0);
    let obcw = bundle(12); // torrential: on the rain map this covers the whole in-grid view
    let source = SliceSource(&obcw);
    let reader = WeatherReader::open(&source).unwrap();
    let mut cache = WeatherCache::new();
    let mut app = App::new(AppState::new(0, 0, 0.05));
    assert!(matches!(app.top_screen(), Screen::Map(_)), "the app starts on the Map");
    // The host's per-frame weather view feed: step range + the product's zoom floor.
    sample_dense(&mut app);

    // The Map with no weather mounted at all — the reference frame the rider expects to see.
    let pristine = render(&mut app, &map, None);
    // The Map with a current, heavy lease offered by the host: identical, to the byte.
    let mut offered = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).unwrap();
    let with_lease = render(&mut app, &map, Some(&mut offered));
    assert_eq!(with_lease.px, pristine.px, "an offered lease never reaches the Map");
    let rain_color = rgb888(rain_style(12).0);
    assert_eq!(with_lease.count(rain_color), 0, "the Map draws no rain");

    // The rain map is where that same lease does draw…
    walk_to_the_rain_map(&mut app);
    sample_dense(&mut app);
    let mut on_rain_map = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).unwrap();
    let rained = render(&mut app, &map, Some(&mut on_rain_map));
    assert!(rained.count(rain_color) > 0, "the rain map is the screen that draws rain");

    // …and the Map the rider comes back to carries none of it. Compared against the *same camera*
    // drawn with no lease at all — the clamp moved the zoom (#1252), so `pristine` is no longer the
    // right reference, but "the lease adds nothing" is still exactly the property under test, and
    // it is the one that fails on the old behaviour.
    walk_back_to_the_map(&mut app);
    sample_dense(&mut app);
    let mut still_offered = RainOverlayAdapter::current(&reader, &mut cache, FRAME_AT).unwrap();
    let after = render(&mut app, &map, Some(&mut still_offered));
    let after_without_lease = render(&mut app, &map, None);
    assert_eq!(after.px, after_without_lease.px, "the rain map's raster must not outlive the rain map");
    for intensity in 1..=15u8 {
        assert_eq!(after.count(rgb888(rain_style(intensity).0)), 0, "no intensity-{intensity} rain on the Map");
    }
}

/// The **camera** half of the same walk, which this PR does *not* fix: the rain map's entry clamp
/// (`AppState::clamp_rain_zoom`) writes the shared `AppState::zoom`, and nothing restores it, so a
/// rider who opens the rain map from a wider-out Map is silently returned to a Map at the radar
/// product's zoom floor. Kept as an executable statement of the defect and ignored until
/// <https://github.com/timohueser/OpenBikeComputer/issues/1252> decides what the camera should do
/// (restore it, or keep the shared camera and say so) — run with `cargo test -- --ignored` to
/// watch it fail.
#[test]
#[ignore = "#1252: the rain map's zoom clamp moves the shared camera and nothing restores it"]
fn the_map_returns_to_the_camera_the_rider_left() {
    let map = build_min_obcm(0);
    let mut app = App::new(AppState::new(0, 0, 0.05));
    sample_dense(&mut app);
    let pristine = render(&mut app, &map, None);
    walk_to_the_rain_map(&mut app);
    sample_dense(&mut app);
    walk_back_to_the_map(&mut app);
    sample_dense(&mut app);
    let after = render(&mut app, &map, None);
    // Compared as a bool, not with `assert_eq!`: a failing frame comparison would otherwise dump
    // two 120 × 120 pixel vectors into the log.
    assert!(after.px == pristine.px, "a rain-map visit must not re-aim the Map's camera");
}
