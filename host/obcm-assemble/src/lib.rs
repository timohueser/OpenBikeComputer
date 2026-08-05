//! `obcm-assemble` — the **cell assembly engine**: baked OBCA grid cells in, one `.obcm` or an OBCA
//! volume set out.
//!
//! The contract this crate implements is [`OBCA_Spec.md`](../../../specs/OBCA_Spec.md), and the
//! spec's §4 is the acceptance bar rather than a design sketch. In one paragraph: because the cell
//! grid is a power-of-two µdeg lattice and an OBCM quadtree subdivides its header bbox at integer
//! floor-midpoints, a grid-aligned assembly bbox subdivides *onto* cell boundaries — so at one depth
//! the assembly's quadtree nodes **are** the cells, and each cell's subtree grafts in with its chunk
//! bytes copied verbatim (§2). What cannot be copied is everything addressed absolutely or
//! file-locally: the POIs, the hours pool, and the whole navigation graph (§2.4). Those are rebuilt.
//!
//! # Shape of the crate
//!
//! - The **engine is GEOS-free and target-neutral**. It reads through
//!   [`obc_formats::io::ByteSource`] and writes through a [`ShardStore`], so it has no filesystem,
//!   no native dependency, and nothing that stops it compiling for `wasm32-unknown-unknown` — which
//!   CI guards, because P4 runs exactly this code in a browser tab.
//! - The **CLI** (`src/main.rs`) is a thin native driver: it opens files, implements the store over
//!   them, and prints. It owns every `std::fs` call in the crate.
//! - The **oracle** (`tests/oracle.rs`) is the acceptance bar of #1024: `pack(X)` versus
//!   `assemble(cut(X))` over the same snapped bbox, compared as *pixels* through the real renderer
//!   and as *routes* through the real A\*.
//!
//! # Order of operations
//!
//! 1. Open every cell through the real reader and refuse the §4.1 disagreements.
//! 2. Snap the assembly bbox (§4.2) — never shrink it afterwards.
//! 3. Rebuild the POI section (§4.5) and the nav graph (§4.6). Both are sized here, which is what
//!    lets every later offset be computed instead of back-patched.
//! 4. Plan the set: one file if it fits (§5.5), else core + coarse + geometry shards (§5.1).
//! 5. Write each shard, verify it through the reader (§4.8), and write the manifest **last** (§5.4).
//!
//! # Which half of §5.7 lives here
//!
//! §5.7 is the design's safety property and it names **two** actors. This crate is only one of them,
//! and the split is worth stating because half of that section reads like an obligation this code is
//! shirking:
//!
//! - **The consumer** MUST project every file of the set *before the download*, from the catalog's
//!   published per-cell and per-band `bytes`, apply the schema's pessimistic per-cell overhead
//!   budget, refuse a selection whose projection exceeds `4 GiB − 1 B`, and warn above ≈ 3.5 GiB for
//!   the core. **None of that can happen here.** The assembler is handed cells that have already
//!   been fetched; by the time it can compute anything, the download it was supposed to prevent has
//!   happened. Those MUSTs belong to whatever holds the catalog — the builder, #1028.
//! - **The assembler** MUST fail rather than emit an over-size file, MUST NOT "solve" an over-size
//!   core by splitting the nav graph or dropping coverage, and SHOULD surface the core warning. That
//!   is `plan_set`'s ceiling refusals (which name the navigation graph, because after §5.1's split
//!   no other explanation is true), [`shard::write`]'s own re-check, [`Summary::warnings`], and
//!   §4.8's re-assertion of every file's actual size.
//!
//! So the projection is bounded at both ends, by two programs: refused before the fetch by the
//! catalog consumer, and re-asserted before the write here.

use std::collections::HashMap;

use obc_formats::io::ByteSource;

pub mod extsort;
pub mod graft;
pub mod grid;
pub mod input;
pub mod nav;
pub mod poi;
pub mod prune;
pub mod qtree;
pub mod schema;
pub mod scratch;
pub mod shard;
pub mod terrain;
pub mod verify;

pub use input::CellInput;
pub use terrain::{TerrainCellInput, TerrainParams, TerrainPlan, TerrainShard, TerrainSink};

/// One selected cell whose canonical band content is empty, as asserted by the
/// pinned catalog. It contributes coverage and therefore participates in bbox
/// and hole checks, but has no OBCM payload to open or graft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownEmptyInput {
    pub id: CellId,
    pub band: String,
}
pub use nav::NavStats;
pub use schema::{Band, BandRole, Schema, Skin, StyleRecord};
pub use scratch::{MemoryScratch, ScratchId, ScratchStore};
pub use shard::{ShardPlan, FILE_CEILING};
pub use verify::VerifyReport;

use grid::{AlignedBox, CellId};
use input::Cell;

