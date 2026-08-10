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
//! - a source already at the lattice pitch and lattice-aligned (MRMS is) is copied cell for cell;
//! - anything else — a coarser model, or a finer/offset window like DWD RV's 9,000 x 14,000 µdeg
//!   trapezoid — is resampled nearest-neighbour at fill time, which for a coarse source means
//!   **cell replication**: one 6.5 km ICON cell paints a block of identical 1 km cells. That is
//!   the rule `OBCG_Spec.md` §6 already mandates.
//!
//! Doing it lazily is a hard requirement, not a preference. WXR1 (#1254) measured the GO on the
//! condition that the baker materialises **one shard per thread** (255 MB steady state); a global
//! 0.01 degree raster is 648 M cells = 648 MB *per frame*, so a GFS floor eagerly upsampled onto
//! the lattice would cost more than the whole 8 GB box before a single tile was encoded.
//!
//! ## The covered domain, stated rather than assumed
//!
//! The floor source is global in longitude — the mosaic closes the antimeridian seam by wrapping,
//! because a global grid's column east of its last one is its first (see [`source_column`]) — but
//! it is **not** global in latitude: GFS drops the two polar grid points, so the lattice rows
//! beyond ±89.875° are outside every source we ingest. That band is 25 of 18,000 rows, it is ice,
//! and no rider is in it, but it is a permanent hole rather than an outage, so it is named:
//! [`Lattice::covered_rows`], asserted by test, and the reason the sentence below says "every cell
//! in the covered domain" rather than "every cell".
//!
//! ## Why there is no provenance channel
//!
//! Locked 2026-08-10 (#1242, #1248): no per-cell resolution plane, no per-tile source label, no
//! coverage descriptor. The mosaic always has a **global floor source**, so every cell in the
//! covered domain always carries a best-available value and "no radar coverage" renders as model
//! fill rather than as dry. Intensity code 15 stays the honest answer for genuinely missing data —
//! a floor-source outage, a shard that failed to bake, or the polar band above — and it is the
//! only distinction worth carrying. `cell_size_m` is therefore pinned to [`Lattice::cell_size_m`],
//! a constant stating the lattice, rather than describing a per-cell source that no longer has one
//! value.
//!
//! The one flag that *is* still provenance, `FLAG_OBSERVED`, is computed rather than assumed: see
//! [`FillOutcome::all_observed`].

use std::time::Instant;

use obc_formats::obcg::{self, FrameInput};
use obc_formats::precip4;
use rayon::prelude::*;

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;
use crate::manifest;
use crate::manifest_v2;
use crate::publish::{self, ObjectStore, PlannedObject};
use crate::source::{mosaic_rank, Adapter, AdapterOutcome, Attribution, BakedProduct};

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

/// The rayon pool the mosaic fills and encodes on.
///
/// **Pinned, and the memory model depends on it.** WXR1 (#1254) measured 398 MB peak on a 4-vCPU
/// box, and the steady-state decomposition is per-thread: one shard's cells (28.3 MB) plus its
/// `max_encoded_len` bound (14.2 MB) live for the duration of each encode. On a 16-core builder
/// the default global pool would make that transient ~680 MB instead of ~170 MB — a property of
/// the machine, not of the code, which is exactly what a published budget must not be. The
/// production VPS is 4 vCPU; `--threads` overrides for a deliberate re-measurement.
pub const BAKE_THREADS: usize = 4;

/// One full turn of longitude, in microdegrees.
const FULL_CIRCLE_UDEG: i64 = 360_000_000;

/// Does this source's grid close the circle in longitude?
///
/// GFS's *window* does not — the adapter drops the antimeridian column, because a `GridGeometry`
/// window cannot cross ±180° — but its *grid* does: the column east of the last one is the first
/// one again. Under the per-product path that distinction was invisible. Under one global lattice
/// it is a 25-column stripe of permanent no-data through Fiji, so the mosaic has to know.
fn is_globally_periodic(source: &GridGeometry) -> bool {
    let span = i64::from(source.width) * i64::from(source.cell_lon_udeg);
    span + i64::from(source.cell_lon_udeg) >= FULL_CIRCLE_UDEG
}

