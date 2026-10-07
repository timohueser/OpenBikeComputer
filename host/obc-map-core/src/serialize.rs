//! Byte framing shared by independent map producers.

use crate::config::LineStyle;
use obc_formats::obcm::{
    nav_index_padding, OffsetScale, UnitWriter, CHUNK_END, FEATURE_HEADER_COMPACT_LEN, HEADER_LEN, LOD_ENTRY_LEN,
    MAGIC, NAV_CHUNK_SIZE, NAV_DIR_LEN, NAV_MAX_PROFILES, NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN,
    NAV_PROFILE_RESERVED_LEN, POI_HOURS_BLOB_LEN, STYLE_DASHED_BIT, STYLE_FIXED_WIDTH_BIT, STYLE_HAS_COLOR2_BIT,
    STYLE_PRIORITY_MASK, STYLE_RECORD_LEN, STYLE_TERRAIN_LAYER_BIT, STYLE_TICKED_BIT, VERSION as OBCM_VERSION,
};
use std::convert::Infallible;
use std::io::{self, Seek, SeekFrom, Write};

/// Largest coordinate delta before inserting intermediate wire points.
pub const MAX_SEGMENT: i64 = 30_000;

/// Append intermediate points between `p1` and `p2` (then `p2`) so no single (dx, dy) step exceeds
/// the 16-bit delta range, using an integer step count and banker's-rounded midpoints.
pub fn densify(p1: (i64, i64), p2: (i64, i64), out: &mut Vec<(i64, i64)>) {
    let dx = p2.0 - p1.0;
    let dy = p2.1 - p1.1;
    let max_dist = dx.abs().max(dy.abs());
    if max_dist > MAX_SEGMENT {
        let steps = max_dist / MAX_SEGMENT + 1;
        for step in 1..steps {
            let t = step as f64 / steps as f64;
            out.push((
                (p1.0 as f64 + dx as f64 * t).round_ties_even() as i64,
                (p1.1 as f64 + dy as f64 * t).round_ties_even() as i64,
            ));
        }
    }
    out.push(p2);
}

/// The `Offset Scale` every `.obcm` this packer writes carries: `U = 16`, a 64 GiB addressable
/// interior. A constant rather than a knob, which pins the byte for determinism.
pub const SCALE: OffsetScale = OffsetScale::DEFAULT;

/// The next unit boundary at or after `cursor`. Every structure a header or directory offset
/// reaches begins on one; the bytes this rounds past are [`obc_formats::obcm::FILLER`].
///
/// The writers below reach their boundaries through [`UnitWriter::begin_section`]. This spelling is
/// for the two places that need the boundary without having a cursor there.
#[inline]
pub fn align_up(cursor: usize) -> usize {
    SCALE.align_up(cursor as u64).expect("a layout cursor never approaches u64::MAX") as usize
}

/// The `uint32` a scaled offset field stores for byte offset `at`.
///
/// A scaled offset cannot name a byte that is not a multiple of `U`, so a non-boundary argument is a
/// bug in the layout above it, not a rounding request — hence the panic rather than a silent round.
#[inline]
pub fn scaled(at: usize) -> u32 {
    SCALE
        .scaled(at as u64)
        .unwrap_or_else(|| panic!("byte {at} is not on a {}-byte unit boundary", SCALE.unit()))
        .units()
}

