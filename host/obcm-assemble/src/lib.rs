//! `obcm-assemble` — the **cell assembly engine**: baked OBCA grid cells in, **one** `.obcm` out.
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
//!   [`obc_formats::io::ByteSource`] and writes through a [`MapStore`], so it has no filesystem,
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
//! 4. Prepare the raster, if the catalog published one — every terrain cell checked and placed, and
//!    the §1.3 region's length settled, because the header states it and the header goes out first.
//! 5. Plan the map: one file, the full ladder, the two rebuilt sections and the spliced raster.
//! 6. Write it, then verify it through the real reader (§4.8) — including the terrain region, read
//!    back through the §1.3 window the header now names.
//!
//! # One file
//!
//! It used to be a *set*: an OBCS manifest plus 1..N OBCM shards partitioned by band role, because
//! FAT32 capped a file at `4 GiB − 1` and OBCM's own offsets were `uint32`. Both walls are gone (the
//! flat store, and v14's scaled offsets), so the roles, the tiling, the manifest, the binding and
//! the half-bound refusal are all gone with them, and the raster that used to be a fourth file is
//! spliced into the map's tail (`OBCM_Spec.md` §1.3). What survives from §5 is the *projection*:
//! every byte is computable before it is written, which is what the ceiling is applied to.
//!
//! # Which half of §5.7 lives here
//!
//! §5.7 is the design's safety property and it names **two** actors. This crate is only one of them,
//! and the split is worth stating because half of that section reads like an obligation this code is
//! shirking:
//!
//! - **The consumer** MUST project the map *before the download*, from the catalog's published
//!   per-cell and per-band `bytes`, apply the schema's pessimistic per-cell overhead budget, and
//!   refuse a selection it cannot store. **None of that can happen here.** The assembler is handed
//!   cells that have already been fetched; by the time it can compute anything, the download it was
//!   supposed to prevent has happened. Those MUSTs belong to whatever holds the catalog — the
//!   builder, #1028. What changed with the set is that the consumer now checks **one** number
//!   against the card's free space rather than several against a ceiling.
//! - **The assembler** MUST fail rather than emit an over-size file and MUST NOT "solve" one by
//!   dropping coverage. That is [`emit::fits_ceiling`], reached from the plan and again from the
//!   write, [`Summary::warnings`], and §4.8's re-assertion of the file's actual size.
//!
//! So the projection is bounded at both ends, by two programs: refused before the fetch by the
//! catalog consumer, and re-asserted before the write here.

use std::collections::HashMap;

use obc_formats::io::ByteSource;

pub mod emit;
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
pub mod terrain;
pub mod verify;

pub use input::CellInput;
pub use terrain::{TerrainCellInput, TerrainParams, TerrainPlan, TerrainRegion};

/// One selected cell whose canonical band content is empty, as asserted by the
/// pinned catalog. It contributes coverage and therefore participates in bbox
/// and hole checks, but has no OBCM payload to open or graft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KnownEmptyInput {
    pub id: CellId,
    pub band: String,
}
pub use emit::{MapPlan, FILE_CEILING};
pub use nav::NavStats;
pub use schema::{Band, BandRole, Schema, Skin, StyleRecord};
pub use scratch::{MemoryScratch, ScratchId, ScratchStore};
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
    /// A ceiling: the per-file interior [`FILE_CEILING`], the `HoursRef` pool, the `uint32` index
    /// space (§5.7).
    Capacity(String),
    /// The §4.8 verify pass rejected the output. A failure here aborts the whole assembly — a
    /// partially written map is not a degraded one, it is an unmountable one.
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

/// How many shard bytes accumulate before [`ShardStore::write`] is called — the write-combining
/// buffer in the shard loop. 1 MiB: big enough that a per-call cost of tens of microseconds (the
/// wasm host's OPFS crossing) disappears into the stream, small enough to be noise against the
/// engine's memory budget.
const SINK_COMBINE: usize = 1024 * 1024;

/// Where the map's bytes go: opened once, streamed into, sealed, then read back for the §4.8
/// verify.
///
/// It used to be a *set* store, with a shard index threaded through every method and a manifest
/// written last as the atomicity token. There is one file now, so there is no index to thread, and
/// the atomicity that trick was faking belongs to whatever the host commits into — the flat store's
/// commit, or a browser save the rider either completes or does not.
pub trait MapStore {
    /// Open the map for streaming writes.
    fn begin(&mut self) -> Result<()>;
    /// Append to the map opened by [`MapStore::begin`].
    fn write(&mut self, buf: &[u8]) -> Result<()>;
    /// Seal the map so it can be read back.
    fn seal(&mut self) -> Result<()>;
    /// A read-only view of the sealed map, for the verify pass.
    fn source(&self) -> Result<&dyn ByteSource>;
}

