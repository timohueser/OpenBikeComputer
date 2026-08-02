//! Format-contract + sampling tests for OBCT.
//!
//! Each test builds a synthetic container **from the spec's field tables** (`build`, below) rather
//! than checking in a binary, so the encoder and the reader are pinned to one layout: if either
//! drifts, these break. Same discipline as `obc-reader`'s `tests/format.rs` with `obcm-testkit`.
//!
//! The fixtures deliberately use a small cell (`2^14` µdeg, 32 samples per edge, 2 × 2 tiles)
//! rather than the v1 `2^19`: a v1 cell is 2 MiB of raster, and the posting/cell pair being
//! *header data* is exactly what makes a small one legal. The tile stays 16 × 16 — that is format
//! shape, not data.

use std::cell::Cell;

use obc_elevation::grid::{cell_base_sample, lattice_coord};
use obc_elevation::{ElevationSource, TerrainElevation, TerrainReader, TileCache};
use obc_formats::io::{ByteSource, Error, SliceSource};
use obc_formats::obct::{
    cell_tiles_log2, DIR_ENTRY_LEN, GRID_ORIGIN, HDR_CELL_COLS, HDR_CELL_LOG2, HDR_CELL_MIN_I, HDR_CELL_MIN_J,
    HDR_CELL_ROWS, HDR_DIRECTORY_OFFSET, HDR_FLAGS, HDR_POSTING_LOG2, HEADER_LEN, NODATA, TILE_BYTES, TILE_SAMPLES,
};

const POSTING_LOG2: u8 = 9;
const CELL_LOG2: u8 = 14;
/// Samples along one cell edge at this pairing: 32.
const CELL_SAMPLES: u32 = 1 << (CELL_LOG2 - POSTING_LOG2);
/// The cell rectangle used by most tests: 47°N / 8°E, 2 × 2 cells with the far corner missing.
const MIN_I: u32 = 19_251;
const MIN_J: u32 = 16_871;
const ROWS: u16 = 2;
const COLS: u16 = 2;

/// The synthetic terrain: a plane in lattice space, `100 + 3·di + 5·dj` metres, anchored on the
/// rectangle's own base sample. A plane is the one surface whose bilinear interpolation has a
/// closed form independent of the interpolator, which is what makes it an oracle rather than a
/// second copy of the code under test.
fn plane(i: u32, j: u32) -> i16 {
    let (bi, bj) = base_sample();
    (100 + 3 * (i as i64 - bi as i64) + 5 * (j as i64 - bj as i64)) as i16
}

/// Lattice index of the rectangle's minimum corner sample.
fn base_sample() -> (u32, u32) {
    (cell_base_sample(MIN_I, POSTING_LOG2, CELL_LOG2), cell_base_sample(MIN_J, POSTING_LOG2, CELL_LOG2))
}

