//! The terrain region of an assembly: downloaded OBCT cells in, one container out, written into
//! the tail of the map file rather than beside it.
//!
//! A published terrain cell is already in its final form — a `1 × 1` OBCT container whose block is
//! the raster for exactly one OBCA square — so assembling terrain is writing a wider directory over
//! the assembly rectangle and copying each block into the slot its id names. There is no geometry
//! to relocate and no seam to unify: the lattice is global and half-open, so two neighbouring cells
//! already agree about every sample.
//!
//! What that leaves is bookkeeping with teeth. The rectangle is the assembly bbox, expressed in
//! terrain cells; an absent or known-empty square is directory `0`, which OBCT makes
//! indistinguishable from an all-`NODATA` block; every input is checked against the catalog before
//! its bytes are copied; and the finished region is read back through
//! [`obc_elevation::TerrainReader`], the same parser the firmware runs, with every present block
//! compared against the object the catalog served.
//!
//! The three stages are separate because the map writer needs them at three different moments.
//! [`TerrainRegion::prepare`] does every check and settles the byte length before the layout,
//! because the region pointer lives in the header and a header is the first thing written.
//! [`TerrainRegion::emit`] streams the container into the map's tail with no seek.
//! [`TerrainRegion::verify`] runs after the file is sealed, on the window the header now names.
//!
//! The container's header and directory come from [`obc_dem::container::container_prefix`], the
//! same module whose [`obc_dem::container::ShardWriter`] the bakery bakes cells with, which is what
//! makes "a cell is a 1 × 1 shard" a fact rather than a claim.

use obc_dem::container::{container_prefix_with_surface, fill_cell_index, CellRect};
use obc_elevation::TerrainReader;
use obc_formats::io::ByteSource;
use obc_formats::obct::{cell_block_len, cell_samples_log2, SurfaceLayout, DIR_ENTRY_LEN, HEADER_LEN, SURFACE_FLAG};
use sha2::{Digest, Sha256};

use crate::grid::{AlignedBox, CellId, GRID_ORIGIN};
use crate::{Error, Result};

/// The lattice a terrain store is published at, minus the parts an assembler has no opinion about.
/// Every input cell's own header must state exactly this, and so must the shard's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainParams {
    /// `log2(P)` of the sample posting in µdeg.
    pub posting_log2: u8,
    /// `log2(S)` of the terrain cell side in µdeg. Independent of any band's cell size.
    pub cell_log2: u8,
}

/// One downloaded terrain cell, as the catalog names it: the square it covers, its bytes, and the
/// digest the index published for it.
pub struct TerrainCellInput<'a> {
    /// The cell id on the **terrain** grid, `<cell_log2>/<i>/<j>`.
    pub id: CellId,
    /// The whole `.obcd` container as published — header, `1 × 1` directory, block.
    pub src: &'a dyn ByteSource,
    /// The `sha256` the pinned terrain index carries for this object. `None` only for a caller with
    /// no catalog, which then gets every structural check and no provenance one.
    pub sha256: Option<[u8; 32]>,
}

/// The rectangle a terrain shard covers, and the cells that fill it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainPlan {
    pub params: TerrainParams,
    pub rect: CellRect,
}

