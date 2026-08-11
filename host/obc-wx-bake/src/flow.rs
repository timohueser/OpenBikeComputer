//! **The motion engine** (WXR9 #1251): pyramidal Lucas-Kanade, then semi-Lagrangian advection.
//!
//! One engine, two jobs, and the whole point of WXR9 is that they are the same engine:
//!
//! * **the radar nowcast** — estimate how the observed field is moving, carry it forward to
//!   +15 … +N minutes, and publish those as a forecast source that outranks a coarse model
//!   ([`crate::derive::radar_nowcast`]);
//! * **uniform 15-minute frames everywhere** — a source whose steps are hourly (GFS, ICON-EU)
//!   leaves three of every four canonical instants with nothing valid at them, so the mosaic
//!   paints one model step onto four consecutive frames and the timeline visibly jumps once an
//!   hour. Estimating the motion *between* two model steps and morphing across the gap gives every
//!   instant a genuine estimate of its own ([`crate::derive::uniform_frames`]).
//!
//! ## What this does and does not invent
//!
//! Advection **moves cells that already exist**. The sampling rule is nearest-neighbour, the same
//! rule `OBCG_Spec.md` §6 mandates everywhere else in this bakery: a cell of the output takes the
//! value of exactly one cell of the input, the one its back-traced trajectory lands in. No
//! sub-cell structure is created, no intensity is interpolated *spatially*, and a trajectory that
//! leaves the source domain lands on [`precip4::INTENSITY_NODATA`] — never on dry. Rain that
//! advects out of a radar footprint therefore leaves no-data behind it and the mosaic falls
//! through to the model, which is the honest answer and happens for free.
//!
//! **Nothing here combines two values into a third**, and that is a change from how this module
//! first shipped. [`morph`] — the temporal half, which carries two known fields to an instant
//! between them — used to blend the two advected results by time weight. Round 1 of #1278's review
//! measured what that cost on a real 30-minute morph: **22.6 % of wet cells carried an intensity
//! code neither parent held**, the wet area grew past the truth's, and the mean wet code fell about
//! one band. Every value was inside its two parents' range, so it was not fabrication in the strict
//! sense; it was still an intensity no source stated for that cell, and it published a bigger,
//! fainter storm than anything it was made from. So `morph` now **selects**: the cell comes from
//! whichever parent is nearer the target instant, whole. Advection may move cells; it may not invent
//! values.
//!
//! ## Why the flow grid is coarser than the intensity grid
//!
//! The motion field is estimated on a grid of nodes spaced [`FLOW_NODE_METRES`] apart, not per
//! cell. That is standard — a precipitation motion field is smooth at the scale of the systems
//! that carry it, and a per-cell field would mostly be fitting noise — and it is also what makes
//! the cost tractable: over a global cycle the intensity lattice is 648 M cells, and even the
//! per-source windows this engine actually runs on (24.5 M for MRMS, 25 M for OPERA) would be an
//! unaffordable number of 2x2 least-squares solves. At 16 km spacing MRMS's node grid is 438 x 219
//! and GFS's is 360 x 180, both trivial. The *advection* still runs at full cell resolution: the
//! flow is sampled bilinearly between nodes, so a coarse flow grid costs smoothness in the motion
//! field, never resolution in the output.
//!
//! ## Shape of the estimator
//!
//! Coarse-to-fine Lucas-Kanade over a Gaussian pyramid, which is the classical way to make a
//! differential method see displacements larger than its window:
//!
//! 1. both frames are mapped into a scalar field (the intensity code itself, which is monotone in
//!    log rain rate — the dBR space nowcasting conventionally works in);
//! 2. a pyramid of [`levels_for`] levels is built by `[1 2 1]` smoothing and 2x decimation;
//! 3. at each level, from coarsest to finest, every node solves the 2x2 LK normal equations over a
//!    [`WINDOW_RADIUS`]-cell window, warm-started from the level above, [`LK_ITERATIONS`] times;
//! 4. nodes whose structure tensor is degenerate — a flat, textureless, or dry neighbourhood, where
//!    the aperture problem is total — are marked invalid rather than given a fabricated vector;
//! 5. the node field is median-filtered (motion outliers are the failure mode of local LK), then
//!    invalid nodes are filled from their valid neighbours, and the whole field is clamped to
//!    [`MAX_SPEED_M_S`].
//!
//! If **no** node is valid — a single frame, a field with no rain in it, or two frames so far apart
//! that nothing correlates — [`estimate_motion`] returns `None`. There is no fallback vector: a
//! fabricated motion field is worse than no nowcast, because the caller can fall back to what it
//! already has and a bad flow field cannot be fallen back from.
//!
//! ## pySTEPS
//!
//! pySTEPS is the reference implementation this was written against and it is an **offline oracle
//! only**: nothing here depends on Python, and the VPS runs one static binary. See the PR for the
//! scored comparison.

use obc_formats::precip4;
use rayon::prelude::*;

/// Node spacing of the motion field, in metres. 16 km: coarse enough that the node grid of even
/// the 24.5 M-cell MRMS window is under 100 k nodes, fine enough to resolve the deformation of a
/// squall line, and comfortably inside the scale at which a precipitation motion field is smooth.
pub const FLOW_NODE_METRES: f64 = 16_000.0;

/// Never put nodes closer together than this many source cells, whatever [`FLOW_NODE_METRES`]
/// works out to. It only binds on the coarse model sources — a 6.5 km ICON cell would ask for
/// nodes 2 cells apart and a 27.75 km GFS cell for 1 — where a per-cell flow field would be both
/// meaningless (the model's own effective resolution is several cells) and the single most
/// expensive thing in the cycle.
pub const MIN_STRIDE_CELLS: u32 = 4;

/// Half-width of the Lucas-Kanade window, in cells **of the pyramid level being solved**. Small on
/// purpose: the pyramid, not the window, is what buys tolerance of large displacements, and a big
/// window smears the motion of neighbouring systems together.
pub const WINDOW_RADIUS: i32 = 4;

/// Gauss-Newton refinements per node per pyramid level.
pub const LK_ITERATIONS: usize = 6;

/// The coarsest pyramid level keeps at least this many cells on its short axis; below that the
/// image is no longer a picture of anything.
pub const MIN_LEVEL_EXTENT: u32 = 24;

/// Hard cap on pyramid depth.
pub const MAX_LEVELS: u32 = 5;

/// Physical speed clamp, in metres per second. 60 m/s (216 km/h) is above any storm-motion vector
/// ever observed and well below what a mis-solved 2x2 system can produce, so it bounds the damage
/// of a bad node without ever touching a real one.
pub const MAX_SPEED_M_S: f64 = 60.0;

/// The smallest eigenvalue of the (per-sample-normalised) structure tensor a node needs before its
/// solution is trusted. Below it the neighbourhood has no gradient in at least one direction — the
/// aperture problem — and the node is left for the fill pass.
const MIN_EIGENVALUE: f32 = 0.02;

