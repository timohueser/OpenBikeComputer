//! The mesher's parameters. They are the same in every region, so they live here with their
//! reasons and not in a rule table.

use crate::faces::{HeightRule, Surface};
use crate::frame::CELL_UNITS;

/// Frame units per level-18 and level-17 grid cell: the terrain's grid where the mesh draws, and the
/// cells of the owned region.
pub const L18: i32 = 1024;
pub const L17: i32 = 2048;
/// Level-17 cells along a side of the cell.
pub const CELLS17: i32 = (CELL_UNITS / L17 as i64) as i32;
/// Level-18 grid points along a side of the cell, both borders included.
pub const SIDE18: i32 = (CELL_UNITS / L18 as i64) as i32 + 1;

/// The level-17 rings that a sharp face's cells grow by into R: one ring of ground that holds the
/// crease at the face's outer edge, and the strip.
pub const FACE_RING: i32 = 2;

/// A grid point keeps this far from every face edge (m): nearer, it adds only a sliver beside the
/// edge's own points.
pub const GRID_CLEARANCE_M: f64 = 0.4;

/// A sharp face: its height rule is neither slope nor terrain. The terrain grid cannot draw its
/// edges, so the mesh draws it.
pub fn sharp(rule: HeightRule) -> bool {
    !matches!(rule, HeightRule::Terrain | HeightRule::Slope)
}

/// The longest piece of a face edge (m). The grid samples a face's inside; an edge has no grid
/// points, so it splits into equal pieces no longer than this:
/// - paved faces, rail beds and platforms: 4 m, the chord of a vertical radius of 200 m stays under
///   1 cm;
/// - shoulders and verges: 3 m, narrow bands beside the carriageway;
/// - blend faces (junction regions, corner fills): 2 m, the blend heights curve;
/// - slope and terrain: 2.4 m, the level-18 grid's diagonal.
pub fn l_max_m(surface: Surface, rule: HeightRule) -> f64 {
    match (rule, surface) {
        (HeightRule::Terrain | HeightRule::Slope, _) => 2.4,
        (HeightRule::Blend, _) | (_, Surface::CornerFill) => 2.0,
        (_, Surface::Shoulder | Surface::Verge) => 3.0,
        _ => 4.0,
    }
}
