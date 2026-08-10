//! `--bbox`: crop the **baked** output, never the raw upstream.
//!
//! A full-domain US pack is hundreds of megabytes of OBCG — the MRMS observation alone is a
//! 7,000 x 3,500 grid. An event pack that lives in the repo has to be a corridor-sized window, so
//! the capture tool crops what the baker emits. Two rules keep the crop from becoming a second
//! resampler:
//!
//! * **Cells are copied, never recomputed.** The crop is a sub-rectangle memcpy of the quantized
//!   cell grid the adapter produced; no interpolation, no re-quantization, no smoothing.
//! * **The window is aligned outward to the frame's tile edge.** Tile boundaries then land in
//!   exactly the same places as in the full bake, so every retained tile's payload bytes are
//!   identical to the uncropped object's. The crop is a *subset* of the full bake, not a
//!   different bake — which is the property that lets a cropped pack still be evidence about the
//!   real product.
//!
//! The crop rides in as an [`Adapter`] wrapper, so [`crate::cycle::run_cycle`], [`crate::emit`],
//! the manifest and the publisher stay exactly the code that runs in production. Nothing in the
//! bakery learns that packs exist.

use crate::fetch::Upstream;
use crate::geometry::GridGeometry;
use crate::manifest::Product;
use crate::pack::BboxUdeg;
use crate::source::{verify_frames_nest, Adapter, AdapterOutcome, BakedFrame, BakedProduct, FrameSource};

/// An adapter that bakes exactly like `inner` and then crops the result to `bbox`.
pub struct CroppedAdapter<'a> {
    inner: &'a dyn Adapter,
    bbox: BboxUdeg,
}

impl<'a> CroppedAdapter<'a> {
    pub fn new(inner: &'a dyn Adapter, bbox: BboxUdeg) -> Self {
        Self { inner, bbox }
    }
}

impl Adapter for CroppedAdapter<'_> {
    fn id(&self) -> &'static str {
        self.inner.id()
    }

    fn bake(
        &self,
        upstream: &mut dyn Upstream,
        previous: Option<&Product>,
        now: i64,
        warnings: &mut Vec<String>,
    ) -> Result<AdapterOutcome, String> {
        match self.inner.bake(upstream, previous, now, warnings)? {
            AdapterOutcome::Unchanged => Ok(AdapterOutcome::Unchanged),
            AdapterOutcome::Baked(product) => {
                let cropped = crop_product(*product, self.bbox)?;
                // The inner adapter proved its frames nest before it returned; the crop moved
                // every origin, so prove it again rather than assume the property survived. It
                // does survive whenever each frame's tile stride is a multiple of the finest
                // cell — which is true today and is exactly what this catches if it stops being.
                verify_frames_nest(&cropped)?;
                Ok(AdapterOutcome::Baked(Box::new(cropped)))
            }
        }
    }
}

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

/// Crop one grid and its cells.
pub fn crop_grid(geometry: &GridGeometry, cells: &[u8], bbox: &BboxUdeg) -> Result<(GridGeometry, Vec<u8>), String> {
    if cells.len() != geometry.cells() {
        return Err("crop: cell count disagrees with the geometry".into());
    }
    let (cropped, window) = crop_geometry(geometry, bbox)?;
    let mut out = Vec::with_capacity(cropped.cells());
    for row in window.row0..window.row1 {
        let start = row as usize * geometry.width as usize;
        out.extend_from_slice(&cells[start + window.col0 as usize..start + window.col1 as usize]);
    }
    Ok((cropped, out))
}

