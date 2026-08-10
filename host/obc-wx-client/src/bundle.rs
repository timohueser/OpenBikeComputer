//! Shard crops + hourly → one OBCW bundle, through the shared `obc_formats::obcw` encoder.
//!
//! Under one uniform lattice there is no window *choice* left to make. Every frame is the same
//! 0.01° grid, so the window is arithmetic on the corridor: align it outward to lattice cells,
//! intersect it with the lattice, and state it. The coarsest-lattice search, the tie-break, and the
//! four remainder guards that used to drop a frame that could not tile the window are all gone with
//! the heterogeneity that made them necessary — one lattice cannot fail to tile itself.
//!
//! ## The one transformation left: a uniform east-west pitch
//!
//! A 0.01° **column** is `1,113 x cos φ` metres wide — 715 m at 50°N, 387 m at Tromsø — while a
//! 0.01° **row** is 1,113 m everywhere. The degree lattice therefore oversamples east-west by
//! `1/cos φ`, and a corridor of fixed *ground* radius costs more and more columns the further north
//! the rider is, for detail that no source produces. Left alone, a 90 km disc is 253 columns at
//! Frankfurt and 465 at Tromsø, and the producer cap has to climb one rung per degree of latitude
//! supported (256 KiB tops out at 55.8°N, 512 KiB at 74.15°N).
//!
//! So the window is resampled onto a column pitch equal to the lattice's north-south cell height —
//! **nearest neighbour**, the rule `OBCG_Spec` §6 and `OBCW_Spec` §5 already mandate everywhere
//! else — and rows are untouched. A 90 km disc is then **162 x 162 cells at every latitude**, cells
//! are square to within 0.4 %, and `cell_size_m = 1113` is simply true.
//!
//! This **bounds** the bundle rather than shrinking it: 162 x 162 is what a 180 km box costs once
//! its cells are ~1,113 m on both axes, which is what the lattice already gives at the equator
//! (where the map is the identity). The trade is real and stated: nearest neighbour can drop a rain
//! feature narrower than the output pitch, which is below the scale of the phenomenon (convective
//! cells are 2-10 km), below the scale of every source, and sub-pixel on a device that renders
//! ~3 px per cell. Measured in #1254 across a 0-80°N sweep, the raw4 worst case is 153.58 kB
//! uniformly — 41 % under the cap.
//!
//! The column map is **integer arithmetic on purpose**: `src_col = (2j+1) * src_cols / (2 * cols)`.
//! The phone computes the same expression, and two implementations rounding a float differently
//! would silently disagree about which source column a rider's cell came from.

use obc_formats::obcw::{
    encode_format, encoded_len, BundleInput, RainFrameInput, HOURLY_COUNT, QUALITY_FORECAST, QUALITY_OBSERVED,
    QUALITY_PARTIAL_COVERAGE, TILE_CELLS,
};
use obc_formats::precip4::{INTENSITY_DRY, INTENSITY_NODATA, TILE_EDGE};

use crate::corridor::Crop;
use crate::manifest_v2::{Bbox, Grid};
use crate::met::Hourly;