/// The source column a lattice longitude samples, or `None` if the source does not reach it.
///
/// Two rules, both worth stating because a later source will land on them:
///
/// * The window is **half-open** `[west, east)`, exactly as `OBCG_Spec.md` §3 defines it. A
///   lattice cell centre landing on an *interior* cell boundary takes the eastern cell
///   (`div_euclid`), deterministically; one landing on the window's *outer* edge is outside the
///   window and gets no data. That is consistent rather than convenient — the alternative snaps a
///   cell that is genuinely outside the source back inside it — and it is why
///   [`Lattice::covered_rows`] is one row tighter at the north than at the south.
/// * A **globally periodic** source wraps. The lattice longitude is first brought into the
///   window's own turn of the circle; if it then lands in the single column the window had to drop
///   at the antimeridian, it takes whichever of the first and last columns is nearer measured
///   *around the circle*. That is still plain nearest-neighbour, just on a cylinder.
pub fn source_column(source: &GridGeometry, lon_udeg: i64) -> Option<u32> {
    let cell = i64::from(source.cell_lon_udeg);
    let width = i64::from(source.width);
    let west = i64::from(source.west_lon_udeg);
    let direct = (lon_udeg - west).div_euclid(cell);
    if (0..width).contains(&direct) {
        return Some(direct as u32);
    }
    if !is_globally_periodic(source) {
        return None;
    }
    let wrapped = (lon_udeg - west).rem_euclid(FULL_CIRCLE_UDEG);
    let column = wrapped.div_euclid(cell);
    if column < width {
        return Some(column as u32);
    }
    // The dropped column. Nearest-neighbour on the circle between the two columns flanking it.
    let centre = |column: i64| west + column * cell + cell / 2;
    let circular = |from: i64, to: i64| {
        let raw = (from - to).abs() % FULL_CIRCLE_UDEG;
        raw.min(FULL_CIRCLE_UDEG - raw)
    };
    let last = width - 1;
    Some(if circular(lon_udeg, centre(0)) <= circular(lon_udeg, centre(last)) { 0 } else { last as u32 })
}

/// The source row a lattice latitude samples, or `None` outside the window. Latitude does not
/// wrap; see [`source_column`] for the half-open-edge rule, which applies here too.
pub fn source_row(source: &GridGeometry, lat_udeg: i64) -> Option<u32> {
    let row = (lat_udeg - i64::from(source.south_lat_udeg)).div_euclid(i64::from(source.cell_lat_udeg));
    (0..i64::from(source.height)).contains(&row).then_some(row as u32)
}

