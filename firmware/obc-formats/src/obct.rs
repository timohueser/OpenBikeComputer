//! OBCT terrain-format constants from `OBCT_Spec.md`.
//!
//! The terrain artifact is a raster on the [OBCA] cell grid: a global sample lattice of `int16`
//! metre heights, cut into 512-byte tiles, grouped into grid cells, published either as a single
//! cell file or as a multi-cell shard — **one container**, because a cell file is only a shard
//! whose cell rectangle is 1×1.
//!
//! Like [`obcm`](crate::obcm) this module is the *byte authority* and nothing else: magic, version,
//! field offsets, the sentinels, and the pure layout arithmetic that turns a lattice coordinate
//! into a byte offset. No reader policy, no cache, no sampling — those live in `obc-elevation`,
//! which is the only consumer that decides what a malformed file means.
//!
//! **Two sizes are data, not shape** (the OBCA §1.5 idiom): the posting `P` and the cell side are
//! header fields, so retuning either is a terrain re-bake rather than a format bump. The *tile* is
//! not: 16 × 16 samples = 512 B is the device's I/O quantum (one SD block, the §8 nav-chunk size),
//! and changing it would change the fetch unit every consumer is budgeted around.
//!
//! [OBCA]: ../../../../specs/OBCA_Spec.md

use crate::io::{validate_prefix, DecodeError};

pub const MAGIC: [u8; 4] = *b"OBCT";
pub const VERSION: u8 = 1;
pub const HEADER_LEN: usize = 32;

/// Header field offsets (spec §4.2). Named rather than inlined because three implementations
/// (this reader, the `obc-dem` baker, and the vectors builder) transcribe the same table.
pub const HDR_MAGIC: usize = 0;
pub const HDR_VERSION: usize = 4;
pub const HDR_POSTING_LOG2: usize = 5;
pub const HDR_CELL_LOG2: usize = 6;
pub const HDR_FLAGS: usize = 7;
pub const HDR_CELL_MIN_I: usize = 8;
pub const HDR_CELL_MIN_J: usize = 12;
pub const HDR_CELL_ROWS: usize = 16;
pub const HDR_CELL_COLS: usize = 18;
pub const HDR_DIRECTORY_OFFSET: usize = 20;
/// Start of the 8 reserved header bytes (`24..32`), which a v1 producer MUST write as zero.
pub const HDR_RESERVED: usize = 24;

/// One directory slot: an absolute `uint32` byte offset to a cell block (spec §4.3).
pub const DIR_ENTRY_LEN: usize = 4;

/// "This cell is not in the file." Not a valid offset for any cell block, because the header
/// precedes every block — so no sentinel bit has to be carved out of the offset space.
pub const DIR_ABSENT: u32 = 0;

/// `log2` of a tile's edge in samples (spec §2).
pub const TILE_LOG2: u32 = 4;
/// Samples along one tile edge: 16.
pub const TILE_SAMPLES: usize = 1 << TILE_LOG2;
/// Bytes per height sample: an `int16`.
pub const SAMPLE_LEN: usize = 2;
/// Bytes in one tile: 16 × 16 × 2 = **512**, one SD block.
pub const TILE_BYTES: usize = TILE_SAMPLES * TILE_SAMPLES * SAMPLE_LEN;

/// "No height here" (spec §1.2). `i16::MIN`, so the whole of `-32767..=32767` stays available as
/// real orthometric metres — a producer MUST NOT emit `-32768` as a height.
pub const NODATA: i16 = i16::MIN;

/// Origin of the sample lattice on **both** axes, µdeg — the OBCA §1.1 grid origin, restated here
/// because the raster shares that grid and `obc-elevation` must not depend on a host crate to
/// learn it. `host/obcm-assemble`'s oracle test pins the three copies against each other.
pub const GRID_ORIGIN: i32 = -(1 << 28);
/// Side of the world box, µdeg (OBCA §1.1): `2^29`.
pub const WORLD_SIDE: u32 = 1 << 29;

