//! Crest lifts (`OBCT_Spec.md` §9): where a finer reference DEM says our lattice loses a summit.
//!
//! A `2^9` posting cannot hold a rock tower. Against 2 m swissALTI3D at Engelberg the bilinear
//! surface through Copernicus GLO-30 runs 100 m below the Hahnen's top, and a panorama drawn from
//! it loses the one feature that makes the mountain recognisable. §1.2 lets the producer take the
//! sample at such a node from the reference instead, and this is that rule.
//!
//! The lift is added to the **native** height before the cell is baked, so §8's pyramid, maxima,
//! error codes and cross-cell index all describe one surface. Peak View, the contours, the ascent
//! integral, the route profile and the altimeter therefore read the same heights, and nothing
//! downstream carries a second plane.
//!
//! The rule is two tests and one dilation, and each part earns its place:
//!
//! * **[`LIFT_M`] against the bilinear surface, not the node.** The interesting quantity is how far
//!   the reference stands above the surface we actually draw, measured where it stands there.
//! * **[`CONVEX_M`] on the reference's own node maxima.** Without it, a steep planar face reads as
//!   a crest — a coarse lattice under-samples a 40° slope honestly — and the whole mountain
//!   inflates. Measured against three photographs, the ungated rule pushed the drawn skyline half a
//!   degree high; the gate brings the median back to zero. It is also what leaves a saddle alone,
//!   so a pass does not move.
//! * **One node of dilation.** A lifted node beside unlifted neighbours makes the bilinear surface
//!   sawtooth, which the panorama shows as a jittering skyline. Extending the selection by one node
//!   lifts a crest along its whole length instead of at scattered points, and it also *improves*
//!   accuracy: on the Rigidalstock it took the root-mean-square skyline error from 0.32° to 0.24°,
//!   which is the 2 m reference's own error against the same photograph.
//!
//! Every part of the rule reads only a node's **2-ring**, and [`LiftMap::bake`] scans that ring
//! beyond the cell it is asked for ([`HALO`]). A node on a cell seam therefore gets the same lift
//! whichever of the two cells computes it, which is what keeps one published cell byte-identical to
//! the same square inside a wide shard.

use obc_formats::obct::{cell_samples_log2, GRID_ORIGIN, NODATA};

/// A node is a candidate when the reference stands this far above our bilinear surface.
pub const LIFT_M: f64 = 10.0;
/// …and only where the reference's node maxima are locally convex by this much.
pub const CONVEX_M: f64 = 3.0;
/// Sub-samples per node axis when scanning a node's half-posting cell for its reference maximum.
/// 32 puts the step below 2 m at the v1 posting of `2^9` µdeg, so a metre-scale tower cannot hide
/// between them. A bake at a much coarser posting would need more, at the square of the cost.
const SUBSAMPLES: u32 = 32;
/// Nodes the scan reaches beyond the map's own range, which is the 2-ring the rule reads: the
/// convexity test needs a node's four neighbours, and the dilation needs their own selection.
const HALO: i64 = 2;
/// A lift past this is worth an operator's attention. There is no ceiling on a lift — where the
/// source lost a rock wall, the reference is the better measurement and a clamp would put the error
/// back — but a reference with a spike in it looks exactly like a cliff, and the two have to be
/// told apart by someone. 200 m is twice the error the rule was built to correct.
pub const REPORT_M: i16 = 200;

/// What the rule did over a run, for the operator's summary. A reference with a spike in it shows
/// up here rather than in a drawn panorama.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LiftTally {
    /// Nodes lifted at all.
    pub nodes: u64,
    /// The largest lift, in whole metres, and the node it is at, in µdeg.
    pub max_m: i16,
    pub max_at: (i32, i32),
    /// Nodes lifted by more than [`REPORT_M`].
    pub over_report: u64,
}

impl LiftTally {
    /// Two tallies as one: the counts add, and the larger maximum keeps its position.
    pub fn join(self, other: Self) -> Self {
        let (max_m, max_at) =
            if other.max_m > self.max_m { (other.max_m, other.max_at) } else { (self.max_m, self.max_at) };
        Self { nodes: self.nodes + other.nodes, over_report: self.over_report + other.over_report, max_m, max_at }
    }
}

