//! Writing the OBCT container (`OBCT_Spec.md` §4): a 32-byte header, a row-major `uint32` offset
//! directory over the cell rectangle, then the present cells' blocks.
//!
//! There is **one** writer for both published artifacts, because there is one format: a terrain
//! *cell* is a shard whose rectangle is 1 × 1 (spec §4.1, principle 5). A baker that had two code
//! paths would be the first place the two could drift.
//!
//! Every byte fact — the magic, the field offsets, the absent sentinel, the block length — comes
//! from [`obc_formats::obct`]. Nothing in this file transcribes the header table.

use std::io::{Seek, SeekFrom, Write};

use obc_formats::obct::{
    cell_block_len, cell_samples_log2, DIR_ABSENT, DIR_ENTRY_LEN, HDR_CELL_COLS, HDR_CELL_LOG2, HDR_CELL_MIN_I,
    HDR_CELL_MIN_J, HDR_CELL_ROWS, HDR_DIRECTORY_OFFSET, HDR_FLAGS, HDR_MAGIC, HDR_POSTING_LOG2, HDR_VERSION,
    HEADER_LEN, MAGIC, VERSION,
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
}

impl<W: Write + Seek> ShardWriter<W> {
    /// Open a container over `out` for a `posting_log2` / `cell_log2` pairing and a cell rectangle,
    /// writing the header and a fully-absent directory.
    pub fn new(mut out: W, posting_log2: u8, cell_log2: u8, rect: CellRect) -> Result<Self, String> {
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
        // (see [`push`]) rather than against the rectangle's worst case: a wide rectangle that is
        // mostly absent is a perfectly ordinary shard, and refusing it here because a hypothetically
        // full one would overflow would reject files that are entirely writable.
        let dir_end = HEADER_LEN as u64 + rect.slots() * DIR_ENTRY_LEN as u64;
        if dir_end + block_len as u64 > u32::MAX as u64 {
            return Err(format!(
                "a {}×{} directory at 2^{cell_log2} µdeg leaves no room inside the uint32 offsets it is made of",
                rect.rows, rect.cols
            ));
        }

        let mut header = [0u8; HEADER_LEN];
        header[HDR_MAGIC..HDR_MAGIC + 4].copy_from_slice(&MAGIC);
        header[HDR_VERSION] = VERSION;
        header[HDR_POSTING_LOG2] = posting_log2;
        header[HDR_CELL_LOG2] = cell_log2;
        header[HDR_FLAGS] = 0; // v1 defines no encoding flags; a reader must refuse any bit set
        header[HDR_CELL_MIN_I..HDR_CELL_MIN_I + 4].copy_from_slice(&rect.min_i.to_le_bytes());
        header[HDR_CELL_MIN_J..HDR_CELL_MIN_J + 4].copy_from_slice(&rect.min_j.to_le_bytes());
        header[HDR_CELL_ROWS..HDR_CELL_ROWS + 2].copy_from_slice(&rect.rows.to_le_bytes());
        header[HDR_CELL_COLS..HDR_CELL_COLS + 2].copy_from_slice(&rect.cols.to_le_bytes());
        // The directory follows the header immediately, which is what a v1 producer MUST write —
        // the field is explicit anyway so a reader follows it rather than the assumption.
        header[HDR_DIRECTORY_OFFSET..HDR_DIRECTORY_OFFSET + 4].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());
        // 24..32 stay zero: reserved, and a reader refuses the file if they are not.

        out.write_all(&header).map_err(|e| format!("writing OBCT header: {e}"))?;
        let slots = rect.slots() as usize;
        out.write_all(&vec![0u8; slots * DIR_ENTRY_LEN]).map_err(|e| format!("writing OBCT directory: {e}"))?;

        Ok(ShardWriter {
            out,
            block_len,
            cell_log2,
            directory: vec![DIR_ABSENT; slots],
            next_slot: 0,
            cursor: HEADER_LEN as u32 + (slots * DIR_ENTRY_LEN) as u32,
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
        let slot = self.next_slot;
        if slot >= self.directory.len() {
            return Err("more cells offered than the rectangle has slots".to_string());
        }
        self.next_slot += 1;
        let Some(block) = block else { return Ok(()) };
        if block.len() != self.block_len as usize {
            return Err(format!("cell block is {} bytes, expected {}", block.len(), self.block_len));
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
        let dir: Vec<u32> = bytes[32..48].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
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
