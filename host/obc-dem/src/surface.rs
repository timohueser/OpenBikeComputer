//! Geographic height levels and maximum-height indices, independent of an observer.

use std::io::{Seek, Write};

use obc_elevation::TerrainReader;
use obc_formats::io::SliceSource;
use obc_formats::obct::{
    approximation_error, approximation_offset, cell_block_len, sample_offset_in_tile, tile_offset_in_cell,
    SurfaceLayout, GRID_ORIGIN, MAX_LEAF_LOG2, NODATA,
};

use crate::container::{CellRect, ShardWriter};

/// Bake one indexed cell from exact lattice heights. Unknown bounds prohibit culling.
pub fn bake_cell(
    ci: u32,
    cj: u32,
    posting_log2: u8,
    cell_log2: u8,
    height: impl FnMut(i32, i32) -> i16,
) -> Result<Option<Vec<u8>>, String> {
    bake_cell_with_crest(ci, cj, posting_log2, cell_log2, height, None)
}

/// As [`bake_cell`], with §9's lifts folded into the maxima and error codes. The height tiles stay
/// native — §8.1 requires that — so only the bounds a panorama reads describe the lifted surface.
pub fn bake_cell_with_crest(
    ci: u32,
    cj: u32,
    posting_log2: u8,
    cell_log2: u8,
    mut height: impl FnMut(i32, i32) -> i16,
    crest: Option<&crate::crest::CrestBlock>,
) -> Result<Option<Vec<u8>>, String> {
    let layout = SurfaceLayout::new(posting_log2, cell_log2).ok_or("surface layout exceeds OBCT limits")?;
    let origin_y = i64::from(GRID_ORIGIN) + (i64::from(ci) << cell_log2);
    let origin_x = i64::from(GRID_ORIGIN) + (i64::from(cj) << cell_log2);
    let mut out = vec![0u8; layout.cell_bytes() as usize];
    let mut any = false;
    for index in 0..layout.level_count() {
        let level = layout.level(index).expect("level in layout");
        let side = 1usize << level.samples_log2;
        let mut grid = Vec::with_capacity((side + 1) * (side + 1));
        for y in 0..=side {
            for x in 0..=side {
                let lat = origin_y + ((y as i64) << level.posting_log2);
                let lon = origin_x + ((x as i64) << level.posting_log2);
                let value = height(lat as i32, lon as i32);
                // The pyramid and the bounds describe the panorama surface; the tiles below keep
                // the native sample, which is what every other elevation consumer reads.
                let lift = crest.map_or(0, |c| c.lift(index, y as u32, x as u32));
                grid.push(if value == NODATA { value } else { value.saturating_add(lift) });
                if y < side && x < side {
                    any |= index == 0 && value != NODATA;
                    let at = level.offset as usize
                        + tile_offset_in_cell((y / 16) as u32, (x / 16) as u32, level.samples_log2 - 4) as usize
                        + sample_offset_in_tile((y % 16) as u32, (x % 16) as u32);
                    out[at..at + 2].copy_from_slice(&value.to_le_bytes());
                }
            }
        }
        for log in MAX_LEAF_LOG2..=level.samples_log2 {
            let width = 1usize << log;
            for y in (0..side).step_by(width) {
                for x in (0..side).step_by(width) {
                    let mut maximum = i16::MIN;
                    if log == MAX_LEAF_LOG2 {
                        for row in y..=y + width {
                            for col in x..=x + width {
                                let value = grid[row * (side + 1) + col];
                                maximum = maximum.max(if value == NODATA { i16::MAX } else { value });
                            }
                        }
                    } else {
                        for dy in [0, width / 2] {
                            for dx in [0, width / 2] {
                                let at =
                                    level.bound_offset((y + dy) as u32, (x + dx) as u32, log - 1).unwrap() as usize;
                                maximum = maximum.max(i16::from_le_bytes([out[at], out[at + 1]]));
                            }
                        }
                    }
                    let at = level.bound_offset(y as u32, x as u32, log).unwrap() as usize;
                    out[at..at + 2].copy_from_slice(&maximum.to_le_bytes());
                    out[approximation_offset(at as u32) as usize] =
                        if maximum == i16::MAX { 255 } else { approximation(&grid, side + 1, y, x, width) };
                }
            }
        }
    }
    Ok(any.then_some(out))
}

