//! The canonical lattice, the priority mosaic, and the sharded emit (WXR3 #1242).
//!
//! This is the module the epic's central sentence lives in: **the baker is the only component
//! that knows a data source exists.** Every adapter's output is normalised onto one global
//! 0.01 degree lattice, overlapping sources are resolved per cell by one ordered priority table
//! ([`crate::source::MOSAIC_PRIORITY`]), and what leaves the bakery is a single provider-agnostic
//! dataset: 24 shards x 9 frames of OBCG, all on the same lattice, all at the same cell size.
//!
//! ## Why the resample is here and not in each adapter
//!
//! The adapters keep fetch, decode, reproject and quantize verbatim; what changed is the meaning
//! of their `GEOMETRY` const. It is now a **source-window description** — where this source has
//! data and at what pitch — rather than an output lattice. The last stage, "resample onto the
//! canonical lattice", is this one shared nearest-neighbour implementation, and it runs **lazily,
//! per shard**:
//!
//! - a source already at or finer than the lattice pitch reprojects straight onto a
//!   canonical-aligned window in its own adapter (DWD RV and MRMS do), so the mosaic copies its
//!   cells with no second hop and no second rounding;
//! - a coarser source stays on its native window and is **cell-replicated** at fill time — one
//!   6.5 km ICON cell paints a block of identical 1 km cells. That is the nearest-neighbour rule
//!   `OBCG_Spec.md` §6 already mandates, applied once.
//!
//! Doing it lazily is a hard requirement, not a preference. WXR1 (#1254) measured the GO on the
//! condition that the baker materialises **one shard per thread** (255 MB steady state); a global
//! 0.01 degree raster is 648 M cells = 648 MB *per frame*, so a GFS floor eagerly upsampled onto
//! the lattice would cost more than the whole 8 GB box before a single tile was encoded.
//!
//! ## Why there is no provenance channel
//!
//! Locked 2026-08-10 (#1242, #1248): no per-cell resolution plane, no per-tile source label, no
//! coverage descriptor. The mosaic always has a **global floor source**, so every cell always
//! carries a best-available value and "no radar coverage" renders as model fill rather than as
//! dry. Intensity code 15 stays the honest answer for genuinely missing data — a floor-source
//! outage or a shard that failed to bake — and it is the only distinction worth carrying.
//! `cell_size_m` is therefore pinned to [`Lattice::cell_size_m`], a constant stating the lattice,
//! rather than describing a per-cell source that no longer has one value.

use std::time::Instant;

use obc_formats::obcg::{self, FrameInput};
use obc_formats::precip4;
use rayon::prelude::*;
use serde::Serialize;

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;
use crate::manifest;
use crate::publish::{self, ObjectStore, PlannedObject};
use crate::source::{mosaic_rank, Adapter, AdapterOutcome, BakedProduct};

// ---------------------------------------------------------------------------------------------
// The lattice
// ---------------------------------------------------------------------------------------------

/// The canonical cell: 0.01 degrees in both axes.
pub const CELL_UDEG: u32 = 10_000;

/// The one value `cell_size_m` ever takes, in metres: 0.01 degrees of **latitude**, which is
/// 1,113 m everywhere. It states the lattice, not a source (see the module comment). The device
/// reads this field — `firmware/obc-app/src/weather.rs` sizes the rain-spread render from it —
/// so it stays a truthful number rather than being removed from a fixed header offset.
pub const LATTICE_CELL_SIZE_M: u16 = 1_113;

/// Frames per cycle and the step between them: +0 .. +120 minutes in 15-minute steps.
pub const CYCLE_FRAMES: u32 = 9;
pub const FRAME_STEP_MIN: u32 = 15;

/// How far a source frame may sit from a canonical frame's validity and still be sampled for it.
/// The coarsest cadence any source publishes is hourly (GFS, ICON-EU), so a half-hour window
/// always finds one; anything further away is a source that has fallen out of the timeline, and
/// the mosaic drops through to the next-priority source rather than painting stale cells.
pub const MAX_FRAME_SKEW_S: i64 = 1_800;

