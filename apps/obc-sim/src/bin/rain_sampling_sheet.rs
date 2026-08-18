//! Side-by-side renderer for the #1185 rain-smoothing round.
//!
//! Draws the **same frame** — same map, same camera, same zoom, same heading — once per
//! [`RainSampling`] mode, straight through the production `obc-render` path, and writes one
//! 240 x 320 PNG per mode plus a labelled contact sheet. The rain comes from a real baked
//! **OBCG** product object rather than a synthetic pattern: the complaint that opened this round
//! ("1 km square blobs … very blocky") is about real radar, so a demo pattern cannot answer it.
//!
//! Reading OBCG directly, instead of going through an OBCW bundle, is deliberate — OBCW is the
//! transport container for the same 4-bit cells (`obc_formats::precip4` is shared by both), so
//! this puts the actual upstream radar in front of the actual renderer with nothing in between
//! and no network.
//!
//! **Provisional (#1185).** This binary exists to produce the comparison images; it goes when the
//! round closes.
//!
//! ```text
//! # what is in this product, and where is it raining?
//! cargo run --release -p obc-sim --bin rain_sampling_sheet -- --obcg <f0.obcg> --survey
//!
//! # the four options, one frame, one directory
//! cargo run --release -p obc-sim --bin rain_sampling_sheet -- \
//!     --obcg <f0.obcg> --map <fixture-root>/sim-grimsel/grimsel.obcm \
//!     --center 7900000,48100000 --mpp 40 --label riding --out-dir /tmp/sheet
//! ```

#[allow(dead_code)]
#[path = "../framebuffer.rs"]
mod framebuffer;
use std::{collections::BTreeMap, path::PathBuf, process};

use embedded_graphics::{pixelcolor::Rgb888, prelude::*};
use framebuffer::Framebuffer;
use obc_formats::{obcg, precip4};
use obc_reader::{rgb565_to_device64, MapCache, MapTables, Reader, SliceSource};
use obc_render::{
    draw_text, Font, RainGrid, RainOverlaySource, RainSampling, RenderConfig, RenderScratch, RenderStats, TextAlign,
    Viewport, RAIN_TILE_CELLS,
};

/// Repeats behind `--bench`: the overlay's own wall time, floored over identical frames.
const BENCH_ROUNDS: u32 = 60;

const PANEL_W: u32 = 240;
const PANEL_H: u32 = 320;

/// The modes rendered, in the order the round presents them.
const MODES: [(RainSampling, &str, &str); 4] = [
    (RainSampling::Nearest, "a-nearest", "A  nearest (today)"),
    (RainSampling::Bilinear, "b-bilinear", "B  bilinear"),
    (RainSampling::Jitter, "c-jitter", "C  jitter half-cell"),
    (RainSampling::EdgeSoften, "d-edge", "D  edge soften 1/4"),
];

// ---------------------------------------------------------------------------------------------
// A real OBCG product object, served through the renderer's own overlay seam.
// ---------------------------------------------------------------------------------------------

/// Whole-object OBCG reader exposing a baked product as a [`RainOverlaySource`].
///
/// OBCG picks a per-grid power-of-two tile edge while the overlay seam speaks OBCW's fixed
/// 16 x 16, so this re-tiles: it decodes the OBCG tile a requested 16 x 16 window falls in
/// (caching the last one, which is what makes a whole frame cheap) and copies the window out.
/// Cells outside the declared grid are `no-data`, exactly as both formats mandate.
struct ObcgSource {
    bytes: Vec<u8>,
    header: obcg::Header,
    /// Decoded OBCG tile cache: index + cells.
    cached: Option<(u32, Vec<u8>)>,
    decodes: u32,
}

