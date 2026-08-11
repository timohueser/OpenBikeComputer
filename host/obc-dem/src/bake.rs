//! The bake: source mosaic → OBCT cell blocks → containers.
//!
//! One function is the whole contract. [`bake_cell`] takes a mosaic and a cell index and returns
//! that cell's block — a pure function of those two things, depending on **no** bbox, no output
//! mode and no neighbouring cell. Everything else here is bookkeeping around it: which cells a box
//! selects ([`cell_rect`]), and whether they land in one shard or one file each.
//!
//! That purity is what the epic actually needs. A cell published on its own must be byte-identical
//! to the same cell inside a wide shard, or the catalog (EL3) and the assembler (EL4) would produce
//! two different rasters for one square of the world and the "one sampling truth" claim would be
//! false at the first seam. It is also what makes the digest pin in the tests meaningful rather than
//! merely stable.

use std::io::{Seek, Write};

use obc_elevation::grid::{cell_base_sample, cell_of, lattice_coord, locate};
use obc_formats::obct::{
    cell_block_len, cell_samples_log2, cell_tiles_log2, sample_offset_in_tile, tile_offset_in_cell, NODATA, TILE_LOG2,
    TILE_SAMPLES,
};

use crate::container::{CellRect, ShardWriter};
use crate::geotiff::DemMosaic;
use crate::BboxUdeg;

/// The v1 baked posting: `2^9` µdeg ≈ 57 × 39 m at 47 °N (`OBCT_Spec.md` §1.3).
pub const V1_POSTING_LOG2: u8 = 9;
/// The v1 published cell side: `2^19` µdeg — 1024² samples, a 2 MiB block (`OBCT_Spec.md` §1.3).
pub const V1_CELL_LOG2: u8 = 19;

/// What a bake was asked for. Posting and cell side are **parameters**, not constants, because they
/// are header data in the format too (the OBCA §1.5 idiom): retuning either is a re-bake, and a
/// sidecar for a small map is legitimately baked at a smaller cell than a published catalog object.
#[derive(Debug, Clone, Copy)]
pub struct BakeParams {
    pub posting_log2: u8,
    pub cell_log2: u8,
    pub bbox: BboxUdeg,
}

/// What one bake produced, for the operator's summary and for a caller that wants to check coverage
/// without re-reading the file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BakeReport {
    pub cells_total: u64,
    pub cells_written: u64,
    pub samples_total: u64,
    pub samples_nodata: u64,
}

/// Quantise a source height in metres to an OBCT sample.
///
/// **Rounding is half away from zero** — `f64::round`'s own rule, and the rule `OBCT_Spec.md` §5.2
/// pins for the read side. Matching them is not cosmetic: the packer integrates ascent from these
/// samples and the device interpolates between them, and a producer that rounded towards `-∞` would
/// put a systematic half-metre bias into every descent and none into any climb.
///
/// A height outside the `int16` range is not clipped, it is voided. `-32768` is the `NODATA`
/// sentinel a producer MUST NOT write as a height, and a source claiming 40 km of elevation is
/// broken rather than steep — silence is the honest answer to both.
#[inline]
pub fn quantise(metres: f64) -> i16 {
    if !metres.is_finite() {
        return NODATA;
    }
    let rounded = metres.round();
    if !(-32767.0..=32767.0).contains(&rounded) {
        return NODATA;
    }
    rounded as i16
}

/// The cell rectangle a bounding box selects: every cell that intersects the box, inclusive of the
/// box's own max edge.
///
/// Cells are half-open (`OBCT_Spec.md` §3.1), so the cell *owning* `max_lat` is the one a query at
/// `max_lat` would be answered from — it has to be in the rectangle or the box's own northern edge
/// would be uncovered. The result is therefore "the cells the box touches", never a rounding of the
/// box down to a cell multiple.
pub fn cell_rect(bbox: BboxUdeg, posting_log2: u8, cell_log2: u8) -> Result<CellRect, String> {
    cell_samples_log2(posting_log2, cell_log2).ok_or_else(|| {
        format!("posting 2^{posting_log2} µdeg with cell 2^{cell_log2} µdeg is not a pairing OBCT permits")
    })?;
    let cell_at = |lat: i32, lon: i32| -> Result<(u32, u32), String> {
        let at = locate(lat, lon, posting_log2)
            .ok_or_else(|| format!("({lat}, {lon}) µdeg is outside the OBCA world box"))?;
        Ok((cell_of(at.i, posting_log2, cell_log2), cell_of(at.j, posting_log2, cell_log2)))
    };
    let (min_i, min_j) = cell_at(bbox.min_lat, bbox.min_lon)?;
    let (max_i, max_j) = cell_at(bbox.max_lat, bbox.max_lon)?;
    let rows =
        u16::try_from(max_i - min_i + 1).map_err(|_| "the box spans more than 65535 cells in latitude".to_string())?;
    let cols =
        u16::try_from(max_j - min_j + 1).map_err(|_| "the box spans more than 65535 cells in longitude".to_string())?;
    Ok(CellRect { min_i, min_j, rows, cols })
}

