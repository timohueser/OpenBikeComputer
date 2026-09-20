//! Writing the OBCT container (`OBCT_Spec.md` §4): a 32-byte header, a row-major `uint32` offset
//! directory over the cell rectangle, then the present cells' blocks.
//!
//! There is **one** layout for every OBCT container in the tree, because there is one format: a
//! terrain *cell* is a shard whose rectangle is 1 × 1 (spec §4.1, principle 5), and since OBCM v14
//! an assembled shard is also a region spliced into a map's tail (`OBCM_Spec.md` §1.3). Two code
//! paths that each decided the layout would be the first place the three could drift.
//!
//! So the layout lives in [`validate`], [`header_bytes`] and [`container_prefix`], and the two
//! *emitters* are thin over it:
//!
//! * [`ShardWriter`] streams a container whose presence it discovers as it goes — the baker's
//!   position, since a cell turns out to be all-`NODATA` only once it has been sampled — and
//!   patches its directory at the end, which needs a seek.
//! * [`container_prefix`] serves a caller that knows every present square up front and cannot
//!   seek, because it is splicing the container into a file it is streaming. It returns the
//!   finished header and directory; the caller writes the present blocks in slot order behind it.
//!
//! `the_streamed_prefix_is_what_the_shard_writer_patches` compares the two byte-for-byte, which is
//! what makes "one layout" a fact rather than a claim.
//!
//! Every byte fact — the magic, the field offsets, the absent sentinel, the block length — comes
//! from [`obc_formats::obct`]. Nothing in this file transcribes the header table.

use std::io::{Seek, SeekFrom, Write};

use obc_formats::obct::{
    cell_block_len, cell_samples_log2, CellIndexLayout, CrestDirectory, CrestLayout, SurfaceLayout, SurfaceLevel,
    CELL_INDEX_FLAG, CREST_FLAG, DIR_ABSENT, DIR_ENTRY_LEN, HDR_CELL_COLS, HDR_CELL_LOG2, HDR_CELL_MIN_I,
    HDR_CELL_MIN_J, HDR_CELL_ROWS, HDR_DIRECTORY_OFFSET, HDR_FLAGS, HDR_MAGIC, HDR_POSTING_LOG2, HDR_VERSION,
    HEADER_LEN, MAGIC, SURFACE_FLAG, SURFACE_VERSION, VERSION,
};

use obc_elevation::grid::axis_cells;

/// The cell rectangle a container covers: the same `(min_i, min_j, rows, cols)` the header carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellRect {
    pub min_i: u32,
    pub min_j: u32,
    pub rows: u16,
    pub cols: u16,
}

impl CellRect {
    /// Cells in the rectangle, in the directory's own row-major order.
    pub fn cells(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        (0..self.rows as u32)
            .flat_map(move |di| (0..self.cols as u32).map(move |dj| (self.min_i + di, self.min_j + dj)))
    }

    /// Number of directory slots.
    pub fn slots(&self) -> u64 {
        self.rows as u64 * self.cols as u64
    }
}

/// Streams an OBCT container: header, a placeholder directory, then one block per present cell,
/// patching the directory at the end.
///
/// Streaming rather than assembling in memory because a single v1 cell block is 2 MiB and a
/// continental shard is thousands of them. The writer holds one directory (`4 · rows · cols` bytes —
/// ~2 KB for a DACH-shaped rectangle) and whatever the caller hands it, never the raster.
pub struct ShardWriter<W: Write + Seek> {
    out: W,
    block_len: u32,
    /// Kept only so an overflow error can name the pairing that caused it.
    cell_log2: u8,
    /// Directory entries in slot order; `DIR_ABSENT` until a block is written for that cell.
    directory: Vec<u32>,
    /// Next slot to be offered a block. Cells arrive in directory order, so the file ends up as a
    /// directory followed by the raster in reading order (spec §4.4's SHOULD).
    next_slot: usize,
    /// Absolute offset the next block will start at.
    cursor: u32,
    cell_index: Option<(CellIndexLayout, SurfaceLevel)>,
    cell_maxima: Vec<i16>,
    /// §9's directory and block length, when this container carries crest planes.
    crest: Option<(CrestDirectory, u32)>,
    /// Crest offsets in slot order, patched at the end beside the cell directory.
    crest_directory: Vec<u32>,
    rect: CellRect,
}

