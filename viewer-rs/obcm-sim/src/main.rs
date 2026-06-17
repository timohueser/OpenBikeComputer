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
//!
//! Interactive: drag to pan, scroll to zoom, Esc/Q to quit.

use std::time::Instant;

use embedded_graphics::pixelcolor::Rgb888;
use obcm_reader::{rgb565_to_device64, rgb565_to_rgb888, Reader};
use obcm_app::{App, AppState, Fix, LocationSource};

mod framebuffer;
mod gpx;
mod gpx_player;
mod gui;
mod sim_location;
use framebuffer::Framebuffer;
use gpx::Track;
use gpx_player::GpxPlayer;

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
            other => {
                if a.map.is_empty() {
                    a.map = other.to_string();
                } else {
                    return Err(format!("unexpected arg: {other}"));
                }
            }
        }
    }
    if a.map.is_empty() {
        return Err("missing map path".into());
    }
    Ok(a)
}

fn color_of(c: u16, true_color: bool) -> Rgb888 {
    let (r, g, b) = if true_color { rgb565_to_rgb888(c) } else { rgb565_to_device64(c) };
    Rgb888::new(r, g, b)
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

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\nusage: obcm-sim <map.obcm> [--size WxH] [--scale N] [--png OUT] [--true-color] [--heading DEG] [--gpx TRACK.gpx] [--at SEC]");
            std::process::exit(2);
        }
    };

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
        let (cx, cy, zoom) = initial_camera(&reader, args.width);
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
        let mut app = App::new(state);
        let mut fb = Framebuffer::new(args.width, args.height);
        let tc = args.true_color;

        let t0 = Instant::now();
        let stats = app.render_frame(&mut fb, &reader, args.width as f32, args.height as f32, |c| {
            color_of(c, tc)
        });
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!("rendered {}/{} features (LOD {}) in {ms:.2} ms", stats.features_drawn, stats.features_tried, stats.lod);

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