impl ObcgSource {
    fn open(path: &PathBuf) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut scratch = vec![0u8; obcg::MAX_PAGE_BYTES.max(precip4::MAX_CELLS)];
        let header = obcg::validate(&bytes, &mut scratch).map_err(|e| format!("{}: {e:?}", path.display()))?;
        Ok(Self { bytes, header, cached: None, decodes: 0 })
    }

    /// The intensity of grid cell `(col, row)`, row 0 south. `None` outside the declared grid.
    fn cell(&mut self, col: u32, row: u32) -> Option<u8> {
        let (tc, tr) = self.header.tile_of_cell(col, row)?;
        let index = self.header.tile_index(tc, tr)?;
        let inside = self.header.cell_index_in_tile(col, row)?;
        if self.cached.as_ref().map(|(i, _)| *i) != Some(index) {
            let mut cells = vec![0u8; self.header.tile_cells()];
            let entry_at = self.header.entry_offset(index)? as usize;
            let page = self.header.page_offset(self.header.page_of_entry(index))? as usize;
            let page_bytes = &self.bytes[page..page + self.header.page_bytes() as usize];
            let within = (entry_at - page) / obcg::DIRECTORY_ENTRY_LEN;
            let entry = obcg::decode_entry(page_bytes, within).ok()?;
            // OBCG directory offsets are absolute in the object, not data-section relative.
            let start = entry.data_offset as usize;
            let payload = &self.bytes[start..start + entry.encoded_len as usize];
            obcg::decode_tile_cells(&self.header, &entry, payload, &mut cells).ok()?;
            self.decodes += 1;
            self.cached = Some((index, cells));
        }
        self.cached.as_ref().map(|(_, cells)| cells[inside])
    }

    /// Decode OBCG tile `index` straight into `out` (`tile_cells()` long), bypassing the
    /// single-entry cache — the survey walks every tile once and would thrash it.
    fn decode_into(&mut self, index: u32, out: &mut [u8]) -> bool {
        let Some(entry_at) = self.header.entry_offset(index) else { return false };
        let Some(page) = self.header.page_offset(self.header.page_of_entry(index)) else { return false };
        let (entry_at, page) = (entry_at as usize, page as usize);
        let page_bytes = &self.bytes[page..page + self.header.page_bytes() as usize];
        let within = (entry_at - page) / obcg::DIRECTORY_ENTRY_LEN;
        let Ok(entry) = obcg::decode_entry(page_bytes, within) else { return false };
        // OBCG directory offsets are absolute in the object, not data-section relative.
        let start = entry.data_offset as usize;
        let payload = &self.bytes[start..start + entry.encoded_len as usize];
        self.decodes += 1;
        obcg::decode_tile_cells(&self.header, &entry, payload, out).is_ok()
    }

    fn grid_of(&self) -> RainGrid {
        RainGrid {
            west_udeg: self.header.west_lon_udeg,
            south_udeg: self.header.south_lat_udeg,
            east_udeg: self.header.east_lon_udeg() as i32,
            north_udeg: self.header.north_lat_udeg() as i32,
            width_cells: self.header.width.min(u16::MAX as u32) as u16,
            height_cells: self.header.height.min(u16::MAX as u32) as u16,
        }
    }
}

/// A window of a product, presented to the renderer as a grid in its own right.
///
/// A CONUS or Europe-wide product is far larger than the 65,535-cell axis the OBCW frame geometry
/// (and so [`RainGrid`]) carries, which is exactly why the real client cuts a corridor around the
/// rider before it ever reaches the device. This does the same cut, with no resampling: the window
/// is a whole number of source cells, so **every cell the renderer sees is a byte-exact upstream
/// cell** and the 1 km geometry the round is judging is untouched.
struct WindowSource {
    product: ObcgSource,
    col0: u32,
    row0: u32,
    grid: RainGrid,
}

impl WindowSource {
    fn new(product: ObcgSource, col0: u32, row0: u32, w: u16, h: u16) -> Self {
        let full = product.grid_of();
        let (dlon, dlat) = (product.header.cell_lon_udeg as i64, product.header.cell_lat_udeg as i64);
        let west = full.west_udeg as i64 + col0 as i64 * dlon;
        let south = full.south_udeg as i64 + row0 as i64 * dlat;
        let grid = RainGrid {
            west_udeg: west as i32,
            south_udeg: south as i32,
            east_udeg: (west + w as i64 * dlon) as i32,
            north_udeg: (south + h as i64 * dlat) as i32,
            width_cells: w,
            height_cells: h,
        };
        Self { product, col0, row0, grid }
    }
}

impl RainOverlaySource for WindowSource {
    fn grid(&self) -> Option<RainGrid> {
        Some(self.grid)
    }