/// Lay bytes out through a [`UnitWriter`] over an in-memory buffer. `at` is the absolute file byte
/// the buffer's first byte lands on, so the cursor finds the boundaries where the reader will look
/// for them rather than where the buffer happens to start.
pub fn lay_out<T>(
    at: usize,
    build: impl FnOnce(&mut UnitWriter<'_, Infallible>) -> Result<T, Infallible>,
) -> (Vec<u8>, T) {
    let mut buf: Vec<u8> = Vec::new();
    let value = {
        let mut sink = |bytes: &[u8]| -> Result<(), Infallible> {
            buf.extend_from_slice(bytes);
            Ok(())
        };
        match build(&mut UnitWriter::new(SCALE, at as u64, &mut sink)) {
            Ok(value) => value,
            Err(never) => match never {},
        }
    };
    (buf, value)
}

/// Walk a layout with a sink that keeps nothing but the cursor: the projection of a section, run
/// through the very code that emits it. `place` sees exactly the byte lengths the write will, so
/// what it reports and what gets written cannot be two different layouts.
pub fn place<T>(at: usize, walk: impl FnOnce(&mut UnitWriter<'_, Infallible>) -> Result<T, Infallible>) -> T {
    let mut discard = |_: &[u8]| -> Result<(), Infallible> { Ok(()) };
    match walk(&mut UnitWriter::new(SCALE, at as u64, &mut discard)) {
        Ok(value) => value,
        Err(never) => match never {},
    }
}

/// A per-map routing profile ready to serialize: a display name plus the two multiplier tables in
/// `u8` fixed-point 1/16, indexed by highway class and surface class (`16` = 1.0x, `0` = forbidden).
/// Built and validated in [`crate::config`], where every non-zero multiplier is at least 16 so the
/// great-circle A* heuristic stays admissible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavProfile {
    /// Display name (UTF-8), truncated to [`NAV_PROFILE_NAME_LEN`] bytes on write, `0xFF`-padded.
    pub name: String,
    /// Multiplier per highway class (5-bit index, 0..=31). `16` = 1.0×, `0` = forbidden.
    pub highway: [u8; 32],
    /// Multiplier per surface class (3-bit index, 0..=7). Same encoding.
    pub surface: [u8; 8],
    /// Flat metres charged per metre of a neighbor entry's `Ascent M`. `0` = climb-blind. Needs no
    /// admissibility bound: the term is additive and non-negative.
    pub climb_weight: u8,
}

/// Largest `chunk_size` (bytes) that keeps every feature within the reader's
/// [`obc_reader::MAX_FEAT_PTS`] vertex cap. A feature's packed bytes are at least
/// `FEATURE_HEADER_COMPACT_LEN + 2 * (total_vertices - 1)`, so a chunk of `chunk_size` bytes carries
/// at most `(chunk_size - 7) / 2 + 1` vertices. Above this the reader silently truncates past-cap
/// vertices and the feature's fill or stroke is corrupt.
pub const MAX_SAFE_CHUNK_SIZE: usize = (obc_reader::MAX_FEAT_PTS - 1) * 2 + FEATURE_HEADER_COMPACT_LEN;

// The safe ceiling must itself fit the on-wire `u16` chunk_size field, or the bound is moot.
const _: () = assert!(MAX_SAFE_CHUNK_SIZE <= u16::MAX as usize, "chunk_size is a u16 in the format");

/// Smallest accepted `chunk_size` (bytes). The format decodes any positive size, but below this
/// even modest features exceed the chunk and `pack_chunk` drops them wholesale, so the pack
/// "succeeds" and the map is silently near-empty. Kept in lock-step with the schema's
/// `chunk_size.minimum`.
pub const MIN_CHUNK_SIZE: usize = 256;

/// Reject a `chunk_size` outside [`MIN_CHUNK_SIZE`]..=[`MAX_SAFE_CHUNK_SIZE`]: above the max the
/// reader silently truncates vertices, below the min features get dropped wholesale.
pub fn validate_chunk_size(chunk_size: usize) -> Result<(), String> {
    if chunk_size > MAX_SAFE_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} exceeds the safe maximum {MAX_SAFE_CHUNK_SIZE}: a single feature \
             could then pack more than {} vertices, which the device reader silently truncates \
             (issue #2). Lower chunk_size, or raise the LOD's simplify tolerance.",
            obc_reader::MAX_FEAT_PTS
        ));
    }
    if chunk_size < MIN_CHUNK_SIZE {
        return Err(format!(
            "chunk_size {chunk_size} is below the minimum {MIN_CHUNK_SIZE}: features larger than the \
             chunk are dropped at pack time, so a tiny chunk_size produces a mostly-empty map."
        ));
    }
    Ok(())
}

/// A style record as packed into the Style Table (`pack_style_dict`).
#[derive(Debug, Clone, Copy)]
pub struct Style {
    pub id: u8,
    pub z_index: i8,
    pub color: u16,
    pub weight: u8,
    /// Priority 1..=4; clamped to that range on pack.
    pub priority: u8,
    /// Line stroke style. Polygons ignore it.
    pub line_style: LineStyle,
    /// Optional RGB565 secondary color. `None` clears the flag bit and writes `0x0000`, which the
    /// reader ignores — black is a legit color, not a sentinel.
    pub color2: Option<u16>,
    /// Fixed width: `weight` is device pixels, off the renderer's zoom ramp.
    pub fixed_width: bool,
    /// Terrain layer: written here, consumed by the device's Settings toggle.
    pub terrain_layer: bool,
}