/// Smallest permitted posting as `log2(µdeg)` — `2^4` µdeg ≈ 1.8 m, finer than any global DEM.
pub const MIN_POSTING_LOG2: u8 = 4;
/// Largest permitted posting as `log2(µdeg)` — `2^16` µdeg ≈ 7 km, coarser than any useful terrain.
pub const MAX_POSTING_LOG2: u8 = 16;
/// Smallest / largest permitted cell side as `log2(µdeg)`, matching the OBCA §1.1 cell-size range.
pub const MIN_CELL_LOG2: u8 = 10;
pub const MAX_CELL_LOG2: u8 = 28;
/// Largest permitted `log2` of a cell's tiles-per-edge (spec §3.2). The bound is arithmetic, not
/// taste: `2^11` tiles per edge already makes a cell block `2^11 · 2^11 · 512 B` = 2 GiB, and one
/// more doubling would push a cell block past the `uint32` offsets the directory is made of.
pub const MAX_CELL_TILES_LOG2: u8 = 11;

/// Samples along one cell edge as a `log2`, or `None` when the pair is out of the spec's range or
/// would make a cell smaller than one tile (spec §4.5).
#[inline]
pub const fn cell_samples_log2(posting_log2: u8, cell_log2: u8) -> Option<u8> {
    if posting_log2 < MIN_POSTING_LOG2 || posting_log2 > MAX_POSTING_LOG2 {
        return None;
    }
    if cell_log2 < MIN_CELL_LOG2 || cell_log2 > MAX_CELL_LOG2 || cell_log2 < posting_log2 {
        return None;
    }
    let samples_log2 = cell_log2 - posting_log2;
    if samples_log2 < TILE_LOG2 as u8 || samples_log2 - TILE_LOG2 as u8 > MAX_CELL_TILES_LOG2 {
        return None;
    }
    Some(samples_log2)
}

/// Tiles along one cell edge as a `log2` (spec §3.2), for a pair [`cell_samples_log2`] accepts.
#[inline]
pub const fn cell_tiles_log2(posting_log2: u8, cell_log2: u8) -> Option<u8> {
    match cell_samples_log2(posting_log2, cell_log2) {
        Some(samples_log2) => Some(samples_log2 - TILE_LOG2 as u8),
        None => None,
    }
}

/// Byte length of one cell block: `tiles_per_edge² · 512` (spec §3.2). Fits `u32` by construction —
/// see [`MAX_CELL_TILES_LOG2`].
#[inline]
pub const fn cell_block_len(posting_log2: u8, cell_log2: u8) -> Option<u32> {
    match cell_tiles_log2(posting_log2, cell_log2) {
        Some(tiles_log2) => Some((1u32 << (2 * tiles_log2 as u32)) * TILE_BYTES as u32),
        None => None,
    }
}

/// Byte offset of sample `(row, col)` inside a tile (spec §2): row-major, **rows advance latitude**.
#[inline]
pub const fn sample_offset_in_tile(row: u32, col: u32) -> usize {
    (row as usize * TILE_SAMPLES + col as usize) * SAMPLE_LEN
}

/// Byte offset of tile `(ti, tj)` inside a cell block (spec §3.2): row-major over the cell's tile
/// grid, `ti` advancing latitude, with `tiles_log2` from [`cell_tiles_log2`].
#[inline]
pub const fn tile_offset_in_cell(ti: u32, tj: u32, tiles_log2: u8) -> u32 {
    ((ti << tiles_log2) + tj) * TILE_BYTES as u32
}

/// Validate the five-byte `magic + version` prefix (spec §4.2). Layout policy beyond this — the
/// directory bounds, the cell-rectangle sanity, the posting/cell pairing — is the reader's.
pub fn validate_header_prefix(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_prefix(bytes, &MAGIC, VERSION, VERSION).map(|_| ())
}

