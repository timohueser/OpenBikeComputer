//! The **terrain region** of an assembly (EL4 #1072, spliced since OBCM v14): downloaded OBCT cells
//! in, one container out — written into the tail of the map file itself
//! ([`OBCM_Spec.md`](../../../specs/OBCM_Spec.md) §1.3) rather than beside it.
//!
//! # Placement, not grafting
//!
//! This is the shortest module in the crate and that is the design. A published terrain cell is
//! *already in its final form* — a `1 × 1` OBCT container whose block is the raster for exactly one
//! OBCA square ([`OBCT_Spec.md`](../../../specs/OBCT_Spec.md) §4.1) — so assembling terrain is
//! writing a wider directory over the assembly rectangle and copying each block into the slot its
//! id names. There is no geometry to relocate, no index to rebuild, no seam to unify: the lattice
//! is global and half-open, so two neighbouring cells already agree about every sample without
//! anyone looking (§3.1).
//!
//! What that leaves is bookkeeping with teeth, and all of it is here:
//!
//! * the **rectangle** is the assembly bbox, in terrain cells (§4.2's box, expressed on a second
//!   grid — see [`TerrainPlan::over`]);
//! * an absent or known-empty square is directory `0`, which OBCT makes indistinguishable from an
//!   all-`NODATA` block (`OBCC_Spec.md` §13.6) — that is why the catalog publishes no object for
//!   open ocean and why a bbox overhanging coverage costs 4 bytes per uncovered cell;
//! * every input is checked against the catalog **before** its bytes are copied (§4.8's posture,
//!   applied to a raster): the digest the catalog published, the header lattice the catalog's
//!   terrain block states, and the `1 × 1`-at-its-own-id shape §13.1 requires of a published cell;
//! * and the finished region is read back through [`obc_elevation::TerrainReader`] — the same parser
//!   the firmware runs, through the same §1.3 window a device will form — with every present block
//!   compared against the object the catalog served.
//!
//! # Prepared, then emitted, then verified
//!
//! The three are separate because the map writer needs them at three different moments.
//! [`TerrainRegion::prepare`] does every check and settles the byte length **before the layout**,
//! because §1.3's region pointer lives in the header and a header is the first thing written.
//! [`TerrainRegion::emit`] streams the container into the map's tail with no seek — the map is being
//! written forward into a file or a browser download, so there is no going back to patch a
//! directory. [`TerrainRegion::verify`] runs after the file is sealed, on the window the header now
//! names.
//!
//! # One layout
//!
//! The container's header and directory come from [`obc_dem::container::container_prefix`], the same
//! module whose [`obc_dem::container::ShardWriter`] the bakery bakes cells with, and the two are
//! pinned byte-for-byte against each other there. A cell published by the bakery and a region
//! assembled here are the same format because they are the same code, which is the only reason "a
//! cell is a 1 × 1 shard" is a fact rather than a claim.
//!
//! # Why the raster has no digest of its own any more
//!
//! It used to have one: the raster was a separate file, the OBCS manifest recorded its SHA-256 in a
//! `terrain` record, and that digest was how a consumer knew which bytes it had. There is no
//! manifest and no separate file — the raster is a run of bytes inside the map, and the map has
//! exactly one identity. A second digest over a subrange of it would be a second answer to a
//! question that now has one, and the first thing that can go stale. What the digest actually
//! bought — "these are the bytes the catalog served" — is bought instead by [`TerrainRegion::verify`]
//! comparing every present block against its source object, which is a stronger claim than a hash
//! of the assembler\'s own output agreeing with itself.

use obc_dem::container::{container_prefix, CellRect};
use obc_elevation::TerrainReader;
use obc_formats::io::ByteSource;
use obc_formats::obct::{cell_block_len, cell_samples_log2, DIR_ENTRY_LEN, HEADER_LEN};
use sha2::{Digest, Sha256};

use crate::grid::{AlignedBox, CellId, GRID_ORIGIN};
use crate::{Error, Result};