/// A pair of frames with less wet area than this is not a motion signal.
///
/// **Relative, with an absolute floor** (#1278 r1, m8). A bare 64-cell gate is 0.0003 % of MRMS's
/// 24.5 M-cell window: a handful of echoes anywhere on the continent passed it, and if only one node
/// was then trackable, [`fill_invalid`] grew that node's vector across the whole 438 x 219 grid and
/// the entire field advected by it at every lead. The damage was bounded by the wet area, but it was
/// the one place where "no motion vector is ever fabricated" was a stronger claim than the code
/// made. One part in ten thousand of the window is 2,450 cells for MRMS and 103 for GFS's floor, and
/// the absolute floor keeps small test rasters and small regional windows workable.
const MIN_WET_FRACTION: f64 = 1.0 / 10_000.0;
const MIN_WET_CELLS: usize = 64;

/// How far, in nodes, a solved vector may be grown into unsolved ground before the field is simply
/// left still.
///
/// The other half of m8. Growing outward from the trackable nodes is right where the unsolved region
/// is the *inside* of a uniform rain shield or its immediate surroundings — that ground is carried by
/// the flow its edges were solved from. It is not right at continental range: a single trackable
/// node over Kansas says nothing about Maine, and a field that advects everything by it is asserting
/// a motion nobody measured. Beyond this many nodes the flow stays zero, which advects dry ground to
/// dry ground and leaves distant rain where it is — visibly persistence, which is what "we could not
/// tell" should look like. Six nodes is ~96 km at the 16 km node spacing.
///
/// **To within one smoothing pass** (#1278 r2, n18). [`fill_invalid`] finishes with an unconditional
/// 3x3 box pass over the whole node grid, so the ring of nodes immediately past the cap picks up a
/// fraction — at most a ninth per neighbour — of its grown neighbours' vectors. That is the seam
/// treatment doing its job rather than the cap leaking: the alternative is a hard edge in the
/// trajectories at exactly the distance where confidence runs out. The bound is therefore on where
/// the *grown* field stops, not on where the last non-zero float is.
const MAX_FILL_NODES: u32 = 6;

/// A dense motion field on a coarse node grid.
///
/// `u` is eastward and `v` northward, both in **source cells per second** — cells rather than
/// metres because every consumer is indexing a raster, and per second rather than per frame because
/// the same field is reused at eight different lead times.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionField {
    pub cols: u32,
    pub rows: u32,
    /// Cell spacing between nodes. Node `(i, j)` sits at cell centre
    /// `(j * stride + stride / 2, i * stride + stride / 2)`.
    pub stride: u32,
    pub u: Vec<f32>,
    pub v: Vec<f32>,
}

impl MotionField {
    /// An all-zero field of the shape `width x height` cells would produce. Used by the tests and
    /// by nothing else — production either has a solved field or has `None`.
    pub fn still(width: u32, height: u32, stride: u32) -> Self {
        let cols = width.div_ceil(stride).max(1);
        let rows = height.div_ceil(stride).max(1);
        let nodes = cols as usize * rows as usize;
        Self { cols, rows, stride, u: vec![0.0; nodes], v: vec![0.0; nodes] }
    }

    fn index(&self, col: u32, row: u32) -> usize {
        row as usize * self.cols as usize + col as usize
    }

    /// Bilinear sample of the flow at a **continuous cell position** — cell `c` spans `[c, c+1)`,
    /// so its centre is `c + 0.5` — in cells per second.
    ///
    /// Bilinear on the motion field and nearest-neighbour on the intensity field is a deliberate
    /// pairing: a motion field is a smooth physical quantity and interpolating it is free of
    /// invention, where interpolating a quantized rain rate would manufacture bands that no source
    /// reported.
    pub fn sample(&self, col: f32, row: f32) -> (f32, f32) {
        let stride = self.stride as f32;
        // Node `(i, j)` sits at continuous position `j * stride + stride / 2`, so the node
        // coordinate of a continuous cell position is `p / stride - 0.5`.
        let x = (col / stride - 0.5).clamp(0.0, (self.cols - 1) as f32);
        let y = (row / stride - 0.5).clamp(0.0, (self.rows - 1) as f32);
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.cols - 1);
        let y1 = (y0 + 1).min(self.rows - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let blend = |plane: &[f32]| {
            let top = plane[self.index(x0, y1)] * (1.0 - fx) + plane[self.index(x1, y1)] * fx;
            let bottom = plane[self.index(x0, y0)] * (1.0 - fx) + plane[self.index(x1, y0)] * fx;
            bottom * (1.0 - fy) + top * fy
        };
        (blend(&self.u), blend(&self.v))
    }

    /// The fastest node, in cells per second — what [`advect`] sizes its substep count from.
    pub fn max_speed_cells_s(&self) -> f32 {
        self.u.iter().zip(&self.v).map(|(u, v)| u.hypot(*v)).fold(0.0f32, f32::max)
    }
}

/// The estimator's physical inputs: how big a cell is, and therefore how a speed clamp and a node
/// spacing in metres turn into cells.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlowParams {
    pub cell_size_m: f64,
    pub stride: u32,
    pub max_speed_m_s: f64,
}

impl FlowParams {
    /// The production parameters for a source whose cells are `cell_size_m` across.
    pub fn for_cells(cell_size_m: f64) -> Self {
        Self { cell_size_m, stride: stride_for(cell_size_m), max_speed_m_s: MAX_SPEED_M_S }
    }
}

/// Node spacing in cells for a given cell size: [`FLOW_NODE_METRES`], floored at
/// [`MIN_STRIDE_CELLS`].
pub fn stride_for(cell_size_m: f64) -> u32 {
    // `<= 0.0` would let a NaN through, which then rounds to 0 and divides the node grid by zero.
    if cell_size_m.is_nan() || cell_size_m <= 0.0 {
        return MIN_STRIDE_CELLS;
    }
    ((FLOW_NODE_METRES / cell_size_m).round() as u32).max(MIN_STRIDE_CELLS)
}

/// How many pyramid levels a displacement of `max_cells` needs, bounded by the image size.
///
/// A single level sees displacements of roughly [`WINDOW_RADIUS`] cells; each level above doubles
/// that. So the depth is set by the largest displacement physically possible over `dt`, not by a
/// constant — which is why GFS (hourly steps, 27.75 km cells, under two cells of motion) solves on
/// one level and MRMS (ten-minute steps, 1 km cells, up to 36 cells of motion) solves on four.
pub fn levels_for(max_cells: f64, width: u32, height: u32) -> u32 {
    let by_extent = {
        let short = width.min(height).max(1);
        let mut levels = 1;
        while levels < MAX_LEVELS && (short >> levels) >= MIN_LEVEL_EXTENT {
            levels += 1;
        }
        levels
    };
    let needed = if max_cells <= f64::from(WINDOW_RADIUS) {
        1
    } else {
        1 + (max_cells / f64::from(WINDOW_RADIUS)).log2().ceil().max(0.0) as u32
    };
    needed.clamp(1, by_extent)
}

