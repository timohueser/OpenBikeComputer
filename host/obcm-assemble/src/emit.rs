//! Laying out one assembled map and writing its bytes: header, style table, LOD table, every LOD
//! region, the POI and nav sections, and — since OBCM v14 — the spliced terrain region
//! ([`OBCM_Spec.md`](../../../specs/OBCM_Spec.md) §1.3).
//!
//! **An assembly is one file.** It was not always: one logical map used to be a small OBCS manifest
//! plus 1..N physical OBCM shards, because two independent 4 GiB ceilings made that necessary —
//! FAT32's per-file cap and OBCM's own `uint32` offsets. Both are gone. The flat store replaced
//! FAT; v14's scaled offsets put the format's interior at 64 GiB. A third wall stood behind them,
//! the read seam's `u32` offsets landing on the same 4 GiB by coincidence rather than by
//! inheritance, and FS7.5-seam removed that one too. A fourth — §5.2's `Bytes`, a `uint32` of bytes
//! in a manifest record — outlived all three and was the last thing splitting a country-scale
//! selection; it died with the manifest that carried it, which is this slice.
//!
//! So there is **one wall left and one place that applies it**: [`FILE_CEILING`], through
//! [`fits_ceiling`]. There is no fast path and no split path to disagree about which wall is which,
//! because there is one path.
//!
//! Nothing here is back-patched. Every offset in the header and the LOD table is known before the
//! first byte goes out, because the graft plan, both rebuilt sections and the raster were all sized
//! first — which is what lets an assembly stream straight into a file, or into a browser's download,
//! rather than into a buffer the size of the map.

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
use crate::schema::StyleRecord;
use crate::scratch::ScratchStore;
use crate::{Error, Result};

/// The hard per-file ceiling: **the smaller of the two walls a written file has to clear.**
///
/// 1. **The format wall** — `OBCM_Spec.md` §1.1's addressable interior at this engine's [`SCALE`],
///    `2^32 units × U`, which at `U = 16` is `2^36` B = 64 GiB. Derived rather than written down,
///    because §1.1 states the producer rule in exactly these terms and [`OffsetScale::covers`] is
///    that sentence as a predicate.
/// 2. **The readable wall** — how far [`obc_formats::io::ByteSource`], the tree's one read
///    interface, can address. It was `u32::MAX`, which is why this had to be a `min` at all: v14
///    raised the format wall sixteen-fold and left the seam at 4 GiB, so a file between the two was
///    one this pipeline would lay out and nothing in this tree could open.
///
/// **FS7.5-seam widened the seam to `u64`, so the readable wall stopped binding and the format wall
/// took over at 64 GiB.** Every implementor and every cache behind the seam counts file offsets in
/// 64 bits now — including the device's, which is the point: the wall this constant expresses has
/// to be one the *reader on the card* can clear, not merely one a 64-bit host can.
///
/// The `min` stays as structure rather than collapsing to the format wall, because two walls is the
/// permanent shape of this: a written file must clear whatever the format can express **and**
/// whatever a reader can reach, and the day either moves this constant follows without anyone
/// re-deriving it. §8's edge pool was already built for the far wall —
/// `NAV_EDGE_MAX_CHUNKS × NAV_CHUNK_SIZE == 1 << 36` is pinned in `obc-formats` as "the pool
/// reaches the interior".
///
/// **There is no third wall any more.** A file this engine wrote used to carry an OBCS §5.2 record
/// whose `Bytes` was a `uint32` of bytes rather than units, and that record — not the format and
/// not the seam — was what held a written file to 4 GiB − 1 long after the other two had moved. The
/// manifest is deleted, so what a producer may write and what a reader may open are the same number
/// again, and it is this one.
pub const FILE_CEILING: u64 = {
    let format = (1u64 << 32) * SCALE.unit();
    let readable = READABLE_CEILING;
    if format < readable {
        format
    } else {
        readable
    }
};

/// How far a byte offset handed to [`obc_formats::io::ByteSource::read_at`] can reach: the whole
/// `u64`, since FS7.5-seam. Named rather than written as `u64::MAX` inline so [`FILE_CEILING`]'s
/// `min` keeps saying *which* wall each side is.
const READABLE_CEILING: u64 = u64::MAX;
const _: () = assert!(FILE_CEILING == 1u64 << 36, "at U = 16 the format's interior is 64 GiB, and it is what binds");
const _: () = assert!(SCALE.covers(FILE_CEILING), "and the scale still covers whatever the min lands on");

