//! The TrailView ground plan of one cell: from the feature tiles about it, its stored terrain pages
//! and the rule tables to exact integer geometry, terrain heights and cross-sections. A pure
//! function of its inputs: integers, `libm` and ordered collections, no I/O and no threads.

pub mod cell;
pub mod debug;
pub mod faces;
pub mod frame;
pub mod graph;
pub mod mesh;
pub mod sections;
pub mod terrain;
pub mod tile;
pub mod transport;
pub mod water_crossings;

use frame::{CellKey, GlobalPoint};
use terrain::{StoredPage, Terrain};
use tile::{own, DecodeError, Feature, Geometry, TileCache};

/// The inputs of one cell: the parts of each read tile that meet the cell grown by its margin, with
/// `own` set, and the terrain under it.
#[derive(Clone, Debug)]
pub struct CellInput {
    pub key: CellKey,
    pub features: Vec<Feature>,
    pub terrain: Terrain,
}

/// Whether the box of the points meets the closed square [x0, y0, x1, y1].
fn box_meets(points: &[GlobalPoint], [x0, y0, x1, y1]: [i64; 4]) -> bool {
    let lo = |f: fn(&GlobalPoint) -> i64| points.iter().map(f).min();
    let hi = |f: fn(&GlobalPoint) -> i64| points.iter().map(f).max();
    match (lo(|p| p.x), lo(|p| p.y), hi(|p| p.x), hi(|p| p.y)) {
        (Some(a), Some(b), Some(c), Some(d)) => c >= x0 && a <= x1 && d >= y0 && b <= y1,
        _ => false,
    }
}

impl CellInput {
    /// The input of a cell from the tiles of `key.feature_tiles()`, in that order. `load` gives a
    /// tile's bytes after gzip, or None: no features. Nothing is clipped. A point or a line part stays
    /// when its box meets the grown cell and it owns an element; an area keeps every copy of each
    /// polygon (an outer ring and its holes) whose outer ring's box meets the grown cell.
    pub fn new(
        key: CellKey,
        cache: &mut TileCache,
        mut load: impl FnMut(u32, u32) -> Option<Vec<u8>>,
        pages: Vec<StoredPage>,
    ) -> Result<CellInput, DecodeError> {
        let tiles = key.feature_tiles();
        let grown = key.grown();
        let mut features = Vec::new();
        for (at, &(column, row)) in tiles.iter().enumerate() {
            for feature in cache.tile(column, row, || load(column, row))?.iter() {
                let mut parts = Vec::new();
                let mut polygon_kept = false;
                for part in &feature.parts {
                    let mut part = part.clone();
                    part.own = own(feature.geometry, &part.points, &tiles, at);
                    let keep = match feature.geometry {
                        Geometry::Area => {
                            if part.outer {
                                polygon_kept = box_meets(&part.points, grown);
                            }
                            polygon_kept
                        }
                        Geometry::Point | Geometry::Line => {
                            box_meets(&part.points, grown) && part.own.iter().any(|&own| own)
                        }
                    };
                    if keep {
                        parts.push(part);
                    }
                }
                if !parts.is_empty() {
                    features.push(Feature { parts, ..feature.clone() });
                }
            }
        }
        Ok(CellInput { key, features, terrain: Terrain::new(key, pages) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame::tile_point;

    fn tile_bytes() -> Vec<u8> {
        std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/trailview-features.bin"),
        )
        .unwrap()
    }

    #[test]
    fn neighbouring_cells_load_a_tile_once_and_a_small_cache_changes_nothing() {
        let origin = tile_point(13, 4286, 2888, false, 0, 0);
        let key = |dx: i64| {
            CellKey::of(GlobalPoint { x: origin.x + dx * frame::CELL_UNITS, y: origin.y + 3 * frame::CELL_UNITS })
        };
        let mut loads = Vec::new();
        let mut cache = TileCache::new(16);
        let build = |cache: &mut TileCache, key: CellKey, loads: &mut Vec<(u32, u32)>| {
            CellInput::new(
                key,
                cache,
                |c, r| {
                    loads.push((c, r));
                    ((c, r) == (4286, 2888)).then(tile_bytes)
                },
                Vec::new(),
            )
            .unwrap()
        };
        let (a, b) = (build(&mut cache, key(3), &mut loads), build(&mut cache, key(4), &mut loads));
        assert_eq!(loads, [(4286, 2888)], "two cells in the tile's middle read it once");
        assert!(!a.features.is_empty() && !b.features.is_empty());
        let mut small = TileCache::new(1);
        for k in [key(0), key(3), key(-1), key(0)] {
            let (big, little) = (build(&mut cache, k, &mut Vec::new()), build(&mut small, k, &mut Vec::new()));
            assert_eq!(format!("{:?}", big.features), format!("{:?}", little.features));
        }
    }
}