/// Build an OBCT container straight from `OBCT_Spec.md` §4: 32-byte header, row-major `uint32`
/// directory over the cell rectangle (`0` = absent), then the present cells' blocks in directory
/// order. `height(i, j)` is evaluated on **absolute lattice indices**; `present(ci, cj)` decides
/// which cells exist.
fn build(rows: u16, cols: u16, height: impl Fn(u32, u32) -> i16, present: impl Fn(u32, u32) -> bool) -> Vec<u8> {
    let tiles_log2 = cell_tiles_log2(POSTING_LOG2, CELL_LOG2).expect("a legal pairing");
    let tiles = 1u32 << tiles_log2;
    let dir_len = rows as usize * cols as usize * DIR_ENTRY_LEN;

    let mut header = [0u8; HEADER_LEN];
    header[..4].copy_from_slice(b"OBCT");
    header[4] = 1;
    header[HDR_POSTING_LOG2] = POSTING_LOG2;
    header[HDR_CELL_LOG2] = CELL_LOG2;
    header[HDR_FLAGS] = 0;
    header[HDR_CELL_MIN_I..HDR_CELL_MIN_I + 4].copy_from_slice(&MIN_I.to_le_bytes());
    header[HDR_CELL_MIN_J..HDR_CELL_MIN_J + 4].copy_from_slice(&MIN_J.to_le_bytes());
    header[HDR_CELL_ROWS..HDR_CELL_ROWS + 2].copy_from_slice(&rows.to_le_bytes());
    header[HDR_CELL_COLS..HDR_CELL_COLS + 2].copy_from_slice(&cols.to_le_bytes());
    header[HDR_DIRECTORY_OFFSET..HDR_DIRECTORY_OFFSET + 4].copy_from_slice(&(HEADER_LEN as u32).to_le_bytes());

    let mut directory = vec![0u8; dir_len];
    let mut blocks: Vec<u8> = Vec::new();
    for di in 0..rows as u32 {
        for dj in 0..cols as u32 {
            let (ci, cj) = (MIN_I + di, MIN_J + dj);
            if !present(ci, cj) {
                continue;
            }
            let offset = (HEADER_LEN + dir_len + blocks.len()) as u32;
            let slot = (di as usize * cols as usize + dj as usize) * DIR_ENTRY_LEN;
            directory[slot..slot + 4].copy_from_slice(&offset.to_le_bytes());
            let (bi, bj) =
                (cell_base_sample(ci, POSTING_LOG2, CELL_LOG2), cell_base_sample(cj, POSTING_LOG2, CELL_LOG2));
            for ti in 0..tiles {
                for tj in 0..tiles {
                    for r in 0..TILE_SAMPLES as u32 {
                        for c in 0..TILE_SAMPLES as u32 {
                            let i = bi + ti * TILE_SAMPLES as u32 + r;
                            let j = bj + tj * TILE_SAMPLES as u32 + c;
                            blocks.extend_from_slice(&height(i, j).to_le_bytes());
                        }
                    }
                }
            }
        }
    }

    let mut file = Vec::with_capacity(HEADER_LEN + dir_len + blocks.len());
    file.extend_from_slice(&header);
    file.extend_from_slice(&directory);
    file.extend_from_slice(&blocks);
    file
}

/// The whole 2 × 2 rectangle minus its far corner — a plane with a hole, the shape most tests want.
fn shard() -> Vec<u8> {
    build(ROWS, COLS, plane, |ci, cj| (ci, cj) != (MIN_I + 1, MIN_J + 1))
}

/// The µdeg coordinate of lattice sample `i` on either axis.
fn coord(i: u32) -> i32 {
    lattice_coord(i, POSTING_LOG2)
}

/// The exact height of the plane at a µdeg coordinate, computed **without** interpolating: on a
/// plane, `h = 100 + 3·(lat − base_lat)/P + 5·(lon − base_lon)/P`. Rounded half away from zero,
/// per spec §5.2. This is the oracle the sampler is checked against.
fn plane_oracle(lat: i32, lon: i32) -> i16 {
    let p = 1i64 << POSTING_LOG2;
    let (bi, bj) = base_sample();
    let num = 100 * p + 3 * (lat as i64 - coord(bi) as i64) + 5 * (lon as i64 - coord(bj) as i64);
    let rounded = if num >= 0 { (num + p / 2) / p } else { -((-num + p / 2) / p) };
    rounded as i16
}

/// A [`ByteSource`] that counts reads, for the cache-behaviour tests.
struct Counting<'a> {
    bytes: &'a [u8],
    reads: Cell<u32>,
}

impl ByteSource for Counting<'_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        self.reads.set(self.reads.get() + 1);
        SliceSource(self.bytes).read_at(offset, buf)
    }
    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

/// A [`ByteSource`] that can be **armed** to fail every read of the directory range, so a test can
/// parse a good file and only then make the medium go bad under it.
struct FlakyDirectory<'a> {
    bytes: &'a [u8],
    armed: Cell<bool>,
    directory: core::ops::Range<u32>,
}

