//! Volume sets: planning the shards, writing one shard's bytes, and the OBCS manifest
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §5).
//!
//! One *logical* map is a small manifest plus 1..N physical OBCM files. Two independent 4 GiB
//! ceilings made that necessary when this was written — FAT32's per-file cap and OBCM's own
//! `uint32` offsets — so sets are the shape from day one and a small map is a **set of one** (§5.5).
//!
//! Both of those are gone: the flat store replaced FAT, and v14 scales offsets to a 64 GiB
//! interior. What binds now is neither — it is the **read seam**, `ByteSource`'s `u32` offsets, and
//! it lands on the same 4 GiB by coincidence rather than by inheritance. See [`FILE_CEILING`].
//!
//! The split obeys one ordering principle and obeys it everywhere: **the core file holds only what
//! cannot be split by bbox, and everything that can be is moved out of it.** The core is the one
//! file that cannot scale horizontally (it holds the single unified nav graph), so its headroom is
//! the scarcest resource in the design and nothing else may spend it.

use obc_formats::obcm::{
    OffsetScale, FILLER, HEADER_LEN, LOD_ENTRY_LEN, MAGIC, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT,
    STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK, STYLE_RECORD_LEN, STYLE_TERRAIN_LAYER_BIT, VERSION,
};
use sha2::{Digest, Sha256};

use crate::graft::{self, LodPlan};
use crate::grid::AlignedBox;
use crate::input::Cell;
use crate::nav::MergedNav;
use crate::poi::PoiSection;
use crate::schema::{BandRole, StyleRecord};
use crate::scratch::ScratchStore;
use crate::{Error, Result};

/// The hard per-file ceiling: **the smaller of the two walls a written file has to clear.**
///
/// 1. **The format wall** — `OBCM_Spec.md` §1.1's addressable interior at this engine's [`SCALE`],
///    `2^32 units × U`, which at `U = 16` is `2^36` B = 64 GiB. Derived rather than written down,
///    because §1.1 states the producer rule in exactly these terms and [`OffsetScale::covers`] is
///    that sentence as a predicate.
/// 2. **The readable wall** — `u32::MAX`, because [`obc_formats::io::ByteSource`] *is* the tree's
///    read interface and it is `read_at(offset: u32)` / `len() -> u32`. Every implementor and every
///    cache behind it is u32-addressed, and `obc-reader` fail-closes past that by design.
///
/// **Today the readable wall binds**, at 4 GiB. v14 raised the format wall sixteen-fold and did not
/// touch the read seam, so a file between the two is one this pipeline could lay out and *nothing
/// in this tree could open* — the fast path would plan it, the assembler would write it, and the
/// first `read_at` past 4 GiB would fail. §5.5 makes that worse rather than better: the single file
/// is still manifest-listed, and OBCA §5.2's `Bytes` is a `uint32` of **bytes**, so a 5 GiB file
/// would be *recorded* as ≈0.7 GiB rather than refused.
///
/// So the ceiling is the `min`, and it moves on its own when the read seam widens (u32 → u64 or
/// scaled offsets through `ByteSource` and its implementors) — a slice of #1420 in its own right,
/// and the prerequisite for DACH-scale single files. §8's edge pool is already built for the far
/// wall: `NAV_EDGE_MAX_CHUNKS × NAV_CHUNK_SIZE == 1 << 36` is pinned in `obc-formats` as "the pool
/// reaches the interior".
pub const FILE_CEILING: u64 = {
    let format = (1u64 << 32) * SCALE.unit();
    let readable = u32::MAX as u64;
    if format < readable {
        format
    } else {
        readable
    }
};
const _: () = assert!(FILE_CEILING <= u32::MAX as u64, "no reader in this tree addresses past a u32");
const _: () = assert!(SCALE.covers(FILE_CEILING), "and the scale still covers whatever the min lands on");

/// The `Offset Scale` every shard this engine writes carries (`OBCM_Spec.md` §1.1): `U = 16`, the
/// same byte `obc-pack` writes, so a cell and the assembly it lands in count offsets in one unit.
///
/// It is also what the §4.1 agreement check refuses a disagreement on: an assembly holds many cell
/// files and one output open at once, and a cell whose `Index Offset` counted a *different* unit
/// would relocate into a plausible byte of the output rather than an obviously wrong one.
pub const SCALE: OffsetScale = OffsetScale::DEFAULT;

/// The byte offset of the style table in every shard this engine writes: the first unit boundary at
/// or after the 49-byte header (§1.2), which at `U = 16` is `64` — so `Style Offset` is `4` and
/// bytes `49..64` are [`FILLER`]. Byte-for-byte `obc-pack`'s own `STYLE_OFFSET`.
pub const STYLE_OFFSET: u64 = 64;
const _: () = assert!(STYLE_OFFSET >= HEADER_LEN as u64);

/// The next unit boundary at or after `cursor` (§1.2's `align_up`). Every structure a header or
/// directory offset reaches begins on one; the `0..U-1` bytes this rounds past are [`FILLER`].
#[inline]
pub fn align_up(cursor: u64) -> u64 {
    SCALE.align_up(cursor).expect("a layout cursor never approaches u64::MAX")
}

/// The filler run [`align_up`] implies at `cursor` — `0..U-1` bytes of `0xFF`.
#[inline]
pub fn filler_len(cursor: u64) -> u64 {
    align_up(cursor) - cursor
}

/// The `uint32` a scaled offset field stores for byte offset `at` (§1.1).
///
/// A scaled offset **cannot** name a byte that is not a multiple of `U`, so a non-boundary argument
/// is a bug in the layout above it rather than a rounding request — but this is an engine that runs
/// in a browser tab, so it is an [`Error::Capacity`] and not a panic. It is also where §1.1's
/// producer rule bites in practice: a layout whose offsets do not fit `uint32` units is one this
/// scale does not cover, and the producer is the only party positioned to notice.
#[inline]
pub fn scaled(at: u64) -> Result<u32> {
    SCALE.scaled(at).map(|o| o.units()).ok_or_else(|| {
        Error::Capacity(format!(
            "byte {at} cannot be named by a scaled offset at `Offset Scale` {} — it is either off the {}-byte unit \
             boundary or past the interior that scale covers (OBCM §1.1)",
            SCALE.log2(),
            SCALE.unit()
        ))
    })
}