/// Everything that can stop an assembly. The variants are the spec's own refusal classes, kept
/// apart because they mean different things to a caller: an [`Error::Input`] is a selection to fix,
/// a [`Error::Capacity`] is coverage to reduce, and a [`Error::Verify`] is a bug in here.
#[derive(Debug)]
pub enum Error {
    /// A §4.1 precondition: mixed schemas, an unaccepted hole, an unaccepted partial cell.
    Input(String),
    /// A cell that does not honour the format or the cell contract.
    Format(String),
    /// A ceiling: the 4 GiB per-file limit, the `HoursRef` pool, the `uint32` index space (§5.7).
    Capacity(String),
    /// The §4.8 verify pass rejected the output. A failure here aborts the whole assembly — a
    /// partially written set is not a degraded map, it is an unmountable one.
    Verify(String),
    /// The byte source or sink failed.
    Io(obc_formats::io::Error),
    /// The host's [`ScratchStore`] failed — the merge could not spill, or could not read back what
    /// it spilled. Its own class rather than an [`Error::Io`] because it says something different to
    /// a caller: the *inputs* and the *output* are fine, the working area is not (no room, no
    /// permission, a quota), and the message names which.
    Scratch(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Input(m) => write!(f, "{m}"),
            Error::Format(m) => write!(f, "{m}"),
            Error::Capacity(m) => write!(f, "{m}"),
            Error::Verify(m) => write!(f, "verify failed: {m}"),
            Error::Io(e) => write!(f, "byte source/sink error: {e:?}"),
            Error::Scratch(m) => write!(f, "scratch store error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

/// A monotonic microsecond clock. The engine never calls `std::time` itself: `Instant` is not
/// available on `wasm32-unknown-unknown`, and phase timings are the epic's watch item, so the host
/// supplies the clock and the engine reports the split.
pub trait Clock {
    fn now_us(&self) -> u64;
}

/// A clock that does not tick — the default when a caller does not care about phase timings.
pub struct NoClock;

impl Clock for NoClock {
    fn now_us(&self) -> u64 {
        0
    }
}

/// Where a set's bytes go. The engine writes shards sequentially and hands each sealed shard back
/// for the §4.8 verify, then writes the manifest **last** (§5.4) — a half-written set therefore has
/// no manifest and is invisible as a map.
pub trait ShardStore {
    /// Open shard `plan.index` for streaming writes.
    fn begin(&mut self, plan: &ShardPlan) -> Result<()>;
    /// Append to the shard opened by [`ShardStore::begin`].
    fn write(&mut self, buf: &[u8]) -> Result<()>;
    /// Seal the open shard so it can be read back.
    fn seal(&mut self) -> Result<()>;
    /// A read-only view of a sealed shard, for the verify pass.
    fn source(&self, index: usize) -> Result<&dyn ByteSource>;
    /// The OBCS manifest. Called once, after every shard is written and verified.
    fn manifest(&mut self, bytes: &[u8]) -> Result<()>;
}

/// An owned byte buffer as a random-access source, so an in-memory shard verifies through exactly
/// the same reader path as a file on a card.
#[derive(Default, Clone, Debug)]
pub struct MemorySource(pub Vec<u8>);

impl ByteSource for MemorySource {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> std::result::Result<(), obc_formats::io::Error> {
        obc_formats::io::SliceSource(&self.0).read_at(offset, buf)
    }
    fn len(&self) -> u32 {
        self.0.len() as u32
    }
}

/// A [`ShardStore`] that keeps the set in memory — the wasm path, and what the tests use.
#[derive(Default, Debug)]
pub struct MemoryStore {
    pub shards: Vec<MemorySource>,
    pub manifest: Vec<u8>,
}

impl ShardStore for MemoryStore {
    fn begin(&mut self, plan: &ShardPlan) -> Result<()> {
        debug_assert_eq!(plan.index, self.shards.len());
        self.shards.push(MemorySource::default());
        Ok(())
    }
    fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.shards.last_mut().expect("a shard is open").0.extend_from_slice(buf);
        Ok(())
    }
    fn seal(&mut self) -> Result<()> {
        Ok(())
    }
    fn source(&self, index: usize) -> Result<&dyn ByteSource> {
        self.shards.get(index).map(|s| s as &dyn ByteSource).ok_or(Error::Io(obc_formats::io::Error::BadOffset))
    }
    fn manifest(&mut self, bytes: &[u8]) -> Result<()> {
        self.manifest = bytes.to_vec();
        Ok(())
    }
}

/// What an assembly can be told to do differently.
#[derive(Clone, Debug)]
pub struct Options {
    /// The set's display name (24 bytes on the wire, §5.2).
    pub name: String,
    /// The card id the derived filenames use (`MS<id>S<kk>.OBM`).
    pub card_id: u16,
    /// Target size for a splittable shard. The default keeps a set's file count small while staying
    /// well under the ceiling; a shard is split only when it exceeds this.
    pub target_shard_bytes: u64,
    /// Proceed although a selected cell is missing — the resulting hole is legal (empty leaves), but
    /// never silent (§4.1).
    pub accept_holes: bool,
    /// Proceed although a cell is `partial` (§3.7).
    pub accept_partial: bool,
    /// Split into a role-partitioned set **even when the whole assembly would fit one file**, which
    /// §5.5's fast path would otherwise take. Three callers want it: a test that has to exercise the
    /// shard planner at fixture scale, an operator reaching for `--force-split` to see what a set of
    /// this selection looks like, and (later) an upload path that prefers several resumable files to
    /// one big one. It changes which files are written, never what they contain.
    ///
    /// It is also the only way to reach the multi-shard planner below the 4 GiB threshold, which is
    /// why it is on the CLI: [`Options::target_shard_bytes`] alone does nothing until the map needs
    /// a set at all.
    pub force_split: bool,
    /// Skip the §4.8 verify pass. The spec makes verification a **precondition of writing a set**,
    /// so this exists only to measure the phase split in a benchmark; a set written with it must not
    /// be handed to a device.
    pub skip_verify: bool,
    /// The most memory the §4.6 merge's sorted passes may hold at once, in bytes (#1116 D2).
    ///
    /// It is the **budget, not the footprint**: a merge whose node stream is smaller than this never
    /// spills at all, and one that is larger generates runs of exactly this size and merges them
    /// back. Either way the answer is the same bytes — `the_merge_is_the_same_map_at_every_budget`
    /// pins that, and the CLI's `--merge-budget-bytes` is how a real region is re-checked at a
    /// budget it does not fit in.
    ///
    /// A host that is rationed (a browser tab) sets it from what it is allowed to use; the default
    /// is deliberately modest, because the merge's *other* structures are still resident alongside
    /// it and the sort is not where a country-scale assembly should spend its heap.
    pub merge_budget_bytes: usize,
}

/// [`Options::merge_budget_bytes`]'s default: 64 MiB.
///
/// A state-sized bake's whole node stream is about that (3.0 M nodes × 16 B on baden-württemberg),
/// so the common case sorts in one run and touches the scratch seam only for the stream itself,
/// while a country-scale one spills in bounded pieces instead of asking for gigabytes.
pub const DEFAULT_MERGE_BUDGET: usize = 64 << 20;

impl Default for Options {
    fn default() -> Self {
        Options {
            name: String::from("Map"),
            card_id: 1,
            target_shard_bytes: 1 << 30,
            accept_holes: false,
            accept_partial: false,
            force_split: false,
            skip_verify: false,
            merge_budget_bytes: DEFAULT_MERGE_BUDGET,
        }
    }
}

/// Phase timings and counters — the split the epic's P3 watch item asked for: how much of an
/// assembly is copy-bound geometry and how much is the nav rewrite.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    pub cells: usize,
    pub open_us: u64,
    pub poi_us: u64,
    /// Reading, unifying, pruning, renumbering and re-emitting the graph (§4.6) — the assembler's
    /// one O(rewrite) component.
    pub nav_us: u64,
    pub plan_us: u64,
    /// Writing every shard, which is where the verbatim geometry copy happens.
    pub write_us: u64,
    pub verify_us: u64,
    pub total_us: u64,
    pub nav: NavStats,
    pub poi_records: usize,
    pub poi_duplicates: usize,
    /// POI records the §7.3 chunk-capacity guard refused (see [`NavStats::dropped_nodes`]).
    pub poi_dropped: usize,
    /// Bytes of geometry copied verbatim — the copy-bound half of the split.
    pub geometry_bytes: u64,
    pub nav_section_bytes: u64,
    pub poi_section_bytes: u64,
}

/// The terrain half of an assembly (EL4): the store's lattice, the downloaded cells, and the
/// seekable sink the shard is written to.
///
/// It is a separate argument rather than another [`ShardStore`] method because terrain is a
/// separate *file* by rule (`OBCA_Spec.md` §5.5) written by a separate writer with a different
/// contract — [`obc_dem::container::ShardWriter`] back-patches its directory, so its sink seeks,
/// while an OBCM shard streams. Bolting a seek onto the OBCM sink to share one trait would make
/// every host implement a capability only the raster needs.
pub struct TerrainJob<'a> {
    /// `OBCC_Spec.md` §13.1's `posting_log2` / `cell_log2`, verbatim from the catalog.
    pub params: TerrainParams,
    /// The downloaded cells. Known-empty squares are simply absent — an absent cell and an
    /// all-`NODATA` one answer identically (`OBCT_Spec.md` §4.3), which is §13.6's whole point.
    pub cells: Vec<TerrainCellInput<'a>>,
    /// Where the shard's bytes go.
    pub sink: &'a mut dyn TerrainSink,
}

