//! The lattice, the priority mosaic, and the sharded emit — **the cycle** (WXR3 #1242).
//!
//! This is the module the epic's central sentence lives in: **the baker is the only component
//! that knows a data source exists.** Every adapter's output is normalised onto one global
//! 0.01 degree lattice, overlapping sources are resolved per cell by one ordered priority table
//! ([`crate::source::MOSAIC_PRIORITY`]), and what leaves the bakery is a single provider-agnostic
//! dataset: 24 shards x 9 frames of OBCG, all on the same lattice, all at the same cell size. It
//! is the only thing the bakery publishes; #1246 deleted the per-product path beside it.
//!
//! ## Why the resample is here and not in each adapter
//!
//! The adapters keep fetch, decode, reproject and quantize verbatim; what changed is the meaning
//! of their `GEOMETRY` const. It is now a **source-window description** — where this source has
//! data and at what pitch — rather than an output lattice. The last stage, "resample onto the
//! canonical lattice", is this one shared nearest-neighbour implementation, and it runs **lazily,
//! per shard**:
//!
//! - a source already at the lattice pitch and lattice-aligned is copied cell for cell — not by a
//!   fast path, of which [`Mosaic::fill`] has none, but because its nearest-neighbour pick is then
//!   the identity. MRMS is, and DWD RV is too since #1246 freed its window to move; an adapter
//!   whose window *can* be put on the lattice should be, because the alternative is a second
//!   rounding of an already-rounded reprojection (see `dwd_rv::GEOMETRY`);
//! - anything else — a coarser model like ICON-EU or the GFS floor, or a window whose origin the
//!   upstream grid fixes off the lattice — is resampled nearest-neighbour at fill time, which for a
//!   coarse source means **cell replication**: one 6.5 km ICON cell paints a block of identical
//!   1 km cells. That is the rule `OBCG_Spec.md` §6 already mandates.
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
//! [`shard_is_observed`].
//!
//! ## Forward frames are forecasts
//!
//! Locked 2026-08-11 (#1248). A frame at `offset_min > 0` may only be painted by **forecast** source
//! data; an observation is eligible for the anchor and for nothing else, everywhere, however near
//! its measurement instant happens to sit. The rule is one function — [`frame_is_eligible`] —
//! applied before the skew window and before the priority table, and it is why a forward frame can
//! never be a repeated "now" image wearing a future `valid_at`.
//!
//! It turns on the source data's **class**, not on its distance, and the two must not be run
//! together. The skew window still applies on top, so one hourly model step is *permitted* to paint
//! four consecutive 15-minute frames — the 11:00 step reaches 10:30, 10:45, 11:00 and 11:15,
//! because the :30 instants are 1,800 s from both flanking steps and the tie breaks toward the later
//! one ([`MosaicLayer::nearest`]). The guarantee that rule alone gives is "a prediction", not "a
//! prediction of exactly this instant". What makes it different from the frozen observation is what
//! the data is *about*: the nearest model step is a defensible answer for 17:15, and a radar scan of
//! 16:58 is not an answer for 17:15 at all.
//!
//! ## …and since WXR9, they are predictions of their own instant
//!
//! Locked 2026-08-11 (#1251). The paragraph above describes what the *rule* allows, and until WXR9
//! it also described what the dataset did: in a GFS-only region — most of the planet — one hourly
//! step really did paint four frames, so the timeline changed once an hour and stood still in
//! between. [`crate::derive`] closes that. Between the adapters and the mosaic, every canonical
//! instant an hourly source skips is filled by morphing its two bracketing steps to that instant
//! along the estimated motion field, and every radar source with a motion baseline gains advected
//! forward frames of its own. The skew window is still the fall-back and still admits four frames
//! per step; it is simply not what happens any more when the source has enough to interpolate from.
//!
//! `OBCG_Spec.md` §3.2 states both halves: `valid_at` is the frame's cadence instant, and the frame
//! is an estimate **for** it.

use std::time::{Duration, Instant};

use obc_formats::obcg::{self, FrameInput};
use obc_formats::precip4;
use rayon::prelude::*;

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;
use crate::manifest_v2;
use crate::publish::{self, ObjectStore, PlannedObject};
use crate::source::{mosaic_rank, Adapter, Attribution, BakedSource, SourceClass};
use crate::timefmt;

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
///
/// **The margin against an hourly source is exactly zero, and the comparison is `<=`.** A cycle
/// anchors on a quarter hour, so every frame instant lands at :00, :15, :30 or :45; the ones at
/// :30 are 1,800 s from *both* flanking hourly steps and are sampled only because the bound is
/// inclusive. Lowering this constant by one second, or moving the cadence off the quarter hour,
/// drops the floor out of two of every four frames — which since #1248 is no longer masked by a
/// radar observation stepping in, and publishes intensity 15 instead. Do not retune it without
/// reading `the_hourly_floor_reaches_every_offset_with_exactly_zero_margin`, which fails if either
/// number moves.
///
/// It is also a *distance* bound and nothing more. Being inside it does not make a source frame
/// eligible for a canonical frame — see [`frame_is_eligible`], which the skew window is applied
/// on top of, never instead of.
pub const MAX_FRAME_SKEW_S: i64 = 1_800;

/// **How recent an observation has to be to own the anchor outright** (#1278 r2, R2-2).
///
/// At f0 — and only at f0 — an observation this close to the frame's instant beats every forecast
/// the same layer offers, however much nearer the forecast is. The rule and the case that forced it
/// are at [`MosaicLayer::nearest`]; this is the number.
///
/// One cadence step, and that is a derivation rather than a taste. A cycle anchors at the quarter
/// hour at or before `now`, and every observation source discovers the newest object at or before
/// `now`, so the observation a layer offers f0 is inside one step of the anchor by construction —
/// the preference covers the whole of the case it exists for and not one second more. Past it the
/// observation competes on distance like anything else, which is what keeps a genuinely stale scan
/// from displacing a model step that is actually about the instant.
pub const ANCHOR_OBSERVATION_PREFERENCE_S: i64 = (FRAME_STEP_MIN as i64) * 60;

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
pub fn is_globally_periodic(source: &GridGeometry) -> bool {
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
    /// Observation or forecast, carried verbatim from the adapter ([`crate::source::SourceClass`]).
    /// It decides two separate things and they must not be confused: **which canonical frames this
    /// frame may paint at all** ([`frame_is_eligible`]), and, for the ones it does paint, whether
    /// they may claim `FLAG_OBSERVED` ([`FillOutcome::all_observed`]).
    pub class: SourceClass,
    /// The **source window**: where this source has data and at what pitch. Not an output
    /// lattice — the mosaic resamples from it.
    pub window: GridGeometry,
    pub cells: Vec<u8>,
}

