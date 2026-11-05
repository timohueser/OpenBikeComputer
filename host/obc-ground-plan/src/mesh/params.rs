//! The mesher's parameters. They are the same in every region, so they live here with their
//! reasons and not in a rule table.

use super::Range;
use crate::faces::{HeightRule, Surface};

/// The epic's least angle; Ruppert's refinement terminates up to 20.7°.
pub const MIN_ANGLE_DEG: f64 = 20.0;
/// Frame units per level-18 and level-17 grid cell: the terrain's grid where each range draws.
pub const L18: i32 = 1024;
pub const L17: i32 = 2048;
/// Level-17 cells along a side of the cell.
pub const CELLS17: i32 = (crate::frame::CELL_UNITS / L17 as i64) as i32;

/// The grid of a range: the terrain's density where the range draws.
pub fn grid(range: Range) -> i32 {
    match range {
        Range::Near => L18,
        Range::Far => L17,
    }
}

/// A grid point keeps this far from every constraint (m): the epic for near, scaled for far.
pub fn grid_clearance_m(range: Range) -> f64 {
    match range {
        Range::Near => 0.4,
        Range::Far => 0.8,
    }
}

/// The longest edge of a face's triangles (m), near; far doubles it.
/// - paved faces, rail beds and platforms: 4 m, the chord error of a vertical radius of 200 m stays
///   under 1 cm;
/// - shoulders and verges: 3 m, narrow bands where the angle bound rules anyway;
/// - blend faces (junction regions, corner fills): 2 m, the blend heights curve;
/// - slope and terrain rules: 2.4 m, over the level-18 grid's diagonal (2.30 m), so grid triangles
///   stay whole.
pub fn l_max_m(surface: Surface, rule: HeightRule, range: Range) -> f64 {
    let near = match (rule, surface) {
        (HeightRule::Terrain | HeightRule::Slope, _) => 2.4,
        (HeightRule::Blend, _) | (_, Surface::CornerFill) => 2.0,
        (_, Surface::Shoulder | Surface::Verge) => 3.0,
        _ => 4.0,
    };
    match range {
        Range::Near => near,
        Range::Far => 2.0 * near,
    }
}

/// The largest ratio of two neighbouring border segments.
pub const BORDER_GRADE: i64 = 3;
/// The largest near `l_max_m` (m): a plan vertex farther from a border span makes no skinny triangle
/// on it.
pub const FOOT_M: f64 = 4.0;
/// A level-18 grid point on a border span keeps this far from the split's other points (m): the
/// near grid clearance.
pub const BORDER_GRID_M: f64 = 0.4;
/// Rounds of strip growth; the property test finds no cell that needs more than one.
pub const STRIP_ROUNDS: i32 = 2;
/// Level-17 cells of R about the cell: two rings of the faces' growth, and two per strip round,
/// each of which reads and changes R within one cell.
pub const REGION_RING: i32 = 2 + 2 * STRIP_ROUNDS;
/// The faces' cells grow by this many rings, so no face reaches the strip and the inner border lies
/// a level-17 cell or more from every face but terrain.
pub const FACE_RING: i32 = 2;
/// Refinement adds at most this many vertices per vertex before it.
pub const REFINE_BUDGET: usize = 3;
/// Concentric shells and Shewchuk's rule apply at input corners under this angle.
pub const SMALL_CORNER_DEG: f64 = 60.0;
/// Two points lie equally far from their segments' common end within this many frame units: a
/// rounding moves each by at most 0.71.
pub const EQUAL_UNITS: f64 = 2.0;

/// The offset of the normal's differences (m): small against the least `l_max_m`, so the normal is
/// the function's at the vertex, and under the 0.41 m texel of a level-18 page.
pub fn normal_step_m(range: Range) -> f64 {
    match range {
        Range::Near => 0.1,
        Range::Far => 0.2,
    }
}

/// The floor lies this far under the drawn mesh (m): the epic. It covers one r16 step of a page,
/// the page's bilinear against the crate's bicubic and f32 rounding.
pub const FLOOR_MARGIN_M: f64 = 0.05;
/// The step of a floor code (m): a u16 spans 655 m.
pub const FLOOR_STEP_M: f64 = 0.01;
