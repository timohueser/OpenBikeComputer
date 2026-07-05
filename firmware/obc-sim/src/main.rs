//! OBC desktop simulator — host shell around the shared renderer.
//!
//! All map drawing lives in `obc_render`, the same code the nRF54L firmware runs
//! against the LS021B7DD02. This binary owns only the host concerns: argument
//! parsing, the eframe window + pan/zoom event loop, PNG output, and the color
//! policy (device 64-color quantization by default, or `--true-color`).
//!
//! The web build (wasm32) reuses only the shared host pieces and the eframe app;
//! the CLI parser and headless-PNG helpers still compile but go unreferenced there,
//! so we quiet the resulting dead-code/import noise for wasm.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use std::time::Instant;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_app::{
    App, AppState, Button, ButtonEvent, CompassSource, Fix, InputClock, InputEvent, InputSource, LocationSource,
    RideClock, Sensors, TrackAction, TrackSink,
};
use obc_reader::{rgb565_to_device64, rgb565_to_rgb888, MapCache, MapTables, Reader, SliceSource};
use obc_render::text::{draw_text, Font, TextAlign};

mod calib;
mod device_input;
mod framebuffer;
mod gui;
mod present;
// `--palette` is a native-only standalone window; keep its native APIs out of the wasm compile.
#[cfg(not(target_arch = "wasm32"))]
mod palette;
mod routes;
mod settings_store;
mod sim_compass;
mod sim_location;
mod track;
// Native-only: the web build has no filesystem to write to.
#[cfg(not(target_arch = "wasm32"))]
mod vec_sink;
use framebuffer::Framebuffer;
use obc_replay::{gpx::Track, BaroSensor, GpxPlayer};
use obc_route::{RouteIndex, RouteReader};
use routes::RouteStore;
use track::TrackStore;

struct Args {
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    /// Launch the GUI, save its first composited frame to this path, then exit.
    screenshot: Option<String>,
    true_color: bool,
    /// Start in heading-up orientation with this course (degrees CW from north).
    heading: Option<f32>,
    /// Preload this GPX track for replay.
    gpx: Option<String>,
    /// With `--gpx --png`, the playback time (seconds) to render the fix at; defaults
    /// to the track midpoint.
    at: Option<f64>,
    /// Headless camera center "lon,lat" (microdegrees); defaults to the bbox center.
    center: Option<(i32, i32)>,
    /// Headless zoom multiplier applied to the bbox-fit zoom (picks a finer LOD).
    zoom_mul: f32,
    /// Render the font/palette preview instead of the map. Needs no map.
    text_demo: bool,
    /// A gesture script applied before a headless `--png` render, to snapshot a specific
    /// screen. Tokens (one char, spaces ignored): `r`/`l` = turn cw/ccw, `p` = press,
    /// `h` = hold, `b` = back, `B` = back-hold, `H`/`M` = leave the encoder / Back held
    /// partway (snapshots the in-flight long-press hint), `w` = wait ~800 ms so an
    /// in-flight animation (the Menu needle sweep) settles before the snapshot.
    script: Option<String>,
    /// Headless `--png` only: render from the device's real power-on state (Home / Idle,
    /// no route) instead of straight from the map.
    boot: bool,
    /// Folder of `.obcr` routes — the stand-in for the device SD card; defaults to `routes/`.
    routes_dir: Option<String>,
    /// Folder for saved `.gpx` tracks + the in-progress `.obct` log; defaults to `tracks/`.
    tracks_dir: Option<String>,
    /// Headless `--gpx` only: after replaying, finalise the active ride to a `.gpx`
    /// (verifies the load→ride→save loop without the GUI).
    save_track: bool,
    /// Convert this GPX into the routes folder and exit. Needs no map.
    import: Option<String>,
    /// Render the device window at the panel's true physical size (needs a saved
    /// calibration). Falls back to the scaled view if uncalibrated.
    physical: bool,
    /// Open the GUI straight into the 1:1 size-calibration screen.
    calibrate: bool,
    /// Show the device's 64-color gamut and nothing else. Needs no map.
    palette: bool,
    /// Initial housing body color: `coral` | `mint` | `mustard` | `slate` (default slate).
    colorway: Option<String>,
    /// Boot straight onto the live Map instead of the Home/Idle screensaver. The native
    /// GUI always boots to Home; the web demo sets this so the page opens on the moving map.
    start_on_map: bool,
    /// Initial battery charge (0–100 %) shown on the Home gauge; stands in for the not-yet-
    /// wired fuel gauge. Defaults to full.
    battery: Option<u8>,
    /// Seed for the Home screensaver's contour pattern. On the device the seed is the
    /// wall-clock millis at each return to Home; this pins it for a headless render.
    home_seed: Option<u32>,
    /// Headless `--png` only: seed the device's local wall-clock to `YYYY-MM-DDTHH:MM` (in manual
    /// mode, so `local_clock()` returns it verbatim). Pins the POI-detail "today's hours" weekday +
    /// the OPEN/CLOSED-now badge for a reproducible render. Defaults to the device default
    /// (2025-01-01 12:00, a Wednesday noon).
    clock: Option<obc_app::settings::DateTime>,
    /// Headless `--png` only: render with a phone linked over BLE, so the connected indicator
    /// shows (the menu title bar / Home). Stands in for the sim control panel's "Phone connected"
    /// toggle when capturing a snapshot.
    ble_connected: bool,
    /// Headless `--png` only: inject a BLE pairing passkey so the host-pushed passkey card is up
    /// (epic #447, P2), for the `passkey-card.png` snapshot. Stands in for the sim control panel's
    /// "Pairing" toggle.
    ble_passkey: Option<u32>,
    /// Headless `--png` only: render with a stored bond, so the Bluetooth screen's Paired row
    /// reads "yes" (and its Forget row arms). Stands in for the control panel's "Paired" toggle.
    ble_paired: bool,
}