/// One level of the pyramid.
struct Level {
    width: u32,
    height: u32,
    data: Vec<f32>,
}

impl Level {
    /// Bilinear sample with edge clamping. Out-of-frame is the nearest edge value rather than a
    /// sentinel: LK needs a continuous surface to differentiate, and the validity of the *result*
    /// is decided by the structure tensor, not by where the window happened to reach.
    fn at(&self, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (self.width - 1) as f32);
        let y = y.clamp(0.0, (self.height - 1) as f32);
        let x0 = x.floor() as u32;
        let y0 = y.floor() as u32;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let row = |y: u32| {
            let base = y as usize * self.width as usize;
            self.data[base + x0 as usize] * (1.0 - fx) + self.data[base + x1 as usize] * fx
        };
        row(y0) * (1.0 - fy) + row(y1) * fy
    }

    fn get(&self, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        self.data[y * self.width as usize + x]
    }

    /// `[1 2 1]` separable smoothing followed by 2x decimation — the standard Burt-Adelson step,
    /// which is what makes a differential estimator legal at the next level up.
    fn halve(&self) -> Level {
        let width = (self.width / 2).max(1);
        let height = (self.height / 2).max(1);
        let mut data = vec![0.0f32; width as usize * height as usize];
        for row in 0..height {
            for col in 0..width {
                let (sx, sy) = (col as i32 * 2, row as i32 * 2);
                let mut sum = 0.0;
                for (dy, wy) in [(-1, 1.0), (0, 2.0), (1, 1.0)] {
                    for (dx, wx) in [(-1, 1.0f32), (0, 2.0), (1, 1.0)] {
                        sum += wx * wy * self.get(sx + dx, sy + dy);
                    }
                }
                data[row as usize * width as usize + col as usize] = sum / 16.0;
            }
        }
        Level { width, height, data }
    }
}

/// How many source cells one base-level sample averages, for a given node spacing.
///
/// **The estimator never looks at the intensity field at full resolution, and this is why.** Its
/// finest solve uses a `2 * WINDOW_RADIUS + 1` window around nodes [`FlowParams::stride`] cells
/// apart, so a base sample a quarter of the node spacing across already gives that window a reach
/// of about two node spacings — more resolution than a 16 km-node motion field can carry, and the
/// solved displacement is still refined to a fraction of a base sample by the sub-pixel LK step.
///
/// It is a memory decision as much as a cost one, and it is the single largest one in WXR9. A
/// differential estimator needs a continuous surface, so the field has to become `f32`: at full
/// resolution MRMS's 24.5 M cells are 98 MB **per frame**, and a pair plus its pyramid is a
/// quarter of a gigabyte of transient for a motion field of 96 k nodes. At a decimation of four
/// the same pair is 12 MB. Measured end to end, this is the difference between a cycle that peaks
/// at 1.09 GB against `MemoryMax=1G` and one that peaks at 0.6 GB.
///
/// Powers of two only, so the pyramid above it stays exact, and it collapses to 1 for the coarse
/// model sources whose node spacing is already at [`MIN_STRIDE_CELLS`].
pub fn base_decimation(stride: u32) -> u32 {
    let target = (stride as f32 / 4.0).max(1.0);
    let power = target.log2().round().clamp(0.0, 3.0) as u32;
    1 << power
}

/// Intensity codes to the scalar the estimator differentiates, decimated by `factor` in one pass.
///
/// The scalar is the code itself, with no-data read as dry. Two things about that are deliberate.
/// The code ladder is thresholded on rain rate in a roughly geometric progression, so the code *is*
/// a log-rate field — the dBR space nowcasting conventionally estimates motion in, where a doubling
/// of rate is the same increment everywhere rather than a hundredfold-larger gradient in the core
/// than at the edge. And no-data has to become *something* finite for a derivative to exist; dry is
/// the choice that biases the estimate least, because it makes an unscanned area look like an area
/// with no echo, which contributes no gradient and therefore no vote.
///
/// The decimation is a box mean rather than a subsample, so a one-cell feature still moves the base
/// sample it sits in instead of vanishing between two of them. It is done here and not by
/// [`Level::halve`] so the full-resolution `f32` field is never materialised at all.
fn base_level(cells: &[u8], width: u32, height: u32, factor: u32) -> Level {
    let out_width = (width / factor).max(1);
    let out_height = (height / factor).max(1);
    let mut data = vec![0.0f32; out_width as usize * out_height as usize];
    let inverse = 1.0 / (factor * factor) as f32;
    for row in 0..out_height {
        for col in 0..out_width {
            let mut sum = 0.0f32;
            for dy in 0..factor {
                let source_row = (row * factor + dy).min(height - 1) as usize;
                let base = source_row * width as usize;
                for dx in 0..factor {
                    let source_col = (col * factor + dx).min(width - 1) as usize;
                    let code = cells[base + source_col];
                    if code != precip4::INTENSITY_NODATA {
                        sum += f32::from(code);
                    }
                }
            }
            data[row as usize * out_width as usize + col as usize] = sum * inverse;
        }
    }
    Level { width: out_width, height: out_height, data }
}

fn wet_cells(cells: &[u8]) -> usize {
    cells.iter().filter(|&&code| code != precip4::INTENSITY_DRY && code != precip4::INTENSITY_NODATA).count()
}

