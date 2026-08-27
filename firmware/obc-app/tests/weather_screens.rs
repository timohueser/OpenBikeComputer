//! WX11 wiring tests: the weather screens end to end through the production `App` — navigation
//! from the Menu's Weather station, the host snapshot feed, the honest dashboard states, the
//! alert card's locked actions, and the rain map's time-step clamp.
//!
//! Full-frame RGB222 PNG pins per language are the epic-closeout sweep (`ui-snapshots.sh` +
//! the former design-review weather previews); here the pins are behavioral: deterministic frames,
//! per-language rendering, and stale-never-looks-dry at the frame level.

use embedded_graphics::pixelcolor::Rgb888;
use obc_app::{App, AppState, Gesture, Screen, WeatherAlertKind, WeatherSnapshot};
use obc_formats::obcw::{HourlyRecord, CONDITION_CLEAR, HOURLY_COUNT};
use obc_reader::{rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource as MapSliceSource};

mod common;
use common::{build_min_obcm, weather_pass, Buf};

const T0: i64 = 1_800_000_000;

/// A synthetic snapshot: nine 15-minute frames from [`T0`] with the given sampled intensities,
/// clear hourly records, valid for a day.
fn snapshot(intensities: &[u8]) -> WeatherSnapshot {
    let hourly: [HourlyRecord; HOURLY_COUNT] = core::array::from_fn(|index| HourlyRecord {
        valid_time_offset_s: index as u32 * 3_600,
        temperature_deci_c: 150,
        precipitation_tenth_mm: 0,
        precipitation_probability_pct: 0,
        condition: CONDITION_CLEAR,
        wind_from_deg: 225,
        wind_speed_deci_ms: 40,
        wind_gust_deci_ms: 60,
        flags: 0,
    });
    let mut frames = heapless::Vec::new();
    for (index, &intensity) in intensities.iter().enumerate() {
        frames
            .push(obc_app::weather::FrameSample {
                valid_at: T0 + index as i64 * 900,
                intensity,
                lat: 0,
                lon: 0,
                past_route_end: false,
                spread_uncertain: false,
            })
            .unwrap();
    }
    WeatherSnapshot {
        generated_at: T0,
        valid_from: T0,
        valid_until: T0 + 24 * 3_600,
        hourly,
        frames,
        frame_cap_s: 900,
        sampled_at: Some((0, 0)),
        pos_in_grid: true,
        current_pos_in_grid: true,
        projected: false,
        frames_truncated: false,
        rain_grid: Some(obc_render::RainGrid {
            west_udeg: -500_000,
            south_udeg: -500_000,
            east_udeg: 500_000,
            north_udeg: 500_000,
            width_cells: 16,
            height_cells: 16,
        }),
    }
}

fn rgb888(c: u16) -> Rgb888 {
    let (r, g, b) = rgb565_to_rgb888(c);
    Rgb888::new(r, g, b)
}

/// [`snapshot`] anchored at `now` instead of [`T0`], with a rain grid dense enough that the zoom
/// floor it implies is well above `AppState::new`'s starting zoom.
fn snapshot_at(now: i64, intensities: &[u8]) -> WeatherSnapshot {
    let mut snap = snapshot(intensities);
    snap.valid_from = now - 3_600;
    snap.valid_until = now + 24 * 3_600;
    for (index, frame) in snap.frames.iter_mut().enumerate() {
        frame.valid_at = now + index as i64 * 900;
    }
    snap.rain_grid = Some(obc_render::RainGrid {
        west_udeg: -100_000,
        south_udeg: -100_000,
        east_udeg: 100_000,
        north_udeg: 100_000,
        width_cells: 4_096,
        height_cells: 4_096,
    });
    snap
}

/// Feed the domain the host's freshly-sampled snapshot — the production path (stage 10) for the
/// rain map's step range and the product's zoom floor. Nothing else derives either.
fn sample(app: &mut App, snap: Option<&WeatherSnapshot>) {
    weather_pass(app, 0, snap, |_| {});
}

/// Report that the provider plane is (or is not) fetching — the production path for the cue.
fn set_refreshing(app: &mut App, fetching: bool) {
    weather_pass(app, 0, None, |facts| facts.note_weather_refreshing(fetching));
}

/// Render one full 240 × 320 frame with the given snapshot.
fn render(app: &mut App, map: &[u8], weather: Option<&WeatherSnapshot>) -> Buf {
    let cache = MapCache::new();
    let src = MapSliceSource(map);
    let tables = MapTables::parse(&src).expect("valid map");
    let reader = Reader::new(&src, &tables, &cache);
    let mut buf = Buf::new(240, 320);
    let mut scratch = Box::new(obc_render::RenderScratch::new());
    app.render_frame_with_rain(Some(&mut scratch), &mut buf, &reader, None, None, weather, 240.0, 320.0, rgb888);
    buf
}