impl Default for Args {
    /// Device resolution + all knobs off — the base for both the CLI parser and the web build.
    fn default() -> Self {
        Args {
            map: String::new(),
            width: 240,
            height: 320,
            scale: 1,
            png: None,
            screenshot: None,
            true_color: false,
            heading: None,
            gpx: None,
            at: None,
            center: None,
            zoom_mul: 1.0,
            text_demo: false,
            script: None,
            boot: false,
            routes_dir: None,
            tracks_dir: None,
            save_track: false,
            import: None,
            physical: false,
            calibrate: false,
            palette: false,
            colorway: None,
            start_on_map: false,
            battery: None,
            home_seed: None,
            clock: None,
            ble_connected: false,
            ble_passkey: None,
            ble_paired: false,
        }
    }
}

impl Args {
    /// Defaults for the web build: the [`Args::default`] base plus in-memory route/track
    /// stores (the `*_dir`s are unused on wasm) and the demo-friendly tweaks.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn web_default() -> Self {
        Args {
            // Warm terracotta body for the web demo, to fit the parchment/forest/amber page.
            colorway: Some("coral".to_string()),
            start_on_map: true,
            ..Args::default()
        }
    }

    pub(crate) fn routes_dir(&self) -> String {
        self.routes_dir.clone().unwrap_or_else(|| "routes".to_string())
    }

    pub(crate) fn tracks_dir(&self) -> String {
        self.tracks_dir.clone().unwrap_or_else(|| "tracks".to_string())
    }

    /// The persisted-settings file (the device's RRAM stand-in). Holds the shared
    /// [`obc_app::settings`] blob, so relaunching restores units / clock / intervals.
    pub(crate) fn settings_path(&self) -> String {
        "obc-settings.bin".to_string()
    }
}