/// Everything about a container that is decided before a byte is written: the pairing is one OBCT
/// permits, the rectangle is on the world grid, and the directory leaves room for a block behind it.
/// Returns the block length at that pairing.
///
/// Shared by both emitters below, so "is this container writable at all" is one answer rather than
/// two that agree today.
fn validate(posting_log2: u8, cell_log2: u8, rect: CellRect) -> Result<u32, String> {
    cell_samples_log2(posting_log2, cell_log2).ok_or_else(|| {
        format!("posting 2^{posting_log2} µdeg with cell 2^{cell_log2} µdeg is not a pairing OBCT permits")
    })?;
    let block_len = cell_block_len(posting_log2, cell_log2).expect("pairing validated above");
    if rect.rows == 0 || rect.cols == 0 {
        return Err("a cell rectangle must be at least 1 × 1".to_string());
    }
    let axis = axis_cells(cell_log2) as u64;
    if rect.min_i as u64 + rect.rows as u64 > axis || rect.min_j as u64 + rect.cols as u64 > axis {
        return Err(format!("cell rectangle {rect:?} runs off the world grid at 2^{cell_log2} µdeg"));
    }
    // A `uint32` addresses the whole file, so the directory alone has to fit one — and it has to
    // fit with room for at least one block behind it. The *blocks* are checked as they arrive
    // (see [`ShardWriter::push`]) rather than against the rectangle's worst case: a wide rectangle
    // that is mostly absent is a perfectly ordinary shard, and refusing it here because a
    // hypothetically full one would overflow would reject files that are entirely writable.
    let dir_end = HEADER_LEN as u64 + rect.slots() * DIR_ENTRY_LEN as u64;
    if dir_end + block_len as u64 > u32::MAX as u64 {
        return Err(format!(
            "a {}×{} directory at 2^{cell_log2} µdeg leaves no room inside the uint32 offsets it is made of",
            rect.rows, rect.cols
        ));
    }
    Ok(block_len)
}

/// The 32-byte OBCT header (`OBCT_Spec.md` §4.2). The one transcription of that table in the tree.
fn header_bytes(posting_log2: u8, cell_log2: u8, rect: CellRect, surface: bool, crest: bool) -> [u8; HEADER_LEN] {
    let mut header = [0u8; HEADER_LEN];
    header[HDR_MAGIC..HDR_MAGIC + 4].copy_from_slice(&MAGIC);
    header[HDR_VERSION] = if surface { SURFACE_VERSION } else { VERSION };
    header[HDR_POSTING_LOG2] = posting_log2;
    header[HDR_CELL_LOG2] = cell_log2;
    header[HDR_FLAGS] = match (surface, crest) {
        (true, true) => SURFACE_FLAG | CELL_INDEX_FLAG | CREST_FLAG,
        (true, false) => SURFACE_FLAG | CELL_INDEX_FLAG,
        _ => 0,
    };
    header[HDR_CELL_MIN_I..HDR_CELL_MIN_I + 4].copy_from_slice(&rect.min_i.to_le_bytes());
    header[HDR_CELL_MIN_J..HDR_CELL_MIN_J + 4].copy_from_slice(&rect.min_j.to_le_bytes());
    header[HDR_CELL_ROWS..HDR_CELL_ROWS + 2].copy_from_slice(&rect.rows.to_le_bytes());
    header[HDR_CELL_COLS..HDR_CELL_COLS + 2].copy_from_slice(&rect.cols.to_le_bytes());
    // The directory follows the header immediately, which is what a v1 producer MUST write —
    // the field is explicit anyway so a reader follows it rather than the assumption.
    header[HDR_DIRECTORY_OFFSET..HDR_DIRECTORY_OFFSET + 4].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
    // 24..32 stay zero: reserved, and a reader refuses the file if they are not.
    header
}