/// Pack the style table: `Count(u8)` then one record per style, sorted by id. Bit 7 of the flags
/// stays reserved and is written `0`.
pub fn pack_style_dict(styles: &[Style]) -> Vec<u8> {
    let mut styles = styles.to_vec();
    styles.sort_by_key(|s| s.id);
    let mut data = Vec::with_capacity(1 + styles.len() * STYLE_RECORD_LEN);
    data.push(styles.len() as u8);
    for s in &styles {
        let priority = (s.priority as i32).clamp(1, 4);
        let mut flags = (priority - 1) as u8 & STYLE_PRIORITY_MASK;
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
        data.push(s.id);
        data.push(s.z_index as u8);
        data.extend_from_slice(&s.color.to_le_bytes());
        data.push(s.weight);
        data.push(flags);
        data.extend_from_slice(&s.color2.unwrap_or(0).to_le_bytes());
    }
    data
}

/// Pack the profile table: one 56-byte record per profile. The name is UTF-8 truncated and
/// `0xFF`-padded (the POI-name convention); the reserved tail is zero, not `0xFF`, because it is a
/// reserved field and not a padded string.
pub fn pack_profile_table(profiles: &[NavProfile]) -> Vec<u8> {
    debug_assert!((1..=NAV_MAX_PROFILES).contains(&profiles.len()), "1..=8 profiles");
    let mut out = Vec::with_capacity(profiles.len() * NAV_PROFILE_LEN);
    for p in profiles {
        let name = p.name.as_bytes();
        let n = name.len().min(NAV_PROFILE_NAME_LEN);
        out.extend_from_slice(&name[..n]);
        out.resize(out.len() + (NAV_PROFILE_NAME_LEN - n), CHUNK_END); // 0xFF-pad the name field
        out.extend_from_slice(&p.highway);
        out.extend_from_slice(&p.surface);
        out.push(p.climb_weight);
        out.resize(out.len() + NAV_PROFILE_RESERVED_LEN, 0);
    }
    debug_assert_eq!(out.len(), profiles.len() * NAV_PROFILE_LEN);
    out
}

/// The regions the nav directory has to name, as [`walk_nav_section`] found them.
struct NavOffsets {
    profile_table_offset: usize,
    index_offset: usize,
    edge_pool_offset: usize,
    snap_index_offset: usize,
}

/// Everything behind a populated graph's profile table, already in wire form.
pub struct NavBody<'a> {
    pub index: &'a [u8],
    pub node_count: u32,
    pub chunks: &'a [u8],
    pub chunk_count: u32,
    pub pool: &'a [u8],
    pub edge_chunk_count: u32,
    pub snap_index: &'a [u8],
    pub snap_node_count: u32,
    pub snap_chunks: &'a [u8],
    pub snap_chunk_count: u32,
}

/// Walk the nav section through `w`, returning the offsets its directory has to state.
///
/// This runs twice and it is the same walk both times: once over a sink that keeps nothing but the
/// cursor, to find the offsets the 40-byte directory carries, and once over the real buffer with
/// that directory in hand. A projection and an emission that were two pieces of code could disagree;
/// two runs of one piece cannot. It is affordable here because the whole body is already resident.
fn walk_nav_section<E>(
    w: &mut UnitWriter<'_, E>,
    directory: &[u8],
    profile_table: &[u8],
    body: Option<&NavBody<'_>>,
) -> Result<NavOffsets, E> {
    debug_assert_eq!(directory.len(), NAV_DIR_LEN);
    // The profile table sits behind the 40-byte directory, at the first unit boundary past it, so
    // the bytes between them are filler.
    w.put(directory)?;
    let profile_table_offset = w.begin_section()? as usize;
    w.put(profile_table)?;
    let Some(b) = body else {
        // Empty graph: the directory and the always-present profile table are the whole section.
        // The zero-length regions still have to be nameable, so all three point at the first unit
        // boundary past the table rather than at its last byte.
        let at = w.begin_section()? as usize;
        return Ok(NavOffsets { profile_table_offset, index_offset: at, edge_pool_offset: at, snap_index_offset: at });
    };

    // `nav_index_padding` chooses each alignment run so that two things hold at once: the index
    // starts on a unit boundary (or no scaled offset could name it), and the fixed 512-byte chunks
    // behind it start on a sector boundary, so a full-chunk read is one card command. The edge pool
    // stays sector-aligned because the node region is whole 512-byte chunks.
    //
    // Every gap here is `0xFF`: one fill byte, one rule — a gap is `0xFF` and a reserved field
    // is `0`.
    w.pad(index_pad(w.at(), b.index.len() as u64))?;
    let index_offset = w.at() as usize;
    w.put(b.index)?;
    w.begin_section()?;
    w.put(b.chunks)?;
    let edge_pool_offset = w.at() as usize;
    w.put(b.pool)?;
    // A snap index of no nodes has no sector to reconcile, so its region only has to be nameable.
    if b.snap_node_count == 0 {
        w.begin_section()?;
    } else {
        w.pad(index_pad(w.at(), b.snap_index.len() as u64))?;
    }
    let snap_index_offset = w.at() as usize;
    w.put(b.snap_index)?;
    w.begin_section()?;
    w.put(b.snap_chunks)?;
    debug_assert_eq!(w.at(), align_up(w.at() as usize) as u64, "the file tail stays aligned");
    Ok(NavOffsets { profile_table_offset, index_offset, edge_pool_offset, snap_index_offset })
}