/// A run of [`FILLER`] long enough for any single §1.2 gap this writer emits — one unit for the
/// section boundaries, one 512-byte sector for §8.1's alignment runs. Sliced, never allocated.
pub(crate) const FILLER_RUN: [u8; obc_formats::obcm::NAV_CHUNK_SIZE] = [FILLER; obc_formats::obcm::NAV_CHUNK_SIZE];

/// Where a producer SHOULD warn about the core (OBCA §5.7), naming the nav graph.
///
/// §5.7 wrote this as "≈ 3.5 GiB" against a `4 GiB − 1 B` ceiling — seven eighths of the wall, i.e.
/// "you are close". Written as the **proportion** rather than the number, so it keeps meaning
/// "close" wherever [`FILE_CEILING`] lands: while the readable wall binds this is ≈3.5 GiB, exactly
/// what §5.7 wrote, and it follows the ceiling up on its own when the read seam widens.
pub const CORE_WARN: u64 = FILE_CEILING / 8 * 7;
const _: () = assert!(CORE_WARN < FILE_CEILING, "a warning above the wall would never fire");

/// Manifest sizes (§5.2), taken from the format authority rather than restated. They were literals
/// here until v3 moved the record width, at which point two of the three numbers this file computes
/// with would have gone stale silently.
pub const MANIFEST_HEADER_LEN: usize = obc_formats::obcs::HEADER_LEN;
pub const MANIFEST_SHARD_LEN: usize = obc_formats::obcs::SHARD_RECORD_LEN;
/// A set holds `1..=32` shards; readers reject `0` or more (§5.2/§5.3).
pub const MAX_SHARDS: usize = 32;
/// Largest card id the §5.2 filenames hold: `MS<id>S<kk>.OBM` is 8.3-safe only while `<id>` is at
/// most three digits, because the firmware's FAT layer creates **short names only**. A four-digit id
/// produces a nine-character basename, which a device silently mangles into a name the derived-name
/// lookup no longer finds.
pub const MAX_CARD_ID: u16 = 999;

/// §5.2's card-id range, checked where the id enters rather than where the string is built.
pub fn check_card_id(card_id: u16) -> Result<()> {
    if card_id > MAX_CARD_ID {
        return Err(Error::Input(format!(
            "card id {card_id} needs four digits; `MS<id>S<kk>.OBM` is 8.3-safe only up to {MAX_CARD_ID} (OBCA §5.2)"
        )));
    }
    Ok(())
}

/// One physical file of the set, planned before a byte is written.
pub struct ShardPlan {
    pub index: usize,
    pub role: BandRole,
    pub box_: AlignedBox,
    /// One entry per ladder level; a level this shard does not carry is [`LodPlan::empty`].
    pub lods: Vec<LodPlan>,
    /// Whether this shard carries the nav graph and the POIs — true for exactly one shard, the core.
    pub core: bool,
    /// Total bytes, computable before the write and re-checked after it (§5.7).
    pub bytes: u64,
    /// Filled by [`write`].
    pub sha256: [u8; 32],
}

impl ShardPlan {
    /// Layout cursor: where each region starts, given the fixed prefix. `u64` throughout, never
    /// `usize`: the crate's `--lib` target is wasm32, where a projection accumulated in a 32-bit
    /// `usize` wraps past 4 GiB and hands §5.7's ceiling a small number it happily accepts.
    ///
    /// Since v14 the cursor also carries §1.2's filler. Four of this file's five region starts are
    /// named by a **scaled** offset and so begin on a unit boundary — the style table behind the
    /// 49-byte header, the LOD table behind the style table, the first LOD's index behind the LOD
    /// table, and each section behind the last — so each is `align_up`'d here and the `0..U-1` bytes
    /// it rounds past are written `0xFF` by [`write`]. The per-LOD and per-section interiors carry
    /// their own gaps ([`LodPlan::region_bytes`], [`crate::poi::PoiSection::section_len`],
    /// [`crate::nav::NavProjection::bytes_at`]), and each of those regions **ends** on a unit
    /// boundary, which is what keeps this cursor aligned without a second rounding step per LOD.
    fn layout(&self, style_len: usize, poi_len: u64, nav: crate::nav::NavProjection) -> Result<Layout> {
        let style_end = STYLE_OFFSET + style_len as u64;
        let lod_table_offset = align_up(style_end);
        let table_end = lod_table_offset + (self.lods.len() * LOD_ENTRY_LEN) as u64;
        let payload_start = align_up(table_end);
        let mut cursor = payload_start;
        let mut lod_offsets = Vec::with_capacity(self.lods.len());
        for l in &self.lods {
            lod_offsets.push(cursor);
            cursor = cursor.checked_add(l.region_bytes()).ok_or_else(|| self.past_u64())?;
        }
        let poi_offset = cursor;
        let nav_offset = poi_offset.checked_add(poi_len).ok_or_else(|| self.past_u64())?;
        let total = nav_offset.checked_add(nav.bytes_at(nav_offset)).ok_or_else(|| self.past_u64())?;
        debug_assert_eq!(poi_offset, align_up(poi_offset), "every LOD region ends on a unit boundary");
        debug_assert_eq!(nav_offset, align_up(nav_offset), "the POI section ends on a unit boundary");
        Ok(Layout {
            lod_table_offset,
            style_gap: lod_table_offset - style_end,
            table_gap: payload_start - table_end,
            lod_offsets,
            poi_offset,
            nav_offset,
            total,
        })
    }

    fn past_u64(&self) -> Error {
        Error::Capacity(format!("shard {}'s layout does not fit a u64 of bytes (OBCA §5.7)", self.index))
    }

    /// A section base that does not fit the host's `usize` — 32-bit in the wasm32 build this engine
    /// ships in. Unreachable behind [`FILE_CEILING`]; an error rather than a cast so that it stays
    /// unreachable if the ceiling ever moves.
    fn past_usize(&self, what: &str, at: u64) -> Error {
        Error::Capacity(format!(
            "shard {}'s {what} section starts at byte {at}, past the {} bytes this host can address \
             (OBCA §5.7)",
            self.index,
            usize::MAX
        ))
    }
}

