//! [`TerrainReader`] — parse an OBCT container and sample it (`OBCT_Spec.md` §4, §5).
//!
//! The reader owns *policy*: what a malformed file is, what a query outside coverage answers, and
//! the exact arithmetic of a bilinear sample. The byte facts it works from — magic, field offsets,
//! sentinels, the layout formulas — all come from [`obc_formats::obct`], the same split
//! `obc-reader` keeps with [`obc_formats::obcm`].

use core::sync::atomic::{AtomicU32, Ordering};

use obc_formats::io::{checked_rd_u16, checked_rd_u32, ByteSource, DecodeError, Error};
use obc_formats::obct::{
    cell_block_len, cell_samples_log2, cell_tiles_log2, sample_offset_in_tile, tile_offset_in_cell,
    validate_header_prefix, DIR_ABSENT, DIR_ENTRY_LEN, GRID_ORIGIN, HDR_CELL_COLS, HDR_CELL_LOG2, HDR_CELL_MIN_I,
    HDR_CELL_MIN_J, HDR_CELL_ROWS, HDR_DIRECTORY_OFFSET, HDR_FLAGS, HDR_POSTING_LOG2, HDR_RESERVED, HEADER_LEN, NODATA,
    TILE_BYTES, TILE_LOG2, TILE_SAMPLES,
};

use crate::grid::{axis_cells, cell_base_sample, cell_of, lattice_coord, locate};
use crate::TileCache;

/// Directory entries validated per read at parse time: 32 × 4 B = 128 B of parse-time stack, which
/// is two orders below the frames #419 went hunting for and is gone before the first sample.
const DIR_SCAN_ENTRIES: usize = 32;

/// Session-unique parse identity, never 0 (a zeroed [`TileCache`] sits at generation 0 = unowned).
static GEN: AtomicU32 = AtomicU32::new(0);

/// The parsed OBCT header (`OBCT_Spec.md` §4.2), all of it resident: 32 bytes, nothing else about
/// the file is held. The directory itself stays on the medium and is read one `uint32` at a time
/// behind the cache's cell memo — a DACH shard's directory is ~2 KB, and a crate that must not
/// allocate has nowhere to put it that isn't a fixed-capacity buffer sized for a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainHeader {
    /// `log2` of the sample posting in µdeg (v1 data: 9).
    pub posting_log2: u8,
    /// `log2` of the terrain cell side in µdeg (v1 data: 19).
    pub cell_log2: u8,
    /// Reserved encoding flags; `0` in v1 and rejected otherwise.
    pub flags: u8,
    /// Cell-rectangle origin on the OBCA grid: minimum cell index in latitude / longitude.
    pub cell_min_i: u32,
    pub cell_min_j: u32,
    /// Cell-rectangle extent, ≥ 1 each. A single **cell file** is `1 × 1`.
    pub cell_rows: u16,
    pub cell_cols: u16,
    /// Absolute byte offset of the offset directory (32 in v1).
    pub directory_offset: u32,
}

impl TerrainHeader {
    /// Samples along one cell edge as a `log2` — validated at parse, so this cannot fail here.
    #[inline]
    fn cell_samples_log2(&self) -> u8 {
        self.cell_log2 - self.posting_log2
    }

    /// The rectangle's bounding square in µdeg, `(min_lat, min_lon, max_lat, max_lon)`, half-open on
    /// the max edges like every OBCA cell square. `int64` because the world box legally overhangs
    /// ±90 / ±180 (OBCA §1.4) and the max corner of the topmost cell is `GRID_ORIGIN + 2^29`.
    pub fn bbox_udeg(&self) -> (i64, i64, i64, i64) {
        let side = 1i64 << self.cell_log2;
        let min_lat = GRID_ORIGIN as i64 + self.cell_min_i as i64 * side;
        let min_lon = GRID_ORIGIN as i64 + self.cell_min_j as i64 * side;
        (min_lat, min_lon, min_lat + self.cell_rows as i64 * side, min_lon + self.cell_cols as i64 * side)
    }
}