/// A global regular lat/lon lattice cut into a fixed grid of shards.
///
/// This is a type rather than a pile of constants so tests can drive the identical code path over
/// a lattice small enough to encode in a debug build. Production uses exactly one instance:
/// [`CANONICAL`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lattice {
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    /// Square cells; the lattice is defined in degrees, not metres.
    pub cell_udeg: u32,
    pub width: u32,
    pub height: u32,
    /// Shard extent in cells. The last shard column/row is truncated to the lattice edge.
    pub shard_width: u32,
    pub shard_height: u32,
    pub tile_edge: u16,
    pub entries_per_page: u16,
    /// The pinned `cell_size_m` every emitted frame declares.
    pub cell_size_m: u16,
}

/// The published lattice: global 0.01 degrees, 36,000 x 18,000 = 648 M cells, cut into a 6 x 4
/// grid of 6,144 x 4,608-cell shards. Every number here is WXR1's measured recommendation
/// (PR #1254): the shard is 94 % of `obcg::MAX_GRID_CELLS` and tile-aligned in both axes, tile
/// edge 256 with the per-tile deflate codec is 14.69 MB of a wet global cycle against 43.60 MB at
/// edge 64, and 128 entries per page dominates corridor fetch cost more than the tile edge does.
pub const CANONICAL: Lattice = Lattice {
    south_lat_udeg: -90_000_000,
    west_lon_udeg: -180_000_000,
    cell_udeg: CELL_UDEG,
    width: 36_000,
    height: 18_000,
    shard_width: 6_144,
    shard_height: 4_608,
    tile_edge: 256,
    entries_per_page: 128,
    cell_size_m: LATTICE_CELL_SIZE_M,
};

/// A rectangular window of a lattice, in lattice cell coordinates (col 0 = west, row 0 = south).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LatticeWindow {
    pub col0: u32,
    pub row0: u32,
    pub width: u32,
    pub height: u32,
}

impl LatticeWindow {
    pub fn cells(&self) -> usize {
        self.width as usize * self.height as usize
    }
}

impl Lattice {
    pub fn shard_cols(&self) -> u32 {
        self.width.div_ceil(self.shard_width)
    }

    pub fn shard_rows(&self) -> u32 {
        self.height.div_ceil(self.shard_height)
    }

    pub fn shard_count(&self) -> u32 {
        self.shard_cols() * self.shard_rows()
    }

    /// Shard `index` in row-major order (south-west first), or `None` past the last shard.
    pub fn shard(&self, index: u32) -> Option<LatticeWindow> {
        if index >= self.shard_count() {
            return None;
        }
        let col = index % self.shard_cols();
        let row = index / self.shard_cols();
        let col0 = col * self.shard_width;
        let row0 = row * self.shard_height;
        Some(LatticeWindow {
            col0,
            row0,
            width: self.shard_width.min(self.width - col0),
            height: self.shard_height.min(self.height - row0),
        })
    }