impl ByteSource for FlakyDirectory<'_> {
    fn read_at(&self, offset: u32, buf: &mut [u8]) -> Result<(), Error> {
        if self.armed.get() && self.directory.contains(&offset) {
            return Err(Error::Io);
        }
        SliceSource(self.bytes).read_at(offset, buf)
    }
    fn len(&self) -> u32 {
        self.bytes.len() as u32
    }
}

/// Sample through a fresh reader + cache — the shape every test below wants.
fn sample_all(bytes: &[u8], points: &[(i32, i32)]) -> Vec<Option<i16>> {
    let src = SliceSource(bytes);
    let reader = TerrainReader::parse(&src).expect("the builder writes a valid container");
    let mut cache = TileCache::<4>::new();
    points.iter().map(|&(lat, lon)| reader.sample(&mut cache, lat, lon)).collect()
}

/// The byte pin: every fixed field lands where §4.2 says, the directory is where §4.3 says, and the
/// sizes close. Hand-decoded here rather than via the reader, so a reader bug cannot hide a layout
/// bug.
#[test]
fn the_container_layout_pins_the_spec_field_tables() {
    let bytes = shard();
    let dir_len = ROWS as usize * COLS as usize * DIR_ENTRY_LEN;
    let cell_bytes = 4 * TILE_BYTES; // 2 × 2 tiles at this pairing

    assert_eq!(&bytes[..4], b"OBCT");
    assert_eq!(bytes[4], 1, "version");
    assert_eq!(bytes[HDR_POSTING_LOG2], 9);
    assert_eq!(bytes[HDR_CELL_LOG2], 14);
    assert_eq!(bytes[HDR_FLAGS], 0, "v1 defines no flag bits");
    assert_eq!(u32::from_le_bytes(bytes[HDR_CELL_MIN_I..HDR_CELL_MIN_I + 4].try_into().unwrap()), MIN_I);
    assert_eq!(u32::from_le_bytes(bytes[HDR_CELL_MIN_J..HDR_CELL_MIN_J + 4].try_into().unwrap()), MIN_J);
    assert_eq!(u16::from_le_bytes(bytes[HDR_CELL_ROWS..HDR_CELL_ROWS + 2].try_into().unwrap()), ROWS);
    assert_eq!(u16::from_le_bytes(bytes[HDR_CELL_COLS..HDR_CELL_COLS + 2].try_into().unwrap()), COLS);
    let dir_at = u32::from_le_bytes(bytes[HDR_DIRECTORY_OFFSET..HDR_DIRECTORY_OFFSET + 4].try_into().unwrap());
    assert_eq!(dir_at as usize, HEADER_LEN, "v1 writes the directory straight after the header");
    assert!(bytes[24..HEADER_LEN].iter().all(|&b| b == 0), "the reserved tail is zero");

    // Three present cells at consecutive block offsets, and the hole reads as the absent sentinel.
    let entry = |slot: usize| {
        let at = HEADER_LEN + slot * DIR_ENTRY_LEN;
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    };
    assert_eq!(entry(0) as usize, HEADER_LEN + dir_len);
    assert_eq!(entry(1) as usize, HEADER_LEN + dir_len + cell_bytes);
    assert_eq!(entry(2) as usize, HEADER_LEN + dir_len + 2 * cell_bytes);
    assert_eq!(entry(3), 0, "the absent cell's slot is the zero sentinel");
    assert_eq!(bytes.len(), HEADER_LEN + dir_len + 3 * cell_bytes);

    // Tile order inside a cell: row-major, rows advancing latitude. The sample at cell-local
    // (row 16, col 0) is the first sample of tile (1, 0), i.e. two tiles in.
    let cell0 = entry(0) as usize;
    let (bi, bj) = base_sample();
    let read = |at: usize| i16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
    assert_eq!(read(cell0), plane(bi, bj));
    assert_eq!(read(cell0 + 2), plane(bi, bj + 1), "the next int16 is one posting east");
    assert_eq!(read(cell0 + 32), plane(bi + 1, bj), "a tile row is 16 samples");
    assert_eq!(read(cell0 + TILE_BYTES), plane(bi, bj + 16), "tile (0,1) is the next 512 B block");
    assert_eq!(read(cell0 + 2 * TILE_BYTES), plane(bi + 16, bj), "tile (1,0) follows it");
}