struct Layout {
    lod_table_offset: u64,
    /// §1.2 filler between the style table and the LOD table.
    style_gap: u64,
    /// …and between the LOD table and the first LOD's index.
    table_gap: u64,
    lod_offsets: Vec<u64>,
    poi_offset: u64,
    nav_offset: u64,
    total: u64,
}

/// Compute a shard's total size without writing it — §5.7's projection, applied to the assembler's
/// own output so an over-size file is refused rather than emitted.
pub fn projected_bytes(
    plan: &ShardPlan,
    style_len: usize,
    poi_len: u64,
    nav: crate::nav::NavProjection,
) -> Result<u64> {
    Ok(plan.layout(style_len, poi_len, nav)?.total)
}

/// The nav section's exact bytes in `plan`, for the assembly report. Kept beside
/// [`projected_bytes`] so reporting and the write use the same absolute-offset arithmetic.
pub fn projected_nav_bytes(
    plan: &ShardPlan,
    style_len: usize,
    poi_len: u64,
    nav: crate::nav::NavProjection,
) -> Result<u64> {
    let layout = plan.layout(style_len, poi_len, nav)?;
    Ok(nav.bytes_at(layout.nav_offset))
}

/// Write one shard: header, style table, LOD table, every LOD region, the POI section, the nav
/// section. Returns `(bytes, sha256)`.
///
/// Nothing is back-patched. Every offset in the header and the LOD table is known before the first
/// byte goes out, because the graft plan and both rebuilt sections were sized first — which is what
/// lets the output stream straight into a file (or a browser's download stream) rather than a buffer.
///
/// `cells` are the shard's own grafted cells; `nav_cells` are the `network` cells the §4.6 merge
/// read, in the order it read them. They are a second list because the merged graph holds its edge
/// records as *addresses* into those cells (§4.6.6) — the core's nav section is streamed out of them
/// here rather than out of a pool the merge would otherwise have had to keep.
///
/// `scratch` must be the store the §4.6 merge spilled into: since #1116 D4 the nav section's index,
/// chunks and pool plan live there too, and they stay valid until `MergedNav::release`.
// The argument list is one shard's whole input: the plan, the cells it grafts, the three rebuilt
// pieces every shard shares, and the sink. Bundling them into a struct would move the noise rather
// than remove it — the same call the packer's `serialize_lods_streaming` makes, for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn write(
    plan: &ShardPlan,
    cells: &[Cell<'_>],
    nav_cells: &[&Cell<'_>],
    styles: &[StyleRecord],
    marker_color: u16,
    poi: &PoiSection,
    nav: &MergedNav,
    profile_table: &[u8],
    scratch: &dyn ScratchStore,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    let style_bytes = pack_style_table(styles);
    // The empty pair a non-core shard carries (§5.1), built **once**: it is written at the end and
    // its length is needed at the start, and building it twice was one section's worth of allocation
    // per shard for no reason.
    let empty_poi = if plan.core { None } else { Some(crate::poi::empty_layout(plan.box_.ubox())) };
    let empty_nav = if plan.core { None } else { Some(MergedNav::empty(Default::default())) };
    let poi_bytes_len = empty_poi.as_ref().map_or_else(|| poi.section_len(), |p| p.section_len());
    let nav_projection = empty_nav.as_ref().map_or(nav, |n| n).projection(profile_table);
    let l = plan.layout(style_bytes.len(), poi_bytes_len, nav_projection)?;
    if l.total > FILE_CEILING {
        return Err(Error::Capacity(format!(
            "shard {} would be {} bytes, past the {FILE_CEILING}-byte interior `Offset Scale` {} covers \
             (OBCM §1.1) — reduce the coverage (OBCA §5.7)",
            plan.index,
            l.total,
            SCALE.log2()
        )));
    }
    // `OBCM_Spec.md` §1.1's one producer rule: **the scale MUST cover the file it writes**. The
    // ceiling above is now *derived from* this rule rather than independent of it, so the two can
    // no longer disagree — which is why this stays: it is the rule stated where the bytes are, and
    // it is the check that survives if the ceiling above is ever re-expressed. A reader that never
    // resolves the last section never sees a thing wrong, so the producer is the only party
    // positioned to notice.
    if !SCALE.covers(l.total) {
        return Err(Error::Capacity(format!(
            "shard {} would be {} bytes, past the interior `Offset Scale` {} addresses (OBCM §1.1)",
            plan.index,
            l.total,
            SCALE.log2()
        )));
    }

    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut out = |buf: &[u8]| -> Result<()> {
        hasher.update(buf);
        written += buf.len() as u64;
        sink(buf)
    };

    // 1. Header (bbox stored lat, lon, lat, lon — `OBCM_Spec.md` §1), then the §1.2 filler that
    //    carries the 49-byte header to the style table's unit boundary.
    out(&header_block(plan.box_, plan.lods.len(), marker_color, l.lod_table_offset, l.poi_offset, l.nav_offset)?)?;

    // 2. Style table (the skin, §4.7) and 3. the LOD table, each followed by the filler that lands
    //    the next scaled-offset-named structure on its boundary.
    out(&style_bytes)?;
    out(&FILLER_RUN[..l.style_gap as usize])?;
    let mut table = Vec::with_capacity(plan.lods.len() * LOD_ENTRY_LEN);
    for (p, &offset) in plan.lods.iter().zip(&l.lod_offsets) {
        push_lod_entry(&mut table, p.max_mpp, scaled(offset)?, p.node_count, p.chunk_size, p.chunk_count);
    }
    out(&table)?;
    out(&FILLER_RUN[..l.table_gap as usize])?;

    // 4. Each LOD region: fresh upper tree, relocated cell blocks, offset table, chunk bytes.
    for p in &plan.lods {
        graft::emit_lod(p, cells, &mut out)?;
    }

    // 5/6. The POI and nav sections — the core's rebuilt ones, or a legal empty pair (§5.1).
    //
    // Both section writers take a `usize` base. That is a 32-bit type in the wasm32 `--lib` build
    // this engine actually ships in, so these conversions are checked rather than cast: a layout
    // past `usize` would otherwise wrap and address a section that is not there. `FILE_CEILING`
    // keeps them unreachable today, but a ceiling is a policy and a cast is forever.
    let poi_base = usize::try_from(l.poi_offset).map_err(|_| plan.past_usize("POI", l.poi_offset))?;
    let nav_base = usize::try_from(l.nav_offset).map_err(|_| plan.past_usize("nav", l.nav_offset))?;
    out(&crate::poi::serialize(empty_poi.as_ref().unwrap_or(poi), poi_base)?)?;
    crate::nav::serialize(empty_nav.as_ref().unwrap_or(nav), profile_table, nav_base, nav_cells, scratch, &mut out)?;

    // §4.8.6: the write must land exactly where §5.7's projection said it would. A `debug_assert`
    // would leave a release build emitting a file whose recorded `Bytes` and header offsets are a
    // sentence about a different layout — and the manifest would be written over it.
    if written != l.total {
        return Err(Error::Verify(format!(
            "shard {} projected to {} bytes but wrote {written} — the §5.7 projection and the write disagree",
            plan.index, l.total
        )));
    }
    Ok((written, hasher.finalize().into()))
}

/// The 49-byte v14 OBCM header (`OBCM_Spec.md` §1), byte-for-byte the packer's `header_bytes`. Split
/// out because it is a **restatement** of `obc-pack`'s serializer, and `tests/pinning.rs` compares
/// the two outputs directly rather than trusting that two copies of a table stay in step.
///
/// The three offsets are given as **byte** offsets and scaled here, exactly as the packer's writer
/// takes them: the shard planner works in bytes throughout (§5.7's ceiling is a byte count) and this
/// is the one seam where they become units.
///
/// The terrain pair is `(0, 0)` — §1.3's unambiguous absence, and the right answer here for a reason
/// the packer does not have: a set's raster is its **own file** (`OBCA_Spec.md` §5.5, `MS<id>.OBD`),
/// so no OBCM shard of a set ever carries an embedded OBCT region.
pub fn header_bytes(
    box_: AlignedBox,
    lod_count: usize,
    marker_color: u16,
    lod_table_offset: u64,
    poi_offset: u64,
    nav_offset: u64,
) -> Result<Vec<u8>> {
    let (min_lon, min_lat, max_lon, max_lat) = box_.ubox();
    let mut head = Vec::with_capacity(HEADER_LEN);
    head.extend_from_slice(&MAGIC);
    head.push(VERSION);
    head.extend_from_slice(&(min_lat as i32).to_le_bytes());
    head.extend_from_slice(&(min_lon as i32).to_le_bytes());
    head.extend_from_slice(&(max_lat as i32).to_le_bytes());
    head.extend_from_slice(&(max_lon as i32).to_le_bytes());
    head.extend_from_slice(&scaled(STYLE_OFFSET)?.to_le_bytes());
    head.push(lod_count as u8);
    head.extend_from_slice(&scaled(lod_table_offset)?.to_le_bytes());
    head.extend_from_slice(&marker_color.to_le_bytes());
    head.extend_from_slice(&scaled(poi_offset)?.to_le_bytes());
    head.extend_from_slice(&scaled(nav_offset)?.to_le_bytes());
    head.push(SCALE.log2());
    head.extend_from_slice(&0u32.to_le_bytes()); // terrain offset — the set's raster is its own file
    head.extend_from_slice(&0u32.to_le_bytes()); // terrain length, `0` exactly when the offset is
    debug_assert_eq!(head.len(), HEADER_LEN);
    Ok(head)
}

/// The header plus the §1.2 filler that carries it to the style table's unit boundary.
fn header_block(
    box_: AlignedBox,
    lod_count: usize,
    marker_color: u16,
    lod_table_offset: u64,
    poi_offset: u64,
    nav_offset: u64,
) -> Result<Vec<u8>> {
    let mut out = header_bytes(box_, lod_count, marker_color, lod_table_offset, poi_offset, nav_offset)?;
    out.resize(STYLE_OFFSET as usize, FILLER);
    Ok(out)
}

/// Append one 18-byte LOD-table entry (`OBCM_Spec.md` §3), byte-for-byte the packer's
/// `push_lod_entry`: `Max Meters/Pixel` (`None` ⇒ `+inf`), index offset, node count, chunk capacity,
/// chunk count. Pinned against the packer alongside the header.
pub fn push_lod_entry(
    table: &mut Vec<u8>,
    max_mpp: Option<f64>,
    index_offset: u32,
    node_count: u32,
    chunk_size: usize,
    chunk_count: u32,
) {
    table.extend_from_slice(&max_mpp.map_or(f32::INFINITY, |v| v as f32).to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&node_count.to_le_bytes());
    table.extend_from_slice(&(chunk_size as u16).to_le_bytes());
    table.extend_from_slice(&chunk_count.to_le_bytes());
}

/// Byte offset of the header's `Style Offset` field (`OBCM_Spec.md` §1: magic 4, version 1, four
/// `int32` bbox fields — `4 + 1 + 16`).
pub const HEADER_STYLE_OFFSET_AT: usize = 21;

/// Resolve a map's `Style Offset` to a **byte** offset, through the file's own `Offset Scale`
/// (§1.1). `None` when the header is short, the scale byte is not one the format defines, or the
/// resolved byte does not fit this host's address space.
///
/// The scale is read out of the image rather than assumed to be [`SCALE`]: this is the one function
/// here that runs over bytes the engine did not write — `obc-bake`'s published thumbnails and the
/// builder's skin editor both hand it a file from somewhere else.
pub fn header_style_offset(map: &[u8]) -> Option<usize> {
    if map.len() < HEADER_LEN {
        return None;
    }
    let scale = OffsetScale::new(map[obc_formats::obcm::HEADER_OFFSET_SCALE_OFF]).ok()?;
    let units = u32::from_le_bytes(
        map[HEADER_STYLE_OFFSET_AT..HEADER_STYLE_OFFSET_AT + 4].try_into().expect("four bytes inside the header"),
    );
    usize::try_from(scale.offset(units).bytes()).ok()
}

/// Byte offset of the header's `Marker Color` field — the one other byte a skin owns.
pub const HEADER_MARKER_COLOR_AT: usize = 30;

/// Why a restamp could not happen. It carries the numbers rather than a sentence, because the two
/// callers word their failures for very different readers: `obc-bake` tells a maintainer to refresh
/// a fixture before a long bake, the skin editor tells a person in a browser tab why the picture
/// did not change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestampError {
    /// Fewer than [`HEADER_LEN`] bytes — not an OBCM image at all.
    ShorterThanHeader,
    /// The header's `Style Offset` points past the end.
    BadStyleOffset,
    /// `offset + 1 + count · record` overflows `usize`.
    TableOverflows,
    /// The declared table runs past the end of the image.
    TableTruncated,
    /// The image declares `count` styles but the skin resolved to only `resolved`.
    TooFewStyles { count: usize, resolved: usize },
    /// The `count`-style table in the image is not the `packed` bytes a fresh pack produces.
    LengthMismatch { count: usize, packed: usize },
    /// The image's style ids are not the skin's — the image belongs to another schema revision.
    IdMismatch { have: Vec<u8>, want: Vec<u8> },
    /// The skin moves style `id` across the rain band boundary
    /// ([`obc_map_scene::RAIN_BELOW_Z`]) relative to the image it stamps onto — which would let a
    /// presentation-only restamp pull a road under the precipitation raster (or lift a ground
    /// fill above it), breaking the locked "roads above rain" paint order with no repack.
    RainBandMoved { id: u8, from: i8, to: i8 },
}