/// The container's fixed prefix — header and **final** offset directory — for a rectangle whose
/// presence is known before any byte is written. `present[k]` is whether slot `k` carries a block,
/// in the rectangle's own row-major order.
///
/// [`ShardWriter`] discovers presence while it bakes (a cell turns out to be all-`NODATA` only once
/// it has been sampled) and therefore patches its directory at the end, which needs a seek. An
/// **assembler** is in the opposite position: it holds every downloaded cell before it starts, and
/// it splices the container into the tail of an OBCM file it is *streaming* (`OBCM_Spec.md` §1.3),
/// where there is no seek to be had. So the same container has to be emitable both ways.
///
/// This is the half that could drift, and it is therefore the half that is shared: a caller writes
/// this prefix and then the present blocks in slot order, and gets the bytes `ShardWriter` would
/// have left behind. `the_streamed_prefix_is_what_the_shard_writer_patches` pins that byte-for-byte
/// rather than leaving it as a claim.
pub fn container_prefix(posting_log2: u8, cell_log2: u8, rect: CellRect, present: &[bool]) -> Result<Vec<u8>, String> {
    container_prefix_with_surface(posting_log2, cell_log2, rect, present, false)
}

/// A surface container aligns its height blocks and stores the fixed v2 cell layout.
pub fn container_prefix_with_surface(
    posting_log2: u8,
    cell_log2: u8,
    rect: CellRect,
    present: &[bool],
    surface: bool,
) -> Result<Vec<u8>, String> {
    container_prefix_with_crest(posting_log2, cell_log2, rect, present, surface, false)
}

/// As [`container_prefix_with_surface`], plus §9's crest directory. Crest blocks are interleaved
/// behind their own cell, so the directory is patched at the end like the cell directory is.
pub fn container_prefix_with_crest(
    posting_log2: u8,
    cell_log2: u8,
    rect: CellRect,
    present: &[bool],
    surface: bool,
    crest: bool,
) -> Result<Vec<u8>, String> {
    let native_len = validate(posting_log2, cell_log2, rect)?;
    let block_len = if surface {
        SurfaceLayout::new(posting_log2, cell_log2).ok_or("surface cell exceeds OBCT offsets")?.cell_bytes()
    } else {
        native_len
    };
    let slots = rect.slots() as usize;
    if present.len() != slots {
        return Err(format!("the presence plan has {} entries for a {slots}-slot rectangle", present.len()));
    }
    let mut out = Vec::with_capacity(HEADER_LEN + slots * DIR_ENTRY_LEN);
    out.extend_from_slice(&header_bytes(posting_log2, cell_log2, rect, surface, crest));
    let prefix_bytes = HEADER_LEN + slots * DIR_ENTRY_LEN;
    let padded = if surface {
        let index =
            CellIndexLayout::new(rect.rows, rect.cols, HEADER_LEN as u32).ok_or("cell index exceeds OBCT offsets")?;
        if crest {
            CrestDirectory::new(rect.rows, rect.cols, index.end()).ok_or("crest directory exceeds OBCT offsets")?.end()
                as usize
        } else {
            index.end() as usize
        }
    } else {
        prefix_bytes
    };
    let mut cursor = padded as u64;
    for &here in present {
        if !here {
            out.extend_from_slice(&DIR_ABSENT.to_le_bytes());
            continue;
        }
        // The same bound `push` applies per block, applied to the whole plan at once: every present
        // block's *end* has to be addressable by the `uint32` the directory is made of.
        if cursor + block_len as u64 > u32::MAX as u64 {
            return Err(format!(
                "this shard has grown past the uint32 offsets the directory is made of — {} present cells is too many at 2^{cell_log2} µdeg",
                present.iter().filter(|p| **p).count()
            ));
        }
        out.extend_from_slice(&(cursor as u32).to_le_bytes());
        cursor += block_len as u64;
    }
    out.resize(padded, 0);
    if surface {
        fill_cell_index(&mut out, rect, &vec![i16::MAX; slots])?;
    }
    Ok(out)
}

