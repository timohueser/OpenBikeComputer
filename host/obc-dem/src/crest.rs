//! Crest planes (`OBCT_Spec.md` §9): where a finer reference DEM says our lattice loses a summit.
//!
//! A `2^9` posting cannot hold a rock tower. Against 2 m swissALTI3D at Engelberg the bilinear
//! surface through Copernicus GLO-30 runs 100 m below the Hahnen's top, and the panorama loses the
//! one feature that makes the mountain recognisable. §9 fixes that with a per-sample lift the
//! panorama alone applies, and §9.4 leaves the choice of samples here.
//!
//! The rule is two tests and one dilation, and each part earns its place:
//!
//! * **[`LIFT_M`] against the bilinear surface, not the node.** The interesting quantity is how far
//!   the reference stands above the surface we actually draw, measured where it stands there.
//! * **[`CONVEX_M`] on the reference's own cell maxima.** Without it, a steep planar face reads as
//!   a crest — a coarse lattice under-samples a 40° slope honestly — and the whole mountain
//!   inflates. Measured against three photographs, the ungated rule pushed the drawn skyline half a
//!   degree high; the gate brings the median back to zero.
//! * **One sample of dilation.** A lifted node beside unlifted neighbours makes the bilinear
//!   surface sawtooth, which the panorama shows as a jittering skyline. Extending the selection by
//!   one sample lifts a crest along its whole length instead of at scattered points, and it also
//!   *improves* accuracy: on the Rigidalstock it took the root-mean-square skyline error from 0.32°
//!   to 0.24°, which is the 2 m reference's own error against the same photograph.

use obc_formats::obct::{CrestLayout, SurfaceLayout, CREST_QUANTUM, GRID_ORIGIN, NODATA};

/// A sample is a candidate when the reference stands this far above our bilinear surface.
pub const LIFT_M: f64 = 10.0;
/// …and only where the reference's cell maxima are locally convex by this much.
pub const CONVEX_M: f64 = 3.0;
/// Sub-samples per node axis when scanning a node's half-posting cell for its reference maximum.
/// 32 puts the step below 2 m at the native posting, so a metre-scale tower cannot hide between
/// them; the cap bounds the cost at the coarse levels, where the posting grows but the towers
/// do not.
const SUBSAMPLES: u32 = 32;

/// One cell's lift planes, indexed the way [`CrestLayout`] lays them out.
pub struct CrestBlock {
    layout: CrestLayout,
    bytes: Vec<u8>,
    /// Lifts over `(side + 1)²` per level, so the surface baker can bound the high-edge vertices
    /// its §8.2 maxima must cover. The file image keeps only the `side²` a cell owns.
    full: Vec<Vec<u8>>,
    sides: Vec<usize>,
    lifted: usize,
}

impl CrestBlock {
    /// Metres this sample's panorama surface stands above its native height. Accepts the inclusive
    /// high edge, which belongs to the neighbouring cell's plane but bounds this cell's maxima.
    pub fn lift(&self, level: usize, y: u32, x: u32) -> i16 {
        let (Some(plane), Some(&side)) = (self.full.get(level), self.sides.get(level)) else { return 0 };
        let stride = side + 1;
        match (y as usize, x as usize) {
            (y, x) if y < stride && x < stride => i16::from(plane[y * stride + x]) * CREST_QUANTUM as i16,
            _ => 0,
        }
    }