/// Stamp a resolved skin onto an OBCM image **in place**: its style table and the header's marker
/// colour, and nothing else. That is the whole of what applying a skin means (`OBCA_Spec.md` §5) —
/// ≈ 2 KB and one `u16` — which is why a skin invalidates no cell and a preview needs no re-pack.
///
/// Only the styles the image actually carries are stamped. A schema that has grown feature types
/// since the image was cut keeps its trailing ones: style ids are assigned in schema document order,
/// so an appended type takes the next free id and leaves every id in the image meaning what it
/// meant — and a type the image has no geometry for cannot change the picture anyway. The image's
/// table must therefore be a **prefix** of the skin's assignment; ids that disagree are refused,
/// because there the bytes mean something the schema no longer says.
///
/// Two callers share this: `obc-bake`'s published thumbnails and the builder's live skin editor.
/// They used to hold a byte-for-byte copy each, down to the three header offsets.
pub fn restamp_style_table(
    map: &mut [u8],
    styles: &[StyleRecord],
    marker_color: u16,
) -> core::result::Result<(), RestampError> {
    if map.len() < HEADER_LEN {
        return Err(RestampError::ShorterThanHeader);
    }
    let style_offset = header_style_offset(map).ok_or(RestampError::BadStyleOffset)?;
    let count = *map.get(style_offset).ok_or(RestampError::BadStyleOffset)? as usize;
    let end = style_offset.checked_add(1 + count * STYLE_RECORD_LEN).ok_or(RestampError::TableOverflows)?;
    let slot = map.get_mut(style_offset..end).ok_or(RestampError::TableTruncated)?;
    if styles.len() < count {
        return Err(RestampError::TooFewStyles { count, resolved: styles.len() });
    }
    let stamped = &styles[..count];
    let packed = pack_style_table(stamped);
    if slot.len() != packed.len() {
        return Err(RestampError::LengthMismatch { count, packed: packed.len() });
    }
    let have: Vec<u8> = slot[1..].chunks_exact(STYLE_RECORD_LEN).map(|record| record[0]).collect();
    let want: Vec<u8> = stamped.iter().map(|style| style.id).collect();
    if have != want {
        return Err(RestampError::IdMismatch { have, want });
    }
    // The rain band boundary (WX10): a restamp may restyle freely on either side of
    // `RAIN_BELOW_Z`, but may not carry a style across it — the renderer draws precipitation at
    // that z split, and "roads above rain" is locked UX a skin must not be able to undo.
    for (record, style) in slot[1..].chunks_exact(STYLE_RECORD_LEN).zip(stamped) {
        let from = record[1] as i8;
        let to = style.z_index;
        if (from >= obc_map_scene::RAIN_BELOW_Z) != (to >= obc_map_scene::RAIN_BELOW_Z) {
            return Err(RestampError::RainBandMoved { id: style.id, from, to });
        }
    }
    slot.copy_from_slice(&packed);
    map[HEADER_MARKER_COLOR_AT..HEADER_MARKER_COLOR_AT + 2].copy_from_slice(&marker_color.to_le_bytes());
    Ok(())
}