/// Set the wall clock to `unix` UTC (offset 0) so the screens' `now_utc` is deterministic.
fn pin_clock(app: &mut App, unix: i64) {
    let mut settings = *app.settings();
    settings.clock = obc_ports::DateTime::from_unix(unix as u32);
    settings.utc_offset_min = 0;
    app.set_settings(settings);
}

/// Navigate Home → Menu → (4 steps) → the Weather station → dashboard.
fn open_dashboard(app: &mut App) {
    app.apply_gesture(Gesture::Press); // Home → Menu
    for _ in 0..4 {
        app.apply_gesture(Gesture::Step(1));
    }
    app.apply_gesture(Gesture::Press); // → Weather dashboard
    assert!(matches!(app.top_screen(), Screen::Weather(_)), "the Menu's fifth station opens the dashboard");
}

/// The Menu's Weather station reaches the dashboard; its actions open the hourly list and the
/// rain map; Back climbs back out and always resets the rain time-step.
#[test]
fn menu_reaches_every_weather_surface() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    open_dashboard(&mut app);

    // HOURLY (the first action row).
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::WeatherHourly(_)));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Weather(_)));

    // RAIN MAP (the second action row) — entry always starts at the current frame.
    app.state.rain_step = 3; // a stale leftover the entry must clear
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)));
    assert_eq!(app.state.rain_step, 0, "the rain map opens on NOW, never a leaked step");

    // Time-steps clamp to the frames that exist — three frames, so two lie ahead of NOW.
    let three = snapshot_at(app.wall_unix_now() as i64, &[0, 0, 0]);
    sample(&mut app, Some(&three));
    assert_eq!(app.weather().steps_ahead(), 2, "three frames, two of them ahead");
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Step(1));
    assert_eq!(app.state.rain_step, 2, "steps clamp at the last future frame");
    app.apply_gesture(Gesture::Step(-5));
    assert_eq!(app.state.rain_step, 0, "steps clamp at NOW");
    app.apply_gesture(Gesture::Step(2));
    app.apply_gesture(Gesture::Back);
    assert!(matches!(app.top_screen(), Screen::Weather(_)));
    assert_eq!(app.state.rain_step, 0, "leaving the rain map resets the step");
}

/// The rain map's zoom-out clamp (owner tuning round 2): entry snaps a wider-out camera to the
/// rain grid's regime floor, an Inspect zoom-out stops at the floor instead of leaving the regime,
/// zooming in stays free, and with no rain grid the clamp is disengaged.
#[test]
fn rain_map_zoom_clamps_to_the_rain_grid_regime_floor() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    // The floor the domain derives from the product's own cell density.
    let dense = snapshot_at(app.wall_unix_now() as i64, &[0; 4]);
    sample(&mut app, Some(&dense));
    let floor = app.weather().zoom_floor();
    assert!(floor > 0.05, "the fixture grid must actually engage the clamp (floor {floor})");
    app.state.zoom = 0.004; // parked far outside the regime (the browse map may be anywhere)
    open_dashboard(&mut app);
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press); // → rain map
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)));
    assert_eq!(app.state.zoom, floor, "entry snaps to the regime floor — the rider never sees out-of-regime");

    // Inspect zoom-out: enter pan, toggle Move → Zoom, step out repeatedly — the camera stops at
    // the floor every time.
    app.apply_gesture(Gesture::Hold); // enter Inspect
    app.apply_gesture(Gesture::Press); // Move → Zoom
    for _ in 0..6 {
        app.apply_gesture(Gesture::Step(1)); // zoom out
    }
    assert!(app.state.zoom >= floor, "Inspect zoom-out clamps at the floor (zoom {})", app.state.zoom);
    // Zooming in is never clamped.
    app.apply_gesture(Gesture::Step(-1));
    assert!(app.state.zoom > floor, "zooming back in is free");

    // No rain grid: the floor is 0.0 and the clamp disengages (the defensive banner remains
    // the backstop for that configuration).
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.state.zoom = 0.004;
    open_dashboard(&mut app);
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert_eq!(app.state.zoom, 0.004, "no rain grid, no clamp");
}

