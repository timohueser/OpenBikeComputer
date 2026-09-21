//! [`TerrainReader`]: parse an OBCT container and sample it.
//!
//! The reader owns policy: what a malformed file is, what a query outside coverage answers, and
//! the exact arithmetic of a bilinear sample. The byte facts come from [`obc_formats::obct`].

use core::sync::atomic::{AtomicU32, Ordering};

use obc_formats::io::{checked_rd_u16, checked_rd_u32, ByteSource, DecodeError, Error};
use obc_formats::obct::{
    cell_block_len, cell_samples_log2, cell_tiles_log2, sample_offset_in_tile, tile_offset_in_cell,
    validate_header_prefix, CellIndexLayout, SurfaceLayout, CELL_INDEX_FLAG, DIR_ABSENT, DIR_ENTRY_LEN, GRID_ORIGIN,
    HDR_CELL_COLS, HDR_CELL_LOG2, HDR_CELL_MIN_I, HDR_CELL_MIN_J, HDR_CELL_ROWS, HDR_DIRECTORY_OFFSET, HDR_FLAGS,
    HDR_POSTING_LOG2, HDR_RESERVED, HEADER_LEN, NODATA, SURFACE_FLAG, SURFACE_VERSION, TILE_BYTES, TILE_LOG2,
    TILE_SAMPLES,
};

use crate::grid::{axis_cells, cell_base_sample, cell_of, lattice_coord, locate};
use crate::TileCache;

/// Directory entries validated per read at parse time: 128 B of parse-time stack, gone before the
/// first sample.
const DIR_SCAN_ENTRIES: usize = 32;

/// Session-unique parse identity, never 0 (a zeroed [`TileCache`] sits at generation 0 = unowned).
static GEN: AtomicU32 = AtomicU32::new(0);

/// The parsed OBCT header, all of it resident: 32 bytes, and nothing else about the file is held.
/// The directory stays on the medium and is read one `uint32` at a time behind the cache's memo,
/// because a crate that must not allocate has nowhere to put a whole directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainHeader {
    /// `log2` of the sample posting in µdeg (v1 data: 9).
    pub posting_log2: u8,
    /// `log2` of the terrain cell side in µdeg (v1 data: 19).
    pub cell_log2: u8,
    /// Zero for native v1 cells; `SURFACE_FLAG` for indexed v2 cells.
    pub flags: u8,
    /// Cell-rectangle origin on the OBCA grid: minimum cell index in latitude and longitude.
    pub cell_min_i: u32,
    pub cell_min_j: u32,
    /// Cell-rectangle extent, at least 1 each. A single cell file is `1 × 1`.
    pub cell_rows: u16,
    pub cell_cols: u16,
    /// Absolute byte offset of the offset directory (32 in v1).
    pub directory_offset: u32,
}

impl TerrainHeader {
    /// Samples along one cell edge as a `log2`, validated at parse.
    #[inline]
    fn cell_samples_log2(&self) -> u8 {
        self.cell_log2 - self.posting_log2
    }

    /// The rectangle's bounding square in µdeg, half-open on the max edges like every OBCA cell
    /// square. `int64` because the world box legally overhangs ±90 and ±180.
    pub fn bbox_udeg(&self) -> (i64, i64, i64, i64) {
        let side = 1i64 << self.cell_log2;
        let min_lat = GRID_ORIGIN as i64 + self.cell_min_i as i64 * side;
        let min_lon = GRID_ORIGIN as i64 + self.cell_min_j as i64 * side;
        (min_lat, min_lon, min_lat + self.cell_rows as i64 * side, min_lon + self.cell_cols as i64 * side)
    }
}

/// Validated terrain layout and cache identity, independent of a borrow of its bytes. Keep it with
/// the immutable source used to parse it.
pub struct TerrainTables {
    header: TerrainHeader,
    cell_bytes: u32,
    cell_tiles_log2: u8,
    generation: u32,
}

impl TerrainTables {
    /// Validate once using the same parser as a directly borrowed reader.
    pub fn parse(src: &dyn ByteSource) -> Result<Self, Error> {
        let reader = TerrainReader::parse(src)?;
        Ok(Self {
            header: reader.header,
            cell_bytes: reader.cell_bytes,
            cell_tiles_log2: reader.cell_tiles_log2,
            generation: reader.generation,
        })
    }