/// The alignment run before a quadtree index of `index_len` bytes starting, unpadded, at `at`.
#[inline]
fn index_pad(at: u64, index_len: u64) -> u64 {
    nav_index_padding(SCALE, at, index_len).expect("a nav index length never approaches u64::MAX") as u64
}

/// The 40-byte nav directory, over the offsets [`walk_nav_section`]'s first pass resolved.
fn nav_directory(offsets: &NavOffsets, body: Option<&NavBody<'_>>, profile_count: usize) -> Vec<u8> {
    let (node_count, node_chunks, edge_chunks, snap_node_count, snap_chunks) = match body {
        Some(b) => (b.node_count, b.chunk_count, b.edge_chunk_count, b.snap_node_count, b.snap_chunk_count),
        None => (0, 0, 0, 0, 0),
    };
    let mut dir = Vec::with_capacity(NAV_DIR_LEN);
    dir.extend_from_slice(&scaled(offsets.index_offset).to_le_bytes());
    dir.extend_from_slice(&node_count.to_le_bytes());
    dir.extend_from_slice(&node_chunks.to_le_bytes());
    dir.extend_from_slice(&scaled(offsets.edge_pool_offset).to_le_bytes());
    dir.extend_from_slice(&edge_chunks.to_le_bytes());
    dir.extend_from_slice(&(NAV_CHUNK_SIZE as u16).to_le_bytes()); // chunk_size (pinned 512)
    dir.extend_from_slice(&scaled(offsets.profile_table_offset).to_le_bytes());
    dir.push(profile_count as u8);
    dir.push(0u8); // reserved — a field, so `0`, unlike a gap
    dir.extend_from_slice(&scaled(offsets.snap_index_offset).to_le_bytes());
    dir.extend_from_slice(&snap_node_count.to_le_bytes());
    dir.extend_from_slice(&snap_chunks.to_le_bytes());
    debug_assert_eq!(dir.len(), NAV_DIR_LEN);
    dir
}

/// Lay the section out at `section_offset`: the placeholder walk that resolves the directory's
/// offsets, then the identical walk that writes the bytes.
pub fn emit_nav_section(
    section_offset: usize,
    profile_table: &[u8],
    body: Option<&NavBody<'_>>,
    profiles: usize,
) -> Vec<u8> {
    let placeholder = [0u8; NAV_DIR_LEN];
    let offsets = place(section_offset, |w| walk_nav_section(w, &placeholder, profile_table, body));
    let directory = nav_directory(&offsets, body, profiles);
    let (out, _) = lay_out(section_offset, |w| walk_nav_section(w, &directory, profile_table, body));
    out
}

/// The byte offset of the style table in every file this packer writes: the first unit boundary at
/// or after the header, which at the default `U = 16` is `80`. Reading the field rather than
/// assuming the table follows the header is what it was always for.
pub const STYLE_OFFSET: usize = 80;
// 80 is a derivation with two halves, and both are asserted.
const _: () = assert!(STYLE_OFFSET >= HEADER_LEN, "the style table cannot start inside the header");
const _: () = assert!(
    (STYLE_OFFSET as u64).is_multiple_of(SCALE.unit()),
    "and it must be a unit boundary a scaled offset can name"
);
const _: () =
    assert!(((STYLE_OFFSET - HEADER_LEN) as u64) < SCALE.unit(), "…the *first* such boundary, so the gap is one unit");