/// The alert card: host-pushed, re-fires update in place (never stack), VIEW RAIN MAP replaces
/// the card with the rain map at step 0, DISMISS pops — and the passkey card outranks it.
#[test]
fn alert_card_actions_and_priority() {
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.show_weather_alert(WeatherAlertKind::Rain, 35);
    assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)));
    let depth_before = app.debug_stack_len();
    app.show_weather_alert(WeatherAlertKind::Storm, 12);
    assert_eq!(app.debug_stack_len(), depth_before, "a re-fired alert updates in place, never stacks");

    // VIEW RAIN MAP (first row) replaces the card.
    app.state.rain_step = 2;
    app.apply_gesture(Gesture::Press);
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)));
    assert_eq!(app.state.rain_step, 0);
    assert_eq!(app.debug_stack_len(), depth_before, "replace, not push — Back returns to the caller");

    // DISMISS pops.
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    app.show_weather_alert(WeatherAlertKind::Storm, 5);
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press);
    assert!(!matches!(app.top_screen(), Screen::WeatherAlert(_)), "DISMISS closes the card");

    // An alert landing over an ALREADY-OPEN rain map: VIEW RAIN MAP pops back to it — never a
    // second identical rain map on the stack (review F4).
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    open_dashboard(&mut app);
    app.apply_gesture(Gesture::Step(1));
    app.apply_gesture(Gesture::Press); // → rain map
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)));
    let depth_on_map = app.debug_stack_len();
    app.state.rain_step = 2; // a viewed future frame the alert answer must reset
    app.show_weather_alert(WeatherAlertKind::Rain, 10);
    assert!(matches!(app.top_screen(), Screen::WeatherAlert(_)));
    app.apply_gesture(Gesture::Press); // VIEW RAIN MAP
    assert!(matches!(app.top_screen(), Screen::WeatherRainMap(_)));
    assert_eq!(app.debug_stack_len(), depth_on_map, "pop, not replace: exactly one rain map remains");
    assert_eq!(app.state.rain_step, 0, "the answered alert lands on the current frame");
}

/// Frame-level honesty + determinism: the dashboard renders byte-identically for identical
/// inputs, differently across the honest states (dry / stale / no-data are visibly distinct),
/// and in all four languages (each visibly different from English).
#[test]
fn dashboard_states_and_languages_render_distinct_deterministic_frames() {
    let map = build_min_obcm(0x07E0);
    let dry = snapshot(&[0; 9]);

    let mut frames: Vec<(&str, Vec<Rgb888>)> = Vec::new();
    for (name, now, feed_snapshot) in [
        ("dry", T0 + 60, Some(&dry)),
        ("stale", T0 + 20_000, Some(&dry)), // frames outrun: UPDATE NEEDED, never dry
        ("nodata", T0 + 60, None),
    ] {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        pin_clock(&mut app, now);
        open_dashboard(&mut app);
        let a = render(&mut app, &map, feed_snapshot);
        let b = render(&mut app, &map, feed_snapshot);
        assert_eq!(a.px, b.px, "{name}: identical inputs render byte-identical frames");
        frames.push((name, a.px));
    }
    for i in 0..frames.len() {
        for j in i + 1..frames.len() {
            assert_ne!(frames[i].1, frames[j].1, "{} and {} must be visibly distinct", frames[i].0, frames[j].0);
        }
    }

    // Language sweep: the dashboard + hourly list draw per-language copy.
    let mut english: Option<Vec<Rgb888>> = None;
    for lang in [
        obc_app::settings::Language::En,
        obc_app::settings::Language::De,
        obc_app::settings::Language::Fr,
        obc_app::settings::Language::Es,
    ] {
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        pin_clock(&mut app, T0 + 60);
        let mut settings = *app.settings();
        settings.language = lang;
        app.set_settings(settings);
        open_dashboard(&mut app);
        let dash = render(&mut app, &map, Some(&dry));
        app.apply_gesture(Gesture::Press); // → hourly
        let _hourly = render(&mut app, &map, Some(&dry));
        match &english {
            None => english = Some(dash.px),
            Some(en) => assert_ne!(en, &dash.px, "{lang:?} renders its own catalog copy"),
        }
    }
}

/// The refresh cue never blanks cached content: the refreshing frame differs from the idle one
/// only in the title bar's right slot (rows below the bar stay byte-identical).
#[test]
fn refresh_cue_keeps_cached_content_visible() {
    let map = build_min_obcm(0x07E0);
    let dry = snapshot(&[0; 9]);
    let mut app = App::new_idle(AppState::new(0, 0, 0.05));
    pin_clock(&mut app, T0 + 60);
    open_dashboard(&mut app);
    let idle = render(&mut app, &map, Some(&dry));
    set_refreshing(&mut app, true);
    let refreshing = render(&mut app, &map, Some(&dry));
    assert_ne!(idle.px, refreshing.px, "the UPDATING cue is visible");
    let bar_rows = 40 * 240; // the title bar band
    assert_eq!(idle.px[bar_rows..], refreshing.px[bar_rows..], "everything below the title bar stays untouched");
}