/// A query exactly on a lattice point returns that sample untouched — including across all four
/// tiles of a cell, which is the tile-addressing pin.
#[test]
fn a_lattice_point_returns_its_own_sample_in_every_tile() {
    let bytes = shard();
    let (bi, bj) = base_sample();
    let points: Vec<_> = [0u32, 1, 15, 16, 17, 31]
        .into_iter()
        .flat_map(|di| [0u32, 1, 15, 16, 31].map(move |dj| (coord(bi + di), coord(bj + dj))))
        .collect();
    let got = sample_all(&bytes, &points);
    for (k, (lat, lon)) in points.iter().enumerate() {
        let (i, j) = (
            bi + ((*lat as i64 - coord(bi) as i64) >> POSTING_LOG2) as u32,
            bj + ((*lon as i64 - coord(bj) as i64) >> POSTING_LOG2) as u32,
        );
        assert_eq!(got[k], Some(plane(i, j)), "lattice sample ({i}, {j})");
    }
}

/// Bilinear on a plane is **exact**: the sampler must agree with the closed form at every
/// sub-posting offset, including the rounding ties.
#[test]
fn bilinear_on_a_plane_matches_the_closed_form_everywhere() {
    let bytes = shard();
    let (bi, bj) = base_sample();
    let step = 1 << POSTING_LOG2;
    let mut points = Vec::new();
    // A dense walk across one tile plus the offsets that land on a half-metre tie.
    for di in 0..17i64 {
        for frac in [0i64, 1, 37, step / 4, step / 3, step / 2, step - 1] {
            points.push((coord(bi) as i64 + di * step + frac, coord(bj) as i64 + frac * 2));
        }
    }
    let points: Vec<(i32, i32)> = points.into_iter().map(|(a, b)| (a as i32, b as i32)).collect();
    for (k, got) in sample_all(&bytes, &points).into_iter().enumerate() {
        let (lat, lon) = points[k];
        assert_eq!(got, Some(plane_oracle(lat, lon)), "at ({lat}, {lon})");
    }
}

/// The seam rule, both halves. A point inside a cell samples identically whether that cell arrives
/// as a 1 × 1 **cell file** or as one cell of a shard (the container is the same format), and a
/// point in the last posting *before* a cell's max edge interpolates across the seam into the
/// neighbour cell — so the surface stays the plane, with no discontinuity at the boundary.
#[test]
fn the_same_point_samples_identically_across_a_cell_seam() {
    let full = shard();
    // The lower-right cell on its own: a 1 × 1 rectangle is just a shard of one cell.
    let alone = build(1, 1, plane, |ci, cj| (ci, cj) == (MIN_I, MIN_J));

    let (bi, bj) = base_sample();
    // Interior of cell (0,0) — covered identically by both files.
    let interior: Vec<(i32, i32)> = (0..8).map(|k| (coord(bi + 3) + 100 * k, coord(bj + 5) + 61 * k)).collect();
    assert_eq!(sample_all(&full, &interior), sample_all(&alone, &interior), "a cell file is a 1×1 shard");

    // Straddling the seam between cell (0,0) and cell (1,0): the last posting of the first cell
    // must fetch its upper corners out of the second one.
    let seam_i = bi + CELL_SAMPLES - 1;
    let straddle: Vec<(i32, i32)> =
        [1i64, 128, 256, 511].into_iter().map(|frac| ((coord(seam_i) as i64 + frac) as i32, coord(bj + 4))).collect();
    for (k, got) in sample_all(&full, &straddle).into_iter().enumerate() {
        let (lat, lon) = straddle[k];
        assert_eq!(got, Some(plane_oracle(lat, lon)), "the cross-cell fetch keeps the plane a plane");
    }
    // The seam sample itself belongs to the *upper* cell under the half-open rule, and both
    // approaches to it agree because interpolation degenerates at a lattice point.
    let on_seam = (coord(seam_i + 1), coord(bj + 4));
    assert_eq!(sample_all(&full, &[on_seam])[0], Some(plane(seam_i + 1, bj + 4)));
}