/// The one remedy for an over-size map, now that a map is one file: there is nothing left to move
/// somewhere else, so the only lever is how much ground the selection covers.
///
/// It used to be the *core's* remedy, distinguished from a geometry shard's ("lower the target
/// shard size") and a raster's (neither — it was one file per set). Those two remedies died with
/// the files they named. One file has one remedy, and a caller that had to pick the right one for
/// the reader can no longer pick wrong.
pub const SIZE_REMEDY: &str = "reduce the coverage (OBCA §4.8)";

/// Does a file of `bytes` fit the one wall a written map has to clear?
///
/// **This is the only place that comparison exists**, and every site that needs it asks here rather
/// than open-coding it. That is structural, not stylistic: the `single_file` exemption FS7.5-seam's
/// review caught was one call site quietly using a larger ceiling, and it survived review because an
/// open-coded `<= CEILING` reads correct whichever constant it names. Routing every gate through the
/// refusal makes them impossible to disagree.
///
/// The landscape this guards is much smaller than it was. There used to be two ceilings, two paths
/// (§5.5's single-file fast path and the role-partitioned split) and three remedies, and the bug
/// class was a path taking the wrong ceiling. Now there is one ceiling, one path and one remedy — so
/// the refusal cannot be *routed* wrongly, only written twice, and `the_only_wall_is_the_formats`
/// pins the number while this function keeps the site singular.
///
/// `what` names the file for the message, because "the map" and "the terrain region it splices" are
/// different things to refuse even though they answer to the same wall.
pub fn fits_ceiling(bytes: u64, what: &str) -> Result<()> {
    if bytes > FILE_CEILING {
        return Err(Error::Capacity(format!(
            "{what} projects to {bytes} bytes, past the {FILE_CEILING}-byte interior an `Offset Scale` of {} \
             addresses (OBCM §1.1) — {SIZE_REMEDY}.",
            SCALE.log2()
        )));
    }
    Ok(())
}

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

/// Where a producer SHOULD warn (OBCA §5.7): seven eighths of the wall, i.e. "you are close".
///
/// §5.7 wrote it as "≈ 3.5 GiB" against a `4 GiB − 1 B` ceiling. It is written here as the
/// **proportion** rather than the number, so it keeps meaning "close" wherever [`FILE_CEILING`]
/// lands — it followed the ceiling up through FS7.5-seam and again when the manifest's wall died,
/// without anyone re-deriving it.
///
/// **It no longer fires for anything a rider can select, and that should be said plainly.** At the
/// current ceiling it sits at ≈56 GiB, and the largest selection v1 contemplates — DACH — is ≈9 GiB.
/// So this is a tripwire on the *format's* limit, not a usable size signal, and the thing a rider
/// actually runs out of is card space, which this engine cannot see: §5.7 puts that projection on
/// the catalog consumer, before the download, precisely because by the time the assembler holds the
/// cells the download it should have prevented has already happened. The builder's own size meter
/// against the card's free space is the live signal; this is the backstop that says a map is
/// approaching the number the *file format* cannot express.
///
/// It stays rather than being deleted because it costs one comparison and the condition is real,
/// just distant — and because a proportion that tracks the ceiling is exactly what does not go
/// stale the next time the ceiling moves.
pub const SIZE_WARN: u64 = FILE_CEILING / 8 * 7;
const _: () = assert!(SIZE_WARN < FILE_CEILING, "a warning above the wall would never fire");

/// One assembled map, laid out before a byte is written.
pub struct MapPlan {
    pub box_: AlignedBox,
    /// One entry per ladder level — a map carries the full ladder (§3.1).
    pub lods: Vec<LodPlan>,
    /// The spliced §1.3 terrain region's exact byte length, or `0` for a map with no elevation.
    /// Known before the layout because [`crate::TerrainPlan::projected_bytes`] computes it from the
    /// rectangle and the present-cell count alone — which is what lets the header state the region's
    /// offset without anything being back-patched.
    pub terrain_bytes: u64,
    /// Total bytes, computable before the write and re-checked after it (§5.7).
    pub bytes: u64,
    /// Filled by [`write`].
    pub sha256: [u8; 32],
}