/// The OBCW producer cap (`OBCW_Spec.md` §2). The window shrinks until the bundle fits.
///
/// Raised to 256 KiB by WXR5 #1244. §2 already called 65,536 "a phone producer policy, separate
/// from the format", so this is a policy number and nothing about the container moved: the device's
/// weather reader is a windowed streamer whose resident bytes are independent of bundle size. What
/// it does cost is BLE time — roughly 10-13 s instead of 2 s against §11.3's 60 s advertising
/// window — which is measured on glass, not argued from a ratio.
pub const PRODUCER_CAP: usize = 262_144;
/// How many times the window may shrink before the builder starts dropping frames instead.
///
/// Kept as the backstop it always was. With the uniform pitch above it should never fire: the
/// swept worst case is 153.58 kB against a 256 KiB cap. It exists for the case the sweep did not
/// imagine, and `report.shrinks` says loudly when that happens.
pub const MAX_SHRINK_ATTEMPTS: u32 = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildError {
    /// Even one frame over a one-cell window would not fit — structurally impossible, but the
    /// builder refuses rather than emitting a bundle that violates the cap.
    TooLarge,
    /// An hourly-only bundle whose corridor has no positive extent: there is no region to state,
    /// and inventing one would put the forecast somewhere it was not asked about.
    InvalidCorridor,
    Encode(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::TooLarge => write!(f, "no window small enough fits the OBCW producer cap"),
            BuildError::InvalidCorridor => write!(f, "the corridor has no positive extent"),
            BuildError::Encode(why) => write!(f, "OBCW encode: {why}"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BuildReport {
    /// Frames dropped, furthest-future first, to fit the producer cap.
    pub dropped_oversize: u32,
    /// How many times the window shrank.
    pub shrinks: u32,
    pub frames: u32,
    pub window_width: u32,
    pub window_height: u32,
    /// The source columns the window spans, before the east-west resample. Equal to
    /// `window_width` at the equator and larger everywhere else; the ratio *is* `cos φ`.
    pub source_columns: u32,
}

/// The lattice every frame lives on, as the manifest states it. The client holds no copy of these
/// numbers: re-cutting the dataset is a baker deploy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lattice {
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub cell_udeg: i64,
    pub width: u32,
    pub height: u32,
    pub cell_size_m: u16,
}

impl From<&Grid> for Lattice {
    fn from(grid: &Grid) -> Self {
        Self {
            south_udeg: i64::from(grid.south_lat_udeg),
            west_udeg: i64::from(grid.west_lon_udeg),
            cell_udeg: i64::from(grid.cell_udeg),
            width: grid.width,
            height: grid.height,
            cell_size_m: grid.cell_size_m,
        }
    }
}

/// One frame of the timeline, as the plan resolved it.
///
/// `dry` is not decoration and not an optimisation: a shard the baker measured as dry publishes no
/// object, so a frame whose corridor is entirely dry arrives here with **no crops at all**. Dropping
/// it would put a hole in the timeline where the honest answer is a rain-free frame, so the dry
/// rectangles are painted as intensity 0 and the frame ships.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FrameInput {
    pub valid_at: i64,
    /// Fetched shard crops, all on the lattice. Up to four, when the corridor straddles a seam.
    pub crops: Vec<Crop>,
    /// Shard rectangles the manifest says are dry everywhere.
    pub dry: Vec<Bbox>,
}

impl FrameInput {
    fn is_empty(&self) -> bool {
        self.crops.is_empty() && self.dry.is_empty()
    }

    /// A frame is an observation only if **every** patch of it is: a mosaic frame that is radar
    /// over the rider and model fill fifty kilometres east is a forecast as far as one per-frame
    /// quality flag can say. Dry shards carry no `observed` flag — the baker published nothing to
    /// hang one on — so a frame with no fetched shards at all cannot claim to be observed.
    fn observed(&self) -> bool {
        !self.crops.is_empty() && self.crops.iter().all(|crop| crop.observed)
    }
}

/// The rain half of a bundle: the lattice it lives on and the timeline over it.
#[derive(Debug, Clone, Copy)]
pub struct Scene<'a> {
    pub lattice: Lattice,
    pub frames: &'a [FrameInput],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    /// South-west corner, on the lattice.
    south: i64,
    west: i64,
    cell: i64,
    /// Lattice columns the window spans, and lattice rows (rows are never resampled).
    src_cols: u32,
    rows: u32,
    /// Output columns after the uniform east-west resample.
    cols: u32,
}

impl Window {
    fn north(&self) -> i64 {
        self.south + i64::from(self.rows) * self.cell
    }

    fn east(&self) -> i64 {
        self.west + i64::from(self.src_cols) * self.cell
    }
}

/// How many ~1,113 m columns span `src_cols` lattice columns at latitude `lat_udeg`.
///
/// `round(src_cols * cos φ)`, floored at one and capped at `src_cols`: the resample only ever
/// *decimates*, because a lattice column is already the finest thing any source produced and
/// stretching it would be inventing detail. At the equator this is the identity.
fn output_columns(src_cols: u32, lat_udeg: i32) -> u32 {
    let cos = (f64::from(lat_udeg) / 1e6).to_radians().cos();
    let scaled = (f64::from(src_cols) * cos).round();
    if !scaled.is_finite() || scaled < 1.0 {
        return 1;
    }
    (scaled as u32).clamp(1, src_cols)
}

fn floor_div(numerator: i64, denominator: i64) -> i64 {
    numerator.div_euclid(denominator)
}

fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    -(-numerator).div_euclid(denominator)
}