/// Coverage edges clamp to the nearest sample of the containing cell rather than extrapolating —
/// and a query whose *own* cell is missing is not covered at all.
#[test]
fn coverage_edges_clamp_and_holes_are_uncovered() {
    let bytes = shard();
    let (bi, bj) = base_sample();

    // Half a posting past the last sample of the rectangle's outer edge (cell (0,1)'s max lon):
    // no neighbour exists, so the longitude corner clamps and the surface flattens eastwards.
    let last_j = bj + 2 * CELL_SAMPLES - 1;
    let inside = (coord(bi + 2), coord(last_j));
    let past = (coord(bi + 2), coord(last_j) + 256);
    let got = sample_all(&bytes, &[inside, past]);
    assert_eq!(got[0], Some(plane(bi + 2, last_j)));
    assert_eq!(got[1], got[0], "clamped to the nearest covered sample, not extrapolated");

    // The absent cell is a hole: every query inside it is uncovered, including one that sits right
    // beside three present cells.
    let hole = (coord(bi + CELL_SAMPLES + 1), coord(bj + CELL_SAMPLES + 1));
    let hole_corner = (coord(bi + CELL_SAMPLES), coord(bj + CELL_SAMPLES));
    assert_eq!(sample_all(&bytes, &[hole, hole_corner]), vec![None, None]);

    // …and a query outside the rectangle entirely, or outside the world box.
    let outside = (coord(bi) - 1, coord(bj));
    assert_eq!(sample_all(&bytes, &[outside])[0], None);
    assert_eq!(sample_all(&bytes, &[(GRID_ORIGIN - 1, 0)])[0], None);
}

/// One `NODATA` corner voids the whole query — never a partial interpolation over three corners.
#[test]
fn a_nodata_corner_voids_the_sample_and_nothing_else() {
    let (bi, bj) = base_sample();
    let (hole_i, hole_j) = (bi + 20, bj + 7);
    let bytes = build(ROWS, COLS, |i, j| if (i, j) == (hole_i, hole_j) { NODATA } else { plane(i, j) }, |_, _| true);

    // Every query whose 2 × 2 corner set touches the void is None: the four cells of postings
    // around the sample.
    let mut voided = Vec::new();
    for di in [-1i64, 0] {
        for dj in [-1i64, 0] {
            voided.push(((coord(hole_i) as i64 + di * 256) as i32, (coord(hole_j) as i64 + dj * 256) as i32));
        }
    }
    assert!(sample_all(&bytes, &voided).iter().all(Option::is_none), "no partial interpolation");

    // Two postings away, the surface is untouched — the void does not spread.
    let clear = (coord(hole_i + 2), coord(hole_j + 2));
    assert_eq!(sample_all(&bytes, &[clear])[0], Some(plane(hole_i + 2, hole_j + 2)));
}

