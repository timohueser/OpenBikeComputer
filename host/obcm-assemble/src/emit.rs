//! Laying out one assembled map and writing its bytes: header, style table, LOD table, every LOD
//! region, the POI and nav sections, and the spliced terrain region.
//!
//! An assembly is one file, and there is one wall it has to clear: [`FILE_CEILING`], through
//! [`fits_ceiling`]. There is no fast path and no split path to disagree about which wall is which.
//!
//! Nothing here is back-patched. Every offset in the header and the LOD table is known before the
//! first byte goes out, because the graft plan, both rebuilt sections and the raster were all sized
//! first — which is what lets an assembly stream straight into a file, or into a browser's
//! download, rather than into a buffer the size of the map.
use obc_formats::obcm::{
    OffsetScale, UnitWriter, HEADER_LANDMARK_LENGTH_OFF, HEADER_LANDMARK_OFFSET_OFF, HEADER_LEN, LOD_ENTRY_LEN, MAGIC,
    STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT, STYLE_PRIORITY_MASK, STYLE_RECORD_LEN,
    STYLE_TERRAIN_LAYER_BIT, STYLE_TICKED_BIT, VERSION,
};
use sha2::{Digest, Sha256};

use crate::graft::{self, LodPlan};
use crate::grid::AlignedBox;
use crate::input::Cell;
use crate::nav::MergedNav;
use crate::poi::PoiSection;
use crate::schema::{LineStyle, StyleRecord};
use crate::scratch::ScratchStore;
use crate::{Error, Result};

/// The hard per-file ceiling: the smaller of the two walls a written file has to clear.
///
/// 1. The format wall — `OBCM_Spec.md`'s addressable interior at this engine's [`SCALE`],
///    `2^32 units × U`, which at `U = 16` is 64 GiB.
/// 2. The readable wall — how far [`obc_formats::io::ByteSource`], the tree's one read interface,
///    can address.
///
/// The `min` stays as structure rather than collapsing to the format wall: a written file must
/// clear whatever the format can express and whatever a reader can reach, including the reader on
/// the card, and the day either moves this constant follows.
pub const FILE_CEILING: u64 = {
    let format = (1u64 << 32) * SCALE.unit();
    let readable = READABLE_CEILING;
    if format < readable {
        format
    } else {
        readable
    }
};

/// How far a byte offset handed to [`obc_formats::io::ByteSource::read_at`] can reach. Named rather
/// than written inline so [`FILE_CEILING`]'s `min` keeps saying which wall each side is.
const READABLE_CEILING: u64 = u64::MAX;
const _: () = assert!(FILE_CEILING == 1u64 << 36, "at U = 16 the format's interior is 64 GiB, and it is what binds");
const _: () = assert!(SCALE.covers(FILE_CEILING), "and the scale still covers whatever the min lands on");

/// The one remedy for an over-size map: a map is one file, so there is nothing left to move
/// somewhere else and the only lever is how much ground the selection covers.
pub const SIZE_REMEDY: &str = "reduce the coverage (OBCA §4.8)";

/// Does a file of `bytes` fit the one wall a written map has to clear?
///
/// This is the only place that comparison exists, and every gate asks here rather than open-coding
/// it: an open-coded `<= CEILING` reads correct whichever constant it names.
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

/// The `Offset Scale` every shard this engine writes carries: `U = 16`, the same byte `obc-pack`
/// writes, so a cell and the assembly it lands in count offsets in one unit.
///
/// It is also what the agreement check refuses a disagreement on: a cell whose `Index Offset`
/// counted a different unit would relocate into a plausible byte of the output rather than an
/// obviously wrong one.
pub const SCALE: OffsetScale = OffsetScale::DEFAULT;

/// The byte offset of the style table in every shard this engine writes: the first unit boundary at
/// or after the 57-byte header. Byte-for-byte `obc-pack`'s own `STYLE_OFFSET`.
pub const STYLE_OFFSET: u64 = 80;
const _: () = assert!(STYLE_OFFSET >= HEADER_LEN as u64);

/// The next unit boundary at or after `cursor`. Every structure a header or directory offset
/// reaches begins on one; the `0..U-1` bytes this rounds past are [`FILLER`].
#[inline]
pub fn align_up(cursor: u64) -> u64 {
    SCALE.align_up(cursor).expect("a layout cursor never approaches u64::MAX")
}