    /// Borrow the same immutable source used for validation, without re-reading bytes. All views
    /// share the parse generation, so a caller can retain one tile cache.
    pub fn reader<'a>(&self, src: &'a dyn ByteSource) -> TerrainReader<'a> {
        TerrainReader {
            src,
            header: self.header,
            cell_bytes: self.cell_bytes,
            cell_tiles_log2: self.cell_tiles_log2,
            generation: self.generation,
        }
    }
}

/// A parsed OBCT container over a byte source.
///
/// Cheap to hold and cheap to build, but not free: `parse` validates the whole directory, so build
/// it once per mounted terrain file rather than per query. Every resident byte of terrain lives in
/// the caller's [`TileCache`].
pub struct TerrainReader<'a> {
    src: &'a dyn ByteSource,
    header: TerrainHeader,
    /// Cell-block length in bytes, derived once from the posting/cell pair.
    cell_bytes: u32,
    /// `log2` of tiles per cell edge, derived once.
    cell_tiles_log2: u8,
    generation: u32,
}

impl<'a> TerrainReader<'a> {
    /// Parse and fully validate an OBCT container: the prefix, the posting and cell pairing, the
    /// cell rectangle against the world grid, and every directory entry against the file's length.
    /// A file that survives this cannot make [`sample`](Self::sample) read outside itself, which
    /// is why validation is eager rather than per query.
    ///
    /// [`Error::BadMagic`] and [`Error::BadVersion`] cover the prefix and an unknown `flags` bit;
    /// [`Error::BadOffset`] covers every structural rejection, because they are all one fault:
    /// this file's arithmetic does not close.
    pub fn parse(src: &'a dyn ByteSource) -> Result<TerrainReader<'a>, Error> {
        let mut head = [0u8; HEADER_LEN];
        src.read_at(0, &mut head)?;
        validate_header_prefix(&head).map_err(|e| match e {
            DecodeError::Version => Error::BadVersion,
            _ => Error::BadMagic,
        })?;

        let flags = head[HDR_FLAGS];
        if !((head[4] == 1 && flags == 0) || (head[4] == SURFACE_VERSION && flags & !CELL_INDEX_FLAG == SURFACE_FLAG)) {
            return Err(Error::BadVersion);
        }
        let header = TerrainHeader {
            posting_log2: head[HDR_POSTING_LOG2],
            cell_log2: head[HDR_CELL_LOG2],
            flags,
            cell_min_i: checked_rd_u32(&head, HDR_CELL_MIN_I).map_err(|_| Error::BadOffset)?,
            cell_min_j: checked_rd_u32(&head, HDR_CELL_MIN_J).map_err(|_| Error::BadOffset)?,
            cell_rows: checked_rd_u16(&head, HDR_CELL_ROWS).map_err(|_| Error::BadOffset)?,
            cell_cols: checked_rd_u16(&head, HDR_CELL_COLS).map_err(|_| Error::BadOffset)?,
            directory_offset: checked_rd_u32(&head, HDR_DIRECTORY_OFFSET).map_err(|_| Error::BadOffset)?,
        };
        if head[HDR_RESERVED..HEADER_LEN].iter().any(|&b| b != 0) {
            return Err(Error::BadOffset);
        }

        // The posting and cell pairing is the file's shape; everything below is arithmetic on it.
        cell_samples_log2(header.posting_log2, header.cell_log2).ok_or(Error::BadOffset)?;
        let cell_tiles_log2 = cell_tiles_log2(header.posting_log2, header.cell_log2).ok_or(Error::BadOffset)?;
        let cell_bytes = cell_block_len(header.posting_log2, header.cell_log2).ok_or(Error::BadOffset)?;

        // A rectangle of at least one cell, wholly inside the world grid at this cell size.
        if header.cell_rows == 0 || header.cell_cols == 0 {
            return Err(Error::BadOffset);
        }
        let axis = axis_cells(header.cell_log2) as u64;
        if header.cell_min_i as u64 + header.cell_rows as u64 > axis {
            return Err(Error::BadOffset);
        }
        if header.cell_min_j as u64 + header.cell_cols as u64 > axis {
            return Err(Error::BadOffset);
        }

        // The directory: fully present, after the header, and not overlapping the cell blocks.
        let total = src.len();
        if flags & SURFACE_FLAG != 0 && total > u32::MAX.into() {
            return Err(Error::BadOffset);
        }
        let entries = header.cell_rows as u64 * header.cell_cols as u64;
        let dir_start = header.directory_offset as u64;
        let dir_end = dir_start + entries * DIR_ENTRY_LEN as u64;
        if dir_start < HEADER_LEN as u64 || dir_end > total {
            return Err(Error::BadOffset);
        }

        let cell_bytes = if flags & SURFACE_FLAG != 0 {
            SurfaceLayout::new(header.posting_log2, header.cell_log2).ok_or(Error::BadOffset)?.cell_bytes()
        } else {
            cell_bytes
        };
        let data_start = if flags & CELL_INDEX_FLAG != 0 {
            CellIndexLayout::new(header.cell_rows, header.cell_cols, header.directory_offset)
                .ok_or(Error::BadOffset)?
                .end() as u64
        } else {
            dir_end
        };
        if data_start > total {
            return Err(Error::BadOffset);
        }
        let reader = TerrainReader { src, header, cell_bytes, cell_tiles_log2, generation: next_generation() };
        reader.validate_directory(dir_start, data_start, entries, total)?;
        Ok(reader)
    }

    /// Every directory entry is either [`DIR_ABSENT`] or an even offset addressing a whole cell
    /// block behind the directory and inside the file. Read in batches, so a wide rectangle costs
    /// a handful of medium reads rather than one per cell.
    fn validate_directory(&self, dir_start: u64, dir_end: u64, entries: u64, total: u64) -> Result<(), Error> {
        let mut buf = [0u8; DIR_SCAN_ENTRIES * DIR_ENTRY_LEN];
        let mut done = 0u64;
        while done < entries {
            let n = ((entries - done) as usize).min(DIR_SCAN_ENTRIES);
            let bytes = &mut buf[..n * DIR_ENTRY_LEN];
            self.src.read_at(dir_start + done * DIR_ENTRY_LEN as u64, bytes)?;
            for entry in bytes.as_chunks::<DIR_ENTRY_LEN>().0 {
                let offset = u32::from_le_bytes([entry[0], entry[1], entry[2], entry[3]]);
                if offset == DIR_ABSENT {
                    continue;
                }
                let start = offset as u64;
                let alignment = if self.header.flags & SURFACE_FLAG != 0 { 512 } else { 2 };
                if offset % alignment != 0 || start < dir_end || start + self.cell_bytes as u64 > total {
                    return Err(Error::BadOffset);
                }
            }
            done += n as u64;
        }
        Ok(())
    }

    /// The parsed header.
    #[inline]
    pub fn header(&self) -> &TerrainHeader {
        &self.header
    }

    /// This parse's session-unique identity, the key a [`TileCache`] binds to.
    #[inline]
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// The height at `(lat, lon)` in whole metres, bilinearly interpolated over the four
    /// surrounding lattice samples, or `None` when the point is not covered, a contributing sample
    /// is [`NODATA`], or the medium failed.
    ///
    /// The three `None` cases are deliberately indistinguishable: every consumer of elevation
    /// already has to behave sanely without it.
    pub fn sample<const N: usize>(&self, cache: &mut TileCache<N>, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        cache.adopt(self.generation);
        let at = locate(lat_udeg, lon_udeg, self.header.posting_log2)?;

        // The query's own cell must be present. Nothing is extrapolated into a hole or beyond
        // coverage; only corners are ever clamped.
        let (home_i, home_j) = (
            cell_of(at.i, self.header.posting_log2, self.header.cell_log2),
            cell_of(at.j, self.header.posting_log2, self.header.cell_log2),
        );
        // A failed directory read and an absent cell both leave the query unanswered here; the
        // distinction only matters for a corner, where one clamps and the other must not.
        let home = self.cell_offset(cache, home_i, home_j).ok().flatten()?;

        let v00 = self.corner(cache, (home_i, home_j), home, at.i, at.j)?;
        let v10 = self.corner(cache, (home_i, home_j), home, at.i + 1, at.j)?;
        let v01 = self.corner(cache, (home_i, home_j), home, at.i, at.j + 1)?;
        let v11 = self.corner(cache, (home_i, home_j), home, at.i + 1, at.j + 1)?;
        Some(bilinear(v00, v01, v10, v11, at.frac_lat, at.frac_lon, self.header.posting_log2))
    }

    /// One bilinear corner: read sample `(i, j)` from whichever cell owns it, falling back to the
    /// nearest sample of the home cell when that cell is not in the file, which is the
    /// coverage-edge clamp. `None` means the sample read as [`NODATA`] or the medium failed. Only
    /// absence clamps: a failed read is never answered with a neighbouring height.
    fn corner<const N: usize>(
        &self,
        cache: &mut TileCache<N>,
        home_cell: (u32, u32),
        home_offset: u32,
        i: u32,
        j: u32,
    ) -> Option<i16> {
        let span_log2 = self.header.cell_samples_log2();
        let (ci, cj) = (
            cell_of(i, self.header.posting_log2, self.header.cell_log2),
            cell_of(j, self.header.posting_log2, self.header.cell_log2),
        );
        let (cell, offset, i, j) = if (ci, cj) == home_cell {
            (home_cell, home_offset, i, j)
        } else {
            match self.cell_offset(cache, ci, cj) {
                Ok(Some(offset)) => ((ci, cj), offset, i, j),
                // The medium failed, so void the sample. A clamp here would answer a read error
                // with a plausible height, which is the guess the format forbids.
                Err(_) => return None,
                // Clamp each out-of-cell axis to the home cell's last sample on that axis.
                Ok(None) => {
                    let last_i = cell_base_sample(home_cell.0, self.header.posting_log2, self.header.cell_log2)
                        + (1 << span_log2)
                        - 1;
                    let last_j = cell_base_sample(home_cell.1, self.header.posting_log2, self.header.cell_log2)
                        + (1 << span_log2)
                        - 1;
                    (home_cell, home_offset, i.min(last_i), j.min(last_j))
                }
            }
        };

        let li = i - cell_base_sample(cell.0, self.header.posting_log2, self.header.cell_log2);
        let lj = j - cell_base_sample(cell.1, self.header.posting_log2, self.header.cell_log2);
        let tile = offset + tile_offset_in_cell(li >> TILE_LOG2, lj >> TILE_LOG2, self.cell_tiles_log2);
        let mask = TILE_SAMPLES as u32 - 1;
        let value = self.tile_sample(cache, tile, li & mask, lj & mask)?;
        (value != NODATA).then_some(value)
    }

    /// The `int16` at `(row, col)` of the tile starting at absolute offset `tile`, through the
    /// cache. A read failure invalidates the reserved slot rather than serving a half-filled tile.
    fn tile_sample<const N: usize>(&self, cache: &mut TileCache<N>, tile: u32, row: u32, col: u32) -> Option<i16> {
        let at = sample_offset_in_tile(row, col);
        if let Some(resident) = cache.get(tile) {
            return Some(i16::from_le_bytes([resident[at], resident[at + 1]]));
        }
        let (slot, buf) = cache.reserve(tile);
        if self.src.read_at(tile.into(), buf).is_err() {
            cache.invalidate(slot);
            return None;
        }
        let filled = cache.tile(slot);
        Some(i16::from_le_bytes([filled[at], filled[at + 1]]))
    }

    /// The offset of cell `(i, j)`: `Ok(Some(_))` present, `Ok(None)` outside the rectangle or
    /// absent from the directory, `Err(_)` the medium failed.
    ///
    /// The three answers are deliberately not collapsed. Absent is a fact about the file and makes
    /// a corner clamp; a failed read is a fact about the card and must void the whole sample.
    /// Folding the error into `None` would let an SD glitch hand back a clamped, plausible height.
    ///
    /// One `uint32` read behind the cache's one-entry memo, so a query whose four corners land in
    /// the same cell costs one directory read between them. At a seam the memo ping-pongs, which
    /// is cheap next to the tile read it is amortising.
    fn cell_offset<const N: usize>(&self, cache: &mut TileCache<N>, i: u32, j: u32) -> Result<Option<u32>, Error> {
        if let Some(offset) = cache.memo(i, j) {
            return Ok(Some(offset));
        }
        let (Some(di), Some(dj)) = (i.checked_sub(self.header.cell_min_i), j.checked_sub(self.header.cell_min_j))
        else {
            return Ok(None);
        };
        if di >= self.header.cell_rows as u32 || dj >= self.header.cell_cols as u32 {
            return Ok(None);
        }
        let slot = di as u64 * self.header.cell_cols as u64 + dj as u64;
        let at = self.header.directory_offset as u64 + slot * DIR_ENTRY_LEN as u64;
        let mut entry = [0u8; DIR_ENTRY_LEN];
        self.src.read_at(at, &mut entry)?;
        let offset = u32::from_le_bytes(entry);
        if offset == DIR_ABSENT {
            return Ok(None);
        }
        cache.remember(i, j, offset);
        Ok(Some(offset))
    }

    /// The µdeg coordinate of lattice sample `i`, exposed so a caller can name the sample a query
    /// landed on.
    #[inline]
    pub fn lattice_coord(&self, i: u32) -> i32 {
        lattice_coord(i, self.header.posting_log2)
    }
}

/// Integer bilinear interpolation.
///
/// The weights are the sub-posting remainders themselves, so the whole expression stays in `i64`:
/// with `P = 2^posting_log2`, `a = frac_lat` and `b = frac_lon`,
///
/// ```text
/// num = v00·(P−a)·(P−b) + v10·a·(P−b) + v01·(P−a)·b + v11·a·b
/// h   = round_half_away_from_zero(num / P²)
/// ```
///
/// Rounding is half-away-from-zero, not `floor`: elevation is signed, and a rider crossing sea
/// level should not see the rounding bias flip sign with the terrain. It also needs no
/// `div_euclid`, so the packer, the device and the browser all reproduce it.
///
/// No corner may be [`NODATA`] here, so `num / P²` is a weighted mean of values in
/// `-32767..=32767` and the `i16` cast is lossless.
#[inline]
fn bilinear(v00: i16, v01: i16, v10: i16, v11: i16, frac_lat: u32, frac_lon: u32, posting_log2: u8) -> i16 {
    let p = 1i64 << posting_log2;
    let (a, b) = (frac_lat as i64, frac_lon as i64);
    let num = v00 as i64 * (p - a) * (p - b) + v10 as i64 * a * (p - b) + v01 as i64 * (p - a) * b + v11 as i64 * a * b;
    let den = p * p;
    let half = den / 2;
    let rounded = if num >= 0 { (num + half) / den } else { -((-num + half) / den) };
    rounded as i16
}

/// Stamp a session-unique generation. `fetch_add + 1` starts the first parse at 1, so 0 stays the
/// never-live unowned value.
fn next_generation() -> u32 {
    GEN.fetch_add(1, Ordering::Relaxed) + 1
}

const _: () = assert!(TILE_BYTES == 512);

#[cfg(test)]
mod tests {
    use super::*;