    pub fn shards(&self) -> impl Iterator<Item = (u32, LatticeWindow)> + '_ {
        (0..self.shard_count()).map(|index| (index, self.shard(index).expect("index < shard_count")))
    }

    /// Cell-centre latitude of a lattice row, in microdegrees.
    pub fn centre_lat_udeg(&self, row: u32) -> i64 {
        i64::from(self.south_lat_udeg) + i64::from(row) * i64::from(self.cell_udeg) + i64::from(self.cell_udeg / 2)
    }

    /// Cell-centre longitude of a lattice column, in microdegrees.
    pub fn centre_lon_udeg(&self, col: u32) -> i64 {
        i64::from(self.west_lon_udeg) + i64::from(col) * i64::from(self.cell_udeg) + i64::from(self.cell_udeg / 2)
    }

    /// The OBCG geometry of one window of this lattice. Every emitted object's header is this.
    pub fn geometry(&self, window: LatticeWindow) -> GridGeometry {
        GridGeometry {
            south_lat_udeg: self.south_lat_udeg + (window.row0 * self.cell_udeg) as i32,
            west_lon_udeg: self.west_lon_udeg + (window.col0 * self.cell_udeg) as i32,
            cell_lat_udeg: self.cell_udeg,
            cell_lon_udeg: self.cell_udeg,
            width: window.width,
            height: window.height,
            cell_size_m: self.cell_size_m,
            tile_edge: self.tile_edge,
            entries_per_page: self.entries_per_page,
        }
    }

    /// Gate the lattice against the OBCG format limits before any bake work: every shard must be
    /// an expressible object, and the shard grid must tile the lattice exactly.
    pub fn validate(&self) -> Result<(), String> {
        if self.cell_udeg == 0 || self.shard_width == 0 || self.shard_height == 0 {
            return Err(format!("degenerate lattice: {self:?}"));
        }
        let mut covered = 0u64;
        for (index, window) in self.shards() {
            self.geometry(window).validate().map_err(|error| format!("shard {index}: {error}"))?;
            covered += window.cells() as u64;
        }
        let expected = u64::from(self.width) * u64::from(self.height);
        if covered != expected {
            return Err(format!("the shard grid covers {covered} cells, the lattice has {expected}"));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// The mosaic
// ---------------------------------------------------------------------------------------------

/// One source frame as its adapter produced it: quantized cells on the source's own window.
#[derive(Debug)]
pub struct SourceFrame {
    pub valid_at: i64,
    /// The **source window**: where this source has data and at what pitch. Not an output
    /// lattice — the mosaic resamples from it.
    pub window: GridGeometry,
    pub cells: Vec<u8>,
}

/// Every frame one source contributes to the mosaic, at its priority rank.
#[derive(Debug)]
pub struct MosaicLayer {
    pub id: &'static str,
    /// Index into [`crate::source::MOSAIC_PRIORITY`]; **lower wins**.
    pub rank: usize,
    pub frames: Vec<SourceFrame>,
}

impl MosaicLayer {
    /// Turn one adapter's baked product into a layer. A composed product's frames each keep their
    /// own source window, which is exactly what the mosaic wants.
    pub fn from_product(product: BakedProduct) -> Result<Self, String> {
        let rank = mosaic_rank(product.id)
            .ok_or_else(|| format!("{}: no row in source::MOSAIC_PRIORITY — the mosaic cannot rank it", product.id))?;
        let anchor = product.geometry;
        let id = product.id;
        let mut frames = Vec::with_capacity(product.frames.len());
        for frame in product.frames {
            let window = frame.source.map_or(anchor, |source| source.geometry);
            if frame.cells.len() != window.cells() {
                return Err(format!("{id}: frame f{} cell count disagrees with its source window", frame.offset_min));
            }
            frames.push(SourceFrame { valid_at: frame.valid_at, window, cells: frame.cells });
        }
        frames.sort_by_key(|frame| frame.valid_at);
        Ok(Self { id, rank, frames })
    }

    /// The frame to sample for a canonical frame valid at `valid_at`: nearest validity wins,
    /// earliest breaks ties, and nothing further away than [`MAX_FRAME_SKEW_S`] is sampled at all.
    fn nearest(&self, valid_at: i64) -> Option<&SourceFrame> {
        self.frames
            .iter()
            .min_by_key(|frame| ((frame.valid_at - valid_at).abs(), frame.valid_at))
            .filter(|frame| (frame.valid_at - valid_at).abs() <= MAX_FRAME_SKEW_S)
    }
}

/// The priority mosaic: every source, ordered, resampled onto one lattice on demand.
#[derive(Debug)]
pub struct Mosaic {
    /// Sorted **worst rank first**, which is what makes the painter's pass below equivalent to
    /// per-cell winner selection.
    layers: Vec<MosaicLayer>,
}

impl Mosaic {
    pub fn new(mut layers: Vec<MosaicLayer>) -> Self {
        layers.sort_by(|left, right| right.rank.cmp(&left.rank).then_with(|| left.id.cmp(right.id)));
        Self { layers }
    }

    pub fn from_products(products: Vec<BakedProduct>) -> Result<Self, String> {
        let layers = products.into_iter().map(MosaicLayer::from_product).collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(layers))
    }

    pub fn layers(&self) -> &[MosaicLayer] {
        &self.layers
    }

    /// Paint one window of `lattice` for the canonical frame valid at `valid_at`.
    ///
    /// **Per-cell winner selection**: a cell is painted by the highest-priority source that both
    /// covers it and has data there. It is implemented as a painter's pass — worst rank first,
    /// overwriting only with non-no-data values — which is the same function and costs no
    /// per-cell winner mask (28 MB a shard) to evaluate. Cells no source covers keep
    /// [`precip4::INTENSITY_NODATA`]: missing is never dry.
    pub fn fill(&self, lattice: &Lattice, valid_at: i64, window: LatticeWindow, out: &mut [u8]) {
        assert_eq!(out.len(), window.cells(), "fill target does not match the window");
        out.fill(precip4::INTENSITY_NODATA);
        // One column map per layer, reused down every row: the east-west nearest-neighbour pick
        // depends only on the column, so a shard pays the division once per column, not per cell.
        let mut columns: Vec<i32> = vec![-1; window.width as usize];
        for layer in &self.layers {
            let Some(frame) = layer.nearest(valid_at) else { continue };
            let source = &frame.window;
            let mut covers = false;
            for (index, slot) in columns.iter_mut().enumerate() {
                let lon = lattice.centre_lon_udeg(window.col0 + index as u32);
                let column = (lon - i64::from(source.west_lon_udeg)).div_euclid(i64::from(source.cell_lon_udeg));
                *slot = if (0..i64::from(source.width)).contains(&column) {
                    covers = true;
                    column as i32
                } else {
                    -1
                };
            }
            if !covers {
                continue;
            }
            for row in 0..window.height as usize {
                let lat = lattice.centre_lat_udeg(window.row0 + row as u32);
                let source_row = (lat - i64::from(source.south_lat_udeg)).div_euclid(i64::from(source.cell_lat_udeg));
                if !(0..i64::from(source.height)).contains(&source_row) {
                    continue;
                }
                let base = source_row as usize * source.width as usize;
                let row_cells = &frame.cells[base..base + source.width as usize];
                let destination = &mut out[row * window.width as usize..(row + 1) * window.width as usize];
                for (cell, column) in destination.iter_mut().zip(&columns) {
                    if *column >= 0 {
                        let value = row_cells[*column as usize];
                        if value != precip4::INTENSITY_NODATA {
                            *cell = value;
                        }
                    }
                }
            }
        }
    }

    /// Which source wins one lattice cell, for diagnostics and for the tests that prove the table
    /// rather than the geometry decides. `None` means no source covers it with data.
    pub fn winner_at(&self, lattice: &Lattice, valid_at: i64, col: u32, row: u32) -> Option<&'static str> {
        let lat = lattice.centre_lat_udeg(row);
        let lon = lattice.centre_lon_udeg(col);
        let mut winner: Option<(usize, &'static str)> = None;
        for layer in &self.layers {
            let Some(frame) = layer.nearest(valid_at) else { continue };
            let source = &frame.window;
            let column = (lon - i64::from(source.west_lon_udeg)).div_euclid(i64::from(source.cell_lon_udeg));
            let source_row = (lat - i64::from(source.south_lat_udeg)).div_euclid(i64::from(source.cell_lat_udeg));
            if !(0..i64::from(source.width)).contains(&column) || !(0..i64::from(source.height)).contains(&source_row) {
                continue;
            }
            let value = frame.cells[source_row as usize * source.width as usize + column as usize];
            if value == precip4::INTENSITY_NODATA {
                continue;
            }
            if winner.is_none_or(|(rank, _)| layer.rank < rank) {
                winner = Some((layer.rank, layer.id));
            }
        }
        winner.map(|(_, id)| id)
    }
}

// ---------------------------------------------------------------------------------------------
// Emit
// ---------------------------------------------------------------------------------------------

/// The cycle's time axis: one reference instant, [`CYCLE_FRAMES`] frames off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CycleTimes {
    pub reference_time: i64,
}