/// The style table (`OBCM_Spec.md` §2): `Count` then one 8-byte record per style, id ascending.
pub fn pack_style_table(styles: &[StyleRecord]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    out.push(styles.len() as u8);
    for s in styles {
        let mut flags = (s.priority.clamp(1, 4) - 1) & STYLE_PRIORITY_MASK;
        if s.dashed {
            flags |= STYLE_DASHED_BIT;
        }
        if s.color2.is_some() {
            flags |= STYLE_HAS_COLOR2_BIT;
        }
        if s.fixed_width {
            flags |= STYLE_FIXED_WIDTH_BIT;
        }
        if s.terrain_layer {
            flags |= STYLE_TERRAIN_LAYER_BIT;
        }
        out.push(s.id);
        out.push(s.z_index as u8);
        out.extend_from_slice(&s.color.to_le_bytes());
        out.push(s.weight);
        out.push(flags);
        out.extend_from_slice(&s.color2.unwrap_or(0).to_le_bytes());
    }
    out
}

/// The set's terrain shard as the manifest records it (§5.2's `terrain` role) — everything the
/// record needs and nothing about how it was built.
#[derive(Clone, Copy, Debug)]
pub struct TerrainRecord {
    pub bytes: u64,
    pub sha256: [u8; 32],
}