/// The terrain shard, as the caller sees it.
#[derive(Clone, Debug)]
pub struct TerrainSummary {
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub filename: String,
    /// Cells with a block in the shard.
    pub cells: usize,
    /// Squares in the rectangle, present or not.
    pub slots: u64,
}

/// One shard, as the caller sees it.
#[derive(Clone, Debug)]
pub struct ShardSummary {
    pub index: usize,
    pub role: BandRole,
    pub bbox: AlignedBox,
    pub bytes: u64,
    pub sha256: [u8; 32],
    pub filename: String,
    pub verify: Option<VerifyReport>,
}

/// What an assembly produced.
#[derive(Clone, Debug)]
pub struct Summary {
    pub assembly_box: AlignedBox,
    pub shards: Vec<ShardSummary>,
    /// The set's terrain shard, or `None` when the assembly carries no raster — an ordinary,
    /// complete map whose profiles are flat (`OBCC_Spec.md` §13).
    pub terrain: Option<TerrainSummary>,
    pub manifest_filename: String,
    /// Every file of the set, terrain included.
    pub bytes: u64,
    pub stats: Stats,
    /// Everything the spec says a producer SHOULD *report* rather than refuse: §5.7's core-headroom
    /// warning, §4.5.2's dropped duplicate POIs, `OBCM_Spec.md` §8.3's degree-cap truncations, and a
    /// chunk-capacity drop from either quadtree.
    ///
    /// The engine has no stderr — it runs in a browser tab — so a warning is a value it returns and
    /// the host decides what to do with. A caller that ignores this field ships the same bytes; a
    /// caller that prints it tells the rider what the spec wanted them told.
    pub warnings: Vec<String>,
}

