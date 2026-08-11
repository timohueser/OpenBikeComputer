//! `--bbox`: the pack's own lattice, cut out of the published one.
//!
//! A global cycle is 24 shards of 6,144 x 4,608 cells; an event pack that lives in the repo has to
//! be a corridor-sized window. So a pack bakes **the real cycle over a smaller lattice** — same
//! cell pitch, same origin phase, same tile edge and paging, same mosaic, same emitter, same
//! publisher — rather than cropping objects after the fact. Two rules make that window honest:
//!
//! * **The window is a whole number of lattice cells of the published lattice**, at the published
//!   pitch, so a pack cell is one canonical cell and no second resampling happens anywhere.
//! * **It is aligned outward to the tile edge**, so tile boundaries land exactly where the global
//!   bake puts them and a pack tile is the tile a production object would carry for that ground.
//!
//! Before #1246 this module cropped a *baked product* instead, because the baker published one
//! object per product per frame on the product's own lattice and a pack was a sub-rectangle of it.
//! There is one lattice now, so choosing a window of it is the whole of what cropping meant.

use crate::canonical::{Lattice, CANONICAL};
use crate::geometry::GridGeometry;
use crate::pack::BboxUdeg;

/// The retained window of `geometry`, in cell indices: `[col0, col1)` x `[row0, row1)`, aligned
/// outward to `tile_edge` so retained tiles are whole tiles of the uncropped grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub col0: u32,
    pub col1: u32,
    pub row0: u32,
    pub row1: u32,
}

impl Window {
    pub fn width(&self) -> u32 {
        self.col1 - self.col0
    }
    pub fn height(&self) -> u32 {
        self.row1 - self.row0
    }
}

/// The retained `[start, end)` cell range of one axis, or `None` when the request and the grid do
/// not overlap on that axis.
///
/// Emptiness is decided in **unclamped** index space, before any clamp or tile alignment touches
/// the numbers. That ordering is the whole correctness argument: clamping first folds a window
/// that lies wholly past an edge back onto the boundary cell, where alignment then widens it into
/// a plausible-looking sliver — an 8-column strip of the CONUS east edge for a bbox over Europe,
/// which is not an empty crop but a *wrong* one.
///
/// The arithmetic runs in `i128` so it is **total** over every `i64` input, including the
/// saturated `i64::MAX` an out-of-range `--bbox` used to produce. `BboxUdeg::validate` rejects
/// those at the boundary, but this function must not depend on having been called correctly:
/// the workspace sets no `[profile.release]`, so `overflow-checks` is off and a wrapped
/// subtraction here would come back as a confident wrong window rather than a panic — the exact
/// sliver class this function exists to refuse.
fn axis(origin: i64, step: i64, limit: u32, low: i64, high: i64, edge: u32) -> Option<(u32, u32)> {
    let (origin, step) = (i128::from(origin), i128::from(step));
    // The cell containing `low`, and the exclusive end that retains the cell containing `high`.
    let first = (i128::from(low) - origin).div_euclid(step);
    let last = (i128::from(high) - origin + step - 1).div_euclid(step);
    let start = first.max(0);
    let end = last.min(i128::from(limit));
    if end <= start {
        return None;
    }
    // Both are now inside `0..=limit`, so the narrowing is exact.
    let (start, end) = (start as u32, end as u32);
    // Only now: align outward to whole tiles of this grid.
    let start = (start / edge) * edge;
    let end = end.div_ceil(edge).saturating_mul(edge).min(limit);
    Some((start, end))
}

/// Compute the retained window, or say plainly that the bbox and the frame do not overlap.
pub fn window(geometry: &GridGeometry, bbox: &BboxUdeg) -> Result<Window, String> {
    let edge = u32::from(geometry.tile_edge);
    let disjoint = |axis_name: &str| {
        format!(
            "crop window {bbox:?} does not intersect the frame's grid on the {axis_name} axis \
             (grid {}..{} lat, {}..{} lon udeg)",
            geometry.south_lat_udeg,
            geometry.north_lat_udeg(),
            geometry.west_lon_udeg,
            geometry.east_lon_udeg()
        )
    };
    let (col0, col1) = axis(
        i64::from(geometry.west_lon_udeg),
        i64::from(geometry.cell_lon_udeg),
        geometry.width,
        bbox.west_udeg,
        bbox.east_udeg,
        edge,
    )
    .ok_or_else(|| disjoint("longitude"))?;
    let (row0, row1) = axis(
        i64::from(geometry.south_lat_udeg),
        i64::from(geometry.cell_lat_udeg),
        geometry.height,
        bbox.south_udeg,
        bbox.north_udeg,
        edge,
    )
    .ok_or_else(|| disjoint("latitude"))?;
    Ok(Window { col0, col1, row0, row1 })
}