/// Bilinear differences attain their extreme heights at vertices and their extreme
/// derivatives at edge endpoints. Check both before merging source cells into one patch.
fn approximation(grid: &[i16], stride: usize, y: usize, x: usize, width: usize) -> u8 {
    let height = |row, col| f64::from(grid[row * stride + col]);
    let a = height(y, x);
    let east = (height(y, x + width) - a) / width as f64;
    let north = (height(y + width, x) - a) / width as f64;
    let cross =
        (a - height(y, x + width) - height(y + width, x) + height(y + width, x + width)) / (width * width) as f64;
    let mut residual = 0.0f64;
    let mut gradient = 0.0f64;
    for row in 0..=width {
        let dx = east + cross * row as f64;
        for col in 0..=width {
            let value = height(y + row, x + col);
            let dy = north + cross * col as f64;
            residual = residual.max((value - (a + north * row as f64 + dx * col as f64)).abs());
            if row < width {
                gradient = gradient.max((height(y + row + 1, x + col) - value - dy).abs());
            }
            if col < width {
                gradient = gradient.max((height(y + row, x + col + 1) - value - dx).abs());
            }
        }
    }
    let encode = |error: f64, derivative| -> u8 {
        (0..15).find(|&code| error <= f64::from(approximation_error(code, derivative))).unwrap_or(15)
    };
    encode(residual, false) | (encode(gradient, true) << 4)
}

/// Convert an existing terrain container without changing any native sample.
pub fn convert<W: Write + Seek>(bytes: &[u8], out: W) -> Result<(), String> {
    convert_with_reference(bytes, out, None)
}

/// As [`convert`], adding §9 crest planes wherever `reference` — a finer DEM in the same
/// geographic frame — says the lattice loses a crest. Cells the reference does not cover come out
/// byte-identical to [`convert`]'s, which is what lets national coverage stop at a border.
pub fn convert_with_reference<W: Write + Seek>(
    bytes: &[u8],
    out: W,
    reference: Option<&crate::geotiff::DemMosaic>,
) -> Result<(), String> {
    let source = SliceSource(bytes);
    let reader = TerrainReader::parse(&source).map_err(|e| format!("invalid source terrain: {e:?}"))?;
    let h = *reader.header();
    let rect = CellRect { min_i: h.cell_min_i, min_j: h.cell_min_j, rows: h.cell_rows, cols: h.cell_cols };
    let mut writer = ShardWriter::with_crest(out, h.posting_log2, h.cell_log2, rect, true, reference.is_some())?;
    let span_log2 = h.cell_log2 - h.posting_log2;
    let sample = |lat: i32, lon: i32| -> i16 {
        let Some(y) = i64::from(lat).checked_sub(i64::from(GRID_ORIGIN)).and_then(|v| u32::try_from(v).ok()) else {
            return NODATA;
        };
        let Some(x) = i64::from(lon).checked_sub(i64::from(GRID_ORIGIN)).and_then(|v| u32::try_from(v).ok()) else {
            return NODATA;
        };
        let (ci, cj) = (y >> h.cell_log2, x >> h.cell_log2);
        let (Some(di), Some(dj)) = (ci.checked_sub(h.cell_min_i), cj.checked_sub(h.cell_min_j)) else { return NODATA };
        if di >= h.cell_rows.into() || dj >= h.cell_cols.into() {
            return NODATA;
        }
        let dir = h.directory_offset as usize + (di as usize * h.cell_cols as usize + dj as usize) * 4;
        let offset = u32::from_le_bytes(bytes[dir..dir + 4].try_into().unwrap());
        if offset == 0 {
            return NODATA;
        }
        let mask = (1 << span_log2) - 1;
        let (sy, sx) = ((y >> h.posting_log2) & mask, (x >> h.posting_log2) & mask);
        let at = offset as usize
            + tile_offset_in_cell(sy >> 4, sx >> 4, span_log2 - 4) as usize
            + sample_offset_in_tile(sy & 15, sx & 15);
        i16::from_le_bytes([bytes[at], bytes[at + 1]])
    };
    for (ci, cj) in rect.cells() {
        let crest = reference.and_then(|dem| {
            crate::crest::bake_cell(ci, cj, h.posting_log2, h.cell_log2, sample, |lat, lon| dem.height(lat, lon))
        });
        let block = bake_cell_with_crest(ci, cj, h.posting_log2, h.cell_log2, sample, crest.as_ref())?;
        let planes = block.as_ref().and(crest.as_ref()).map(|c| c.bytes());
        writer.push_with_crest(block.as_deref(), planes)?;
    }
    writer.finish()?;
    Ok(())
}

