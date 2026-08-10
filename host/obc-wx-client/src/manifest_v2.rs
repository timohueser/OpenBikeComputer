//! The client's view of `wx/v2/manifest.json` (OBCG §10) — **selection as arithmetic** (#1243).
//!
//! v1's reader exists to *choose*: it ranks products by tier, tests bbox containment and compares
//! staleness. None of that survives here, because there is nothing to choose between. This module
//! answers exactly two questions, and neither is a policy:
//!
//! 1. **which shards cover my bbox** — [`Grid::shards_for`], a handful of divisions;
//! 2. **what is at that shard** — [`Frame::state_of`]: an object to fetch, a dry shard, or a shard
//!    off the lattice entirely.
//!
//! [`Manifest::plan`] is those two joined for a whole timeline, and it is the function WXR5 calls.
//! It returns a [`PlanOutcome`] rather than a bare list, because "no objects to fetch" is four
//! different sentences to a rider — no rain, off the map, no source here ever, or no weather at all
//! because the generation expired — and only the first is about rain. An empty `Vec` cannot say
//! which, so it is not what this module hands back.
//!
//! The object key is composed, never read: [`Grid::shard_key`] builds
//! `<key_prefix>/<generation>/f<offset>/s<col>-<row>.obcg`, which is why the manifest does not
//! carry 216 key strings.
//!
//! ## Missing is not dry
//!
//! [`ShardState`] is deliberately three-valued, and the whole point of the presence bitmap:
//!
//! - [`ShardState::Present`] — the object exists. A 404, a short body or a CRC mismatch is an
//!   **error** to retry and then surface, never an absence of rain.
//!   The manifest is the integrity anchor: `bytes` and `object_crc32` are checked against what
//!   comes back, exactly as in v1.
//! - [`ShardState::Dry`] — the baker measured every cell of that shard as dry and published
//!   nothing. There is no request to make and no failure to report.
//! - [`ShardState::OutOfDomain`] — the bbox reaches off the lattice. Not weather, not an error:
//!   geometry.
//!
//! A shard that is entirely **no-data** is `Present` with an object full of intensity 15, because
//! "we do not know" is data the rider is owed. Only genuinely dry shards are absent.
//!
//! Two conditions sit above the per-shard answer and are reported by [`Manifest::plan`] rather than
//! left to a caller to remember: a generation past its `stale_after` is
//! [`PlanOutcome::Expired`] — no weather, which is not no rain — and a bbox whose every lattice row
//! is outside `covered_rows` is [`PlanOutcome::Uncovered`], where objects exist but are intensity 15
//! in every frame, forever.
//!
//! Strictness splits the way the phone splits it and for the same reason: the document is strict
//! (bad JSON, an unknown `version` or an unusable grid is a hard failure), an entry is lenient (a
//! malformed frame is skipped and counted, never fatal).

use serde::Deserialize;

pub const MANIFEST_KEY: &str = "wx/v2/manifest.json";
pub const MANIFEST_VERSION: u32 = 2;

/// OBCG 10.4's retention cap, mirrored here because the reader enforces it.
pub const RETAINED_PREVIOUS_GENERATIONS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestError {
    Malformed(String),
    UnsupportedVersion(u32),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Malformed(why) => write!(f, "malformed manifest: {why}"),
            ManifestError::UnsupportedVersion(v) => write!(f, "unsupported manifest version {v}"),
        }
    }
}

// ── validated model ────────────────────────────────────────────────────────────────────────

/// One shard's identity: its column and row on the fixed global shard grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardId {
    pub col: u32,
    pub row: u32,
}

/// Ordered by **`(row, col)`** — the order the manifest states for `shards[]`, the order
/// `shards_for` returns, and the order the presence bit index `row * shard_cols + col` counts in.
/// Written out rather than derived because the derive would order by `(col, row)`, and one
/// ordering silently disagreeing with the document is exactly how a binary search over the shard
/// list starts answering `Dry` for shards that exist.
impl Ord for ShardId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.row, self.col).cmp(&(other.row, other.col))
    }
}