/// The OBCM header. Every offset field here is scaled, and both serializers share it.
///
/// `obc-pack` writes a map with no embedded terrain, so the terrain pair is `(0, 0)`: unambiguous
/// absence, since the header occupies byte `0` and no region can begin there. The producer of a
/// terrain region is `obcm-assemble`, which is what splices the catalog's terrain cells. `--terrain`
/// on this side is an `ElevationSource` for the per-edge climb, and never a carried raster.
pub fn header_bytes(
    lod_count: usize,
    marker_color: u16,
    global_bbox: (i64, i64, i64, i64),
    lod_table_offset: usize,
    poi_section_offset: usize,
    nav_section_offset: usize,
    dark_style_offset: usize,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN);
    out.extend_from_slice(&MAGIC);
    out.push(OBCM_VERSION);
    out.extend_from_slice(&(global_bbox.1 as i32).to_le_bytes()); // min_lat
    out.extend_from_slice(&(global_bbox.0 as i32).to_le_bytes()); // min_lon
    out.extend_from_slice(&(global_bbox.3 as i32).to_le_bytes()); // max_lat
    out.extend_from_slice(&(global_bbox.2 as i32).to_le_bytes()); // max_lon
    out.extend_from_slice(&scaled(STYLE_OFFSET).to_le_bytes());
    out.push(lod_count as u8);
    out.extend_from_slice(&scaled(lod_table_offset).to_le_bytes());
    out.extend_from_slice(&marker_color.to_le_bytes());
    out.extend_from_slice(&scaled(poi_section_offset).to_le_bytes());
    out.extend_from_slice(&scaled(nav_section_offset).to_le_bytes());
    out.push(SCALE.log2());
    out.extend_from_slice(&0u32.to_le_bytes()); // terrain offset — no embedded raster
    out.extend_from_slice(&0u32.to_le_bytes()); // terrain length, `0` exactly when the offset is
    out.extend_from_slice(&[0; 16]); // optional landmark and peak sections
    out.extend_from_slice(&scaled(dark_style_offset).to_le_bytes());
    out.extend_from_slice(&marker_color.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN);
    out
}

/// Where the file's fixed prefix puts the LOD table and the first LOD's index: each is named by a
/// scaled offset, so each begins on the first unit boundary past the structure before it. Both
/// serializers need these before they write the header that states them, which is why this is
/// arithmetic rather than a cursor.
pub fn prefix_offsets(style_len: usize, lod_count: usize) -> (usize, usize) {
    let lod_table_offset = align_up(STYLE_OFFSET + style_len);
    (lod_table_offset, align_up(lod_table_offset + lod_count * LOD_ENTRY_LEN))
}

/// Append one LOD-table entry. `None` max_mpp is `+inf`, the coarsest layer; `cs` is the chunk
/// capacity bound rather than a stride.
pub fn push_lod_entry(table: &mut Vec<u8>, max_mpp: Option<f64>, index_offset: u32, nc: u32, cs: usize, cc: u32) {
    let mpp_f: f32 = max_mpp.map_or(f32::INFINITY, |v| v as f32);
    table.extend_from_slice(&mpp_f.to_le_bytes());
    table.extend_from_slice(&index_offset.to_le_bytes());
    table.extend_from_slice(&nc.to_le_bytes());
    table.extend_from_slice(&(cs as u16).to_le_bytes());
    table.extend_from_slice(&cc.to_le_bytes());
}

/// The one producer rule: the scale must cover the file it writes. A file whose bytes reach past
/// what its scale can address is malformed, and the producer that laid it out is the only party
/// positioned to notice — a reader that never resolves the last section never sees anything wrong.
pub fn check_scale_covers(total: u64) {
    assert!(
        SCALE.covers(total),
        "a {total}-byte map does not fit the {}-byte-unit interior this packer writes (§1.1)",
        SCALE.unit()
    );
}

/// One encoded drawing level. Counts describe the index and chunk buffers.
pub struct LodBytes<'a> {
    pub index: &'a [u8],
    pub nodes: u32,
    pub chunks: &'a [u8],
    pub chunk_count: u32,
}