/// The filler run [`align_up`] implies at `cursor` — `0..U-1` bytes of `0xFF`.
#[inline]
pub fn filler_len(cursor: u64) -> u64 {
    align_up(cursor) - cursor
}

/// The `uint32` a scaled offset field stores for byte offset `at`.
///
/// A scaled offset cannot name a byte that is not a multiple of `U`, so a non-boundary argument is
/// a bug in the layout above it rather than a rounding request — but this engine runs in a browser
/// tab, so it is an [`Error::Capacity`] and not a panic.
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

/// This engine's cursor: a [`UnitWriter`] over the assembly's output sink.
///
/// Every writer below takes one rather than a bare byte sink, so the unit boundaries are found by
/// the same cursor the bytes go through and no section writer needs a position counter of its own.
pub type MapWriter<'a> = UnitWriter<'a, Error>;

/// Walk a layout with a cursor whose sink keeps nothing, so a section's projection comes from the
/// arithmetic that emits it rather than from a second copy of it.
pub(crate) fn place<T>(at: u64, walk: impl FnOnce(&mut UnitWriter<'_, Error>) -> Result<T>) -> Result<T> {
    let mut discard = |_: &[u8]| -> Result<()> { Ok(()) };
    walk(&mut UnitWriter::new(SCALE, at, &mut discard))
}

/// Where a producer warns: seven eighths of the wall. A proportion rather than a number, so it
/// keeps meaning "close" wherever [`FILE_CEILING`] lands.
///
/// At the current ceiling it sits at ≈56 GiB and the largest selection contemplated is ≈9 GiB, so
/// this is a tripwire on the format's limit, not a usable size signal. What a rider runs out of is
/// card space, and the builder's size meter against the free space is that signal.
pub const SIZE_WARN: u64 = FILE_CEILING / 8 * 7;
const _: () = assert!(SIZE_WARN < FILE_CEILING, "a warning above the wall would never fire");

/// One assembled map, laid out before a byte is written.
pub struct MapPlan {
    pub box_: AlignedBox,
    /// One entry per ladder level; a map carries the full ladder.
    pub lods: Vec<LodPlan>,
    /// The spliced terrain region's exact byte length, or `0` for a map with no elevation. Known
    /// before the layout, which is what lets the header state the region's offset without a
    /// back-patch.
    pub terrain_bytes: u64,
    /// Optional landmark region, including its final unit padding.
    pub landmark_bytes: u64,
    pub peak_bytes: u64,
    /// Surface tiles start on SD block boundaries in the complete map.
    pub surface_terrain: bool,

    /// Total bytes, computable before the write and re-checked after it.
    pub bytes: u64,
    /// Filled by [`write`].
    pub sha256: [u8; 32],
}

impl MapPlan {
    /// Layout cursor: where each region starts, given the fixed prefix. `u64` throughout, never
    /// `usize`: the crate's `--lib` target is wasm32, where a projection accumulated in a 32-bit
    /// `usize` wraps past 4 GiB and hands the ceiling a small number it happily accepts.
    ///
    /// Region starts use scaled offsets and begin on a unit boundary. The landmark region follows
    /// navigation; terrain is last. Each per-LOD and per-section interior carries its own gaps and
    /// ends on a unit boundary, which keeps this cursor aligned without a second rounding step.
    ///
    /// The terrain region is the one region that does not end on a boundary: an OBCT container is
    /// whatever length the raster makes it, `Terrain Length` counts units, and the difference is
    /// filler at the file's tail.
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

        let landmark_offset = if self.landmark_bytes == 0 { 0 } else { nav_end };
        let landmark_end = nav_end.checked_add(self.landmark_bytes).ok_or_else(|| self.past_u64())?;
        let peak_offset = if self.peak_bytes == 0 { 0 } else { landmark_end };
        let landmark_end = landmark_end.checked_add(self.peak_bytes).ok_or_else(|| self.past_u64())?;
        // Terrain sits last, so that splicing it moves no other offset. A map with no raster ends
        // after its landmarks and writes `(0, 0)`, which is unambiguous because byte 0 is the
        // header itself.
        let (terrain_offset, terrain_len, total) = if self.terrain_bytes == 0 {
            (0, 0, landmark_end)
        } else {
            let at = if self.surface_terrain { (landmark_end + 511) & !511 } else { align_up(landmark_end) };
            let end = at.checked_add(self.terrain_bytes).ok_or_else(|| self.past_u64())?;
            let total = align_up(end);
            (at, total - at, total)
        };
        Ok(Layout {
            lod_table_offset,
            lod_offsets,
            poi_offset,
            nav_offset,
            landmark_offset,
            peak_offset,
            terrain_offset,
            terrain_len,
            total,
        })
    }

    fn past_u64(&self) -> Error {
        Error::Capacity("the map's layout does not fit a u64 of bytes".into())
    }

    /// A section base that does not fit the host's `usize`, which is 32-bit in the wasm32 build
    /// this engine ships in. Unreachable behind [`FILE_CEILING`], and an error rather than a cast
    /// so that it stays unreachable if the ceiling moves.
    fn past_usize(&self, what: &str, at: u64) -> Error {
        Error::Capacity(format!(
            "the map's {what} section starts at byte {at}, past the {} bytes this host can address",
            usize::MAX
        ))
    }
}

