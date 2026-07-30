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

use std::collections::HashMap;

use obc_formats::io::ByteSource;

pub mod graft;
pub mod grid;
pub mod input;
pub mod nav;
pub mod poi;
pub mod qtree;
pub mod schema;
pub mod shard;
pub mod verify;

pub use input::CellInput;
pub use nav::NavStats;
pub use schema::{Band, BandRole, Schema, Skin, StyleRecord};
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
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Input(m) => write!(f, "{m}"),
            Error::Format(m) => write!(f, "{m}"),
            Error::Capacity(m) => write!(f, "{m}"),
            Error::Verify(m) => write!(f, "verify failed: {m}"),
            Error::Io(e) => write!(f, "byte source/sink error: {e:?}"),
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
    /// §5.5's fast path would otherwise take. Two callers want it: a test that has to exercise the
    /// shard planner at fixture scale, and (later) an upload path that prefers several resumable
    /// files to one big one. It changes which files are written, never what they contain.
    pub force_split: bool,
    /// Skip the §4.8 verify pass. The spec makes verification a **precondition of writing a set**,
    /// so this exists only to measure the phase split in a benchmark; a set written with it must not
    /// be handed to a device.
    pub skip_verify: bool,
}

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
    /// Bytes of geometry copied verbatim — the copy-bound half of the split.
    pub geometry_bytes: u64,
    pub nav_section_bytes: u64,
    pub poi_section_bytes: u64,
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
    pub manifest_filename: String,
    pub bytes: u64,
    pub stats: Stats,
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
    let t_start = clock.now_us();
    schema.validate().map_err(Error::Input)?;
    let styles = skin.resolve(schema).map_err(Error::Input)?;

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
    let ids: Vec<CellId> = cells.iter().map(|c| c.id).collect();
    let assembly = grid::assembly_box(&ids, schema.s_max_log2()).map_err(Error::Input)?;
    if !opts.accept_holes {
        check_no_holes(schema, &cells, assembly)?;
    }

    // --- 3. The two rebuilds. POIs are cheap; the nav graph is the assembler's real work. ---
    let core_band = schema.core_band().expect("validated: exactly one core band");
    let core_cells: Vec<&Cell<'_>> = cells.iter().filter(|c| c.band == core_band.id).collect();
    let merged_pois = poi::merge(&core_cells)?;
    let poi_section = poi::layout(&merged_pois, assembly.ubox());
    let t_poi = clock.now_us();
    let merged_nav = nav::merge(&core_cells, core_band.cell_log2, schema.routing.min_component_edges, assembly.ubox())?;
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
        cells: cells.len(),
        open_us: t_open - t_start,
        poi_us: t_poi - t_open,
        nav_us: t_nav - t_poi,
        plan_us: t_plan - t_nav,
        nav: merged_nav.stats.clone(),
        poi_records: merged_pois.pois.len(),
        poi_duplicates: merged_pois.duplicates,
        nav_section_bytes: nav_len as u64,
        poi_section_bytes: poi_len as u64,
        ..Default::default()
    };
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
                &styles,
                skin.marker_color,
                &poi_section,
                &merged_nav,
                &profile_table,
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
            Some(verify::verify_shard(src, plan.box_, plan.core)?)
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
    let manifest = shard::manifest(&plans, assembly, schema.revision, &opts.name)?;
    store.manifest(&manifest)?;

    stats.write_us = write_us;
    stats.verify_us = verify_us;
    stats.total_us = clock.now_us() - t_start;
    Ok(Summary {
        assembly_box: assembly,
        bytes: summaries.iter().map(|s| s.bytes).sum(),
        manifest_filename: shard::manifest_filename(opts.card_id),
        shards: summaries,
        stats,
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
/// that covers strictly less ground than the finest band does has a hole in it, and a hole must be
/// accepted, never discovered.
fn check_no_holes(schema: &Schema, cells: &[Cell<'_>], assembly: AlignedBox) -> Result<()> {
    // The union of the finest band's cells is the selection's own footprint; every other band must
    // cover it. (Coarser bands cover *more* — that is §1.2's generosity, not a hole.)
    let finest = schema.bands.iter().max_by_key(|b| std::cmp::Reverse(b.cell_log2));
    let Some(finest) = finest else { return Ok(()) };
    let footprint: Vec<CellId> = cells.iter().filter(|c| c.band == finest.id).map(|c| c.id).collect();
    for band in &schema.bands {
        if band.id == finest.id {
            continue;
        }
        let have: std::collections::HashSet<(i64, i64)> =
            cells.iter().filter(|c| c.band == band.id).map(|c| (c.id.i, c.id.j)).collect();
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
    let _ = assembly;
    Ok(())
}

/// Plan the volume set (§5.1/§5.5): one file when everything fits, else a core shard, one coarse
/// shard, and as many bbox-partitioned geometry shards as the target size needs.
fn plan_set(
    schema: &Schema,
    cells: &[Cell<'_>],
    assembly: AlignedBox,
    chunk_size: usize,
    opts: &Options,
    lens: (usize, usize, usize, usize, usize),
) -> Result<Vec<ShardPlan>> {
    let (style_len, poi_len, nav_len, empty_poi_len, empty_nav_len) = lens;

    // The single-file fast path: try the whole map as one core shard (§5.5).
    let all_lods: Vec<usize> = (0..schema.lods.len()).collect();
    let single = build_shard(schema, cells, assembly, chunk_size, &all_lods, 0, BandRole::Core, true)?;
    let bytes = shard::projected_bytes(&single, style_len, poi_len, nav_len);
    if bytes <= FILE_CEILING && !opts.force_split {
        let mut single = single;
        single.bytes = bytes;
        return Ok(vec![single]);
    }

    // Otherwise: the core carries no geometry at all, so every byte that can scale horizontally
    // does (§5.1).
    let mut plans = vec![build_shard(schema, cells, assembly, chunk_size, &[], 0, BandRole::Core, true)?];
    let core_bytes = shard::projected_bytes(&plans[0], style_len, poi_len, nav_len);
    if core_bytes > FILE_CEILING {
        return Err(Error::Capacity(format!(
            "the core file projects to {core_bytes} bytes, past the {FILE_CEILING}-byte ceiling. The **navigation \
             graph** is what fills it — reduce the coverage (OBCA §5.7).",
        )));
    }

    let mut index = 1usize;
    for band in &schema.bands {
        if band.role == BandRole::Core {
            continue;
        }
        let boxes = if band.role == BandRole::Coarse {
            // Exactly one by default, spanning the whole assembly, so a zoomed-out viewport is a
            // single-file read (§5.1).
            vec![assembly]
        } else {
            split_boxes(schema, cells, assembly, band, opts.target_shard_bytes)?
        };
        for b in boxes {
            let plan = build_shard(schema, cells, b, chunk_size, &band.lods, index, band.role, false)?;
            let bytes = shard::projected_bytes(&plan, style_len, empty_poi_len, empty_nav_len);
            if bytes > FILE_CEILING {
                return Err(Error::Capacity(format!(
                    "a {} shard projects to {bytes} bytes, past the ceiling — lower the target shard size",
                    band.role.as_str()
                )));
            }
            let mut plan = plan;
            plan.bytes = bytes;
            plans.push(plan);
            index += 1;
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

/// Recursive quadtree split of one geometry band's ground until each node holds at most
/// `target_bytes`, never below the band's own cell size (a smaller box would straddle a cell).
fn split_boxes(
    schema: &Schema,
    cells: &[Cell<'_>],
    box_: AlignedBox,
    band: &Band,
    target_bytes: u64,
) -> Result<Vec<AlignedBox>> {
    let bytes = band_bytes_in(schema, cells, box_, band)?;
    if bytes <= target_bytes || box_.span_log2 <= band.cell_log2 {
        return Ok(vec![box_]);
    }
    let mut out = Vec::new();
    for child in box_.children() {
        out.extend(split_boxes(schema, cells, child, band, target_bytes)?);
    }
    Ok(out)
}

/// The bytes one band contributes inside `box_` — index nodes, offset table and chunks of the LODs
/// it carries. Exactly the sum §5.7 says a consumer can compute before fetching anything.
fn band_bytes_in(schema: &Schema, cells: &[Cell<'_>], box_: AlignedBox, band: &Band) -> Result<u64> {
    let mut total = 0u64;
    for c in cells.iter().filter(|c| c.band == band.id && box_.contains_cell(c.id)) {
        for &lod in &band.lods {
            let l = c.lod(lod)?;
            total += l.node_count as u64 * 4 + (l.chunk_count as u64 + 1) * 4 + l.chunk_bytes_total as u64;
        }
    }
    let _ = schema;
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