/// Streaming file framing. Producers encode one level at a time and supply the section bytes.
pub struct MapWriter<W> {
    output: W,
    cursor: u64,
    styles: Vec<u8>,
    table: Vec<u8>,
    table_offset: usize,
    levels: usize,
}

impl<W: Write + Seek> MapWriter<W> {
    pub fn new(
        output: W,
        levels: usize,
        styles: &[Style],
        marker: u16,
        bounds: (i64, i64, i64, i64),
    ) -> io::Result<Self> {
        let styles = pack_style_dict(styles);
        let (table_offset, payload_start) = prefix_offsets(styles.len(), levels);
        let mut writer =
            Self { output, cursor: 0, styles, table: Vec::with_capacity(levels * LOD_ENTRY_LEN), table_offset, levels };
        let header = header_bytes(levels, marker, bounds, table_offset, STYLE_OFFSET, STYLE_OFFSET, STYLE_OFFSET);
        // Keep the borrowed style bytes outside the mutable writer walk.
        let styles = std::mem::take(&mut writer.styles);
        writer.emit(|w| {
            w.put(&header)?;
            let at = w.begin_section()?;
            debug_assert_eq!(at, STYLE_OFFSET as u64);
            w.put(&styles)?;
            let at = w.begin_section()?;
            debug_assert_eq!(at, table_offset as u64);
            w.put(&vec![0; levels * LOD_ENTRY_LEN])?;
            let at = w.begin_section()?;
            debug_assert_eq!(at, payload_start as u64);
            Ok(())
        })?;
        writer.styles = styles;
        Ok(writer)
    }