/// One cell's lifts in whole metres, over the nodes it owns and its inclusive high edge.
///
/// Native level only. The coarser §8.1 levels select native posts, so a pyramid baked through
/// [`apply`](Self::apply) carries the lift at every level without a plane of its own.
pub struct LiftMap {
    /// Lattice coordinate of the node at index `(0, 0)`, in µdeg.
    origin_y: i64,
    origin_x: i64,
    /// Native posting in µdeg — the spacing of every index in this map.
    step: i64,
    /// Nodes per axis: the cell's own side, plus its inclusive high edge.
    stride: usize,
    /// Lift in whole metres per node, row-major, never negative.
    lifts: Vec<i16>,
    tally: LiftTally,
}

impl LiftMap {
    /// Build one cell's lift map, or `None` when the rule selects no node in it — which is the
    /// common case, since national reference coverage stops at borders.
    ///
    /// `native` is the unlifted lattice, sampled in µdeg: the gap is measured against the bilinear
    /// surface through it. `reference` is the finer DEM in the same geographic frame, answering
    /// `None` outside its coverage, which may stop at any node.
    pub fn bake(
        ci: u32,
        cj: u32,
        posting_log2: u8,
        cell_log2: u8,
        mut native: impl FnMut(i32, i32) -> i16,
        reference: &dyn Fn(f64, f64) -> Option<f64>,
    ) -> Option<LiftMap> {
        let side = 1i64 << cell_samples_log2(posting_log2, cell_log2)?;
        let step = 1i64 << posting_log2;
        let origin_y = i64::from(GRID_ORIGIN) + (i64::from(ci) << cell_log2);
        let origin_x = i64::from(GRID_ORIGIN) + (i64::from(cj) << cell_log2);
        let scan = Scan::new(side, HALO);

        // The reference maximum inside each node's own half-posting cell, and how far that maximum
        // stands above the bilinear surface we would otherwise draw there.
        let mut node_max = vec![f64::NEG_INFINITY; scan.nodes()];
        let mut over = vec![f64::NEG_INFINITY; scan.nodes()];
        let mut any = false;
        for y in scan.axis() {
            for x in scan.axis() {
                let lat = origin_y + y * step;
                let lon = origin_x + x * step;
                // One cheap probe rules out the whole node outside coverage, which is most of a
                // 58 × 40 km cell whenever the reference is a national dataset.
                if reference(lat as f64 / 1e6, lon as f64 / 1e6).is_none() {
                    continue;
                }
                // The node's own cell spans the four intervals that meet at it, so the surface over
                // that cell is defined by the 3×3 lattice around it.
                let mut around = [[0i16; 3]; 3];
                for (dy, row) in around.iter_mut().enumerate() {
                    for (dx, corner) in row.iter_mut().enumerate() {
                        *corner = native((lat + (dy as i64 - 1) * step) as i32, (lon + (dx as i64 - 1) * step) as i32);
                    }
                }
                // No bilinear surface to measure against, so no gap and no lift. A hole in the
                // native lattice therefore stays a hole, and its neighbours are left alone.
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
                        top = top.max(value);
                        gap = gap.max(value - bilinear(&around, fy, fx));
                    }
                }
                if top.is_finite() {
                    node_max[scan.at(y, x)] = top;
                    over[scan.at(y, x)] = gap;
                    any = true;
                }
            }
        }
        if !any {
            return None;
        }

        let core = select(&node_max, &over, &scan);
        let stride = (side + 1) as usize;
        let mut lifts = vec![0i16; stride * stride];
        let mut tally = LiftTally::default();
        for y in 0..=side {
            for x in 0..=side {
                let top = node_max[scan.at(y, x)];
                let (lat, lon) = ((origin_y + y * step) as i32, (origin_x + x * step) as i32);
                let here = native(lat, lon);
                if here == NODATA || !top.is_finite() || !dilated(&core, &scan, y, x) {
                    continue;
                }
                // Whole metres, because the sample it lands in is whole metres (§1.2). Never
                // negative: the reference may sit below our surface in a hollow, and §9 raises
                // crests rather than editing the lattice wherever the two disagree.
                let lift = (top.round() - f64::from(here)).clamp(0.0, f64::from(i16::MAX)) as i16;
                if lift == 0 {
                    continue;
                }
                lifts[y as usize * stride + x as usize] = lift;
                // The tally counts the nodes this cell *owns*, not its inclusive high edge: the
                // edge is the next cell's node 0 and is tallied there, so a run's count is one per
                // written sample rather than two per seam.
                if y == side || x == side {
                    continue;
                }
                tally.nodes += 1;
                tally.over_report += u64::from(lift > REPORT_M);
                if lift > tally.max_m {
                    (tally.max_m, tally.max_at) = (lift, (lat, lon));
                }
            }
        }
        (tally.nodes > 0).then_some(LiftMap { origin_y, origin_x, step, stride, lifts, tally })
    }

    /// The native sampler with this map's lifts added. `NODATA` passes through — a lift describes a
    /// height and there is none at a hole — and the addition saturates rather than wrapping.
    pub fn apply<'a>(&'a self, mut native: impl FnMut(i32, i32) -> i16 + 'a) -> impl FnMut(i32, i32) -> i16 + 'a {
        move |lat, lon| {
            let height = native(lat, lon);
            if height == NODATA {
                height
            } else {
                height.saturating_add(self.at(lat, lon))
            }
        }
    }

    /// Metres this node is lifted by; zero outside the map, and at a coordinate off its lattice.
    pub fn at(&self, lat: i32, lon: i32) -> i16 {
        let node = |value: i32, origin: i64| -> Option<usize> {
            let delta = i64::from(value) - origin;
            (delta % self.step == 0).then_some(())?;
            usize::try_from(delta / self.step).ok().filter(|&n| n < self.stride)
        };
        let (Some(y), Some(x)) = (node(lat, self.origin_y), node(lon, self.origin_x)) else { return 0 };
        self.lifts[y * self.stride + x]
    }

    /// What the rule did in this cell.
    pub fn tally(&self) -> LiftTally {
        self.tally
    }
}

