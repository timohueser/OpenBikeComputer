//! Crops + hourly → one OBCW bundle, through the shared `obc_formats::obcw` encoder.
//!
//! The one interesting decision is the **common window**. OBCW states a single geographic window
//! in its header and lets each frame declare its own cell count over it, which is what makes a
//! composed product (a 1 km radar observation followed by 3 km model frames) representable at
//! all. The window is therefore built on the **coarsest** crop's lattice: only a coarse lattice's
//! own cells can tile a window exactly, and deriving it from a fine frame would make every model
//! frame untileable and drop the whole forward half of the timeline.
//!
//! A frame whose lattice does not tile that window exactly is **dropped and counted**, never
//! resampled. That is the epic's no-fabricated-precision rule taken literally: a frame we cannot
//! place on the grid without inventing cell edges does not go in the bundle.

use obc_formats::obcw::{
    encode_format, encoded_len, BundleInput, RainFrameInput, HOURLY_COUNT, QUALITY_FORECAST, QUALITY_OBSERVED,
    QUALITY_PARTIAL_COVERAGE, TILE_CELLS,
};
use obc_formats::precip4::{INTENSITY_NODATA, TILE_EDGE};

use crate::corridor::Crop;
use crate::manifest::SourceClass;
use crate::met::Hourly;