    /// A lattice point returns that sample untouched: two neighbouring cells only agree at a seam
    /// if this holds.
    #[test]
    fn a_lattice_point_returns_its_own_sample() {
        for v in [-32767i16, -1, 0, 1, 4321, 32767] {
            assert_eq!(bilinear(v, 1000, 2000, 3000, 0, 0, 9), v);
        }
    }

    /// On a plane the interpolation is exact: halfway between four corners of a plane is their mean.
    #[test]
    fn the_midpoint_of_a_plane_is_the_mean_of_its_corners() {
        assert_eq!(bilinear(100, 110, 120, 130, 256, 256, 9), 115);
        assert_eq!(bilinear(0, 0, 100, 100, 256, 0, 9), 50, "pure latitude interpolation");
        assert_eq!(bilinear(0, 100, 0, 100, 0, 256, 9), 50, "pure longitude interpolation");
    }

    /// Rounding is symmetric about zero, the property `floor` would not have.
    #[test]
    fn rounding_is_half_away_from_zero() {
        // A quarter-weight on a 1 m step: 0.25 rounds to 0, 0.5 rounds away from zero.
        assert_eq!(bilinear(0, 0, 1, 1, 128, 0, 9), 0);
        assert_eq!(bilinear(0, 0, 1, 1, 256, 0, 9), 1);
        assert_eq!(bilinear(0, 0, -1, -1, 128, 0, 9), 0);
        assert_eq!(bilinear(0, 0, -1, -1, 256, 0, 9), -1);
    }

    /// The extreme legal values at the extreme legal posting: no overflow, no clipping.
    #[test]
    fn the_full_int16_range_survives_the_widest_posting() {
        let p = obc_formats::obct::MAX_POSTING_LOG2;
        let last = (1u32 << p) - 1;
        assert_eq!(bilinear(32767, 32767, 32767, 32767, last, last, p), 32767);
        assert_eq!(bilinear(-32767, -32767, -32767, -32767, last, last, p), -32767);
        assert_eq!(bilinear(-32767, 32767, -32767, 32767, 0, 1 << (p - 1), p), 0);
    }

    #[test]
    fn each_parse_gets_its_own_never_zero_generation() {
        let (a, b) = (next_generation(), next_generation());
        assert_ne!(a, 0);
        assert_ne!(a, b);
    }
}