/// A **failed directory read voids the sample** — it must never be mistaken for "that cell is
/// absent" and answered with the coverage clamp, which would hand back an entirely plausible
/// height on a card that just glitched.
///
/// The setup isolates the corner path: sample once inside the home cell so the cell memo holds it,
/// *then* arm the failure, then sample a point whose upper corners cross the cell seam. The home
/// cell now comes from the memo, so the only read that fails is the neighbour's directory entry —
/// the exact spot where absence and I/O failure look alike.
#[test]
fn a_failed_directory_read_voids_the_sample_instead_of_clamping() {
    let bytes = shard();
    let dir_len = (ROWS as u32) * (COLS as u32) * DIR_ENTRY_LEN as u32;
    let src = FlakyDirectory {
        bytes: &bytes,
        armed: Cell::new(false),
        directory: HEADER_LEN as u32..HEADER_LEN as u32 + dir_len,
    };
    let reader = TerrainReader::parse(&src).expect("parse happens before the medium goes bad");
    let mut cache = TileCache::<4>::new();
    let (bi, bj) = base_sample();

    // Half a posting below the cell seam: the upper corners live in the cell above.
    let straddle = (coord(bi + CELL_SAMPLES - 1) + 256, coord(bj + 4));
    let healthy = reader.sample(&mut cache, straddle.0, straddle.1);
    assert_eq!(healthy, Some(plane_oracle(straddle.0, straddle.1)), "the healthy answer crosses the seam");

    // Warm the memo with the home cell, then break the card.
    let mut cache = TileCache::<4>::new();
    assert!(reader.sample(&mut cache, coord(bi + 2), coord(bj + 4)).is_some());
    src.armed.set(true);
    assert_eq!(reader.sample(&mut cache, straddle.0, straddle.1), None, "an I/O error is not an absent cell");

    // And the clamp it must not have taken: had the error been read as absence, the seam query
    // would have answered the home cell's edge sample — a wrong number that looks right.
    let clamped_if_wrong = plane(bi + CELL_SAMPLES - 1, bj + 4);
    assert_ne!(healthy, Some(clamped_if_wrong), "the two answers really are distinguishable");

    // Disarmed, the same reader and the same warm cache answer normally again.
    src.armed.set(false);
    assert_eq!(reader.sample(&mut cache, straddle.0, straddle.1), healthy);
}

/// Every structural fault is refused at parse, before a query can read past the file.
#[test]
fn malformed_containers_are_rejected_at_parse() {
    let good = shard();
    let reject = |bytes: &[u8], why: &str| {
        let src = SliceSource(bytes);
        assert!(TerrainReader::parse(&src).is_err(), "{why}");
    };

    reject(&good[..HEADER_LEN - 1], "a file shorter than the header");
    reject(&good[..HEADER_LEN + 4], "a truncated directory");
    reject(&good[..good.len() - 1], "a truncated final cell block");

    let mutate = |at: usize, bytes: &[u8]| {
        let mut v = good.clone();
        v[at..at + bytes.len()].copy_from_slice(bytes);
        v
    };
    reject(&mutate(0, b"OBCM"), "the wrong magic");
    reject(&mutate(4, &[2]), "an unsupported version");
    reject(&mutate(HDR_FLAGS, &[1]), "an unknown flag bit — an encoding this build does not have");
    reject(&mutate(HDR_POSTING_LOG2, &[CELL_LOG2]), "a posting as coarse as the cell (no whole tile fits)");
    reject(&mutate(HDR_POSTING_LOG2, &[1]), "a posting outside the permitted range");
    reject(&mutate(HDR_CELL_LOG2, &[29]), "a cell size off the OBCA grid");
    reject(&mutate(HDR_CELL_ROWS, &0u16.to_le_bytes()), "an empty rectangle");
    reject(&mutate(HDR_CELL_MIN_I, &u32::MAX.to_le_bytes()), "a rectangle off the world grid");
    reject(&mutate(HDR_DIRECTORY_OFFSET, &4u32.to_le_bytes()), "a directory overlapping the header");
    reject(&mutate(24, &[1]), "a non-zero reserved byte");
    reject(&mutate(HEADER_LEN, &(good.len() as u32).to_le_bytes()), "a cell block past the file end");
    reject(&mutate(HEADER_LEN, &1u32.to_le_bytes()), "a cell block inside the directory");
    reject(&mutate(HEADER_LEN, &((HEADER_LEN + 16 + 1) as u32).to_le_bytes()), "an odd cell-block offset");

    // The good file still parses — the mutations above are the only thing being rejected.
    assert!(TerrainReader::parse(&SliceSource(&good)).is_ok());
}