/// The OBCS set manifest (§5.2): `72 + 64 × Shard Count` bytes, fixed-layout and little-endian, so a
/// device parses it with no allocation.
///
/// `terrain` is the optional `Role == 3` record, and it is written **last** — the invariant that
/// lets every reader treat the leading records as the OBCM shards without a role filter.
///
/// The manifest is **unbound**: every member `ObjectId` is `0`, because an assembly has no store to
/// have been given ids by. Binding is the uploading client's step (§5.2).
pub fn manifest(
    shards: &[ShardPlan],
    terrain: Option<TerrainRecord>,
    assembly: AlignedBox,
    schema_revision: u32,
    name: &str,
) -> Result<Vec<u8>> {
    let records = shards.len() + terrain.is_some() as usize;
    if shards.is_empty() || records > MAX_SHARDS {
        return Err(Error::Capacity(format!(
            "a set holds 1..={MAX_SHARDS} records; this one needs {records} ({} shard(s){})",
            shards.len(),
            if terrain.is_some() { " plus terrain" } else { "" }
        )));
    }
    let core = shards
        .iter()
        .position(|s| s.core)
        .ok_or_else(|| Error::Verify("the set has no core shard (OBCA §5.3)".into()))?;
    // §5.3: past the single-file fast path a set MUST carry at least one shard of each non-core
    // role. A reader has no schema to consult, so "the schema names no coarse band" is not an
    // excuse it can hear — it sees a manifest missing `Role == 2` and refuses to mount, and every
    // shard the host wrote is dead weight on the card. Catch it here, where the set is still ours.
    if shards.len() > 1 {
        for (role, what) in [(BandRole::Geometry, "geometry"), (BandRole::Coarse, "coarse")] {
            if !shards.iter().any(|s| !s.core && s.role == role) {
                return Err(Error::Verify(format!(
                    "a multi-shard set has no {what} shard, so no reader will mount it (OBCA §5.3). The schema's band \
                     table must name a band with role {what:?} for selections past the single-file threshold."
                )));
            }
        }
    }

    // `Set Id` is a content identity: two assemblies of the same cells with the same skin produce
    // the same id, which is what lets an upload notice the set is already present. Terrain is in the
    // chain — a set that gained or re-baked its raster is a different set on the card.
    let mut id_hash = Sha256::new();
    for s in shards {
        id_hash.update(s.sha256);
    }
    if let Some(t) = &terrain {
        id_hash.update(t.sha256);
    }
    let set_id: [u8; 32] = id_hash.finalize().into();

    let (min_lon, min_lat, max_lon, max_lat) = assembly.ubox();
    let mut out = Vec::with_capacity(MANIFEST_HEADER_LEN + records * MANIFEST_SHARD_LEN);
    out.extend_from_slice(b"OBCS");
    out.push(obc_formats::obcs::VERSION); // manifest version
    out.push(VERSION); // the OBCM version of every OBCM shard
    out.push(records as u8);
    out.push(core as u8);
    out.extend_from_slice(&schema_revision.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags, reserved
    out.extend_from_slice(&(min_lat as i32).to_le_bytes());
    out.extend_from_slice(&(min_lon as i32).to_le_bytes());
    out.extend_from_slice(&(max_lat as i32).to_le_bytes());
    out.extend_from_slice(&(max_lon as i32).to_le_bytes());
    out.extend_from_slice(&set_id[..16]);
    out.extend_from_slice(&fold_name(name));
    debug_assert_eq!(out.len(), MANIFEST_HEADER_LEN);

    // §5.2's `Bytes` is a `uint32` of **bytes**, not units — so unlike every offset in an OBCM file
    // it did *not* widen in v14, and it is the narrowest wall a set's member has to clear. A bare
    // `as u32` here does not refuse an over-size shard, it *misreports* one: a 5 GiB file records as
    // ≈0.7 GiB, and every consumer that trusts the manifest to size a download then trusts a number
    // the file contradicts. `FILE_CEILING` makes it unreachable; this makes it unspellable.
    let push_record = |out: &mut Vec<u8>, role: u8, box_: AlignedBox, bytes: u64, sha256: &[u8; 32]| -> Result<()> {
        let (min_lon, min_lat, max_lon, max_lat) = box_.ubox();
        out.push(role);
        out.push(0); // flags
        out.extend_from_slice(&[0, 0]); // reserved
        out.extend_from_slice(&(min_lat as i32).to_le_bytes());
        out.extend_from_slice(&(min_lon as i32).to_le_bytes());
        out.extend_from_slice(&(max_lat as i32).to_le_bytes());
        out.extend_from_slice(&(max_lon as i32).to_le_bytes());
        let recorded = u32::try_from(bytes).map_err(|_| {
            Error::Capacity(format!(
                "a shard of {bytes} bytes cannot be recorded in OBCA §5.2's `uint32` Bytes field — \
                 the manifest would state {} instead",
                bytes as u32
            ))
        })?;
        out.extend_from_slice(&recorded.to_le_bytes());
        out.extend_from_slice(sha256);
        // v3's member `ObjectId`, written **unbound** (§5.2). An assembler cannot do better: the id
        // is minted by the store on the card the set is eventually sent to, and this set has not met
        // one — it may be sent to several, or to none. The client that uploads it patches each id in
        // with `obc_formats::obcs::bind_member` as the members commit, before the manifest itself is
        // committed, which §5.4 already made the last write of a set.
        out.extend_from_slice(&obc_formats::obcs::MEMBER_ID_NONE.to_le_bytes());
        Ok(())
    };
    for s in shards {
        push_record(&mut out, s.role.wire(), s.box_, s.bytes, &s.sha256)?;
    }
    // §5.2: the terrain record last, spanning the assembly bbox. Last is not a convention — a
    // reader takes the leading records as the OBCM shards and their index as the `S<kk>` in a
    // derived filename, so a raster anywhere else would rename every shard after it.
    if let Some(t) = &terrain {
        push_record(&mut out, obc_formats::obcs::Role::Terrain.id(), assembly, t.bytes, &t.sha256)?;
    }
    debug_assert_eq!(out.len(), MANIFEST_HEADER_LEN + records * MANIFEST_SHARD_LEN);
    // The set is about to be written; the format authority is what decides whether it is one
    // (§5.3). Parsing our own bytes back is cheap and catches a role, an ordering or a bbox the
    // planner got wrong here rather than on a card.
    obc_formats::obcs::parse(&out)
        .map_err(|e| Error::Verify(format!("the manifest this assembly wrote does not validate: {e:?} (OBCA §5.3)")))?;
    Ok(out)
}

/// A 24-byte display name: printable ASCII, `0xFF`-padded — the `OBCM_Spec.md` §7.3 name convention.
fn fold_name(name: &str) -> [u8; 24] {
    let mut out = [0xFFu8; 24];
    for (slot, b) in out.iter_mut().zip(name.bytes()) {
        *slot = if (0x20..=0x7E).contains(&b) { b } else { b'?' };
    }
    out
}

/// The filename a shard lives under at the card root (§5.2): `MS<id>S<kk>.OBM`. Derived, never
/// stored — a stored name is a second source of truth that can disagree with the directory.
pub fn shard_filename(card_id: u16, index: usize) -> String {
    format!("MS{card_id}S{index:02}.OBM")
}

/// The manifest's own filename: `MS<id>.OBS` (§5.2).
pub fn manifest_filename(card_id: u16) -> String {
    format!("MS{card_id}.OBS")
}

/// The terrain shard's filename: `MS<id>.OBD` (§5.2) — no `S<kk>`, because there is at most one.
/// It is also the `OBCT_Spec.md` §4.6 sidecar of `MS<id>.OBS`, so a host that resolves terrain by
/// sidecar and one that reads the manifest role open the same file.
pub fn terrain_filename(card_id: u16) -> String {
    format!("MS{card_id}.OBD")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::MemoryScratch;

    fn plan(index: usize, role: BandRole, core: bool) -> ShardPlan {
        ShardPlan {
            index,
            role,
            box_: AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 },
            lods: Vec::new(),
            core,
            bytes: 1234,
            sha256: [index as u8; 32],
        }
    }

    #[test]
    fn manifest_layout_is_the_spec_table() {
        let shards =
            vec![plan(0, BandRole::Core, true), plan(1, BandRole::Coarse, false), plan(2, BandRole::Geometry, false)];
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 };
        let m = manifest(&shards, None, bx, 7, "Switzerland").expect("manifest");
        assert_eq!(m.len(), MANIFEST_HEADER_LEN + 3 * MANIFEST_SHARD_LEN);
        assert_eq!(&m[0..4], b"OBCS");
        assert_eq!(m[4], 3, "manifest v3 — member ObjectIds are a hard cut");
        assert_eq!(m[5], VERSION);
        assert_eq!(m[6], 3, "shard count");
        assert_eq!(m[7], 0, "core shard index");
        assert_eq!(u32::from_le_bytes(m[8..12].try_into().unwrap()), 7, "schema revision");
        assert_eq!(u32::from_le_bytes(m[12..16].try_into().unwrap()), 0, "flags reserved");
        assert_eq!(i32::from_le_bytes(m[16..20].try_into().unwrap()), 47_185_920, "min lat first");
        assert_eq!(&m[48..59], b"Switzerland");
        assert_eq!(m[59], 0xFF, "the name pads with 0xFF");
        // Shard records: role byte, then the bbox in the header's own order, size, digest.
        assert_eq!(m[72], 0, "core role");
        assert_eq!(m[72 + MANIFEST_SHARD_LEN], 2, "coarse role");
        assert_eq!(u32::from_le_bytes(m[92..96].try_into().unwrap()), 1234);
        assert_eq!(m[96], 0, "the first shard's digest");
        // v3: every member id is `0` — an assembly has no store to have been given ids by, so the
        // manifest an assembler writes is unbound and the uploading client binds it.
        let parsed = obc_formats::obcs::parse(&m).expect("the assembler's own manifest parses");
        assert!(!parsed.is_bound(), "an assembled set is unbound (OBCA §5.2)");
        for record in 0..3 {
            let at = MANIFEST_HEADER_LEN + record * MANIFEST_SHARD_LEN + obc_formats::obcs::MEMBER_ID_OFFSET;
            assert_eq!(&m[at..at + 8], &[0u8; 8], "record {record}'s member id");
        }
    }

    #[test]
    fn a_set_with_no_core_is_refused() {
        let shards = vec![plan(0, BandRole::Geometry, false)];
        let bx = AlignedBox { min_lat: 0, min_lon: 0, span_log2: 20 };
        assert!(manifest(&shards, None, bx, 1, "x").is_err());
    }

    /// §5.3: a reader has no schema, so a multi-shard set missing a whole role does not mount. The
    /// host must not write one — a schema whose band table names no `coarse` band would otherwise
    /// produce a manifest that looks valid and is not.
    #[test]
    fn a_multi_shard_set_missing_a_role_is_refused() {
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 };
        let no_coarse = vec![plan(0, BandRole::Core, true), plan(1, BandRole::Geometry, false)];
        let err = format!("{}", manifest(&no_coarse, None, bx, 1, "x").expect_err("no coarse shard"));
        assert!(err.contains("no coarse shard"), "got: {err}");
        let no_geometry = vec![plan(0, BandRole::Core, true), plan(1, BandRole::Coarse, false)];
        let err = format!("{}", manifest(&no_geometry, None, bx, 1, "x").expect_err("no geometry shard"));
        assert!(err.contains("no geometry shard"), "got: {err}");
        // …and the single-file fast path is exempt, because §5.5 says the one shard is the core.
        manifest(&[plan(0, BandRole::Core, true)], None, bx, 1, "x").expect("a set of one is legal");
    }

    /// §5.2's `terrain` role: the last record, spanning the assembly, counted by `Shard Count`,
    /// and in the `Set Id` chain — so a set that gains a raster is a different set on the card.
    #[test]
    fn the_terrain_record_is_last_and_changes_the_set_id() {
        let bx = AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 };
        let shards = vec![plan(0, BandRole::Core, true)];
        let terrain = TerrainRecord { bytes: 6_192, sha256: [0x77; 32] };
        let plain = manifest(&shards, None, bx, 7, "Grimsel").expect("no raster is a complete map");
        let with = manifest(&shards, Some(terrain), bx, 7, "Grimsel").expect("core + terrain");

        assert_eq!(plain.len(), MANIFEST_HEADER_LEN + MANIFEST_SHARD_LEN);
        assert_eq!(with.len(), MANIFEST_HEADER_LEN + 2 * MANIFEST_SHARD_LEN);
        assert_eq!(with[6], 2, "Shard Count counts every record");
        assert_eq!(with[7], 0, "…and the core is still record 0");
        let base = MANIFEST_HEADER_LEN + MANIFEST_SHARD_LEN;
        assert_eq!(with[base], obc_formats::obcs::Role::Terrain.id());
        assert_eq!(i32::from_le_bytes(with[base + 4..base + 8].try_into().unwrap()) as i64, bx.min_lat);
        assert_eq!(u32::from_le_bytes(with[base + 20..base + 24].try_into().unwrap()), 6_192);
        assert_eq!(&with[base + 24..base + 56], &[0x77; 32]);
        assert_ne!(&with[32..48], &plain[32..48], "the Set Id covers the raster");

        // The parsed manifest keeps the raster out of the OBCM shard list, which is what every
        // mount path iterates.
        let parsed = obc_formats::obcs::parse(&with).expect("valid");
        assert_eq!(parsed.shard_count(), 1);
        assert_eq!(parsed.record_count(), 2);
        assert_eq!(parsed.terrain().map(|t| t.bytes), Some(6_192));
        assert!(parsed.is_single_file(), "terrain is its own file, so this is still §5.5's fast path");
    }

    /// §5.7's ceiling is only a ceiling if the projection can exceed it. The layout is `u64` for
    /// that reason: in the wasm32 `--lib` build a `usize` cursor wraps at 4 GiB, so an over-size
    /// selection would project *small*, take the single-file path, and stream a file whose header
    /// offsets belong to a layout that does not exist.
    #[test]
    fn a_layout_past_the_ceiling_is_refused_rather_than_wrapped() {
        let mut p = plan(0, BandRole::Geometry, false);
        // Two LODs of 40 GB each: past v14's 64 GiB interior, where v13's test only had to clear
        // 4 GiB. Raising these numbers with the ceiling is the point of the test.
        p.lods = vec![LodPlan { node_count: 1, chunk_bytes: 40_000_000_000, ..LodPlan::empty(0, None, 4096) }; 2];
        // The empty pair this non-core shard carries, which is what `write` will project against.
        let poi = crate::poi::empty_layout(p.box_.ubox());
        let nav = MergedNav::empty(Default::default());
        let poi_len: u64 = poi.section_len();
        let nav_projection = nav.projection(&[]);
        let nav_len = nav_projection.bytes_at(0);

        let projected: u64 = projected_bytes(&p, 0, poi_len, nav_projection).expect("a u64 holds it");
        // The fixed prefix is v14's: the 49-byte header rounded to the style table's boundary (an
        // empty style table here), that rounded again past two 18-byte LOD entries — and each LOD
        // region carries its own §1.2 gap between its four-byte index, its four-byte offset table
        // and its chunks.
        let prefix = align_up(align_up(STYLE_OFFSET) + 2 * LOD_ENTRY_LEN as u64);
        assert_eq!(prefix, 112);
        assert_eq!(projected, prefix + 2 * (40_000_000_000 + 4 + 4 + 8) + poi_len + nav_len);
        assert!(projected > FILE_CEILING);

        let mut sink = |_: &[u8]| -> Result<()> { panic!("a refused shard writes no bytes") };
        let err = write(&p, &[], &[], &[], 0, &poi, &nav, &[], &MemoryScratch::new(), &mut sink)
            .expect_err("past the ceiling");
        assert!(matches!(err, Error::Capacity(_)), "got: {err}");
        assert!(format!("{err}").contains("past the"), "got: {err}");
    }

    #[test]
    fn filenames_are_eight_three_safe() {
        assert_eq!(terrain_filename(7), "MS7.OBD");
        assert_eq!(terrain_filename(999), "MS999.OBD");
        assert_eq!(shard_filename(7, 0), "MS7S00.OBM");
        assert_eq!(shard_filename(MAX_CARD_ID, 31), "MS999S31.OBM");
        assert_eq!(manifest_filename(7), "MS7.OBS");
        assert!(shard_filename(MAX_CARD_ID, 31).split('.').next().unwrap().len() <= 8, "8.3 short name");
        // The bound itself, checked where the id enters: one more digit breaks the short name.
        check_card_id(MAX_CARD_ID).expect("three digits fit");
        let err = format!("{}", check_card_id(MAX_CARD_ID + 1).expect_err("four digits do not"));
        assert!(err.contains("8.3-safe"), "got: {err}");
        assert_eq!(shard_filename(1000, 0).split('.').next().unwrap().len(), 9, "…which is what it would look like");
    }

    #[test]
    fn style_records_are_eight_bytes_with_the_flag_bits() {
        let s = StyleRecord {
            id: 3,
            z_index: -2,
            color: 0x1234,
            weight: 5,
            priority: 2,
            dashed: true,
            color2: Some(0xBEEF),
            fixed_width: false,
            terrain_layer: false,
        };
        let bytes = pack_style_table(&[s]);
        assert_eq!(bytes.len(), 1 + STYLE_RECORD_LEN);
        assert_eq!(bytes[0], 1);
        assert_eq!(bytes[1], 3);
        assert_eq!(bytes[2] as i8, -2);
        assert_eq!(u16::from_le_bytes([bytes[3], bytes[4]]), 0x1234);
        assert_eq!(bytes[5], 5);
        assert_eq!(bytes[6], 1 | STYLE_DASHED_BIT | STYLE_HAS_COLOR2_BIT, "priority 2 ⇒ bits 0-1 = 1");
        assert_eq!(u16::from_le_bytes([bytes[7], bytes[8]]), 0xBEEF);

        // #1095: a stamped skin carries the two new bits through to the record it writes, so a
        // restyled cell tree keeps a contour hairline and terrain-layer-tagged instead of quietly
        // clearing them back to a ramped road (bits 6-7 stay reserved and written 0).
        let terrain = StyleRecord { fixed_width: true, terrain_layer: true, ..s };
        let bytes = pack_style_table(&[terrain]);
        assert_eq!(
            bytes[6],
            1 | STYLE_DASHED_BIT | STYLE_HAS_COLOR2_BIT | STYLE_FIXED_WIDTH_BIT | STYLE_TERRAIN_LAYER_BIT
        );
        assert_eq!(bytes[6] & obc_formats::obcm::STYLE_RESERVED_MASK, 0, "bits 6-7 stay 0");
    }
}