impl TerrainPlan {
    /// The terrain rectangle over an assembly bbox.
    ///
    /// The assembly bbox is a grid-aligned power-of-two square whose corner is congruent to
    /// `GRID_ORIGIN` modulo `S_MAX`. A terrain cell no larger than that square therefore tiles it
    /// exactly, so the rectangle is the assembly bbox to the microdegree.
    ///
    /// A terrain cell larger than the assembly square is refused rather than accommodated: the
    /// rectangle would overhang the assembly bbox, and growing the assembly box to fit a raster is
    /// forbidden. At the current pairing this cannot happen; a schema that made it possible is a
    /// configuration to fix.
    pub fn over(params: TerrainParams, assembly: AlignedBox) -> Result<TerrainPlan> {
        cell_samples_log2(params.posting_log2, params.cell_log2).ok_or_else(|| {
            Error::Input(format!(
                "terrain posting 2^{} µdeg with cell 2^{} µdeg is not a pairing OBCT permits (OBCT §4.5)",
                params.posting_log2, params.cell_log2
            ))
        })?;
        let cell_log2 = params.cell_log2 as u32;
        if cell_log2 > assembly.span_log2 {
            return Err(Error::Input(format!(
                "the terrain cell is 2^{cell_log2} µdeg but the assembly bbox is only 2^{} µdeg across: one terrain \
                 square would overhang the map, and OBCA §4.2 forbids growing the assembly box to fit it",
                assembly.span_log2
            )));
        }
        // Fitting inside the box is not the same as tiling it. The assembly corner is snapped to
        // `S_MAX`, the largest band's cell, and the span may be several times that — so a terrain
        // cell larger than `S_MAX` passes the span check above and still lands off the corner,
        // which would put the whole raster half a cell from the ground it describes.
        if !crate::grid::on_grid_line(assembly.min_lat, cell_log2)
            || !crate::grid::on_grid_line(assembly.min_lon, cell_log2)
        {
            return Err(Error::Input(format!(
                "the assembly corner ({}, {}) is not on the 2^{cell_log2} µdeg terrain grid: the box is snapped to the \
                 schema's largest band cell (OBCA §4.2), so a terrain cell larger than that does not tile it",
                assembly.min_lat, assembly.min_lon
            )));
        }
        let side = 1i64 << cell_log2;
        let min_i = (assembly.min_lat - GRID_ORIGIN) / side;
        let min_j = (assembly.min_lon - GRID_ORIGIN) / side;
        let span = 1u64 << (assembly.span_log2 - cell_log2);
        let axis = u16::try_from(span).map_err(|_| {
            Error::Capacity(format!(
                "an assembly of 2^{} µdeg needs {span} terrain cells per axis, past the uint16 the OBCT cell \
                 rectangle is made of",
                assembly.span_log2
            ))
        })?;
        Ok(TerrainPlan { params, rect: CellRect { min_i: min_i as u32, min_j: min_j as u32, rows: axis, cols: axis } })
    }

    /// The shard's exact byte length, computed from the rectangle and the cell count alone, with
    /// nothing fetched and nothing written. It is the pre-download projection the builder shows and
    /// the number the write is checked against, so the two are one claim rather than two
    /// estimates.
    pub fn projected_bytes(&self, present_cells: u64) -> u64 {
        let block = cell_block_len(self.params.posting_log2, self.params.cell_log2).unwrap_or(0) as u64;
        HEADER_LEN as u64 + self.rect.slots() * DIR_ENTRY_LEN as u64 + present_cells * block
    }

    /// The shard's bbox in µdeg, `(min_lon, min_lat, max_lon, max_lat)` — equal to the assembly
    /// bbox by [`TerrainPlan::over`]'s construction, and asserted as such by the caller.
    pub fn ubox(&self) -> (i64, i64, i64, i64) {
        let side = 1i64 << self.params.cell_log2;
        let min_lat = GRID_ORIGIN + self.rect.min_i as i64 * side;
        let min_lon = GRID_ORIGIN + self.rect.min_j as i64 * side;
        (min_lon, min_lat, min_lon + self.rect.cols as i64 * side, min_lat + self.rect.rows as i64 * side)
    }
}

/// Where a present cell's block lives inside a published `1 × 1` container: straight after the
/// 32-byte header and its single directory entry.
struct CellLayout {
    offset: u32,
    bytes: u32,
    surface: bool,
}