/// The square of node coordinates a bake scans: `-halo ..= side + halo` on both axes, which is the
/// map's own range widened by the ring every test reads.
struct Scan {
    low: i64,
    high: i64,
}

impl Scan {
    fn new(side: i64, halo: i64) -> Self {
        Self { low: -halo, high: side + halo }
    }

    fn axis(&self) -> core::ops::RangeInclusive<i64> {
        self.low..=self.high
    }

    fn nodes(&self) -> usize {
        let side = (self.high - self.low + 1) as usize;
        side * side
    }

    /// Flat index of a node coordinate. Callers stay inside [`axis`](Self::axis).
    fn at(&self, y: i64, x: i64) -> usize {
        let side = (self.high - self.low + 1) as usize;
        (y - self.low) as usize * side + (x - self.low) as usize
    }

    /// Whether the scan holds this node with `margin` nodes to spare on every side.
    fn holds(&self, y: i64, x: i64, margin: i64) -> bool {
        let inner = self.low + margin..=self.high - margin;
        inner.contains(&y) && inner.contains(&x)
    }
}

/// The bilinear surface over the node's own cell, at a fractional offset in `[-1/2, 1/2]` of one
/// interval from the node. `around` is the 3×3 lattice centred on it, so the offset's sign picks
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
/// node whose own four neighbours it is convex over by [`CONVEX_M`].
///
/// A node the reference misses at any of those five places is left unselected. The test has no
/// answer there, and inventing one — by clamping to the node itself, say — would make the lift
/// depend on which cell asked for it.
fn select(node_max: &[f64], over: &[f64], scan: &Scan) -> Vec<bool> {
    let mut core = vec![false; scan.nodes()];
    for y in scan.axis() {
        for x in scan.axis() {
            if !scan.holds(y, x, 1) {
                continue;
            }
            let here = node_max[scan.at(y, x)];
            if !here.is_finite() || over[scan.at(y, x)] <= LIFT_M {
                continue;
            }
            let around =
                [scan.at(y - 1, x), scan.at(y + 1, x), scan.at(y, x - 1), scan.at(y, x + 1)].map(|at| node_max[at]);
            if around.iter().any(|v| !v.is_finite()) {
                continue;
            }
            core[scan.at(y, x)] = here - around.iter().sum::<f64>() / 4.0 > CONVEX_M;
        }
    }
    core
}