/// A parsed OBCT container over a byte source, with the sampling rules of `OBCT_Spec.md` §5.
///
/// Cheap to hold (a header, a source reference and a generation stamp) and cheap to build — but not
/// free: `parse` validates the whole directory, so build it once per mounted terrain file rather
/// than per query. Every resident byte of terrain lives in the caller's [`TileCache`].
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
    /// Parse and **fully validate** an OBCT container: the prefix, the posting/cell pairing, the
    /// cell rectangle against the world grid, and every directory entry against the file's own
    /// length. A file that survives this cannot make [`sample`](Self::sample) read outside itself,
    /// which is why validation is eager rather than per query — the alternative is a bounds check on
    /// the hot path for a fault that is a property of the file, not of the query.
    ///
    /// Errors reuse the shared [`Error`] set: [`Error::BadMagic`] / [`Error::BadVersion`] for the
    /// prefix and for an unknown `flags` bit (a file using an encoding this build does not have is
    /// not a file it may guess at), and [`Error::BadOffset`] for every structural rejection — they
    /// are all one fault: *this file's arithmetic does not close*.
    pub fn parse(src: &'a dyn ByteSource) -> Result<TerrainReader<'a>, Error> {
        let mut head = [0u8; HEADER_LEN];
        src.read_at(0, &mut head)?;
        validate_header_prefix(&head).map_err(|e| match e {
            DecodeError::Version => Error::BadVersion,
            _ => Error::BadMagic,
        })?;

        let flags = head[HDR_FLAGS];
        if flags != 0 {
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

        // The posting/cell pairing is the file's shape: everything below is arithmetic on it.
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
        let entries = header.cell_rows as u64 * header.cell_cols as u64;
        let dir_start = header.directory_offset as u64;
        let dir_end = dir_start + entries * DIR_ENTRY_LEN as u64;
        if dir_start < HEADER_LEN as u64 || dir_end > total {
            return Err(Error::BadOffset);
        }

        let reader = TerrainReader { src, header, cell_bytes, cell_tiles_log2, generation: next_generation() };
        reader.validate_directory(dir_start, dir_end, entries, total)?;
        Ok(reader)
    }

    /// Every directory entry is either [`DIR_ABSENT`] or an even offset addressing a whole cell
    /// block that lies behind the directory and inside the file. Read in [`DIR_SCAN_ENTRIES`]-entry
    /// batches so a wide rectangle costs a handful of medium reads, not one per cell.
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
                if offset % 2 != 0 || start < dir_end || start + self.cell_bytes as u64 > total {
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
    /// surrounding lattice samples per `OBCT_Spec.md` §5 — or `None` when the point is not covered,
    /// any contributing sample is [`NODATA`], or the medium failed.
    ///
    /// The three `None` cases are deliberately indistinguishable to the caller. Every consumer of
    /// elevation already has to behave sanely without it (that is what [`NullElevation`] pins), so
    /// giving them a fourth thing to branch on would buy nothing but branches.
    ///
    /// [`NullElevation`]: crate::NullElevation
    pub fn sample<const N: usize>(&self, cache: &mut TileCache<N>, lat_udeg: i32, lon_udeg: i32) -> Option<i16> {
        cache.adopt(self.generation);
        let at = locate(lat_udeg, lon_udeg, self.header.posting_log2)?;

        // §5.1 step 3: the query's own cell must be present. Nothing is extrapolated into a hole or
        // beyond coverage — only *corners* are ever clamped.
        let (home_i, home_j) = (
            cell_of(at.i, self.header.posting_log2, self.header.cell_log2),
            cell_of(at.j, self.header.posting_log2, self.header.cell_log2),
        );
        // A failed directory read and an absent cell both leave the query unanswered here; the
        // distinction only matters for a *corner* (§5.3), where one clamps and the other must not.
        let home = self.cell_offset(cache, home_i, home_j).ok().flatten()?;

        let v00 = self.corner(cache, (home_i, home_j), home, at.i, at.j)?;
        let v10 = self.corner(cache, (home_i, home_j), home, at.i + 1, at.j)?;
        let v01 = self.corner(cache, (home_i, home_j), home, at.i, at.j + 1)?;
        let v11 = self.corner(cache, (home_i, home_j), home, at.i + 1, at.j + 1)?;
        Some(bilinear(v00, v01, v10, v11, at.frac_lat, at.frac_lon, self.header.posting_log2))
    }

    /// One bilinear corner (`OBCT_Spec.md` §5.3): read sample `(i, j)` from whichever cell owns it,
    /// falling back to the nearest sample of the **home** cell when that cell is not in the file —
    /// the coverage-edge clamp. `None` means the sample read as [`NODATA`], or the medium failed on
    /// the way to it; §5.4 propagates either to the whole query. Only *absence* clamps: a failed
    /// read is never answered with a neighbouring height.
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
                // The medium failed: void the sample. A clamp here would answer a read error with a
                // plausible height, which is exactly the guess the format forbids.
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
    /// **The three answers are deliberately not collapsed.** "Absent" is a fact about the file and
    /// makes a corner clamp (§5.3); a failed read is a fact about the *card* and must void the whole
    /// sample. Folding the error into `None` would let an SD glitch hand back a clamped, entirely
    /// plausible height — the one thing the format's "a hole is silence, never a guess" principle
    /// forbids, and the reason the tile path invalidates its slot rather than serving a short read.
    ///
    /// One `uint32` read behind the cache's one-entry memo. A query whose four corners land in the
    /// same cell therefore costs one directory read between them; at a seam the memo ping-pongs
    /// between the home cell and its neighbour, so a straddling query can pay one read per crossing
    /// corner — cheap next to the tile read it is amortising, and the reason this is a memo rather
    /// than a resident directory.
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

    /// The µdeg coordinate of lattice sample `i` — exposed so a caller (the `obc-dem` baker's
    /// cross-check, a debug overlay) can name the sample a query landed on.
    #[inline]
    pub fn lattice_coord(&self, i: u32) -> i32 {
        lattice_coord(i, self.header.posting_log2)
    }
}

/// Integer bilinear interpolation (`OBCT_Spec.md` §5.2).
///
/// The weights are the sub-posting remainders themselves, so the whole expression stays in `i64`:
/// with `P = 2^posting_log2`, `a = frac_lat` and `b = frac_lon`,
///
/// ```text
/// num = v00·(P−a)·(P−b) + v10·a·(P−b) + v01·(P−a)·b + v11·a·b
/// h   = round_half_away_from_zero(num / P²)
/// ```
///
/// **Rounding is half-away-from-zero**, not `floor`: elevation is signed and a rider crossing sea
/// level should not see the rounding bias flip sign with the terrain. It is also the one rule that
/// needs no `div_euclid` — truncating division plus a sign test reproduces it in any language,
/// which matters because the packer, the device and (later) the browser all evaluate this.
///
/// No corner may be [`NODATA`] here (§5.4 rejects the query first), so `num / P²` is a weighted mean
/// of values in `-32767..=32767` and the `i16` cast is lossless.
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
/// never-live "unowned cache" value.
fn next_generation() -> u32 {
    GEN.fetch_add(1, Ordering::Relaxed) + 1
}

const _: () = assert!(TILE_BYTES == 512);

#[cfg(test)]
mod tests {
    use super::*;

    /// A lattice point (both remainders zero) returns that sample untouched — the identity the
    /// whole seam rests on, because two neighbouring cells only agree at a seam if this holds.
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

    /// Rounding is symmetric about zero — the property `floor` would not have.
    #[test]
    fn rounding_is_half_away_from_zero() {
        // A quarter-weight on a 1 m step: 0.25 rounds to 0, 0.5 rounds away from zero.
        assert_eq!(bilinear(0, 0, 1, 1, 128, 0, 9), 0);
        assert_eq!(bilinear(0, 0, 1, 1, 256, 0, 9), 1);
        assert_eq!(bilinear(0, 0, -1, -1, 128, 0, 9), 0);
        assert_eq!(bilinear(0, 0, -1, -1, 256, 0, 9), -1);
    }

    /// The extreme legal values, at the extreme legal posting: no overflow, no clipping.
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
