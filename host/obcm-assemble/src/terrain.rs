//! The **terrain shard** of an assembly (EL4, #1072): downloaded OBCT cells in, one `.obcd`
//! container out, verified before the manifest names it.
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
//! * and the finished shard is read back through [`obc_elevation::TerrainReader`] — the same parser
//!   the firmware runs — and every present block compared with the source it came from.
//!
//! # One writer
//!
//! The bytes are written by [`obc_dem::container::ShardWriter`], the single OBCT container writer
//! in the tree. A cell published by the bakery and a shard assembled here come out of the same
//! code, which is the only reason "a cell is a 1 × 1 shard" is a fact rather than a claim.

use std::io::{Read, Seek, SeekFrom, Write};

use obc_dem::container::{CellRect, ShardWriter};
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

/// A sink the terrain shard is written to.
///
/// Seekable, unlike the OBCM [`crate::ShardStore`], because the OBCT container writer back-patches
/// its offset directory once at the end (`OBCT_Spec.md` §4.3) and there is exactly one writer in
/// the tree. Readable, because §4.8 reads the finished file back through the real reader before the
/// manifest may name it. Both hosts have such a sink for free: a `File` for the CLI, a
/// `Cursor<Vec<u8>>` for the browser.
pub trait TerrainSink: Read + Write + Seek {}

impl<T: Read + Write + Seek> TerrainSink for T {}

/// A `Read + Seek` sink presented as the random-access source the reader wants, so the read-back
/// runs through exactly the parser a device runs rather than a second opinion about the bytes.
struct SinkSource<'a> {
    sink: std::cell::RefCell<&'a mut dyn TerrainSink>,
    len: u32,
}

impl ByteSource for SinkSource<'_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> std::result::Result<(), obc_formats::io::Error> {
        let mut sink = self.sink.borrow_mut();
        sink.seek(SeekFrom::Start(offset as u64)).map_err(|_| obc_formats::io::Error::Io)?;
        sink.read_exact(buf).map_err(|_| obc_formats::io::Error::Io)
    }
    fn len(&self) -> u32 {
        self.len
    }
}