/// Does `source` reach the lattice cell at `(col, row)`? The geometric half of coverage — it says
/// nothing about whether the cell there holds data or no-data.
pub fn source_reaches(source: &GridGeometry, lattice: &Lattice, col: u32, row: u32) -> bool {
    source_column(source, lattice.centre_lon_udeg(col)).is_some()
        && source_row(source, lattice.centre_lat_udeg(row)).is_some()
}

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

    /// The `(col, row)` of shard `index`, row-major from the south-west. `(col, row)` is the
    /// shard's **identity** — it is what the object key and the manifest's presence bitmap are
    /// both written in (`manifest_v2::shard_key`); the flat index is only an iteration order.
    pub fn shard_col_row(&self, index: u32) -> (u32, u32) {
        (index % self.shard_cols(), index / self.shard_cols())
    }

    /// Shard `index` in row-major order (south-west first), or `None` past the last shard.
    pub fn shard(&self, index: u32) -> Option<LatticeWindow> {
        if index >= self.shard_count() {
            return None;
        }
        let (col, row) = self.shard_col_row(index);
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

    /// The lattice rows at least one source's domain reaches — **the dataset's covered domain**.
    ///
    /// Derived from the floor source rather than declared, so it cannot drift from it: the last
    /// row of [`crate::source::MOSAIC_PRIORITY`] is the only global source, so where it does not
    /// reach, nothing does. Longitude is fully covered because [`source_column`] wraps; latitude is
    /// not, because GFS's grid drops the two polar points. For [`CANONICAL`] that leaves rows
    /// 12..17987 — 25 rows above 89.875° north or south have no source at all and are published as
    /// intensity 15 forever. The bound is one row tighter at the north than at the south because
    /// the source window is half-open and the lattice row centre at exactly +89.875° lands on its
    /// north edge.
    ///
    /// This is what makes "every cell always carries a best-available value" a checkable claim
    /// instead of a slogan. `covered_domain_is_exactly_what_the_floor_reaches` pins it.
    pub fn covered_rows(&self) -> std::ops::Range<u32> {
        let floor = crate::source::gfs::GEOMETRY;
        let reached = |row: u32| source_row(&floor, self.centre_lat_udeg(row)).is_some();
        let first = (0..self.height).find(|&row| reached(row)).unwrap_or(self.height);
        let end = (first..self.height).find(|&row| !reached(row)).unwrap_or(self.height);
        first..end
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
    /// Was this frame an **observation** upstream (`obcg::FLAG_OBSERVED`), or a forecast? The only
    /// provenance the published dataset still carries, and it is carried per object rather than
    /// assumed — see [`FillOutcome::all_observed`].
    pub observed: bool,
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
    /// The source's licence line. There is no per-cell provenance, so every layer in the mosaic
    /// may have painted any cell and **all** of these must be displayable together — the manifest
    /// carries the list, not a choice.
    pub attribution: Attribution,
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
        let attribution = product.attribution;
        let mut frames = Vec::with_capacity(product.frames.len());
        for frame in product.frames {
            let window = frame.source.map_or(anchor, |source| source.geometry);
            if frame.cells.len() != window.cells() {
                return Err(format!("{id}: frame f{} cell count disagrees with its source window", frame.offset_min));
            }
            frames.push(SourceFrame {
                valid_at: frame.valid_at,
                observed: frame.flags & obcg::FLAG_OBSERVED != 0,
                window,
                cells: frame.cells,
            });
        }
        frames.sort_by_key(|frame| frame.valid_at);
        Ok(Self { id, rank, attribution, frames })
    }

    /// The frame to sample for a canonical frame valid at `valid_at`.
    ///
    /// Three rules, in order:
    ///
    /// 1. nothing further than [`MAX_FRAME_SKEW_S`] away is sampled at all — a source that far out
    ///    of the timeline has fallen out of it, and the mosaic drops through to the next rank;
    /// 2. nearest validity wins;
    /// 3. **an observation never outranks an equally close forecast.** This is the rule that
    ///    matters, and it is not the obvious one. The `us` layer holds a 1 km MRMS observation at
    ///    f0 and 3 km HRRR forecasts ahead of it; without this, a nearest-only tie-break lets the
    ///    frozen observation paint a *forward* frame that a real forecast for that exact instant is
    ///    also offering. Re-using an observation as a forecast is a persistence nowcast, which is
    ///    WXR9 #1251's job to do deliberately and well — not something the frame picker should
    ///    stumble into. It is still allowed when the layer has nothing better inside the skew
    ///    window, because a 15-minute-old radar field beats falling through to a 27.75 km model.
    ///
    /// Remaining ties break toward the *later* frame: at equal distance, the field valid after the
    /// target is about weather that has not happened yet, and the one before it is already past.
    fn nearest(&self, valid_at: i64) -> Option<&SourceFrame> {
        self.frames.iter().filter(|frame| (frame.valid_at - valid_at).abs() <= MAX_FRAME_SKEW_S).min_by_key(|frame| {
            let distance = (frame.valid_at - valid_at).abs();
            (distance, frame.observed && distance != 0, std::cmp::Reverse(frame.valid_at))
        })
    }
}

/// What one [`Mosaic::fill`] actually managed to paint. The emitter turns this into the frame's
/// flags and the cycle into its "did anything at all get baked" gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillOutcome {
    /// Did any source paint any cell of the window? `false` is a window that is entirely no-data.
    pub painted: bool,
    /// Is every cell of this window painted by an **observation** source frame?
    ///
    /// This is the whole of `FLAG_OBSERVED` for the canonical dataset, and it is **exact**, not a
    /// conservative approximation — which is why [`Mosaic::fill`] paints best-rank-first into
    /// empty cells rather than worst-rank-first over the top. A layer that writes nothing that
    /// survives writes nothing at all, so "every layer that owns a cell here was an observation"
    /// is precisely "every cell here is an observation", and it costs one comparison per cell
    /// instead of a 28 MB per-cell provenance plane — the mechanism #1242 refused to build.
    ///
    /// The alternative, which this replaces, was to set `FLAG_OBSERVED` on f0 unconditionally. On a
    /// global mosaic that is a lie for ~85 % of the planet, where f0 is model fill: the same
    /// category of untruth `cell_size_m` was just retired for, and the one the device can still act
    /// on. A shard entirely inside a radar footprint at f0 still gets it, honestly.
    pub all_observed: bool,
    /// Is every cell of this window **dry** — intensity 0, and not one no-data cell among them?
    ///
    /// This is the only condition under which an object is not published (WXR4 #1243): the
    /// manifest's presence bitmap says the shard is dry and there is nothing to fetch. The test is
    /// deliberately the strictest one available — a single no-data cell publishes the object — so
    /// that a bitmap-clear shard can never be an outage, a floor failure or the polar band wearing
    /// a dry shard's clothes. Missing is not dry; only dry is dry.
    pub all_dry: bool,
}