/// An owned byte buffer as a random-access source, so an in-memory shard verifies through exactly
/// the same reader path as a file on a card.
#[derive(Default, Clone, Debug)]
pub struct MemorySource(pub Vec<u8>);

impl ByteSource for MemorySource {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> std::result::Result<(), obc_formats::io::Error> {
        obc_formats::io::SliceSource(&self.0).read_at(offset, buf)
    }
    fn len(&self) -> u64 {
        self.0.len() as u64
    }
}

/// A [`MapStore`] that keeps the map in memory — what the tests use, and the wasm path for a map
/// small enough to hold.
#[derive(Default, Debug)]
pub struct MemoryStore {
    pub map: MemorySource,
}

impl MapStore for MemoryStore {
    fn begin(&mut self) -> Result<()> {
        self.map.0.clear();
        Ok(())
    }
    fn write(&mut self, buf: &[u8]) -> Result<()> {
        self.map.0.extend_from_slice(buf);
        Ok(())
    }
    fn seal(&mut self) -> Result<()> {
        Ok(())
    }
    fn source(&self) -> Result<&dyn ByteSource> {
        Ok(&self.map)
    }
}

/// What an assembly can be told to do differently.
#[derive(Clone, Debug)]
pub struct Options {
    /// Proceed although a selected cell is missing — the resulting hole is legal (empty leaves), but
    /// never silent (§4.1).
    pub accept_holes: bool,
    /// Proceed although a cell is `partial` (§3.7).
    pub accept_partial: bool,
    /// Skip the §4.8 verify pass. The spec makes verification a **precondition of writing a map**,
    /// so this exists only to measure the phase split in a benchmark; a map written with it must not
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
            accept_holes: false,
            accept_partial: false,
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

/// The terrain half of an assembly (EL4): the store's lattice and the downloaded cells.
///
/// It is a separate argument rather than another [`MapStore`] method because the raster is not a
/// *stage* of the map's emission that a host could interleave — it is an input, checked and placed
/// before the header is written, and then streamed into the tail like any other region
/// (`OBCM_Spec.md` §1.3). The sink it used to carry died with the file it used to be.
pub struct TerrainJob<'a> {
    /// `OBCC_Spec.md` §13.1's `posting_log2` / `cell_log2`, verbatim from the catalog.
    pub params: TerrainParams,
    /// The downloaded cells. Known-empty squares are simply absent — an absent cell and an
    /// all-`NODATA` one answer identically (`OBCT_Spec.md` §4.3), which is §13.6's whole point.
    pub cells: Vec<TerrainCellInput<'a>>,
}

/// The spliced raster, as the caller sees it.
///
/// There is no digest here and that is deliberate: the raster is a run of bytes inside the map, and
/// the map has one identity ([`Summary::sha256`]). See [`terrain`]'s module header for why the
/// separate `terrain` record's SHA-256 was not replaced by a subrange digest.
#[derive(Clone, Copy, Debug)]
pub struct TerrainSummary {
    /// The OBCT container's exact length, before §1.3's round-up to a unit boundary.
    pub bytes: u64,
    /// Cells with a block in the region.
    pub cells: usize,
    /// Squares in the rectangle, present or not.
    pub slots: u64,
}

/// What an assembly produced: one file.
#[derive(Clone, Debug)]
pub struct Summary {
    pub assembly_box: AlignedBox,
    /// The whole file, raster included.
    pub bytes: u64,
    pub sha256: [u8; 32],
    /// The §4.8 report, or `None` under [`Options::skip_verify`].
    pub verify: Option<VerifyReport>,
    /// The map's terrain region, or `None` when the assembly carries no raster — an ordinary,
    /// complete map whose profiles are flat (`OBCC_Spec.md` §13).
    pub terrain: Option<TerrainSummary>,
    pub stats: Stats,
    /// Everything the spec says a producer SHOULD *report* rather than refuse: §5.7's headroom
    /// warning, §4.5.2's dropped duplicate POIs, `OBCM_Spec.md` §8.3's degree-cap truncations, and a
    /// chunk-capacity drop from either quadtree.
    ///
    /// The engine has no stderr — it runs in a browser tab — so a warning is a value it returns and
    /// the host decides what to do with. A caller that ignores this field ships the same bytes; a
    /// caller that prints it tells the rider what the spec wanted them told.
    pub warnings: Vec<String>,
}