/// Check one published cell against the catalog and the lattice, and return its block offset.
/// Everything here is a property of the downloaded bytes, so it runs before a single one is copied:
/// a bad cell must never reach the shard, not even to be caught on the way out.
fn check_cell(cell: &TerrainCellInput<'_>, params: TerrainParams) -> Result<CellLayout> {
    let bad = |what: String| Error::Format(format!("terrain cell {}: {what}", cell.id));

    // The container itself, through the real reader: magic, version, flags, the posting/cell
    // pairing, the rectangle against the world grid, and every directory entry against the file's
    // own length. A truncated download fails here rather than as a short read half a megabyte into
    // the copy.
    let reader = TerrainReader::parse(cell.src).map_err(|e| bad(format!("not a usable OBCT container ({e:?})")))?;
    let header = reader.header();
    if header.posting_log2 != params.posting_log2 || header.cell_log2 != params.cell_log2 {
        return Err(bad(format!(
            "is posting 2^{} / cell 2^{} µdeg, but the catalog's terrain block says posting 2^{} / cell 2^{} — one \
             assembly is one lattice (OBCC §13.2)",
            header.posting_log2, header.cell_log2, params.posting_log2, params.cell_log2
        )));
    }
    // A published cell is a 1 × 1 container at exactly its own id. A wider rectangle is a shard,
    // and a 1 × 1 at some other square is a cell filed under the wrong name — either would place a
    // raster over ground it is not.
    if header.cell_rows != 1 || header.cell_cols != 1 {
        return Err(bad(format!(
            "is a {}×{} rectangle; a published cell is 1 × 1 (OBCC §13.1) and a wider one is a shard",
            header.cell_rows, header.cell_cols
        )));
    }
    if header.cell_min_i as i64 != cell.id.i || header.cell_min_j as i64 != cell.id.j {
        return Err(bad(format!(
            "covers square {}/{} but the catalog files it under {}/{}",
            header.cell_min_i, header.cell_min_j, cell.id.i, cell.id.j
        )));
    }

    // The digest the catalog published, over the object as downloaded. This is the one check that
    // says the content is what was promised rather than merely well-formed.
    if let Some(expected) = cell.sha256 {
        let mut hasher = Sha256::new();
        let mut cursor = 0u64;
        let mut buf = [0u8; 8192];
        let total = cell.src.len();
        while cursor < total {
            let n = ((total - cursor).min(buf.len() as u64)) as usize;
            cell.src.read_at(cursor, &mut buf[..n]).map_err(Error::Io)?;
            hasher.update(&buf[..n]);
            cursor += n as u64;
        }
        let actual: [u8; 32] = hasher.finalize().into();
        if actual != expected {
            return Err(bad(format!(
                "digest mismatch — the catalog pins {} and the {} downloaded bytes hash to {}",
                hex(&expected),
                total,
                hex(&actual)
            )));
        }
    }

    // The block has to be wholly inside the object. `TerrainReader::parse` already asserted it for
    // the directory entry it read; restated here because this is where the copy's bounds come from.
    let surface = header.flags & SURFACE_FLAG != 0;
    let block_len = if surface {
        SurfaceLayout::new(params.posting_log2, params.cell_log2)
            .ok_or_else(|| bad("invalid surface layout".into()))?
            .cell_bytes()
    } else {
        cell_block_len(params.posting_log2, params.cell_log2).ok_or_else(|| bad("invalid native layout".into()))?
    };
    let mut entry = [0; 4];
    cell.src.read_at(header.directory_offset.into(), &mut entry).map_err(Error::Io)?;
    let offset = u32::from_le_bytes(entry);
    let end = u64::from(offset) + u64::from(block_len);
    if offset == 0 || end > cell.src.len() {
        return Err(bad(format!("is {} bytes; a {block_len}-byte block needs {end}", cell.src.len())));
    }
    Ok(CellLayout { offset, bytes: block_len, surface })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The raster, checked and ordered, ready to be spliced into a map's tail.
///
/// Constructing one parses every input, matches it against the catalog and places it in its slot,
/// so [`TerrainRegion::emit`] is a copy that cannot fail on the data and [`TerrainRegion::bytes`]
/// is a length the header can be written from.
pub struct TerrainRegion<'a> {
    plan: TerrainPlan,
    cells: &'a [TerrainCellInput<'a>],
    /// One entry per rectangle slot in the directory's own row-major order: which input fills it,
    /// or `None` for a square the catalog publishes nothing for.
    slots: Vec<Option<usize>>,
    /// The header and offset directory, from the one OBCT layout (`obc_dem::container`).
    prefix: Vec<u8>,
    block_len: usize,
    input_offsets: Vec<u32>,
    bytes: u64,
}