impl CycleTimes {
    /// Anchor a cycle at the 15-minute boundary at or before `now`, so a timer that fires a few
    /// seconds late publishes the same frame validities as one that fires early.
    pub fn anchored_at(now: i64) -> Self {
        let step = i64::from(FRAME_STEP_MIN) * 60;
        Self { reference_time: now.div_euclid(step) * step }
    }

    pub fn offsets_min(&self) -> impl Iterator<Item = u32> {
        (0..CYCLE_FRAMES).map(|frame| frame * FRAME_STEP_MIN)
    }

    pub fn valid_at(&self, offset_min: u32) -> i64 {
        self.reference_time + i64::from(offset_min) * 60
    }
}

/// One publishable shard object.
#[derive(Debug)]
pub struct CanonicalObject {
    pub key: String,
    pub shard: u32,
    pub offset_min: u32,
    pub bytes: Vec<u8>,
    pub object_crc32: u32,
}

/// Immutable object key for one shard of one frame. **Placeholder addressing**: WXR4 #1243 owns
/// the published key scheme and the manifest that indexes it; this exists so WXR3 can emit and
/// verify the dataset without pre-empting that design. It deliberately publishes under a `v2`
/// prefix beside the live `wx/v1` tree, which is the cutover shape the epic requires.
pub fn shard_key(reference_time: i64, offset_min: u32, shard: u32) -> String {
    format!("wx/v2/{}/f{offset_min}/s{shard}.obcg", manifest::key_timestamp(reference_time))
}