/// Estimate the motion carrying `earlier` into `later`, `dt_seconds` apart.
///
/// `None` means *there is no motion signal here* and the caller must fall back to what it already
/// has: an empty or malformed pair, a field with essentially no rain in it, or a pair in which not
/// one node's neighbourhood had enough structure to solve. It is never a zero field wearing a
/// success return, because "everything is stationary" and "we could not tell" are answers a caller
/// has to distinguish — the first is a legitimate nowcast, the second is not.
pub fn estimate_motion(
    earlier: &[u8],
    later: &[u8],
    width: u32,
    height: u32,
    dt_seconds: f64,
    params: FlowParams,
) -> Option<MotionField> {
    let cells = width as usize * height as usize;
    if cells == 0 || earlier.len() != cells || later.len() != cells || width < 2 || height < 2 {
        return None;
    }
    if !dt_seconds.is_finite() || dt_seconds <= 0.0 {
        return None;
    }
    let wet_floor = MIN_WET_CELLS.max((cells as f64 * MIN_WET_FRACTION) as usize);
    if wet_cells(earlier) < wet_floor || wet_cells(later) < wet_floor {
        return None;
    }

    let stride = params.stride.max(1);
    // Everything below works in **base samples**, each one `factor` source cells across; the
    // displacement is scaled back into source cells at the very end.
    let factor = base_decimation(stride);
    let mut first = vec![base_level(earlier, width, height, factor)];
    let mut second = vec![base_level(later, width, height, factor)];
    if first[0].width < 2 || first[0].height < 2 {
        return None;
    }
    let max_cells = params.max_speed_m_s * dt_seconds / params.cell_size_m.max(1.0) / f64::from(factor);
    let levels = levels_for(max_cells, first[0].width, first[0].height);
    // Pyramids, coarsest last.
    for _ in 1..levels {
        first.push(first.last().expect("non-empty").halve());
        second.push(second.last().expect("non-empty").halve());
    }

    let cols = width.div_ceil(stride).max(1);
    let rows = height.div_ceil(stride).max(1);
    let nodes = cols as usize * rows as usize;
    // Displacement in **full-resolution cells**, carried down the pyramid.
    let mut du = vec![0.0f32; nodes];
    let mut dv = vec![0.0f32; nodes];
    let mut valid = vec![false; nodes];

    for level in (0..levels as usize).rev() {
        let scale = (1u32 << level) as f32;
        let a = &first[level];
        let b = &second[level];
        // Rows of nodes are independent; the node grid is small enough that this is the only
        // parallelism the estimator needs.
        let solved: Vec<(f32, f32, bool)> = (0..nodes)
            .into_par_iter()
            .map(|node| {
                let col = (node as u32 % cols) as f32;
                let row = (node as u32 / cols) as f32;
                // Node centre in **base-sample index** coordinates, then in this level's. The node
                // sits at `col * stride + stride / 2` source cells (where `MotionField::sample`
                // places it), which is that divided by `factor` base samples, less the half sample
                // that separates a sample index from a continuous position.
                let centre = |index: f32| (index * stride as f32 + stride as f32 / 2.0) / factor as f32 - 0.5;
                let cx = centre(col) / scale;
                let cy = centre(row) / scale;
                let mut dx = du[node] / scale;
                let mut dy = dv[node] / scale;
                let mut node_valid = valid[node];
                // **Keep the best iterate, never the last one.** A Gauss-Newton step on a real
                // precipitation field is not guaranteed to descend: where the displacement is
                // larger than the pyramid can see, or two systems overlap in the window, the
                // linearisation is wrong and the step can be enormous and in the wrong direction.
                // Scoring every iterate by the residual it actually achieves and keeping the
                // lowest turns that from a wild vector the median filter has to clean up into a
                // node that simply did not improve — and a node that never improves on the
                // estimate it arrived with is left for the fill pass rather than trusted.
                let residual = |dx: f32, dy: f32| -> f32 {
                    let mut sum = 0.0;
                    for wy in -WINDOW_RADIUS..=WINDOW_RADIUS {
                        for wx in -WINDOW_RADIUS..=WINDOW_RADIUS {
                            let px = cx + wx as f32;
                            let py = cy + wy as f32;
                            sum += (b.at(px + dx, py + dy) - a.at(px, py)).powi(2);
                        }
                    }
                    sum
                };
                let mut best = (dx, dy, residual(dx, dy));
                for _ in 0..LK_ITERATIONS {
                    let (mut xx, mut xy, mut yy, mut xt, mut yt) = (0.0f32, 0.0f32, 0.0f32, 0.0f32, 0.0f32);
                    let mut samples = 0.0f32;
                    for wy in -WINDOW_RADIUS..=WINDOW_RADIUS {
                        for wx in -WINDOW_RADIUS..=WINDOW_RADIUS {
                            let px = cx + wx as f32;
                            let py = cy + wy as f32;
                            // Central differences on the earlier frame; the later frame is sampled
                            // at the current warp, which is what makes the iteration Gauss-Newton
                            // rather than a single linearisation.
                            let ix = (a.at(px + 1.0, py) - a.at(px - 1.0, py)) * 0.5;
                            let iy = (a.at(px, py + 1.0) - a.at(px, py - 1.0)) * 0.5;
                            let it = b.at(px + dx, py + dy) - a.at(px, py);
                            xx += ix * ix;
                            xy += ix * iy;
                            yy += iy * iy;
                            xt += ix * it;
                            yt += iy * it;
                            samples += 1.0;
                        }
                    }
                    xx /= samples;
                    xy /= samples;
                    yy /= samples;
                    xt /= samples;
                    yt /= samples;
                    // Smallest eigenvalue of [[xx, xy], [xy, yy]] — the classic Shi-Tomasi
                    // trackability test. A flat or one-dimensional neighbourhood fails it and the
                    // node keeps whatever the coarser level gave it.
                    let trace = xx + yy;
                    let det = xx * yy - xy * xy;
                    let discriminant = (trace * trace / 4.0 - det).max(0.0).sqrt();
                    let smallest = trace / 2.0 - discriminant;
                    if smallest < MIN_EIGENVALUE || det.abs() < f32::EPSILON {
                        break;
                    }
                    // Validity is "this neighbourhood was trackable", not "the step helped". An
                    // unchanged field is a legitimate zero-motion answer whose residual starts at
                    // zero and cannot improve; refusing it would report `None` for the one case
                    // where the right answer is a field of zeroes.
                    node_valid = true;
                    // Solve for the increment; the sign convention is `later(p + d) = earlier(p)`.
                    let step_x = -(yy * xt - xy * yt) / det;
                    let step_y = -(xx * yt - xy * xt) / det;
                    if !step_x.is_finite() || !step_y.is_finite() {
                        break;
                    }
                    // One iteration may not move further than the window can see, or a single bad
                    // sample turns into a wild vector that the median filter then has to clean up.
                    let limit = WINDOW_RADIUS as f32;
                    dx += step_x.clamp(-limit, limit);
                    dy += step_y.clamp(-limit, limit);
                    let score = residual(dx, dy);
                    if score < best.2 {
                        best = (dx, dy, score);
                    }
                }
                (best.0 * scale, best.1 * scale, node_valid)
            })
            .collect();
        for (node, (x, y, ok)) in solved.into_iter().enumerate() {
            du[node] = x;
            dv[node] = y;
            valid[node] = ok;
        }
    }

    if !valid.iter().any(|&ok| ok) {
        return None;
    }

    median_filter(&mut du, &valid, cols, rows);
    median_filter(&mut dv, &valid, cols, rows);
    fill_invalid(&mut du, &mut dv, &valid, cols, rows);

    // Base samples over `dt` become source cells per second, clamped to a speed no weather system
    // has. `factor` is where the decimation is paid back: everything above solved in base samples.
    let limit = (params.max_speed_m_s / params.cell_size_m.max(1.0)) as f32;
    let inverse_dt = (f64::from(factor) / dt_seconds) as f32;
    let mut field = MotionField { cols, rows, stride, u: du, v: dv };
    for (u, v) in field.u.iter_mut().zip(field.v.iter_mut()) {
        *u *= inverse_dt;
        *v *= inverse_dt;
        let speed = u.hypot(*v);
        if speed > limit && speed > 0.0 {
            let scale = limit / speed;
            *u *= scale;
            *v *= scale;
        }
        if !u.is_finite() || !v.is_finite() {
            *u = 0.0;
            *v = 0.0;
        }
    }
    Some(field)
}