    fn emit(&mut self, bytes: impl FnOnce(&mut UnitWriter<'_, io::Error>) -> io::Result<()>) -> io::Result<()> {
        let mut sink = |bytes: &[u8]| self.output.write_all(bytes);
        let mut w = UnitWriter::new(SCALE, self.cursor, &mut sink);
        bytes(&mut w)?;
        self.cursor = w.at();
        Ok(())
    }

    pub fn position(&self) -> usize {
        self.cursor as usize
    }

    pub fn lod(&mut self, chunk_size: usize, max_mpp: Option<f64>, bytes: Option<LodBytes<'_>>) -> io::Result<()> {
        assert!(self.table.len() < self.levels * LOD_ENTRY_LEN, "all declared levels are already written");
        let (nodes, chunks) = bytes.as_ref().map_or((0, 0), |b| (b.nodes, b.chunk_count));
        push_lod_entry(&mut self.table, max_mpp, scaled(self.cursor as usize), nodes, chunk_size, chunks);
        self.emit(|w| match bytes {
            Some(bytes) => {
                w.put(bytes.index)?;
                w.put(bytes.chunks)
            }
            None => {
                w.put(&0u32.to_le_bytes())?;
                w.begin_section()?;
                Ok(())
            }
        })
    }

    pub fn finish(mut self, poi: &[u8], nav: &[u8], landmarks: &[u8], peaks: &[u8]) -> io::Result<u64> {
        assert_eq!(self.table.len(), self.levels * LOD_ENTRY_LEN, "every declared level must be written");
        let poi_offset = self.position();
        let nav_offset = poi_offset + poi.len();
        let mut landmark = (0, 0);
        let mut peak = (0, 0);
        let mut dark_style = 0;
        let styles = std::mem::take(&mut self.styles);
        self.emit(|w| {
            w.put(poi)?;
            w.put(nav)?;
            for (bytes, section) in [(landmarks, &mut landmark), (peaks, &mut peak)] {
                if !bytes.is_empty() {
                    let start = w.begin_section()? as usize;
                    w.put(bytes)?;
                    *section = (start, w.begin_section()? as usize - start);
                }
            }
            dark_style = w.begin_section()? as usize;
            w.put(&styles)
        })?;
        check_scale_covers(self.cursor);
        self.output.seek(SeekFrom::Start(self.table_offset as u64))?;
        self.output.write_all(&self.table)?;
        self.output.seek(SeekFrom::Start(32))?;
        self.output.write_all(&scaled(poi_offset).to_le_bytes())?;
        self.output.write_all(&scaled(nav_offset).to_le_bytes())?;
        for (offset, (start, length)) in [
            (obc_formats::obcm::HEADER_LANDMARK_OFFSET_OFF, landmark),
            (obc_formats::obcm::HEADER_PEAK_OFFSET_OFF, peak),
        ] {
            self.output.seek(SeekFrom::Start(offset as u64))?;
            self.output.write_all(&scaled(start).to_le_bytes())?;
            self.output.write_all(&scaled(length).to_le_bytes())?;
        }
        self.output.seek(SeekFrom::Start(obc_formats::obcm::HEADER_DARK_STYLE_OFFSET_OFF as u64))?;
        self.output.write_all(&scaled(dark_style).to_le_bytes())?;
        self.output.seek(SeekFrom::Start(self.cursor))?;
        Ok(self.cursor)
    }
}

/// Pack the hours-pool section: `count u16` then the blobs back to back. An empty pool is just the
/// `0` count.
pub fn pack_hours_pool(pool: &[[u8; POI_HOURS_BLOB_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + pool.len() * POI_HOURS_BLOB_LEN);
    out.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    for blob in pool {
        out.extend_from_slice(blob);
    }
    out
}

/// Encoded category bytes, with counts for the POI directory.
pub struct PoiBytes {
    pub cat_id: u8,
    pub index: Vec<u8>,
    pub node_count: u32,
    pub chunks: Vec<u8>,
    pub chunk_count: u32,
}

/// Frame encoded POI category blocks and their shared hours pool.
pub fn emit_poi_section(section_offset: usize, blocks: &[PoiBytes], pool: &[[u8; POI_HOURS_BLOB_LEN]]) -> Vec<u8> {
    use obc_formats::obcm::{POI_CAT_ENTRY_LEN, POI_CHUNK_SIZE};
    // Directory size: count byte, chunk_size u16, one entry per category, and the hours-pool
    // offset and count.
    let dir_len = 1 + 2 + blocks.len() * POI_CAT_ENTRY_LEN + 4 + 2;

    // Categories are laid out sequentially after the directory: [index][filler][chunks] each, with
    // empties contributing nothing but their directory entry. Every `Index Offset` is scaled, so
    // each index starts on a unit boundary. 512 is a multiple of `U` at every legal scale, so the
    // chunks need no filler between them and the region ends aligned for the next category, which
    // is why every `begin_section` in the loop below is a no-op after the first.
    //
    // The cursor starts past the directory, because the directory's own bytes cannot be written
    // until this walk has resolved the offsets they carry.
    let (payload, (cat_entries, hours_pool_offset)) = lay_out(section_offset + dir_len, |w| {
        let mut cat_entries = Vec::with_capacity(blocks.len());
        for b in blocks {
            cat_entries.push((b.cat_id, scaled(w.begin_section()? as usize), b.node_count, b.chunk_count));
            w.put(&b.index)?;
            w.begin_section()?;
            w.put(&b.chunks)?;
        }
        // The hours pool, then the run that leaves the nav directory behind it nameable.
        let hours_pool_offset = w.begin_section()? as usize;
        w.put(&pack_hours_pool(pool))?;
        w.begin_section()?;
        Ok((cat_entries, hours_pool_offset))
    });

    let mut out = Vec::with_capacity(dir_len + payload.len());
    out.push(blocks.len() as u8);
    out.extend_from_slice(&(POI_CHUNK_SIZE as u16).to_le_bytes());
    for (cat_id, index_offset, node_count, chunk_count) in cat_entries {
        out.push(cat_id);
        out.extend_from_slice(&index_offset.to_le_bytes());
        out.extend_from_slice(&node_count.to_le_bytes());
        out.extend_from_slice(&chunk_count.to_le_bytes());
    }
    out.extend_from_slice(&scaled(hours_pool_offset).to_le_bytes());
    out.extend_from_slice(&(pool.len() as u16).to_le_bytes());
    debug_assert_eq!(out.len(), dir_len);
    out.extend_from_slice(&payload);
    out
}

/// The same POI directory and empty hours pool as a map with no place records.
pub fn empty_poi_section(section_offset: usize) -> Vec<u8> {
    let blocks: Vec<_> = obc_formats::obcm::PoiCategory::ALL
        .iter()
        .map(|category| PoiBytes {
            cat_id: category.id(),
            index: Vec::new(),
            node_count: 0,
            chunks: Vec::new(),
            chunk_count: 0,
        })
        .collect();
    emit_poi_section(section_offset, &blocks, &[])
}