/// The OBCW v1 producer cap (`OBCW_Spec.md` §2). The window shrinks until the bundle fits.
pub const PRODUCER_CAP: usize = 65_536;
/// How many times the window may shrink before the builder starts dropping frames instead.
pub const MAX_SHRINK_ATTEMPTS: u32 = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Even one frame over a one-cell window would not fit — structurally impossible, but the
    /// builder refuses rather than emitting a bundle that violates the cap.
    TooLarge,
    Encode(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::TooLarge => write!(f, "no window small enough fits the OBCW producer cap"),
            BuildError::Encode(why) => write!(f, "OBCW encode: {why}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildReport {
    /// Frames whose lattice could not tile the common window. Never resampled.
    pub dropped_incompatible: u32,
    /// Frames dropped, furthest-future first, to fit the producer cap.
    pub dropped_oversize: u32,
    /// How many times the window shrank.
    pub shrinks: u32,
    pub frames: u32,
    pub window_width: u32,
    pub window_height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    south: i64,
    west: i64,
    lat_stride: i64,
    lon_stride: i64,
    cols: u32,
    rows: u32,
}

impl Window {
    fn north(&self) -> i64 {
        self.south + i64::from(self.rows) * self.lat_stride
    }

    fn east(&self) -> i64 {
        self.west + i64::from(self.cols) * self.lon_stride
    }
}

/// Build the bundle. `anchor` is the rider's `(lat, lon)` in microdegrees — a shrunken window
/// re-centres on it, because a window that shrinks toward the corridor midpoint walks off the
/// back of a fast rider.
pub fn build(
    generation: u32,
    request_id: u32,
    generated_at: i64,
    anchor: (i32, i32),
    crops: &[Crop],
    hourly: &Hourly,
) -> Result<(Vec<u8>, BuildReport), BuildError> {
    let mut report = BuildReport::default();

    // One frame per timestamp, ascending — OBCW requires strictly increasing `valid_at`.
    let mut usable: Vec<&Crop> = crops.iter().collect();
    usable.sort_by_key(|crop| crop.valid_at);
    usable.dedup_by_key(|crop| crop.valid_at);

    let mut window = match initial_window(&usable) {
        Some(window) => window,
        // No rain at all: an hourly-only bundle still states a region, so the screens can say
        // *hourly only here* instead of guessing. One cell spanning the anchor's degree.
        None => Window {
            south: i64::from(anchor.0) - 500_000,
            west: i64::from(anchor.1) - 500_000,
            lat_stride: 1_000_000,
            lon_stride: 1_000_000,
            cols: 1,
            rows: 1,
        },
    };

    let mut attempt = 0u32;
    loop {
        let mut incompatible = 0u32;
        let mut partial_frames = Vec::new();
        for crop in &usable {
            match rain_frame(crop, &window) {
                Some(frame) => partial_frames.push(frame),
                None => incompatible += 1,
            }
        }
        let frames: Vec<RainFrameInput<'_>> = partial_frames
            .iter()
            .map(|(valid_at, width, height, cell_size_m, quality, tiles)| RainFrameInput {
                valid_at: *valid_at,
                width: *width,
                height: *height,
                cell_size_m: *cell_size_m,
                quality_flags: *quality,
                tiles,
            })
            .collect();
        let valid_from = hourly.valid_from;
        let valid_until = frames
            .iter()
            .map(|frame| frame.valid_at)
            .max()
            .unwrap_or(valid_from)
            .max(valid_from + HOURLY_COUNT as i64 * 3_600);
        let input = BundleInput {
            generation,
            request_id,
            generated_at,
            valid_from,
            valid_until,
            south_lat_udeg: window.south as i32,
            west_lon_udeg: window.west as i32,
            north_lat_udeg: window.north() as i32,
            east_lon_udeg: window.east() as i32,
            grid_origin_lat_udeg: window.south as i32,
            grid_origin_lon_udeg: window.west as i32,
            flags: 0,
            hourly: &hourly.records,
            frames: &frames,
        };
        let length = encoded_len(&input).map_err(|error| BuildError::Encode(format!("{error:?}")))? as usize;
        if length <= PRODUCER_CAP {
            let mut bytes = vec![0u8; length];
            let written =
                encode_format(&input, &mut bytes).map_err(|error| BuildError::Encode(format!("{error:?}")))?;
            bytes.truncate(written);
            report.dropped_incompatible = incompatible;
            report.frames = frames.len() as u32;
            report.window_width = window.cols;
            report.window_height = window.rows;
            return Ok((bytes, report));
        }
        drop(frames);
        drop(partial_frames);
        // Trimming the window beats dropping a frame: a shorter corridor still answers every
        // timestamp, while a missing frame puts a hole in the two-hour timeline.
        if attempt < MAX_SHRINK_ATTEMPTS {
            if let Some(shrunk) = shrink(&window, anchor) {
                window = shrunk;
                attempt += 1;
                report.shrinks += 1;
                continue;
            }
        }
        if usable.len() > 1 {
            usable.pop(); // the furthest-future frame goes first
            report.dropped_oversize += 1;
            attempt = 0;
            continue;
        }
        return Err(BuildError::TooLarge);
    }
}

/// The coarsest crop's own extent: the only lattice every other crop has a chance of tiling.
fn initial_window(crops: &[&Crop]) -> Option<Window> {
    let coarsest = crops.iter().max_by_key(|crop| {
        (u64::from(crop.cell_lat_udeg) * u64::from(crop.cell_lon_udeg), std::cmp::Reverse(crop.valid_at))
    })?;
    Some(Window {
        south: coarsest.south_udeg,
        west: coarsest.west_udeg,
        lat_stride: i64::from(coarsest.cell_lat_udeg),
        lon_stride: i64::from(coarsest.cell_lon_udeg),
        cols: coarsest.width,
        rows: coarsest.height,
    })
}

fn shrink(window: &Window, anchor: (i32, i32)) -> Option<Window> {
    let drop_cols = (window.cols / 8).max(1);
    let drop_rows = (window.rows / 8).max(1);
    let cols = window.cols.checked_sub(drop_cols).filter(|cols| *cols > 0)?;
    let rows = window.rows.checked_sub(drop_rows).filter(|rows| *rows > 0)?;
    let anchor_col = ((i64::from(anchor.1) - window.west) / window.lon_stride).clamp(0, i64::from(window.cols) - 1);
    let anchor_row = ((i64::from(anchor.0) - window.south) / window.lat_stride).clamp(0, i64::from(window.rows) - 1);
    let first_col = (anchor_col - i64::from(cols) / 2).clamp(0, i64::from(window.cols - cols));
    let first_row = (anchor_row - i64::from(rows) / 2).clamp(0, i64::from(window.rows - rows));
    Some(Window {
        south: window.south + first_row * window.lat_stride,
        west: window.west + first_col * window.lon_stride,
        lat_stride: window.lat_stride,
        lon_stride: window.lon_stride,
        cols,
        rows,
    })
}

type PreparedFrame = (i64, u16, u16, u16, u32, Vec<[u8; TILE_CELLS]>);

/// Lay one crop onto the common window as 16 × 16 OBCW tiles, or refuse it.
fn rain_frame(crop: &Crop, window: &Window) -> Option<PreparedFrame> {
    let lat_stride = i64::from(crop.cell_lat_udeg);
    let lon_stride = i64::from(crop.cell_lon_udeg);
    // Exact tiling or nothing: the window's origin must sit on this crop's lattice and its
    // extent must be a whole number of this crop's cells.
    if (window.south - crop.south_udeg).rem_euclid(lat_stride) != 0
        || (window.west - crop.west_udeg).rem_euclid(lon_stride) != 0
        || (window.north() - window.south).rem_euclid(lat_stride) != 0
        || (window.east() - window.west).rem_euclid(lon_stride) != 0
    {
        return None;
    }
    let width = (window.east() - window.west) / lon_stride;
    let height = (window.north() - window.south) / lat_stride;
    if width <= 0 || height <= 0 || width > u16::MAX as i64 || height > u16::MAX as i64 {
        return None;
    }
    let (width, height) = (width as u32, height as u32);
    let col_offset = (window.west - crop.west_udeg) / lon_stride;
    let row_offset = (window.south - crop.south_udeg) / lat_stride;

    let edge = TILE_EDGE as u32;
    let tile_cols = width.div_ceil(edge);
    let tile_rows = height.div_ceil(edge);
    let mut tiles = vec![[INTENSITY_NODATA; TILE_CELLS]; (tile_cols * tile_rows) as usize];
    let mut saw_no_data = false;
    for row in 0..height {
        for col in 0..width {
            let source_col = col_offset + i64::from(col);
            let source_row = row_offset + i64::from(row);
            let value = if source_col >= 0 && source_row >= 0 {
                crop.cell(source_col as u32, source_row as u32).unwrap_or(INTENSITY_NODATA)
            } else {
                INTENSITY_NODATA
            };
            if value == INTENSITY_NODATA {
                saw_no_data = true;
            }
            let tile = (row / edge) * tile_cols + col / edge;
            tiles[tile as usize][((row % edge) * edge + col % edge) as usize] = value;
        }
    }
    let mut quality = match crop.source_class {
        SourceClass::Observation => QUALITY_OBSERVED,
        SourceClass::Forecast => QUALITY_FORECAST,
    };
    if saw_no_data || crop.partial {
        quality |= QUALITY_PARTIAL_COVERAGE;
    }
    Some((crop.valid_at, width as u16, height as u16, crop.cell_size_m, quality, tiles))
}

/// A bundle with no rain frames at all — the explicit hourly-only state, built through the same
/// encoder so the device path is identical.
pub fn hourly_only(
    generation: u32,
    request_id: u32,
    generated_at: i64,
    anchor: (i32, i32),
    hourly: &Hourly,
) -> Result<Vec<u8>, BuildError> {
    build(generation, request_id, generated_at, anchor, &[], hourly).map(|(bytes, _)| bytes)
}