impl<'a> TerrainRegion<'a> {
    /// Check every input and settle the region's layout, before the map's header is written.
    ///
    /// `cells` may arrive in any order and may name squares outside the rectangle. The first is
    /// sorted here, into the directory's own row-major order; the second is an error, because a
    /// selected cell the assembly box does not cover is a selection that does not mean what it
    /// says.
    pub fn prepare(plan: TerrainPlan, cells: &'a [TerrainCellInput<'a>]) -> Result<Self> {
        let block_len = cell_block_len(plan.params.posting_log2, plan.params.cell_log2)
            .ok_or_else(|| Error::Input("the terrain pairing has no block length".into()))?;

        // Index the inputs by square, refusing a duplicate and a square outside the rectangle. Both
        // would otherwise be resolved silently — the first by whichever copy the iteration reached
        // last, the second by dropping ground the rider selected.
        let mut by_square: std::collections::HashMap<(u32, u32), usize> = std::collections::HashMap::new();
        for (k, cell) in cells.iter().enumerate() {
            if cell.id.log2 != plan.params.cell_log2 as u32 {
                return Err(Error::Input(format!(
                    "terrain cell {} is not on the store's 2^{} grid",
                    cell.id, plan.params.cell_log2
                )));
            }
            let key = (cell.id.i as u32, cell.id.j as u32);
            let inside = (key.0 as u64) >= plan.rect.min_i as u64
                && (key.0 as u64) < plan.rect.min_i as u64 + plan.rect.rows as u64
                && (key.1 as u64) >= plan.rect.min_j as u64
                && (key.1 as u64) < plan.rect.min_j as u64 + plan.rect.cols as u64;
            if !inside {
                return Err(Error::Input(format!(
                    "terrain cell {} lies outside the assembly rectangle — the selection and the assembly bbox \
                     disagree",
                    cell.id
                )));
            }
            if by_square.insert(key, k).is_some() {
                return Err(Error::Input(format!("terrain cell {} was handed over more than once", cell.id)));
            }
        }

        // Check every input before any of it is placed, so a bad cell aborts before the map's header
        // has committed to a region length.
        let layouts = cells.iter().map(|cell| check_cell(cell, plan.params)).collect::<Result<Vec<_>>>()?;
        let surface = layouts.first().is_some_and(|layout| layout.surface);
        if layouts.iter().any(|layout| layout.surface != surface) {
            return Err(Error::Format("terrain cells use different surface encodings".into()));
        }
        let block_len = layouts.first().map_or(block_len, |layout| layout.bytes);
        let input_offsets = layouts.iter().map(|layout| layout.offset).collect();

        let slots: Vec<Option<usize>> = plan.rect.cells().map(|key| by_square.get(&key).copied()).collect();
        let present: Vec<bool> = slots.iter().map(Option::is_some).collect();
        let mut prefix = container_prefix_with_surface(
            plan.params.posting_log2,
            plan.params.cell_log2,
            plan.rect,
            &present,
            surface,
        )
        .map_err(Error::Format)?;
        if surface {
            let level = SurfaceLayout::new(plan.params.posting_log2, plan.params.cell_log2)
                .expect("validated surface")
                .level(0)
                .unwrap();
            let root = level.bound_offset(0, 0, level.samples_log2).expect("native cell root");
            let mut maxima = vec![i16::MAX; slots.len()];
            for (slot, input) in slots.iter().enumerate() {
                if let Some(k) = input {
                    let mut value = [0; 2];
                    cells[*k]
                        .src
                        .read_at(u64::from(layouts[*k].offset) + u64::from(root), &mut value)
                        .map_err(Error::Io)?;
                    maxima[slot] = i16::from_le_bytes(value);
                }
            }
            fill_cell_index(&mut prefix, plan.rect, &maxima).map_err(Error::Format)?;
        }
        let bytes = prefix.len() as u64 + by_square.len() as u64 * block_len as u64;
        Ok(TerrainRegion { plan, cells, slots, prefix, block_len: block_len as usize, input_offsets, bytes })
    }

    /// The container's exact byte length — what `Terrain Length` rounds up from, and what the map's
    /// layout reserves.
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Equivalent terrain size with only the original native height lattice.
    pub fn native_bytes(&self) -> u64 {
        self.plan.projected_bytes(self.cells() as u64)
    }

    pub fn has_surface(&self) -> bool {
        self.prefix[obc_formats::obct::HDR_FLAGS] & SURFACE_FLAG != 0
    }

    /// Squares with a block. The rest of the rectangle is directory `0`.
    pub fn cells(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Rectangle slots, present or not — the directory's own length in entries.
    pub fn slots(&self) -> u64 {
        self.plan.rect.slots()
    }

    /// Stream the container: the prefix, then every present block in slot order.
    ///
    /// No seek, which is why [`obc_dem::container::container_prefix`] exists: this runs in the
    /// middle of a map being written forward into a file or a browser download, and the directory
    /// has to be right the first time.
    pub fn emit(&self, w: &mut crate::emit::MapWriter<'_>) -> Result<()> {
        let start = w.at();
        w.put(&self.prefix)?;
        let mut block = vec![0u8; self.block_len];
        for &slot in &self.slots {
            let Some(k) = slot else { continue };
            self.cells[k].src.read_at(self.input_offsets[k].into(), &mut block).map_err(Error::Io)?;
            w.put(&block)?;
        }
        let written = w.at() - start;
        if written != self.bytes {
            return Err(Error::Verify(format!(
                "the terrain region projected to {} bytes but emitted {written}",
                self.bytes
            )));
        }
        Ok(())
    }

    /// Verify the raster against the window the finished map's header names — a byte source whose
    /// offset `0` is the region's first byte.
    ///
    /// This proves the header's region pointer resolves to a container that parses and whose every
    /// block is the object the catalog served.
    pub fn verify(&self, window: &dyn ByteSource) -> Result<()> {
        let reader = TerrainReader::parse(window)
            .map_err(|e| Error::Verify(format!("the spliced terrain region does not parse ({e:?})")))?;
        let header = *reader.header();
        if header.posting_log2 != self.plan.params.posting_log2 || header.cell_log2 != self.plan.params.cell_log2 {
            return Err(Error::Verify("the terrain region's lattice is not the catalog's".into()));
        }
        if header.flags != self.prefix[obc_formats::obct::HDR_FLAGS] {
            return Err(Error::Verify("the terrain surface encoding differs from its sources".into()));
        }
        if header.cell_min_i != self.plan.rect.min_i
            || header.cell_min_j != self.plan.rect.min_j
            || header.cell_rows != self.plan.rect.rows
            || header.cell_cols != self.plan.rect.cols
        {
            return Err(Error::Verify("the terrain region's rectangle is not the assembly rectangle".into()));
        }

        // Every slot: present exactly where an input was, absent everywhere else, and every present
        // block byte-for-byte the source it came from.
        let mut mine = vec![0u8; self.block_len];
        let mut theirs = vec![0u8; self.block_len];
        for (slot, (&filled_by, (ci, cj))) in self.slots.iter().zip(self.plan.rect.cells()).enumerate() {
            let entry_at = header.directory_offset + (slot * DIR_ENTRY_LEN) as u32;
            let mut raw = [0u8; DIR_ENTRY_LEN];
            window.read_at(entry_at.into(), &mut raw).map_err(Error::Io)?;
            let offset = u32::from_le_bytes(raw);
            match filled_by {
                None => {
                    if offset != 0 {
                        return Err(Error::Verify(format!(
                            "terrain slot ({ci}, {cj}) has no cell but the directory points at {offset}"
                        )));
                    }
                }
                Some(k) => {
                    if offset == 0 {
                        return Err(Error::Verify(format!(
                            "terrain cell {} was written but its directory slot is absent",
                            self.cells[k].id
                        )));
                    }
                    window.read_at(offset.into(), &mut mine).map_err(Error::Io)?;
                    self.cells[k].src.read_at(self.input_offsets[k].into(), &mut theirs).map_err(Error::Io)?;
                    if mine != theirs {
                        return Err(Error::Verify(format!(
                            "terrain cell {}'s block in the map is not the block the catalog served",
                            self.cells[k].id
                        )));
                    }
                }
            }
        }
        let mut actual_prefix = vec![0; self.prefix.len()];
        window.read_at(0, &mut actual_prefix).map_err(Error::Io)?;
        if actual_prefix != self.prefix {
            return Err(Error::Verify(
                "the terrain directory or cell maximum index differs from the prepared map".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use obc_dem::container::ShardWriter;
    use obc_formats::io::SliceSource;

    use super::*;

    #[test]
    fn surface_cells_are_placed_with_all_levels_and_verified() {
        let bytes = published(602, 526, 37);
        let mut converted = Cursor::new(Vec::new());
        obc_dem::surface::convert(&bytes, &mut converted).unwrap();
        let source = SliceSource(converted.get_ref());
        let cells = [TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &source, sha256: None }];
        let region = TerrainRegion::prepare(plan(), &cells).unwrap();
        assert!(region.bytes() > region.native_bytes());
        let mut assembled = region.prefix.clone();
        assembled.extend_from_slice(&converted.get_ref()[region.input_offsets[0] as usize..]);
        region.verify(&SliceSource(&assembled)).unwrap();
        let index = obc_formats::obct::CellIndexLayout::new(plan().rect.rows, plan().rect.cols, 32).unwrap();
        assembled[index.offset as usize] ^= 1;
        let error = region.verify(&SliceSource(&assembled)).unwrap_err();
        assert!(error.to_string().contains("cell maximum index"));
    }

    const POSTING: u8 = 14;
    const CELL: u8 = 19;
    /// `2^(19-14) = 32` samples an edge ⇒ `2` tiles an edge ⇒ `4 × 512` bytes.
    const BLOCK: usize = 2048;

    fn params() -> TerrainParams {
        TerrainParams { posting_log2: POSTING, cell_log2: CELL }
    }

    /// A published cell, written through the one OBCT writer exactly as the bakery writes it.
    fn published(i: u32, j: u32, fill: u8) -> Vec<u8> {
        let mut w =
            ShardWriter::new(Cursor::new(Vec::new()), POSTING, CELL, CellRect { min_i: i, min_j: j, rows: 1, cols: 1 })
                .expect("a legal 1 × 1 container");
        w.push(Some(&vec![fill; BLOCK])).expect("the block is the right length");
        w.finish().expect("finish").into_inner()
    }

    fn digest(bytes: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(bytes);
        h.finalize().into()
    }

    /// The fixture assembly box: `2^20` µdeg, which is 2 × 2 terrain cells at `2^19`.
    fn assembly() -> AlignedBox {
        AlignedBox { min_lat: 47_185_920, min_lon: 7_340_032, span_log2: 20 }
    }

    fn plan() -> TerrainPlan {
        TerrainPlan::over(params(), assembly()).expect("2^19 terrain tiles a 2^20 assembly")
    }

    /// Emit a region into a `Vec`, the way the map writer splices it.
    fn emitted(region: &TerrainRegion<'_>) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut sink = |buf: &[u8]| -> Result<()> {
                out.extend_from_slice(buf);
                Ok(())
            };
            region.emit(&mut crate::emit::MapWriter::new(crate::emit::SCALE, 0, &mut sink)).expect("the region emits");
        }
        out
    }

    #[test]
    fn the_rectangle_is_the_assembly_bbox_to_the_microdegree() {
        let plan = plan();
        assert_eq!(plan.rect.rows, 2);
        assert_eq!(plan.rect.cols, 2);
        assert_eq!(plan.ubox(), assembly().ubox(), "one bbox for the map and the raster");
        // …and a terrain cell wider than the assembly square is refused rather than overhung.
        let tiny = AlignedBox { span_log2: 18, ..assembly() };
        assert!(TerrainPlan::over(params(), tiny).is_err());
        // An impossible pairing is refused at the plan, not at the write.
        assert!(TerrainPlan::over(TerrainParams { posting_log2: 14, cell_log2: 17 }, assembly()).is_err());
    }

    /// The region's bytes and the read-back over them: absent squares cost four bytes, the
    /// projection is the emission, and every present block lands in the slot its id names.
    #[test]
    fn absent_squares_cost_four_bytes_and_the_projection_is_the_emission() {
        let a = published(602, 527, 0xA1);
        let b = published(602, 526, 0xB2);
        let (sa, sb) = (SliceSource(&a), SliceSource(&b));
        let cells = vec![
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 527).unwrap(), src: &sa, sha256: Some(digest(&a)) },
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &sb, sha256: Some(digest(&b)) },
        ];
        let region = TerrainRegion::prepare(plan(), &cells).expect("two of four squares present");

        assert_eq!(region.cells(), 2);
        assert_eq!(region.slots(), 4);
        assert_eq!(region.bytes(), plan().projected_bytes(2), "§5.7: the projection is the emission");
        assert_eq!(region.bytes() as usize, 32 + 4 * 4 + 2 * BLOCK);

        let bytes = emitted(&region);
        assert_eq!(bytes.len() as u64, region.bytes());
        assert_eq!(&bytes[..4], b"OBCT");
        let dir: Vec<u32> = bytes[32..48].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        // Row-major with latitude as the row: (602, 526) is slot 0, (602, 527) slot 1, row 181 empty.
        assert_eq!(dir, vec![48, 48 + BLOCK as u32, 0, 0]);
        assert!(bytes[48..48 + BLOCK].iter().all(|&b| b == 0xB2), "each block landed in its own slot");

        // …and it verifies through a window over exactly those bytes, including the filler tail a
        // real splice leaves behind.
        region.verify(&SliceSource(&bytes)).expect("the region reads back");
        let mut padded = bytes.clone();
        padded.extend_from_slice(&[obc_formats::obcm::FILLER; 15]);
        region.verify(&SliceSource(&padded)).expect("a window longer than the container is still the container");
    }

    /// A selection with no downloadable terrain at all — every square known-empty — is a legal
    /// region of pure directory. It says "no elevation here" in 48 bytes rather than by being
    /// absent, which is the difference between a rider whose map has no terrain and one whose
    /// terrain failed to download.
    #[test]
    fn an_all_known_empty_selection_is_a_directory_and_nothing_else() {
        let region = TerrainRegion::prepare(plan(), &[]).expect("an empty rectangle is legal");
        assert_eq!(region.cells(), 0);
        assert_eq!(region.bytes(), 32 + 4 * 4);
        let bytes = emitted(&region);
        assert!(bytes[32..].iter().all(|&b| b == 0), "every slot absent");
        region.verify(&SliceSource(&bytes)).expect("a directory-only region is verifiable");
    }

    /// The read-back is a comparison against the catalog's object, not a self-check: a region whose
    /// bytes were corrupted after emission fails even though it still parses as a container with
    /// the right lattice and rectangle.
    #[test]
    fn a_corrupted_block_fails_the_read_back() {
        let a = published(602, 526, 0xB2);
        let sa = SliceSource(&a);
        let cells = vec![TerrainCellInput {
            id: CellId::new(CELL as u32, 602, 526).unwrap(),
            src: &sa,
            sha256: Some(digest(&a)),
        }];
        let region = TerrainRegion::prepare(plan(), &cells).expect("one square present");
        let mut bytes = emitted(&region);
        region.verify(&SliceSource(&bytes)).expect("the honest bytes verify");

        bytes[48 + BLOCK / 2] ^= 0xFF;
        let err = region.verify(&SliceSource(&bytes)).expect_err("a flipped raster bit must not pass");
        assert!(format!("{err}").contains("not the block the catalog served"), "got: {err}");

        // …and so does a directory entry pointed somewhere else, which is the failure a hand-written
        // prefix would produce.
        let mut moved = emitted(&region);
        moved[32..36].copy_from_slice(&0u32.to_le_bytes());
        let err = region.verify(&SliceSource(&moved)).expect_err("an absent slot where a block was written");
        assert!(format!("{err}").contains("directory slot is absent"), "got: {err}");
    }

    #[test]
    fn a_truncated_cell_block_is_refused_before_anything_is_written() {
        let full = published(602, 526, 7);
        let cut = &full[..full.len() - 1];
        let src = SliceSource(cut);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("a short container is not a cell");
        assert!(format!("{err}").contains("not a usable OBCT container"), "got: {err}");
    }

    #[test]
    fn a_digest_mismatch_is_refused() {
        let bytes = published(602, 526, 7);
        let src = SliceSource(&bytes);
        let cells = vec![TerrainCellInput {
            id: CellId::new(CELL as u32, 602, 526).unwrap(),
            src: &src,
            sha256: Some([0xEE; 32]),
        }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("the catalog pins the bytes");
        assert!(format!("{err}").contains("digest mismatch"), "got: {err}");
    }

    #[test]
    fn a_directory_offset_out_of_bounds_is_refused() {
        let mut bytes = published(602, 526, 7);
        // Point the single directory entry past the end of the file.
        bytes[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        let src = SliceSource(&bytes);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("the arithmetic must close");
        assert!(format!("{err}").contains("not a usable OBCT container"), "got: {err}");
    }

    #[test]
    fn a_lattice_or_shape_mismatch_is_refused() {
        // A cell baked at another posting: legal OBCT, wrong store.
        let mut w =
            ShardWriter::new(Cursor::new(Vec::new()), 13, CELL, CellRect { min_i: 602, min_j: 526, rows: 1, cols: 1 })
                .expect("2^13 posting is a legal pairing too");
        let len = cell_block_len(13, CELL).unwrap() as usize;
        w.push(Some(&vec![0; len])).unwrap();
        let other = w.finish().unwrap().into_inner();
        let src = SliceSource(&other);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("one assembly is one lattice");
        assert!(format!("{err}").contains("one assembly is one lattice"), "got: {err}");

        // A container filed under the wrong square.
        let wrong = published(602, 527, 1);
        let src = SliceSource(&wrong);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("the id names the square");
        assert!(format!("{err}").contains("files it under"), "got: {err}");

        // A shard offered where a published cell was promised.
        let mut w = ShardWriter::new(Cursor::new(Vec::new()), POSTING, CELL, plan().rect).unwrap();
        for _ in 0..4 {
            w.push(None).unwrap();
        }
        let shardish = w.finish().unwrap().into_inner();
        let src = SliceSource(&shardish);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("a wider rectangle is a shard");
        assert!(format!("{err}").contains("a published cell is 1 × 1"), "got: {err}");
    }

    #[test]
    fn a_cell_outside_the_rectangle_or_listed_twice_is_refused() {
        let bytes = published(610, 526, 1);
        let src = SliceSource(&bytes);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 610, 526).unwrap(), src: &src, sha256: None }];
        let err = TerrainRegion::prepare(plan(), &cells).map(|_| ()).expect_err("outside the assembly");
        assert!(format!("{err}").contains("outside the assembly rectangle"), "got: {err}");

        let bytes = published(602, 526, 1);
        let src = SliceSource(&bytes);
        let twice = vec![
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None },
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None },
        ];
        let err = TerrainRegion::prepare(plan(), &twice).map(|_| ()).expect_err("one cell, one square");
        assert!(format!("{err}").contains("more than once"), "got: {err}");
    }
}