/// Populate the prefix from inclusive native cell maxima; missing cells are unbounded.
pub fn fill_cell_index(prefix: &mut [u8], rect: CellRect, maxima: &[i16]) -> Result<(), String> {
    if maxima.len() != rect.slots() as usize {
        return Err("cell maxima do not match the terrain rectangle".into());
    }
    let layout =
        CellIndexLayout::new(rect.rows, rect.cols, HEADER_LEN as u32).ok_or("cell index exceeds OBCT offsets")?;
    if prefix.len() < layout.end() as usize {
        return Err("cell index prefix is truncated".into());
    }
    let (mut rows, mut cols) = (rect.rows as usize, rect.cols as usize);
    let mut plane: Vec<i16> = maxima.iter().map(|&h| if h == i16::MIN { i16::MAX } else { h }).collect();
    let mut at = layout.offset as usize;
    loop {
        for &height in &plane {
            prefix[at..at + 2].copy_from_slice(&height.to_le_bytes());
            at += 2;
        }
        if rows == 1 && cols == 1 {
            break;
        }
        let (next_rows, next_cols) = (rows.div_ceil(2), cols.div_ceil(2));
        let mut next = Vec::with_capacity(next_rows * next_cols);
        for y in 0..next_rows {
            for x in 0..next_cols {
                let mut maximum = i16::MIN;
                for dy in 0..2 {
                    for dx in 0..2 {
                        if y * 2 + dy < rows && x * 2 + dx < cols {
                            maximum = maximum.max(plane[(y * 2 + dy) * cols + x * 2 + dx]);
                        }
                    }
                }
                next.push(maximum);
            }
        }
        plane = next;
        rows = next_rows;
        cols = next_cols;
    }
    Ok(())
}

impl<W: Write + Seek> ShardWriter<W> {
    /// Open a container over `out` for a `posting_log2` / `cell_log2` pairing and a cell rectangle,
    /// writing the header and a fully-absent directory.
    pub fn new(out: W, posting_log2: u8, cell_log2: u8, rect: CellRect) -> Result<Self, String> {
        Self::with_surface(out, posting_log2, cell_log2, rect, false)
    }

    pub fn with_surface(
        out: W,
        posting_log2: u8,
        cell_log2: u8,
        rect: CellRect,
        surface: bool,
    ) -> Result<Self, String> {
        Self::with_crest(out, posting_log2, cell_log2, rect, surface, false)
    }