/// Where each region starts. The gaps between them are not here: [`write`] reaches them by asking
/// its cursor for the next unit boundary.
struct Layout {
    lod_table_offset: u64,
    lod_offsets: Vec<u64>,
    poi_offset: u64,
    nav_offset: u64,
    landmark_offset: u64,
    peak_offset: u64,
    /// Byte offset of the terrain region, or `0` for a map with no elevation.
    terrain_offset: u64,
    /// The region's length including the filler `Terrain Length`'s unit count rounds up to, so the
    /// header's pair is `(offset, len)` in bytes and both scale by the same rule. `0` exactly when
    /// `terrain_offset` is: a reader refuses a file that sets one alone.
    terrain_len: u64,

    total: u64,
}

/// Compute a map's total size without writing it, so an over-size file is refused rather than
/// emitted.
pub fn projected_bytes(plan: &MapPlan, style_len: usize, poi_len: u64, nav: crate::nav::NavProjection) -> Result<u64> {
    Ok(plan.layout(style_len, poi_len, nav)?.total)
}

/// Bytes added before terrain by summit metadata and the surface sector alignment.
pub fn peak_view_prefix_bytes(
    plan: &MapPlan,
    style_len: usize,
    poi_len: u64,
    summit_bytes: u64,
    nav: crate::nav::NavProjection,
) -> Result<u64> {
    let layout = plan.layout(style_len, poi_len, nav)?;
    let native_nav = layout
        .nav_offset
        .checked_sub(summit_bytes)
        .ok_or_else(|| Error::Capacity("summit section exceeds the map layout".into()))?;
    let native_start = align_up(native_nav + nav.bytes_at(native_nav) + plan.landmark_bytes + plan.peak_bytes);
    Ok(layout.terrain_offset - native_start)
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
/// section, and, when the assembly has a raster, the spliced terrain region. Returns
/// `(bytes, sha256)`.
///
/// Nothing is back-patched. Every offset in the header and the LOD table is known before the first
/// byte goes out, which is what lets the output stream straight into a file or a browser's download
/// stream rather than into a buffer.
///
/// The raster is spliced here rather than appended. An append would mean writing the map, closing
/// it and coming back with a seek, but the terrain offset lives in the header of a file whose first
/// byte has already gone out, and the merged nav graph is still holding the scratch streams its
/// section is written from. Splicing mid-stream keeps one pass and one resident graph.
///
/// `cells` are the grafted cells; `nav_cells` are the `network` cells the merge read, in the order
/// it read them. They are a second list because the merged graph holds its edge records as
/// addresses into those cells, and the nav section is streamed out of them here.
///
/// `scratch` must be the store the merge spilled into: the nav section's index, chunks and pool
/// plan live there and stay valid until `MergedNav::release`.
// The argument list is one map's whole input. Bundling it into a struct would move the noise rather
// than remove it.
#[allow(clippy::too_many_arguments)]
pub fn write(
    plan: &MapPlan,
    cells: &[Cell<'_>],
    nav_cells: &[&Cell<'_>],
    styles: &[StyleRecord],
    marker_color: u16,
    poi: &PoiSection,
    landmarks: &crate::landmarks::LandmarkSection,
    peaks: &crate::peaks::PeakSection,
    nav: &MergedNav,
    profile_table: &[u8],
    terrain: Option<&crate::terrain::TerrainRegion<'_>>,
    scratch: &dyn ScratchStore,
    sink: &mut dyn FnMut(&[u8]) -> Result<()>,
) -> Result<(u64, [u8; 32])> {
    let style_bytes = pack_style_table(styles);
    let nav_projection = nav.projection(profile_table);
    let l = plan.layout(style_bytes.len(), poi.section_len(), nav_projection)?;
    if plan.landmark_bytes != landmarks.section_len() || plan.peak_bytes != peaks.section_len() {
        return Err(Error::Verify("landmark section differs from the map plan".into()));
    }
    debug_assert_eq!(
        plan.terrain_bytes,
        terrain.map_or(0, |t| t.bytes()),
        "the plan's raster length is the raster it is handed"
    );
    fits_ceiling(l.total, "the map")?;
    // The one producer rule: the scale must cover the file it writes. Stated where the bytes are,
    // because a reader that never resolves the last section never sees a thing wrong and the
    // producer is the only party positioned to notice.
    if !SCALE.covers(l.total) {
        return Err(Error::Capacity(format!(
            "the map would be {} bytes, past the interior `Offset Scale` {} addresses (OBCM §1.1)",
            l.total,
            SCALE.log2()
        )));
    }

    let mut hasher = Sha256::new();
    // The bytes the sink actually received, counted independently of where the cursor thinks it is.
    // The two differ only if a writer below reached for `UnitWriter::advance`, a projection's tool,
    // which would leave a hole in the file.
    let mut delivered: u64 = 0;
    // Scoped so the cursor gives `hasher` and `delivered` back before they are read.
    let ended_at = {
        let mut out = |buf: &[u8]| -> Result<()> {
            hasher.update(buf);
            delivered += buf.len() as u64;
            sink(buf)
        };
        let mut w = MapWriter::new(SCALE, 0, &mut out);

        // 1. Header (bbox stored lat, lon, lat, lon), then the filler that carries the 65-byte
        //    header to the style table's unit boundary.
        let mut header = header_bytes(
            plan.box_,
            plan.lods.len(),
            marker_color,
            l.lod_table_offset,
            l.poi_offset,
            l.nav_offset,
            l.terrain_offset,
            l.terrain_len,
        )?;
        header[HEADER_LANDMARK_OFFSET_OFF..HEADER_LANDMARK_OFFSET_OFF + 4]
            .copy_from_slice(&scaled(l.landmark_offset)?.to_le_bytes());
        header[HEADER_LANDMARK_LENGTH_OFF..HEADER_LANDMARK_LENGTH_OFF + 4]
            .copy_from_slice(&scaled(plan.landmark_bytes)?.to_le_bytes());
        header[obc_formats::obcm::HEADER_PEAK_OFFSET_OFF..obc_formats::obcm::HEADER_PEAK_OFFSET_OFF + 4]
            .copy_from_slice(&scaled(l.peak_offset)?.to_le_bytes());
        header[obc_formats::obcm::HEADER_PEAK_LENGTH_OFF..obc_formats::obcm::HEADER_PEAK_LENGTH_OFF + 4]
            .copy_from_slice(&scaled(plan.peak_bytes)?.to_le_bytes());
        w.put(&header)?;
        w.begin_section()?;

        // 2. Style table (the skin) and 3. the LOD table, each followed by the filler that lands
        //    the next scaled-offset-named structure on its boundary.
        w.put(&style_bytes)?;
        w.begin_section()?;
        let mut table = Vec::with_capacity(plan.lods.len() * LOD_ENTRY_LEN);
        for (p, &offset) in plan.lods.iter().zip(&l.lod_offsets) {
            push_lod_entry(&mut table, p.max_mpp, scaled(offset)?, p.node_count, p.chunk_size, p.chunk_count);
        }
        w.put(&table)?;
        w.begin_section()?;

        // 4. Each LOD region: fresh upper tree, relocated cell blocks, offset table, chunk bytes.
        for p in &plan.lods {
            graft::emit_lod(p, cells, &mut w)?;
        }

        // 5/6. The POI and nav sections.
        //
        // The nav writer takes a `usize` base, which is 32-bit in the wasm32 `--lib` build this
        // engine ships in, so the conversion is checked rather than cast: a layout past `usize`
        // would wrap and address a section that is not there.
        let nav_base = usize::try_from(l.nav_offset).map_err(|_| plan.past_usize("nav", l.nav_offset))?;
        crate::poi::emit(poi, &mut w)?;
        crate::nav::serialize(nav, profile_table, nav_base, nav_cells, scratch, &mut w)?;
        landmarks.emit(nav_cells, &mut w)?;
        peaks.emit(nav_cells, &mut w)?;

        // 7. The raster: the filler that carries the nav section to the region's unit boundary, the
        //    OBCT container verbatim, then the filler `Terrain Length`'s unit count rounds up to.
        if let Some(region) = terrain {
            w.pad(
                l.terrain_offset
                    .checked_sub(w.at())
                    .ok_or_else(|| Error::Verify("terrain starts before the current map cursor".into()))?,
            )?;
            region.emit(&mut w)?;
            w.begin_section()?;
        }

        w.at()
    };

    // The write must land exactly where the projection said it would. A `debug_assert` would leave
    // a release build emitting a file whose header offsets describe a layout that does not exist.
    if delivered != l.total || ended_at != l.total {
        return Err(Error::Verify(format!(
            "the map projected to {} bytes, the cursor ended at {ended_at} and {delivered} were written — the §5.7 \
             projection and the write disagree",
            l.total
        )));
    }
    Ok((delivered, hasher.finalize().into()))
}

/// The 57-byte OBCM header, byte-for-byte the packer's `header_bytes`. Split out because it is a
/// restatement of `obc-pack`'s serializer, and `tests/pinning.rs` compares the two outputs directly
/// rather than trusting that two copies of a table stay in step.
///
/// Every offset is given as a byte offset and scaled here: the planner works in bytes throughout
/// and this is the one seam where they become units.
///
/// `terrain_offset` / `terrain_len` are the terrain region pointer, and `(0, 0)` is its unambiguous
/// absence. The packer has no raster to splice and always writes the zero pair.
// Six offsets and a bbox is what the header is; a struct would restate the table one more time.
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
    // A reader refuses a file that sets one of the pair without the other, so a producer must never
    // emit one. Checked here rather than trusted from the layout, because the rule belongs with the
    // bytes.
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
    head.extend_from_slice(&[0; 16]); // optional landmark and peak sections
    debug_assert_eq!(head.len(), HEADER_LEN);
    Ok(head)
}

/// Append one 18-byte LOD-table entry, byte-for-byte the packer's `push_lod_entry`:
/// `Max Meters/Pixel` (`None` ⇒ `+inf`), index offset, node count, chunk capacity, chunk count.
/// Pinned against the packer alongside the header.
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

/// Byte offset of the header's `Style Offset` field: magic 4, version 1, four `int32` bbox fields.
pub const HEADER_STYLE_OFFSET_AT: usize = 21;

/// Resolve a map's `Style Offset` to a byte offset, through the file's own `Offset Scale`. `None`
/// when the header is short, the scale byte is not one the format defines, or the resolved byte
/// does not fit this host's address space.
///
/// The scale is read out of the image rather than assumed to be [`SCALE`]: this is the one function
/// here that runs over bytes the engine did not write.
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
/// callers word their failures for very different readers: a maintainer refreshing a fixture, and a
/// person in a browser tab whose picture did not change.
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
}

/// Stamp a resolved skin onto an OBCM image in place: its style table and the header's marker
/// colour, and nothing else. That is the whole of what applying a skin means — about 2 KB and one
/// `u16` — which is why a skin invalidates no cell and a preview needs no re-pack.
///
/// Only the styles the image carries are stamped. Style ids are assigned in schema document order,
/// so a schema that has grown feature types keeps every id in the image meaning what it meant. The
/// image's table must be a prefix of the skin's assignment; ids that disagree are refused, because
/// there the bytes mean something the schema no longer says.
pub fn restamp_style_table(
    map: &mut [u8],
    styles: &[StyleRecord],
    marker_color: u16,
) -> core::result::Result<(), RestampError> {
    if map.len() < HEADER_LEN {
        return Err(RestampError::ShorterThanHeader);
    }
    // The table is restamped in a `map` that is already resident, so the file offset legitimately
    // becomes a `usize`: the narrowing is against RAM, not against the read seam.
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
    let have: Vec<u8> = slot[1..].as_chunks::<STYLE_RECORD_LEN>().0.iter().map(|record| record[0]).collect();
    let want: Vec<u8> = stamped.iter().map(|style| style.id).collect();
    if have != want {
        return Err(RestampError::IdMismatch { have, want });
    }
    slot.copy_from_slice(&packed);
    map[HEADER_MARKER_COLOR_AT..HEADER_MARKER_COLOR_AT + 2].copy_from_slice(&marker_color.to_le_bytes());
    Ok(())
}

/// The style table: `Count` then one 8-byte record per style, id ascending.
pub fn pack_style_table(styles: &[StyleRecord]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    out.push(styles.len() as u8);
    for s in styles {
        let mut flags = (s.priority.clamp(1, 4) - 1) & STYLE_PRIORITY_MASK;
        match s.line_style {
            LineStyle::Solid => {}
            LineStyle::Dashed => flags |= STYLE_DASHED_BIT,
            LineStyle::Ticked => flags |= STYLE_TICKED_BIT,
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
        MapPlan {
            box_: bx(),
            lods: Vec::new(),
            landmark_bytes: 0,
            peak_bytes: 0,
            terrain_bytes: 0,
            surface_terrain: false,
            bytes: 1234,
            sha256: [0; 32],
        }
    }

    /// One wall, and it is the format's: the number a producer may write and the number a reader
    /// may open are the same.
    ///
    /// A `const` block, because these are relationships between constants and a compile error is
    /// the right failure.
    #[test]
    fn the_only_wall_is_the_formats() {
        const { assert!(FILE_CEILING == 1 << 36, "§1.1's interior at U = 16") };
        const { assert!(SCALE.covers(FILE_CEILING), "and the scale covers it") };
        const { assert!(FILE_CEILING > u32::MAX as u64, "past the 4 GiB every dead wall landed on") };
    }

    /// The plan-time refusal: one byte past the wall is rejected before anything is written, and
    /// the message names the file and the remedy.
    #[test]
    fn one_byte_past_the_wall_is_refused_before_anything_is_written() {
        assert!(fits_ceiling(FILE_CEILING, "the map").is_ok(), "the wall itself fits");
        let err = fits_ceiling(FILE_CEILING + 1, "the map").expect_err("one byte past must refuse");
        match err {
            Error::Capacity(m) => {
                assert!(m.contains("the map"), "the refusal names what it refused: {m}");
                assert!(m.contains("reduce the coverage"), "and the remedy: {m}");
                assert!(m.contains("OBCM"), "and which rule is the wall: {m}");
            }
            other => panic!("an over-size plan is a capacity refusal, got {other:?}"),
        }
    }

    /// The writer side of the far offsets: `obc-reader`'s `far_offsets.rs` proves a map parses past
    /// 4 GiB, and this proves one can be laid out there.
    ///
    /// A genuinely >4 GiB assembly is far too heavy for CI, so this works at the projection level,
    /// which is where the `u32` hazards live: the layout cursor, `scaled()`'s unit conversion, and
    /// the header's `uint32` fields.
    #[test]
    fn a_layout_past_four_gibibytes_is_projected_and_addressed_in_full() {
        // Two LODs of 3 GB each: 6 GB of chunks, past 4 GiB and inside the 64 GiB wall.
        let mut p = plan();
        p.lods = vec![LodPlan { node_count: 1, chunk_bytes: 3_000_000_000, ..LodPlan::empty(0, None, 4096) }; 2];
        let poi = crate::poi::empty_layout(p.box_.ubox()).expect("an empty section lays out");
        let nav = MergedNav::empty(Default::default());
        let nav_projection = nav.projection(&[]);

        let projected = projected_bytes(&p, 0, poi.section_len(), nav_projection).expect("a u64 holds it");
        assert!(projected > u32::MAX as u64, "the projection is past 4 GiB: {projected}");
        assert!(fits_ceiling(projected, "the map").is_ok(), "…and inside the format's interior");
        // Nothing wrapped: the total is the sum of its parts computed in u64.
        let prefix = align_up(align_up(STYLE_OFFSET) + 2 * LOD_ENTRY_LEN as u64);
        let expected = prefix + 2 * (3_000_000_000 + 4 + 4 + 8) + poi.section_len() + nav_projection.bytes_at(0);
        assert_eq!(projected, expected);

        // The nav section starts past 4 GiB and the header field that names it is a `uint32` of
        // 16-byte units, so the round trip is checked: a silently truncating conversion would
        // produce a header that points into the geometry.
        let l = p.layout(0, poi.section_len(), nav_projection).expect("the layout");
        assert!(l.nav_offset > u32::MAX as u64, "the nav section is past 4 GiB: {}", l.nav_offset);
        let units = scaled(l.nav_offset).expect("a scaled offset names it");
        assert_eq!(SCALE.offset(units).bytes(), l.nav_offset, "the unit count resolves back to the byte");

        let head = header_bytes(p.box_, 2, 0, l.lod_table_offset, l.poi_offset, l.nav_offset, 0, 0)
            .expect("the header holds a far layout");
        let field = u32::from_le_bytes(head[36..40].try_into().expect("§1's Nav Offset field"));
        assert_eq!(field, units, "the header carries the unit count, not a truncated byte offset");
    }

    /// The ceiling is only a ceiling if the projection can exceed it. The layout is `u64` for that
    /// reason: in the wasm32 `--lib` build a `usize` cursor wraps at 4 GiB, so an over-size
    /// selection would project small, pass the gate, and stream a file whose header offsets belong
    /// to a layout that does not exist.
    #[test]
    fn a_layout_past_the_ceiling_is_refused_rather_than_wrapped() {
        let mut p = plan();
        // Two LODs of 40 GB each: past the 64 GiB interior.
        p.lods = vec![LodPlan { node_count: 1, chunk_bytes: 40_000_000_000, ..LodPlan::empty(0, None, 4096) }; 2];
        let poi = crate::poi::empty_layout(p.box_.ubox()).expect("an empty section lays out");
        let nav = MergedNav::empty(Default::default());
        let nav_projection = nav.projection(&[]);

        let projected = projected_bytes(&p, 0, poi.section_len(), nav_projection).expect("a u64 holds it");
        assert!(projected > FILE_CEILING);

        let mut sink = |_: &[u8]| -> Result<()> { panic!("a refused map writes no bytes") };
        let err = write(
            &p,
            &[],
            &[],
            &[],
            0,
            &poi,
            &crate::landmarks::LandmarkSection::default(),
            &crate::peaks::PeakSection::default(),
            &nav,
            &[],
            None,
            &MemoryScratch::new(),
            &mut sink,
        )
        .expect_err("past the ceiling");
        assert!(matches!(err, Error::Capacity(_)), "got: {err}");
        assert!(format!("{err}").contains("past the"), "got: {err}");
    }

    /// Terrain's absence is a pair: `Terrain Offset == 0` iff `Terrain Length == 0`, and a reader
    /// refuses a file that sets one alone, so a producer must be unable to write one.
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
            line_style: LineStyle::Dashed,
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

        // A stamped skin carries every style bit through to the record it writes, so a restyled
        // cell tree keeps its contour hairlines and terrain-layer tags instead of being cleared
        // back to a ramped road.
        let terrain = StyleRecord { fixed_width: true, terrain_layer: true, ..s };
        let bytes = pack_style_table(&[terrain]);
        assert_eq!(
            bytes[6],
            1 | STYLE_DASHED_BIT | STYLE_HAS_COLOR2_BIT | STYLE_FIXED_WIDTH_BIT | STYLE_TERRAIN_LAYER_BIT
        );
        assert_eq!(bytes[6] & obc_formats::obcm::STYLE_RESERVED_MASK, 0, "bits 6-7 stay 0");
    }
}