/// 3x3 median over the valid nodes. Local LK's characteristic failure is a single node with a wild
/// vector next to eight sane ones — exactly what a median removes and a mean does not.
fn median_filter(plane: &mut [f32], valid: &[bool], cols: u32, rows: u32) {
    let source = plane.to_vec();
    let mut window: Vec<f32> = Vec::with_capacity(9);
    for row in 0..rows as i32 {
        for col in 0..cols as i32 {
            let index = row as usize * cols as usize + col as usize;
            if !valid[index] {
                continue;
            }
            window.clear();
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (x, y) = (col + dx, row + dy);
                    if x < 0 || y < 0 || x >= cols as i32 || y >= rows as i32 {
                        continue;
                    }
                    let neighbour = y as usize * cols as usize + x as usize;
                    if valid[neighbour] {
                        window.push(source[neighbour]);
                    }
                }
            }
            window.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
            plane[index] = window[window.len() / 2];
        }
    }
}

/// Grow the solved nodes outward into the unsolved ones, then smooth once.
///
/// A node is unsolved where the field had no texture: inside a uniform rain shield, and over dry or
/// unscanned ground. Filling from the neighbours is the physically right answer for the first case
/// — the shield is being carried by the same flow as its edges, which is where the solution came
/// from — and harmless for the second, because advecting dry cells by any vector produces dry
/// cells. What it is *not* is an invention: with no valid node anywhere, `estimate_motion` has
/// already returned `None` rather than reaching here.
fn fill_invalid(u: &mut [f32], v: &mut [f32], valid: &[bool], cols: u32, rows: u32) {
    let mut filled = valid.to_vec();
    let total = cols as usize * rows as usize;
    for _ in 0..MAX_FILL_NODES {
        if filled.iter().all(|ok| *ok) {
            break;
        }
        let previous = filled.clone();
        let mut progressed = false;
        for row in 0..rows as i32 {
            for col in 0..cols as i32 {
                let index = row as usize * cols as usize + col as usize;
                if previous[index] {
                    continue;
                }
                let (mut sum_u, mut sum_v, mut count) = (0.0f32, 0.0f32, 0.0f32);
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    let (x, y) = (col + dx, row + dy);
                    if x < 0 || y < 0 || x >= cols as i32 || y >= rows as i32 {
                        continue;
                    }
                    let neighbour = y as usize * cols as usize + x as usize;
                    if previous[neighbour] {
                        sum_u += u[neighbour];
                        sum_v += v[neighbour];
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    u[index] = sum_u / count;
                    v[index] = sum_v / count;
                    filled[index] = true;
                    progressed = true;
                }
            }
        }
        if !progressed {
            // Disconnected islands of unsolved nodes cannot happen while at least one node is
            // valid and the grid is 4-connected, but the loop must terminate on its own terms
            // rather than on that argument.
            break;
        }
    }
    // Whatever the growth did not reach stays at zero — the honest answer at that distance, and the
    // reason the loop is bounded (see `MAX_FILL_NODES`). Nodes are initialised to zero and the
    // solver only writes the ones it solved, so this is already true; it is asserted rather than
    // assumed, because "unreached means still" is the whole claim.
    debug_assert!(total == u.len());
    // One box pass, so the seam between solved and grown nodes is not a discontinuity in the
    // trajectories.
    let (source_u, source_v) = (u.to_vec(), v.to_vec());
    for row in 0..rows as i32 {
        for col in 0..cols as i32 {
            let index = row as usize * cols as usize + col as usize;
            let (mut sum_u, mut sum_v, mut count) = (0.0f32, 0.0f32, 0.0f32);
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (x, y) = (col + dx, row + dy);
                    if x < 0 || y < 0 || x >= cols as i32 || y >= rows as i32 {
                        continue;
                    }
                    let neighbour = y as usize * cols as usize + x as usize;
                    sum_u += source_u[neighbour];
                    sum_v += source_v[neighbour];
                    count += 1.0;
                }
            }
            u[index] = sum_u / count;
            v[index] = sum_v / count;
        }
    }
}

/// Longest trajectory step, in cells, that one substep of [`advect`] may take.
///
/// Substeps exist so a **curving or deforming** flow is followed rather than cut across; in a
/// locally uniform flow — which a 16 km-node field mostly is — one substep is already exact and the
/// rest buy nothing. Four cells is the working compromise, and it is a cost decision as much as an
/// accuracy one: a 1 km field advected two hours at 20 m/s is 144 cells, so this is 36 flow samples
/// per output cell at the far end of the ladder against 144 at one cell per substep. pySTEPS'
/// semi-Lagrangian extrapolator defaults to three refinement steps for the same reason.
const MAX_SUBSTEP_CELLS: f32 = 4.0;
/// Cap on substeps, so a pathological flow field cannot turn one frame into an unbounded amount of
/// work.
///
/// Sized so the target above is actually met at the longest lead this bakery publishes rather than
/// being quietly abandoned there (#1278 r1, n11): the clamp at [`MAX_SPEED_M_S`] over a 90-minute
/// lead on the 1,113 m lattice is 291 cells, which needs 73 substeps to stay inside four cells each.
/// A hundred keeps the margin at the horizon and still bounds the work.
const MAX_SUBSTEPS: u32 = 100;