    /// The layout the file image follows.
    pub fn layout(&self) -> CrestLayout {
        self.layout
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Samples this block lifts at all, over every level — the number §9.4's rule selected.
    pub fn lifted(&self) -> usize {
        self.lifted
    }
}

/// Build one cell's crest block, or `None` when the reference covers none of it.
///
/// `height` is the native lattice, the same closure the surface baker samples. `reference` is the
/// finer DEM in the same geographic frame, answering `None` outside its coverage — which is the
/// common case, since national LiDAR stops at borders.
pub fn bake_cell(
    ci: u32,
    cj: u32,
    posting_log2: u8,
    cell_log2: u8,
    mut height: impl FnMut(i32, i32) -> i16,
    reference: impl Fn(f64, f64) -> Option<f64>,
) -> Option<CrestBlock> {
    let surface = SurfaceLayout::new(posting_log2, cell_log2)?;
    let layout = CrestLayout::new(surface);
    let mut bytes = vec![0u8; layout.block_bytes() as usize];
    let mut full: Vec<Vec<u8>> = Vec::with_capacity(layout.level_count());
    let mut sides: Vec<usize> = Vec::with_capacity(layout.level_count());
    let mut lifted = 0usize;
    let origin_y = i64::from(GRID_ORIGIN) + (i64::from(ci) << cell_log2);
    let origin_x = i64::from(GRID_ORIGIN) + (i64::from(cj) << cell_log2);

    for index in 0..layout.level_count() {
        let level = surface.level(index)?;
        let side = 1usize << level.samples_log2;
        // The inclusive high edge is scanned too: §8.2's maxima cover it, so the lift there has to
        // be known even though the neighbouring cell's plane is the one that stores it.
        let stride = side + 1;
        let step = 1i64 << level.posting_log2;
        // The reference maximum inside each node's own half-posting cell, and how far that
        // maximum stands above the bilinear surface we would otherwise draw there.
        let mut node_max = vec![f64::NEG_INFINITY; stride * stride];
        let mut over = vec![f64::NEG_INFINITY; stride * stride];
        let mut any = false;
        for y in 0..stride {
            for x in 0..stride {
                let lat = origin_y + (y as i64) * step;
                let lon = origin_x + (x as i64) * step;
                // One cheap probe rules out the whole node outside coverage, which is most of a
                // 58 x 40 km cell whenever the reference is a national dataset.
                if reference(lat as f64 / 1e6, lon as f64 / 1e6).is_none() {
                    continue;
                }
                // The node's own cell spans the four intervals that meet at it, so the surface
                // over that cell is defined by the 3x3 lattice around it.
                let mut around = [[0i16; 3]; 3];
                for (dy, row) in around.iter_mut().enumerate() {
                    for (dx, corner) in row.iter_mut().enumerate() {
                        *corner = height((lat + (dy as i64 - 1) * step) as i32, (lon + (dx as i64 - 1) * step) as i32);
                    }
                }
                if around.iter().flatten().any(|&h| h == NODATA) {
                    continue;
                }
                let (mut top, mut gap) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
                for sy in 0..SUBSAMPLES {
                    // The node owns the half posting either side of itself, so its cell spans
                    // [-1/2, +1/2] of the interval in both axes.
                    let fy = (sy as f64 + 0.5) / SUBSAMPLES as f64 - 0.5;
                    for sx in 0..SUBSAMPLES {
                        let fx = (sx as f64 + 0.5) / SUBSAMPLES as f64 - 0.5;
                        let plat = lat as f64 + fy * step as f64;
                        let plon = lon as f64 + fx * step as f64;
                        let Some(value) = reference(plat / 1e6, plon / 1e6) else { continue };
                        let surface_here = bilinear(&around, fy, fx);
                        top = top.max(value);
                        gap = gap.max(value - surface_here);
                    }
                }
                if top.is_finite() {
                    node_max[y * stride + x] = top;
                    over[y * stride + x] = gap;
                    any = true;
                }
            }
        }
        let mut plane_lifts = vec![0u8; stride * stride];
        if any {
            let core = select(&node_max, &over, stride);
            for y in 0..stride {
                for x in 0..stride {
                    if !dilated(&core, stride, y, x) {
                        continue;
                    }
                    let at = y * stride + x;
                    let native =
                        height(origin_y as i32 + (y as i64 * step) as i32, origin_x as i32 + (x as i64 * step) as i32);
                    if native == NODATA || !node_max[at].is_finite() {
                        continue;
                    }
                    let raw = (node_max[at] - f64::from(native)) / f64::from(CREST_QUANTUM);
                    let steps = raw.round().clamp(0.0, 255.0) as u8;
                    if steps == 0 {
                        continue;
                    }
                    plane_lifts[at] = steps;
                    if y < side && x < side {
                        bytes[layout.sample_offset(index, y as u32, x as u32)? as usize] = steps;
                        lifted += 1;
                    }
                }
            }
        }
        full.push(plane_lifts);
        sides.push(side);
    }
    (lifted > 0).then_some(CrestBlock { layout, bytes, full, sides, lifted })
}

/// The bilinear surface over the node's own cell, at a fractional offset in `[-1/2, 1/2]` of one
/// interval from the node. `around` is the 3x3 lattice centred on it, so the offset's sign picks
/// which of the four meeting intervals the point falls in and the rest is one interpolation.
fn bilinear(around: &[[i16; 3]; 3], fy: f64, fx: f64) -> f64 {
    let (qy, ty) = if fy >= 0.0 { (1usize, fy) } else { (0usize, fy + 1.0) };
    let (qx, tx) = if fx >= 0.0 { (1usize, fx) } else { (0usize, fx + 1.0) };
    let a = f64::from(around[qy][qx]);
    let b = f64::from(around[qy][qx + 1]);
    let c = f64::from(around[qy + 1][qx]);
    let d = f64::from(around[qy + 1][qx + 1]);
    a * (1.0 - tx) * (1.0 - ty) + b * tx * (1.0 - ty) + c * (1.0 - tx) * ty + d * tx * ty
}

/// Nodes the rule selects before dilation: the reference stands [`LIFT_M`] above our surface at a
/// point where its own cell maxima are convex by [`CONVEX_M`].
fn select(node_max: &[f64], over: &[f64], side: usize) -> Vec<bool> {
    let at = |y: usize, x: usize| node_max[y.min(side - 1) * side + x.min(side - 1)];
    let mut core = vec![false; side * side];
    for y in 0..side {
        for x in 0..side {
            let here = node_max[y * side + x];
            if !here.is_finite() || over[y * side + x] <= LIFT_M {
                continue;
            }
            let around = [at(y.saturating_sub(1), x), at(y + 1, x), at(y, x.saturating_sub(1)), at(y, x + 1)];
            if around.iter().any(|v| !v.is_finite()) {
                continue;
            }
            let mean = around.iter().sum::<f64>() / 4.0;
            core[y * side + x] = here - mean > CONVEX_M;
        }
    }
    core
}

/// Whether a node is selected or touches one, which is §9.4's one-sample extension.
fn dilated(core: &[bool], side: usize, y: usize, x: usize) -> bool {
    for dy in y.saturating_sub(1)..=(y + 1).min(side - 1) {
        for dx in x.saturating_sub(1)..=(x + 1).min(side - 1) {
            if core[dy * side + dx] {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cone on an otherwise planar slope: the rule must lift its tip and leave the plane alone.
    #[test]
    fn a_tower_is_lifted_and_the_plane_it_stands_on_is_not() {
        let posting = 9u8;
        let cell = 16u8;
        let step = (1i64 << posting) as f64;
        let origin_y = f64::from(GRID_ORIGIN);
        let origin_x = f64::from(GRID_ORIGIN);
        // A 40 degree planar slope, which a coarse lattice under-samples but must not inflate.
        let plane = |lat: f64, lon: f64| (lat - origin_y) / step * 40.0 + (lon - origin_x) / step * 8.0;
        let peak_lat = origin_y + 8.0 * step;
        let peak_lon = origin_x + 8.0 * step;
        let reference = |lat_deg: f64, lon_deg: f64| {
            let (lat, lon) = (lat_deg * 1e6, lon_deg * 1e6);
            let d = ((lat - peak_lat).powi(2) + (lon - peak_lon).powi(2)).sqrt() / step;
            Some(plane(lat, lon) + (120.0 - 120.0 * d).max(0.0))
        };
        let height = |lat: i32, lon: i32| plane(f64::from(lat), f64::from(lon)).round() as i16;
        let block = bake_cell(0, 0, posting, cell, height, reference).expect("the tower is a crest");

        let tip = block.lift(0, 8, 8);
        assert!((100..=130).contains(&tip), "the tip is lifted to the tower's own height, got {tip}");
        assert_eq!(block.lift(0, 1, 1), 0, "a planar slope four samples away is left alone");
        assert_eq!(block.lift(0, 14, 14), 0, "and so is one on the far side");
        assert!(block.lift(0, 8, 9) > 0, "the one-sample extension reaches the tip's neighbour");
        assert!(block.lifted() < 40, "the rule stays local to the tower, lifted {}", block.lifted());
        // Every level carries the tower, so a ridge crossing a level change does not step.
        assert!(block.lift(1, 4, 4) > 0, "the coarser level is lifted too");
    }

    #[test]
    fn a_reference_that_covers_nothing_produces_no_block() {
        let height = |_: i32, _: i32| 1000i16;
        assert!(bake_cell(0, 0, 9, 16, height, |_, _| None).is_none());
    }
}