    /// A container that may carry §9 crest planes. Each present cell's plane follows its own
    /// block, so one streaming pass writes both and the two directories are patched together.
    pub fn with_crest(
        mut out: W,
        posting_log2: u8,
        cell_log2: u8,
        rect: CellRect,
        surface: bool,
        crest: bool,
    ) -> Result<Self, String> {
        let native_len = validate(posting_log2, cell_log2, rect)?;
        let block_len = if surface {
            SurfaceLayout::new(posting_log2, cell_log2).ok_or("surface cell exceeds OBCT offsets")?.cell_bytes()
        } else {
            native_len
        };
        let slots = rect.slots() as usize;
        let prefix = container_prefix_with_crest(posting_log2, cell_log2, rect, &vec![false; slots], surface, crest)?;
        out.write_all(&prefix).map_err(|e| format!("writing OBCT prefix: {e}"))?;

        Ok(ShardWriter {
            out,
            block_len,
            cell_log2,
            directory: vec![DIR_ABSENT; slots],
            next_slot: 0,
            cursor: prefix.len() as u32,
            cell_index: if surface {
                Some((
                    CellIndexLayout::new(rect.rows, rect.cols, HEADER_LEN as u32).expect("validated prefix"),
                    SurfaceLayout::new(posting_log2, cell_log2).expect("validated surface").level(0).unwrap(),
                ))
            } else {
                None
            },
            cell_maxima: if surface { vec![i16::MAX; slots] } else { Vec::new() },
            crest: (surface && crest).then(|| {
                let index = CellIndexLayout::new(rect.rows, rect.cols, HEADER_LEN as u32).expect("validated prefix");
                let layout = CrestLayout::new(SurfaceLayout::new(posting_log2, cell_log2).expect("validated surface"));
                (
                    CrestDirectory::new(rect.rows, rect.cols, index.end()).expect("validated crest directory"),
                    layout.block_bytes(),
                )
            }),
            crest_directory: if surface && crest { vec![DIR_ABSENT; slots] } else { Vec::new() },
            rect,
        })
    }

    /// Byte length of one cell block at this pairing.
    pub fn block_len(&self) -> u32 {
        self.block_len
    }

    /// Offer the next cell in directory order. `None` writes nothing and leaves the slot at the
    /// absent sentinel — which is how a cell with no data at all is published (spec §4.3), and the
    /// reason a bbox that overhangs coverage costs 4 bytes per uncovered cell rather than 2 MiB.
    pub fn push(&mut self, block: Option<&[u8]>) -> Result<(), String> {
        self.push_with_crest(block, None)
    }

    /// As [`push`](Self::push), with the cell's §9 crest planes. A crest block without a cell block
    /// is refused: a lift describes a native sample, so there has to be one.
    pub fn push_with_crest(&mut self, block: Option<&[u8]>, crest: Option<&[u8]>) -> Result<(), String> {
        let slot = self.next_slot;
        if slot >= self.directory.len() {
            return Err("more cells offered than the rectangle has slots".to_string());
        }
        self.next_slot += 1;
        let Some(block) = block else {
            if crest.is_some() {
                return Err(format!("cell {slot} offered crest planes without a block"));
            }
            return Ok(());
        };
        if crest.is_some() && self.crest.is_none() {
            return Err("this container was not opened for crest planes".to_string());
        }
        if let (Some(bytes), Some((_, len))) = (crest, self.crest) {
            if bytes.len() != len as usize {
                return Err(format!("crest block is {} bytes, expected {len}", bytes.len()));
            }
        }
        if block.len() != self.block_len as usize {
            return Err(format!("cell block is {} bytes, expected {}", block.len(), self.block_len));
        }
        if let Some((_, level)) = self.cell_index {
            let at = level.bound_offset(0, 0, level.samples_log2).expect("native cell root") as usize;
            self.cell_maxima[slot] = i16::from_le_bytes([block[at], block[at + 1]]);
        }
        // The directory is made of `uint32` offsets, so this block's *end* has to be addressable —
        // checked here, where the actual file length is known, rather than pessimistically at open.
        if self.cursor as u64 + self.block_len as u64 > u32::MAX as u64 {
            return Err(format!(
                "this shard has grown past the uint32 offsets the directory is made of — {} present cells is too many at 2^{} µdeg",
                self.directory.iter().filter(|&&e| e != DIR_ABSENT).count(),
                self.cell_log2
            ));
        }
        self.out.write_all(block).map_err(|e| format!("writing OBCT cell block: {e}"))?;
        self.directory[slot] = self.cursor;
        self.cursor += self.block_len;
        if let Some(bytes) = crest {
            if self.cursor as u64 + bytes.len() as u64 > u32::MAX as u64 {
                return Err("this shard's crest planes have grown past its uint32 offsets".to_string());
            }
            self.out.write_all(bytes).map_err(|e| format!("writing OBCT crest block: {e}"))?;
            self.crest_directory[slot] = self.cursor;
            self.cursor += bytes.len() as u32;
        }
        Ok(())
    }