/// Assemble `cells` into one map file (§4).
pub fn assemble(
    cells: Vec<CellInput<'_>>,
    schema: &Schema,
    skin: &Skin,
    opts: &Options,
    store: &mut dyn MapStore,
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
    store: &mut dyn MapStore,
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
    store: &mut dyn MapStore,
    clock: &dyn Clock,
) -> Result<Summary> {
    assemble_with_default_scratch(cells, known_empty, None, schema, skin, opts, store, clock)
}

/// Assemble the map, with the raster spliced in if there is one (EL4 #1072, `OBCM_Spec.md` §1.3).
///
/// The raster is **prepared before the layout and emitted inside the write**, which is the only
/// order §1.3 admits: the region's offset and length live in the header, and the header is the first
/// thing written. A terrain failure therefore aborts before a byte goes out rather than leaving a
/// half-elevated file behind.
///
/// `scratch` is where the §4.6 merge spills the passes it may not hold in memory (#1116 D2) — the
/// third host seam, alongside the store and the clock, and the reason the engine can sort a
/// country-scale graph without a filesystem of its own. [`assemble`] and
/// [`assemble_with_known_empty`] supply a [`MemoryScratch`] for callers that have nowhere to put it.
// One assembly is exactly these nine things; a struct would restate the signature (see
// `build_map` for the same call).
#[allow(clippy::too_many_arguments)]
pub fn assemble_full(
    cells: Vec<CellInput<'_>>,
    known_empty: Vec<KnownEmptyInput>,
    terrain: Option<TerrainJob<'_>>,
    schema: &Schema,
    skin: &Skin,
    opts: &Options,
    store: &mut dyn MapStore,
    clock: &dyn Clock,
    scratch: &dyn ScratchStore,
) -> Result<Summary> {
    let t_start = clock.now_us();
    schema.validate().map_err(Error::Input)?;
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

    // --- 4. The raster, prepared before anything is laid out. ---
    //
    // §1.3's region pointer is a **header** field, so the map cannot be laid out until the raster's
    // length is known, and the length is not known until every cell has been checked and placed.
    // That ordering is why this runs here rather than beside the write: a bad terrain cell must
    // abort the assembly before the header commits to a region that will not be there.
    let terrain_region = match &terrain {
        None => None,
        Some(job) => {
            let plan = terrain::TerrainPlan::over(job.params, assembly)?;
            debug_assert_eq!(plan.ubox(), assembly.ubox(), "the rectangle is the assembly bbox by construction");
            Some(terrain::TerrainRegion::prepare(plan, &job.cells)?)
        }
    };
    // The raster answers to the same wall as everything else now, and is refused at plan time for
    // the same reason: a region this engine cannot address is one it must not start writing.
    if let Some(region) = &terrain_region {
        emit::fits_ceiling(region.bytes(), "the terrain region")?;
    }

    // --- 5. Plan the map. ---
    let style_len = emit::pack_style_table(&styles).len();
    let poi_len = poi_section.section_len();
    let nav_projection = merged_nav.projection(&profile_table);
    let mut plan = plan_map(
        schema,
        &cells,
        assembly,
        chunk_size,
        terrain_region.as_ref().map_or(0, |r| r.bytes()),
        (style_len, poi_len, nav_projection),
    )?;
    let t_plan = clock.now_us();

    let nav_len = emit::projected_nav_bytes(&plan, style_len, poi_len, nav_projection)?;
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
        nav_section_bytes: nav_len,
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
    if plan.bytes >= emit::SIZE_WARN {
        warnings.push(format!(
            "the map projects to {} bytes, past the seven-eighths mark where OBCA §5.7 says to warn. One file holds \
             the whole selection now, so the only thing that reduces it is reducing the coverage. The hard ceiling \
             is {FILE_CEILING} bytes.",
            plan.bytes
        ));
    }
    // --- 6. Write the one file, then read it back through the real reader (§4.8). ---
    let t0 = clock.now_us();
    store.begin()?;
    let (bytes, digest) = {
        // Write-combining, because the emitters hand this sink records of tens of bytes — §8.2
        // chunks, pool records, pad runs — millions of times at country scale. A native file absorbs
        // that at a microsecond a call; the wasm host's every call is an OPFS crossing at tens of
        // them, which turned a measured 25 s native Switzerland into a projected hour in a tab.
        // Combining *here* keeps every MapStore dumb and the byte stream identical; the flush sits
        // before `seal` because the wasm sink's append cursor is the truncation point seal pins the
        // file's length to.
        let mut pending: Vec<u8> = Vec::with_capacity(SINK_COMBINE);
        let mut sink = |buf: &[u8]| -> Result<()> {
            if buf.len() >= SINK_COMBINE {
                // Already big — flush what waits (order!) and pass through.
                if !pending.is_empty() {
                    store.write(&pending)?;
                    pending.clear();
                }
                return store.write(buf);
            }
            if pending.len() + buf.len() > SINK_COMBINE {
                store.write(&pending)?;
                pending.clear();
            }
            pending.extend_from_slice(buf);
            Ok(())
        };
        let written = emit::write(
            &plan,
            &cells,
            &core_cells,
            &styles,
            skin.marker_color,
            &poi_section,
            &merged_nav,
            &profile_table,
            terrain_region.as_ref(),
            scratch,
            &mut sink,
        )?;
        if !pending.is_empty() {
            store.write(&pending)?;
        }
        written
    };
    store.seal()?;
    plan.bytes = bytes;
    plan.sha256 = digest;
    stats.geometry_bytes = plan.lods.iter().map(|l| l.chunk_bytes).sum::<u64>();
    let write_us = clock.now_us() - t0;

    let t1 = clock.now_us();
    let report = if opts.skip_verify {
        None
    } else {
        let src = store.source()?;
        if src.len() != bytes {
            return Err(Error::Verify(format!(
                "the map was written as {bytes} bytes but reads back as {} (OBCA §4.8)",
                src.len()
            )));
        }
        let report = verify::verify_map(src, plan.box_, scratch, opts.merge_budget_bytes)?;
        // §4.8 on the raster, through the §1.3 window the header now names — so what is checked is
        // the region a *device* will resolve, not a file the assembler happens to still have open.
        if let Some(region) = &terrain_region {
            let window = verify::terrain_window(src)?;
            region.verify(&window)?;
        }
        Some(report)
    };
    let verify_us = clock.now_us() - t1;
    merged_nav.release(scratch);

    let terrain_summary =
        terrain_region.as_ref().map(|r| TerrainSummary { bytes: r.bytes(), cells: r.cells(), slots: r.slots() });

    stats.write_us = write_us;
    stats.verify_us = verify_us;
    stats.total_us = clock.now_us() - t_start;
    Ok(Summary {
        assembly_box: assembly,
        bytes,
        sha256: digest,
        verify: report,
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

/// Plan the map: the full ladder in one file, sized from the graft plans and the three rebuilt
/// pieces.
///
/// This used to be a *set* planner — a single-file fast path, and behind it a core shard carrying
/// the nav graph and POIs, one coarse shard spanning the assembly, and a recursive quadtree split of
/// the geometry role into as many shards as a target size demanded, with a role-completeness check
/// and a 32-shard cap. All of it existed to work around two 4 GiB ceilings. Both are gone, so what
/// is left is the thing the fast path already did: build one plan, over every LOD.
fn plan_map(
    schema: &Schema,
    cells: &[Cell<'_>],
    assembly: AlignedBox,
    chunk_size: usize,
    terrain_bytes: u64,
    lens: (usize, u64, nav::NavProjection),
) -> Result<MapPlan> {
    let (style_len, poi_len, nav_projection) = lens;
    let all_lods: Vec<usize> = (0..schema.lods.len()).collect();
    let mut plan = build_map(schema, cells, assembly, chunk_size, &all_lods, terrain_bytes)?;
    plan.bytes = emit::projected_bytes(&plan, style_len, poi_len, nav_projection)?;
    // **The gate is the refusal.** Taking this path means "this file may be written", and what a
    // file has to clear is `emit::fits_ceiling` and nothing else. Open-coding the comparison here
    // read identically and was how FS7.5-seam's `single_file` bug survived review: a site that said
    // `FILE_CEILING` while the writable wall was smaller. There is one wall now, which removes the
    // *class* of that bug rather than merely its instance — but the routing stays, because the
    // property worth keeping is "one comparison in the crate", not "the two constants happen to be
    // equal today".
    emit::fits_ceiling(plan.bytes, "the map")?;
    Ok(plan)
}

/// Build the map's plan: a graft plan per ladder level (§3.1).
fn build_map(
    schema: &Schema,
    cells: &[Cell<'_>],
    box_: AlignedBox,
    chunk_size: usize,
    lods: &[usize],
    terrain_bytes: u64,
) -> Result<MapPlan> {
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
    Ok(MapPlan { box_, lods: plans, terrain_bytes, bytes: 0, sha256: [0; 32] })
}