    fn tile(&mut self, tile_index: u32, out: &mut [u8; RAIN_TILE_CELLS]) -> bool {
        let tile_cols = (self.grid.width_cells as u32).div_ceil(16);
        let (tr, tc) = (tile_index / tile_cols, tile_index % tile_cols);
        for cy in 0..16u32 {
            for cx in 0..16u32 {
                let (row, col) = (tr * 16 + cy, tc * 16 + cx);
                // Padding beyond the declared window is no-data, as OBCW section 5 mandates.
                let v = if row < self.grid.height_cells as u32 && col < self.grid.width_cells as u32 {
                    self.product.cell(self.col0 + col, self.row0 + row).unwrap_or(precip4::INTENSITY_NODATA)
                } else {
                    precip4::INTENSITY_NODATA
                };
                out[(cy * 16 + cx) as usize] = v;
            }
        }
        true
    }
}

/// A single `.obcm` opened for rendering.
///
/// The simulator's own `map_set::LoadedMap` does this and more (terrain sidecars, the session-long
/// tables and cache); none of it is wanted here, and pulling that module into a `bin` drags its
/// `#[cfg(test)]` block along with it. One file, one reader.
struct Basemap {
    source: &'static SliceSource<'static>,
    tables: &'static MapTables,
    cache: &'static MapCache,
}

impl Basemap {
    fn open(path: &str) -> Result<Self, String> {
        let bytes: &'static [u8] =
            Box::leak(std::fs::read(path).map_err(|e| format!("{path}: {e}"))?.into_boxed_slice());
        let source: &'static SliceSource<'static> = Box::leak(Box::new(SliceSource(bytes)));
        let tables: &'static MapTables =
            Box::leak(Box::new(MapTables::parse(source).map_err(|e| format!("{path}: not an OBCM map ({e:?})"))?));
        let cache: &'static MapCache = Box::leak(Box::new(MapCache::new()));
        Ok(Self { source, tables, cache })
    }

    fn reader(&self) -> Reader<'_> {
        Reader::new(self.source, self.tables, self.cache)
    }
}

// ---------------------------------------------------------------------------------------------
// Survey: what is in the object, and where is the weather?
// ---------------------------------------------------------------------------------------------