/// Crop a whole baked product: every frame on its own lattice, plus the product's nominal one.
pub fn crop_product(product: BakedProduct, bbox: BboxUdeg) -> Result<BakedProduct, String> {
    let BakedProduct {
        id,
        product_code,
        tier,
        geometry,
        reference_time,
        staleness_deadline,
        attribution,
        upstream_etag,
        frames,
    } = product;
    let mut cropped_frames = Vec::with_capacity(frames.len());
    for frame in frames {
        let frame_geometry = frame.source.map_or(geometry, |source| source.geometry);
        let (new_geometry, cells) = crop_grid(&frame_geometry, &frame.cells, &bbox)
            .map_err(|error| format!("{id} f{}: {error}", frame.offset_min))?;
        cropped_frames.push(BakedFrame {
            offset_min: frame.offset_min,
            valid_at: frame.valid_at,
            flags: frame.flags,
            // A frame that carried its own provenance keeps it, on its own cropped lattice. A
            // frame that inherited the product's stays inheriting.
            source: frame.source.map(|source| FrameSource { geometry: new_geometry, ..source }),
            cells,
        });
    }
    // The nominal lattice is cropped the same way, so the manifest's product bbox stays the
    // honest intersection of what the timeline actually answers.
    let (nominal, _) = crop_geometry(&geometry, &bbox)?;
    Ok(BakedProduct {
        id,
        product_code,
        tier,
        geometry: nominal,
        reference_time,
        staleness_deadline,
        attribution,
        upstream_etag,
        frames: cropped_frames,
    })
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

    fn cells() -> Vec<u8> {
        (0..GRID.cells()).map(|index| (index % 13) as u8).collect()
    }

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
        // East edge 50 cells in → col1 rounds up to 64; north edge 35 cells in → row1 = 48.
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
    /// confident answer rather than a panic.
    ///
    /// Measured against the real MRMS lattice, the old `i64` version wrapped on exactly two of the
    /// four extremes, and they are **not** the two the review cited:
    ///
    /// | edge | old `i64` (wrapping) | correct |
    /// |---|---|---|
    /// | `west = i64::MIN` (lon origin negative) | `Some((0, 4032))` | same — no overflow |
    /// | `north = i64::MAX` (lat origin positive) | `Some((2048, 3500))` | same — no overflow |
    /// | `east = i64::MAX` (lon origin negative) | `None` | `Some((3328, 7000))` |
    /// | `south = i64::MIN` (lat origin positive) | `None` | `Some((0, 2368))` |
    ///
    /// So the arithmetic hazard was a *false disjointness* — fail-safe, if wrong. The genuinely
    /// dangerous half of this defect is the one `BboxUdeg::validate` fixes: `1e30` saturates to
    /// `i64::MAX`, which does not overflow, and a fat-fingered decimal therefore cropped half of
    /// CONUS silently instead of being refused.
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

    /// The real geometries, the real crop: the US product's two lattices must still nest after
    /// the crop moved both origins, or a client holding the bundle drops the 1 km radar frame.
    #[test]
    fn cropping_preserves_the_us_products_lattice_nesting() {
        use crate::source::{hrrr, mrms};
        let bbox =
            BboxUdeg { south_udeg: 40_500_000, west_udeg: -96_500_000, north_udeg: 43_500_000, east_udeg: -90_000_000 };
        let (observation, _) = crop_geometry(&mrms::GEOMETRY, &bbox).expect("the observation crops");
        let (forecast, _) = crop_geometry(&hrrr::GEOMETRY, &bbox).expect("the forecast crops");
        assert!(observation.cell_area() < forecast.cell_area());
        assert!(
            observation.nests_under(&forecast),
            "cropped lattices stopped nesting: observation={observation:?} forecast={forecast:?}"
        );
    }

    /// The crop copies cells and re-anchors the origin: every retained cell keeps its ground
    /// position to the microdegree.
    #[test]
    fn cropped_cells_are_the_same_cells_at_the_same_coordinates() {
        let bbox = BboxUdeg {
            south_udeg: 20_300_000,
            west_udeg: -129_400_000,
            north_udeg: 20_700_000,
            east_udeg: -128_900_000,
        };
        let (geometry, cropped) = crop_grid(&GRID, &cells(), &bbox).unwrap();
        let window = window(&GRID, &bbox).unwrap();
        assert_eq!((geometry.width, geometry.height), (window.width(), window.height()));
        assert_eq!(cropped.len(), geometry.cells());
        for row in 0..geometry.height {
            for col in 0..geometry.width {
                let source = (row + window.row0) as usize * GRID.width as usize + (col + window.col0) as usize;
                assert_eq!(cropped[(row * geometry.width + col) as usize], cells()[source]);
                // Same ground, to the microdegree.
                assert!((geometry.center_lat_deg(row) - GRID.center_lat_deg(row + window.row0)).abs() < 1e-12);
                assert!((geometry.center_lon_deg(col) - GRID.center_lon_deg(col + window.col0)).abs() < 1e-12);
            }
        }
    }

    /// Tile alignment is the whole point: a cropped object's retained tiles must encode to the
    /// same bytes as the full object's, so a cropped pack is a subset of the real bake.
    #[test]
    fn retained_tiles_encode_identically_to_the_uncropped_object() {
        let full_cells = cells();
        let bbox = BboxUdeg {
            south_udeg: 20_320_000,
            west_udeg: -129_360_000,
            north_udeg: 20_640_000,
            east_udeg: -129_040_000,
        };
        let (cropped_geometry, cropped_cells) = crop_grid(&GRID, &full_cells, &bbox).unwrap();
        let window = window(&GRID, &bbox).unwrap();

        let encode = |geometry: &GridGeometry, cells: &[u8]| {
            let input = obcg::FrameInput {
                product_id: obcg::PRODUCT_EXPERIMENTAL,
                tier: obcg::TIER_RADAR,
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
                    read_tile(&full, tile_col + window.col0 / edge, tile_row + window.row0 / edge),
                    "tile ({tile_col},{tile_row}) is not the uncropped object's tile"
                );
            }
        }
    }
}