/// Crop a geometry alone. The returned origin is the retained window's south-west corner — exact
/// integer microdegrees, so the cropped grid georeferences to the same ground as the cells it
/// will keep.
pub fn crop_geometry(geometry: &GridGeometry, bbox: &BboxUdeg) -> Result<(GridGeometry, Window), String> {
    let window = window(geometry, bbox)?;
    let shift = |origin: i32, index: u32, step: u32| -> Result<i32, String> {
        i32::try_from(i64::from(origin) + i64::from(index) * i64::from(step))
            .map_err(|_| "crop: the cropped origin overflows microdegrees".to_string())
    };
    let cropped = GridGeometry {
        south_lat_udeg: shift(geometry.south_lat_udeg, window.row0, geometry.cell_lat_udeg)?,
        west_lon_udeg: shift(geometry.west_lon_udeg, window.col0, geometry.cell_lon_udeg)?,
        width: window.width(),
        height: window.height(),
        ..*geometry
    };
    cropped.validate()?;
    Ok((cropped, window))
}

/// The pack's lattice: the published lattice restricted to `bbox`, as **one shard**.
///
/// Same origin phase, same 0.01 degree pitch, same tile edge and paging as [`CANONICAL`], so every
/// object a pack holds is the shape production emits and every cell is a canonical cell. One shard
/// because a pack is corridor-sized by construction: `s0-0` is the whole window, and the manifest
/// it publishes is a real manifest with a one-bit presence bitmap.
pub fn sub_lattice(bbox: &BboxUdeg) -> Result<Lattice, String> {
    let full = CANONICAL.geometry(crate::canonical::LatticeWindow {
        col0: 0,
        row0: 0,
        width: CANONICAL.width,
        height: CANONICAL.height,
    });
    let cut = window(&full, bbox)?;
    // Widen before multiplying, like `axis()` above and for the same reason: `overflow-checks` is
    // off in this workspace, so a `u32` product that wrapped would come back as a confident wrong
    // origin rather than a panic. Today's bounds cannot reach it; the standard should not differ
    // by twenty lines within one file.
    let shift = |index: u32, origin: i32| -> Result<i32, String> {
        i32::try_from(i64::from(origin) + i64::from(index) * i64::from(CANONICAL.cell_udeg))
            .map_err(|_| "the pack lattice origin overflows microdegrees".to_string())
    };
    let lattice = Lattice {
        south_lat_udeg: shift(cut.row0, CANONICAL.south_lat_udeg)?,
        west_lon_udeg: shift(cut.col0, CANONICAL.west_lon_udeg)?,
        width: cut.width(),
        height: cut.height(),
        shard_width: cut.width(),
        shard_height: cut.height(),
        ..CANONICAL
    };
    lattice.validate()?;
    if lattice.shard_count() != 1 {
        return Err(format!("a pack lattice must be one shard, not {}", lattice.shard_count()));
    }
    Ok(lattice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::obcg;

    const GRID: GridGeometry = GridGeometry {
        south_lat_udeg: 20_000_000,
        west_lon_udeg: -130_000_000,
        cell_lat_udeg: 10_000,
        cell_lon_udeg: 10_000,
        width: 200,
        height: 100,
        cell_size_m: 1_000,
        tile_edge: 16,
        entries_per_page: 512,
    };

    #[test]
    fn the_window_is_aligned_outward_to_whole_tiles() {
        // A bbox one cell inside the grid's south-west corner still starts at tile 0.
        let bbox = BboxUdeg {
            south_udeg: 20_010_000,
            west_udeg: -129_990_000,
            north_udeg: 20_350_000,
            east_udeg: -129_500_000,
        };
        let window = window(&GRID, &bbox).unwrap();
        assert_eq!(window.col0 % 16, 0);
        assert_eq!(window.row0 % 16, 0);
        assert_eq!(window.col0, 0);
        assert_eq!(window.row0, 0);
        // East edge 50 cells in -> col1 rounds up to 64; north edge 35 cells in -> row1 = 48.
        assert_eq!(window.col1, 64);
        assert_eq!(window.row1, 48);
    }

    /// The grid's own extent is not a multiple of the tile edge (200 x 100 at 16), so a crop that
    /// reaches the far corner must stop at the grid, not past it.
    #[test]
    fn the_window_never_runs_past_the_grid() {
        let bbox = BboxUdeg {
            south_udeg: 20_000_000 - 5_000_000,
            west_udeg: -140_000_000,
            north_udeg: 90_000_000,
            east_udeg: 0,
        };
        let window = window(&GRID, &bbox).unwrap();
        assert_eq!(window, Window { col0: 0, col1: 200, row0: 0, row1: 100 });
    }

    /// Each edge, on its own. A bbox that misses on **one** axis must be refused even though the
    /// other axis overlaps perfectly — the bug this pins produced a plausible-looking sliver of
    /// the boundary tile instead: `--bbox 45,-5,50,5` (Europe) against the CONUS grid silently
    /// cropped an 8-column strip of the east edge, out over the Atlantic. The old single-bbox
    /// guard test passed only because its window missed on *both* axes, so the latitude check
    /// masked the hole in the longitude one.
    #[test]
    fn a_bbox_disjoint_on_either_axis_alone_is_an_error_not_a_boundary_sliver() {
        // GRID spans 20.0..21.0 N, -130.0..-128.0 E.
        let cases = [
            ("east of the grid", 20_200_000i64, -127_000_000i64, 20_800_000i64, -126_000_000i64, "longitude"),
            ("west of the grid", 20_200_000, -133_000_000, 20_800_000, -131_000_000, "longitude"),
            ("north of the grid", 22_000_000, -129_500_000, 23_000_000, -128_500_000, "latitude"),
            ("south of the grid", 18_000_000, -129_500_000, 19_000_000, -128_500_000, "latitude"),
        ];
        for (name, south, west, north, east, axis) in cases {
            let bbox = BboxUdeg { south_udeg: south, west_udeg: west, north_udeg: north, east_udeg: east };
            let Err(error) = window(&GRID, &bbox) else {
                panic!("{name}: a bbox {axis}-disjoint from the grid must be refused, not cropped");
            };
            assert!(error.contains("does not intersect"), "{name}: {error}");
            assert!(error.contains(axis), "{name}: the message must name the {axis} axis — {error}");
        }
        // Touching an edge exactly covers zero cells, and is refused for the same reason.
        let touching = BboxUdeg {
            south_udeg: 20_200_000,
            west_udeg: -128_000_000,
            north_udeg: 20_800_000,
            east_udeg: -127_000_000,
        };
        assert!(window(&GRID, &touching).is_err(), "a bbox starting exactly at the east edge covers no cell");
    }

    /// The inner of the two overflow defences: even the extreme `i64` values a saturating cast
    /// can produce must give an honest answer, not a wrapped one.
    ///
    /// `BboxUdeg::validate` now rejects these at the boundary, so this is reachable only by
    /// constructing a `BboxUdeg` directly — but the workspace sets no `[profile.release]`, so a
    /// release build has `overflow-checks` off and a wrapped subtraction here comes back as a
    /// confident answer rather than a panic. Measured against the real lattice, the old `i64`
    /// version wrapped on exactly two of the four extremes, and the failure mode was a *false
    /// disjointness* — fail-safe, if wrong. The genuinely dangerous half of the defect is the one
    /// `BboxUdeg::validate` fixes: `1e30` saturates to `i64::MAX`, which does not overflow, so a
    /// fat-fingered decimal cropped half a continent silently instead of being refused.
    #[test]
    fn saturated_bbox_edges_cannot_wrap_the_axis_arithmetic() {
        let grid = crate::source::mrms::GEOMETRY;
        let sane =
            BboxUdeg { south_udeg: 40_500_000, west_udeg: -96_500_000, north_udeg: 43_500_000, east_udeg: -90_000_000 };
        let full = window(&grid, &sane).expect("the sane window crops");

        // A window whose west edge is the most negative i64 still starts at the grid's west edge…
        let west = BboxUdeg { west_udeg: i64::MIN, ..sane };
        assert_eq!(window(&grid, &west).unwrap(), Window { col0: 0, ..full });
        // …and one whose east edge saturates positive still stops at the grid's east edge.
        let east = BboxUdeg { east_udeg: i64::MAX, ..sane };
        assert_eq!(window(&grid, &east).unwrap(), Window { col1: grid.width, ..full });
        let south = BboxUdeg { south_udeg: i64::MIN, ..sane };
        assert_eq!(window(&grid, &south).unwrap(), Window { row0: 0, ..full });
        let north = BboxUdeg { north_udeg: i64::MAX, ..sane };
        assert_eq!(window(&grid, &north).unwrap(), Window { row1: grid.height, ..full });

        // Wholly past an edge stays disjoint at the extremes too, in both directions.
        let past_east =
            BboxUdeg { south_udeg: 40_500_000, west_udeg: i64::MAX - 1, north_udeg: 43_500_000, east_udeg: i64::MAX };
        assert!(window(&grid, &past_east).is_err());
        let past_west =
            BboxUdeg { south_udeg: 40_500_000, west_udeg: i64::MIN, north_udeg: 43_500_000, east_udeg: i64::MIN + 1 };
        assert!(window(&grid, &past_west).is_err());
        // And the whole planet is the whole grid, not an overflow.
        let planet = BboxUdeg { south_udeg: i64::MIN, west_udeg: i64::MIN, north_udeg: i64::MAX, east_udeg: i64::MAX };
        assert_eq!(window(&grid, &planet).unwrap(), Window { col0: 0, col1: grid.width, row0: 0, row1: grid.height });
    }

    /// The composite of the four: disjoint on both axes at once.
    #[test]
    fn a_wholly_disjoint_bbox_is_an_error_not_an_empty_frame() {
        let bbox =
            BboxUdeg { south_udeg: -40_000_000, west_udeg: 10_000_000, north_udeg: -30_000_000, east_udeg: 20_000_000 };
        let error = window(&GRID, &bbox).unwrap_err();
        assert!(error.contains("does not intersect"), "{error}");
    }

    /// A pack lattice is the published lattice, restricted: same pitch, same origin phase, same
    /// tile edge and paging, and one shard. If any of those drifted, a pack would stop being
    /// evidence about what production emits.
    #[test]
    fn a_pack_lattice_is_the_published_one_restricted_to_the_bbox() {
        let bbox =
            BboxUdeg { south_udeg: 40_500_000, west_udeg: -96_500_000, north_udeg: 43_500_000, east_udeg: -90_000_000 };
        let lattice = sub_lattice(&bbox).expect("Iowa is on the lattice");
        assert_eq!(lattice.cell_udeg, CANONICAL.cell_udeg);
        assert_eq!(lattice.cell_size_m, CANONICAL.cell_size_m);
        assert_eq!((lattice.tile_edge, lattice.entries_per_page), (CANONICAL.tile_edge, CANONICAL.entries_per_page));
        assert_eq!(lattice.shard_count(), 1);
        // Origin phase and tile alignment against the global lattice, in cells.
        let col0 =
            (i64::from(lattice.west_lon_udeg) - i64::from(CANONICAL.west_lon_udeg)) / i64::from(CANONICAL.cell_udeg);
        let row0 =
            (i64::from(lattice.south_lat_udeg) - i64::from(CANONICAL.south_lat_udeg)) / i64::from(CANONICAL.cell_udeg);
        assert_eq!(col0 % i64::from(CANONICAL.tile_edge), 0, "the window must start on a global tile boundary");
        assert_eq!(row0 % i64::from(CANONICAL.tile_edge), 0, "the window must start on a global tile boundary");
        // And it must actually contain the bbox it was asked for.
        assert!(i64::from(lattice.west_lon_udeg) <= bbox.west_udeg);
        assert!(i64::from(lattice.south_lat_udeg) <= bbox.south_udeg);
        let geometry = lattice.geometry(lattice.shard(0).expect("one shard"));
        assert!(geometry.east_lon_udeg() >= bbox.east_udeg);
        assert!(geometry.north_lat_udeg() >= bbox.north_udeg);
    }

    /// Tile alignment is the point: a pack object's tiles must encode to the bytes a full-lattice
    /// object would carry for the same ground, or a pack stops being evidence about the real bake.
    #[test]
    fn tiles_of_an_aligned_window_encode_identically_to_the_uncropped_object() {
        let full_cells: Vec<u8> = (0..GRID.cells()).map(|index| (index % 13) as u8).collect();
        let bbox = BboxUdeg {
            south_udeg: 20_320_000,
            west_udeg: -129_360_000,
            north_udeg: 20_640_000,
            east_udeg: -129_040_000,
        };
        let cut = window(&GRID, &bbox).unwrap();
        let cropped_geometry = GridGeometry {
            south_lat_udeg: GRID.south_lat_udeg + (cut.row0 * GRID.cell_lat_udeg) as i32,
            west_lon_udeg: GRID.west_lon_udeg + (cut.col0 * GRID.cell_lon_udeg) as i32,
            width: cut.width(),
            height: cut.height(),
            ..GRID
        };
        let mut cropped_cells = Vec::with_capacity(cropped_geometry.cells());
        for row in cut.row0..cut.row1 {
            let start = row as usize * GRID.width as usize;
            cropped_cells.extend_from_slice(&full_cells[start + cut.col0 as usize..start + cut.col1 as usize]);
        }

        let encode = |geometry: &GridGeometry, cells: &[u8]| {
            let input = obcg::FrameInput {
                flags: obcg::FLAG_OBSERVED,
                valid_at: 1_800_000_000,
                reference_time: 1_800_000_000,
                south_lat_udeg: geometry.south_lat_udeg,
                west_lon_udeg: geometry.west_lon_udeg,
                cell_lat_udeg: geometry.cell_lat_udeg,
                cell_lon_udeg: geometry.cell_lon_udeg,
                width: geometry.width,
                height: geometry.height,
                cell_size_m: geometry.cell_size_m,
                tile_edge: geometry.tile_edge,
                entries_per_page: geometry.entries_per_page,
                cells,
            };
            let mut scratch = vec![0u8; usize::from(geometry.tile_edge) * usize::from(geometry.tile_edge)];
            let len = obcg::encoded_len(&input, &mut scratch).unwrap() as usize;
            let mut bytes = vec![0u8; len];
            obcg::encode_format(&input, &mut scratch, &mut bytes).unwrap();
            bytes
        };
        let full = encode(&GRID, &full_cells);
        let cropped = encode(&cropped_geometry, &cropped_cells);

        let read_tile = |bytes: &[u8], tile_col: u32, tile_row: u32| -> Vec<u8> {
            let header = obcg::decode_header(bytes[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
            let index = header.tile_index(tile_col, tile_row).unwrap();
            let page = header.page_of_entry(index);
            let offset = header.page_offset(page).unwrap() as usize;
            let slice = &bytes[offset..offset + header.page_bytes() as usize];
            let entry =
                obcg::decode_entry(slice, (index - page * u32::from(header.entries_per_page)) as usize).unwrap();
            if entry.is_dry() {
                return Vec::new();
            }
            bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)].to_vec()
        };
        let tiles_across = cropped_geometry.width.div_ceil(u32::from(cropped_geometry.tile_edge));
        let tiles_up = cropped_geometry.height.div_ceil(u32::from(cropped_geometry.tile_edge));
        assert!(tiles_across > 0 && tiles_up > 0);
        let edge = u32::from(GRID.tile_edge);
        for tile_row in 0..tiles_up {
            for tile_col in 0..tiles_across {
                assert_eq!(
                    read_tile(&cropped, tile_col, tile_row),
                    read_tile(&full, tile_col + cut.col0 / edge, tile_row + cut.row0 / edge),
                    "tile ({tile_col},{tile_row}) is not the uncropped object's tile"
                );
            }
        }
    }
}