/// Parse a `--clock` value `YYYY-MM-DDTHH:MM` into a [`DateTime`](obc_app::settings::DateTime).
/// Rejects a malformed stamp with a message (out-of-range fields are clamped by `Settings::decode`'s
/// sanitiser when seeded, but the format itself must be well-formed).
fn parse_clock(s: &str) -> Result<obc_app::settings::DateTime, String> {
    let (date, time) = s.split_once('T').ok_or("--clock format is YYYY-MM-DDTHH:MM")?;
    let mut d = date.split('-');
    let mut t = time.split(':');
    let year = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock year")?;
    let month = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock month")?;
    let day = d.next().and_then(|v| v.parse().ok()).ok_or("bad --clock day")?;
    let hour = t.next().and_then(|v| v.parse().ok()).ok_or("bad --clock hour")?;
    let minute = t.next().and_then(|v| v.parse().ok()).ok_or("bad --clock minute")?;
    Ok(obc_app::settings::DateTime { year, month, day, hour, minute })
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--size" => {
                let s = it.next().ok_or("--size needs WxH")?;
                let (w, h) = s.split_once('x').ok_or("--size format is WxH")?;
                a.width = w.parse().map_err(|_| "bad width")?;
                a.height = h.parse().map_err(|_| "bad height")?;
            }
            "--scale" => a.scale = it.next().and_then(|s| s.parse().ok()).ok_or("bad --scale")?,
            "--png" => a.png = Some(it.next().ok_or("--png needs a path")?),
            "--screenshot" => a.screenshot = Some(it.next().ok_or("--screenshot needs a path")?),
            "--true-color" => a.true_color = true,
            "--heading" => a.heading = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --heading")?),
            "--gpx" => a.gpx = Some(it.next().ok_or("--gpx needs a path")?),
            "--at" => a.at = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --at")?),
            "--center" => {
                let s = it.next().ok_or("--center needs lon,lat")?;
                let (lon, lat) = s.split_once(',').ok_or("--center format is lon,lat")?;
                a.center = Some((
                    lon.trim().parse().map_err(|_| "bad --center lon")?,
                    lat.trim().parse().map_err(|_| "bad --center lat")?,
                ));
            }
            "--zoom" => a.zoom_mul = it.next().and_then(|s| s.parse().ok()).ok_or("bad --zoom")?,
            "--text-demo" => a.text_demo = true,
            "--script" => a.script = Some(it.next().ok_or("--script needs a token string")?),
            "--boot" => a.boot = true,
            "--routes-dir" => a.routes_dir = Some(it.next().ok_or("--routes-dir needs a path")?),
            "--tracks-dir" => a.tracks_dir = Some(it.next().ok_or("--tracks-dir needs a path")?),
            "--save-track" => a.save_track = true,
            "--import" => a.import = Some(it.next().ok_or("--import needs a GPX path")?),
            "--physical" => a.physical = true,
            "--calibrate" => a.calibrate = true,
            "--palette" => a.palette = true,
            "--colorway" => a.colorway = Some(it.next().ok_or("--colorway needs a name")?),
            "--battery" => {
                a.battery = Some(
                    it.next().and_then(|s| s.parse().ok()).filter(|&b| b <= 100).ok_or("--battery needs 0..=100")?,
                )
            }
            "--home-seed" => {
                a.home_seed = Some(it.next().and_then(|s| s.parse().ok()).ok_or("--home-seed needs a u32")?)
            }
            "--clock" => {
                a.clock = Some(parse_clock(&it.next().ok_or("--clock needs YYYY-MM-DDTHH:MM")?)?);
            }
            "--ble-connected" => a.ble_connected = true,
            "--ble-passkey" => {
                a.ble_passkey = Some(
                    it.next()
                        .and_then(|s| s.parse().ok())
                        .filter(|&n| n <= 999_999)
                        .ok_or("--ble-passkey needs 0..=999999")?,
                )
            }
            "--ble-paired" => a.ble_paired = true,
            other => {
                if a.map.is_empty() {
                    a.map = other.to_string();
                } else {
                    return Err(format!("unexpected arg: {other}"));
                }
            }
        }
    }
    // `--text-demo`, `--palette` and `--import` need no map file.
    if a.map.is_empty() && !a.text_demo && !a.palette && a.import.is_none() {
        return Err("missing map path".into());
    }
    Ok(a)
}

fn color_of(c: u16, true_color: bool) -> Rgb888 {
    let (r, g, b) = if true_color { rgb565_to_rgb888(c) } else { rgb565_to_device64(c) };
    Rgb888::new(r, g, b)
}

/// Pack 8-bit RGB into RGB565 (the color space the renderer quantizes from), so the
/// demo palette below can be written as the spec's hexes.
const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
}

