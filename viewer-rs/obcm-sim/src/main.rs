//! OBCM desktop simulator — host shell around the shared `obcm` renderer.
//!
//! All map drawing (projection, LOD selection, polygon fill, lines) lives in
//! `obcm::render`, the same code the nRF5340 firmware will run against the
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
use obcm::{rgb565_to_device64, rgb565_to_rgb888, Reader};
use obcm_app::{App, AppState};

mod framebuffer;
mod gui;
mod sim_location;
use framebuffer::Framebuffer;

struct Args {
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    /// Launch the GUI, save its first composited frame to this path, then exit.
    screenshot: Option<String>,
    true_color: bool,
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
fn initial_camera(reader: &Reader, width: u32) -> (f64, f64, f64) {
    let b = reader.bbox;
    let cam_lon = (b.min_lon + b.max_lon) as f64 / 2.0;
    let cam_lat = (b.min_lat + b.max_lat) as f64 / 2.0;
    let span_lon = (b.max_lon - b.min_lon).max(1) as f64;
    (cam_lon, cam_lat, width as f64 / span_lon)
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
            eprintln!("error: {e}\nusage: obcm-sim <map.obcm> [--size WxH] [--scale N] [--png OUT] [--true-color]");
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
        let mut app = App::new(AppState::new(cx, cy, zoom));
        let mut fb = Framebuffer::new(args.width, args.height);
        let tc = args.true_color;

        let t0 = Instant::now();
        let stats = app.render_frame(&mut fb, &reader, args.width as f64, args.height as f64, |c| {
            color_of(c, tc)
        });
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!("rendered {} features (LOD {}) in {ms:.2} ms", stats.features, stats.lod);

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