/// Print the frame's geometry, its intensity histogram, and the wettest windows — so a frame is
/// aimed at real rain rather than at an empty quarter of a continent.
fn survey(product: &mut ObcgSource, window: u32) {
    let h = product.header;
    let grid = product.grid_of();
    println!(
        "flags {:#06x} | {} x {} cells of {} m | valid_at {} ref {}",
        h.flags, h.width, h.height, h.cell_size_m, h.valid_at, h.reference_time
    );
    println!(
        "bbox lon {:.4}..{:.4} lat {:.4}..{:.4} | cell {:.5} x {:.5} deg | tile edge {}",
        grid.west_udeg as f64 / 1e6,
        grid.east_udeg as f64 / 1e6,
        grid.south_udeg as f64 / 1e6,
        grid.north_udeg as f64 / 1e6,
        h.cell_lon_udeg as f64 / 1e6,
        h.cell_lat_udeg as f64 / 1e6,
        h.tile_edge
    );

    // Exhaustive, in tile order: every declared cell is counted exactly once and every OBCG tile
    // decodes exactly once. (A strided sample over a continent-sized product reported zero rain
    // where there plainly was some — sampling is not good enough to aim a comparison frame.)
    let mut hist: BTreeMap<u8, u64> = BTreeMap::new();
    let windows_across = h.width.div_ceil(window);
    let mut scores = vec![0u64; (windows_across * h.height.div_ceil(window)) as usize];
    let edge = h.tile_edge as u32;
    let mut cells = vec![0u8; h.tile_cells()];
    for index in 0..h.tile_count() {
        if !product.decode_into(index, &mut cells) {
            continue;
        }
        let (tr, tc) = (index / h.tile_cols(), index % h.tile_cols());
        for cy in 0..edge {
            let row = tr * edge + cy;
            if row >= h.height {
                break;
            }
            for cx in 0..edge {
                let col = tc * edge + cx;
                if col >= h.width {
                    break;
                }
                let v = cells[(cy * edge + cx) as usize];
                *hist.entry(v).or_default() += 1;
                if (1..=12).contains(&v) {
                    // Weight by band so a storm core outranks a wide drizzle sheet.
                    scores[((row / window) * windows_across + col / window) as usize] += v as u64 * v as u64;
                }
            }
        }
    }
    let total: u64 = hist.values().sum();
    println!("\nintensity histogram over all {total} declared cells:");
    for (code, n) in &hist {
        let name = match code {
            0 => "dry",
            15 => "NO-DATA",
            13 | 14 => "reserved",
            _ => "rain",
        };
        println!("  {code:>2} {name:<9} {n:>10}  {:>5.2}%", *n as f64 * 100.0 / total as f64);
    }

    let mut best: Vec<(u64, u32, u32)> = scores
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, (i as u32 % windows_across) * window, (i as u32 / windows_across) * window))
        .collect();
    best.sort_unstable_by_key(|&(score, _, _)| core::cmp::Reverse(score));
    println!("\nwettest {window}-cell windows (col0, row0 -> centre lon,lat):");
    for &(score, c, r) in best.iter().take(8) {
        let lon = grid.west_udeg as f64 + (c + window / 2) as f64 * h.cell_lon_udeg as f64;
        let lat = grid.south_udeg as f64 + (r + window / 2) as f64 * h.cell_lat_udeg as f64;
        println!("  score {score:>8}  col {c:>5} row {r:>5}  --center {:.0},{:.0}", lon, lat);
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

struct StdClock(std::time::Instant);
impl obc_render::Clock for StdClock {
    fn now_us(&self) -> u64 {
        self.0.elapsed().as_micros() as u64
    }
}

/// Render one frame in one mode. Returns the framebuffer and the overlay's stats.
#[allow(clippy::too_many_arguments)]
fn render_one(
    map: &Basemap,
    source: &mut dyn RainOverlaySource,
    centre: (i32, i32),
    mpp: f32,
    heading_deg: Option<f32>,
    mode: RainSampling,
) -> (Framebuffer, RenderStats) {
    let mut fb = Framebuffer::new(PANEL_W, PANEL_H);
    let mut scratch = Box::new(RenderScratch::new());
    let zoom = obc_render::zoom_for_mpp(mpp);
    let vp = match heading_deg {
        Some(deg) => Viewport::new_rotated(PANEL_W as f32, PANEL_H as f32, centre.0, centre.1, zoom, deg.to_radians()),
        None => Viewport::new(PANEL_W as f32, PANEL_H as f32, centre.0, centre.1, zoom),
    };
    // The panel's own RGB222 quantizer, so these frames are what the glass shows.
    let color_fn = |c: u16| {
        let (r, g, b) = rgb565_to_device64(c);
        Rgb888::new(r, g, b)
    };
    let clock = StdClock(std::time::Instant::now());
    let cfg = RenderConfig::default();
    let reader = map.reader();
    let bg = color_fn(reader.backdrop_style().map_or(0xFFFF, |s| s.color));
    let stats = scratch.render_rain_sampled_timed(&mut fb, &reader, &vp, bg, cfg, Some(source), mode, color_fn, &clock);
    (fb, stats)
}

/// Height of a contact sheet's caption strip, in px.
const CAPTION_H: u32 = 26;
/// Gutter between panels on a contact sheet.
const GUTTER: u32 = 8;

/// Render one caption strip through the production text renderer, so the labels on the sheet are
/// the device's own font rather than something a host toolchain invented.
fn caption(text: &str, width: u32) -> Framebuffer {
    let mut fb = Framebuffer::new(width, CAPTION_H);
    let white = Rgb888::new(255, 255, 255);
    let ink = Rgb888::new(0, 0, 0);
    let _ = fb.clear(white);
    draw_text(&mut fb, text, Point::new(2, 2), Font::Label, TextAlign::Left, ink);
    fb
}

/// Glue the four panels into one labelled strip — a PR body can then carry one image per scene
/// instead of four, and nobody can accidentally compare frames from different cameras.
fn contact_sheet(panels: &[(Framebuffer, &str)], scene: &str, path: &std::path::Path) -> Result<(), String> {
    let n = panels.len() as u32;
    let w = n * PANEL_W + (n + 1) * GUTTER;
    let h = PANEL_H + CAPTION_H * 2 + 3 * GUTTER;
    let mut out = image::RgbImage::from_pixel(w, h, image::Rgb([255, 255, 255]));

    let title = caption(scene, w);
    image::imageops::replace(&mut out, &to_image(&title)?, 0, GUTTER as i64);

    for (i, (fb, label)) in panels.iter().enumerate() {
        let x = (GUTTER + i as u32 * (PANEL_W + GUTTER)) as i64;
        image::imageops::replace(&mut out, &to_image(fb)?, x, (CAPTION_H + 2 * GUTTER) as i64);
        let strip = caption(label, PANEL_W);
        image::imageops::replace(&mut out, &to_image(&strip)?, x, (CAPTION_H + PANEL_H + 3 * GUTTER) as i64);
    }
    out.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn to_image(fb: &Framebuffer) -> Result<image::RgbImage, String> {
    image::RgbImage::from_raw(fb.width(), fb.height(), fb.as_rgb888().to_vec())
        .ok_or_else(|| "framebuffer size mismatch".to_string())
}

fn save(fb: &Framebuffer, path: &std::path::Path) -> Result<(), String> {
    let img = image::RgbImage::from_raw(fb.width(), fb.height(), fb.as_rgb888().to_vec())
        .ok_or("framebuffer size mismatch")?;
    img.save(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Print the raw cell field around a coordinate as ASCII — one character per provider cell, `.`
/// dry, `-` no-data, `1..9`/`a`/`b`/`c` the bands. This is the ground truth a comparison frame is
/// drawn from: it is what "1 km blobs" looks like before any renderer touches it.
fn probe(product: &mut ObcgSource, centre: (i32, i32), cols: u32, rows: u32) {
    let h = product.header;
    let full = product.grid_of();
    let cc = (centre.0 as i64 - full.west_udeg as i64) / h.cell_lon_udeg as i64;
    let cr = (centre.1 as i64 - full.south_udeg as i64) / h.cell_lat_udeg as i64;
    println!(
        "cells around {:.4},{:.4} (col {cc}, row {cr}) — north at top:",
        centre.0 as f64 / 1e6,
        centre.1 as f64 / 1e6
    );
    for r in (0..rows).rev() {
        let row = cr - rows as i64 / 2 + r as i64;
        let mut line = String::new();
        for c in 0..cols {
            let col = cc - cols as i64 / 2 + c as i64;
            let ch = if row < 0 || col < 0 {
                ' '
            } else {
                match product.cell(col as u32, row as u32) {
                    None => ' ',
                    Some(0) => '.',
                    Some(15) => '-',
                    Some(v) if v <= 9 => (b'0' + v) as char,
                    Some(v) => (b'a' + v - 10) as char,
                }
            };
            line.push(ch);
        }
        println!("  {line}");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut obcg_path: Option<PathBuf> = None;
    let mut map_path: Option<String> = None;
    let mut out_dir = PathBuf::from(".");
    let mut label = String::from("frame");
    let mut centre: Option<(i32, i32)> = None;
    let mut mpp = 40.0f32;
    let mut heading: Option<f32> = None;
    let mut window_cells: u16 = 512;
    let mut do_survey = false;
    let mut do_probe = false;
    let mut bench = false;

    let mut i = 0;
    while i < args.len() {
        let take = |i: &mut usize| -> String {
            *i += 1;
            args.get(*i).cloned().unwrap_or_else(|| {
                eprintln!("missing value for {}", args[*i - 1]);
                process::exit(2)
            })
        };
        match args[i].as_str() {
            "--obcg" => obcg_path = Some(PathBuf::from(take(&mut i))),
            "--map" => map_path = Some(take(&mut i)),
            "--out-dir" => out_dir = PathBuf::from(take(&mut i)),
            "--label" => label = take(&mut i),
            "--mpp" => mpp = take(&mut i).parse().expect("--mpp"),
            "--heading" => heading = Some(take(&mut i).parse().expect("--heading")),
            "--window-cells" => window_cells = take(&mut i).parse().expect("--window-cells"),
            "--survey" => do_survey = true,
            "--probe" => do_probe = true,
            "--bench" => bench = true,
            "--center" => {
                let v = take(&mut i);
                let (lon, lat) = v.split_once(',').expect("--center LON,LAT in microdegrees");
                centre = Some((lon.trim().parse().expect("lon"), lat.trim().parse().expect("lat")));
            }
            other => {
                eprintln!("unknown flag {other}");
                process::exit(2);
            }
        }
        i += 1;
    }

    let Some(obcg_path) = obcg_path else {
        eprintln!("usage: rain_sampling_sheet --obcg <product.obcg> [--survey] [--map M.obcm]");
        eprintln!("       [--center LON,LAT] [--mpp M] [--heading DEG] [--label NAME] [--out-dir DIR]");
        process::exit(2);
    };
    let mut product = ObcgSource::open(&obcg_path).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1)
    });

    if do_survey {
        survey(&mut product, window_cells as u32);
        return;
    }
    if do_probe {
        let Some(c) = centre else {
            eprintln!("--probe needs --center LON,LAT");
            process::exit(2);
        };
        probe(&mut product, c, 96, 48);
        return;
    }

    let Some(centre) = centre else {
        eprintln!("--center LON,LAT (microdegrees) is required unless --survey");
        process::exit(2);
    };

    // Cut the corridor window the same way the real client does: whole source cells around the
    // camera, so the renderer sees byte-exact upstream 1 km cells.
    let full = product.grid_of();
    let (dlon, dlat) = (product.header.cell_lon_udeg as i64, product.header.cell_lat_udeg as i64);
    let col_c = (centre.0 as i64 - full.west_udeg as i64) / dlon;
    let row_c = (centre.1 as i64 - full.south_udeg as i64) / dlat;
    let col0 = (col_c - window_cells as i64 / 2).clamp(0, (product.header.width as i64 - 1).max(0)) as u32;
    let row0 = (row_c - window_cells as i64 / 2).clamp(0, (product.header.height as i64 - 1).max(0)) as u32;
    let w = window_cells.min((product.header.width - col0) as u16);
    let h = window_cells.min((product.header.height - row0) as u16);
    eprintln!(
        "window: {w} x {h} cells from (col {col0}, row {row0}) of {} x {}",
        product.header.width, product.header.height
    );

    // A map is always loaded — the basemap is half of what a rider judges. Where the product's
    // ground has no registered map (MRMS is CONUS-only and every registered map is
    // Alpine/Rhine), the camera simply sits off that map and the basemap renders empty: the raster
    // is then judged on its own, which the sheet's caption must say.
    let Some(map_path) = map_path else {
        eprintln!("--map PATH is required when rendering (run `obc fixtures sync sim` first)");
        process::exit(2);
    };
    let map = Basemap::open(&map_path).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1)
    });

    std::fs::create_dir_all(&out_dir).expect("out dir");
    let mut panels: Vec<(Framebuffer, &str)> = Vec::new();
    for (mode, slug, title) in MODES {
        let mut source = WindowSource::new(ObcgSource::open(&obcg_path).expect("reopen"), col0, row0, w, h);
        let (fb, stats) = render_one(&map, &mut source, centre, mpp, heading, mode);
        let path = out_dir.join(format!("{label}-{slug}.png"));
        save(&fb, &path).unwrap_or_else(|e| {
            eprintln!("{e}");
            process::exit(1)
        });
        println!(
            "{title:<26} {:>6} px painted  {:>3} tiles decoded  {:>6} us  {}{}",
            stats.rain_px,
            stats.rain_tiles,
            stats.rain_us,
            path.display(),
            if stats.rain_out_of_regime { "  [OUT OF REGIME]" } else { "" }
        );
        panels.push((fb, title));
    }
    if bench {
        // One frame's overlay time is mostly noise on a host; the useful number is the floor over
        // many repeats of the identical frame, which is what the device's own budget would see.
        println!("\nbench: best of {BENCH_ROUNDS} identical frames (rain_us, overlay only)");
        for (mode, _, title) in MODES {
            let mut best = u32::MAX;
            let mut tiles = 0;
            for _ in 0..BENCH_ROUNDS {
                let mut source = WindowSource::new(ObcgSource::open(&obcg_path).expect("reopen"), col0, row0, w, h);
                let (_, stats) = render_one(&map, &mut source, centre, mpp, heading, mode);
                best = best.min(stats.rain_us);
                tiles = stats.rain_tiles;
            }
            println!("  {title:<22} {best:>5} us   {tiles:>3} tiles");
        }
    }

    let sheet = out_dir.join(format!("{label}-sheet.png"));
    let scene = format!("{label}   {mpp:.0} m/px   1 km cells   same camera in all four");
    contact_sheet(&panels, &scene, &sheet).unwrap_or_else(|e| {
        eprintln!("{e}");
        process::exit(1)
    });
    println!("sheet: {}", sheet.display());
}