// The "explorer's field map" palette, in RGB565 so it travels through the same
// `color_of` quantization as map styles.
const PARCHMENT: u16 = rgb565(0xEA, 0xDF, 0xC0);
const HUD: u16 = rgb565(0x2E, 0x25, 0x1A);
const INK: u16 = rgb565(0x2C, 0x21, 0x14);
const AMBER: u16 = rgb565(0xE3, 0xA5, 0x2B);
const FOREST: u16 = rgb565(0x4F, 0x6B, 0x43);
const WOOD: u16 = rgb565(0x5B, 0x3F, 0x28);
const WARNING: u16 = rgb565(0xC0, 0x49, 0x2E);

/// Render the font ladder + palette through the device-64 `color_of`, so the PNG
/// shows exactly what the panel would (`--true-color` shows the un-quantized reference).
fn render_text_demo(fb: &mut Framebuffer, true_color: bool) {
    let col = |c: u16| color_of(c, true_color);
    let w = fb.width() as i32;

    let _ = fb.clear(col(PARCHMENT));
    let _ = fb.fill_solid(&Rectangle::new(Point::zero(), Size::new(fb.width(), 28)), col(HUD));
    draw_text(fb, "TERMINUS FONT DEMO", Point::new(w / 2, 3), Font::Label, TextAlign::Center, col(PARCHMENT));

    // Font ladder: each tier's caption over a true-size sample, annotated with its measured
    // cap height in mm so the size targets are checkable (render `--physical` for device scale).
    let sample = "12.5 km/h";
    let mut y = 36;
    for (caption, font) in [
        ("Label  ter24  2.0mm", Font::Label),
        ("Body   ter28  2.4mm", Font::Body),
        ("Disply ter32  2.7mm", Font::Display),
    ] {
        draw_text(fb, caption, Point::new(8, y), Font::Label, TextAlign::Left, col(WOOD));
        y += Font::Label.line_height() as i32 + 2;
        draw_text(fb, sample, Point::new(8, y), font, TextAlign::Left, col(INK));
        y += font.line_height() as i32 + 8;
    }

    // Palette — each name in its own color, so the PNG shows whether they stay distinct
    // and legible after device-64 quantization.
    for (name, c) in [("amber", AMBER), ("forest", FOREST), ("wood", WOOD), ("warning", WARNING)] {
        draw_text(fb, name, Point::new(8, y), Font::Label, TextAlign::Left, col(c));
        y += Font::Label.line_height() as i32 + 2;
    }

    y += 6;
    draw_text(fb, "LEFT", Point::new(8, y), Font::Label, TextAlign::Left, col(INK));
    draw_text(fb, "CENTER", Point::new(w / 2, y), Font::Label, TextAlign::Center, col(INK));
    draw_text(fb, "RIGHT", Point::new(w - 8, y), Font::Label, TextAlign::Right, col(INK));
}

/// The starting camera for a freshly-opened map: centered on the bbox, zoomed so
/// its longitude span fills the window width. Returns `(cam_lon, cam_lat, zoom)`
/// in the [`AppState`] convention (microdegrees, pixels-per-microdegree).
fn initial_camera(reader: &Reader, width: u32) -> (i32, i32, f32) {
    let b = reader.bbox;
    let cam_lon = (b.min_lon as i64 + b.max_lon as i64) / 2;
    let cam_lat = (b.min_lat as i64 + b.max_lat as i64) / 2;
    let span_lon = (b.max_lon as i64 - b.min_lon as i64).max(1) as f32;
    (cam_lon as i32, cam_lat as i32, width as f32 / span_lon)
}

/// Advance the GPX replay by `dt` seconds and run one app tick on the **playback**
/// clock. The millis derive from playback-time (not wall-clock), so Avg. Speed isn't
/// scaled by the replay-speed multiplier. Shared by the live GUI loop and the headless
/// `--png` replay.
fn replay_step<'s>(
    app: &mut App,
    player: &'s mut GpxPlayer,
    baro: &'s mut BaroSensor,
    compass: Option<&'s mut dyn CompassSource>,
    dt: f64,
    route: Option<&RouteReader>,
    track: Option<&'s mut dyn TrackSink>,
) {
    // The sensor handles share one lifetime `'s` so the invariant `Sensors<'a>` can bind them
    // together. The compass only matters while stationary (GPS course drops to `None`).
    player.advance(dt);
    baro.feed(player.elevation_at(player.time()), player.time());
    let now_ms = (player.time() * 1000.0) as u32;
    let sensors =
        Sensors { loc: player, altimeter: Some(baro), temperature: None, clock: None, compass, track, fuel: None };
    app.tick(RideClock(now_ms), sensors, route);
}