impl PartialOrd for ShardId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// What the manifest says is at one shard of one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShardState {
    /// Fetch it; a 404 here is an error, not dry.
    Present { key: String, bytes: u64, object_crc32: u32, observed: bool },
    /// Every cell is dry. Nothing to fetch, nothing missing.
    Dry,
    /// Not a shard of this lattice.
    OutOfDomain,
}

/// The grid, as the manifest states it. Everything a client used to hardcode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grid {
    pub south_lat_udeg: i32,
    pub west_lon_udeg: i32,
    pub cell_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub shard_width: u32,
    pub shard_height: u32,
    pub shard_cols: u32,
    pub shard_rows: u32,
    pub tile_edge: u16,
    pub entries_per_page: u16,
    pub cell_size_m: u16,
    /// Lattice rows with a source behind them; outside it every frame is intensity 15 forever.
    pub covered_rows: std::ops::Range<u32>,
    /// The prefix of every object key, from the manifest so the tree can move.
    pub key_prefix: String,
    /// The current generation — the key's second segment.
    pub generation: String,
}

impl Grid {
    pub fn shard_count(&self) -> u32 {
        self.shard_cols * self.shard_rows
    }

    /// The bit index of a shard in a frame's presence bitmap.
    pub fn bit_of(&self, shard: ShardId) -> Option<u32> {
        (shard.col < self.shard_cols && shard.row < self.shard_rows).then(|| shard.row * self.shard_cols + shard.col)
    }

    /// The object key of one shard of one frame. Composed, never read from the document.
    pub fn shard_key(&self, offset_min: u32, shard: ShardId) -> String {
        format!("{}/{}/f{offset_min}/s{}-{}.obcg", self.key_prefix, self.generation, shard.col, shard.row)
    }

    /// The geometry of one shard's OBCG object. `None` for a shard off the grid.
    ///
    /// Interior shards are exactly `shard_width x shard_height`; the last column and row are
    /// **short**, because the lattice is not required to be a whole number of shards. Rounding that
    /// up instead would make the client expect an object bigger than the one the baker published,
    /// and the header check would then refuse every edge shard on the planet.
    pub fn shard_geometry(&self, shard: ShardId) -> Option<ShardGeometry> {
        self.bit_of(shard)?;
        let first_col = shard.col * self.shard_width;
        let first_row = shard.row * self.shard_height;
        Some(ShardGeometry {
            south_udeg: self.south_lat_udeg + (i64::from(first_row) * i64::from(self.cell_udeg)) as i32,
            west_udeg: self.west_lon_udeg + (i64::from(first_col) * i64::from(self.cell_udeg)) as i32,
            cell_udeg: self.cell_udeg,
            width: self.shard_width.min(self.width - first_col),
            height: self.shard_height.min(self.height - first_row),
            cell_size_m: self.cell_size_m,
            tile_edge: self.tile_edge,
            entries_per_page: self.entries_per_page,
        })
    }

    /// Do any of the lattice rows this bbox touches have a **source** behind them?
    ///
    /// `covered_rows` is not decoration: rows outside it are published as intensity 15 in every
    /// frame, forever, because no source we ingest reaches them (#1242's polar band). A corridor
    /// wholly inside that band has objects it *could* fetch, and they would all decode to "we do
    /// not know" — so [`Manifest::plan`] answers [`PlanOutcome::Uncovered`] instead of issuing nine
    /// Range reads to learn a permanent fact the manifest already stated.
    ///
    /// **Private on purpose.** It answers with a bare `bool` and does not validate its bbox, so a
    /// caller handing it a 0..360 longitude or a latitude past a pole would get a confident wrong
    /// answer with nowhere to report the problem. Its one call site is [`Manifest::plan`], which
    /// reaches it only after [`Grid::shards_for`] has already accepted the window; anything a
    /// consumer needs from it arrives as [`PlanOutcome::Uncovered`], which cannot be silently
    /// misread as "no rain".
    fn any_row_has_a_source(&self, bbox: &Bbox) -> bool {
        let Some((row0, row1)) = self.cell_span(bbox.south_udeg, bbox.north_udeg, self.south_lat_udeg, self.height)
        else {
            return false;
        };
        row0 < self.covered_rows.end && row1 >= self.covered_rows.start
    }