    /// Patch the directory and return the finished writer.
    pub fn finish(mut self) -> Result<W, String> {
        if self.next_slot != self.directory.len() {
            return Err(format!(
                "{} of {} cells were never offered",
                self.directory.len() - self.next_slot,
                self.directory.len()
            ));
        }
        let bytes: Vec<u8> = self.directory.iter().flat_map(|e| e.to_le_bytes()).collect();
        self.out.seek(SeekFrom::Start(HEADER_LEN as u64)).map_err(|e| format!("seeking to the OBCT directory: {e}"))?;
        self.out.write_all(&bytes).map_err(|e| format!("patching the OBCT directory: {e}"))?;
        if let Some((directory, _)) = self.crest {
            let bytes: Vec<u8> = self.crest_directory.iter().flat_map(|e| e.to_le_bytes()).collect();
            self.out
                .seek(SeekFrom::Start(directory.offset.into()))
                .map_err(|e| format!("seeking to the crest directory: {e}"))?;
            self.out.write_all(&bytes).map_err(|e| format!("patching the crest directory: {e}"))?;
        }
        if let Some((layout, _)) = self.cell_index {
            let mut prefix = vec![0; layout.end() as usize];
            fill_cell_index(&mut prefix, self.rect, &self.cell_maxima)?;
            self.out.seek(SeekFrom::Start(layout.offset.into())).map_err(|e| format!("seeking to cell maxima: {e}"))?;
            self.out.write_all(&prefix[layout.offset as usize..]).map_err(|e| format!("writing cell maxima: {e}"))?;
        }
        self.out.flush().map_err(|e| format!("flushing OBCT output: {e}"))?;
        Ok(self.out)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn block(len: u32, fill: u8) -> Vec<u8> {
        vec![fill; len as usize]
    }

    /// The smallest legal pairing (a cell exactly one tile wide) written as a 1 × 1 container: the
    /// published-cell shape, byte for byte against the spec's §4.1 layout.
    #[test]
    fn a_one_by_one_container_is_header_directory_block() {
        let rect = CellRect { min_i: 7, min_j: 9, rows: 1, cols: 1 };
        let mut w = ShardWriter::new(Cursor::new(Vec::new()), 9, 13, rect).unwrap();
        assert_eq!(w.block_len(), 512, "a 2^13 cell at 2^9 posting is exactly one tile");
        w.push(Some(&block(512, 0xAB))).unwrap();
        let bytes = w.finish().unwrap().into_inner();

        assert_eq!(bytes.len(), 32 + 4 + 512);
        assert_eq!(&bytes[..4], b"OBCT");
        assert_eq!(bytes[4..8], [1, 9, 13, 0]);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 9);
        assert_eq!(u16::from_le_bytes(bytes[16..18].try_into().unwrap()), 1);
        assert_eq!(u16::from_le_bytes(bytes[18..20].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[20..24].try_into().unwrap()), 32);
        assert!(bytes[24..32].iter().all(|&b| b == 0), "the reserved bytes are a rejection condition");
        assert_eq!(u32::from_le_bytes(bytes[32..36].try_into().unwrap()), 36, "the block starts after the directory");
        assert!(bytes[36..].iter().all(|&b| b == 0xAB));
    }