/// Whether a node is selected or touches one, which is §9's one-node extension.
fn dilated(core: &[bool], scan: &Scan, y: i64, x: i64) -> bool {
    (y - 1..=y + 1).any(|dy| (x - 1..=x + 1).any(|dx| scan.holds(dy, dx, 0) && core[scan.at(dy, dx)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cone on a planar slope, in the geographic frame the baker works in.
    struct Cone {
        posting: u8,
        cell: u8,
        peak: (f64, f64),
        height: f64,
    }

    impl Cone {
        fn new(ci: u32, cj: u32, posting: u8, cell: u8, at: (f64, f64), height: f64) -> Self {
            let step = (1i64 << posting) as f64;
            let origin_y = f64::from(GRID_ORIGIN) + (f64::from(ci) * (1i64 << cell) as f64);
            let origin_x = f64::from(GRID_ORIGIN) + (f64::from(cj) * (1i64 << cell) as f64);
            Self { posting, cell, peak: (origin_y + at.0 * step, origin_x + at.1 * step), height }
        }

        /// A 40° planar slope, which a coarse lattice under-samples but must not inflate.
        fn plane(&self, lat: f64, lon: f64) -> f64 {
            let step = (1i64 << self.posting) as f64;
            (lat - f64::from(GRID_ORIGIN)) / step * 40.0 + (lon - f64::from(GRID_ORIGIN)) / step * 8.0
        }

        fn reference(&self) -> impl Fn(f64, f64) -> Option<f64> + '_ {
            move |lat_deg: f64, lon_deg: f64| {
                let (lat, lon) = (lat_deg * 1e6, lon_deg * 1e6);
                let step = (1i64 << self.posting) as f64;
                let d = ((lat - self.peak.0).powi(2) + (lon - self.peak.1).powi(2)).sqrt() / step;
                Some(self.plane(lat, lon) + (self.height - self.height * d).max(0.0))
            }
        }

        fn native(&self) -> impl Fn(i32, i32) -> i16 + '_ {
            move |lat: i32, lon: i32| self.plane(f64::from(lat), f64::from(lon)).round() as i16
        }

        fn bake(&self, ci: u32, cj: u32) -> Option<LiftMap> {
            let reference = self.reference();
            LiftMap::bake(ci, cj, self.posting, self.cell, self.native(), &reference)
        }
    }

    /// A node at `(y, x)` of the cell, in µdeg.
    fn node(ci: u32, cj: u32, posting: u8, cell: u8, y: i64, x: i64) -> (i32, i32) {
        let step = 1i64 << posting;
        let origin_y = i64::from(GRID_ORIGIN) + (i64::from(ci) << cell);
        let origin_x = i64::from(GRID_ORIGIN) + (i64::from(cj) << cell);
        ((origin_y + y * step) as i32, (origin_x + x * step) as i32)
    }

    /// The tower is lifted to its own height, the plane it stands on is not, and the lift reaches
    /// the tip's neighbours but no further.
    #[test]
    fn a_tower_is_lifted_and_the_plane_it_stands_on_is_not() {
        let cone = Cone::new(0, 0, 9, 16, (8.0, 8.0), 120.0);
        let map = cone.bake(0, 0).expect("the tower is a crest");
        let at = |y, x| map.at(node(0, 0, 9, 16, y, x).0, node(0, 0, 9, 16, y, x).1);

        assert!((100..=130).contains(&at(8, 8)), "the tip is lifted to the tower's own height, got {}", at(8, 8));
        assert_eq!(at(1, 1), 0, "a planar slope four nodes away is left alone");
        assert_eq!(at(14, 14), 0, "and so is one on the far side");
        assert!(at(8, 9) > 0, "the one-node extension reaches the tip's neighbour");

        // The tally is what an operator sees: how much, how many, and where the worst of it is.
        let tally = map.tally();
        assert!(tally.nodes < 40, "the rule stays local to the tower, lifted {}", tally.nodes);
        assert_eq!(tally.max_m, at(8, 8), "the largest lift is the tip's");
        assert_eq!(tally.max_at, node(0, 0, 9, 16, 8, 8), "and it is reported at the tip");
        assert_eq!(tally.over_report, 0, "a 120 m tower is not worth an operator's attention");
        assert_eq!(LiftTally::default().join(tally), tally, "joining an empty tally changes nothing");
    }

    /// `apply` adds the lift to the native height, leaves a hole a hole, and leaves a coordinate
    /// the map does not describe alone.
    #[test]
    fn apply_adds_the_lift_and_keeps_nodata_a_hole() {
        let cone = Cone::new(0, 0, 9, 16, (8.0, 8.0), 120.0);
        let map = cone.bake(0, 0).expect("the tower is a crest");
        let tip = node(0, 0, 9, 16, 8, 8);
        let native = cone.native();
        let hole = |lat: i32, lon: i32| if (lat, lon) == tip { NODATA } else { native(lat, lon) };

        let mut lifted = map.apply(cone.native());
        assert_eq!(lifted(tip.0, tip.1), native(tip.0, tip.1) + map.at(tip.0, tip.1));
        let far = node(0, 0, 9, 16, 1, 1);
        assert_eq!(lifted(far.0, far.1), native(far.0, far.1), "an unlifted node passes through");

        let mut over_hole = map.apply(hole);
        assert_eq!(over_hole(tip.0, tip.1), NODATA, "a lift never fills a hole");

        // Half a posting off the lattice is not a node this map describes.
        assert_eq!(map.at(tip.0 + (1 << 8), tip.1), 0);
        // Nor is a node in the next cell along.
        assert_eq!(map.at(node(0, 1, 9, 16, 8, 8).0, node(0, 1, 9, 16, 8, 8).1), 0);
    }

    /// The whole point of the halo: a node on a cell seam is lifted by the same amount whichever
    /// cell computes it. A tower astride the seam exercises both the convexity ring and the
    /// dilation across it.
    #[test]
    fn two_adjacent_cells_agree_on_every_shared_edge_lift() {
        let (posting, cell) = (9u8, 16u8);
        let side = 1i64 << cell_samples_log2(posting, cell).unwrap();
        // The tower sits on the eastern seam of cell (0, 0), which is the western edge of (0, 1).
        let cone = Cone::new(0, 0, posting, cell, (40.0, side as f64), 120.0);
        let west = cone.bake(0, 0).expect("the seam tower is a crest in the west cell");
        let east = cone.bake(0, 1).expect("…and in the east cell");

        let mut shared = 0;
        for y in 0..=side {
            let (lat, lon) = node(0, 0, posting, cell, y, side);
            assert_eq!(west.at(lat, lon), east.at(lat, lon), "seam node {y} disagrees");
            shared += i32::from(west.at(lat, lon) > 0);
        }
        assert!(shared > 0, "the test would pass on two empty edges");
    }

    /// A reference that covers nothing produces no map, so the cell bakes exactly as it would
    /// without one.
    #[test]
    fn a_reference_that_covers_nothing_produces_no_map() {
        let native = |_: i32, _: i32| 1000i16;
        assert!(LiftMap::bake(0, 0, 9, 16, native, &|_, _| None).is_none());
        // …and neither does one that covers the cell but finds no crest in it.
        let flat = |_: f64, _: f64| Some(1000.0);
        assert!(LiftMap::bake(0, 0, 9, 16, native, &flat).is_none());
    }
}