/// Bake one terrain cell: every lattice sample the cell owns, point-sampled from `mosaic`.
///
/// Returns `None` when **every** sample is `NODATA` — the cell is then published as an absent
/// directory slot rather than 2 MiB of sentinel. That is not a compression trick: an all-void cell
/// and a missing cell answer identically under §5 (`None` at every query, with the §5.3 clamp
/// reaching for the containing cell either way), so writing the bytes would buy nothing.
///
/// The returned block is laid out per §3.2 — tiles row-major with `ti` advancing latitude, samples
/// row-major within a tile with `row` advancing latitude — and the offsets come from `obc-formats`
/// rather than from this file's own arithmetic.
pub fn bake_cell(mosaic: &DemMosaic, ci: u32, cj: u32, posting_log2: u8, cell_log2: u8) -> Option<Vec<u8>> {
    let samples_log2 = cell_samples_log2(posting_log2, cell_log2).expect("caller validated the pairing");
    let tiles_log2 = cell_tiles_log2(posting_log2, cell_log2).expect("caller validated the pairing");
    let block_len = cell_block_len(posting_log2, cell_log2).expect("caller validated the pairing") as usize;
    let span = 1u32 << samples_log2;
    let base_i = cell_base_sample(ci, posting_log2, cell_log2);
    let base_j = cell_base_sample(cj, posting_log2, cell_log2);

    let mut block = vec![0u8; block_len];
    let mut any = false;
    for li in 0..span {
        let lat_deg = f64::from(lattice_coord(base_i + li, posting_log2)) / 1e6;
        let (ti, row) = (li >> TILE_LOG2, li & (TILE_SAMPLES as u32 - 1));
        for lj in 0..span {
            let lon_deg = f64::from(lattice_coord(base_j + lj, posting_log2)) / 1e6;
            let value = match mosaic.height(lat_deg, lon_deg) {
                Some(metres) => quantise(metres),
                None => NODATA,
            };
            any |= value != NODATA;
            let (tj, col) = (lj >> TILE_LOG2, lj & (TILE_SAMPLES as u32 - 1));
            let at = tile_offset_in_cell(ti, tj, tiles_log2) as usize + sample_offset_in_tile(row, col);
            block[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
    }
    any.then_some(block)
}

/// Count the `NODATA` samples in a block — the operator's coverage number, read back from the bytes
/// that were actually written rather than tallied while writing them.
fn nodata_in(block: &[u8]) -> u64 {
    block.chunks_exact(2).filter(|s| i16::from_le_bytes([s[0], s[1]]) == NODATA).count() as u64
}

/// Bake every cell the box selects into **one** container on `out` — the terrain *shard* a rider
/// carries beside a map (`OBCT_Spec.md` §4.1).
///
/// `progress` is called once per cell with `(index, total, ci, cj, written)` so a CLI can say what
/// it is doing without this module owning a progress abstraction.
pub fn bake_shard<W: Write + Seek>(
    mosaic: &DemMosaic,
    params: BakeParams,
    out: W,
    mut progress: impl FnMut(u64, u64, u32, u32, bool),
) -> Result<BakeReport, String> {
    let rect = cell_rect(params.bbox, params.posting_log2, params.cell_log2)?;
    let mut writer = ShardWriter::new(out, params.posting_log2, params.cell_log2, rect)?;
    let per_cell = writer.block_len() as u64 / 2;
    let total = rect.slots();
    let mut report = BakeReport { cells_total: total, samples_total: total * per_cell, ..BakeReport::default() };

    for (index, (ci, cj)) in rect.cells().enumerate() {
        let block = bake_cell(mosaic, ci, cj, params.posting_log2, params.cell_log2);
        match &block {
            Some(bytes) => {
                report.cells_written += 1;
                report.samples_nodata += nodata_in(bytes);
            }
            None => report.samples_nodata += per_cell,
        }
        writer.push(block.as_deref())?;
        progress(index as u64 + 1, total, ci, cj, block.is_some());
    }
    writer.finish()?;
    Ok(report)
}

/// The canonical file name of a published cell: the OBCA cell id with `/` replaced by `_`.
///
/// The id itself (`<log2>/<i>/<j>`, zero-padded per `OBCA_Spec.md` §1.3) is the catalog's name for
/// the square, so deriving the file name from it rather than inventing a second naming scheme keeps
/// EL3's mapping a substitution instead of a lookup table. The padding rule is
/// [`obc_elevation::grid::id_width`] rather than a local `max(4, …)`, because it is what makes an
/// id a *key*: one square, one string, in a store addressed by that string. This crate cannot see
/// `obc-pack`'s `grid::id_width`, and both call the same leaf so they cannot drift.
pub fn cell_file_name(cell_log2: u8, ci: u32, cj: u32) -> String {
    let width = obc_elevation::grid::id_width(cell_log2);
    format!("{cell_log2}_{ci:0width$}_{cj:0width$}.obcd", width = width)
}

/// Write one already-baked cell block as a **1 × 1 container** at `path` — the published
/// terrain-cell shape (`OBCT_Spec.md` §4.1).
///
/// Split out of [`bake_cells`] so a caller that owns its own naming can still write the *one*
/// container this crate writes. The bakery (EL3) is exactly that caller: a catalog lays its objects
/// out as `cells/terrain/<i>/<j>.obcd` rather than in one flat directory, and it must not reach for
/// a second writer to get there — a second writer is the first place the published cell and the
/// assembled shard could drift apart.
pub fn write_cell_file(
    path: &std::path::Path,
    posting_log2: u8,
    cell_log2: u8,
    ci: u32,
    cj: u32,
    block: &[u8],
) -> Result<(), String> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut writer = ShardWriter::new(
        std::io::BufWriter::new(file),
        posting_log2,
        cell_log2,
        CellRect { min_i: ci, min_j: cj, rows: 1, cols: 1 },
    )?;
    writer.push(Some(block))?;
    writer.finish()?;
    Ok(())
}

/// Bake every cell the box selects into **one file each** — the terrain *cells* a bakery publishes
/// and a catalog names, each a container whose rectangle is 1 × 1.
///
/// A cell with no data at all is not written: there is no object to publish, and the catalog's
/// known-empty runs (`OBCC_Spec.md`) are the right place to say so.
pub fn bake_cells(
    mosaic: &DemMosaic,
    params: BakeParams,
    dir: &std::path::Path,
    mut progress: impl FnMut(u64, u64, u32, u32, bool),
) -> Result<BakeReport, String> {
    let rect = cell_rect(params.bbox, params.posting_log2, params.cell_log2)?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let per_cell = cell_block_len(params.posting_log2, params.cell_log2).expect("validated by cell_rect") as u64 / 2;
    let total = rect.slots();
    let mut report = BakeReport { cells_total: total, samples_total: total * per_cell, ..BakeReport::default() };

    for (index, (ci, cj)) in rect.cells().enumerate() {
        let block = bake_cell(mosaic, ci, cj, params.posting_log2, params.cell_log2);
        match block {
            Some(bytes) => {
                report.cells_written += 1;
                report.samples_nodata += nodata_in(&bytes);
                let path = dir.join(cell_file_name(params.cell_log2, ci, cj));
                write_cell_file(&path, params.posting_log2, params.cell_log2, ci, cj, &bytes)?;
                progress(index as u64 + 1, total, ci, cj, true);
            }
            None => {
                report.samples_nodata += per_cell;
                progress(index as u64 + 1, total, ci, cj, false);
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::obct::GRID_ORIGIN;

    /// Half away from zero, symmetric about sea level — the property `floor` would not have, and
    /// the one the read side pins from the other direction.
    #[test]
    fn quantisation_rounds_half_away_from_zero() {
        assert_eq!(quantise(1000.4), 1000);
        assert_eq!(quantise(1000.5), 1001);
        assert_eq!(quantise(0.5), 1);
        assert_eq!(quantise(-0.5), -1);
        assert_eq!(quantise(-1000.5), -1001);
        assert_eq!(quantise(-1000.4), -1000);
        assert_eq!(quantise(0.0), 0);
    }

    /// The sentinel is never written as a height, and an impossible height is silence rather than a
    /// clipped one — a clip would put a believable 32767 into the raster.
    #[test]
    fn an_impossible_height_is_voided_and_never_clipped() {
        assert_eq!(quantise(f64::NAN), NODATA);
        assert_eq!(quantise(f64::INFINITY), NODATA);
        assert_eq!(quantise(32767.0), 32767);
        assert_eq!(quantise(32767.5), NODATA);
        assert_eq!(quantise(-32767.0), -32767);
        assert_eq!(quantise(-32768.0), NODATA, "the NODATA sentinel is not a height a producer may write");
    }

    /// A box selects every cell it touches, including the one owning its max edge.
    #[test]
    fn a_box_selects_the_cells_it_touches_including_its_max_edge() {
        let side = 1i64 << V1_CELL_LOG2;
        // A box wholly inside one cell.
        let inside = BboxUdeg { min_lat: 46_500_000, min_lon: 8_200_000, max_lat: 46_600_000, max_lon: 8_300_000 };
        let rect = cell_rect(inside, V1_POSTING_LOG2, V1_CELL_LOG2).unwrap();
        assert_eq!((rect.rows, rect.cols), (1, 1));

        // A box straddling a cell boundary on both axes: the boundary coordinate belongs to the
        // upper cell, so a box that reaches it must carry both.
        let boundary_lat = (GRID_ORIGIN as i64 + rect.min_i as i64 * side + side) as i32;
        let boundary_lon = (GRID_ORIGIN as i64 + rect.min_j as i64 * side + side) as i32;
        let straddling = BboxUdeg {
            min_lat: boundary_lat - 1,
            min_lon: boundary_lon - 1,
            max_lat: boundary_lat,
            max_lon: boundary_lon,
        };
        let rect = cell_rect(straddling, V1_POSTING_LOG2, V1_CELL_LOG2).unwrap();
        assert_eq!((rect.rows, rect.cols), (2, 2));
    }

    /// The Grimsel bbox `build-map-package.sh` pins, at the v1 pairing — the shape the sidecar assets were
    /// sized around, and the reason they are baked at a smaller cell.
    #[test]
    fn the_grimsel_box_straddles_four_v1_cells() {
        let grimsel = BboxUdeg::parse("46.48261,8.15034,46.72070,8.46007").unwrap();
        let rect = cell_rect(grimsel, V1_POSTING_LOG2, V1_CELL_LOG2).unwrap();
        assert_eq!((rect.rows, rect.cols), (2, 2), "0.24° × 0.31° still lands on four 0.52° cells");
        // …and on a 2^16 cell it is a 4 × 6 rectangle of 32 KiB blocks instead of four 2 MiB ones.
        let rect = cell_rect(grimsel, V1_POSTING_LOG2, 16).unwrap();
        assert_eq!((rect.rows, rect.cols), (4, 6));
    }

    #[test]
    fn a_cell_file_name_is_its_catalog_id() {
        // 2^19 cells: 1024 per axis, so 4 digits (the `max(4, …)` floor).
        assert_eq!(cell_file_name(19, 600, 527), "19_0600_0527.obcd");
        assert_eq!(cell_file_name(19, 0, 1023), "19_0000_1023.obcd");
        // 2^16 cells: 8192 per axis — still 4 digits.
        assert_eq!(cell_file_name(16, 4805, 4220), "16_4805_4220.obcd");
        // 2^10 cells: 524288 per axis — 6 digits, and the padding widens rather than truncating.
        assert_eq!(cell_file_name(10, 7, 8), "10_000007_000008.obcd");
    }

    #[test]
    fn an_impossible_pairing_is_refused_before_any_work() {
        let b = BboxUdeg { min_lat: 0, min_lon: 0, max_lat: 1, max_lon: 1 };
        assert!(cell_rect(b, 9, 12).is_err(), "a cell smaller than one tile");
        assert!(cell_rect(b, 9, 13).is_ok());
    }
}