/// **Which canonical frame is being painted**: its place on the cadence and the instant it is
/// about. The two travel together everywhere the mosaic is asked a question, because
/// [`frame_is_eligible`] needs the offset and the distance rule needs the instant, and a caller
/// that could pass one without the other could ask for a forward frame while the mosaic believed
/// it was painting the anchor.
///
/// The fields are private and there are exactly two constructors — [`CycleTimes::slot`], which
/// derives both from one reference time, and [`FrameSlot::anchor`], which can only make an anchor.
/// Neither can produce a pair that disagrees, which is what makes the paragraph above a property
/// rather than a hope: with public fields `FrameSlot { offset_min: 0, valid_at: t + 900 }` would
/// have been an ordinary struct literal that quietly re-admits observations to a forward frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSlot {
    offset_min: u32,
    valid_at: i64,
}

impl FrameSlot {
    /// The anchor slot of a cycle referenced at `valid_at` — f0, the only slot an observation may
    /// paint.
    pub fn anchor(valid_at: i64) -> Self {
        Self { offset_min: 0, valid_at }
    }

    /// Minutes ahead of the cycle's reference time; `0` is the anchor.
    pub fn offset_min(&self) -> u32 {
        self.offset_min
    }

    /// `reference_time + offset_min x 60` — what the emitted frame's `valid_at` states.
    pub fn valid_at(&self) -> i64 {
        self.valid_at
    }
}

/// **The eligibility rule: a forward frame is a forecast, always** (Timo's decision, #1248).
///
/// A source frame that was an *observation* upstream has one valid time and it is a measurement
/// instant in the past. It is therefore eligible for exactly one canonical frame — the anchor, the
/// only frame whose instant an observation can exist for. Every frame at `offset_min > 0` is about
/// the future, and what may paint it is a **forecast**: HRRR's leads, ICON-EU's and GFS's steps, and
/// DWD RV's own nowcast members, which the adapter already classes `Forecast` for every lead but 0.
///
/// It is a test on the class and not on the distance, and the skew window still runs on top of it,
/// so a forecast may paint a frame it is not exactly valid at. **The quantity is four consecutive
/// 15-minute frames per hourly step** — the 11:00 step wins 10:30, 10:45, 11:00 and 11:15, since a
/// :30 instant sits 1,800 s from both flanking steps and [`MosaicLayer::nearest`] breaks that tie
/// toward the later one. Ordinary rather than degraded, and worth stating as a number: it is the
/// thing a reader would otherwise assume away.
///
/// That is not the same latitude the frozen observation was taking: the nearest prediction is a
/// defensible answer for 17:15, and a measurement of 16:58 is not an answer for 17:15 in any sense.
/// `OBCG_Spec.md` §3.2 is normative and says exactly this.
///
/// This replaces the rule WXR7 shipped, which let any observation inside [`MAX_FRAME_SKEW_S`]
/// paint f+15 and f+30 as long as the object said Forecast. Labelling it honestly was not enough:
/// one "now" image published at three validities is three statements about three instants, and
/// only one of them is about the instant anything measured. Where no forecast source reaches a
/// forward frame the honest answer is intensity 15, not a frozen field — and in practice the
/// hourly GFS floor reaches all nine offsets, which
/// `the_floor_offers_an_eligible_forecast_at_every_one_of_the_nine_offsets` pins.
///
/// Persistence as a *deliberate* product — a radar-derived forecast, extrapolated rather than
/// frozen — is WXR9 #1251's job. It joins the mosaic as a source whose forward frames are
/// forecasts, and this rule admits it on exactly those terms.
pub fn frame_is_eligible(class: SourceClass, offset_min: u32) -> bool {
    if offset_min == 0 {
        // The anchor takes anything: an observation of that instant, or a forecast for it.
        return true;
    }
    // Exhaustive on purpose, and `#[non_exhaustive]`-free on purpose, so adding a variant to
    // `SourceClass` fails to compile here rather than defaulting into forward eligibility. A
    // `!is_observation()` would have admitted a future `Analysis` or `Nowcast` variant silently —
    // the same shape as the `flags & FLAG_OBSERVED != 0` this rule just replaced one level down.
    match class {
        SourceClass::Observation => false,
        SourceClass::Forecast => true,
    }
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
    /// Turn one adapter's output into a layer.
    pub fn from_source(source: BakedSource) -> Result<Self, String> {
        let rank = mosaic_rank(source.id)
            .ok_or_else(|| format!("{}: no row in source::MOSAIC_PRIORITY — the mosaic cannot rank it", source.id))?;
        let window = source.geometry;
        let id = source.id;
        let attribution = source.attribution;
        let mut frames = Vec::with_capacity(source.frames.len());
        for frame in source.frames {
            if frame.cells.len() != window.cells() {
                return Err(format!("{id}: frame f{} cell count disagrees with its source window", frame.offset_min));
            }
            // `class` is a two-variant enum with no default, so this cannot silently decode a
            // forgotten or malformed classification as Forecast — which the old `flags & bit != 0`
            // did for `0`, for both bits, and for any reserved bit. See `source::SourceClass`.
            frames.push(SourceFrame { valid_at: frame.valid_at, class: frame.class, window, cells: frame.cells });
        }
        frames.sort_by_key(|frame| frame.valid_at);
        Ok(Self { id, rank, attribution, frames })
    }

    /// The frame to sample for canonical frame `slot`.
    ///
    /// Three rules, in order:
    ///
    /// 1. **eligibility** — [`frame_is_eligible`]: a forward slot takes forecasts only, so an
    ///    observation is refused outright there however near it sits. This is a filter and not a
    ///    preference; there is no "unless the layer has nothing better", because a frozen
    ///    observation is not a weaker answer to "what will the sky be doing at 19:15", it is an
    ///    answer to a different question;
    /// 2. nothing further than [`MAX_FRAME_SKEW_S`] away is sampled at all — a source that far out
    ///    of the timeline has fallen out of it, and the mosaic drops through to the next rank;
    /// 3. nearest validity wins.
    ///
    /// **Rule 3 has an exception at the anchor, and it is the design rule rather than a tie-break:
    /// f0 is what an observation is for.** A recent observation — within
    /// [`ANCHOR_OBSERVATION_PREFERENCE_S`] of the instant — beats *any* forecast at f0, however
    /// much nearer the forecast is. Everything below that promotion is ordinary nearest-validity,
    /// breaking toward the *later* frame, and at every forward slot rule 1 has already left only
    /// forecasts so none of this applies.
    ///
    /// This is a deliberate change made in round 2 of #1278's review, and the case that forced it is
    /// worth recording. DWD RV's members are five minutes apart and its run is on a five-minute
    /// boundary, so when the run sits 300 s *after* the anchor, the tar offers a lead-5 **forecast**
    /// valid at exactly f0's instant and a lead-0 **observation** 300 s away. Plain nearest-validity
    /// took the forecast, distance 0 — and with it went `FLAG_OBSERVED` over the whole of Germany,
    /// in one run phase out of three, flapping with RV's publication schedule. What f0 asks is "is it
    /// raining on me *now*", and a five-minute-old radar composite answers that better than a
    /// zero-minute-old extrapolation **of that same composite**, which is exactly what an RV lead-5
    /// member is. The old ordering said the opposite in its own comment ("an equally close forecast
    /// beats an observation that is not valid at the target instant") and that reasoning is sound for
    /// two *unrelated* fields; it is not sound when the forecast is derived from the observation it
    /// is beating, and it was never the rule #1248 set.
    ///
    /// The preference is **bounded** rather than absolute, because a genuinely stale scan is a
    /// different thing: past [`ANCHOR_OBSERVATION_PREFERENCE_S`] the observation goes back to
    /// competing on distance like anything else, and past [`MAX_FRAME_SKEW_S`] it is not sampled at
    /// all. `the_anchor_prefers_a_recent_observation_over_an_exact_forecast` pins both edges.
    ///
    /// Rule 1 is not a within-layer rule and could not be one. Under WXR7 it was, so the priority
    /// table let a higher-ranked single-frame radar layer paint forward frames that a lower-ranked
    /// model layer had real forecasts for — the MRMS-over-HRRR case #1248 closed. Eligibility is
    /// decided per source frame, before any layer is compared with any other.
    fn nearest(&self, slot: FrameSlot) -> Option<&SourceFrame> {
        self.frames
            .iter()
            .filter(|frame| {
                frame_is_eligible(frame.class, slot.offset_min)
                    && (frame.valid_at - slot.valid_at).abs() <= MAX_FRAME_SKEW_S
            })
            .min_by_key(|frame| {
                let distance = (frame.valid_at - slot.valid_at).abs();
                // `false` sorts first, so this promotes a recent observation at the anchor above
                // every forecast, whatever their distances.
                let demoted = !(slot.offset_min == 0
                    && frame.class.is_observation()
                    && distance <= ANCHOR_OBSERVATION_PREFERENCE_S);
                (demoted, distance, std::cmp::Reverse(frame.valid_at))
            })
    }
}