impl MapPlan {
    /// Layout cursor: where each region starts, given the fixed prefix. `u64` throughout, never
    /// `usize`: the crate's `--lib` target is wasm32, where a projection accumulated in a 32-bit
    /// `usize` wraps past 4 GiB and hands §5.7's ceiling a small number it happily accepts.
    ///
    /// Since v14 the cursor also carries §1.2's filler. Five of this file's six region starts are
    /// named by a **scaled** offset and so begin on a unit boundary — the style table behind the
    /// 49-byte header, the LOD table behind the style table, the first LOD's index behind the LOD
    /// table, each section behind the last, and the terrain region behind the nav section — so each
    /// is `align_up`'d here and the `0..U-1` bytes it rounds past are written `0xFF` by [`write`].
    /// The per-LOD and per-section interiors carry their own gaps ([`LodPlan::region_bytes`],
    /// [`crate::poi::PoiSection::section_len`], [`crate::nav::NavProjection::bytes_at`]), and each of
    /// those regions **ends** on a unit boundary, which is what keeps this cursor aligned without a
    /// second rounding step per LOD.
    ///
    /// The terrain region is the one exception to "regions end on a boundary", and §1.3 says so: an
    /// OBCT container is whatever length the raster makes it, `Terrain Length` counts **units**, and
    /// the difference is filler at the file's tail. So the region is rounded up here and the file
    /// ends on a unit boundary like everything else.
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
        let nav_end = nav_offset.checked_add(nav.bytes_at(nav_offset)).ok_or_else(|| self.past_u64())?;
        debug_assert_eq!(poi_offset, align_up(poi_offset), "every LOD region ends on a unit boundary");
        debug_assert_eq!(nav_offset, align_up(nav_offset), "the POI section ends on a unit boundary");

