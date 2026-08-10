//! The client's view of `wx/v2/manifest.json` (OBCG §10) — **selection as arithmetic** (#1243).
//!
//! v1's reader exists to *choose*: it ranks products by tier, tests bbox containment and compares
//! staleness. None of that survives here, because there is nothing to choose between. This module
//! answers exactly two questions, and neither is a policy:
//!
//! 1. **which shards cover my bbox** — [`Grid::shards_for`], four divisions;
//! 2. **what is at that shard** — [`Frame::state_of`]: an object to fetch, a dry shard, or a shard
//!    off the lattice entirely.
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
//! Strictness splits the way the phone splits it and for the same reason: the document is strict
//! (bad JSON, an unknown `version` or an unusable grid is a hard failure), an entry is lenient (a
//! malformed frame is skipped and counted, never fatal).

use serde::Deserialize;

pub const MANIFEST_KEY: &str = "wx/v2/manifest.json";
pub const MANIFEST_VERSION: u32 = 2;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShardId {
    pub col: u32,
    pub row: u32,
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

    /// Every shard covering `bbox`, ascending by `(row, col)` — **the whole of what used to be
    /// product selection**, and it is four divisions.
    ///
    /// The lattice window is half-open in both axes, so a bbox edge landing exactly on a shard
    /// boundary belongs to the shard it opens, not the one it closes; a bbox reaching past the
    /// lattice is clipped to it rather than refused, because a corridor at the edge of the domain
    /// is a real thing and the shards it does reach are still answerable. A bbox entirely off the
    /// lattice yields nothing, which [`Frame::state_of`] reports as
    /// [`ShardState::OutOfDomain`] shard by shard.
    pub fn shards_for(&self, bbox: &Bbox) -> Vec<ShardId> {
        let cell = i64::from(self.cell_udeg);
        // Cell index of a coordinate, in the axis's own units, unclamped.
        let cell_of = |value: i64, origin: i64| (value - origin).div_euclid(cell);
        // The last cell an edge touches: an edge exactly on a cell boundary closes the cell before
        // it (half-open), which `ceil - 1` gives and a plain floor does not.
        let last_cell = |value: i64, origin: i64| (value - origin + cell - 1).div_euclid(cell) - 1;

        let west = i64::from(self.west_lon_udeg);
        let south = i64::from(self.south_lat_udeg);
        let col0 = cell_of(bbox.west_udeg, west).clamp(0, i64::from(self.width) - 1) as u32;
        let col1 = last_cell(bbox.east_udeg, west).clamp(0, i64::from(self.width) - 1) as u32;
        let row0 = cell_of(bbox.south_udeg, south).clamp(0, i64::from(self.height) - 1) as u32;
        let row1 = last_cell(bbox.north_udeg, south).clamp(0, i64::from(self.height) - 1) as u32;
        if col1 < col0 || row1 < row0 {
            return Vec::new();
        }
        let mut shards = Vec::new();
        for row in row0 / self.shard_height..=row1 / self.shard_height {
            for col in col0 / self.shard_width..=col1 / self.shard_width {
                shards.push(ShardId { col, row });
            }
        }
        shards
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bbox {
    pub south_udeg: i64,
    pub west_udeg: i64,
    pub north_udeg: i64,
    pub east_udeg: i64,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub offset_min: u32,
    pub valid_at: i64,
    /// Presence, one bit per shard, `row * shard_cols + col`.
    present: Vec<u8>,
    /// Exactly the shards `present` names.
    pub shards: Vec<Shard>,
}

impl Frame {
    pub fn is_present(&self, grid: &Grid, shard: ShardId) -> bool {
        let Some(bit) = grid.bit_of(shard) else { return false };
        self.present.get((bit / 8) as usize).is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
    }

    /// The three-valued answer. This is the function "a 404 must not mean dry" reduces to.
    pub fn state_of(&self, grid: &Grid, shard: ShardId) -> ShardState {
        if grid.bit_of(shard).is_none() {
            return ShardState::OutOfDomain;
        }
        if !self.is_present(grid, shard) {
            return ShardState::Dry;
        }
        match self.shards.iter().find(|entry| entry.id == shard) {
            Some(entry) => ShardState::Present {
                key: grid.shard_key(self.offset_min, shard),
                bytes: entry.bytes,
                object_crc32: entry.object_crc32,
                observed: entry.observed,
            },
            // Unreachable for a manifest this module parsed — `validate_frame` rejects a frame
            // whose bitmap and list disagree — and stated rather than unwrapped so it stays so.
            None => ShardState::Dry,
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

impl Manifest {
    pub fn frame(&self, offset_min: u32) -> Option<&Frame> {
        self.frames.iter().find(|frame| frame.offset_min == offset_min)
    }

    /// Every object this client would fetch to cover `bbox` across the whole timeline, in fetch
    /// order. Dry and out-of-domain shards produce nothing, which is the point.
    pub fn plan(&self, bbox: &Bbox) -> Vec<(u32, ShardId, ShardState)> {
        let shards = self.grid.shards_for(bbox);
        let mut plan = Vec::new();
        for frame in &self.frames {
            for shard in &shards {
                let state = frame.state_of(&self.grid, *shard);
                if matches!(state, ShardState::Present { .. }) {
                    plan.push((frame.offset_min, *shard, state));
                }
            }
        }
        plan
    }
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
    first: u32,
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
    if wire.previous_generations.iter().any(|generation| !is_safe_segment(generation)) {
        return Err(field("previous_generations"));
    }
    let grid = validate_grid(wire.lattice, wire.key_prefix, wire.generation.clone())?;
    let freshness = Freshness {
        manifest_max_age_s: wire.freshness.manifest_max_age_s.max(0),
        next_generation_expected_at: parse_rfc3339(&wire.freshness.next_generation_expected_at)
            .ok_or_else(|| field("freshness.next_generation_expected_at"))?,
        stale_after: parse_rfc3339(&wire.freshness.stale_after).ok_or_else(|| field("freshness.stale_after"))?,
    };
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
    if wire.covered_rows.first > wire.covered_rows.end || wire.covered_rows.end > wire.height {
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
        covered_rows: wire.covered_rows.first..wire.covered_rows.end,
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