/// The lattice a terrain store is published at — `OBCC_Spec.md` §13.1's block, minus the parts an
/// assembler has no opinion about. Every input cell's own header must state exactly this, and so
/// must the shard's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainParams {
    /// `log2(P)` of the sample posting in µdeg (`OBCT_Spec.md` §1.1).
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
    /// The `sha256` the pinned terrain index carries for this object (`OBCC_Spec.md` §13.1).
    /// `None` only for a caller with no catalog (the CLI reading a local tree), which then gets
    /// every structural check and no provenance one.
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
    /// `GRID_ORIGIN` modulo `S_MAX` (§2.1). A terrain cell no larger than that square therefore
    /// tiles it **exactly**: the corner is a multiple of the terrain cell size too, and the side is
    /// a whole number of them. So the rectangle is the assembly bbox, to the microdegree, and the
    /// manifest can record one bbox for the raster and the map alike.
    ///
    /// A terrain cell *larger* than the assembly square is refused rather than accommodated. The
    /// rectangle would then overhang the assembly bbox, and §5.3 requires every shard's bbox to lie
    /// inside it — so the alternatives are a manifest a reader rejects or an assembly box grown to
    /// fit a raster, which §4.2 explicitly forbids. At the v1 pairing this cannot happen (terrain is
    /// `2^19`, `S_MAX` is `2^20`); a schema that made it possible is a configuration to fix.
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
        // **Fitting inside the box is not the same as tiling it.** §4.2 snaps the assembly corner to
        // `S_MAX` — the *largest band's* cell — and the span may be several times that, so a
        // selection wider than `S_MAX` has a corner aligned to `S_MAX` and no finer. A terrain cell
        // larger than `S_MAX` therefore passes the span check above and still lands off the corner,
        // which would put the whole raster half a cell from the ground it describes.
        //
        // The v1 pairing cannot reach it (terrain is `2^19`, `S_MAX` is `2^20`), and every published
        // lattice so far is smaller than every band's cell — which is exactly why it was worth
        // making a refusal rather than leaving it to hold by luck. It was a `debug_assert` in the
        // caller until an end-to-end test tripped it, i.e. it was a release-build silent mis-place.
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

    /// The shard's exact byte length — §5.7's projection for the raster, computed from the
    /// rectangle and the cell count alone, with nothing fetched and nothing written.
    ///
    /// It is the pre-download projection the builder shows *and* the number the write is checked
    /// against, which is what makes the two the same claim rather than two estimates.
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
const CELL_BLOCK_OFFSET: u32 = HEADER_LEN as u32 + DIR_ENTRY_LEN as u32;

/// Check one published cell against the catalog and against `OBCC_Spec.md` §13.1, and return its
/// block offset. Everything here is a property of the downloaded bytes, so it runs **before** a
/// single one is copied — a bad cell must never reach the shard, not even to be caught on the way
/// out.
fn check_cell(cell: &TerrainCellInput<'_>, params: TerrainParams, block_len: u32) -> Result<u32> {
    let bad = |what: String| Error::Format(format!("terrain cell {}: {what}", cell.id));

    // The container itself, through the real reader: magic, version, flags, the posting/cell
    // pairing, the rectangle against the world grid, and every directory entry against the file's
    // own length (OBCT §4.5). A truncated download fails here, on the length check, rather than as
    // a short read half a megabyte into the copy.
    let reader = TerrainReader::parse(cell.src).map_err(|e| bad(format!("not a usable OBCT container ({e:?})")))?;
    let header = reader.header();
    if header.posting_log2 != params.posting_log2 || header.cell_log2 != params.cell_log2 {
        return Err(bad(format!(
            "is posting 2^{} / cell 2^{} µdeg, but the catalog's terrain block says posting 2^{} / cell 2^{} — one \
             assembly is one lattice (OBCC §13.2)",
            header.posting_log2, header.cell_log2, params.posting_log2, params.cell_log2
        )));
    }
    // §13.1: a *published cell* is a 1 × 1 container at exactly its own id. A wider rectangle is a
    // shard, and a 1 × 1 at some other square is a cell filed under the wrong name — either would
    // place a raster over ground it is not.
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
    // says the *content* is what was promised rather than merely well-formed.
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
    let end = CELL_BLOCK_OFFSET as u64 + block_len as u64;
    if end > cell.src.len() {
        return Err(bad(format!("is {} bytes; a {block_len}-byte block needs {end}", cell.src.len())));
    }
    Ok(CELL_BLOCK_OFFSET)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The raster, checked and ordered, ready to be spliced into a map's tail (§1.3).
///
/// Constructing one is the whole of the "before any byte is written" half of §4.8: every input is
/// parsed, matched against the catalog and placed in its slot here, so [`TerrainRegion::emit`] is a
/// copy that cannot fail on the data and [`TerrainRegion::bytes`] is a length the header can be
/// written from.
pub struct TerrainRegion<'a> {
    plan: TerrainPlan,
    cells: &'a [TerrainCellInput<'a>],
    /// One entry per rectangle slot in the directory's own row-major order: which input fills it,
    /// or `None` for a square the catalog publishes nothing for.
    slots: Vec<Option<usize>>,
    /// The header and offset directory, from the one OBCT layout (`obc_dem::container`).
    prefix: Vec<u8>,
    block_len: usize,
    bytes: u64,
}