        // §1.3: terrain sits last, precisely so that splicing it moves no other offset. A map with
        // no raster ends at the nav section and writes `(0, 0)` — the header pair that means "this
        // map carries no elevation", which is unambiguous because byte 0 is the header itself.
        let (terrain_offset, terrain_len, nav_gap, terrain_gap, total) = if self.terrain_bytes == 0 {
            (0, 0, 0, 0, nav_end)
        } else {
            let at = align_up(nav_end);
            let end = at.checked_add(self.terrain_bytes).ok_or_else(|| self.past_u64())?;
            let total = align_up(end);
            (at, total - at, at - nav_end, total - end, total)
        };
        Ok(Layout {
            lod_table_offset,
            style_gap: lod_table_offset - style_end,
            table_gap: payload_start - table_end,
            lod_offsets,
            poi_offset,
            nav_offset,
            nav_gap,
            terrain_offset,
            terrain_len,
            terrain_gap,
            total,
        })
    }

    fn past_u64(&self) -> Error {
        Error::Capacity("the map's layout does not fit a u64 of bytes (OBCA §5.7)".into())
    }

    /// A section base that does not fit the host's `usize` — 32-bit in the wasm32 build this engine
    /// ships in. Unreachable behind [`FILE_CEILING`]; an error rather than a cast so that it stays
    /// unreachable if the ceiling ever moves.
    fn past_usize(&self, what: &str, at: u64) -> Error {
        Error::Capacity(format!(
            "the map's {what} section starts at byte {at}, past the {} bytes this host can address (OBCA §5.7)",
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
    /// §1.2 filler between the end of the nav section and the terrain region — `0` for a map with
    /// no raster, whose nav section is the last thing in the file.
    nav_gap: u64,
    /// Byte offset of the §1.3 terrain region, or `0` for a map with no elevation.
    terrain_offset: u64,
    /// The region's length **including** the filler `Terrain Length`'s unit count rounds up to, so
    /// that the header's pair is `(offset, len)` in bytes and both scale by the same rule. `0`
    /// exactly when `terrain_offset` is — §1.3 makes a reader refuse a file that sets one alone.
    terrain_len: u64,
    /// §1.2 filler between the OBCT container's last byte and the unit boundary `Terrain Length`
    /// rounds up to — §1.3's "the window is up to `U − 1` bytes longer than the container".
    terrain_gap: u64,
    total: u64,
}

/// Compute a shard's total size without writing it — §5.7's projection, applied to the assembler's
/// own output so an over-size file is refused rather than emitted.
pub fn projected_bytes(plan: &MapPlan, style_len: usize, poi_len: u64, nav: crate::nav::NavProjection) -> Result<u64> {
    Ok(plan.layout(style_len, poi_len, nav)?.total)
}

/// The nav section's exact bytes in `plan`, for the assembly report. Kept beside
/// [`projected_bytes`] so reporting and the write use the same absolute-offset arithmetic.
pub fn projected_nav_bytes(
    plan: &MapPlan,
    style_len: usize,
    poi_len: u64,
    nav: crate::nav::NavProjection,
) -> Result<u64> {
    let layout = plan.layout(style_len, poi_len, nav)?;
    Ok(nav.bytes_at(layout.nav_offset))
}

/// Write the map: header, style table, LOD table, every LOD region, the POI section, the nav
/// section, and — when the assembly has a raster — the spliced §1.3 terrain region. Returns
/// `(bytes, sha256)`.
///
/// Nothing is back-patched. Every offset in the header and the LOD table is known before the first
/// byte goes out, because the graft plan, both rebuilt sections and the raster were sized first —
/// which is what lets the output stream straight into a file (or a browser's download stream) rather
/// than a buffer.
///
/// **The raster is spliced here rather than appended**, and the difference is not stylistic. An
/// append would mean writing the map, closing it, and coming back with a seek — but the terrain
/// offset lives in the *header*, at byte 33 of a file whose first byte has already gone out, and the
/// merged nav graph is still resident and still holding the scratch streams its section is written
/// from. Splicing mid-stream keeps one pass, one resident graph, and one place where a byte can be
/// wrong. §1.3 put terrain last for exactly this reason: it moves no other offset.
///
/// `cells` are the grafted cells; `nav_cells` are the `network` cells the §4.6 merge read, in the
/// order it read them. They are a second list because the merged graph holds its edge records as
/// *addresses* into those cells (§4.6.6) — the nav section is streamed out of them here rather than
/// out of a pool the merge would otherwise have had to keep.
///
/// `scratch` must be the store the §4.6 merge spilled into: since #1116 D4 the nav section's index,
/// chunks and pool plan live there too, and they stay valid until `MergedNav::release`.
// The argument list is one map's whole input: the plan, the cells it grafts, the rebuilt pieces, the
// raster, and the sink. Bundling them into a struct would move the noise rather than remove it —
// the same call the packer's `serialize_lods_streaming` makes, for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn write(
    plan: &MapPlan,
    cells: &[Cell<'_>],
    nav_cells: &[&Cell<'_>],
    styles: &[StyleRecord],
    marker_color: u16,
    poi: &PoiSection,
    nav: &MergedNav,
    profile_table: &[u8],
    terrain: Option<&crate::terrain::TerrainRegion<'_>>,
    scratch: &dyn ScratchStore,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    let style_bytes = pack_style_table(styles);
    let nav_projection = nav.projection(profile_table);
    let l = plan.layout(style_bytes.len(), poi.section_len(), nav_projection)?;
    debug_assert_eq!(
        plan.terrain_bytes,
        terrain.map_or(0, |t| t.bytes()),
        "the plan's raster length is the raster it is handed"
    );
    fits_ceiling(l.total, "the map")?;
    // `OBCM_Spec.md` §1.1's one producer rule: **the scale MUST cover the file it writes**. The
    // ceiling above is now *derived from* this rule rather than independent of it, so the two can
    // no longer disagree — which is why this stays: it is the rule stated where the bytes are, and
    // it is the check that survives if the ceiling above is ever re-expressed. A reader that never
    // resolves the last section never sees a thing wrong, so the producer is the only party
    // positioned to notice.
    if !SCALE.covers(l.total) {
        return Err(Error::Capacity(format!(
            "the map would be {} bytes, past the interior `Offset Scale` {} addresses (OBCM §1.1)",
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
    out(&header_block(
        plan.box_,
        plan.lods.len(),
        marker_color,
        l.lod_table_offset,
        l.poi_offset,
        l.nav_offset,
        l.terrain_offset,
        l.terrain_len,
    )?)?;

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

    // 5/6. The POI and nav sections.
    //
    // Both section writers take a `usize` base. That is a 32-bit type in the wasm32 `--lib` build
    // this engine actually ships in, so these conversions are checked rather than cast: a layout
    // past `usize` would otherwise wrap and address a section that is not there. `FILE_CEILING`
    // keeps them unreachable today, but a ceiling is a policy and a cast is forever.
    let poi_base = usize::try_from(l.poi_offset).map_err(|_| plan.past_usize("POI", l.poi_offset))?;
    let nav_base = usize::try_from(l.nav_offset).map_err(|_| plan.past_usize("nav", l.nav_offset))?;
    out(&crate::poi::serialize(poi, poi_base)?)?;
    crate::nav::serialize(nav, profile_table, nav_base, nav_cells, scratch, &mut out)?;

    // 7. The raster (§1.3): the filler that carries the nav section to the region's unit boundary,
    //    the OBCT container verbatim, then the filler `Terrain Length`'s unit count rounds up to.
    if let Some(region) = terrain {
        out(&FILLER_RUN[..l.nav_gap as usize])?;
        region.emit(&mut out)?;
        out(&FILLER_RUN[..l.terrain_gap as usize])?;
    }

    // §4.8.6: the write must land exactly where §5.7's projection said it would. A `debug_assert`
    // would leave a release build emitting a file whose header offsets are a sentence about a
    // layout that does not exist.
    if written != l.total {
        return Err(Error::Verify(format!(
            "the map projected to {} bytes but wrote {written} — the §5.7 projection and the write disagree",
            l.total
        )));
    }
    Ok((written, hasher.finalize().into()))
}

/// The 49-byte v14 OBCM header (`OBCM_Spec.md` §1), byte-for-byte the packer's `header_bytes`. Split
/// out because it is a **restatement** of `obc-pack`'s serializer, and `tests/pinning.rs` compares
/// the two outputs directly rather than trusting that two copies of a table stay in step.
///
/// Every offset is given as a **byte** offset and scaled here, exactly as the packer's writer takes
/// them: the planner works in bytes throughout (§5.7's ceiling is a byte count) and this is the one
/// seam where they become units.
///
/// `terrain_offset` / `terrain_len` are §1.3's region pointer, and `(0, 0)` is its unambiguous
/// absence — a complete map whose profiles are flat. This is where `obc-pack` and this engine now
/// genuinely differ: the packer has no raster to splice and always writes the zero pair, while an
/// assembly has one whenever the catalog published a terrain lattice for the selection.
// Six offsets and a bbox is what the §1 header *is*; a struct would restate the table one more time.
#[allow(clippy::too_many_arguments)]
pub fn header_bytes(
    box_: AlignedBox,
    lod_count: usize,
    marker_color: u16,
    lod_table_offset: u64,
    poi_offset: u64,
    nav_offset: u64,
    terrain_offset: u64,
    terrain_len: u64,
) -> Result<Vec<u8>> {
    // §1.3: a reader MUST refuse a file that sets one of the pair without the other, so a producer
    // must never emit one. Checked here rather than trusted from the layout, because this function
    // is also the packer's pinned twin and the rule belongs with the bytes.
    if (terrain_offset == 0) != (terrain_len == 0) {
        return Err(Error::Verify(format!(
            "the terrain region is ({terrain_offset}, {terrain_len}) — §1.3 makes `0` mean absence for both fields \
             or neither"
        )));
    }
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
    head.extend_from_slice(&scaled(terrain_offset)?.to_le_bytes());
    head.extend_from_slice(&scaled(terrain_len)?.to_le_bytes());
    debug_assert_eq!(head.len(), HEADER_LEN);
    Ok(head)
}

/// The header plus the §1.2 filler that carries it to the style table's unit boundary.
// The header's own argument list plus nothing; see `header_bytes`.
#[allow(clippy::too_many_arguments)]
fn header_block(
    box_: AlignedBox,
    lod_count: usize,
    marker_color: u16,
    lod_table_offset: u64,
    poi_offset: u64,
    nav_offset: u64,
    terrain_offset: u64,
    terrain_len: u64,
) -> Result<Vec<u8>> {
    let mut out = header_bytes(
        box_,
        lod_count,
        marker_color,
        lod_table_offset,
        poi_offset,
        nav_offset,
        terrain_offset,
        terrain_len,
    )?;
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
pub fn header_style_offset(map: &[u8]) -> Option<u64> {
    if map.len() < HEADER_LEN {
        return None;
    }
    let scale = OffsetScale::new(map[obc_formats::obcm::HEADER_OFFSET_SCALE_OFF]).ok()?;
    let units = u32::from_le_bytes(
        map[HEADER_STYLE_OFFSET_AT..HEADER_STYLE_OFFSET_AT + 4].try_into().expect("four bytes inside the header"),
    );
    Some(scale.offset(units).bytes())
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
    // The table is restamped in a `map` that is already resident, so this is one of the places
    // where the file offset legitimately becomes a `usize` — the narrowing is against RAM, not
    // against the seam, and it fails closed for a resident buffer that cannot hold the offset.
    let style_offset =
        header_style_offset(map).and_then(|at| usize::try_from(at).ok()).ok_or(RestampError::BadStyleOffset)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scratch::MemoryScratch;

    fn bx() -> AlignedBox {
        AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 }
    }

    fn plan() -> MapPlan {
        MapPlan { box_: bx(), lods: Vec::new(), terrain_bytes: 0, bytes: 1234, sha256: [0; 32] }
    }

    /// **One wall, and it is the format's.** Three others stood here at various times — FAT32's
    /// per-file cap, the read seam's `u32` offsets, and OBCA §5.2's `uint32` `Bytes` — and each was
    /// the binding one for a while. All three are gone, so the number a producer may write and the
    /// number a reader may open are the same again.
    ///
    /// A `const` block: these are relationships between constants, so a compile error is the right
    /// failure and a test run is merely where it gets read.
    #[test]
    fn the_only_wall_is_the_formats() {
        const { assert!(FILE_CEILING == 1 << 36, "§1.1's interior at U = 16") };
        const { assert!(SCALE.covers(FILE_CEILING), "and the scale covers it") };
        const { assert!(FILE_CEILING > u32::MAX as u64, "past the 4 GiB every dead wall landed on") };
    }

    /// The plan-time refusal: one byte past the wall is rejected **before anything is written**, and
    /// the message names the file and the one remedy that is left.
    #[test]
    fn one_byte_past_the_wall_is_refused_before_anything_is_written() {
        assert!(fits_ceiling(FILE_CEILING, "the map").is_ok(), "the wall itself fits");
        let err = fits_ceiling(FILE_CEILING + 1, "the map").expect_err("one byte past must refuse");
        match err {
            Error::Capacity(m) => {
                assert!(m.contains("the map"), "the refusal names what it refused: {m}");
                assert!(m.contains("reduce the coverage"), "and the remedy (OBCA §5.7): {m}");
                assert!(m.contains("OBCM"), "and which rule is the wall: {m}");
            }
            other => panic!("an over-size plan is a capacity refusal, got {other:?}"),
        }
    }

    /// **The writer side of the far offsets.** `obc-reader`'s `far_offsets.rs` proves a map *parses*
    /// past 4 GiB; this proves one can be *laid out* there — that every cursor, every scaled offset
    /// and the header field they land in survive the crossing.
    ///
    /// A genuinely >4 GiB assembly is far too heavy for CI (it would have to materialise the bytes),
    /// so this works at the projection level, which is exactly where the u32 hazards live: the
    /// layout cursor, `scaled()`'s unit conversion, and the header's `uint32` fields. The first real
    /// >4 GiB single file is a DACH bake, and it is owner-run.
    #[test]
    fn a_layout_past_four_gibibytes_is_projected_and_addressed_in_full() {
        // Two LODs of 3 GB each: 6 GB of chunks, comfortably past every wall that used to bind and
        // comfortably inside the 64 GiB that does.
        let mut p = plan();
        p.lods = vec![LodPlan { node_count: 1, chunk_bytes: 3_000_000_000, ..LodPlan::empty(0, None, 4096) }; 2];
        let poi = crate::poi::empty_layout(p.box_.ubox());
        let nav = MergedNav::empty(Default::default());
        let nav_projection = nav.projection(&[]);

        let projected = projected_bytes(&p, 0, poi.section_len(), nav_projection).expect("a u64 holds it");
        assert!(projected > u32::MAX as u64, "the projection is past 4 GiB: {projected}");
        assert!(fits_ceiling(projected, "the map").is_ok(), "…and inside the format's interior");
        // Nothing wrapped: the total is the sum of its parts computed in u64.
        let prefix = align_up(align_up(STYLE_OFFSET) + 2 * LOD_ENTRY_LEN as u64);
        let expected = prefix + 2 * (3_000_000_000 + 4 + 4 + 8) + poi.section_len() + nav_projection.bytes_at(0);
        assert_eq!(projected, expected);

        // The nav section starts past 4 GiB, and the header field that names it is a `uint32` of
        // 16-byte units — which is the whole point of v14's scaling. Check the round trip, because a
        // silently truncating conversion here would produce a header that points into the geometry.
        let l = p.layout(0, poi.section_len(), nav_projection).expect("the layout");
        assert!(l.nav_offset > u32::MAX as u64, "the nav section is past 4 GiB: {}", l.nav_offset);
        let units = scaled(l.nav_offset).expect("a scaled offset names it");
        assert_eq!(SCALE.offset(units).bytes(), l.nav_offset, "the unit count resolves back to the byte");

        let head = header_bytes(p.box_, 2, 0, l.lod_table_offset, l.poi_offset, l.nav_offset, 0, 0)
            .expect("the header holds a far layout");
        let field = u32::from_le_bytes(head[36..40].try_into().expect("§1's Nav Offset field"));
        assert_eq!(field, units, "the header carries the unit count, not a truncated byte offset");
    }

    /// §5.7's ceiling is only a ceiling if the projection can exceed it. The layout is `u64` for
    /// that reason: in the wasm32 `--lib` build a `usize` cursor wraps at 4 GiB, so an over-size
    /// selection would project *small*, pass the gate, and stream a file whose header offsets belong
    /// to a layout that does not exist.
    #[test]
    fn a_layout_past_the_ceiling_is_refused_rather_than_wrapped() {
        let mut p = plan();
        // Two LODs of 40 GB each: past v14's 64 GiB interior.
        p.lods = vec![LodPlan { node_count: 1, chunk_bytes: 40_000_000_000, ..LodPlan::empty(0, None, 4096) }; 2];
        let poi = crate::poi::empty_layout(p.box_.ubox());
        let nav = MergedNav::empty(Default::default());
        let nav_projection = nav.projection(&[]);

        let projected = projected_bytes(&p, 0, poi.section_len(), nav_projection).expect("a u64 holds it");
        assert!(projected > FILE_CEILING);

        let mut sink = |_: &[u8]| -> Result<()> { panic!("a refused map writes no bytes") };
        let err = write(&p, &[], &[], &[], 0, &poi, &nav, &[], None, &MemoryScratch::new(), &mut sink)
            .expect_err("past the ceiling");
        assert!(matches!(err, Error::Capacity(_)), "got: {err}");
        assert!(format!("{err}").contains("past the"), "got: {err}");
    }

    /// §1.3's absence is a **pair**: `Terrain Offset == 0` iff `Terrain Length == 0`, and a reader
    /// refuses a file that sets one alone. So a producer must be unable to write one — the layout
    /// keeps them in step, and this is the check that says so even if a caller hand-builds a header.
    #[test]
    fn a_half_present_terrain_pair_cannot_be_written() {
        let l = plan().layout(0, 0, MergedNav::empty(Default::default()).projection(&[])).expect("layout");
        for (offset, len) in [(l.total, 0u64), (0, 4096)] {
            let err = header_bytes(bx(), 0, 0, l.lod_table_offset, l.poi_offset, l.nav_offset, offset, len)
                .expect_err("one without the other is not a legal header");
            assert!(format!("{err}").contains("absence"), "got: {err}");
        }
        // …and both zero is the ordinary map with no elevation.
        header_bytes(bx(), 0, 0, l.lod_table_offset, l.poi_offset, l.nav_offset, 0, 0).expect("no raster is legal");
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