/// The lattice-aligned window covering `corridor`, intersected with the lattice. `None` when the
/// corridor and the lattice do not overlap at all.
fn initial_window(lattice: &Lattice, corridor: &Bbox, anchor: (i32, i32)) -> Option<Window> {
    let cell = lattice.cell_udeg;
    let first_col = floor_div(corridor.west_udeg - lattice.west_udeg, cell).max(0);
    let last_col = ceil_div(corridor.east_udeg - lattice.west_udeg, cell).min(i64::from(lattice.width));
    let first_row = floor_div(corridor.south_udeg - lattice.south_udeg, cell).max(0);
    let last_row = ceil_div(corridor.north_udeg - lattice.south_udeg, cell).min(i64::from(lattice.height));
    if first_col >= last_col || first_row >= last_row {
        return None;
    }
    let src_cols = (last_col - first_col) as u32;
    Some(Window {
        south: lattice.south_udeg + first_row * cell,
        west: lattice.west_udeg + first_col * cell,
        cell,
        src_cols,
        rows: (last_row - first_row) as u32,
        cols: output_columns(src_cols, anchor.0),
    })
}

/// Build the bundle. `anchor` is the rider's `(lat, lon)` in microdegrees — it re-centres a shrunken
/// window, because one that shrinks toward the corridor midpoint walks off the back of a fast rider,
/// and its latitude sets the resample pitch. `corridor` is the region the job asked about, and is
/// what an **hourly-only** bundle declares: the screens then say *hourly only here* over the area
/// the question was actually about.
///
/// `scene` is `None` when there is no rain half at all — an unreachable service, an expired
/// generation, a corridor off the map. That is deliberately not the same value as a scene whose
/// frames are all dry, which is a real, all-zero timeline.
pub fn build(
    generation: u32,
    request_id: u32,
    generated_at: i64,
    anchor: (i32, i32),
    corridor: &Bbox,
    scene: Option<Scene<'_>>,
    hourly: &Hourly,
) -> Result<(Vec<u8>, BuildReport), BuildError> {
    let mut report = BuildReport::default();

    // One frame per timestamp, ascending — OBCW requires strictly increasing `valid_at`.
    let mut usable: Vec<&FrameInput> = match scene {
        Some(scene) => scene.frames.iter().filter(|frame| !frame.is_empty()).collect(),
        None => Vec::new(),
    };
    usable.sort_by_key(|frame| frame.valid_at);
    usable.dedup_by_key(|frame| frame.valid_at);

    let mut window = match scene.filter(|_| !usable.is_empty()).and_then(|scene| {
        initial_window(&scene.lattice, corridor, anchor).map(|window| (window, scene.lattice.cell_size_m))
    }) {
        Some((window, cell_size_m)) => {
            report.source_columns = window.src_cols;
            Some((window, cell_size_m))
        }
        // No rain at all: an hourly-only bundle still states a region, so the screens can say
        // *hourly only here* instead of guessing. One cell spanning the **corridor** — the region
        // the job asked about — rather than an invented degree around the anchor.
        None => {
            let (lat_span, lon_span) =
                (corridor.north_udeg - corridor.south_udeg, corridor.east_udeg - corridor.west_udeg);
            if lat_span <= 0 || lon_span <= 0 {
                return Err(BuildError::InvalidCorridor);
            }
            usable.clear();
            None
        }
    };

    let mut attempt = 0u32;
    loop {
        let mut partial_frames = Vec::new();
        if let Some((window, cell_size_m)) = &window {
            for frame in &usable {
                partial_frames.push(rain_frame(frame, window, *cell_size_m));
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
        let (south, west, north, east) = match &window {
            Some((window, _)) => (window.south, window.west, window.north(), window.east()),
            None => (corridor.south_udeg, corridor.west_udeg, corridor.north_udeg, corridor.east_udeg),
        };
        let input = BundleInput {
            generation,
            request_id,
            generated_at,
            valid_from,
            valid_until,
            south_lat_udeg: south as i32,
            west_lon_udeg: west as i32,
            north_lat_udeg: north as i32,
            east_lon_udeg: east as i32,
            grid_origin_lat_udeg: south as i32,
            grid_origin_lon_udeg: west as i32,
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
            report.frames = frames.len() as u32;
            if let Some((window, _)) = &window {
                report.window_width = window.cols;
                report.window_height = window.rows;
            }
            return Ok((bytes, report));
        }
        drop(frames);
        drop(partial_frames);
        // Trimming the window beats dropping a frame: a shorter corridor still answers every
        // timestamp, while a missing frame puts a hole in the two-hour timeline.
        if attempt < MAX_SHRINK_ATTEMPTS {
            if let Some((current, cell_size_m)) = window {
                if let Some(shrunk) = shrink(&current, anchor) {
                    window = Some((shrunk, cell_size_m));
                    attempt += 1;
                    report.shrinks += 1;
                    continue;
                }
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

/// Trim an eighth off each axis, re-centred on the rider.
///
/// The trim happens in **source-lattice cells**, so the window's corners stay lattice-aligned
/// integers and the output column count is re-derived from the smaller span. Shrinking the output
/// grid directly would leave the window's east edge on a fractional cell boundary, and the two
/// implementations would then have to agree on how to round it.
fn shrink(window: &Window, anchor: (i32, i32)) -> Option<Window> {
    let drop_cols = (window.src_cols / 8).max(1);
    let drop_rows = (window.rows / 8).max(1);
    let src_cols = window.src_cols.checked_sub(drop_cols).filter(|cols| *cols > 0)?;
    let rows = window.rows.checked_sub(drop_rows).filter(|rows| *rows > 0)?;
    let anchor_col = ((i64::from(anchor.1) - window.west) / window.cell).clamp(0, i64::from(window.src_cols) - 1);
    let anchor_row = ((i64::from(anchor.0) - window.south) / window.cell).clamp(0, i64::from(window.rows) - 1);
    let first_col = (anchor_col - i64::from(src_cols) / 2).clamp(0, i64::from(window.src_cols - src_cols));
    let first_row = (anchor_row - i64::from(rows) / 2).clamp(0, i64::from(window.rows - rows));
    Some(Window {
        south: window.south + first_row * window.cell,
        west: window.west + first_col * window.cell,
        cell: window.cell,
        src_cols,
        rows,
        cols: output_columns(src_cols, anchor.0),
    })
}

type PreparedFrame = (i64, u16, u16, u16, u32, Vec<[u8; TILE_CELLS]>);

/// Lay one frame's shards onto the window as 16 × 16 OBCW tiles, resampling east-west.
///
/// Three paints, in this order, and the order is the semantics: everything starts **no-data**, dry
/// shards become **intensity 0**, and fetched cells overwrite both. A cell no shard covers stays
/// no-data — never dry — and marks the frame partially covered.
fn rain_frame(frame: &FrameInput, window: &Window, cell_size_m: u16) -> PreparedFrame {
    let (width, height) = (window.cols, window.rows);
    let edge = TILE_EDGE as u32;
    let tile_cols = width.div_ceil(edge);
    let tile_rows = height.div_ceil(edge);
    let mut tiles = vec![[INTENSITY_NODATA; TILE_CELLS]; (tile_cols * tile_rows) as usize];
    let mut saw_no_data = false;

    for row in 0..height {
        // Rows are never resampled: the lattice's north-south pitch is the output pitch.
        let cell_south = window.south + i64::from(row) * window.cell;
        for col in 0..width {
            // Nearest neighbour by cell centre, in exact integer arithmetic.
            let source_col = ((2 * u64::from(col) + 1) * u64::from(window.src_cols) / (2 * u64::from(window.cols)))
                .min(u64::from(window.src_cols) - 1) as i64;
            let cell_west = window.west + source_col * window.cell;

            let mut value = INTENSITY_NODATA;
            if frame.dry.iter().any(|rect| {
                rect.south_udeg <= cell_south
                    && cell_south < rect.north_udeg
                    && rect.west_udeg <= cell_west
                    && cell_west < rect.east_udeg
            }) {
                value = INTENSITY_DRY;
            }
            for crop in &frame.crops {
                let local_col = (cell_west - crop.west_udeg) / i64::from(crop.cell_lon_udeg);
                let local_row = (cell_south - crop.south_udeg) / i64::from(crop.cell_lat_udeg);
                if local_col < 0 || local_row < 0 {
                    continue;
                }
                if let Some(cell) = crop.cell(local_col as u32, local_row as u32) {
                    value = cell;
                    break;
                }
            }
            if value == INTENSITY_NODATA {
                saw_no_data = true;
            }
            let tile = (row / edge) * tile_cols + col / edge;
            tiles[tile as usize][((row % edge) * edge + col % edge) as usize] = value;
        }
    }

    let mut quality = if frame.observed() { QUALITY_OBSERVED } else { QUALITY_FORECAST };
    if saw_no_data || frame.crops.iter().any(|crop| crop.partial) {
        quality |= QUALITY_PARTIAL_COVERAGE;
    }
    (frame.valid_at, width as u16, height as u16, cell_size_m, quality, tiles)
}

/// A bundle with no rain frames at all — the explicit hourly-only state, built through the same
/// encoder so the device path is identical.
pub fn hourly_only(
    generation: u32,
    request_id: u32,
    generated_at: i64,
    anchor: (i32, i32),
    corridor: &Bbox,
    hourly: &Hourly,
) -> Result<Vec<u8>, BuildError> {
    build(generation, request_id, generated_at, anchor, corridor, None, hourly).map(|(bytes, _)| bytes)
}