    /// **The two emitters are one layout.** [`container_prefix`] claims to return exactly what
    /// [`ShardWriter::finish`] would have patched in; this is that claim, checked byte-for-byte over
    /// a rectangle with present and absent slots in both orders, so a directory that started at the
    /// wrong cursor or skipped an absent slot's four bytes cannot pass.
    ///
    /// It is the whole guarantee behind the streamed splice of `OBCM_Spec.md` §1.3: the assembler
    /// writes this prefix into the middle of a map it cannot seek back into, and a container that
    /// disagreed with the baker's by one entry would be a raster read at the wrong offsets.
    #[test]
    fn the_streamed_prefix_is_what_the_shard_writer_patches() {
        let rect = CellRect { min_i: 3, min_j: 5, rows: 2, cols: 3 };
        for present in [
            [true, false, true, false, false, true],
            [false, false, false, false, false, false],
            [true, true, true, true, true, true],
            [false, true, false, true, true, false],
        ] {
            let mut w = ShardWriter::new(Cursor::new(Vec::new()), 9, 13, rect).unwrap();
            for (k, &here) in present.iter().enumerate() {
                w.push(here.then(|| block(512, k as u8)).as_deref()).unwrap();
            }
            let patched = w.finish().unwrap().into_inner();

            let mut streamed = container_prefix(9, 13, rect, &present).unwrap();
            for (k, _) in present.iter().enumerate().filter(|(_, here)| **here) {
                streamed.extend_from_slice(&block(512, k as u8));
            }
            assert_eq!(streamed, patched, "presence {present:?}");
        }
        // The prefix is the whole of what a seek would have fixed up, so it is exactly the header
        // and the directory — never a byte of raster.
        let prefix = container_prefix(9, 13, rect, &[false; 6]).unwrap();
        assert_eq!(prefix.len(), HEADER_LEN + 6 * DIR_ENTRY_LEN);
        // A presence plan that does not describe this rectangle is a caller bug, not a shorter file.
        assert!(container_prefix(9, 13, rect, &[false; 5]).is_err(), "one entry per slot");
        // The pairing and rectangle refusals are the writer's own, reached through the same `validate`.
        assert!(container_prefix(9, 12, rect, &[false; 6]).is_err(), "not a pairing OBCT permits");
    }

    #[test]
    fn surface_cell_index_matches_both_writers_and_preserves_unknowns() {
        let rect = CellRect { min_i: 3, min_j: 5, rows: 3, cols: 5 };
        let layout = SurfaceLayout::new(9, 13).unwrap();
        let level = layout.level(0).unwrap();
        let root = level.bound_offset(0, 0, level.samples_log2).unwrap() as usize;
        let mut maxima: Vec<i16> = (0..15).map(|n| 100 + n).collect();
        maxima[0] = i16::MAX;
        let present: Vec<bool> = maxima.iter().map(|&h| h != i16::MAX).collect();
        let mut streamed = container_prefix_with_surface(9, 13, rect, &present, true).unwrap();
        fill_cell_index(&mut streamed, rect, &maxima).unwrap();
        let mut writer = ShardWriter::with_surface(Cursor::new(Vec::new()), 9, 13, rect, true).unwrap();
        for (&height, &here) in maxima.iter().zip(&present) {
            if here {
                let mut cell = vec![0; layout.cell_bytes() as usize];
                cell[root..root + 2].copy_from_slice(&height.to_le_bytes());
                writer.push(Some(&cell)).unwrap();
                streamed.extend_from_slice(&cell);
            } else {
                writer.push(None).unwrap();
            }
        }
        assert_eq!(writer.finish().unwrap().into_inner(), streamed);
        let index = CellIndexLayout::new(rect.rows, rect.cols, HEADER_LEN as u32).unwrap();
        let read = |y, x, log| {
            let at = index.node_offset(y, x, log).unwrap() as usize;
            i16::from_le_bytes(streamed[at..at + 2].try_into().unwrap())
        };
        assert_eq!(read(0, 0, 1), i16::MAX);
        assert_eq!(read(1, 2, 1), 114, "partial edge parent excludes cells outside coverage");
        assert_eq!(read(0, 0, 3), i16::MAX, "missing data stays unbounded through the root");
        assert!(streamed[index.offset as usize + index.bytes as usize..index.end() as usize].iter().all(|&b| b == 0));
    }