/// Reconcile the track store to the app's tracking intent (drains the one-shot action,
/// opens / closes the `.obct` log). The save name comes from the active route's catalog entry.
fn reconcile_tracks(app: &mut App, tracks: &mut TrackStore) {
    let action = app.activity.take_track_action();
    let session = app.activity.session;
    let name = app.activity.active_route.and_then(|i| app.routes().get(i)).map(|r| r.name.as_str().to_string());
    tracks.reconcile(action, session, name.as_deref());
}

/// Encode a framebuffer to a PNG, upscaling by `scale` with nearest-neighbor so the
/// device's hard pixel edges stay crisp.
fn write_png(fb: &Framebuffer, scale: u32, path: &str) -> Result<(), String> {
    let (w, h) = (fb.width(), fb.height());
    let base = image::RgbImage::from_raw(w, h, fb.as_rgb888().to_vec()).ok_or("framebuffer size mismatch")?;
    let out = if scale > 1 {
        image::imageops::resize(&base, w * scale, h * scale, image::imageops::FilterType::Nearest)
    } else {
        base
    };
    out.save(path).map_err(|e| format!("save_png failed: {e}"))
}

/// A scripted [`InputSource`] that replays a fixed queue of raw events — the
/// headless counterpart to the control panel's [`device_input::DeviceInput`].
struct ScriptInput(std::collections::VecDeque<InputEvent>);
impl InputSource for ScriptInput {
    fn poll(&mut self) -> Option<InputEvent> {
        self.0.pop_front()
    }
}

/// Feed one batch of raw events to the app at time `now` (ms).
fn feed(app: &mut App, now: u32, events: Vec<InputEvent>) {
    app.handle_input(InputClock(now), &mut ScriptInput(events.into()));
}

/// Apply a gesture script (see `Args::script`) to `app`. Synthesizes the raw encoder/Back
/// events with a rising clock — including the threshold crossing that turns a held button
/// into a `Hold`/`BackHold` — exactly as the real recognizer would see them.
///
/// `render` draws one throwaway headless frame against the current app state — the `d` token uses
/// it to **flush lazy draw-time state** that only fills at draw (the POI-list snapshot, then the
/// detail's hours read), so a script can `p` into a POI *and then* `d p` to open its detail (the
/// Press needs the snapshot the first draw takes). Without a `d` the whole script runs before the
/// single final render, so lazy state never fills mid-script.
fn apply_script(app: &mut App, script: &str, render: &mut dyn FnMut(&mut App)) {
    let down = |b| InputEvent::Button(ButtonEvent::Down(b));
    let up = |b| InputEvent::Button(ButtonEvent::Up(b));
    let hold = obc_app::DEFAULT_HOLD_MS;
    let mut now: u32 = 100;

    // A turn detent: feed it, then nudge the clock.
    let turn = |app: &mut App, now: &mut u32, dir: i32| {
        feed(app, *now, vec![InputEvent::Turn(dir)]);
        *now += 30;
    };
    // A tap: down, then up 80 ms later (well under the long-press threshold).
    let tap = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += 80;
        feed(app, *now, vec![up(b)]);
        *now += 30;
    };
    // A long-press: hold past the threshold (one empty tick fires `Hold`/`BackHold`), then release.
    let press_hold = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += hold + 80;
        feed(app, *now, vec![]);
        *now += 30;
        feed(app, *now, vec![up(b)]);
        *now += 30;
    };
    // Held partway (no release, no threshold crossing): snapshots the in-flight long-press hint.
    let partial_hold = |app: &mut App, now: &mut u32, b| {
        feed(app, *now, vec![down(b)]);
        *now += hold * 55 / 100; // ~55% toward the threshold
        feed(app, *now, vec![]); // samples the in-flight progress for the render
    };

    for ch in script.chars() {
        match ch {
            ' ' => {}
            'r' => turn(app, &mut now, 1),
            'l' => turn(app, &mut now, -1),
            'p' => tap(app, &mut now, Button::Encoder),
            'b' => tap(app, &mut now, Button::Back),
            'h' => press_hold(app, &mut now, Button::Encoder),
            'B' => press_hold(app, &mut now, Button::Back),
            'H' => partial_hold(app, &mut now, Button::Encoder),
            'M' => partial_hold(app, &mut now, Button::Back),
            // Settle: step the clock ~800 ms in animation-sized ticks (a sweep integrates a
            // dt-capped step per poll, so one big jump would leave it mid-flight) until any
            // time-driven animation (the Menu needle) has finished. Not for use after `H`/`M` —
            // the empty feeds would cross the hold threshold and fire the `Hold`/`BackHold`
            // those tokens deliberately leave armed.
            'w' => {
                for _ in 0..8 {
                    now += 100;
                    feed(app, now, vec![]);
                }
            }
            // Draw one throwaway frame to flush lazy draw-time state (the POI-list snapshot / the
            // detail's hours read) so the next gesture sees it — e.g. `p d p` opens a POI list, fills
            // its snapshot, then presses a POI into its detail.
            'd' => render(app),
            other => eprintln!("warning: ignoring unknown --script token '{other}'"),
        }
    }
}