/// Assemble `cells` into a volume set (§4, §5).
pub fn assemble(
    cells: Vec<CellInput<'_>>,
    schema: &Schema,
    skin: &Skin,
    opts: &Options,
    store: &mut dyn ShardStore,
    clock: &dyn Clock,
) -> Result<Summary> {
    assemble_with_known_empty(cells, Vec::new(), schema, skin, opts, store, clock)
}

/// The scratch a caller that has not supplied one gets: [`MemoryScratch`].
///
/// It keeps the small API small, and it is honest about what it costs — a spill held in RAM is the
/// residency the spill exists to remove, so a host that is rationed should hand in its own (the CLI
/// hands in temp files) rather than take this.
// The same eight things `assemble_full` takes, minus the seam this supplies.
#[allow(clippy::too_many_arguments)]
fn assemble_with_default_scratch(
    cells: Vec<CellInput<'_>>,
    known_empty: Vec<KnownEmptyInput>,
    terrain: Option<TerrainJob<'_>>,
    schema: &Schema,
    skin: &Skin,
    opts: &Options,
    store: &mut dyn ShardStore,
    clock: &dyn Clock,
) -> Result<Summary> {
    let scratch = MemoryScratch::new();
    assemble_full(cells, known_empty, terrain, schema, skin, opts, store, clock, &scratch)
}

/// Assemble artifacts plus explicit zero-byte coverage from a pinned catalog.
///
/// Kept beside [`assemble`] so native callers that only have artifacts retain
/// the small API, while hosted builders can preserve selected empty ground
/// without manufacturing an empty OBCM object for every such cell.
pub fn assemble_with_known_empty(
    cells: Vec<CellInput<'_>>,
    known_empty: Vec<KnownEmptyInput>,
    schema: &Schema,
    skin: &Skin,
    opts: &Options,
    store: &mut dyn ShardStore,
    clock: &dyn Clock,
) -> Result<Summary> {
    assemble_with_default_scratch(cells, known_empty, None, schema, skin, opts, store, clock)
}