const _: () = assert!(TILE_BYTES == 512);
const _: () = assert!(HDR_RESERVED + 8 == HEADER_LEN);
const _: () = assert!(GRID_ORIGIN as i64 + WORLD_SIDE as i64 == -(GRID_ORIGIN as i64));
// The v1 pairing of §1.5 must survive its own bounds: posting 2^9, cell 2^19 → 64 tiles per edge.
const _: () = assert!(matches!(cell_tiles_log2(9, 19), Some(6)));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_field_offsets_tile_the_header_exactly() {
        // Each field starts where the previous one ends — the table in §4.2 read as arithmetic.
        assert_eq!(HDR_MAGIC + 4, HDR_VERSION);
        assert_eq!(HDR_VERSION + 1, HDR_POSTING_LOG2);
        assert_eq!(HDR_POSTING_LOG2 + 1, HDR_CELL_LOG2);
        assert_eq!(HDR_CELL_LOG2 + 1, HDR_FLAGS);
        assert_eq!(HDR_FLAGS + 1, HDR_CELL_MIN_I);
        assert_eq!(HDR_CELL_MIN_I + 4, HDR_CELL_MIN_J);
        assert_eq!(HDR_CELL_MIN_J + 4, HDR_CELL_ROWS);
        assert_eq!(HDR_CELL_ROWS + 2, HDR_CELL_COLS);
        assert_eq!(HDR_CELL_COLS + 2, HDR_DIRECTORY_OFFSET);
        assert_eq!(HDR_DIRECTORY_OFFSET + 4, HDR_RESERVED);
        assert_eq!(HDR_RESERVED + 8, HEADER_LEN);
    }

    #[test]
    fn tile_arithmetic_pins_the_512_byte_quantum() {
        assert_eq!(TILE_SAMPLES, 16);
        assert_eq!(TILE_BYTES, 512);
        assert_eq!(sample_offset_in_tile(0, 0), 0);
        assert_eq!(sample_offset_in_tile(0, 1), 2);
        assert_eq!(sample_offset_in_tile(1, 0), 32, "a row is 16 samples wide, and rows advance lat");
        assert_eq!(sample_offset_in_tile(15, 15), TILE_BYTES - SAMPLE_LEN);
        // v1 shape: a 2^19 cell at 2^9 posting is 64 × 64 tiles = 2 MiB.
        assert_eq!(cell_tiles_log2(9, 19), Some(6));
        assert_eq!(cell_block_len(9, 19), Some(64 * 64 * 512));
        assert_eq!(tile_offset_in_cell(0, 1, 6), 512);
        assert_eq!(tile_offset_in_cell(1, 0, 6), 64 * 512);
    }

    #[test]
    fn posting_and_cell_pairs_are_rejected_outside_the_spec_range() {
        assert_eq!(cell_samples_log2(9, 19), Some(10)); // v1: 1024 samples per cell edge
        assert_eq!(cell_samples_log2(9, 13), Some(4), "the smallest legal cell is exactly one tile");
        assert_eq!(cell_samples_log2(9, 12), None, "…one posting smaller is not");
        assert_eq!(cell_samples_log2(MIN_POSTING_LOG2 - 1, 19), None);
        assert_eq!(cell_samples_log2(MAX_POSTING_LOG2 + 1, 28), None);
        assert_eq!(cell_samples_log2(9, MIN_CELL_LOG2 - 1), None);
        assert_eq!(cell_samples_log2(9, MAX_CELL_LOG2 + 1), None);
        // A cell block must stay inside the uint32 offsets the directory is made of.
        assert_eq!(cell_tiles_log2(4, 19), Some(MAX_CELL_TILES_LOG2));
        assert_eq!(cell_tiles_log2(4, 20), None);
        assert!(cell_block_len(4, 19).is_some());
    }

    #[test]
    fn sentinels_and_grid_constants_match_the_spec() {
        assert_eq!(MAGIC, *b"OBCT");
        assert_eq!(VERSION, 1);
        assert_eq!(NODATA, -32768);
        assert_eq!(DIR_ABSENT, 0);
        assert_eq!(GRID_ORIGIN, -268_435_456);
        assert_eq!(WORLD_SIDE, 536_870_912);
        assert!(validate_header_prefix(b"OBCT\x01").is_ok());
        assert!(validate_header_prefix(b"OBCM\x01").is_err());
        assert!(validate_header_prefix(b"OBCT\x02").is_err());
        assert!(validate_header_prefix(b"OBC").is_err());
    }
}