/// The priority mosaic: every source, ordered, resampled onto one lattice on demand.
#[derive(Debug)]
pub struct Mosaic {
    /// Sorted **best rank first** — the order [`MOSAIC_PRIORITY`](crate::source::MOSAIC_PRIORITY)
    /// states, which is the order [`Mosaic::fill`] and [`Mosaic::winner_at`] both walk.
    layers: Vec<MosaicLayer>,
}

impl Mosaic {
    pub fn new(mut layers: Vec<MosaicLayer>) -> Self {
        layers.sort_by(|left, right| left.rank.cmp(&right.rank).then_with(|| left.id.cmp(right.id)));
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
    /// **Per-cell winner selection, literally**: layers are walked best rank first and each writes
    /// only into cells still holding [`precip4::INTENSITY_NODATA`], so the first source that both
    /// covers a cell and has data there owns it and nothing overwrites it. Cells no source covers
    /// keep no-data: missing is never dry.
    ///
    /// The order matters beyond taste. Worst-rank-first with unconditional overwrite computes the
    /// same *cells* one comparison cheaper, but it cannot tell you which layer owns the result —
    /// a model layer entirely overwritten by radar still looks like it painted. Best-rank-first
    /// makes "this layer wrote something" mean "this layer owns a final cell", which is what makes
    /// [`FillOutcome::all_observed`] exact rather than a guess, and it is why `FLAG_OBSERVED` on
    /// this dataset is worth anything.
    pub fn fill(&self, lattice: &Lattice, valid_at: i64, window: LatticeWindow, out: &mut [u8]) -> FillOutcome {
        assert_eq!(out.len(), window.cells(), "fill target does not match the window");
        out.fill(precip4::INTENSITY_NODATA);
        let mut outcome = FillOutcome { painted: false, all_observed: true, all_dry: false };
        // One column map per layer, reused down every row: the east-west nearest-neighbour pick
        // depends only on the column, so a shard pays the division once per column, not per cell.
        let mut columns: Vec<i32> = vec![-1; window.width as usize];
        for layer in &self.layers {
            let Some(frame) = layer.nearest(valid_at) else { continue };
            let source = &frame.window;
            let mut covers = false;
            for (index, slot) in columns.iter_mut().enumerate() {
                *slot = match source_column(source, lattice.centre_lon_udeg(window.col0 + index as u32)) {
                    Some(column) => {
                        covers = true;
                        column as i32
                    }
                    None => -1,
                };
            }
            if !covers {
                continue;
            }
            let mut painted_here = false;
            for row in 0..window.height as usize {
                let Some(source_row) = source_row(source, lattice.centre_lat_udeg(window.row0 + row as u32)) else {
                    continue;
                };
                let base = source_row as usize * source.width as usize;
                let row_cells = &frame.cells[base..base + source.width as usize];
                let destination = &mut out[row * window.width as usize..(row + 1) * window.width as usize];
                for (cell, column) in destination.iter_mut().zip(&columns) {
                    if *column >= 0 && *cell == precip4::INTENSITY_NODATA {
                        let value = row_cells[*column as usize];
                        if value != precip4::INTENSITY_NODATA {
                            *cell = value;
                            painted_here = true;
                        }
                    }
                }
            }
            if painted_here {
                outcome.painted = true;
                outcome.all_observed &= frame.observed;
            }
        }
        outcome.all_observed &= outcome.painted;
        // One pass, and it short-circuits on the first wet or no-data cell — so the wet case, the
        // one that matters for bake time, pays almost nothing, and only a genuinely dry shard
        // walks all 28 M cells to earn its omission from the published set.
        outcome.all_dry = !out.iter().any(|&cell| cell != precip4::INTENSITY_DRY);
        outcome
    }

    /// Which source wins one lattice cell, for diagnostics and for the tests that prove the table
    /// rather than the geometry decides. `None` means no source covers it with data.
    pub fn winner_at(&self, lattice: &Lattice, valid_at: i64, col: u32, row: u32) -> Option<&'static str> {
        let lat = lattice.centre_lat_udeg(row);
        let lon = lattice.centre_lon_udeg(col);
        let mut winner: Option<&'static str> = None;
        for layer in &self.layers {
            let Some(frame) = layer.nearest(valid_at) else { continue };
            let source = &frame.window;
            let (Some(column), Some(source_row)) = (source_column(source, lon), source_row(source, lat)) else {
                continue;
            };
            let value = frame.cells[source_row as usize * source.width as usize + column as usize];
            if value == precip4::INTENSITY_NODATA {
                continue;
            }
            // First hit wins, over the same best-first layer order `fill` walks — so the two
            // cannot disagree, including on an equal rank, without one of them being rewritten.
            winner = Some(layer.id);
            break;
        }
        winner
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
    /// The shard's identity on the grid — what the key and the manifest bitmap are written in.
    pub col: u32,
    pub row: u32,
    pub offset_min: u32,
    pub bytes: Vec<u8>,
    pub object_crc32: u32,
    /// What the mosaic managed to paint into this shard — the source of its flags, of its manifest
    /// presence bit, and of the cycle's "nothing baked at all" gate.
    pub fill: FillOutcome,
}

/// Immutable object key for one shard of one frame:
/// `wx/v2/<generation>/f<offset-min>/s<col>-<row>.obcg`, normative in `OBCG_Spec.md` §10 and
/// composed by [`manifest_v2::shard_key`] — the client computes the identical string from the
/// manifest's `key_prefix` and `generation` plus its own bbox arithmetic.
pub fn shard_key(lattice: &Lattice, reference_time: i64, offset_min: u32, shard: u32) -> String {
    let (col, row) = lattice.shard_col_row(shard);
    manifest_v2::shard_key(manifest_v2::KEY_PREFIX, &manifest_v2::generation_id(reference_time), offset_min, col, row)
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
    // Left uninitialised-as-zero rather than pre-filled with no-data: `fill` is the authority on
    // the initial value and sets every cell to `INTENSITY_NODATA` first, so pre-filling here would
    // be a second 28 MB pass for nothing.
    let mut cells = vec![0u8; window.cells()];
    let fill = mosaic.fill(lattice, valid_at, window, &mut cells);

    let input = FrameInput {
        // The dataset has exactly one product because it *is* the product: one lattice, one cell
        // size, best available everywhere. The per-source codes stay in the registry until WXR7
        // deletes the multi-product path. `tier` says `TIER_MOSAIC`, which means **no tier**
        // (#1243): a frame that is 1 km radar over Germany and 27.75 km model over the Pacific is
        // not "radar", and the header slot must be nonzero, so it gets a code that says so.
        // Manifest v2 carries no tier at all and nothing may select on this byte.
        product_id: obcg::PRODUCT_MOSAIC,
        tier: obcg::TIER_MOSAIC,
        // Measured, not assumed. See `FillOutcome::all_observed`: an f0 shard over open ocean is
        // GFS model fill and says Forecast; one inside a radar footprint says Observed. The header
        // requires exactly one of the two, so an all-no-data shard says Forecast.
        flags: if fill.all_observed { obcg::FLAG_OBSERVED } else { obcg::FLAG_FORECAST },
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
    let (col, row) = lattice.shard_col_row(shard);
    Ok(CanonicalObject {
        key: shard_key(lattice, times.reference_time, offset_min, shard),
        shard,
        col,
        row,
        offset_min,
        object_crc32: header.object_crc32,
        bytes,
        fill,
    })
}

/// Bake and hand over one whole cycle, **one frame at a time**.
///
/// The frame is the streaming unit: its shards are encoded in parallel (one shard of cells per
/// thread, WXR1's condition) and passed to `sink` before the next frame starts, so the baker never
/// holds more than one frame's objects — 24 objects, ~46 MB at the measured worst case — on top of
/// the resident sources.
///
/// `threads` sizes an **own** pool rather than borrowing rayon's global one, because the
/// per-thread working set *is* the memory budget: a shard's cells plus its encode bound live for
/// the duration of each encode, so the peak is `threads x ~42.5 MB`. [`BAKE_THREADS`] is the
/// production value and the one WXR1's 398 MB was measured at. The `sink` runs on the calling
/// thread, outside the pool — it owns the object store, which is not `Send`.
pub fn bake_cycle(
    lattice: &Lattice,
    mosaic: &Mosaic,
    times: CycleTimes,
    threads: usize,
    sink: &mut dyn FnMut(CanonicalObject) -> Result<(), String>,
) -> Result<(), String> {
    lattice.validate()?;
    if threads == 0 {
        return Err("the bake pool needs at least one thread".to_string());
    }
    let pool =
        rayon::ThreadPoolBuilder::new().num_threads(threads).build().map_err(|error| format!("bake pool: {error}"))?;
    for offset_min in times.offsets_min() {
        // Collected as `Vec<Result<_>>` rather than straight into `Result<Vec<_>>`: rayon's
        // `FromParallelIterator for Result` yields *an* error, not the first one, so two bad shards
        // would give a different message run to run. A baker's failure text has to be reproducible.
        let results: Vec<Result<CanonicalObject, String>> = pool.install(|| {
            (0..lattice.shard_count())
                .into_par_iter()
                .map(|shard| emit_shard(lattice, mosaic, times, offset_min, shard))
                .collect()
        });
        let mut objects = Vec::with_capacity(results.len());
        for result in results {
            objects.push(result?);
        }
        for object in objects {
            sink(object)?;
        }
    }
    Ok(())
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
    /// Shards that were entirely dry and so were **not** published; their manifest presence bit is
    /// clear. Reported because a sudden jump in it is the shape a broken source has.
    pub dry_shards: usize,
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
            "fetched {} upstream bytes; published {} objects / {} bytes ({} dry shards omitted); {} ms",
            self.fetched_bytes, self.published_objects, self.published_bytes, self.dry_shards, self.elapsed_ms
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
/// `threads` is [`BAKE_THREADS`] in production and is what the memory budget is stated against.
pub fn run_canonical_cycle(
    lattice: &Lattice,
    adapters: &[&dyn Adapter],
    upstream: &mut dyn Upstream,
    store: &mut dyn ObjectStore,
    now: i64,
    threads: usize,
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
    // Every source in the mosaic may have painted any cell, so every licence line travels with the
    // dataset, in priority order — there is no per-cell provenance to attribute more precisely.
    let attribution = mosaic
        .layers()
        .iter()
        .map(|layer| manifest_v2::AttributionEntry {
            source_id: layer.id.to_string(),
            text: layer.attribution.text.to_string(),
            url: layer.attribution.url.to_string(),
        })
        .collect();
    // The retention chain is read back out of the published manifest rather than kept anywhere:
    // the baker is stateless, and its only state is the document it publishes.
    let carried = manifest_v2::carried_generations(store.get(manifest_v2::MANIFEST_KEY)?.as_deref());
    let mut document = manifest_v2::Builder::new(lattice, times, now, attribution, carried);

    let mut published_objects = 0usize;
    let mut published_bytes = 0u64;
    let mut fetchable: Vec<(String, u64)> = Vec::new();
    let mut painted_objects = 0usize;
    let mut dry_shards = 0usize;
    let mut total_objects = 0usize;
    bake_cycle(lattice, &mosaic, times, threads, &mut |object| {
        total_objects += 1;
        painted_objects += usize::from(object.fill.painted);
        // A shard whose every cell is dry is not published, and the manifest's presence bitmap says
        // so — the bit stays clear and the client reads "dry here", never a 404 it has to interpret.
        // A shard with so much as one no-data cell *is* published, so absence can never be an
        // outage wearing a dry shard's clothes.
        if object.fill.all_dry {
            dry_shards += 1;
            return Ok(());
        }
        document.record(
            object.offset_min,
            object.col,
            object.row,
            object.bytes.len() as u64,
            object.object_crc32,
            object.fill.all_observed,
        );
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

    // A cycle in which no source painted a single cell is not a dataset, it is 216 objects of
    // "we do not know" about the whole planet — the state where every adapter fell out of the skew
    // window at once, or the floor is gone. Publishing it would swap a manifest that says the
    // service is current, so it fails closed and the previous generation stands. WXR8's freshness
    // probe is the backstop; this is the baker refusing to be the cause.
    if total_objects > 0 && painted_objects == 0 {
        return Err(format!(
            "every one of the {total_objects} baked objects is entirely no-data — no source reached the lattice; \
             refusing to publish a blank cycle"
        ));
    }

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
            key: manifest_v2::MANIFEST_KEY.to_string(),
            bytes: manifest_v2::to_json(&document.finish()).into_bytes(),
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
        dry_shards,
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
