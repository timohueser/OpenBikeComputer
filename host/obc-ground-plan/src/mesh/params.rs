//! The mesher's parameters. They are the same in every region, so they live here with their
//! reasons and not in a rule table.

use crate::faces::{HeightRule, Surface};
use crate::frame::{CELL_UNITS, FRAME_BITS};

/// The terrain level whose grid the mesh uses: the finest level that the terrain draws. Every grid
/// size below derives from it.
pub const MESH_LEVEL: u32 = 18;
/// Frame units per grid cell: a terrain node of a level spans 2^(`FRAME_BITS` − level) units in 64
/// grid cells (`GRID` of `terrain/lod.ts`).
pub const GRID: i32 = 1 << (FRAME_BITS - MESH_LEVEL - 6);
/// Frame units per cell of the region: a grid cell of the next coarser level, 2 × 2 grid cells.
pub const COARSE: i32 = 2 * GRID;
/// Region cells along a side of the cell.
pub const CELLS: i32 = (CELL_UNITS / COARSE as i64) as i32;
/// Grid points along a side of the cell, both borders included.
pub const SIDE: i32 = (CELL_UNITS / GRID as i64) as i32 + 1;

/// The region rings that a sharp face's cells grow by into R: one ring of ground that holds the
/// crease at the face's outer edge, and the strip.
pub const FACE_RING: i32 = 2;

/// A grid point keeps this far from every face edge (m): nearer, it adds only a sliver beside the
/// edge's own points.
pub const GRID_CLEARANCE_M: f64 = 0.4;

/// A sharp face: its height rule is neither slope nor terrain, or it is a bank, whose top (a crease
/// narrower than the grid) its outer edge holds, or a crossing's zone, whose slopes leave the water out, so
/// that it meets the water, the banks and the slopes beside it in steps (its headwall and wings). The
/// terrain grid cannot draw its edges, so the mesh draws it.
pub fn sharp(surface: Surface, rule: HeightRule) -> bool {
    matches!(surface, Surface::Bank | Surface::Zone) || !matches!(rule, HeightRule::Terrain | HeightRule::Slope)
}

/// The longest piece of a face edge (m). The grid samples a face's inside; an edge has no grid
/// points, so it splits into equal pieces no longer than this:
/// - paved faces, rail beds and platforms: 4 m, the chord of a vertical radius of 200 m stays under
///   1 cm;
/// - shoulders and verges: 3 m, narrow bands beside the carriageway;
/// - blend faces (junction regions, corner fills): 2 m, the blend heights curve;
/// - water: 4 m, its level runs along its line as a paved face's height;
/// - slope, bank and terrain: 2.4 m, the grid's diagonal at the mesh level.
pub fn l_max_m(surface: Surface, rule: HeightRule) -> f64 {
    match (rule, surface) {
        (HeightRule::Owner, Surface::Water) => 4.0,
        (HeightRule::Terrain | HeightRule::Slope, _) => 2.4,
        (HeightRule::Blend, _) | (_, Surface::CornerFill) => 2.0,
        (_, Surface::Shoulder | Surface::Verge) => 3.0,
        _ => 4.0,
    }
}