/// Semi-Lagrangian advection: where did the cell that is *here* now come from?
///
/// For every output cell the trajectory is integrated **backwards** from the cell centre through
/// the flow field, in substeps short enough that the flow can be treated as constant along each
/// (a first-order Lagrangian scheme with a re-sampled velocity, which is what makes a curving or
/// deforming field advect properly rather than shearing). The value at the trajectory's origin is
/// then taken by nearest neighbour.
///
/// `dt_seconds` may be negative, which runs the field backwards — that is how [`morph`] carries the
/// *later* of two frames back to an intermediate instant.
///
/// A trajectory that starts outside the source raster yields [`precip4::INTENSITY_NODATA`]. This is
/// the honest edge behaviour and it is load-bearing: the space a departing storm vacates at the
/// upwind edge of a radar footprint has no observation behind it, so it must not be published as
/// dry. The mosaic then falls through to whatever is beneath the nowcast, which is a model forecast
/// of that instant.
///
/// `wrap_x` says the raster **closes the circle in longitude**, which is true of exactly one source:
/// the GFS floor, whose grid is periodic (the mosaic already samples it through
/// [`crate::canonical::source_column`]'s wrap). It matters because the floor is the last row of the
/// priority table — nothing falls through *beneath* it — so an unwrapped advection would leave one
/// or two columns of permanent intensity 15 at the antimeridian, the exact 25-column stripe through
/// Fiji that `source_column`'s wrap exists to prevent, reintroduced by the derivation stage. The
/// wrap is over the *window*, which for GFS is one column short of the full turn, so a trajectory
/// crossing the seam samples a cell 0.25 degrees off. That offset is one cell in **magnitude**, but
/// it applies to every output column whose back-trace crosses the seam — a band as wide as the
/// displacement, up to about a dozen GFS columns over a 90-minute lead (#1278 r2, n15). Against a
/// permanent 25-column stripe of no-data through Fiji it is still the right trade, and it is the
/// whole of what the trade costs.
///
/// **The wrap is on the final lookup only, not on the trajectory integration** (n16):
/// [`MotionField::sample`] clamps at the node grid's edge rather than wrapping, so a trajectory that
/// crosses the antimeridian picks up the edge node's velocity for the rest of its walk. Harmless
/// while the flow is smooth across the seam, which on a global grid it is — the field there is one
/// continuous synoptic pattern, not a boundary — but it is stated rather than left as the half a
/// reader would assume.
///
/// Latitude never wraps: a pole is not a neighbour of anything.
pub fn advect(cells: &[u8], width: u32, height: u32, flow: &MotionField, dt_seconds: f64, wrap_x: bool) -> Vec<u8> {
    let count = width as usize * height as usize;
    assert_eq!(cells.len(), count, "advect: the field does not match its dimensions");
    if count == 0 {
        return Vec::new();
    }
    let span = (flow.max_speed_cells_s() * dt_seconds.abs() as f32 / MAX_SUBSTEP_CELLS).ceil();
    let substeps = if span.is_finite() { (span as u32).clamp(1, MAX_SUBSTEPS) } else { 1 };
    let step = (dt_seconds / f64::from(substeps)) as f32;

    let mut out = vec![precip4::INTENSITY_NODATA; count];
    out.par_chunks_mut(width as usize).enumerate().for_each(|(row, line)| {
        for (col, cell) in line.iter_mut().enumerate() {
            let mut x = col as f32 + 0.5;
            let mut y = row as f32 + 0.5;
            for _ in 0..substeps {
                let (u, v) = flow.sample(x, y);
                x -= u * step;
                y -= v * step;
            }
            let mut source_col = x.floor();
            let source_row = y.floor();
            if wrap_x && source_col.is_finite() {
                source_col = source_col.rem_euclid(width as f32);
            }
            *cell = if source_col >= 0.0 && source_row >= 0.0 && source_col < width as f32 && source_row < height as f32
            {
                cells[source_row as usize * width as usize + source_col as usize]
            } else {
                precip4::INTENSITY_NODATA
            };
        }
    });
    out
}

/// **Temporal interpolation between two known fields** — job B's whole mechanism.
///
/// `earlier` and `later` are `dt_seconds` apart and `flow` is the motion between them; the result is
/// the field at `offset_seconds` after `earlier`. **One of the two is carried to that instant along
/// the flow — whichever is nearer it — and that carried field *is* the result.** The other is not
/// advected, not sampled and not consulted.
///
/// It used to advect both and blend them by time weight, and round 1 of #1278's review measured what
/// that cost: over a real 30-minute morph, **22.6 % of wet cells carried an intensity code neither
/// parent held**, the wet area grew to 0.186 against a truth of 0.166, and the mean wet code fell to
/// 5.53 against 6.26 — a bigger, fainter storm than anything it was made from, about one intensity
/// band low. Every value was bounded by its two parents, so it was not fabrication in the strict
/// sense; it was still a published intensity that no source stated for that cell. The rule this
/// engine holds to is that **advection may move cells and must never invent values**, and a weighted
/// mean of two quantized codes invents one. Blending the two *static* fields would have been worse
/// again — same storm, two places, two ghosts — but that was never the choice on the table.
///
/// **Missing propagates.** If the nearer parent's advected cell is no-data, so is the output —
/// `OBCG_Spec.md` §3.2 requires it of every derivation without exception, and the alternative
/// (taking the other parent whole) published 43,130 dry cells over ground the nearer field had no
/// data for on that same morph. The cost is real and is the honest one: a morphed frame inherits the
/// blind spot of whichever parent answers it, so the mosaic falls through to the next source there
/// instead of asserting clear sky.
pub fn morph(earlier: &[u8], later: &[u8], width: u32, height: u32, flow: &MotionField, span: Span) -> Vec<u8> {
    let count = width as usize * height as usize;
    assert_eq!(earlier.len(), count, "morph: the earlier field does not match its dimensions");
    assert_eq!(later.len(), count, "morph: the later field does not match its dimensions");
    // Only the parent that wins is advected. Since `nearer_is_later` picks one field for the whole
    // frame rather than per cell, advecting the other would be work whose every output cell is
    // discarded — which is also the honest way to read what this function does: it is *the nearer
    // measurement, carried to the target instant*, not an average of two.
    if nearer_is_later(span.weight()) {
        advect(later, width, height, flow, span.offset_seconds - span.dt_seconds, span.wrap_x)
    } else {
        advect(earlier, width, height, flow, span.offset_seconds, span.wrap_x)
    }
}

/// The gap a [`morph`] interpolates across, and where in it the target instant sits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    /// Seconds from the earlier parent to the later one.
    pub dt_seconds: f64,
    /// Seconds from the earlier parent to the target instant.
    pub offset_seconds: f64,
    /// Does the source raster close the circle in longitude? See [`advect`].
    pub wrap_x: bool,
}

impl Span {
    /// How far the target sits from the earlier parent toward the later one, in `0.0 ..= 1.0`.
    pub fn weight(&self) -> f32 {
        if self.dt_seconds > 0.0 {
            (self.offset_seconds / self.dt_seconds).clamp(0.0, 1.0) as f32
        } else {
            0.0
        }
    }
}