    /// An absent cell costs its four directory bytes and nothing else, and the blocks that *are*
    /// present stay contiguous behind the directory.
    #[test]
    fn an_absent_cell_is_a_zero_slot_and_no_bytes() {
        let rect = CellRect { min_i: 0, min_j: 0, rows: 2, cols: 2 };
        let mut w = ShardWriter::new(Cursor::new(Vec::new()), 9, 13, rect).unwrap();
        w.push(Some(&block(512, 1))).unwrap();
        w.push(None).unwrap();
        w.push(None).unwrap();
        w.push(Some(&block(512, 2))).unwrap();
        let bytes = w.finish().unwrap().into_inner();

        assert_eq!(bytes.len(), 32 + 16 + 2 * 512);
        let dir: Vec<u32> = bytes[32..48].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
        assert_eq!(dir, vec![48, 0, 0, 48 + 512]);
    }

    /// The rectangle iterates row-major with latitude as the row — the directory's own order, and
    /// the order `push` expects cells in.
    #[test]
    fn the_rectangle_walks_in_directory_order() {
        let rect = CellRect { min_i: 10, min_j: 20, rows: 2, cols: 3 };
        let cells: Vec<(u32, u32)> = rect.cells().collect();
        assert_eq!(cells, vec![(10, 20), (10, 21), (10, 22), (11, 20), (11, 21), (11, 22)]);
        assert_eq!(rect.slots(), 6);
    }

    #[test]
    fn structurally_impossible_containers_are_refused_at_open() {
        let rect = CellRect { min_i: 0, min_j: 0, rows: 1, cols: 1 };
        // A cell smaller than one tile is not a pairing OBCT permits.
        assert!(ShardWriter::new(Cursor::new(Vec::new()), 9, 12, rect).is_err());
        assert!(ShardWriter::new(Cursor::new(Vec::new()), 9, 13, rect).is_ok());
        // An empty rectangle, and one that runs off the world grid.
        assert!(ShardWriter::new(Cursor::new(Vec::new()), 9, 13, CellRect { min_i: 0, min_j: 0, rows: 0, cols: 1 })
            .is_err());
        let last = axis_cells(13);
        assert!(ShardWriter::new(Cursor::new(Vec::new()), 9, 13, CellRect { min_i: last, min_j: 0, rows: 1, cols: 1 })
            .is_err());
        // A wide-but-sparse rectangle is fine. 64 × 64 v1 cells would be 8 GiB if every one were
        // present, but a shard is not obliged to carry them — refusing it here would reject files
        // that write perfectly well, so the uint32 bound is enforced per block as they arrive.
        let wide = CellRect { min_i: 0, min_j: 0, rows: 64, cols: 64 };
        assert!(ShardWriter::new(Cursor::new(Vec::new()), 9, 19, wide).is_ok());
        // A directory so wide that no block could follow it inside a uint32 *is* refused, though.
        let vast = CellRect { min_i: 0, min_j: 0, rows: u16::MAX, cols: u16::MAX };
        assert!(ShardWriter::new(Cursor::new(Vec::new()), 9, 19, vast).is_err());
    }

    #[test]
    fn a_short_block_or_a_short_run_is_an_error_not_a_truncated_file() {
        let rect = CellRect { min_i: 0, min_j: 0, rows: 1, cols: 2 };
        let mut w = ShardWriter::new(Cursor::new(Vec::new()), 9, 13, rect).unwrap();
        assert!(w.push(Some(&block(256, 0))).is_err(), "a half block would desynchronise every later offset");
        let mut w = ShardWriter::new(Cursor::new(Vec::new()), 9, 13, rect).unwrap();
        w.push(None).unwrap();
        assert!(w.finish().is_err(), "a directory slot that was never offered is a bug, not an absent cell");
    }
}