/// Mosaic one shard of one frame and encode it, through the same `encoded length -> encode ->
/// validate` path `emit` uses, so a baker bug can never publish an object the phone would reject.
pub fn emit_shard(
    lattice: &Lattice,
    mosaic: &Mosaic,
    times: CycleTimes,
    offset_min: u32,
    shard: u32,
) -> Result<CanonicalObject, String> {
    let window = lattice.shard(shard).ok_or_else(|| format!("shard {shard} is not on this lattice"))?;
    let geometry = lattice.geometry(window);
    geometry.validate()?;
    let valid_at = times.valid_at(offset_min);
    let mut cells = vec![precip4::INTENSITY_NODATA; window.cells()];
    mosaic.fill(lattice, valid_at, window, &mut cells);

    let input = FrameInput {
        // The dataset has exactly one product because it *is* the product: one lattice, one cell
        // size, best available everywhere. The per-source codes stay in the registry until WXR7
        // deletes the multi-product path. `tier` is vestigial for the same reason — nothing may
        // select on it any more (WXR5 deletes the client policy that did).
        product_id: obcg::PRODUCT_MOSAIC,
        tier: obcg::TIER_RADAR,
        flags: if offset_min == 0 { obcg::FLAG_OBSERVED } else { obcg::FLAG_FORECAST },
        valid_at,
        reference_time: times.reference_time,
        south_lat_udeg: geometry.south_lat_udeg,
        west_lon_udeg: geometry.west_lon_udeg,
        cell_lat_udeg: geometry.cell_lat_udeg,
        cell_lon_udeg: geometry.cell_lon_udeg,
        width: geometry.width,
        height: geometry.height,
        cell_size_m: geometry.cell_size_m,
        tile_edge: geometry.tile_edge,
        entries_per_page: geometry.entries_per_page,
        cells: &cells,
    };
    let mut scratch = vec![0u8; usize::from(geometry.tile_edge) * usize::from(geometry.tile_edge)];
    // Size with the geometry bound, not with `encoded_len`: the exact length is only knowable by
    // running the codec choice, which compresses every codec-2 tile, so asking would compress the
    // shard twice.
    let bound = obcg::max_encoded_len(&input).map_err(|error| format!("s{shard} f{offset_min}: {error:?}"))? as usize;
    let mut bytes = vec![0u8; bound];
    let len = obcg::encode_format(&input, &mut scratch, &mut bytes)
        .map_err(|error| format!("s{shard} f{offset_min}: {error:?}"))?;
    bytes.truncate(len);
    // Give the slack back: 24 objects of one frame are held at once, and the raw4 bound is an
    // order of magnitude over the real deflate length.
    bytes.shrink_to_fit();
    // The shard's cells are 28 MB and nothing below needs them: release them before the
    // self-validation pass rather than at the end of the function, so the per-thread high-water
    // mark is one cell image, not one cell image plus a finished object.
    drop(cells);
    let header = obcg::validate(&bytes, &mut scratch)
        .map_err(|error| format!("s{shard} f{offset_min}: emitted object failed self-validation: {error:?}"))?;
    Ok(CanonicalObject {
        key: shard_key(times.reference_time, offset_min, shard),
        shard,
        offset_min,
        object_crc32: header.object_crc32,
        bytes,
    })
}

