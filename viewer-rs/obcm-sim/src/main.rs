//! OBCM desktop simulator — host shell around the shared `obcm` renderer.
//!
//! All map drawing (projection, LOD selection, polygon fill, lines) lives in
//! `obcm_render`, the same code the nRF5340 firmware will run against the
//! LS021B7DD02. This binary only owns the host concerns: argument parsing, the
//! SDL window + pan/zoom event loop, PNG output, and the color policy (device
//! 64-color quantization by default, or `--true-color`).
//!
//! Usage:
//!   obcm-sim <map.obcm> [--size WxH] [--scale N] [--png OUT.png] [--true-color]
//!     [--heading DEG] [--gpx TRACK.gpx] [--at SEC] [--center LON,LAT] [--zoom MULT]
//!     [--routes-dir DIR] [--import GPX]
//!
//! `--center`/`--zoom` aim the headless `--png` camera at a spot and zoom level
//! (e.g. to inspect a specific chunk boundary); `--zoom` multiplies the bbox-fit
//! zoom. `--routes-dir` points at the folder of `.obcr` routes (the device-SD
//! stand-in; default `routes/`); `--import GPX` converts a GPX into it and exits, the
//! host-side run of the same conversion the device does on a USB drop. Routes can also
//! be dropped onto the window to import them live.
//!
//! Interactive: drag to pan, scroll to zoom, Esc/Q to quit.

use std::time::Instant;

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obcm_reader::{rgb565_to_device64, rgb565_to_rgb888, Reader};
use obcm_render::text::{draw_text, Font, TextAlign};
use obcm_app::{App, AppState, Button, ButtonEvent, Fix, InputEvent, InputSource, LocationSource};

mod device_input;
mod framebuffer;
mod gpx;
mod gpx_player;
mod gui;
mod routes;
mod sim_location;
use framebuffer::Framebuffer;
use gpx::Track;
use gpx_player::GpxPlayer;
use obcm_route::RouteReader;
use routes::RouteStore;

struct Args {
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    /// Launch the GUI, save its first composited frame to this path, then exit.
    screenshot: Option<String>,
    true_color: bool,
    /// Start in heading-up orientation with this course (degrees CW from north),
    /// so a rotated frame can be rendered headlessly (`--png`) or shown on launch.
    heading: Option<f32>,
    /// Preload this GPX track for replay (the GUI opens with it loaded; `--png`
    /// renders the fix at `--at`).
    gpx: Option<String>,
    /// With `--gpx --png`, the playback time (seconds) to render the fix at;
    /// defaults to the track midpoint.
    at: Option<f64>,
    /// Headless camera center "lon,lat" (microdegrees); defaults to the bbox
    /// center. Lets `--png` target a specific spot (e.g. a chunk boundary).
    center: Option<(i32, i32)>,
    /// Headless zoom multiplier applied to the bbox-fit zoom (e.g. `30` zooms in
    /// ~30×, picking a finer LOD). Defaults to 1 (whole-map overview).
    zoom_mul: f32,
    /// Render the font/palette preview instead of the map (slice-1 text check).
    /// Needs no map; writes to `--png` (default `text_demo.png`) and exits.
    text_demo: bool,
    /// A gesture script applied to the app before a headless `--png` render, so a
    /// specific screen can be snapshotted. Tokens (one char each, spaces ignored):
    /// `r`/`l` = turn cw/ccw, `p` = press, `h` = hold, `b` = back, `B` = back-hold.
    /// E.g. `--script B` opens the Menu; `--script p` opens Ride control.
    script: Option<String>,
    /// Boot at the device's real power-on state (Home / Idle, no route) instead of
    /// the map. Use with `--script` to walk the Home → Route menu → Map flow.
    boot: bool,
    /// Folder of `.obcr` routes — the simulator's stand-in for the device SD card.
    /// The Route menu lists these; defaults to `routes/`.
    routes_dir: Option<String>,
    /// Convert this GPX into the routes folder and exit (the device does the same on
    /// a USB drop). Headless; needs no map.
    import: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        map: String::new(),
        width: 240,
        height: 320,
        scale: 3,
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
        import: None,
    };
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
            "--heading" => {
                a.heading = Some(it.next().and_then(|s| s.parse().ok()).ok_or("bad --heading")?)
            }
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
            "--import" => a.import = Some(it.next().ok_or("--import needs a GPX path")?),
            other => {
                if a.map.is_empty() {
                    a.map = other.to_string();
                } else {
                    return Err(format!("unexpected arg: {other}"));
                }
            }
        }
    }
    // `--text-demo` and `--import` need no map file.
    if a.map.is_empty() && !a.text_demo && a.import.is_none() {
        return Err("missing map path".into());
    }
    Ok(a)
}

fn color_of(c: u16, true_color: bool) -> Rgb888 {
    let (r, g, b) = if true_color { rgb565_to_rgb888(c) } else { rgb565_to_device64(c) };
    Rgb888::new(r, g, b)
}

