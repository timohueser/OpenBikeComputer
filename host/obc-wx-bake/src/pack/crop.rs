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
use crate::source::{Adapter, AdapterOutcome, BakedFrame, BakedProduct, FrameSource};

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
            AdapterOutcome::Baked(product) => Ok(AdapterOutcome::Baked(Box::new(crop_product(*product, self.bbox)?))),
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

/// Compute the retained window, or say plainly that the bbox and the frame do not overlap.
pub fn window(geometry: &GridGeometry, bbox: &BboxUdeg) -> Result<Window, String> {
    let edge = u32::from(geometry.tile_edge);
    let lower = |value: i64, origin: i64, step: i64, limit: u32| -> u32 {
        let index = (value - origin).div_euclid(step);
        let index = index.clamp(0, i64::from(limit)) as u32;
        (index / edge) * edge
    };
    let upper = |value: i64, origin: i64, step: i64, limit: u32| -> u32 {
        // Ceiling division: the cell containing `value` is retained whole.
        let index = (value - origin + step - 1).div_euclid(step);
        let index = index.clamp(0, i64::from(limit)) as u32;
        index.div_ceil(edge).saturating_mul(edge).min(limit)
    };
    let west = i64::from(geometry.west_lon_udeg);
    let south = i64::from(geometry.south_lat_udeg);
    let col0 = lower(bbox.west_udeg, west, i64::from(geometry.cell_lon_udeg), geometry.width);
    let col1 = upper(bbox.east_udeg, west, i64::from(geometry.cell_lon_udeg), geometry.width);
    let row0 = lower(bbox.south_udeg, south, i64::from(geometry.cell_lat_udeg), geometry.height);
    let row1 = upper(bbox.north_udeg, south, i64::from(geometry.cell_lat_udeg), geometry.height);
    if col1 <= col0 || row1 <= row0 {
        return Err(format!(
            "crop window {bbox:?} does not intersect the frame's grid ({}..{}, {}..{} udeg)",
            geometry.south_lat_udeg,
            geometry.north_lat_udeg(),
            geometry.west_lon_udeg,
            geometry.east_lon_udeg()
        ));
    }
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

    #[test]
    fn a_disjoint_bbox_is_an_error_not_an_empty_frame() {
        let bbox =
            BboxUdeg { south_udeg: -40_000_000, west_udeg: 10_000_000, north_udeg: -30_000_000, east_udeg: 20_000_000 };
        let error = window(&GRID, &bbox).unwrap_err();
        assert!(error.contains("does not intersect"), "{error}");
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