/// Web entry: hand the page's canvas to the shared eframe app (see [`gui::run_web`]).
#[cfg(target_arch = "wasm32")]
fn main() {
    gui::run_web();
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\nusage: obc-sim <map.obcm> [--size WxH] [--scale N] [--png OUT] [--true-color] [--heading DEG] [--gpx TRACK.gpx] [--at SEC] [--center LON,LAT] [--zoom MULT] [--text-demo] [--palette] [--script TOKENS] [--boot] [--routes-dir DIR] [--tracks-dir DIR] [--save-track] [--import GPX] [--physical] [--calibrate] [--colorway NAME] [--battery PCT] [--home-seed N] [--clock YYYY-MM-DDTHH:MM] [--ble-connected] [--ble-passkey N] [--ble-paired]");
            std::process::exit(2);
        }
    };

    // Font/palette preview: render text on a blank panel and exit. Before the map read (needs none).
    if args.text_demo {
        let mut fb = Framebuffer::new(args.width, args.height);
        render_text_demo(&mut fb, args.true_color);
        let path = args.png.as_deref().unwrap_or("text_demo.png");
        if let Err(e) = write_png(&fb, args.scale, path) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        eprintln!("wrote {path}");
        return;
    }

    // `--palette`: the device's 64-color gamut on a standalone color-test screen. Needs no
    // map. With `--png` it writes the frame headlessly (diffable in CI); else a minimal window.
    if args.palette {
        if let Some(path) = &args.png {
            let mut fb = Framebuffer::new(args.width, args.height);
            palette::draw_palette(&mut fb);
            if let Err(e) = write_png(&fb, args.scale, path) {
                eprintln!("{e}");
                std::process::exit(1);
            }
            eprintln!("wrote {path}");
        } else if let Err(e) = palette::run(args.width, args.height, args.scale) {
            eprintln!("palette error: {e}");
            std::process::exit(1);
        }
        return;
    }

    // `--import` converts a GPX into the routes folder (the device's USB-drop path). Needs no map.
    if let Some(gpx) = &args.import {
        let dir = args.routes_dir();
        let mut store = RouteStore::open(&dir);
        match store.import_gpx(std::path::Path::new(gpx)) {
            Ok(s) => eprintln!(
                "imported {gpx} → {dir}/ | {} km, +{} m / -{} m | {} pts, {} chunks, ele {}..{} m",
                (s.total_distance_m + 500) / 1000,
                s.total_ascent_m,
                s.total_descent_m,
                s.point_count,
                s.chunk_count,
                s.min_ele_m,
                s.max_ele_m
            ),
            Err(e) => {
                eprintln!("import failed: {e}");
                std::process::exit(1);
            }
        }
        return;
    }

    let bytes = std::fs::read(&args.map).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", args.map);
        std::process::exit(1);
    });

    // Validate + log once up front; the borrow ends with this block so `bytes` can move
    // into the GUI (which rebuilds the cheap `Reader` view per frame).
    {
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap_or_else(|e| {
            eprintln!("invalid OBCM file: {e:?}");
            std::process::exit(1);
        });
        let reader = Reader::new(&src, &tables, &cache);
        eprintln!(
            "OBCM v{} | bbox {:?} | {} LODs | {} styles",
            reader.version,
            reader.bbox,
            reader.lods().len(),
            (0..=255).filter(|&i| reader.style(i).is_some()).count()
        );
        for (i, l) in reader.lods().iter().enumerate() {
            eprintln!(
                "  LOD {i}: max_mpp {} | {} nodes | chunk_size {} | {} chunks",
                l.max_mpp, l.node_count, l.chunk_size, l.chunk_count
            );
        }
    }

    // Headless mode: render one frame through the shared app, save PNG, exit.
    if let Some(path) = &args.png {
        let cache = MapCache::new();
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).expect("validated above");
        let reader = Reader::new(&src, &tables, &cache);
        let (mut cx, mut cy, mut zoom) = initial_camera(&reader, args.width);
        if let Some((lon, lat)) = args.center {
            cx = lon;
            cy = lat;
        }
        zoom *= args.zoom_mul;
        let mut state = AppState::new(cx, cy, zoom);
        if let Some(b) = args.battery {
            state.battery_pct = b;
        }
        // `--heading` renders a rotated (heading-up) frame; the rotation derives from the
        // fix's course, so seed one at the map center.
        if let Some(deg) = args.heading {
            state.heading_up = true;
            state.user_fix = Some(Fix { lat: cy, lon: cx, course: Some(deg), speed_mps: None });
        }
        // `--gpx` renders the replayed fix at `--at` (default: track midpoint). Seed the
        // camera/heading from that fix now; the replay up to `--at` runs below (after the
        // route opens) so the snapshot shows live riding state, not just a static marker.
        let mut player: Option<GpxPlayer> = None;
        let mut replay_to = 0.0_f64;
        if let Some(path) = &args.gpx {
            match Track::load(std::path::Path::new(path)) {
                Ok(track) => {
                    let mut p = GpxPlayer::new(track);
                    let at = args.at.unwrap_or(p.duration() / 2.0);
                    replay_to = at;
                    p.seek(at);
                    if let Some(fix) = p.poll() {
                        state.heading_up = fix.course.is_some();
                        state.user_fix = Some(fix);
                        state.cam_lon = fix.lon;
                        state.cam_lat = fix.lat;
                    }
                    player = Some(p);
                }
                Err(e) => {
                    eprintln!("cannot load GPX: {e}");
                    std::process::exit(1);
                }
            }
        }
        let mut app = if args.boot { App::new_idle(state) } else { App::new(state) };
        if let Some(seed) = args.home_seed {
            app.reseed_home(seed);
        }
        // `--clock`: seed the local wall-clock in manual mode (`gps_time = false` ⇒ `local_clock()`
        // returns it verbatim), pinning the POI-detail weekday + OPEN/CLOSED-now badge. `set_settings`
        // restamps the WallClock from this local set-point (see `App::set_settings`).
        if let Some(clock) = args.clock {
            let settings = obc_app::settings::Settings { gps_time: false, clock, ..Default::default() };
            app.set_settings(settings);
        }
        // Load the routes folder so the Route menu has real entries and a picked route
        // can be drawn.
        let mut store = RouteStore::open(args.routes_dir());
        app.set_routes_with_ids(store.catalog(), store.ids());
        // Inject the BLE link state (epic #447) **before** the script runs: `--ble-connected` shows
        // the connected indicator, `--ble-passkey N` puts the host-pushed passkey card up (P2), and
        // `--ble-paired` a stored bond — so a scripted gesture on the Bluetooth screen (its Forget
        // hold arms only while paired) sees the bond, exactly as the control panel drives it live.
        let link = if args.ble_connected { obc_app::BleLink::Connected } else { obc_app::BleLink::Advertising };
        app.set_ble_status(obc_app::BleStatus { link, passkey: args.ble_passkey, paired: args.ble_paired });
        if let Some(script) = &args.script {
            // The `d` token flushes lazy draw-time state (the POI snapshot / detail hours) by drawing
            // one throwaway frame against the map reader — `route: None` since the POI screens the
            // token targets never draw the route, and the route isn't opened until below anyway.
            let (rw, rh, rtc) = (args.width, args.height, args.true_color);
            let mut render = |app: &mut App| {
                let mut fb = Framebuffer::new(rw, rh);
                let _ = app.render_frame(&mut fb, &reader, None, rw as f32, rh as f32, |c| color_of(c, rtc));
            };
            apply_script(&mut app, script, &mut render);
            // A scripted hold-to-delete in the Route menu (epic #447 P6) records a delete request;
            // execute it here (delete the file + re-feed the id-carrying catalog) so the rendered
            // frame reflects the route being gone, mirroring the GUI's per-frame drain.
            if let Some(id) = app.take_route_delete() {
                if store.delete_by_id(id) {
                    app.set_routes_with_ids(store.catalog(), store.ids());
                }
            }
        }

        // The script may have loaded a route; open its geometry for the Map.
        store.sync_active(app.activity.active_route);
        let route_src = store.active_source();
        let route_index = route_src.as_ref().and_then(|s| RouteIndex::read(s).ok());
        let route = match (route_index.as_ref(), route_src.as_ref()) {
            (Some(idx), Some(s)) => Some(RouteReader::new(idx, s)),
            _ => None,
        };

        let mut tracks = TrackStore::open(args.tracks_dir());

        // Replay the track from the start up to `--at`, ticking the app each step so the
        // map-matcher locks on and the ride accumulators + breadcrumb fill. A coarse-but-
        // bounded step keeps long tracks fast while staying under the dropout/teleport gates.
        if let Some(p) = player.as_mut() {
            let mut baro = BaroSensor::new();
            p.seek(0.0);
            p.play();
            let step = (replay_to / 400.0).clamp(1.0, 8.0);
            let mut t = 0.0;
            while t < replay_to {
                reconcile_tracks(&mut app, &mut tracks);
                replay_step(&mut app, p, &mut baro, None, step, route.as_ref(), tracks.sink());
                t += step;
            }
        }

        // `--save-track`: finalise the active ride to a `.gpx` (verifies the save loop).
        if args.save_track {
            if tracks.is_recording() {
                app.activity.request_track(TrackAction::Save);
                app.activity.end_session();
                reconcile_tracks(&mut app, &mut tracks);
            } else {
                eprintln!("--save-track: no active ride (start a route first, e.g. --boot --script ppp)");
            }
        }

        let mut fb = Framebuffer::new(args.width, args.height);
        let tc = args.true_color;

        // Time the whole frame draw into `render_us` (the no_std renderer has no clock, so
        // the host fills it) — same field the live panel shows.
        let t0 = Instant::now();
        let mut stats =
            app.render_frame(&mut fb, &reader, route.as_ref(), args.width as f32, args.height as f32, |c| {
                color_of(c, tc)
            });
        stats.render_us = t0.elapsed().as_micros() as u32;
        let cache_reqs = stats.map_chunk_hits + stats.map_chunk_misses;
        let hit_pct = if cache_reqs == 0 { 0.0 } else { 100.0 * stats.map_chunk_hits as f32 / cache_reqs as f32 };
        eprintln!(
            "rendered {}/{} features ({} chunks, LOD {}, {} dropped) | route {}/{} drawn, {} chunks in {:.2} ms | spans {:.0}% points {:.0}% rings {:.0}% | map-cache {:.0}% hit, {} reads, {} B",
            stats.features_drawn,
            stats.features_tried,
            stats.chunks_visited,
            stats.lod,
            stats.features_dropped,
            stats.route_points_drawn,
            stats.route_points,
            stats.route_chunks,
            stats.render_us as f64 / 1000.0,
            stats.span_utilization * 100.0,
            stats.point_utilization * 100.0,
            stats.ring_utilization * 100.0,
            hit_pct,
            stats.map_sd_reads,
            stats.map_bytes_read
        );

        if let Err(e) = write_png(&fb, args.scale, path) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        eprintln!("wrote {path}");
        return;
    }

    // Interactive: hand the map to the eframe host window.
    if let Err(e) = gui::run(bytes, args) {
        eprintln!("gui error: {e}");
        std::process::exit(1);
    }
}
