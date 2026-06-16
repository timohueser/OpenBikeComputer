//! OBCM desktop simulator.
//!
//! Renders an .obcm map through `embedded-graphics` — the same drawing path the
//! nRF5340 firmware will use against the LS021B7DD02. Defaults to the device's
//! 240x320 / 64-color (RGB222) look so the preview matches the panel.
//!
//! Usage:
//!   obcm-sim <map.obcm> [--size WxH] [--scale N] [--png OUT.png] [--true-color]
//!
//! Interactive: drag to pan, scroll to zoom, Esc/Q to quit.

use std::time::Instant;

use embedded_graphics::{
    pixelcolor::Rgb888,
    prelude::*,
    primitives::{Polyline, PrimitiveStyle, Rectangle},
};
use embedded_graphics_simulator::{
    sdl2::Keycode, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use obcm::{rgb565_to_device64, rgb565_to_rgb888, BBox, Kind, Reader};

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

/// Screen projection with longitude aspect correction (microdegrees -> pixels).
struct Viewport {
    w: f64,
    h: f64,
    cam_lon: f64,
    cam_lat: f64,
    zoom: f64, // pixels per microdegree
    aspect: f64,
}

impl Viewport {
    fn to_screen(&self, lon: i32, lat: i32) -> (i32, i32) {
        let x = (lon as f64 - self.cam_lon) * self.zoom * self.aspect + self.w / 2.0;
        let y = (self.cam_lat - lat as f64) * self.zoom + self.h / 2.0;
        (x as i32, y as i32)
    }
    fn to_map(&self, x: f64, y: f64) -> (f64, f64) {
        let lon = (x - self.w / 2.0) / (self.zoom * self.aspect) + self.cam_lon;
        let lat = self.cam_lat - (y - self.h / 2.0) / self.zoom;
        (lon, lat)
    }
    fn visible_bbox(&self) -> BBox {
        let (min_lon, max_lat) = self.to_map(0.0, 0.0);
        let (max_lon, min_lat) = self.to_map(self.w, self.h);
        BBox {
            min_lon: min_lon as i32,
            min_lat: min_lat as i32,
            max_lon: max_lon as i32,
            max_lat: max_lat as i32,
        }
    }
}

fn color_of(c: u16, true_color: bool) -> Rgb888 {
    let (r, g, b) = if true_color { rgb565_to_rgb888(c) } else { rgb565_to_device64(c) };
    Rgb888::new(r, g, b)
}

/// Scanline even-odd polygon fill over exterior + interior rings (holes handled
/// naturally by the even-odd rule). `rings` are already-projected screen points.
fn fill_polygon(
    display: &mut SimulatorDisplay<Rgb888>,
    rings: &[Vec<(i32, i32)>],
    color: Rgb888,
    w: i32,
    h: i32,
) {
    let mut ymin = i32::MAX;
    let mut ymax = i32::MIN;
    for ring in rings {
        for &(_, y) in ring {
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    if ymin > ymax {
        return;
    }
    let mut xs: Vec<f32> = Vec::new();
    for y in ymin..=ymax {
        let yc = y as f32 + 0.5;
        xs.clear();
        for ring in rings {
            let n = ring.len();
            if n < 2 {
                continue;
            }
            let mut j = n - 1;
            for i in 0..n {
                let (xi, yi) = (ring[i].0 as f32, ring[i].1 as f32);
                let (xj, yj) = (ring[j].0 as f32, ring[j].1 as f32);
                if (yi <= yc && yc < yj) || (yj <= yc && yc < yi) {
                    xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi));
                }
                j = i;
            }
        }
        if xs.len() < 2 {
            continue;
        }
        xs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            let x0 = (xs[k].ceil() as i32).max(0);
            let x1 = (xs[k + 1].floor() as i32).min(w - 1);
            if x1 >= x0 {
                let _ = display.fill_solid(
                    &Rectangle::new(Point::new(x0, y), Size::new((x1 - x0 + 1) as u32, 1)),
                    color,
                );
            }
            k += 2;
        }
    }
}

fn render(
    display: &mut SimulatorDisplay<Rgb888>,
    reader: &Reader,
    vp: &Viewport,
    bg: Rgb888,
    true_color: bool,
    w: i32,
    h: i32,
) -> (usize, f64) {
    let t0 = Instant::now();
    display.clear(bg).ok();

    // Query + decode visible chunks.
    let view = vp.visible_bbox();
    let mut feats: Vec<obcm::Feature> = Vec::new();
    for (cid, node) in reader.query(&view) {
        feats.extend(reader.decode_chunk(cid, &node));
    }
    // Painter's order by z-index.
    feats.sort_by_key(|f| reader.style(f.style_id).map(|s| s.z_index).unwrap_or(0));

    for f in &feats {
        let style = match reader.style(f.style_id) {
            Some(s) => s,
            None => continue,
        };
        let color = color_of(style.color, true_color);
        match f.kind {
            Kind::Polygon => {
                let mut rings: Vec<Vec<(i32, i32)>> = Vec::with_capacity(1 + f.interiors.len());
                rings.push(f.exterior.iter().map(|&(lon, lat)| vp.to_screen(lon, lat)).collect());
                for inner in &f.interiors {
                    rings.push(inner.iter().map(|&(lon, lat)| vp.to_screen(lon, lat)).collect());
                }
                fill_polygon(display, &rings, color, w, h);
            }
            Kind::Line => {
                let pts: Vec<Point> = f
                    .exterior
                    .iter()
                    .map(|&(lon, lat)| {
                        let (x, y) = vp.to_screen(lon, lat);
                        Point::new(x.clamp(-4 * w, 4 * w), y.clamp(-4 * h, 4 * h))
                    })
                    .collect();
                if pts.len() >= 2 {
                    let weight = style.weight.max(1) as u32;
                    let _ = Polyline::new(&pts)
                        .into_styled(PrimitiveStyle::with_stroke(color, weight))
                        .draw(display);
                }
            }
        }
    }
    (feats.len(), t0.elapsed().as_secs_f64() * 1000.0)
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
        "OBCM v{} | bbox {:?} | chunk_size {} | {} styles",
        reader.version,
        reader.bbox,
        reader.chunk_size,
        (0..=255).filter(|&i| reader.style(i).is_some()).count()
    );

    let (w, h) = (args.width as i32, args.height as i32);
    let b = reader.bbox;
    let center_lat = (b.min_lat + b.max_lat) as f64 / 2.0;
    let span_lon = (b.max_lon - b.min_lon).max(1) as f64;
    let mut vp = Viewport {
        w: args.width as f64,
        h: args.height as f64,
        cam_lon: (b.min_lon + b.max_lon) as f64 / 2.0,
        cam_lat: center_lat,
        zoom: args.width as f64 / span_lon,
        aspect: (center_lat / 1e6).to_radians().cos(),
    };

    // Background = sea color (style 99) if present, else dark grey.
    let bg = reader
        .style(99)
        .map(|s| color_of(s.color, args.true_color))
        .unwrap_or(Rgb888::new(30, 30, 30));

    let mut display = SimulatorDisplay::<Rgb888>::new(Size::new(args.width, args.height));
    let output = OutputSettingsBuilder::new().scale(args.scale).build();

    // Headless mode: render one frame, save PNG, exit.
    if let Some(path) = &args.png {
        let (n, ms) = render(&mut display, &reader, &vp, bg, args.true_color, w, h);
        eprintln!("rendered {n} features in {ms:.2} ms");
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
            let (n, ms) = render(&mut display, &reader, &vp, bg, args.true_color, w, h);
            eprint!("\r{n} features  {ms:.1} ms   ");
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