/// Pack 8-bit RGB into RGB565 (the format/style color space the renderer
/// quantizes from), so the demo palette below can be written as the spec's hexes.
const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    (((r as u16) >> 3) << 11) | (((g as u16) >> 2) << 5) | ((b as u16) >> 3)
}

// The "explorer's field map" palette from docs/bikepacking-computer-ui-spec.md,
// in RGB565 so it travels through the same `color_of` quantization as map styles.
const PARCHMENT: u16 = rgb565(0xEA, 0xDF, 0xC0);
const HUD: u16 = rgb565(0x2E, 0x25, 0x1A); // wood-dark HUD strip
const INK: u16 = rgb565(0x2C, 0x21, 0x14);
const AMBER: u16 = rgb565(0xE3, 0xA5, 0x2B);
const FOREST: u16 = rgb565(0x4F, 0x6B, 0x43);
const WOOD: u16 = rgb565(0x5B, 0x3F, 0x28);
const WARNING: u16 = rgb565(0xC0, 0x49, 0x2E);

/// Slice-1 verification: render the font ladder + palette on a parchment panel
/// with the elevation/menu HUD strip, through the device-64 `color_of` so the
/// PNG shows exactly what the panel would. Proves text renders and that each
/// palette color survives quantization (`--true-color` shows the un-quantized
/// reference for comparison).
fn render_text_demo(fb: &mut Framebuffer, true_color: bool) {
    let col = |c: u16| color_of(c, true_color);
    let w = fb.width() as i32;

    let _ = fb.clear(col(PARCHMENT));
    let _ = fb.fill_solid(&Rectangle::new(Point::zero(), Size::new(fb.width(), 22)), col(HUD));
    draw_text(fb, "TEXT DEMO", Point::new(w / 2, 6), Font::Label, TextAlign::Center, col(PARCHMENT));

    // Font ladder — the three sizes, in ink.
    let mut y = 30;
    for (label, font) in
        [("Label 6x10", Font::Label), ("Body 9x15", Font::Body), ("Display 10x20", Font::Display)]
    {
        draw_text(fb, label, Point::new(8, y), font, TextAlign::Left, col(INK));
        y += font.line_height() as i32 + 6;
    }

    // Palette — each name drawn in its own color, so the PNG shows whether amber,
    // forest, wood and warning stay distinct and legible after device-64 quantization.
    y += 6;
    for (name, c) in [("amber", AMBER), ("forest", FOREST), ("wood", WOOD), ("warning", WARNING)] {
        draw_text(fb, name, Point::new(8, y), Font::Body, TextAlign::Left, col(c));
        y += Font::Body.line_height() as i32 + 4;
    }

    // Alignment row + a big number, mirroring the menu counter / stat tiles.
    y += 8;
    draw_text(fb, "LEFT", Point::new(8, y), Font::Label, TextAlign::Left, col(INK));
    draw_text(fb, "CENTER", Point::new(w / 2, y), Font::Label, TextAlign::Center, col(INK));
    draw_text(fb, "RIGHT", Point::new(w - 8, y), Font::Label, TextAlign::Right, col(INK));
    y += 22;
    draw_text(fb, "42.1 km", Point::new(w / 2, y), Font::Display, TextAlign::Center, col(INK));
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

/// Encode a framebuffer to a PNG, upscaling by `scale` with nearest-neighbor so
/// the device's hard pixel edges stay crisp (matching the old simulator output).
fn write_png(fb: &Framebuffer, scale: u32, path: &str) -> Result<(), String> {
    let (w, h) = (fb.width(), fb.height());
    let base = image::RgbImage::from_raw(w, h, fb.as_rgb888().to_vec())
        .ok_or("framebuffer size mismatch")?;
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
    app.handle_input(now, &mut ScriptInput(events.into()));
}

/// Apply a gesture script (see `Args::script`) to `app`, so a headless render can
/// snapshot any screen. Synthesizes the raw encoder/Back events with a rising
/// clock — including the threshold crossing that turns a held button into a
/// `Hold`/`BackHold` — exactly as the real recognizer would see them.
fn apply_script(app: &mut App, script: &str) {
    let down = |b| InputEvent::Button(ButtonEvent::Down(b));
    let up = |b| InputEvent::Button(ButtonEvent::Up(b));
    let hold = obcm_app::DEFAULT_HOLD_MS;
    let mut now: u32 = 100;
    for ch in script.chars() {
        match ch {
            ' ' => {}
            'r' => {
                feed(app, now, vec![InputEvent::Turn(1)]);
                now += 30;
            }
            'l' => {
                feed(app, now, vec![InputEvent::Turn(-1)]);
                now += 30;
            }
            'p' => {
                feed(app, now, vec![down(Button::Encoder)]);
                now += 80;
                feed(app, now, vec![up(Button::Encoder)]);
                now += 30;
            }
            'b' => {
                feed(app, now, vec![down(Button::Back)]);
                now += 80;
                feed(app, now, vec![up(Button::Back)]);
                now += 30;
            }
            'h' => {
                feed(app, now, vec![down(Button::Encoder)]);
                now += hold + 80;
                feed(app, now, vec![]); // a tick past the threshold fires `Hold`
                now += 30;
                feed(app, now, vec![up(Button::Encoder)]);
                now += 30;
            }
            'B' => {
                feed(app, now, vec![down(Button::Back)]);
                now += hold + 80;
                feed(app, now, vec![]); // fires `BackHold`
                now += 30;
                feed(app, now, vec![up(Button::Back)]);
                now += 30;
            }
            other => eprintln!("warning: ignoring unknown --script token '{other}'"),
        }
    }
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\nusage: obcm-sim <map.obcm> [--size WxH] [--scale N] [--png OUT] [--true-color] [--heading DEG] [--gpx TRACK.gpx] [--at SEC] [--center LON,LAT] [--zoom MULT] [--text-demo] [--script TOKENS] [--boot] [--routes-dir DIR] [--import GPX]");
            std::process::exit(2);
        }
    };

    // Slice-1 font/palette preview: render text on a blank panel and exit. Comes
    // before the map read so it needs no map file.
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

    // `--import` converts a GPX into the routes folder — the device's USB-drop path,
    // run on the host. Needs no map, so it comes before the map read.
    if let Some(gpx) = &args.import {
        let dir = args.routes_dir.clone().unwrap_or_else(|| "routes".to_string());
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

    // Validate + log once up front; the borrow ends with this block so `bytes`
    // can move into the GUI (which rebuilds the cheap `Reader` view per frame).
    {
        let reader = Reader::new(&bytes).unwrap_or_else(|e| {
            eprintln!("invalid OBCM file: {e:?}");
            std::process::exit(1);
        });
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
        let reader = Reader::new(&bytes).expect("validated above");
        let (mut cx, mut cy, mut zoom) = initial_camera(&reader, args.width);
        if let Some((lon, lat)) = args.center {
            cx = lon;
            cy = lat;
        }
        zoom *= args.zoom_mul;
        let mut state = AppState::new(cx, cy, zoom);
        // `--heading` renders a rotated (heading-up) frame headlessly; the rotation
        // is derived from the fix's course, so seed one at the map center.
        if let Some(deg) = args.heading {
            state.heading_up = true;
            state.user_fix = Some(Fix { lat: cy, lon: cx, course: Some(deg), speed_mps: None });
        }
        // `--gpx` renders the replayed fix at `--at` (default: track midpoint),
        // a headless way to check the marker sits on the track with a derived
        // heading. The fix's course drives heading-up just like a live replay.
        if let Some(path) = &args.gpx {
            match Track::load(std::path::Path::new(path)) {
                Ok(track) => {
                    let mut player = GpxPlayer::new(track);
                    let at = args.at.unwrap_or(player.duration() / 2.0);
                    player.seek(at);
                    if let Some(fix) = player.poll() {
                        state.heading_up = fix.course.is_some();
                        state.user_fix = Some(fix);
                        state.cam_lon = fix.lon;
                        state.cam_lat = fix.lat;
                    }
                }
                Err(e) => {
                    eprintln!("cannot load GPX: {e}");
                    std::process::exit(1);
                }
            }
        }
        let mut app = if args.boot { App::new_idle(state) } else { App::new(state) };
        // Load the routes folder so the Route menu has real entries and a picked route
        // can be drawn (the device reads the same off its SD card).
        let mut store = RouteStore::open(args.routes_dir.clone().unwrap_or_else(|| "routes".to_string()));
        app.set_routes(store.catalog());
        // Drive the app to a specific screen before snapshotting (e.g. the Menu).
        if let Some(script) = &args.script {
            apply_script(&mut app, script);
        }
        // After the script may have loaded a route, open its geometry for the Map.
        store.sync_active(app.activity.active_route);
        let route_src = store.active_source();
        let route = route_src.as_ref().and_then(|s| RouteReader::open(s).ok());

        let mut fb = Framebuffer::new(args.width, args.height);
        let tc = args.true_color;

        let t0 = Instant::now();
        let stats = app.render_frame(&mut fb, &reader, route.as_ref(), args.width as f32, args.height as f32, |c| {
            color_of(c, tc)
        });
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "rendered {}/{} features ({} chunks, LOD {}, {} dropped) in {ms:.2} ms | spans {:.0}% points {:.0}% rings {:.0}%",
            stats.features_drawn,
            stats.features_tried,
            stats.chunks_visited,
            stats.lod,
            stats.features_dropped,
            stats.span_utilization * 100.0,
            stats.point_utilization * 100.0,
            stats.ring_utilization * 100.0
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
