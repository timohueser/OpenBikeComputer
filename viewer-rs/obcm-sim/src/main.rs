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

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use embedded_graphics_simulator::{
    sdl2::Keycode, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use obcm::{rgb565_to_device64, rgb565_to_rgb888, MapRenderer, Reader, Viewport};

struct Args {
    map: String,
    width: u32,
    height: u32,
    scale: u32,
    png: Option<String>,
    true_color: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        map: String::new(),
        width: 240,
        height: 320,
        scale: 3,
        png: None,
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

/// Render one frame through the shared renderer and report (features, lod, ms).
fn draw(
    renderer: &mut MapRenderer,
    display: &mut SimulatorDisplay<Rgb888>,
    reader: &Reader,
    vp: &Viewport,
    bg: Rgb888,
    true_color: bool,
) -> (usize, usize, f64) {
    let t0 = Instant::now();
    let stats = renderer.render(display, reader, vp, bg, |c| color_of(c, true_color));
    (stats.features, stats.lod, t0.elapsed().as_secs_f64() * 1000.0)
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

    let b = reader.bbox;
    let center_lat = (b.min_lat + b.max_lat) as f64 / 2.0;
    let span_lon = (b.max_lon - b.min_lon).max(1) as f64;
    let mut vp = Viewport::new(
        args.width as f64,
        args.height as f64,
        (b.min_lon + b.max_lon) as f64 / 2.0,
        center_lat,
        args.width as f64 / span_lon,
    );

    // Background = the backdrop style's color (lowest z-index, by convention the
    // sea/background), else dark grey for an empty style table.
    let bg = reader
        .backdrop_style()
        .map(|s| color_of(s.color, args.true_color))
        .unwrap_or(Rgb888::new(30, 30, 30));

    let mut display = SimulatorDisplay::<Rgb888>::new(Size::new(args.width, args.height));
    let output = OutputSettingsBuilder::new().scale(args.scale).build();
    let mut renderer = MapRenderer::new();

    // Headless mode: render one frame, save PNG, exit.
    if let Some(path) = &args.png {
        let (n, lod, ms) = draw(&mut renderer, &mut display, &reader, &vp, bg, args.true_color);
        eprintln!("rendered {n} features (LOD {lod}) in {ms:.2} ms");
        display
            .to_rgb_output_image(&output)
            .save_png(path)
            .unwrap_or_else(|e| {
                eprintln!("save_png failed: {e}");
                std::process::exit(1);
            });
        eprintln!("wrote {path}");
        return;
    }

    // Interactive window.
    let mut window = Window::new("OBCM Simulator", &output);
    let mut dragging = false;
    let mut last_mouse = (0i32, 0i32);
    let mut dirty = true;
    loop {
        if dirty {
            let (n, lod, ms) = draw(&mut renderer, &mut display, &reader, &vp, bg, args.true_color);
            eprint!("\rLOD {lod}  {n} features  {ms:.1} ms   ");
            dirty = false;
        }
        window.update(&display);
        for event in window.events() {
            match event {
                SimulatorEvent::Quit => return,
                SimulatorEvent::KeyDown { keycode, .. } => {
                    if keycode == Keycode::Escape || keycode == Keycode::Q {
                        return;
                    }
                }
                SimulatorEvent::MouseButtonDown { point, .. } => {
                    dragging = true;
                    last_mouse = (point.x, point.y);
                }
                SimulatorEvent::MouseButtonUp { .. } => dragging = false,
                SimulatorEvent::MouseMove { point } => {
                    if dragging {
                        let dx = (point.x - last_mouse.0) as f64;
                        let dy = (point.y - last_mouse.1) as f64;
                        vp.cam_lon -= dx / (vp.zoom * vp.aspect);
                        vp.cam_lat += dy / vp.zoom;
                        vp.refresh_aspect();
                        dirty = true;
                    }
                    last_mouse = (point.x, point.y);
                }
                SimulatorEvent::MouseWheel { scroll_delta, .. } => {
                    let factor = if scroll_delta.y > 0 { 1.2 } else { 1.0 / 1.2 };
                    let (mx, my) = (last_mouse.0 as f64, last_mouse.1 as f64);
                    let (olon, olat) = vp.to_map(mx, my);
                    vp.zoom *= factor;
                    let (nlon, nlat) = vp.to_map(mx, my);
                    vp.cam_lon += olon - nlon;
                    vp.cam_lat += olat - nlat;
                    dirty = true;
                }
                _ => {}
            }
        }
    }
}