/// Bake and hand over one whole cycle, **one frame at a time**.
///
/// The frame is the streaming unit: its shards are encoded in parallel (one shard of cells per
/// thread, WXR1's condition) and passed to `sink` before the next frame starts, so the baker never
/// holds more than one frame's objects — 24 objects, ~46 MB at the measured worst case — on top of
/// the resident sources.
pub fn bake_cycle(
    lattice: &Lattice,
    mosaic: &Mosaic,
    times: CycleTimes,
    sink: &mut dyn FnMut(CanonicalObject) -> Result<(), String>,
) -> Result<(), String> {
    lattice.validate()?;
    for offset_min in times.offsets_min() {
        let objects = (0..lattice.shard_count())
            .into_par_iter()
            .map(|shard| emit_shard(lattice, mosaic, times, offset_min, shard))
            .collect::<Result<Vec<_>, String>>()?;
        for object in objects {
            sink(object)?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The placeholder manifest
// ---------------------------------------------------------------------------------------------

/// **Placeholder**, owned by WXR4 #1243.
///
/// The canonical dataset needs *some* document naming its objects so the publisher can prove them
/// fetchable and a reader can find them, but manifest v2 — addressing, retention, the freshness
/// contract, what replaces tiers and bboxes — is WXR4's design and must not be guessed at here.
/// This is the minimum honest statement of what was published: the lattice, the time axis, and
/// every object with its length and CRC. It is not schema-pinned and nothing consumes it yet.
#[derive(Debug, Serialize)]
pub struct PlaceholderManifest {
    pub version: u32,
    pub note: &'static str,
    pub generated_at: String,
    pub reference_time: String,
    pub lattice: PlaceholderLattice,
    pub objects: Vec<PlaceholderObject>,
}

#[derive(Debug, Serialize)]
pub struct PlaceholderLattice {
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub cell_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub shard_width: u32,
    pub shard_height: u32,
    pub tile_edge: u16,
    pub entries_per_page: u16,
    pub cell_size_m: u16,
}

#[derive(Debug, Serialize)]
pub struct PlaceholderObject {
    pub key: String,
    pub shard: u32,
    pub offset_min: u32,
    pub valid_at: String,
    pub bytes: u64,
    pub object_crc32: String,
}

/// The placeholder manifest's key, beside the live `wx/v1/manifest.json` rather than over it.
pub const PLACEHOLDER_MANIFEST_KEY: &str = "wx/v2/manifest.json";
pub const PLACEHOLDER_MANIFEST_VERSION: u32 = 2;
const PLACEHOLDER_NOTE: &str =
    "placeholder: WXR4 #1243 defines the manifest for the canonical dataset; nothing consumes this document yet";

impl PlaceholderManifest {
    pub fn new(lattice: &Lattice, times: CycleTimes, generated_at: i64) -> Self {
        Self {
            version: PLACEHOLDER_MANIFEST_VERSION,
            note: PLACEHOLDER_NOTE,
            generated_at: manifest::rfc3339(generated_at),
            reference_time: manifest::rfc3339(times.reference_time),
            lattice: PlaceholderLattice {
                south_lat_udeg: lattice.south_lat_udeg,
                west_lon_udeg: lattice.west_lon_udeg,
                cell_udeg: lattice.cell_udeg,
                width: lattice.width,
                height: lattice.height,
                shard_width: lattice.shard_width,
                shard_height: lattice.shard_height,
                tile_edge: lattice.tile_edge,
                entries_per_page: lattice.entries_per_page,
                cell_size_m: lattice.cell_size_m,
            },
            objects: Vec::new(),
        }
    }

    pub fn record(&mut self, times: CycleTimes, object: &CanonicalObject) {
        self.objects.push(PlaceholderObject {
            key: object.key.clone(),
            shard: object.shard,
            offset_min: object.offset_min,
            valid_at: manifest::rfc3339(times.valid_at(object.offset_min)),
            bytes: object.bytes.len() as u64,
            object_crc32: format!("0x{:08X}", object.object_crc32),
        });
    }

    pub fn to_json(&self) -> String {
        let mut json = serde_json::to_string_pretty(self).expect("the placeholder manifest always serializes");
        json.push('\n');
        json
    }
}

// ---------------------------------------------------------------------------------------------
// The cycle
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct CanonicalReport {
    /// `(source id, priority rank, frames contributed)`, best rank first.
    pub layers: Vec<(String, usize, usize)>,
    pub reference_time: i64,
    pub fetched_bytes: u64,
    pub published_objects: usize,
    pub published_bytes: u64,
    pub elapsed_ms: u128,
    pub warnings: Vec<String>,
}

impl CanonicalReport {
    pub fn summary(&self) -> String {
        let mut lines = vec![format!("canonical cycle anchored at {}", manifest::rfc3339(self.reference_time))];
        for (id, rank, frames) in &self.layers {
            lines.push(format!("  #{rank} {id}: {frames} source frames"));
        }
        lines.push(format!(
            "fetched {} upstream bytes; published {} objects / {} bytes; {} ms",
            self.fetched_bytes, self.published_objects, self.published_bytes, self.elapsed_ms
        ));
        for warning in &self.warnings {
            lines.push(format!("warning: {warning}"));
        }
        lines.join("\n")
    }
}

/// One canonical cycle: bake every adapter, mosaic them, publish the shard set, manifest last.
///
/// Unlike the per-product cycle this never short-circuits on an unchanged upstream — the mosaic
/// needs every source's *cells*, not just the knowledge that its objects are already published,
/// so `previous` is deliberately `None` for every adapter. Caching decoded upstreams across
/// cycles is a WXR8 ops question, not a correctness one.
/// `lattice` is [`CANONICAL`] in production; it is a parameter so the fixture tests can drive this
/// exact orchestration over a sub-window of the same lattice that a debug build can encode.
pub fn run_canonical_cycle(
    lattice: &Lattice,
    adapters: &[&dyn Adapter],
    upstream: &mut dyn Upstream,
    store: &mut dyn ObjectStore,
    now: i64,
    dry_run: bool,
) -> Result<CanonicalReport, String> {
    let started = Instant::now();
    let mut warnings = Vec::new();
    let times = CycleTimes::anchored_at(now);

    let mut products = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        match adapter.bake(upstream, None, now, &mut warnings)? {
            AdapterOutcome::Baked(product) => products.push(*product),
            AdapterOutcome::Unchanged => {
                return Err(format!("{}: reported Unchanged with no previous entry", adapter.id()))
            }
        }
    }
    let mosaic = Mosaic::from_products(products)?;
    let mut layers: Vec<(String, usize, usize)> =
        mosaic.layers().iter().map(|layer| (layer.id.to_string(), layer.rank, layer.frames.len())).collect();
    layers.sort_by_key(|(_, rank, _)| *rank);

    let mut document = PlaceholderManifest::new(lattice, times, now);
    let mut published_objects = 0usize;
    let mut published_bytes = 0u64;
    let mut fetchable: Vec<(String, u64)> = Vec::new();
    bake_cycle(lattice, &mosaic, times, &mut |object| {
        document.record(times, &object);
        if dry_run {
            return Ok(());
        }
        let planned = PlannedObject {
            key: object.key.clone(),
            bytes: object.bytes,
            cache_control: publish::FRAME_CACHE_CONTROL,
            content_type: "application/octet-stream",
        };
        let len = planned.bytes.len() as u64;
        store.put(&planned).map_err(|error| format!("{}: {error}", planned.key))?;
        published_objects += 1;
        published_bytes += len;
        fetchable.push((planned.key, len));
        Ok(())
    })?;

    if !dry_run {
        // The same frames-first, manifest-last proof `publish` gives the v1 tree: every object the
        // manifest is about to name must already be fetchable at the destination, at its length.
        for (key, expected) in &fetchable {
            match store.head(key)? {
                Some(remote) if remote == *expected => {}
                Some(remote) => {
                    return Err(format!(
                        "{key}: published as {remote} bytes but the manifest expects {expected} — refusing to swap the manifest in"
                    ))
                }
                None => return Err(format!("{key}: not fetchable — refusing to swap the manifest in")),
            }
        }
        let planned = PlannedObject {
            key: PLACEHOLDER_MANIFEST_KEY.to_string(),
            bytes: document.to_json().into_bytes(),
            cache_control: publish::MANIFEST_CACHE_CONTROL,
            content_type: "application/json",
        };
        published_bytes += planned.bytes.len() as u64;
        published_objects += 1;
        store.put(&planned).map_err(|error| format!("{}: {error}", planned.key))?;
    }

    Ok(CanonicalReport {
        layers,
        reference_time: times.reference_time,
        fetched_bytes: upstream.fetched_bytes(),
        published_objects,
        published_bytes,
        elapsed_ms: started.elapsed().as_millis(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_lattice_is_wxr1s_measured_recommendation() {
        assert_eq!(u64::from(CANONICAL.width) * u64::from(CANONICAL.height), 648_000_000);
        assert_eq!((CANONICAL.shard_cols(), CANONICAL.shard_rows(), CANONICAL.shard_count()), (6, 4, 24));
        assert_eq!(CANONICAL.tile_edge, 256);
        assert_eq!(CANONICAL.entries_per_page, 128);
        CANONICAL.validate().expect("every shard is an expressible OBCG object and the grid tiles the lattice");
    }

    /// The shard grid must partition the lattice: no gap (a permanent no-data stripe no source
    /// could ever paint) and no overlap (two objects claiming the same cell).
    #[test]
    fn the_shard_grid_partitions_the_lattice_exactly() {
        let mut column_edges = Vec::new();
        let mut row_edges = Vec::new();
        for (index, window) in CANONICAL.shards() {
            assert!(u64::from(window.width) * u64::from(window.height) <= obcg::MAX_GRID_CELLS, "shard {index}");
            if window.row0 == 0 {
                column_edges.push((window.col0, window.col0 + window.width));
            }
            if window.col0 == 0 {
                row_edges.push((window.row0, window.row0 + window.height));
            }
        }
        assert_eq!(column_edges.first().expect("a first column").0, 0);
        assert_eq!(column_edges.last().expect("a last column").1, CANONICAL.width);
        assert_eq!(row_edges.last().expect("a last row").1, CANONICAL.height);
        for pair in column_edges.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "shard columns must abut exactly");
        }
        for pair in row_edges.windows(2) {
            assert_eq!(pair[0].1, pair[1].0, "shard rows must abut exactly");
        }
    }

    /// The shard's south-west corner is a lattice cell corner, and its geometry restates the
    /// lattice rather than the source that painted it.
    #[test]
    fn shard_geometry_is_the_lattice_not_a_source() {
        for (_, window) in CANONICAL.shards() {
            let geometry = CANONICAL.geometry(window);
            assert_eq!(geometry.cell_lat_udeg, CELL_UDEG);
            assert_eq!(geometry.cell_lon_udeg, CELL_UDEG);
            assert_eq!(geometry.cell_size_m, LATTICE_CELL_SIZE_M);
            assert_eq!(i64::from(geometry.south_lat_udeg) % i64::from(CELL_UDEG), 0);
            assert_eq!(i64::from(geometry.west_lon_udeg) % i64::from(CELL_UDEG), 0);
        }
        let last = CANONICAL.shard(CANONICAL.shard_count() - 1).expect("the last shard");
        assert_eq!(CANONICAL.geometry(last).north_lat_udeg(), 90_000_000);
        assert_eq!(CANONICAL.geometry(last).east_lon_udeg(), 180_000_000);
    }

    #[test]
    fn a_cycle_anchors_on_the_quarter_hour_and_spans_two_hours() {
        let times = CycleTimes::anchored_at(manifest::parse_rfc3339("2026-08-09T14:37:11Z").expect("timestamp"));
        assert_eq!(manifest::rfc3339(times.reference_time), "2026-08-09T14:30:00Z");
        let offsets: Vec<u32> = times.offsets_min().collect();
        assert_eq!(offsets, vec![0, 15, 30, 45, 60, 75, 90, 105, 120]);
        assert_eq!(manifest::rfc3339(times.valid_at(120)), "2026-08-09T16:30:00Z");
    }
}