/// Assemble a set, with the raster if there is one (EL4, #1072).
///
/// The terrain shard is written **after** every OBCM shard is written and verified and **before**
/// the manifest, which is the only order §5.4 admits: the manifest is the atomicity token, so
/// nothing it names may be missing when it lands, and a terrain failure must leave the set
/// unmountable rather than half-elevated.
/// `scratch` is where the §4.6 merge spills the passes it may not hold in memory (#1116 D2) — the
/// third host seam, alongside the store and the clock, and the reason the engine can sort a
/// country-scale graph without a filesystem of its own. [`assemble`] and
/// [`assemble_with_known_empty`] supply a [`MemoryScratch`] for callers that have nowhere to put it.
// One assembly is exactly these nine things; a struct would restate the signature (see
// `build_shard` for the same call).
#[allow(clippy::too_many_arguments)]
pub fn assemble_full(
    cells: Vec<CellInput<'_>>,
    known_empty: Vec<KnownEmptyInput>,
    terrain: Option<TerrainJob<'_>>,
    schema: &Schema,
    skin: &Skin,
    opts: &Options,
    store: &mut dyn ShardStore,
    clock: &dyn Clock,
    scratch: &dyn ScratchStore,
) -> Result<Summary> {
    let t_start = clock.now_us();
    schema.validate().map_err(Error::Input)?;
    shard::check_card_id(opts.card_id)?;
    let styles = skin.resolve(schema).map_err(Error::Input)?;
    let mut warnings: Vec<String> = Vec::new();

    // --- 1. Open every cell through the real reader, and refuse the §4.1 disagreements. ---
    let cache = obc_reader::MapCache::new_boxed();
    let mut open: Vec<Cell<'_>> = Vec::with_capacity(cells.len());
    for c in cells {
        if schema.band(&c.band).is_none() {
            return Err(Error::Input(format!("cell {}: band {:?} is not in the schema", c.id, c.band)));
        }
        open.push(Cell::open(c, &cache)?);
    }
    input::check_agreement(&open, opts.accept_partial)?;
    let cells = open;

    let mut coverage: Vec<(String, CellId)> = cells.iter().map(|c| (c.band.clone(), c.id)).collect();
    let mut seen: std::collections::HashSet<(String, CellId)> = coverage.iter().cloned().collect();
    for empty in known_empty {
        let band = schema.band(&empty.band).ok_or_else(|| {
            Error::Input(format!("known-empty cell {}: band {:?} is not in the schema", empty.id, empty.band))
        })?;
        if empty.id.log2 != band.cell_log2 {
            return Err(Error::Input(format!(
                "known-empty cell {} is not band {:?}'s 2^{} grid",
                empty.id, empty.band, band.cell_log2
            )));
        }
        if !seen.insert((empty.band.clone(), empty.id)) {
            return Err(Error::Input(format!(
                "cell {} of band {:?} is listed more than once across artifacts and known-empty coverage",
                empty.id, empty.band
            )));
        }
        coverage.push((empty.band, empty.id));
    }
    // The skin may only restyle ids the cells' chunk bytes actually reference (§4.7/§6.2): the
    // schema owns the assignment, the skin owns the values, and a mismatch here would ship a map
    // with an invisible layer or a style nothing draws.
    let resolved_ids: Vec<u8> = styles.iter().map(|s| s.id).collect();
    if resolved_ids != cells[0].style_ids {
        return Err(Error::Input(format!(
            "the stamped style table has ids {resolved_ids:?} but the cells were baked with {:?} — the skin does not \
             match this schema revision (OBCA §4.7)",
            cells[0].style_ids
        )));
    }
    let t_open = clock.now_us();

    // Coverage sanity (§4.1): a missing cell inside the selection is legal and produces empty
    // leaves, but the caller has to have said so.
    let profile_table = cells[0].profile_table.clone();
    let chunk_size = pick_chunk_size(schema, &cells)?;

    // --- 2. The assembly bbox: the minimal grid-aligned power-of-two box (§4.2). ---
    let ids: Vec<CellId> = coverage.iter().map(|(_, id)| *id).collect();
    let assembly = grid::assembly_box(&ids, schema.s_max_log2()).map_err(Error::Input)?;
    if !opts.accept_holes {
        check_no_holes(schema, &coverage)?;
    }

    // --- 3. The two rebuilds. POIs are cheap; the nav graph is the assembler's real work. ---
    let core_band = schema.core_band().expect("validated: exactly one core band");
    let core_cells: Vec<&Cell<'_>> = cells.iter().filter(|c| c.band == core_band.id).collect();
    let merged_pois = poi::merge(&core_cells)?;
    let poi_section = poi::layout(&merged_pois, assembly.ubox());
    let t_poi = clock.now_us();
    let merged_nav = nav::merge(
        &core_cells,
        core_band.cell_log2,
        schema.routing.min_component_edges,
        assembly.ubox(),
        scratch,
        opts.merge_budget_bytes,
    )?;
    let t_nav = clock.now_us();

    // --- 4. Plan the set. ---
    let style_len = shard::pack_style_table(&styles).len();
    let poi_len = poi_section.section_len();
    let nav_len = merged_nav.section_len(&profile_table);
    let empty_poi_len = poi::empty_layout(assembly.ubox()).section_len();
    let empty_nav_len = nav::MergedNav::empty(Default::default()).section_len(&profile_table);
    let mut plans = plan_set(
        schema,
        &cells,
        assembly,
        chunk_size,
        opts,
        (style_len, poi_len, nav_len, empty_poi_len, empty_nav_len),
    )?;
    let t_plan = clock.now_us();

    // --- 5. Write, verify, then the manifest — in that order, always (§5.4). ---
    let mut stats = Stats {
        cells: coverage.len(),
        open_us: t_open - t_start,
        poi_us: t_poi - t_open,
        nav_us: t_nav - t_poi,
        plan_us: t_plan - t_nav,
        nav: merged_nav.stats.clone(),
        poi_records: merged_pois.pois.len(),
        poi_duplicates: merged_pois.duplicates,
        poi_dropped: poi_section.dropped(),
        nav_section_bytes: nav_len as u64,
        poi_section_bytes: poi_len as u64,
        ..Default::default()
    };

    // Everything the spec says to report rather than refuse (§4.5.2, §5.7, `OBCM_Spec.md` §8.3).
    if stats.poi_duplicates > 0 {
        warnings.push(format!(
            "{} POI record(s) were dropped as duplicates of a (lat, lon, subtype) already seen. §3.6 gives each POI \
             exactly one cell, so a non-zero count means the selection overlaps itself or a cell was baked twice \
             (OBCA §4.5.2).",
            stats.poi_duplicates
        ));
    }
    if stats.nav.degree_truncated > 0 {
        warnings.push(format!(
            "{} adjacency entrie(s) were dropped at the §8.3 degree cap of {}. Each dropped arc survives one-way \
             through the neighbour's own record, which §8.3 permits, but the turn is gone in one direction.",
            stats.nav.degree_truncated,
            obc_formats::obcm::NAV_MAX_DEGREE
        ));
    }
    for (count, what) in
        [(stats.nav.dropped_nodes, "junction"), (stats.poi_dropped, "POI")].into_iter().filter(|(n, _)| *n > 0)
    {
        warnings.push(format!(
            "{count} {what} record(s) exceeded their chunk's capacity and were dropped rather than written past it — \
             co-located records past the quadtree's recursion floor."
        ));
    }
    if let Some(core) = plans.iter().find(|p| p.core) {
        if core.bytes >= shard::CORE_WARN {
            warnings.push(format!(
                "the core shard is {} bytes, past the ~3.5 GiB mark where OBCA §5.7 says to warn. The **navigation \
                 graph** is what fills it — the core is nav plus POIs and nothing else, so the only thing that \
                 reduces it is reducing the coverage. The hard ceiling is {FILE_CEILING} bytes.",
                core.bytes
            ));
        }
    }
    let mut summaries = Vec::with_capacity(plans.len());
    let mut verify_us = 0u64;
    let mut write_us = 0u64;
    for plan in &mut plans {
        let t0 = clock.now_us();
        store.begin(plan)?;
        let (bytes, digest) = {
            let mut sink = |buf: &[u8]| store.write(buf);
            shard::write(
                plan,
                &cells,
                &core_cells,
                &styles,
                skin.marker_color,
                &poi_section,
                &merged_nav,
                &profile_table,
                scratch,
                &mut sink,
            )?
        };
        store.seal()?;
        plan.bytes = bytes;
        plan.sha256 = digest;
        stats.geometry_bytes += plan.lods.iter().map(|l| l.chunk_bytes).sum::<u64>();
        write_us += clock.now_us() - t0;

        let t1 = clock.now_us();
        let report = if opts.skip_verify {
            None
        } else {
            let src = store.source(plan.index)?;
            if src.len() as u64 != bytes {
                return Err(Error::Verify(format!(
                    "shard {} was written as {bytes} bytes but reads back as {} (OBCA §5.3)",
                    plan.index,
                    src.len()
                )));
            }
            Some(verify::verify_shard(src, plan.box_, plan.core, scratch, opts.merge_budget_bytes)?)
        };
        verify_us += clock.now_us() - t1;
        summaries.push(ShardSummary {
            index: plan.index,
            role: plan.role,
            bbox: plan.box_,
            bytes,
            sha256: digest,
            filename: shard::shard_filename(opts.card_id, plan.index),
            verify: report,
        });
    }
    check_set_invariants(&plans, assembly)?;
    // Every shard that could name the merged graph's scratch streams has been written and verified,
    // so the terrain raster below gets the whole scratch area rather than sharing it (#1116 D4).
    merged_nav.release(scratch);

    // The raster, between the last verified shard and the manifest (§5.4).
    let terrain_summary = match terrain {
        None => None,
        Some(job) => {
            let t0 = clock.now_us();
            let plan = terrain::TerrainPlan::over(job.params, assembly)?;
            debug_assert_eq!(plan.ubox(), assembly.ubox(), "the rectangle is the assembly bbox by construction");
            let written = terrain::write_shard(plan, &job.cells, job.sink)?;
            if written.bytes > FILE_CEILING {
                return Err(Error::Capacity(format!(
                    "the terrain shard is {} bytes, past the {FILE_CEILING}-byte ceiling. Terrain is one file per set \
                     in v1, so the only thing that reduces it is reducing the coverage (OBCA §5.7).",
                    written.bytes
                )));
            }
            verify_us += clock.now_us() - t0;
            Some(TerrainSummary {
                bytes: written.bytes,
                sha256: written.sha256,
                filename: shard::terrain_filename(opts.card_id),
                cells: written.cells,
                slots: written.slots,
            })
        }
    };

    let terrain_record = terrain_summary.as_ref().map(|t| shard::TerrainRecord { bytes: t.bytes, sha256: t.sha256 });
    let manifest = shard::manifest(&plans, terrain_record, assembly, schema.revision, &opts.name)?;
    store.manifest(&manifest)?;

    stats.write_us = write_us;
    stats.verify_us = verify_us;
    stats.total_us = clock.now_us() - t_start;
    Ok(Summary {
        assembly_box: assembly,
        bytes: summaries.iter().map(|s| s.bytes).sum::<u64>() + terrain_summary.as_ref().map_or(0, |t| t.bytes),
        manifest_filename: shard::manifest_filename(opts.card_id),
        shards: summaries,
        terrain: terrain_summary,
        stats,
        warnings,
    })
}