/// The header a consumer reads back, including the coverage box EL4/EL7 will project.
#[test]
fn the_parsed_header_describes_the_coverage_rectangle() {
    let bytes = shard();
    let src = SliceSource(&bytes);
    let reader = TerrainReader::parse(&src).unwrap();
    let h = reader.header();
    assert_eq!((h.posting_log2, h.cell_log2, h.flags), (POSTING_LOG2, CELL_LOG2, 0));
    assert_eq!((h.cell_min_i, h.cell_min_j, h.cell_rows, h.cell_cols), (MIN_I, MIN_J, ROWS, COLS));
    let side = 1i64 << CELL_LOG2;
    let (min_lat, min_lon, max_lat, max_lon) = h.bbox_udeg();
    assert_eq!(min_lat, GRID_ORIGIN as i64 + MIN_I as i64 * side);
    assert_eq!(min_lon, GRID_ORIGIN as i64 + MIN_J as i64 * side);
    assert_eq!((max_lat - min_lat, max_lon - min_lon), (ROWS as i64 * side, COLS as i64 * side));
    // The rectangle really is around 47°N / 8°E, i.e. the fixture is not sitting in the ocean of
    // some other hemisphere because an index was transcribed wrong.
    assert!((46_900_000..47_100_000).contains(&min_lat));
    assert!((7_900_000..8_100_000).contains(&min_lon));
}

/// The cache is what makes a walk affordable: a repeated query costs no reads at all, and a walk
/// inside one tile costs one tile read plus one directory read, not one per corner.
#[test]
fn the_tile_cache_absorbs_the_corner_and_repeat_reads() {
    let bytes = shard();
    let src = Counting { bytes: &bytes, reads: Cell::new(0) };
    let reader = TerrainReader::parse(&src).unwrap();
    let mut cache = TileCache::<4>::new();
    let (bi, bj) = base_sample();

    src.reads.set(0);
    let first = reader.sample(&mut cache, coord(bi + 3) + 100, coord(bj + 3) + 100);
    assert!(first.is_some());
    assert_eq!(src.reads.get(), 2, "one directory entry + one tile, for all four corners");

    src.reads.set(0);
    for _ in 0..16 {
        assert_eq!(reader.sample(&mut cache, coord(bi + 3) + 100, coord(bj + 3) + 100), first);
    }
    assert_eq!(src.reads.get(), 0, "a repeat query touches the medium not at all");

    src.reads.set(0);
    for k in 0..8 {
        reader.sample(&mut cache, coord(bi + 3) + 60 * k, coord(bj + 3) + 60 * k);
    }
    assert_eq!(src.reads.get(), 0, "…and neither does a walk inside the resident tile");
    let (hits, misses) = cache.stats();
    assert!(hits > misses, "{hits} hits vs {misses} misses");
}

/// The seam consumers actually hold: [`TerrainElevation`] answers the same numbers the reader does,
/// and a [`NullElevation`] in its place answers `None` — the substitution the epic's "removing
/// terrain changes nothing else" claim rests on.
#[test]
fn the_elevation_source_seam_agrees_with_the_reader() {
    let bytes = shard();
    let src = SliceSource(&bytes);
    let (bi, bj) = base_sample();
    let points: Vec<(i32, i32)> = (0..12).map(|k| (coord(bi + 2) + 37 * k, coord(bj + 2) + 91 * k)).collect();

    let mut terrain = TerrainElevation::<4>::parse(&src).unwrap();
    let through_seam: Vec<_> = points.iter().map(|&(lat, lon)| terrain.sample(lat, lon)).collect();
    assert_eq!(through_seam, sample_all(&bytes, &points));
    assert!(through_seam.iter().all(Option::is_some));

    let mut null = obc_elevation::NullElevation;
    assert!(points.iter().all(|&(lat, lon)| null.sample(lat, lon).is_none()));
}