/// What one [`Mosaic::fill`] actually managed to paint. The emitter turns this into the frame's
/// flags and the cycle into its "did anything at all get baked" gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FillOutcome {
    /// Did any source paint any cell of the window? `false` is a window that is entirely no-data.
    pub painted: bool,
    /// Was every cell of this window painted by a source frame that was an **observation
    /// upstream**?
    ///
    /// **This is one of the two halves of `FLAG_OBSERVED`, not the whole of it** — see
    /// [`CanonicalObject::observed`] for the other. It is exact rather than a conservative
    /// approximation, which is why [`Mosaic::fill`] paints best-rank-first into empty cells rather
    /// than worst-rank-first over the top: a layer that writes nothing that survives writes nothing
    /// at all, so "every layer that owns a cell here was an observation" is precisely "every cell
    /// here came from an observation", and it costs one comparison per cell instead of a 28 MB
    /// per-cell provenance plane — the mechanism #1242 refused to build.
    ///
    /// The alternative, which this replaces, was to set `FLAG_OBSERVED` on f0 unconditionally. On a
    /// global mosaic that is a lie for ~85 % of the planet, where f0 is model fill: the same
    /// category of untruth `cell_size_m` was just retired for, and the one the device can still act
    /// on.
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

    pub fn from_sources(sources: Vec<BakedSource>) -> Result<Self, String> {
        let layers = sources.into_iter().map(MosaicLayer::from_source).collect::<Result<Vec<_>, _>>()?;
        Ok(Self::new(layers))
    }

    pub fn layers(&self) -> &[MosaicLayer] {
        &self.layers
    }

    /// Paint one window of `lattice` for canonical frame `slot`.
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
    pub fn fill(&self, lattice: &Lattice, slot: FrameSlot, window: LatticeWindow, out: &mut [u8]) -> FillOutcome {
        assert_eq!(out.len(), window.cells(), "fill target does not match the window");
        out.fill(precip4::INTENSITY_NODATA);
        let mut outcome = FillOutcome { painted: false, all_observed: true, all_dry: false };
        // One column map per layer, reused down every row: the east-west nearest-neighbour pick
        // depends only on the column, so a shard pays the division once per column, not per cell.
        let mut columns: Vec<i32> = vec![-1; window.width as usize];
        for layer in &self.layers {
            let Some(frame) = layer.nearest(slot) else { continue };
            let source = &frame.window;
            let mut covers = false;
            for (index, mapped) in columns.iter_mut().enumerate() {
                *mapped = match source_column(source, lattice.centre_lon_udeg(window.col0 + index as u32)) {
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
                outcome.all_observed &= frame.class.is_observation();
            }
        }
        outcome.all_observed &= outcome.painted;
        // One pass, and it short-circuits on the first wet or no-data cell — so the wet case, the
        // one that matters for bake time, pays almost nothing, and only a genuinely dry shard
        // walks all 28 M cells to earn its omission from the published set.
        outcome.all_dry = !out.iter().any(|&cell| cell != precip4::INTENSITY_DRY);
        outcome
    }

    /// Every source that may have painted a cell, **in priority order** — the manifest's
    /// `attribution[]`.
    ///
    /// It is the whole layer set and not a subset, and that is forced rather than chosen: there is
    /// no per-cell provenance (#1242), so any cell of any shard may have come from any layer, and
    /// every one of these lines has to be displayable together. A source joining the mosaic
    /// therefore joins this list by existing — WXR6's two OPERA rows needed no edit here — which is
    /// the only arrangement where a licence cannot be silently dropped by forgetting a second place.
    pub fn attribution(&self) -> Vec<manifest_v2::AttributionEntry> {
        self.layers
            .iter()
            .map(|layer| manifest_v2::AttributionEntry {
                source_id: layer.id.to_string(),
                text: layer.attribution.text.to_string(),
                url: layer.attribution.url.to_string(),
            })
            .collect()
    }

    /// Which source wins one lattice cell, for diagnostics and for the tests that prove the table
    /// rather than the geometry decides. `None` means no source covers it with data.
    pub fn winner_at(&self, lattice: &Lattice, slot: FrameSlot, col: u32, row: u32) -> Option<&'static str> {
        let lat = lattice.centre_lat_udeg(row);
        let lon = lattice.centre_lon_udeg(col);
        let mut winner: Option<&'static str> = None;
        for layer in &self.layers {
            let Some(frame) = layer.nearest(slot) else { continue };
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

    /// The [`FrameSlot`] at `offset_min` — the pair every mosaic question is asked in.
    pub fn slot(&self, offset_min: u32) -> FrameSlot {
        FrameSlot { offset_min, valid_at: self.valid_at(offset_min) }
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
    /// What the mosaic managed to paint into this shard — the source of its manifest presence bit
    /// and of the cycle's "nothing baked at all" gate.
    pub fill: FillOutcome,
    /// **`FLAG_OBSERVED` as published**, and the same bit the manifest's per-shard `observed`
    /// carries. Both read this rather than [`FillOutcome::all_observed`], so the two can never
    /// disagree and a caller cannot forget the offset rule below.
    pub observed: bool,
}

/// Does this shard get `FLAG_OBSERVED`?
///
/// Two conditions, and since #1248 they coincide:
///
/// 1. every cell came from a source frame that was an observation upstream
///    ([`FillOutcome::all_observed`]); and
/// 2. **this is the anchor frame.**
///
/// The second is now implied by the first. [`frame_is_eligible`] refuses an observation for every
/// slot but f0, so a shard at `offset_min > 0` cannot have been painted by one at all and
/// `all_observed` is false there by construction — the two facts `OBCG_Spec.md` §3.2 requires are
/// one fact about where an observation is allowed to be. That is a much better place to be than
/// where WXR7 left it: the offset clause used to be load-bearing, catching frozen observations that
/// the frame picker had already handed a future validity to, and it is normative in the spec
/// precisely because that class of object was possible. It is not possible any more.
///
/// The clause stays in the code regardless. It is the spec's sentence written down at the one place
/// the bit is decided, it costs a comparison per shard, and it means the flag survives a future
/// source or picker change that reopens the gap — `an_observation_can_only_ever_paint_the_anchor`
/// pins the coincidence so the redundancy cannot quietly become a disagreement.
///
/// Note what is *not* required: that the source frame be valid at exactly `valid_at`. An
/// observation almost never lands on the quarter hour, so demanding equality would make every f0
/// a forecast and throw away the one provenance bit the device still acts on. Being inside the skew
/// window is the contract, and the manifest states `max_source_skew_s` so a consumer can caveat
/// "radar, up to N minutes old" with a number instead of a guess.
///
/// `obc-wx-client` reaches the same offset clause and **not** the same second one, which is worth
/// stating rather than glossing: its `observed_frame` is `offset_min == 0 && (now - valid_at).abs()
/// <= max_source_skew_s`, a *freshness* test against the reader's wall clock, where this is a
/// *provenance* test about what painted the cells. They answer different questions and can
/// disagree — a client reading an hour-old generation calls its f0 a forecast even though the
/// object says Observed, which is the client being right about its own situation, per
/// `OBCW_Spec.md` §5.1. Neither is derived from the other.
pub fn shard_is_observed(offset_min: u32, fill: &FillOutcome) -> bool {
    offset_min == 0 && fill.all_observed
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
    let slot = times.slot(offset_min);
    // Left uninitialised-as-zero rather than pre-filled with no-data: `fill` is the authority on
    // the initial value and sets every cell to `INTENSITY_NODATA` first, so pre-filling here would
    // be a second 28 MB pass for nothing.
    let mut cells = vec![0u8; window.cells()];
    let fill = mosaic.fill(lattice, slot, window, &mut cells);
    let observed = shard_is_observed(offset_min, &fill);
    // Measured, not assumed. See `shard_is_observed`: an f0 shard over open ocean is GFS model fill
    // and is a Forecast; one inside a radar footprint is an Observation; and every frame ahead of f0
    // is a Forecast, because nothing but a forecast is allowed to paint one. An all-no-data shard
    // says Forecast, which is what the format's "exactly one source-class bit" leaves.
    let class = if observed { SourceClass::Observation } else { SourceClass::Forecast };

    let input = FrameInput {
        // The one place the published bit is written, and it goes through `SourceClass::obcg_flag`
        // so the emitter and the adapters cannot drift into two different mappings.
        flags: class.obcg_flag(),
        valid_at: slot.valid_at,
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
        observed,
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
pub struct CycleReport {
    /// `(source id, priority rank, frames contributed)`, best rank first.
    pub layers: Vec<(String, usize, usize)>,
    /// What WXR9's derivation stage did this cycle: the nowcast layers it built, the frames it
    /// interpolated onto the cadence, and the nowcasts it could not build. Reported rather than
    /// warned — see [`crate::derive::DeriveReport`] for why the two channels are separate.
    pub derived: crate::derive::DeriveReport,
    pub reference_time: i64,
    pub fetched_bytes: u64,
    pub published_objects: usize,
    /// Shards that were entirely dry and so were **not** published; their manifest presence bit is
    /// clear. Reported because a sudden jump in it is the shape a broken source has.
    pub dry_shards: usize,
    pub published_bytes: u64,
    /// What the retention sweep retired after the manifest swap (WXR8 #1247). Empty for the first
    /// three cycles of a fresh bucket and for any re-bake; one generation per cycle in steady
    /// state. `accounted_bytes` is real against a directory store and `0` against R2, which cannot
    /// report a deleted object's length without a second round-trip — see [`crate::sweep`].
    pub swept: crate::sweep::SweepReport,
    pub elapsed_ms: u128,
    pub warnings: Vec<String>,
}

impl CycleReport {
    pub fn summary(&self) -> String {
        let mut lines = vec![format!("cycle anchored at {}", timefmt::rfc3339(self.reference_time))];
        for (id, rank, frames) in &self.layers {
            lines.push(format!("  #{rank} {id}: {frames} source frames"));
        }
        lines.extend(self.derived.lines());
        lines.push(format!(
            "fetched {} upstream bytes; published {} objects / {} bytes ({} dry shards omitted); {} ms",
            self.fetched_bytes, self.published_objects, self.published_bytes, self.dry_shards, self.elapsed_ms
        ));
        if !self.swept.generations.is_empty() {
            let bytes = if self.swept.accounted_bytes > 0 {
                format!(" / {} bytes", self.swept.accounted_bytes)
            } else {
                String::new()
            };
            lines.push(format!(
                "retired {} ({} objects{bytes})",
                self.swept.generations.join(", "),
                self.swept.deleted_objects
            ));
        }
        for warning in &self.warnings {
            lines.push(format!("warning: {warning}"));
        }
        lines.join("\n")
    }
}

/// Read the just-published manifest back and prove it is the document we wrote.
///
/// The frames get this treatment already — `head` at the exact length, before the swap, "every
/// object the manifest is about to name must already be fetchable". This is the same rule applied
/// to the one object that was exempt from it, and it is the object that matters most: unreadable,
/// it wedges every subsequent cycle by §10.4's torn-read rule *and* it is what licenses the sweep
/// standing directly below this call.
///
/// A failure here fails the cycle. That is deliberate even though the objects and the manifest are
/// already published: what this detects is a manifest at the live key that is not the one this
/// process intended, which is a state an operator needs in the journal now rather than as a wedged
/// cycle in fifteen minutes. Nothing is deleted, so the previous generations all still stand.
fn verify_published_manifest(store: &mut dyn ObjectStore, written: &[u8]) -> Result<(), String> {
    let key = manifest_v2::MANIFEST_KEY;
    let Some(readback) = store.get(key)? else {
        return Err(format!(
            "{key}: published, then read back as absent — refusing to sweep against a manifest that is not there"
        ));
    };
    if readback != written {
        return Err(format!(
            "{key}: read back {} bytes, not the {} just written — refusing to sweep against a \
             manifest that is not the one this cycle published",
            readback.len(),
            written.len()
        ));
    }
    manifest_v2::from_json(&readback)
        .map(|_| ())
        .map_err(|error| format!("{key}: published but does not parse back ({error}) — refusing to sweep"))
}

/// **A cycle must never publish a manifest older than the one already at the key.**
///
/// Round 1 of #1274's review reproduced why. The sweep itself is correct at every step — it only
/// ever deletes generations *its own* manifest does not name — but a manifest that goes *backwards*
/// republishes an old chain over a newer one, and that old chain names generations the newer cycles
/// legitimately swept. By `OBCG_Spec.md` §10.3 a 404 on a named generation is an **error**, not a
/// degradation, so every client that falls back gets one; and it persists two more cycles, because
/// the next cycle faithfully carries the bad chain forward.
///
/// Two ways in, and neither needs a race between timers:
///
/// * a bake started **by hand** while another is running. The unit serializes instances with
///   `flock`, but the lock is in `ExecStart`, not in this binary, so a bare `obc-wx-bake cycle
///   --r2` typed into a shell is outside it. The runbook now routes every manual bake through
///   `systemctl start`; this is the half of that fix that does not depend on anyone reading it.
/// * a **backwards clock step** of one cadence step or more, with no concurrency at all
///   (`ProtectClock=yes` stops the *service* changing the clock, not the host).
///
/// One comparison collapses the whole class into a lost tick, with the same fail-closed posture
/// §10.4 already demands of a torn read. Equality is fine: re-baking a reference time is the
/// idempotent republish the whole design rests on.
fn refuse_to_go_backwards(carried: &manifest_v2::Carried, times: CycleTimes) -> Result<(), String> {
    let Some(previous) = carried.named().first() else { return Ok(()) };
    let Some(previous_time) = timefmt::parse_key_timestamp(previous) else { return Ok(()) };
    if previous_time > times.reference_time {
        return Err(format!(
            "the published manifest is generation {previous} ({}), which is newer than the {} this \
             cycle is anchored at ({}) — refusing to publish a manifest that goes backwards, because \
             its retention chain would name generations a later cycle already swept. Check the box's \
             clock, and that no bake was started by hand outside `systemctl start \
             obc-wx-bake@cycle.service` (the flock lives in the unit, not in this binary)",
            timefmt::rfc3339(previous_time),
            manifest_v2::generation_id(times.reference_time),
            timefmt::rfc3339(times.reference_time),
        ));
    }
    Ok(())
}

/// How long the publish may take: every object written, every one proved fetchable, the manifest
/// swapped and read back.
///
/// It exists because the alternative backstop is the unit's `TimeoutStartSec=600`, and that is a
/// SIGKILL — delivered at an unknown point, quite possibly between the manifest swap and the
/// read-back, which is the one window where nobody can say what state the bucket is in. A budget
/// turns that into an ordinary `Err` before the swap. Sized against the measured phase (~23 s at a
/// 40 ms round trip) with an order of magnitude of headroom, and it plus [`SWEEP_BUDGET`] leaves
/// well over half the unit's timeout for the fetch and the mosaic.
const PUBLISH_BUDGET: Duration = Duration::from_secs(240);

/// How long the retention sweep may take. Separate from [`PUBLISH_BUDGET`] and spent *after* the
/// cycle has already succeeded: an exhausted sweep budget is a warning about unreferenced objects
/// the lifecycle rule will collect, never a failed cycle.
const SWEEP_BUDGET: Duration = Duration::from_secs(120);

/// **The cycle**: bake every adapter, mosaic them, publish the shard set, manifest last — then
/// retire the generation the new manifest no longer names.
///
/// The sweep is the last step for the reason `crate::sweep` exists to state: it is the only
/// destructive operation in this crate, its licence is a chain carried out of a predecessor
/// manifest this cycle read successfully, and it runs strictly after the swap so that nothing it
/// deletes was ever referenced by a document a client could still be holding. Its failures are
/// warnings — the objects are already unreferenced — and the bucket's 1-day lifecycle rule is the
/// backstop for whatever it leaks.
///
/// It never short-circuits on an unchanged upstream — the mosaic needs every source's *cells*, not
/// the knowledge that its objects are already published, so every source is re-fetched and
/// re-decoded every cycle. Caching decoded upstreams across cycles is a WXR8 ops question, not a
/// correctness one.
///
/// A failure anywhere before the manifest swap publishes nothing and leaves the previous
/// generation and its objects fully consistent. That is the whole of the safety story now that
/// there is one dataset: there are no other products to carry forward, so a bad cycle costs one
/// cycle of freshness and the next tick recovers.
///
/// `lattice` is [`CANONICAL`] in production; it is a parameter so the fixture tests and the event
/// packs can drive this exact orchestration over a sub-window of the same lattice that a debug
/// build can encode. `threads` is [`BAKE_THREADS`] in production and is what the memory budget is
/// stated against.
pub fn run_cycle(
    lattice: &Lattice,
    adapters: &[&dyn Adapter],
    upstream: &mut dyn Upstream,
    store: &mut dyn ObjectStore,
    now: i64,
    threads: usize,
    dry_run: bool,
) -> Result<CycleReport, String> {
    let started = Instant::now();
    let mut warnings = Vec::new();
    let times = CycleTimes::anchored_at(now);

    let mut sources = Vec::with_capacity(adapters.len());
    for adapter in adapters {
        sources.push(adapter.bake(upstream, now, &mut warnings)?);
    }
    // WXR9 #1251, between the adapters and the mosaic and nowhere else: the radar sources gain an
    // advected forward layer, and the hourly model sources gain a frame at every canonical instant
    // their own steps skip. Both produce ordinary `BakedSource`s on their parent's window, so
    // everything below this line is unchanged — which is the test of whether WXR3's mosaic was
    // built widely enough. Every failure inside is a warning and leaves the mosaic as it was.
    // On the same `threads` the bake is measured at, and for the same reason: the derivation's own
    // parallelism is inside `flow`, so on a 16-core builder it would report a wall time the 4-vCPU
    // production box will never see. A published budget must not be a property of the machine.
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()
        .map_err(|error| format!("derive pool: {error}"))?;
    let (sources, derived) = pool.install(|| crate::derive::derive_sources(sources, times));
    let mosaic = Mosaic::from_sources(sources)?;
    let mut layers: Vec<(String, usize, usize)> =
        mosaic.layers().iter().map(|layer| (layer.id.to_string(), layer.rank, layer.frames.len())).collect();
    layers.sort_by_key(|(_, rank, _)| *rank);
    // Every source in the mosaic may have painted any cell, so every licence line travels with the
    // dataset, in priority order — there is no per-cell provenance to attribute more precisely.
    let attribution = mosaic.attribution();
    // The retention chain is read back out of the published manifest rather than kept anywhere:
    // the baker is stateless, and its only state is the document it publishes. A manifest that is
    // there but unreadable fails the cycle rather than publishing an empty chain — see
    // `manifest_v2::carried_generations`, where the reasoning lives.
    let carried = manifest_v2::carried_generations(store.get(manifest_v2::MANIFEST_KEY)?.as_deref(), &mut warnings)?;
    refuse_to_go_backwards(&carried, times)?;
    // The **uncapped** candidate list, not a capped chain: `Builder::new` filters the generation
    // being published out before it takes two, and capping first costs a re-bake a retained
    // generation (round 1 of #1274's review). `manifest_v2::Carried` deliberately has no capped
    // accessor to pass here by mistake.
    let mut document = manifest_v2::Builder::new(lattice, times, now, attribution, carried.named().to_vec());

    let mut published_objects = 0usize;
    let mut published_bytes = 0u64;
    let mut swept = crate::sweep::SweepReport::default();
    // The whole generation, staged in memory, and then published as **one batch** (#1279). It is
    // ~16 MB against a cycle that peaks near 400 MB on the mosaic, so the cost is noise; what it
    // buys is a phase the store can run several requests at a time over, which is the difference
    // between a 220 s cycle and a 50 s one on a box measured at 18 % utilization while publishing.
    // Nothing about the ordering changes: this vector is filled, then written, then every key in it
    // is proved fetchable, and only then does the manifest move.
    let mut planned: Vec<PlannedObject> = Vec::new();
    let mut painted_objects = 0usize;
    let mut blank_forward_objects = 0usize;
    let mut dry_shards = 0usize;
    let mut total_objects = 0usize;
    bake_cycle(lattice, &mosaic, times, threads, &mut |object| {
        total_objects += 1;
        painted_objects += usize::from(object.fill.painted);
        // A forward frame with no eligible forecast source anywhere in its window. Since #1248 this
        // is a state the dataset can actually reach — the radar that used to mask an absent model
        // is no longer eligible here — and it publishes a shard of intensity 15 rather than a
        // frozen field, which is the honest answer but not a quiet one. It should be impossible
        // while the hourly GFS floor is healthy (`the_hourly_floor_reaches_every_offset_with_
        // exactly_zero_margin`), so seeing it means the floor is degraded, and a run of cycles
        // reporting it is what a freshness probe should be escalating on.
        if object.offset_min > 0 && !object.fill.painted {
            blank_forward_objects += 1;
        }
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
            object.observed,
        );
        if dry_run {
            return Ok(());
        }
        planned.push(PlannedObject {
            key: object.key.clone(),
            bytes: object.bytes,
            cache_control: publish::FRAME_CACHE_CONTROL,
            content_type: "application/octet-stream",
        });
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

    // Not fatal — a partial dataset is still worth publishing, and intensity 15 is a truthful thing
    // to publish — but it is the one degradation #1248 introduced that has no other symptom, so it
    // is said out loud rather than left to be inferred from a rendered frame.
    if blank_forward_objects > 0 {
        warnings.push(format!(
            "{blank_forward_objects} forward-frame shards had no eligible forecast source and published intensity 15; \
             an observation cannot stand in for one, so this is the global floor degraded"
        ));
    }

    if !dry_run {
        // Everything from here to the read-back is bounded in wall-clock. The unit's
        // `TimeoutStartSec` is a SIGKILL, and being SIGKILLed part-way through a publish is the one
        // moment nobody can say whether the manifest swapped — so the publish gets a budget of its
        // own and fails the ordinary way, before the swap, when it cannot meet it.
        store.begin_phase(PUBLISH_BUDGET);

        // **Phase one**: every frame object, written and acknowledged. `put_all` does not return
        // until each one is durably at the destination — or until one failed, which fails the cycle
        // here, with the manifest untouched and the previous generation whole.
        store.put_all(&planned)?;
        published_objects += planned.len();
        published_bytes += planned.iter().map(|object| object.bytes.len() as u64).sum::<u64>();

        // **Phase two**: frames first, manifest last. Every object the manifest is about to name
        // must already be fetchable at the destination, at its length. Sequential and unbatched on
        // purpose — this is the proof, and it reads back what phase one claims it wrote.
        //
        // The two arms below really are two different failures, and #1280 is why that is worth
        // saying: rclone v1.60.1 reported an absent object as 0 bytes, so an object that was never
        // uploaded arrived here as a *length mismatch* and the `None` arm was unreachable against
        // the live store. #1280 fixed that by reading `count` out of `rclone size --json`; #1279
        // removed the subprocess the question was being asked through, so `None` is now a 404 and
        // nothing else can produce it.
        for object in &planned {
            let key = &object.key;
            let expected = object.bytes.len() as u64;
            match store.head(key)? {
                Some(remote) if remote == expected => {}
                Some(remote) => {
                    return Err(format!(
                        "{key}: published as {remote} bytes but the manifest expects {expected} — refusing to swap the manifest in"
                    ))
                }
                None => return Err(format!("{key}: not fetchable — refusing to swap the manifest in")),
            }
        }
        // **Phase three**: the one mutable object, alone, on its own `put`. It is never part of a
        // batch and never concurrent with one — the swap is the moment the new generation becomes
        // the answer, and it happens after phase two returned for every key.
        let manifest = document.finish();
        let manifest_object = PlannedObject {
            key: manifest_v2::MANIFEST_KEY.to_string(),
            bytes: manifest_v2::to_json(&manifest).into_bytes(),
            cache_control: publish::MANIFEST_CACHE_CONTROL,
            content_type: "application/json",
        };
        published_bytes += manifest_object.bytes.len() as u64;
        published_objects += 1;
        // No key prefix here: every store's errors already name the key they are about, and two
        // stores' worth of prefixes made `wx/v2/manifest.json: wx/v2/manifest.json: status 403`.
        store.put(&manifest_object)?;

        // **Read the licence back before spending it.** Every one of the 216 frame objects above
        // was `head`ed at its exact length before this swap; the manifest is the object that both
        // wedges the next cycle if it is unreadable *and* is the entire authority for ~216
        // deletions, and until round 1 of #1274's review it was the only one nobody checked. This
        // bucket has a recorded history of tearing bodies mid-stream. `get` + `from_json` + a byte
        // compare is one request against that.
        verify_published_manifest(store, &manifest_object.bytes)?;
        // The publish is done and proved. The sweep gets its own, separate budget below: its
        // failures are warnings, so it must not be able to fail a cycle that has already succeeded.
        store.end_phase();

        // **After the swap, and only after it** (WXR8 #1247). The manifest above is durably in
        // place *and verified readable*, so the generations it no longer names are unreferenced and
        // may go. `carried` is the §10.4 licence — a chain read out of a predecessor this cycle
        // successfully parsed — and it is the only thing `sweep` will act on: a torn read never gets
        // here, because it failed the cycle before a single object was published. Sweep failures are
        // warnings; the cycle succeeded the moment the manifest landed.
        store.begin_phase(SWEEP_BUDGET);
        swept = crate::sweep::sweep(store, lattice, times, &carried, &manifest);
        store.end_phase();
        // Extended, not drained: `CycleReport::swept.warnings` is a documented public field, and a
        // field that is always empty because its only writer moved out of it is a lie (#1274 r1).
        warnings.extend(swept.warnings.iter().cloned());
    }

    Ok(CycleReport {
        layers,
        derived,
        reference_time: times.reference_time,
        fetched_bytes: upstream.fetched_bytes(),
        published_objects,
        dry_shards,
        published_bytes,
        swept,
        elapsed_ms: started.elapsed().as_millis(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_published_lattice_is_wxr1s_measured_recommendation() {
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

    /// Every source in the mosaic reaches `attribution[]`, in priority order — including WXR6's two
    /// OPERA rows, whose CC BY 4.0 terms are a licence obligation rather than a nicety. Built from
    /// the same `Mosaic::attribution` the cycle publishes, over the real per-adapter constants, so
    /// a source added to `MOSAIC_PRIORITY` without a licence line cannot pass.
    #[test]
    fn every_mosaic_source_reaches_the_manifests_attribution_in_priority_order() {
        use crate::source::{dwd_rv, gfs, hrrr, icon_eu, mrms, opera_cirrus, opera_nimbus, MOSAIC_PRIORITY};
        let known: Vec<(&'static str, Attribution)> = vec![
            (dwd_rv::ID, dwd_rv::ATTRIBUTION),
            (mrms::ID, mrms::ATTRIBUTION),
            (opera_cirrus::ID, opera_cirrus::ATTRIBUTION),
            (opera_nimbus::ID, opera_nimbus::ATTRIBUTION),
            // WXR9 #1251's derived layers carry their parent's licence plus the modification they
            // made to it. They are sources in this list on exactly the same terms as the fetched
            // ones, because a rider cannot tell which of them painted a cell.
            (mrms::NOWCAST.id, mrms::NOWCAST.attribution),
            (opera_cirrus::NOWCAST.id, opera_cirrus::NOWCAST.attribution),
            (hrrr::ID, hrrr::ATTRIBUTION),
            (icon_eu::ID, icon_eu::ATTRIBUTION),
            (gfs::ID, gfs::ATTRIBUTION),
        ];
        assert_eq!(known.len(), MOSAIC_PRIORITY.len(), "a source joined the table with no licence line here");

        // Deliberately shuffled into the table's *reverse* order: the mosaic sorts by rank, so a
        // list that came out right by accident of input order would not prove anything.
        let layers = known
            .iter()
            .rev()
            .map(|(id, attribution)| MosaicLayer {
                id,
                rank: mosaic_rank(id).expect("every known source has a row"),
                attribution: *attribution,
                frames: Vec::new(),
            })
            .collect();
        let attribution = Mosaic::new(layers).attribution();
        assert_eq!(
            attribution.iter().map(|entry| entry.source_id.as_str()).collect::<Vec<_>>(),
            MOSAIC_PRIORITY.iter().map(|source| source.id).collect::<Vec<_>>()
        );
        let opera: Vec<&str> = attribution
            .iter()
            .filter(|entry| entry.source_id.starts_with("opera-"))
            .map(|entry| entry.text.as_str())
            .collect();
        // Three OPERA rows since WXR9: the two fetched composites and the nowcast derived from
        // CIRRUS. All three carry CC BY 4.0, which is the point — a derived layer inherits the
        // obligation it was derived under, and forgetting that on one of them would put an
        // unattributed EUMETNET pixel on the wire.
        assert_eq!(opera.len(), 3);
        assert!(opera.iter().all(|text| text.contains("CC BY 4.0")), "OPERA's licence must survive to the manifest");
    }

    /// **The floor's coverage of all nine offsets rests on `<=` at exactly 1,800 s, twice per
    /// hour — so the margin is pinned rather than assumed.**
    ///
    /// Since #1248 a forward frame with no eligible forecast source publishes intensity 15 rather
    /// than falling back on a frozen observation, which makes this arithmetic load-bearing in a way
    /// it was not before: the floor is the only thing standing between a coverage hole and a
    /// two-hour column of "we do not know". Three facts, and the third is the one a retune would
    /// break silently:
    ///
    /// 1. every offset of every anchor phase is inside the window (coverage);
    /// 2. the worst case is **exactly** [`MAX_FRAME_SKEW_S`] — zero margin, reached at the :30
    ///    phase, where the frame instant is equidistant from both flanking hourly steps;
    /// 3. so one second off the window, or a cadence that leaves the quarter hour, drops the floor
    ///    out of two frames in four.
    #[test]
    fn the_hourly_floor_reaches_every_offset_with_exactly_zero_margin() {
        // "Hourly" is read off the floor adapter's own retained lead set rather than restated as a
        // local constant, so a GFS re-tune to three-hourly leads fails here instead of quietly
        // invalidating the arithmetic below.
        let leads = crate::source::gfs::LEADS_H;
        let step_h = leads[1] - leads[0];
        assert!(
            leads.windows(2).all(|pair| pair[1] - pair[0] == step_h),
            "the floor's leads are not evenly spaced; the nearest-step arithmetic below assumes they are"
        );
        assert_eq!(step_h, 1, "the floor is hourly — a coarser one cannot cover a 15-minute cadence at this window");
        let hour = i64::from(step_h) * 3_600;

        // A cycle anchors on a quarter hour, so its phase within one floor step is one of four.
        let phases: Vec<i64> =
            (0..hour / (i64::from(FRAME_STEP_MIN) * 60)).map(|step| step * i64::from(FRAME_STEP_MIN) * 60).collect();
        assert_eq!(phases, vec![0, 900, 1_800, 2_700], "the anchoring rule admits exactly these four phases");

        let mut worst = 0i64;
        let mut worst_case = (0i64, 0u32);
        for phase in phases {
            let times = CycleTimes::anchored_at(phase);
            for offset_min in times.offsets_min() {
                // Distance from this frame's instant to the nearer of the two hourly steps
                // bracketing it. An hourly source publishes on the hour, whatever its run.
                let into_hour = times.valid_at(offset_min).rem_euclid(hour);
                let distance = into_hour.min(hour - into_hour);
                assert!(
                    distance <= MAX_FRAME_SKEW_S,
                    "anchor phase {phase}s, f+{offset_min}: an hourly step is {distance}s away, outside the window \
                     — the floor does not reach this frame and it would publish intensity 15"
                );
                if distance > worst {
                    worst = distance;
                    worst_case = (phase, offset_min);
                }
            }
        }
        assert_eq!(
            worst, MAX_FRAME_SKEW_S,
            "the worst case must be exactly the window (anchor phase {}s, f+{}) — if this is now less, someone \
             widened the window or moved the cadence and the zero-margin note on MAX_FRAME_SKEW_S is stale; if it \
             is more, the floor no longer reaches every frame",
            worst_case.0, worst_case.1
        );
    }

    #[test]
    fn a_cycle_anchors_on_the_quarter_hour_and_spans_two_hours() {
        let times = CycleTimes::anchored_at(timefmt::parse_rfc3339("2026-08-09T14:37:11Z").expect("timestamp"));
        assert_eq!(timefmt::rfc3339(times.reference_time), "2026-08-09T14:30:00Z");
        let offsets: Vec<u32> = times.offsets_min().collect();
        assert_eq!(offsets, vec![0, 15, 30, 45, 60, 75, 90, 105, 120]);
        assert_eq!(timefmt::rfc3339(times.valid_at(120)), "2026-08-09T16:30:00Z");
    }
}