/// The per-LOD chunk capacity the output writes. The schema states it; when it does not, the cells'
/// own value is taken — but every cell must agree, or two chunks would mean different things.
fn pick_chunk_size(schema: &Schema, cells: &[Cell<'_>]) -> Result<usize> {
    let mut found: Option<usize> = None;
    for c in cells {
        for l in &c.lods {
            if l.node_count == 0 {
                continue;
            }
            match found {
                None => found = Some(l.chunk_size),
                Some(v) if v != l.chunk_size => {
                    return Err(Error::Input(format!(
                        "cells disagree on the LOD chunk capacity ({v} and {}) — they are not one schema revision",
                        l.chunk_size
                    )))
                }
                _ => {}
            }
        }
    }
    match (schema.chunk_size, found) {
        (0, Some(v)) => Ok(v),
        (0, None) => Ok(obc_formats::obcm::POI_CHUNK_SIZE * 8), // no geometry anywhere: any legal value
        (v, Some(f)) if v != f => Err(Error::Input(format!(
            "the schema says chunk_size {v} but the cells were written at {f} — they are not one schema revision"
        ))),
        (v, _) => Ok(v),
    }
}

/// §4.1: every band's coverage must be the cells whose square intersects the selection. The
/// assembler cannot know the caller's selection polygon, so it applies the checkable half — a band
/// that covers strictly less ground than the selection's finest cells do has a hole in it, and a
/// hole must be accepted, never discovered.
///
/// The footprint is every cell of **every** band at the smallest cell size in the table, not one
/// band picked from among them. At the v1 table `fine` and `network` share `2^18`, so "the finest
/// band" is a tie that `max_by_key` would resolve by table position — which decides *which* of two
/// under-covered bands gets reported, and can let the other one's hole through. Taking their union
/// removes the tie-break and checks strictly more.
fn check_no_holes(schema: &Schema, coverage: &[(String, CellId)]) -> Result<()> {
    let Some(finest_log2) = schema.bands.iter().map(|b| b.cell_log2).min() else { return Ok(()) };
    let footprint: Vec<CellId> = {
        let mut ids: Vec<CellId> = coverage
            .iter()
            .filter(|(band, _)| schema.band(band).is_some_and(|b| b.cell_log2 == finest_log2))
            .map(|(_, id)| *id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    for band in &schema.bands {
        let have: std::collections::HashSet<(i64, i64)> =
            coverage.iter().filter(|(band_id, _)| *band_id == band.id).map(|(_, id)| (id.i, id.j)).collect();
        for f in &footprint {
            let (min_lon, min_lat, _, _) = f.square();
            let covering = CellId::containing(band.cell_log2, min_lat, min_lon);
            if !have.contains(&(covering.i, covering.j)) {
                return Err(Error::Input(format!(
                    "band {:?} is missing cell {covering}, which covers {f}: the assembly would have a hole there. \
                     Accept the hole explicitly if that is intended (OBCA §4.1).",
                    band.id
                )));
            }
        }
    }
    Ok(())
}

/// Plan the volume set (§5.1/§5.5): one file when everything fits, else a core shard, one coarse
/// shard, and as many bbox-partitioned geometry shards as the target size needs.
///
/// # One tiling per role, not per band
///
/// §5.1 defines the split by **role**, and it says so in the plural: "geometry shards carry the
/// `mid`- and `fine`-band LODs and nothing else", and "the shards of one role **tile** the assembly
/// bbox". A tiling per *band* is therefore not a finer-grained version of the same thing — at the v1
/// table, where `mid` and `fine` are both `role = geometry`, it emits two overlapping antichains of
/// `Role == 1` shards whose areas sum to twice the assembly, which §5.3's own validation rejects.
/// So the geometry role is planned **once**, over the combined bytes of every geometry band, and
/// each shard it produces carries the union of those bands' LODs.
fn plan_set(
    schema: &Schema,
    cells: &[Cell<'_>],
    assembly: AlignedBox,
    chunk_size: usize,
    opts: &Options,
    lens: (usize, u64, u64, u64, u64),
) -> Result<Vec<ShardPlan>> {
    let (style_len, poi_len, nav_len, empty_poi_len, empty_nav_len) = lens;

    // The single-file fast path: try the whole map as one core shard (§5.5). Skipped outright under
    // `force_split`, which would otherwise plan the whole map twice to throw the first one away.
    if !opts.force_split {
        let all_lods: Vec<usize> = (0..schema.lods.len()).collect();
        let mut single = build_shard(schema, cells, assembly, chunk_size, &all_lods, 0, BandRole::Core, true)?;
        single.bytes = shard::projected_bytes(&single, style_len, poi_len, nav_len)?;
        if single.bytes <= FILE_CEILING {
            return Ok(vec![single]);
        }
    }

    // Otherwise: the core carries no geometry at all, so every byte that can scale horizontally
    // does (§5.1).
    let mut plans = vec![build_shard(schema, cells, assembly, chunk_size, &[], 0, BandRole::Core, true)?];
    let core_bytes = shard::projected_bytes(&plans[0], style_len, poi_len, nav_len)?;
    if core_bytes > FILE_CEILING {
        return Err(Error::Capacity(format!(
            "the core file projects to {core_bytes} bytes, past the {FILE_CEILING}-byte ceiling. The **navigation \
             graph** is what fills it — reduce the coverage (OBCA §5.7).",
        )));
    }
    plans[0].bytes = core_bytes;

    // §5.3, checked before a byte is written: a multi-shard set with a whole role missing does not
    // mount, and the schema is the only place that can be wrong about it.
    for role in [BandRole::Coarse, BandRole::Geometry] {
        if !schema.bands.iter().any(|b| b.role == role) {
            return Err(Error::Input(format!(
                "the selection needs a volume set, but the schema's band table names no {} band — a set with no {} \
                 shard is one no reader mounts (OBCA §5.3)",
                role.as_str(),
                role.as_str()
            )));
        }
    }

    let mut index = 1usize;
    let mut push = |plans: &mut Vec<ShardPlan>, box_: AlignedBox, lods: &[usize], role: BandRole| -> Result<()> {
        let mut plan = build_shard(schema, cells, box_, chunk_size, lods, index, role, false)?;
        plan.bytes = shard::projected_bytes(&plan, style_len, empty_poi_len, empty_nav_len)?;
        if plan.bytes > FILE_CEILING {
            return Err(Error::Capacity(format!(
                "a {} shard projects to {} bytes, past the ceiling — lower the target shard size",
                role.as_str(),
                plan.bytes
            )));
        }
        plans.push(plan);
        index += 1;
        Ok(())
    };

    // The coarse role: exactly one shard spanning the whole assembly, so a zoomed-out viewport is a
    // single-file read (§5.1). At most one band may claim it (the schema validates that).
    for band in schema.bands.iter().filter(|b| b.role == BandRole::Coarse) {
        push(&mut plans, assembly, &band.lods, BandRole::Coarse)?;
    }

    // The geometry role: one tiling over every geometry band at once (see the note above).
    let geometry: Vec<&Band> = schema.bands.iter().filter(|b| b.role == BandRole::Geometry).collect();
    if !geometry.is_empty() {
        let mut lods: Vec<usize> = geometry.iter().flat_map(|b| b.lods.iter().copied()).collect();
        lods.sort_unstable();
        // A box below the *coarsest* geometry band's cell size would straddle one of its cells, so
        // the recursion floor is that band's, not each band's own.
        let floor = geometry.iter().map(|b| b.cell_log2).max().expect("non-empty");
        for b in split_boxes(cells, assembly, &geometry, floor, opts.target_shard_bytes)? {
            push(&mut plans, b, &lods, BandRole::Geometry)?;
        }
    }

    if plans.len() > shard::MAX_SHARDS {
        return Err(Error::Capacity(format!(
            "the selection needs {} shards; a set holds at most {} (OBCA §5.2) — raise the target shard size",
            plans.len(),
            shard::MAX_SHARDS
        )));
    }
    Ok(plans)
}

/// Recursive quadtree split of the geometry role's ground until each node holds at most
/// `target_bytes`, never below `floor_log2` (a smaller box would straddle a cell of the coarsest
/// band being tiled).
fn split_boxes(
    cells: &[Cell<'_>],
    box_: AlignedBox,
    bands: &[&Band],
    floor_log2: u32,
    target_bytes: u64,
) -> Result<Vec<AlignedBox>> {
    if bands_bytes_in(cells, box_, bands)? <= target_bytes || box_.span_log2 <= floor_log2 {
        return Ok(vec![box_]);
    }
    let mut out = Vec::new();
    for child in box_.children() {
        out.extend(split_boxes(cells, child, bands, floor_log2, target_bytes)?);
    }
    Ok(out)
}

/// The bytes `bands` together contribute inside `box_` — index nodes, offset table and chunks of the
/// LODs they carry. Exactly the sum §5.7 says a consumer can compute before fetching anything.
fn bands_bytes_in(cells: &[Cell<'_>], box_: AlignedBox, bands: &[&Band]) -> Result<u64> {
    let mut total = 0u64;
    for band in bands {
        for c in cells.iter().filter(|c| c.band == band.id && box_.contains_cell(c.id)) {
            for &lod in &band.lods {
                let l = c.lod(lod)?;
                total += l.node_count as u64 * 4 + (l.chunk_count as u64 + 1) * 4 + l.chunk_bytes_total as u64;
            }
        }
    }
    Ok(total)
}

/// Build one shard's plan: a graft plan per ladder level, empty for the levels this shard's role
/// does not carry (§5.1) — every shard lists the full ladder (§3.1).
// One shard is defined by exactly these eight facts; a struct would restate the signature.
#[allow(clippy::too_many_arguments)]
fn build_shard(
    schema: &Schema,
    cells: &[Cell<'_>],
    box_: AlignedBox,
    chunk_size: usize,
    lods: &[usize],
    index: usize,
    role: BandRole,
    core: bool,
) -> Result<ShardPlan> {
    let mut plans = Vec::with_capacity(schema.lods.len());
    for (i, entry) in schema.lods.iter().enumerate() {
        if !lods.contains(&i) {
            plans.push(graft::LodPlan::empty(i, entry.max_mpp, chunk_size));
            continue;
        }
        let band = schema.band_of_lod(i).ok_or_else(|| Error::Input(format!("LOD {i} is in no band")))?;
        let present: HashMap<(i64, i64), usize> = cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.band == band.id && box_.contains_cell(c.id))
            .map(|(k, c)| ((c.id.i, c.id.j), k))
            .collect();
        plans.push(graft::plan_lod(i, entry.max_mpp, chunk_size, box_, band.cell_log2, &present, cells)?);
    }
    Ok(ShardPlan { index, role, box_, lods: plans, core, bytes: 0, sha256: [0; 32] })
}

/// §4.8.6's set invariants, checked once the shards are written: exactly one core whose bbox is the
/// assembly bbox, every file under the ceiling, and each non-core role tiling the assembly bbox
/// without overlap.
fn check_set_invariants(plans: &[ShardPlan], assembly: AlignedBox) -> Result<()> {
    let cores: Vec<&ShardPlan> = plans.iter().filter(|p| p.core).collect();
    if cores.len() != 1 {
        return Err(Error::Verify(format!("a set has exactly one core shard, found {}", cores.len())));
    }
    if cores[0].box_ != assembly {
        return Err(Error::Verify("the core shard's bbox is not the assembly bbox (OBCA §5.3)".into()));
    }
    for p in plans {
        if p.bytes > FILE_CEILING {
            return Err(Error::Verify(format!("shard {} is {} bytes, past the ceiling", p.index, p.bytes)));
        }
    }
    for role in [BandRole::Coarse, BandRole::Geometry] {
        let boxes: Vec<AlignedBox> = plans.iter().filter(|p| !p.core && p.role == role).map(|p| p.box_).collect();
        if boxes.is_empty() {
            continue;
        }
        let area: u128 = boxes.iter().map(|b| 1u128 << (2 * b.span_log2)).sum();
        if area != 1u128 << (2 * assembly.span_log2) {
            return Err(Error::Verify(format!(
                "the {} shards do not tile the assembly bbox (OBCA §5.1)",
                role.as_str()
            )));
        }
        for (i, a) in boxes.iter().enumerate() {
            for b in &boxes[i + 1..] {
                let (a_min_lon, a_min_lat, a_max_lon, a_max_lat) = a.ubox();
                let (b_min_lon, b_min_lat, b_max_lon, b_max_lat) = b.ubox();
                let overlaps =
                    a_min_lon < b_max_lon && b_min_lon < a_max_lon && a_min_lat < b_max_lat && b_min_lat < a_max_lat;
                if overlaps {
                    return Err(Error::Verify(format!("two {} shards overlap", role.as_str())));
                }
            }
        }
    }
    Ok(())
}