    /// The half-open cell interval `[lo, hi)` of one axis, intersected with the lattice — or `None`
    /// if the two do not overlap.
    ///
    /// **The intersection is tested on the unclamped interval.** Clamping first is the bug this
    /// spells out: an interval lying wholly east of the lattice collapses onto its last column
    /// instead of vanishing, and a rider off the map is served another continent's shard rather
    /// than told there is nothing there. An edge landing exactly on a cell boundary closes the cell
    /// before it, which `ceil - 1` gives and a plain floor does not.
    fn cell_span(&self, lo: i64, hi: i64, origin: i32, extent: u32) -> Option<(u32, u32)> {
        let cell = i64::from(self.cell_udeg);
        let origin = i64::from(origin);
        let first = (lo - origin).div_euclid(cell).max(0);
        let last = ((hi - origin + cell - 1).div_euclid(cell) - 1).min(i64::from(extent) - 1);
        (first <= last).then_some((first as u32, last as u32))
    }

    /// Every shard covering `bbox`, ascending by `(row, col)` — **the whole of what used to be
    /// product selection**, and it is a handful of divisions.
    ///
    /// Coordinates are microdegrees in the **-180..180 / -90..90** convention, and that is checked
    /// rather than assumed: a longitude in the 0..360 form (352,150,000 meaning -7.85 degrees) is
    /// [`BboxError::OutOfRange`], never silently reinterpreted, because the alternative is a
    /// corridor answered from the wrong hemisphere with no error anywhere. `west > east` is not
    /// malformed — it **means the window crosses the antimeridian**, and it is served by splitting
    /// into `[west, 180)` and `[-180, east)`. See `OBCG_Spec.md` §10.2, which is normative for all
    /// of this, and note that an empty result is *not* "everywhere dry": [`Manifest::plan`] reports
    /// it as [`PlanOutcome::OutOfDomain`], which is the whole reason this returns a `Result` and a
    /// possibly-empty `Vec` rather than folding both into one.
    pub fn shards_for(&self, bbox: &Bbox) -> Result<Vec<ShardId>, BboxError> {
        bbox.validate()?;
        let Some((row0, row1)) = self.cell_span(bbox.south_udeg, bbox.north_udeg, self.south_lat_udeg, self.height)
        else {
            return Ok(Vec::new());
        };
        // One interval normally; two when the window crosses the antimeridian.
        let spans: [(i64, i64); 2] = if bbox.west_udeg < bbox.east_udeg {
            [(bbox.west_udeg, bbox.east_udeg), (0, 0)]
        } else {
            [(bbox.west_udeg, 180_000_000), (-180_000_000, bbox.east_udeg)]
        };
        let mut cols = std::collections::BTreeSet::new();
        for (lo, hi) in spans {
            if lo >= hi {
                continue;
            }
            if let Some((col0, col1)) = self.cell_span(lo, hi, self.west_lon_udeg, self.width) {
                cols.extend(col0 / self.shard_width..=col1 / self.shard_width);
            }
        }
        let mut shards = Vec::new();
        for row in row0 / self.shard_height..=row1 / self.shard_height {
            shards.extend(cols.iter().map(|&col| ShardId { col, row }));
        }
        Ok(shards)
    }
}

/// One shard's OBCG geometry, **derived** from the stated lattice rather than carried per object.
///
/// This is what replaces v1's per-frame `geometry` block: 216 objects a cycle cannot each restate a
/// grid that is a division away, so the manifest states the lattice once and the client computes.
/// The check it feeds is unchanged — the fetched header must agree with this before a cell is
/// trusted (see [`ShardGeometry::agrees_with`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardGeometry {
    pub south_udeg: i32,
    pub west_udeg: i32,
    pub cell_udeg: u32,
    pub width: u32,
    pub height: u32,
    pub cell_size_m: u16,
    pub tile_edge: u16,
    pub entries_per_page: u16,
}