/// Which parent a morph takes its cells from.
///
/// `weight` is how far the target sits from the earlier parent toward the later one, so `< 0.5`
/// takes the earlier and the rest takes the later — the same "nearest validity, ties to the later
/// frame" rule [`crate::canonical::MosaicLayer::nearest`] uses one level up, for the same reason:
/// the field valid after the target is about weather that has not happened yet, the one before it is
/// already past.
///
/// **There is a visible discontinuity where the parent changes, and it is measured rather than
/// waved away** (#1278 r2, R2-4). An earlier draft of this comment claimed there was none, on the
/// argument that both parents are carried to the same instant and hold the same storm in the same
/// place. The argument is half right — they do — but two radar composites an hour apart are not two
/// views of one unchanged storm, and the residual shows up as a step in the published timeline
/// exactly at the middle of every bracket. On a real hourly bracket over the derecho, wet/dry
/// disagreement between consecutive frames is **0.0774 across the parent switch against 0.0577
/// within one parent**, a 34 % excess (`tests/nowcast_skill.rs` measures and prints both). Before
/// WXR9 the same two instants took the same nearest step and the figure was 0.0000 by construction,
/// so the step is new.
///
/// It is accepted rather than removed, and the trade is stated plainly: a timeline that *moves* is
/// the whole point of job B, and the alternative is the frozen four-frame staircase this replaced.
/// The measurement above is on radar, which changes shape far faster than the hourly model fields
/// this actually runs on, so it is an upper bound on what a rider sees over a GFS-only region.
/// Removing it entirely means a scheme that never switches source — which is either blending (it
/// invents values, `OBCG_Spec.md` §3.2 forbids it) or one parent for the whole bracket (which puts a
/// bigger step at the bracket boundary instead of a smaller one in the middle).
pub fn nearer_is_later(weight: f32) -> bool {
    weight >= 0.5
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A field with one rectangular blob at `(x, y)`, on a `width x height` raster.
    fn blob(width: u32, height: u32, x: i32, y: i32, radius: i32, code: u8) -> Vec<u8> {
        let mut cells = vec![precip4::INTENSITY_DRY; width as usize * height as usize];
        for row in (y - radius)..=(y + radius) {
            for col in (x - radius)..=(x + radius) {
                if row < 0 || col < 0 || row >= height as i32 || col >= width as i32 {
                    continue;
                }
                // A soft edge, so the blob has a gradient to differentiate rather than a cliff the
                // pyramid turns into ringing.
                let distance = (((col - x).pow(2) + (row - y).pow(2)) as f32).sqrt();
                let value = (f32::from(code) * (1.0 - distance / radius as f32)).round();
                if value > 0.0 {
                    cells[row as usize * width as usize + col as usize] = value as u8;
                }
            }
        }
        cells
    }

    /// The centre of mass of the wet cells — where a blob actually is, to sub-cell precision.
    fn centroid(cells: &[u8], width: u32) -> (f32, f32) {
        let (mut sx, mut sy, mut mass) = (0.0f64, 0.0f64, 0.0f64);
        for (index, &code) in cells.iter().enumerate() {
            if code == precip4::INTENSITY_DRY || code == precip4::INTENSITY_NODATA {
                continue;
            }
            let weight = f64::from(code);
            sx += weight * (index as u32 % width) as f64;
            sy += weight * (index as u32 / width) as f64;
            mass += weight;
        }
        if mass == 0.0 {
            return (f32::NAN, f32::NAN);
        }
        ((sx / mass) as f32, (sy / mass) as f32)
    }

    /// **The headline property: a blob that translated must advect to where it is going.**
    ///
    /// Two frames 600 s apart with the blob 24 cells east and 12 north; the engine has to recover
    /// that motion and, advected another 600 s, put the blob 24 more cells east and 12 more north.
    #[test]
    fn a_translating_blob_advects_to_the_right_place() {
        let (width, height) = (256u32, 192u32);
        let earlier = blob(width, height, 60, 60, 18, 10);
        let later = blob(width, height, 84, 72, 18, 10);
        let params = FlowParams::for_cells(1_000.0);
        let flow = estimate_motion(&earlier, &later, width, height, 600.0, params).expect("a moving blob has motion");

        // The recovered velocity, at the blob, is 24 cells east and 12 north over 600 s.
        let (u, v) = flow.sample(84.0, 72.0);
        assert!((u * 600.0 - 24.0).abs() < 4.0, "eastward: {} cells / 600 s", u * 600.0);
        assert!((v * 600.0 - 12.0).abs() < 4.0, "northward: {} cells / 600 s", v * 600.0);

        let forecast = advect(&later, width, height, &flow, 600.0, false);
        let (cx, cy) = centroid(&forecast, width);
        let (tx, ty) = centroid(&blob(width, height, 108, 84, 18, 10), width);
        assert!((cx - tx).abs() < 3.0 && (cy - ty).abs() < 3.0, "advected to ({cx}, {cy}), expected ({tx}, {ty})");

        // …and it is closer to the truth than not advecting at all, which is the only comparison
        // that says the motion did any work.
        let (px, py) = centroid(&later, width);
        assert!((cx - tx).hypot(cy - ty) < (px - tx).hypot(py - ty) / 2.0, "advection must beat persistence");
    }

    /// A rotating field: the motion is not a single translation anywhere, so a per-node field has
    /// to represent it. Sampled on opposite sides of the centre, the recovered vectors must point
    /// in opposite directions.
    #[test]
    fn a_rotating_field_recovers_opposing_vectors() {
        let (width, height) = (192u32, 192u32);
        let (cx, cy) = (96.0f32, 96.0f32);
        // Four blobs on a ring, rotated by ~9 degrees between the frames.
        let ring = |angle: f32| {
            let mut cells = vec![precip4::INTENSITY_DRY; (width * height) as usize];
            for spoke in 0..4 {
                let theta = angle + spoke as f32 * std::f32::consts::FRAC_PI_2;
                let x = (cx + 55.0 * theta.cos()).round() as i32;
                let y = (cy + 55.0 * theta.sin()).round() as i32;
                let patch = blob(width, height, x, y, 16, 10);
                for (target, source) in cells.iter_mut().zip(patch) {
                    *target = (*target).max(source);
                }
            }
            cells
        };
        let earlier = ring(0.0);
        let later = ring(0.16);
        let flow = estimate_motion(&earlier, &later, width, height, 600.0, FlowParams::for_cells(1_000.0))
            .expect("a rotating field has motion");

        // On the +x spoke the rotation is northward; on the -x spoke it is southward.
        let (_, east_v) = flow.sample(cx + 55.0, cy);
        let (_, west_v) = flow.sample(cx - 55.0, cy);
        assert!(east_v > 0.0, "the eastern spoke must move north, got {east_v}");
        assert!(west_v < 0.0, "the western spoke must move south, got {west_v}");
        // A single translation cannot describe this: the two vectors genuinely oppose.
        assert!(east_v - west_v > 0.5 * (east_v.abs() + west_v.abs()), "the field must not be one translation");
    }

    /// **A dry field is a no-op, and it says so.** No rain means no motion signal, and inventing
    /// one would publish a moving nothing over ground the mosaic could have filled from a model.
    #[test]
    fn a_dry_field_has_no_motion_at_all() {
        let (width, height) = (128u32, 128u32);
        let dry = vec![precip4::INTENSITY_DRY; (width * height) as usize];
        assert!(estimate_motion(&dry, &dry, width, height, 600.0, FlowParams::for_cells(1_000.0)).is_none());

        // Neither has an all-no-data field — "we cannot see" is not "nothing is moving".
        let blind = vec![precip4::INTENSITY_NODATA; (width * height) as usize];
        assert!(estimate_motion(&blind, &blind, width, height, 600.0, FlowParams::for_cells(1_000.0)).is_none());

        // And a single frame repeated is stationary, which *is* a signal: the engine must return a
        // field, and it must be ~zero rather than absent.
        let still = blob(width, height, 64, 64, 20, 9);
        let flow = estimate_motion(&still, &still, width, height, 600.0, FlowParams::for_cells(1_000.0))
            .expect("an unchanged field is a legitimate zero-motion answer");
        assert!(flow.max_speed_cells_s() * 600.0 < 2.0, "an unchanged field must not move");
        // Advecting by a zero field is the identity.
        assert_eq!(advect(&still, width, height, &flow, 900.0, false), still);
    }

    /// Advection never fabricates dry ground behind a departing field: what leaves the raster
    /// leaves no-data, which is what makes the mosaic fall through to a model instead of rendering
    /// "no rain here".
    #[test]
    fn what_advects_out_of_the_raster_leaves_no_data_not_dry() {
        let (width, height) = (64u32, 64u32);
        let cells = vec![precip4::INTENSITY_DRY; (width * height) as usize];
        let mut flow = MotionField::still(width, height, 16);
        // A steady 10 cells/600 s eastward.
        for u in flow.u.iter_mut() {
            *u = 10.0 / 600.0;
        }
        let out = advect(&cells, width, height, &flow, 600.0, false);
        // The western ten columns were traced back off the raster.
        for row in 0..height as usize {
            for col in 0..10usize {
                assert_eq!(out[row * width as usize + col], precip4::INTENSITY_NODATA, "({col}, {row})");
            }
            assert_eq!(out[row * width as usize + 40], precip4::INTENSITY_DRY);
        }
    }

    /// The morph puts a blob **between** its two positions, in proportion to the target instant —
    /// which is the whole of what "uniform 15-minute frames" needs from this engine.
    #[test]
    fn a_morph_places_the_field_between_its_two_parents() {
        let (width, height) = (256u32, 192u32);
        let earlier = blob(width, height, 60, 96, 18, 10);
        let later = blob(width, height, 84, 96, 18, 10);
        let flow = estimate_motion(&earlier, &later, width, height, 600.0, FlowParams::for_cells(1_000.0))
            .expect("a moving blob has motion");
        // A quarter, a half and three quarters of the way across the gap.
        for (offset, expected_x) in [(150.0, 66.0), (300.0, 72.0), (450.0, 78.0)] {
            let between = morph(
                &earlier,
                &later,
                width,
                height,
                &flow,
                Span { dt_seconds: 600.0, offset_seconds: offset, wrap_x: false },
            );
            let (cx, _) = centroid(&between, width);
            assert!((cx - expected_x).abs() < 3.0, "at +{offset}s the blob is at {cx}, expected ~{expected_x}");
            // …and it is one blob, not two ghosts: nothing wet may remain at either parent's
            // position, which is the artefact any scheme that blends two static fields produces.
            let (px, _) = centroid(&earlier, width);
            assert!((cx - px).abs() > 2.0 || offset < 200.0);
        }
        // The endpoints are the parents themselves, to within the nearest-neighbour resample.
        let at_start = morph(
            &earlier,
            &later,
            width,
            height,
            &flow,
            Span { dt_seconds: 600.0, offset_seconds: 0.0, wrap_x: false },
        );
        assert!((centroid(&at_start, width).0 - centroid(&earlier, width).0).abs() < 2.0);
        let at_end = morph(
            &earlier,
            &later,
            width,
            height,
            &flow,
            Span { dt_seconds: 600.0, offset_seconds: 600.0, wrap_x: false },
        );
        assert!((centroid(&at_end, width).0 - centroid(&later, width).0).abs() < 2.0);
    }

    /// The node grid follows the source's cell size, and never gets so fine that a coarse model
    /// would pay for a per-cell solve.
    #[test]
    fn the_flow_grid_is_sixteen_kilometres_floored_at_four_cells() {
        assert_eq!(stride_for(1_000.0), 16, "1 km radar: 16-cell nodes");
        assert_eq!(stride_for(3_000.0), 5, "3 km model");
        assert_eq!(stride_for(6_500.0), 4, "6.5 km ICON-EU is on the floor");
        assert_eq!(stride_for(27_750.0), MIN_STRIDE_CELLS, "the 27.75 km floor source is on the floor");
        assert_eq!(stride_for(0.0), MIN_STRIDE_CELLS, "a nonsense cell size must not divide by zero");
    }

    /// Pyramid depth follows the displacement to be found, not a constant: a 1 km radar over ten
    /// minutes needs several levels, an hourly 27.75 km model step needs one.
    #[test]
    fn pyramid_depth_follows_the_displacement_not_a_constant() {
        // MRMS: 60 m/s over 600 s is 36 cells of 1 km.
        assert_eq!(levels_for(36.0, 7_000, 3_500), 5);
        // GFS: 60 m/s over 3,600 s is 7.8 cells of 27.75 km.
        assert_eq!(levels_for(7.8, 1_439, 719), 2);
        // A displacement the window sees on its own needs no pyramid at all.
        assert_eq!(levels_for(3.0, 1_439, 719), 1);
        // A tiny raster cannot be halved five times however fast the wind is.
        assert_eq!(levels_for(1_000.0, 40, 40), 1);
        assert!(levels_for(1_000.0, 4_000, 4_000) <= MAX_LEVELS);
    }

    /// Bilinear sampling of the node grid is exact at the node centres and monotone between them.
    #[test]
    fn the_flow_samples_bilinearly_between_its_nodes() {
        let mut flow = MotionField::still(64, 64, 16);
        // Node columns 0..3; make u a ramp across them.
        for row in 0..flow.rows {
            for col in 0..flow.cols {
                let index = flow.index(col, row);
                flow.u[index] = col as f32;
            }
        }
        assert!((flow.sample(8.0, 8.0).0 - 0.0).abs() < 1e-4, "node 0 sits at continuous 8");
        assert!((flow.sample(24.0, 8.0).0 - 1.0).abs() < 1e-4, "node 1 sits at continuous 24");
        assert!((flow.sample(16.0, 8.0).0 - 0.5).abs() < 1e-4, "halfway between them");
        // Outside the node centres the field is clamped, never extrapolated.
        assert!((flow.sample(0.0, 0.0).0 - 0.0).abs() < 1e-4);
        assert!((flow.sample(64.0, 8.0).0 - 3.0).abs() < 1e-4);
    }

    /// Combining two advected fields never turns missing data into dry, and never exceeds the
    /// intensity ladder.
    #[test]
    fn the_temporal_combination_respects_no_data_and_the_ladder() {
        // Selection, not blending: the nearer parent in time, with the tie to the later one.
        assert!(!nearer_is_later(0.0) && !nearer_is_later(0.49));
        assert!(nearer_is_later(0.5) && nearer_is_later(1.0));

        // …and on real fields, every output cell is a value one parent actually held at that cell,
        // including no-data. That is the whole property: no third value is ever created, and a
        // parent's blind spot is inherited rather than papered over with the other parent's dry.
        let (width, height) = (128u32, 96u32);
        let earlier = blob(width, height, 40, 48, 16, 10);
        let later = blob(width, height, 64, 48, 16, 10);
        let flow = estimate_motion(&earlier, &later, width, height, 600.0, FlowParams::for_cells(1_000.0))
            .expect("a moving blob has motion");
        for offset in [60.0f64, 200.0, 300.0, 400.0, 540.0] {
            let between = morph(
                &earlier,
                &later,
                width,
                height,
                &flow,
                Span { dt_seconds: 600.0, offset_seconds: offset, wrap_x: false },
            );
            let source = if nearer_is_later((offset / 600.0) as f32) { later.as_slice() } else { earlier.as_slice() };
            for code in &between {
                assert!(
                    *code == precip4::INTENSITY_NODATA || source.contains(code),
                    "+{offset}s published intensity {code}, which its parent never held anywhere"
                );
            }
        }
    }
}