impl<'a> TerrainRegion<'a> {
    /// Check every input and settle the region's layout, before the map's header is written.
    ///
    /// `cells` may arrive in any order and may name squares outside the rectangle; the first is
    /// sorted here (the directory's own row-major order), the second is an error, because a selected
    /// cell the assembly box does not cover is a selection that does not mean what it says.
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
        for cell in cells {
            check_cell(cell, plan.params, block_len)?;
        }

        let slots: Vec<Option<usize>> = plan.rect.cells().map(|key| by_square.get(&key).copied()).collect();
        let present: Vec<bool> = slots.iter().map(Option::is_some).collect();
        let prefix = container_prefix(plan.params.posting_log2, plan.params.cell_log2, plan.rect, &present)
            .map_err(Error::Format)?;
        let bytes = prefix.len() as u64 + by_square.len() as u64 * block_len as u64;
        debug_assert_eq!(bytes, plan.projected_bytes(by_square.len() as u64), "the projection is the layout");
        Ok(TerrainRegion { plan, cells, slots, prefix, block_len: block_len as usize, bytes })
    }

    /// The container's exact byte length — what §1.3's `Terrain Length` rounds up from, and what the
    /// map's layout reserves.
    pub fn bytes(&self) -> u64 {
        self.bytes
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
    /// No seek, which is the entire reason [`obc_dem::container::container_prefix`] exists: this
    /// runs in the middle of a map being written forward into a file or a browser download, and the
    /// directory has to be right the first time.
    pub fn emit(&self, w: &mut crate::emit::MapWriter<'_>) -> Result<()> {
        let start = w.at();
        w.put(&self.prefix)?;
        let mut block = vec![0u8; self.block_len];
        for &slot in &self.slots {
            let Some(k) = slot else { continue };
            self.cells[k].src.read_at(CELL_BLOCK_OFFSET.into(), &mut block).map_err(Error::Io)?;
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

    /// §4.8 on the raster, run against the **window the finished map's header names** — a byte
    /// source whose offset `0` is the region's first byte (§1.3).
    ///
    /// This is the read-back the raster used to get as a separate file, moved onto the seam a device
    /// actually reads through. It is a stronger check than it was: the old one proved the assembler
    /// could re-read its own file, this one proves the header's region pointer resolves to a
    /// container that parses and whose every block is the object the catalog served.
    pub fn verify(&self, window: &dyn ByteSource) -> Result<()> {
        let reader = TerrainReader::parse(window)
            .map_err(|e| Error::Verify(format!("the spliced terrain region does not parse ({e:?})")))?;
        let header = *reader.header();
        if header.posting_log2 != self.plan.params.posting_log2 || header.cell_log2 != self.plan.params.cell_log2 {
            return Err(Error::Verify("the terrain region's lattice is not the catalog's".into()));
        }
        if header.cell_min_i != self.plan.rect.min_i
            || header.cell_min_j != self.plan.rect.min_j
            || header.cell_rows != self.plan.rect.rows
            || header.cell_cols != self.plan.rect.cols
        {
            return Err(Error::Verify("the terrain region's rectangle is not the assembly rectangle".into()));
        }

        // Every slot: present exactly where an input was, absent everywhere else, and every present
        // block byte-for-byte the source it came from. This is the raster's version of §4.8.2 —
        // there is nothing to decode, so what "the bytes arrived where the directory says" means is
        // a comparison against the object the catalog served.
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
                    self.cells[k].src.read_at(CELL_BLOCK_OFFSET.into(), &mut theirs).map_err(Error::Io)?;
                    if mine != theirs {
                        return Err(Error::Verify(format!(
                            "terrain cell {}'s block in the map is not the block the catalog served",
                            self.cells[k].id
                        )));
                    }
                }
            }
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

    /// The fixture assembly box: `2^20` µdeg at the OBCA §7 worked-example corner, which is 2 × 2
    /// terrain cells at `2^19`.
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

    /// The region's bytes, and the §4.8 read-back over them: absent squares cost four bytes, the
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
        let dir: Vec<u32> = bytes[32..48].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        // Row-major with latitude as the row: (602, 526) is slot 0, (602, 527) slot 1, row 181 empty.
        assert_eq!(dir, vec![48, 48 + BLOCK as u32, 0, 0]);
        assert!(bytes[48..48 + BLOCK].iter().all(|&b| b == 0xB2), "each block landed in its own slot");

        // …and it verifies through a window over exactly those bytes — the shape §1.3 hands a
        // consumer, including the up-to-`U−1` filler tail a real splice leaves behind.
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

    /// **The read-back is a comparison against the catalog's object, not a self-check.** A region
    /// whose bytes were corrupted after emission fails even though it still parses as a container
    /// with the right lattice and rectangle — which is the property that replaced the manifest's
    /// per-raster SHA-256.
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