impl ShardGeometry {
    pub fn bounds(&self) -> Bbox {
        Bbox {
            south_udeg: i64::from(self.south_udeg),
            west_udeg: i64::from(self.west_udeg),
            north_udeg: i64::from(self.south_udeg) + i64::from(self.height) * i64::from(self.cell_udeg),
            east_udeg: i64::from(self.west_udeg) + i64::from(self.width) * i64::from(self.cell_udeg),
        }
    }

    /// Does the fetched OBCG header say what the lattice promised? A manifest that mis-states the
    /// grid, or an object published against a different one, is caught here rather than decoded.
    pub fn agrees_with(&self, header: &obc_formats::obcg::Header) -> bool {
        self.south_udeg == header.south_lat_udeg
            && self.west_udeg == header.west_lon_udeg
            && self.cell_udeg == header.cell_lat_udeg
            && self.cell_udeg == header.cell_lon_udeg
            && self.width == header.width
            && self.height == header.height
            && self.cell_size_m == header.cell_size_m
            && self.tile_edge == header.tile_edge
            && self.entries_per_page == header.entries_per_page
    }
}

/// Why a bbox is not a window this client will answer.
///
/// Both are caller bugs rather than weather, and both are reported rather than repaired: a client
/// that clamps a malformed corridor answers the wrong question confidently, which is worse than
/// answering none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BboxError {
    /// A coordinate outside ±90° latitude or ±180° longitude. Longitudes are **-180..180**; the
    /// 0..360 spelling is this error.
    OutOfRange,
    /// A window with no area: `south >= north`, or `west == east`.
    Empty,
}

impl std::fmt::Display for BboxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BboxError::OutOfRange => write!(f, "bbox coordinates are outside +/-90 lat, +/-180 lon"),
            BboxError::Empty => write!(f, "bbox has no area"),
        }
    }
}

/// A geographic window in microdegrees, in the **-180..180 / -90..90** convention.
///
/// `west > east` means the window crosses the antimeridian; every other spelling of that idea —
/// 0..360 longitudes, a west below -180, an east above 180 — is [`BboxError::OutOfRange`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bbox {
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub north_udeg: i64,
    pub east_udeg: i64,
}