/// What a terrain assembly produced — everything the manifest's `terrain` record needs.
#[derive(Clone, Copy, Debug)]
pub struct TerrainShard {
    pub bytes: u64,
    pub sha256: [u8; 32],
    /// Present cells written. The rest of the rectangle is directory `0`.
    pub cells: usize,
    /// Rectangle slots, present or not — the directory's own length in entries.
    pub slots: u64,
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
        let mut cursor = 0u32;
        let mut buf = [0u8; 8192];
        let total = cell.src.len();
        while cursor < total {
            let n = buf.len().min((total - cursor) as usize);
            cell.src.read_at(cursor, &mut buf[..n]).map_err(Error::Io)?;
            hasher.update(&buf[..n]);
            cursor += n as u32;
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
    if end > cell.src.len() as u64 {
        return Err(bad(format!("is {} bytes; a {block_len}-byte block needs {end}", cell.src.len())));
    }
    Ok(CELL_BLOCK_OFFSET)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Assemble the terrain shard: write it through the one OBCT container writer, then verify it
/// (§4.8) before returning. A failure here aborts the whole assembly — the manifest has not been
/// written, so nothing mounts (§5.4).
///
/// `cells` may arrive in any order and may contain squares outside the rectangle; the first is
/// sorted here (the directory's own row-major order), the second is an error, because a selected
/// cell the assembly box does not cover is a selection that does not mean what it says.
pub fn write_shard(
    plan: TerrainPlan,
    cells: &[TerrainCellInput<'_>],
    out: &mut dyn TerrainSink,
) -> Result<TerrainShard> {
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
                "terrain cell {} lies outside the assembly rectangle — the selection and the assembly bbox disagree",
                cell.id
            )));
        }
        if by_square.insert(key, k).is_some() {
            return Err(Error::Input(format!("terrain cell {} was handed over more than once", cell.id)));
        }
    }

    // Check every input first, so a bad cell aborts before any byte is written.
    for cell in cells {
        check_cell(cell, plan.params, block_len)?;
    }

    let expected = plan.projected_bytes(by_square.len() as u64);
    let mut hasher = Sha256::new();
    let mut written = 0u64;
    {
        // The writer sees a wrapper that hashes and counts on the way past. `ShardWriter` seeks
        // back once to patch the directory, so the digest is taken from the finished file below
        // rather than from this stream — the counter is what the wrapper is really for.
        let mut w = ShardWriter::new(&mut *out, plan.params.posting_log2, plan.params.cell_log2, plan.rect)
            .map_err(Error::Input)?;
        let mut block = vec![0u8; block_len as usize];
        for (ci, cj) in plan.rect.cells() {
            match by_square.get(&(ci, cj)) {
                // An absent square is four bytes of directory and nothing else — the same answer
                // OBCT gives for an all-`NODATA` cell, which is why known-empty ocean is published
                // as a row run and never as 2 MiB of sentinel (OBCC §13.6).
                None => w.push(None).map_err(Error::Format)?,
                Some(&k) => {
                    cells[k].src.read_at(CELL_BLOCK_OFFSET, &mut block).map_err(Error::Io)?;
                    w.push(Some(&block)).map_err(Error::Format)?;
                }
            }
        }
        w.finish().map_err(Error::Format)?;
    }
    out.flush().map_err(|_| Error::Io(obc_formats::io::Error::Io))?;
    let len = out.seek(SeekFrom::End(0)).map_err(|_| Error::Io(obc_formats::io::Error::Io))?;
    if len != expected {
        return Err(Error::Verify(format!(
            "the terrain shard projected to {expected} bytes but wrote {len} — the §5.7 projection and the write \
             disagree"
        )));
    }
    written += len;
    let len32 = u32::try_from(len).map_err(|_| {
        Error::Capacity(format!(
            "the terrain shard is {len} bytes, past the {} the OBCT directory's uint32 offsets can address",
            u32::MAX
        ))
    })?;

    // --- §4.8, on the raster: read the finished file back through the real reader. ---
    let cells_written = by_square.len();
    {
        let source = SinkSource { sink: std::cell::RefCell::new(&mut *out), len: len32 };
        // The parse is the whole of OBCT §4.5: header, pairing, rectangle against the world grid,
        // and **every** directory entry even, after the directory, and wholly inside the file.
        let reader = TerrainReader::parse(&source)
            .map_err(|e| Error::Verify(format!("the terrain shard does not parse ({e:?})")))?;
        let header = *reader.header();
        if header.posting_log2 != plan.params.posting_log2 || header.cell_log2 != plan.params.cell_log2 {
            return Err(Error::Verify("the terrain shard's lattice is not the catalog's".into()));
        }
        if header.cell_min_i != plan.rect.min_i
            || header.cell_min_j != plan.rect.min_j
            || header.cell_rows != plan.rect.rows
            || header.cell_cols != plan.rect.cols
        {
            return Err(Error::Verify("the terrain shard's rectangle is not the assembly rectangle".into()));
        }

        // Every slot: present exactly where an input was, absent everywhere else, and every present
        // block byte-for-byte the source it came from. This is the raster's version of §4.8.2 —
        // there is nothing to decode, so what "the bytes arrived where the directory says" means is
        // a comparison against the object the catalog served.
        let mut mine = vec![0u8; block_len as usize];
        let mut theirs = vec![0u8; block_len as usize];
        for (slot, (ci, cj)) in plan.rect.cells().enumerate() {
            let entry_at = header.directory_offset + (slot * DIR_ENTRY_LEN) as u32;
            let mut raw = [0u8; DIR_ENTRY_LEN];
            source.read_at(entry_at, &mut raw).map_err(Error::Io)?;
            let offset = u32::from_le_bytes(raw);
            match by_square.get(&(ci, cj)) {
                None => {
                    if offset != 0 {
                        return Err(Error::Verify(format!(
                            "terrain slot ({ci}, {cj}) has no cell but the directory points at {offset}"
                        )));
                    }
                }
                Some(&k) => {
                    if offset == 0 {
                        return Err(Error::Verify(format!(
                            "terrain cell {} was written but its directory slot is absent",
                            cells[k].id
                        )));
                    }
                    source.read_at(offset, &mut mine).map_err(Error::Io)?;
                    cells[k].src.read_at(CELL_BLOCK_OFFSET, &mut theirs).map_err(Error::Io)?;
                    if mine != theirs {
                        return Err(Error::Verify(format!(
                            "terrain cell {}'s block in the shard is not the block the catalog served",
                            cells[k].id
                        )));
                    }
                }
            }
        }
    }

    // The shard's own digest, over the finished file — what the manifest records (§5.2).
    out.seek(SeekFrom::Start(0)).map_err(|_| Error::Io(obc_formats::io::Error::Io))?;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = out.read(&mut buf).map_err(|_| Error::Io(obc_formats::io::Error::Io))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(TerrainShard {
        bytes: written,
        sha256: hasher.finalize().into(),
        cells: cells_written,
        slots: plan.rect.slots(),
    })
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

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

    #[test]
    fn absent_squares_cost_four_bytes_and_the_projection_is_the_write() {
        let plan = plan();
        let a = published(602, 527, 0xA1);
        let b = published(602, 526, 0xB2);
        let (sa, sb) = (SliceSource(&a), SliceSource(&b));
        let cells = vec![
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 527).unwrap(), src: &sa, sha256: Some(digest(&a)) },
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &sb, sha256: Some(digest(&b)) },
        ];
        let mut out = Cursor::new(Vec::new());
        let shard = write_shard(plan, &cells, &mut out).expect("two of four squares present");

        assert_eq!(shard.cells, 2);
        assert_eq!(shard.slots, 4);
        assert_eq!(shard.bytes, plan.projected_bytes(2), "§5.7: the projection is the write");
        assert_eq!(shard.bytes as usize, 32 + 4 * 4 + 2 * BLOCK);
        let bytes = out.into_inner();
        assert_eq!(&bytes[..4], b"OBCT");
        let dir: Vec<u32> = bytes[32..48].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
        // Row-major with latitude as the row: (602, 526) is slot 0, (602, 527) slot 1, row 181 empty.
        assert_eq!(dir, vec![48, 48 + BLOCK as u32, 0, 0]);
        assert!(bytes[48..48 + BLOCK].iter().all(|&b| b == 0xB2), "each block landed in its own slot");
        assert_eq!(shard.sha256, digest(&bytes), "the recorded digest is the file's");
    }

    /// A selection with no downloadable terrain at all — every square known-empty — is a legal
    /// shard of pure directory. It says "no elevation here" in 48 bytes rather than by being absent,
    /// which is the difference between a rider whose map has no terrain and one whose terrain
    /// failed to download.
    #[test]
    fn an_all_known_empty_selection_is_a_directory_and_nothing_else() {
        let mut out = Cursor::new(Vec::new());
        let shard = write_shard(plan(), &[], &mut out).expect("an empty rectangle is legal");
        assert_eq!(shard.cells, 0);
        assert_eq!(shard.bytes, 32 + 4 * 4);
        assert!(out.into_inner()[32..].iter().all(|&b| b == 0), "every slot absent");
    }

    #[test]
    fn a_truncated_cell_block_is_refused_before_anything_is_written() {
        let full = published(602, 526, 7);
        let cut = &full[..full.len() - 1];
        let src = SliceSource(cut);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let mut out = Cursor::new(Vec::new());
        let err = write_shard(plan(), &cells, &mut out).expect_err("a short container is not a cell");
        assert!(format!("{err}").contains("not a usable OBCT container"), "got: {err}");
        assert!(out.into_inner().is_empty(), "nothing reached the sink");
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
        let err = write_shard(plan(), &cells, &mut Cursor::new(Vec::new())).expect_err("the catalog pins the bytes");
        assert!(format!("{err}").contains("digest mismatch"), "got: {err}");
    }

    #[test]
    fn a_directory_offset_out_of_bounds_is_refused() {
        let mut bytes = published(602, 526, 7);
        // Point the single directory entry past the end of the file.
        bytes[32..36].copy_from_slice(&u32::MAX.to_le_bytes());
        let src = SliceSource(&bytes);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = write_shard(plan(), &cells, &mut Cursor::new(Vec::new())).expect_err("the arithmetic must close");
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
        let err = write_shard(plan(), &cells, &mut Cursor::new(Vec::new())).expect_err("one assembly is one lattice");
        assert!(format!("{err}").contains("one assembly is one lattice"), "got: {err}");

        // A container filed under the wrong square.
        let wrong = published(602, 527, 1);
        let src = SliceSource(&wrong);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = write_shard(plan(), &cells, &mut Cursor::new(Vec::new())).expect_err("the id names the square");
        assert!(format!("{err}").contains("files it under"), "got: {err}");

        // A shard offered where a published cell was promised.
        let mut w = ShardWriter::new(Cursor::new(Vec::new()), POSTING, CELL, plan().rect).unwrap();
        for _ in 0..4 {
            w.push(None).unwrap();
        }
        let shardish = w.finish().unwrap().into_inner();
        let src = SliceSource(&shardish);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None }];
        let err = write_shard(plan(), &cells, &mut Cursor::new(Vec::new())).expect_err("a wider rectangle is a shard");
        assert!(format!("{err}").contains("a published cell is 1 × 1"), "got: {err}");
    }

    #[test]
    fn a_cell_outside_the_rectangle_or_listed_twice_is_refused() {
        let bytes = published(610, 526, 1);
        let src = SliceSource(&bytes);
        let cells = vec![TerrainCellInput { id: CellId::new(CELL as u32, 610, 526).unwrap(), src: &src, sha256: None }];
        let err = write_shard(plan(), &cells, &mut Cursor::new(Vec::new())).expect_err("outside the assembly");
        assert!(format!("{err}").contains("outside the assembly rectangle"), "got: {err}");

        let bytes = published(602, 526, 1);
        let src = SliceSource(&bytes);
        let twice = vec![
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None },
            TerrainCellInput { id: CellId::new(CELL as u32, 602, 526).unwrap(), src: &src, sha256: None },
        ];
        let err = write_shard(plan(), &twice, &mut Cursor::new(Vec::new())).expect_err("one cell, one square");
        assert!(format!("{err}").contains("more than once"), "got: {err}");
    }
}