/// Select the container encoding from a block produced by one of the two terrain bakers.
pub fn is_surface_block(posting: u8, cell: u8, bytes: usize) -> bool {
    cell_block_len(posting, cell).is_some_and(|native| bytes != native as usize)
        && SurfaceLayout::new(posting, cell).is_some_and(|layout| bytes == layout.cell_bytes() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_elevation::TileCache;

    #[test]
    fn approximation_bounds_contain_interior_heights_and_gradients() {
        let surface = |kind, y: i32, x: i32| -> i16 {
            if kind == 2 && x == 16 {
                NODATA
            } else if kind == 0 {
                (100 + 3 * y - 2 * x) as i16
            } else {
                (100 + y * y / 3 - x * x / 2 + if (y, x) == (9, 7) { 90 } else { 0 }) as i16
            }
        };
        let interpolate = |corners: [f64; 4], y: f64, x: f64| {
            let [a, b, c, d] = corners;
            let cross = a - b - c + d;
            (a + (b - a) * x + (c - a) * y + cross * x * y, b - a + cross * y, c - a + cross * x)
        };
        let level = SurfaceLayout::new(9, 13).unwrap().level(0).unwrap();
        for kind in 0..3 {
            let bytes =
                bake_cell(1 << 15, 1 << 15, 9, 13, |lat, lon| surface(kind, lat >> 9, lon >> 9)).unwrap().unwrap();
            for log in 2..=4 {
                let width = 1i32 << log;
                for y in (0..16).step_by(width as usize) {
                    for x in (0..16).step_by(width as usize) {
                        let at = level.bound_offset(y as u32, x as u32, log).unwrap();
                        let code = bytes[approximation_offset(at) as usize];
                        if kind == 2 && x + width == 16 {
                            assert_eq!(code, 255, "unknown inclusive edge cannot be approximated");
                            continue;
                        }
                        if kind == 0 {
                            assert_eq!(code, 0, "an exact plane needs no error allowance");
                        }
                        let height_error = f64::from(approximation_error(code, false));
                        let gradient_error = f64::from(approximation_error(code >> 4, true));
                        let corners = |y, x, width| {
                            [
                                surface(kind, y, x),
                                surface(kind, y, x + width),
                                surface(kind, y + width, x),
                                surface(kind, y + width, x + width),
                            ]
                            .map(f64::from)
                        };
                        let coarse = corners(y, x, width);
                        for row in 0..width {
                            for col in 0..width {
                                let native = corners(y + row, x + col, 1);
                                for fy in [0.0, 0.25, 0.75, 1.0] {
                                    for fx in [0.0, 0.25, 0.75, 1.0] {
                                        let actual = interpolate(native, fy, fx);
                                        let merged = interpolate(
                                            coarse,
                                            (f64::from(row) + fy) / f64::from(width),
                                            (f64::from(col) + fx) / f64::from(width),
                                        );
                                        assert!((actual.0 - merged.0).abs() <= height_error + 1e-9);
                                        assert!(
                                            (actual.1 - merged.1 / f64::from(width)).abs() <= gradient_error + 1e-9
                                        );
                                        assert!(
                                            (actual.2 - merged.2 / f64::from(width)).abs() <= gradient_error + 1e-9
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn conversion_preserves_native_samples_and_bounds_include_the_high_edge() {
        let rect = CellRect { min_i: 1200, min_j: 1100, rows: 1, cols: 2 };
        let mut native = ShardWriter::new(std::io::Cursor::new(Vec::new()), 9, 14, rect).unwrap();
        for value in [10i16, 900] {
            native.push(Some(&value.to_le_bytes().repeat(1024))).unwrap();
        }
        let bytes = native.finish().unwrap().into_inner();
        let mut converted = std::io::Cursor::new(Vec::new());
        convert(&bytes, &mut converted).unwrap();
        let source = SliceSource(converted.get_ref());
        let reader = TerrainReader::parse(&source).unwrap();
        let lat = GRID_ORIGIN + (1200 << 14);
        let lon = GRID_ORIGIN + (1100 << 14);
        assert_eq!(reader.sample(&mut TileCache::<4>::new(), lat, lon), Some(10));
        let first = u32::from_le_bytes(converted.get_ref()[32..36].try_into().unwrap());
        assert_eq!(&converted.get_ref()[first as usize..first as usize + 2048], &bytes[40..40 + 2048]);
        let level = SurfaceLayout::new(9, 14).unwrap().level(0).unwrap();
        let at = first as usize + level.bound_offset(0, 28, 2).unwrap() as usize;
        assert_eq!(i16::from_le_bytes(converted.get_ref()[at..at + 2].try_into().unwrap()), 900);
    }
}