impl Bbox {
    pub fn validate(&self) -> Result<(), BboxError> {
        if !(-90_000_000..=90_000_000).contains(&self.south_udeg)
            || !(-90_000_000..=90_000_000).contains(&self.north_udeg)
            || !(-180_000_000..=180_000_000).contains(&self.west_udeg)
            || !(-180_000_000..=180_000_000).contains(&self.east_udeg)
        {
            return Err(BboxError::OutOfRange);
        }
        if self.south_udeg >= self.north_udeg || self.west_udeg == self.east_udeg {
            return Err(BboxError::Empty);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cadence {
    pub frame_step_min: u32,
    pub frames: u32,
    /// How far from its stated validity a cell's underlying source frame may have been.
    pub max_source_skew_s: i64,
}

/// Deadlines, absolute, from the document. A client compares timestamps; it holds no constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Freshness {
    pub manifest_max_age_s: i64,
    pub next_generation_expected_at: i64,
    pub stale_after: i64,
}

impl Freshness {
    /// Inclusive: the generation is usable up to and including its deadline second. Past it there
    /// is **no weather** — which is not the same as no rain, and must never render as dry.
    pub fn is_usable(&self, now: i64) -> bool {
        now <= self.stale_after
    }

    /// Should this document be re-fetched before being used again?
    pub fn manifest_is_stale(&self, fetched_at: i64, now: i64) -> bool {
        now - fetched_at > self.manifest_max_age_s
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribution {
    pub source_id: String,
    pub text: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shard {
    pub id: ShardId,
    pub bytes: u64,
    pub object_crc32: u32,
    pub observed: bool,
}

/// One frame of the timeline.
///
/// `present` and `shards` are **private**, and that is load-bearing rather than tidy. They are two
/// spellings of one fact, proved equal exactly once by [`validate_frame`]; leaving them public
/// would let a caller desync them after parsing and get a silent `Dry` for a shard the manifest
/// says exists — the forbidden answer, reached through the one defaulting branch in the module.
/// With them private there is no defaulting branch at all: [`Frame::state_of`] looks the shard up
/// in the sorted list, and *that lookup is* the bitmap read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub offset_min: u32,
    pub valid_at: i64,
    /// Presence, one bit per shard, `row * shard_cols + col`.
    present: Vec<u8>,
    /// Exactly the shards `present` names, ascending by `(row, col)`.
    shards: Vec<Shard>,
}

impl Frame {
    /// Every published shard of this frame, ascending by `(row, col)`.
    pub fn shards(&self) -> &[Shard] {
        &self.shards
    }

    /// The presence bit as the document spells it. Equal to `state_of(..) == Present` by
    /// construction; kept because a probe or a diagnostics panel wants the bitmap's own answer, and
    /// `the_bitmap_and_the_lookup_are_the_same_answer` pins that they cannot diverge.
    pub fn is_present(&self, grid: &Grid, shard: ShardId) -> bool {
        let Some(bit) = grid.bit_of(shard) else { return false };
        self.present.get((bit / 8) as usize).is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
    }

    /// The three-valued answer. This is the function "a 404 must not mean dry" reduces to.
    pub fn state_of(&self, grid: &Grid, shard: ShardId) -> ShardState {
        if grid.bit_of(shard).is_none() {
            return ShardState::OutOfDomain;
        }
        match self.shards.binary_search_by_key(&shard, |entry| entry.id) {
            Ok(index) => {
                let entry = &self.shards[index];
                ShardState::Present {
                    key: grid.shard_key(self.offset_min, shard),
                    bytes: entry.bytes,
                    object_crc32: entry.object_crc32,
                    observed: entry.observed,
                }
            }
            Err(_) => ShardState::Dry,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub generation: String,
    pub generated_at: i64,
    pub reference_time: i64,
    /// Superseded generations still fetchable, newest first.
    pub previous_generations: Vec<String>,
    pub grid: Grid,
    pub cadence: Cadence,
    pub freshness: Freshness,
    pub attribution: Vec<Attribution>,
    pub frames: Vec<Frame>,
    /// Frames the parser refused. Evidence for the diagnostics panel, never control flow.
    pub skipped_frames: usize,
}

/// Why a plan has no objects in it — or that it does.
///
/// **Every one of these is a different thing to show a rider, and only one of them is rain.** An
/// empty `Vec` cannot say which, so it is not what [`Manifest::plan`] returns: WXR5 must match on
/// this, and the compiler will not let it render "off the map" or "no weather" as "no rain".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanOutcome {
    /// The dataset answers this bbox. `fetch` and `dry` describe it — and `fetch` being empty with
    /// `dry` populated is the *real* "no rain anywhere near you".
    Covered,
    /// The bbox is off the lattice, or is not a window this client will interpret (see
    /// [`BboxError`]). There is no answer here — which is not an answer of "no rain".
    OutOfDomain,
    /// On the lattice, but every row it touches is outside `covered_rows`: no source reaches it in
    /// any frame, ever. The objects exist and are entirely intensity 15, so fetching them would buy
    /// nine round trips and the word "unknown".
    Uncovered,
    /// This generation is past its `stale_after` and no fresher manifest replaced it. **No
    /// weather** — the rider is owed that sentence, and never a dry map.
    Expired,
}

impl Manifest {
    pub fn frame(&self, offset_min: u32) -> Option<&Frame> {
        self.frames.iter().find(|frame| frame.offset_min == offset_min)
    }

    /// What this client should do to cover `bbox` at `now`, across the whole timeline.
    ///
    /// **The WXR5-facing contract.** Read [`PlanOutcome`] first and the vectors second: outside
    /// [`PlanOutcome::Covered`] both vectors are empty *and mean nothing*, and rendering that as a
    /// dry map is the failure this whole issue exists to make impossible. Inside `Covered`, `fetch`
    /// names objects that MUST exist — a 404, a short body or a CRC mismatch is an error to retry
    /// and then surface — and `dry` names shards the baker measured as dry, which need no request
    /// and report no failure.
    ///
    /// Expiry is checked here rather than left to a caller's discipline, because "did anyone
    /// remember to call `is_usable` first" is exactly the kind of contract that holds until the one
    /// call site that forgets.
    pub fn plan(&self, bbox: &Bbox, now: i64) -> Plan {
        let empty = |outcome| Plan { outcome, fetch: Vec::new(), dry: Vec::new() };
        if !self.freshness.is_usable(now) {
            return empty(PlanOutcome::Expired);
        }
        let Ok(shards) = self.grid.shards_for(bbox) else { return empty(PlanOutcome::OutOfDomain) };
        if shards.is_empty() {
            return empty(PlanOutcome::OutOfDomain);
        }
        if !self.grid.any_row_has_a_source(bbox) {
            return empty(PlanOutcome::Uncovered);
        }
        let mut plan = Plan { outcome: PlanOutcome::Covered, fetch: Vec::new(), dry: Vec::new() };
        for frame in &self.frames {
            for shard in &shards {
                match frame.state_of(&self.grid, *shard) {
                    ShardState::Present { key, bytes, object_crc32, observed } => plan.fetch.push(PlannedRead {
                        offset_min: frame.offset_min,
                        shard: *shard,
                        key,
                        bytes,
                        object_crc32,
                        observed,
                    }),
                    ShardState::Dry => plan.dry.push((frame.offset_min, *shard)),
                    // `shards_for` only ever yields shards of this grid.
                    ShardState::OutOfDomain => {}
                }
            }
        }
        plan
    }
}

/// One object to fetch, with everything needed to verify it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRead {
    pub offset_min: u32,
    pub shard: ShardId,
    pub key: String,
    pub bytes: u64,
    pub object_crc32: u32,
    pub observed: bool,
}

/// The three outcomes of a corridor, kept apart by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub outcome: PlanOutcome,
    /// Objects that MUST exist. Empty outside [`PlanOutcome::Covered`].
    pub fetch: Vec<PlannedRead>,
    /// `(offset_min, shard)` the baker measured as dry everywhere. Empty outside `Covered`.
    pub dry: Vec<(u32, ShardId)>,
}

// ── wire model ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WireManifest {
    version: u32,
    generation: String,
    generated_at: String,
    reference_time: String,
    key_prefix: String,
    #[serde(default)]
    previous_generations: Vec<String>,
    lattice: WireLattice,
    cadence: WireCadence,
    freshness: WireFreshness,
    #[serde(default)]
    attribution: Vec<WireAttribution>,
    frames: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
struct WireLattice {
    south_lat_udeg: i32,
    west_lon_udeg: i32,
    cell_udeg: u32,
    width: u32,
    height: u32,
    shard_width: u32,
    shard_height: u32,
    shard_cols: u32,
    shard_rows: u32,
    tile_edge: u16,
    entries_per_page: u16,
    cell_size_m: u16,
    covered_rows: WireRowRange,
}

#[derive(Deserialize)]
struct WireRowRange {
    start: u32,
    end: u32,
}

#[derive(Deserialize)]
struct WireCadence {
    frame_step_min: u32,
    frames: u32,
    max_source_skew_s: i64,
}

#[derive(Deserialize)]
struct WireFreshness {
    manifest_max_age_s: i64,
    next_generation_expected_at: String,
    stale_after: String,
}

#[derive(Deserialize)]
struct WireAttribution {
    source_id: String,
    text: String,
    url: String,
}

#[derive(Deserialize)]
struct WireFrame {
    offset_min: u32,
    valid_at: String,
    present: String,
    shards: Vec<WireShard>,
}

#[derive(Deserialize)]
struct WireShard {
    col: u32,
    row: u32,
    bytes: u64,
    object_crc32: String,
    observed: bool,
}

pub fn parse_rfc3339(text: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(text).ok().map(|time| time.timestamp())
}

pub fn parse(bytes: &[u8]) -> Result<Manifest, ManifestError> {
    let wire: WireManifest =
        serde_json::from_slice(bytes).map_err(|error| ManifestError::Malformed(error.to_string()))?;
    if wire.version != MANIFEST_VERSION {
        return Err(ManifestError::UnsupportedVersion(wire.version));
    }
    let field = |name: &str| ManifestError::Malformed(name.to_string());
    let generated_at = parse_rfc3339(&wire.generated_at).ok_or_else(|| field("generated_at"))?;
    let reference_time = parse_rfc3339(&wire.reference_time).ok_or_else(|| field("reference_time"))?;
    // The generation is a key segment, and the key prefix is joined onto the service origin: a
    // manifest must not be able to steer the client off its own tree.
    if !is_safe_segment(&wire.generation) || !is_safe_prefix(&wire.key_prefix) {
        return Err(field("generation/key_prefix"));
    }
    // 10.4 caps the chain at two, normatively: a longer list means this client and the service's
    // sweep disagree about which generations exist, and guessing which of them is right is how a
    // client ends up reading an object a sweep already collected. Raising the cap is a version bump.
    if wire.previous_generations.len() > RETAINED_PREVIOUS_GENERATIONS
        || wire.previous_generations.iter().any(|generation| !is_safe_segment(generation))
    {
        return Err(field("previous_generations"));
    }
    let grid = validate_grid(wire.lattice, wire.key_prefix, wire.generation.clone())?;
    let freshness = Freshness {
        manifest_max_age_s: wire.freshness.manifest_max_age_s.max(0),
        next_generation_expected_at: parse_rfc3339(&wire.freshness.next_generation_expected_at)
            .ok_or_else(|| field("freshness.next_generation_expected_at"))?,
        stale_after: parse_rfc3339(&wire.freshness.stale_after).ok_or_else(|| field("freshness.stale_after"))?,
    };
    let wire_frame_count = wire.frames.len();
    let mut frames = Vec::new();
    let mut skipped_frames = 0usize;
    for value in wire.frames {
        match serde_json::from_value::<WireFrame>(value).ok().and_then(|frame| validate_frame(frame, &grid)) {
            Some(frame) => frames.push(frame),
            None => skipped_frames += 1,
        }
    }
    // §10: frames are a timeline. Out-of-order or duplicated validities make the OBCW re-encode
    // (which requires strictly increasing `valid_at`) unbuildable later, so refuse now.
    if frames.windows(2).any(|pair| pair[1].valid_at <= pair[0].valid_at) {
        return Err(field("frames are not a strictly increasing timeline"));
    }
    // `offset_min` alone names the object, so two frames sharing one would name the same object at
    // two validities — and `frame()` would silently answer with whichever came first.
    if frames.windows(2).any(|pair| pair[1].offset_min == pair[0].offset_min) {
        return Err(field("two frames share an offset_min"));
    }
    // Both cheap, both derivable, both catching a mis-derived cycle rather than a hostile one: the
    // frame count the cadence promises, and a deadline ordering that says the generation expires
    // before its own replacement is due.
    if wire_frame_count != wire.cadence.frames as usize {
        return Err(field("cadence.frames disagrees with the frame list"));
    }
    if freshness.stale_after < freshness.next_generation_expected_at {
        return Err(field("stale_after is before the next generation is due"));
    }
    Ok(Manifest {
        generation: wire.generation,
        generated_at,
        reference_time,
        previous_generations: wire.previous_generations,
        grid,
        cadence: Cadence {
            frame_step_min: wire.cadence.frame_step_min,
            frames: wire.cadence.frames,
            max_source_skew_s: wire.cadence.max_source_skew_s,
        },
        freshness,
        attribution: wire
            .attribution
            .into_iter()
            .map(|entry| Attribution { source_id: entry.source_id, text: entry.text, url: entry.url })
            .collect(),
        frames,
        skipped_frames,
    })
}

/// The grid is **document-level**: a client that cannot address the dataset has nothing to degrade
/// to, so an unusable one is a hard failure rather than a skipped entry.
fn validate_grid(wire: WireLattice, key_prefix: String, generation: String) -> Result<Grid, ManifestError> {
    let bad = |why: &str| ManifestError::Malformed(format!("lattice: {why}"));
    if wire.cell_udeg == 0 || wire.width == 0 || wire.height == 0 || wire.shard_width == 0 || wire.shard_height == 0 {
        return Err(bad("degenerate"));
    }
    // The shard grid must be exactly the one that tiles the lattice, or the client's arithmetic and
    // the baker's disagree about which object holds a cell.
    if wire.shard_cols != wire.width.div_ceil(wire.shard_width)
        || wire.shard_rows != wire.height.div_ceil(wire.shard_height)
    {
        return Err(bad("the shard grid does not tile the lattice"));
    }
    // OBCG §1/§3, checked before a byte is fetched: a shard the header could only reject is not
    // worth a Range read.
    if u64::from(wire.shard_width) * u64::from(wire.shard_height) > obc_formats::obcg::MAX_GRID_CELLS
        || wire.shard_width > obc_formats::obcg::MAX_GRID_DIM
        || wire.shard_height > obc_formats::obcg::MAX_GRID_DIM
        || wire.tile_edge < obc_formats::obcg::MIN_TILE_EDGE
        || wire.tile_edge > obc_formats::obcg::MAX_TILE_EDGE
        || !wire.tile_edge.is_power_of_two()
        || wire.entries_per_page == 0
        || wire.entries_per_page > obc_formats::obcg::MAX_ENTRIES_PER_PAGE
        || wire.cell_size_m == 0
    {
        return Err(bad("a shard would not be an addressable OBCG object"));
    }
    if wire.covered_rows.start > wire.covered_rows.end || wire.covered_rows.end > wire.height {
        return Err(bad("covered_rows is not a range of the lattice"));
    }
    Ok(Grid {
        south_lat_udeg: wire.south_lat_udeg,
        west_lon_udeg: wire.west_lon_udeg,
        cell_udeg: wire.cell_udeg,
        width: wire.width,
        height: wire.height,
        shard_width: wire.shard_width,
        shard_height: wire.shard_height,
        shard_cols: wire.shard_cols,
        shard_rows: wire.shard_rows,
        tile_edge: wire.tile_edge,
        entries_per_page: wire.entries_per_page,
        cell_size_m: wire.cell_size_m,
        covered_rows: wire.covered_rows.start..wire.covered_rows.end,
        key_prefix,
        generation,
    })
}

fn validate_frame(wire: WireFrame, grid: &Grid) -> Option<Frame> {
    let valid_at = parse_rfc3339(&wire.valid_at)?;
    let present = unhex(&wire.present)?;
    let count = grid.shard_count();
    if present.len() != (count.div_ceil(8)) as usize {
        return None;
    }
    // Bits past the last shard must be zero, or "how many shards are there" has two answers.
    for bit in count..present.len() as u32 * 8 {
        if present[(bit / 8) as usize] & (1 << (bit % 8)) != 0 {
            return None;
        }
    }
    let mut shards = Vec::with_capacity(wire.shards.len());
    for shard in wire.shards {
        let id = ShardId { col: shard.col, row: shard.row };
        let bit = grid.bit_of(id)?;
        // The bitmap and the list are one statement, so a frame where they disagree is refused
        // rather than reconciled: silently trusting either one is how a dry shard becomes a
        // missing object, or the reverse.
        if present[(bit / 8) as usize] & (1 << (bit % 8)) == 0 {
            return None;
        }
        let object_crc32 = u32::from_str_radix(shard.object_crc32.strip_prefix("0x")?, 16).ok()?;
        if shard.bytes == 0 || shard.bytes > i32::MAX as u64 {
            return None;
        }
        shards.push(Shard { id, bytes: shard.bytes, object_crc32, observed: shard.observed });
    }
    shards.sort_by_key(|shard| shard.id);
    shards.dedup_by_key(|shard| shard.id);
    if shards.len() != present.iter().map(|byte| byte.count_ones() as usize).sum::<usize>() {
        return None;
    }
    Some(Frame { offset_min: wire.offset_min, valid_at, present, shards })
}

/// A key segment the client is willing to put in a URL: no separators, no traversal, no emptiness.
fn is_safe_segment(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 64
        && text.chars().all(|character| character.is_ascii_alphanumeric() || character == '-' || character == '_')
}

fn is_safe_prefix(text: &str) -> bool {
    !text.is_empty()
        && !text.starts_with('/')
        && !text.ends_with('/')
        && !text.contains("..")
        && text.split('/').all(is_safe_segment)
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if text.is_empty() || !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len() / 2).map(|index| u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()).collect()
}
