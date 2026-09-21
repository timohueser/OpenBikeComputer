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
//! * **`LIFT_M` against the bilinear surface, not the node.** The interesting quantity is how far
//!   the reference stands above the surface we actually draw, measured where it stands there. That
//!   same gap is also *how far* a node rises, because `node_max - native` is the difference between
//!   a maximum over a node's cell and a point sample at its centre, and on a slope that is half a
//!   posting of fall rather than a crest. Measured against the Engelberg photographs, lifting by
//!   the gap holds or improves every view and takes the two worst from 0.45° to 0.30° and from
//!   0.84° to 0.50° of root-mean-square skyline error.
//! * **`CONVEX_M` on the reference's own node maxima.** Without it, a steep planar face reads as
//!   a crest — a coarse lattice under-samples a 40° slope honestly — and the whole mountain
//!   inflates. Measured against three photographs, the ungated rule pushed the drawn skyline half a
//!   degree high; the gate brings the median back to zero. It is also what leaves a saddle alone,
//!   so a pass does not move.
//! * **One node of dilation, bounded by the gap.** A lifted node beside unlifted neighbours makes
//!   the bilinear surface sawtooth, which the panorama shows as a jittering skyline, so the
//!   selection is extended by one node and a crest is lifted along its whole length. The gap is
//!   what keeps that honest: over Engelberg the dilation reaches 10,863 nodes — 11.5 % of the
//!   94,379 selected — where the reference stands at or below the surface we draw, and lifting
//!   those to `node_max` put the drawn skyline a third of a degree high at two viewpoints. Bounded
//!   by the gap they rise by nothing and the drawn median comes back within 0.17°.
//!
//! Every part of the rule reads only a node's **2-ring**, and [`LiftMap::bake`] scans that ring
//! beyond the cell it is asked for ([`HALO`]). A node on a cell seam therefore gets the same lift
//! whichever of the two cells computes it, which is what keeps one published cell byte-identical to
//! the same square inside a wide shard.
//!
//! ## One pass over the reference, per cell
//!
//! The reference is the archive of `reference.rs`: max-pooled whole metres on a `2^6` µdeg lattice,
//! in tiles a bake streams. So the scan is **pixel-driven**, not node-driven. It visits the tiles
//! the cell's node window touches, one decoded tile in memory at a time, and drops each pixel into
//! the node whose half-posting cell holds its centre. Every archive pixel inside the window is read
//! exactly once, which is both the cheapest possible pass and a finer probe grid than any node-side
//! sub-sampling: at the v1 posting a node's cell holds 64 archive pixels.
//!
//! A pooled pixel is a maximum, not a sample, so the gap under one is measured against the
//! **highest** the native surface reaches over that pixel's own footprint
//! ([`roof`](NativeWindow::roof)). Measuring it at the pixel's centre instead reads the gap high
//! wherever the ground is steep, and lifts nodes the reference does not stand `LIFT_M` above.

use obc_formats::obct::{cell_samples_log2, GRID_ORIGIN, NODATA};

use crate::reference::{ReferenceArchive, TileLookup, Window, NO_PIXEL, STEP_LOG2};

/// A node is a candidate when the reference stands this far above our bilinear surface.
const LIFT_M: f64 = 10.0;
/// …and only where the reference's node maxima are locally convex by this much.
const CONVEX_M: f64 = 3.0;
/// Nodes the scan reaches beyond the map's own range, which is the 2-ring the rule reads: the
/// convexity test needs a node's four neighbours, and the dilation needs their own selection.
const HALO: i64 = 2;
/// Half an archive pixel, µdeg: the reach of a pooled pixel's footprint from its centre.
const HALF_PIXEL: i64 = 1 << (STEP_LOG2 - 1);
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

/// The µdeg box of reference pixels one cell's rule reads: the `side + 1` nodes the cell's lift map
/// holds and a two-node halo, each node reaching half a posting on every side.
///
/// Public because a caller that wants to know which tiles a cell reads — a mirror, a test — should
/// ask this rather than re-derive the halo.
pub fn cell_window(ci: u32, cj: u32, posting_log2: u8, cell_log2: u8) -> Option<Window> {
    let side = 1i64 << cell_samples_log2(posting_log2, cell_log2)?;
    let step = 1i64 << posting_log2;
    let origin_y = i64::from(GRID_ORIGIN) + (i64::from(ci) << cell_log2);
    let origin_x = i64::from(GRID_ORIGIN) + (i64::from(cj) << cell_log2);
    Some(Window {
        lat_lo: origin_y - HALO * step - step / 2,
        lat_hi: origin_y + (side + HALO) * step + step / 2,
        lon_lo: origin_x - HALO * step - step / 2,
        lon_hi: origin_x + (side + HALO) * step + step / 2,
    })
}

/// What one cell's scan produced: the map, and what the archive could not give it.
///
/// The two are separate because a cell with no lift at all still has something to report — a mirror
/// that is short of its box costs a whole cell's lifts and does not fail.
pub struct CellLift {
    /// The lifts, or `None` when the rule selected nothing in this cell.
    pub map: Option<LiftMap>,
    /// Tiles the index named for this cell's window that the archive does not hold.
    pub absent_tiles: Vec<(u32, u32)>,
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
    /// Every source that contributed a pixel to a tile this cell **decoded**, sorted. An index
    /// entry whose tile a mirror does not hold is not in here: a container cannot be derived from
    /// bytes the bake never read, and attribution follows what was read.
    sources: Vec<String>,
}

impl LiftMap {
    /// Build one cell's lift map, or `None` when the rule selects no node in it — which is the
    /// common case, since national reference coverage stops at borders.
    ///
    /// `native` is the unlifted lattice, sampled in µdeg: the gap is measured against the bilinear
    /// surface through it. `archive` holds the reference, and coverage may stop at any pixel.
    pub fn bake(
        ci: u32,
        cj: u32,
        posting_log2: u8,
        cell_log2: u8,
        native: impl FnMut(i32, i32) -> i16,
        archive: &ReferenceArchive,
    ) -> Result<CellLift, String> {
        let samples_log2 = cell_samples_log2(posting_log2, cell_log2).ok_or_else(|| {
            format!("posting 2^{posting_log2} µdeg with cell 2^{cell_log2} µdeg is not a pairing OBCT permits")
        })?;
        // A node has to own at least one archive pixel, or the rule has nothing to measure: the
        // reference maximum inside its half-posting cell, and the roof of a pixel's footprint, are
        // both areas of the archive lattice. Below that step the two lattices invert — a pixel would
        // span several nodes — and the pass silently produced no lift at all rather than saying so.
        if posting_log2 < STEP_LOG2 {
            return Err(format!(
                "posting 2^{posting_log2} µdeg is finer than the reference archive's 2^{STEP_LOG2} µdeg step, \
                 so a node owns no archive pixel"
            ));
        }
        let window = cell_window(ci, cj, posting_log2, cell_log2).expect("the pairing is checked above");
        let side = 1i64 << samples_log2;
        let step = 1i64 << posting_log2;
        let origin_y = i64::from(GRID_ORIGIN) + (i64::from(ci) << cell_log2);
        let origin_x = i64::from(GRID_ORIGIN) + (i64::from(cj) << cell_log2);
        let scan = Scan::new(side, HALO);

        // The native heights the rule reads: every node the scan holds, plus the one further ring
        // its 3×3 hole test and its bilinear intervals reach into.
        let heights = NativeWindow::sample(origin_y, origin_x, posting_log2, scan.low - 1, scan.high + 1, native);
        // A node with a hole anywhere in its 3×3 native ring is out of the rule altogether (§9.1):
        // there is no bilinear surface there to measure a gap against. Deciding it once per node
        // rather than once per pixel is also what keeps the hole test off the inner loop — and it is
        // what lets the loop below interpolate without checking for a hole, because a pixel lands in
        // the node nearest it, and the interval holding the pixel's whole footprint has all four of
        // its corners in that node's 3×3 ring.
        let holed: Vec<bool> = scan.nodes_iter().map(|(y, x)| heights.hole_in_ring(y, x)).collect();

        // The reference maximum inside each node's own half-posting cell, and how far that maximum
        // stands above the bilinear surface we would otherwise draw there.
        let mut node_max = vec![NO_PIXEL; scan.nodes()];
        let mut gap = vec![f64::NEG_INFINITY; scan.nodes()];
        let mut any = false;
        // Attribution follows what was decoded, not what the index promised.
        let mut contributors: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
        let mut absent_tiles = Vec::new();
        for (ti, tj) in window.tiles() {
            let (tile, sources) = match archive.tile(ti, tj)? {
                TileLookup::Held { tile, sources } => (tile, sources),
                TileLookup::Absent => {
                    absent_tiles.push((ti, tj));
                    continue;
                }
                TileLookup::Unknown => continue,
            };
            contributors.extend(sources.iter().map(String::as_str));
            tile.centres_in(window, |lat, lon, height| {
                let at = scan.at(node_of(lat, origin_y, step), node_of(lon, origin_x, step));
                if holed[at] {
                    return;
                }
                node_max[at] = node_max[at].max(height);
                gap[at] = gap[at].max(f64::from(height) - heights.roof(lat, lon));
                any = true;
            });
        }
        if !any {
            return Ok(CellLift { map: None, absent_tiles });
        }

        let core = select(&node_max, &gap, &scan);
        let stride = (side + 1) as usize;
        let mut lifts = vec![0i16; stride * stride];
        let mut tally = LiftTally::default();
        for y in 0..=side {
            for x in 0..=side {
                let at = scan.at(y, x);
                let top = node_max[at];
                let (lat, lon) = ((origin_y + y * step) as i32, (origin_x + x * step) as i32);
                let here = heights.at(y, x);
                if here == NODATA || top == NO_PIXEL || !dilated(&core, &scan, y, x) {
                    continue;
                }
                // The node rises by the **gap** — how far the reference stands above the surface —
                // and never past `node_max`, the highest reference the node owns. Never negative
                // either: the reference may sit below our surface in a hollow, and §9 raises crests
                // rather than editing the lattice wherever the two disagree. Whole metres, because
                // an archive pixel is whole metres and so is the sample it lands in (§1.2), which
                // makes §9.1's `round` the identity here.
                let to_top = i32::from(top) - i32::from(here);
                let lift = to_top.min(gap[at].round() as i32).clamp(0, i32::from(i16::MAX)) as i16;
                if lift == 0 {
                    continue;
                }
                lifts[y as usize * stride + x as usize] = lift;
                // The tally is a **report**, not the existence test: it counts the nodes this cell
                // owns, because the inclusive high edge is the next cell's node 0 and is counted
                // there, so a run's count is one per written sample rather than two per seam.
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
        // A map exists when it holds **any** lift, including one only on the inclusive high edge.
        // Gating on the owned-node count instead would let a cell whose coverage begins on its own
        // seam answer 0 there while its neighbour answers the lift, which is exactly the
        // disagreement the halo exists to prevent.
        let any_lift = lifts.iter().any(|&lift| lift != 0);
        let sources = contributors.into_iter().map(str::to_string).collect();
        let map = any_lift.then_some(LiftMap { origin_y, origin_x, step, stride, lifts, tally, sources });
        Ok(CellLift { map, absent_tiles })
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

    /// Every reference source this cell's lifts are derived from, sorted — the attribution that
    /// must travel with a container holding this cell (§9.3).
    pub fn sources(&self) -> &[String] {
        &self.sources
    }
}

/// The node whose half-posting cell holds a reference pixel centre: the nearest node.
///
/// Pixel centres sit on odd multiples of `2^5` µdeg, so at every posting from `2^7` µdeg up a centre
/// is never exactly half way between two nodes and there is no tie to break. At `2^6` µdeg — a
/// posting no production bake uses, and the archive's own step — every centre is exactly half way,
/// and the rounding then goes north and east; that is deterministic, which is all the lift rule
/// needs from it.
fn node_of(centre: i64, origin: i64, step: i64) -> i64 {
    (centre - origin + step / 2).div_euclid(step)
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

    fn side(&self) -> usize {
        (self.high - self.low + 1) as usize
    }

    fn nodes(&self) -> usize {
        self.side() * self.side()
    }

    /// Every node coordinate the scan holds, in the row-major order [`at`](Self::at) indexes.
    fn nodes_iter(&self) -> impl Iterator<Item = (i64, i64)> + '_ {
        self.axis().flat_map(move |y| self.axis().map(move |x| (y, x)))
    }

    /// Flat index of a node coordinate. Callers stay inside [`axis`](Self::axis).
    fn at(&self, y: i64, x: i64) -> usize {
        (y - self.low) as usize * self.side() + (x - self.low) as usize
    }

    /// Whether the scan holds this node **and** the ring around it, which is what the convexity
    /// test reads. `HALO` guarantees it for every node a map stores.
    fn has_ring(&self, y: i64, x: i64) -> bool {
        let inner = self.low + 1..=self.high - 1;
        inner.contains(&y) && inner.contains(&x)
    }
}

/// The native lattice over the square of node coordinates one cell's rule reads, sampled once.
///
/// The pass over archive pixels reads this a few times per pixel — 67 million pixels for a v1 cell
/// — so the sampler behind it, a bilinear over a GeoTIFF mosaic, is called once per node instead of
/// once per probe.
struct NativeWindow {
    origin_y: i64,
    origin_x: i64,
    posting_log2: u8,
    step: i64,
    low: i64,
    side: usize,
    heights: Vec<i16>,
}

impl NativeWindow {
    fn sample(
        origin_y: i64,
        origin_x: i64,
        posting_log2: u8,
        low: i64,
        high: i64,
        mut native: impl FnMut(i32, i32) -> i16,
    ) -> NativeWindow {
        let step = 1i64 << posting_log2;
        let side = (high - low + 1) as usize;
        let mut heights = Vec::with_capacity(side * side);
        for y in low..=high {
            for x in low..=high {
                heights.push(native((origin_y + y * step) as i32, (origin_x + x * step) as i32));
            }
        }
        NativeWindow { origin_y, origin_x, posting_log2, step, low, side, heights }
    }

    /// The native height at a node coordinate. Outside the window is a hole, which stops the rule
    /// rather than reading a neighbour's value — but `HALO` means no caller asks.
    fn at(&self, y: i64, x: i64) -> i16 {
        let index = |v: i64| usize::try_from(v - self.low).ok().filter(|&i| i < self.side);
        match (index(y), index(x)) {
            (Some(y), Some(x)) => self.heights[y * self.side + x],
            _ => NODATA,
        }
    }

    /// Whether any of the 3×3 native heights around a node is a hole (§9.1).
    fn hole_in_ring(&self, y: i64, x: i64) -> bool {
        (y - 1..=y + 1).any(|dy| (x - 1..=x + 1).any(|dx| self.at(dy, dx) == NODATA))
    }

    /// The bilinear native surface at a µdeg coordinate inside the window.
    ///
    /// In corner-and-slope form, as `DemMosaic::height` is and for the same reason: over four equal
    /// corners the three difference terms are exactly `0.0`, so a flat surface stays flat to the bit
    /// and a gap over it is exactly the reference's own height above it.
    fn surface_at(&self, lat: i64, lon: i64) -> f64 {
        let (dy, dx) = (lat - self.origin_y, lon - self.origin_x);
        // A posting is a power of two, so `>>` is the floor division and `&` the remainder — for a
        // coordinate south or west of the cell origin as well, which is where the halo reads.
        let (iy, ix) = (dy >> self.posting_log2, dx >> self.posting_log2);
        let step = self.step as f64;
        let (fy, fx) = ((dy & (self.step - 1)) as f64 / step, (dx & (self.step - 1)) as f64 / step);
        let v00 = f64::from(self.at(iy, ix));
        let v10 = f64::from(self.at(iy + 1, ix));
        let v01 = f64::from(self.at(iy, ix + 1));
        let v11 = f64::from(self.at(iy + 1, ix + 1));
        v00 + (v10 - v00) * fy + (v01 - v00) * fx + (v11 - v01 - v10 + v00) * fy * fx
    }

    /// The **highest** the native surface reaches over one archive pixel's footprint: the pixel's
    /// own square, [`HALF_PIXEL`] µdeg either side of the centre this is given.
    ///
    /// §9.1's gap has to be a lower bound on how far the reference stands above our surface, and an
    /// archive pixel is a *maximum* that may have come from anywhere inside its square. Measuring
    /// it against the surface at the pixel's centre therefore reads the gap high on steep ground —
    /// up to 3 m on a 40° face, which is a third of the 10 m gate — and lifts nodes the reference
    /// does not stand 10 m above. Against the roof of the footprint the gap can only read low.
    ///
    /// Four corners settle it. A bilinear patch is a saddle, so its maximum over an axis-aligned
    /// rectangle is at a corner, and each corner is evaluated in the lattice interval that holds it.
    ///
    /// **Requires a posting of at least [`STEP_LOG2`]**, which [`LiftMap::bake`] refuses otherwise.
    /// At or above that step the lattice interval boundaries are multiples of the pixel side, so a
    /// footprint lies inside one interval and the four corners are the exact maximum. Below it a
    /// footprint would span several intervals and could enclose a lattice node, whose height the
    /// corners would miss — the maximum would then read low and the gap high, which is the error
    /// this function exists to remove.
    fn roof(&self, lat: i64, lon: i64) -> f64 {
        let (south, north) = (lat - HALF_PIXEL, lat + HALF_PIXEL);
        let (west, east) = (lon - HALF_PIXEL, lon + HALF_PIXEL);
        self.surface_at(south, west)
            .max(self.surface_at(south, east))
            .max(self.surface_at(north, west))
            .max(self.surface_at(north, east))
    }
}

/// Nodes the rule selects before dilation: the reference stands `LIFT_M` above our surface at a
/// node whose own four neighbours it is convex over by `CONVEX_M`.
///
/// A node the reference misses at any of those five places is left unselected. The test has no
/// answer there, and inventing one — by clamping to the node itself, say — would make the lift
/// depend on which cell asked for it.
fn select(node_max: &[i16], gap: &[f64], scan: &Scan) -> Vec<bool> {
    let mut core = vec![false; scan.nodes()];
    for (y, x) in scan.nodes_iter() {
        if !scan.has_ring(y, x) {
            continue;
        }
        let here = node_max[scan.at(y, x)];
        if here == NO_PIXEL || gap[scan.at(y, x)] <= LIFT_M {
            continue;
        }
        let around =
            [scan.at(y - 1, x), scan.at(y + 1, x), scan.at(y, x - 1), scan.at(y, x + 1)].map(|at| node_max[at]);
        if around.contains(&NO_PIXEL) {
            continue;
        }
        let mean = around.iter().map(|&v| f64::from(v)).sum::<f64>() / 4.0;
        core[scan.at(y, x)] = f64::from(here) - mean > CONVEX_M;
    }
    core
}

/// Whether a node is selected or touches one, which is §9's one-node extension. 8-connected: a
/// node that touches a selected node at a corner is lifted too. [`HALO`] keeps the whole ring
/// inside `core` for every node a map stores, so there is nothing to bounds-test here.
fn dilated(core: &[bool], scan: &Scan, y: i64, x: i64) -> bool {
    (y - 1..=y + 1).any(|dy| (x - 1..=x + 1).any(|dx| core[scan.at(dy, dx)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pixel centre belongs to the node nearest it, on both sides of the origin and at a negative
    /// coordinate — the arithmetic every lift depends on, and the one place a `>> log2` would have
    /// been wrong.
    #[test]
    fn a_pixel_centre_belongs_to_the_node_nearest_it() {
        let step = 512i64;
        let origin = -1_024_000i64;
        // The eight centres of the node's own cell either side of it, and the next node's first.
        for (offset, want) in [(-256 + 32, 0), (-32, 0), (32, 0), (256 - 32, 0), (256 + 32, 1), (512 + 32, 1)] {
            assert_eq!(node_of(origin + offset, origin, step), want, "centre {offset} µdeg from the node");
        }
        // Below the origin the answer is negative rather than clamped, which is what lets the halo
        // reach into the cell to the south and west.
        assert_eq!(node_of(origin - 256 - 32, origin, step), -1);
        assert_eq!(node_of(origin - 512 - 32, origin, step), -1);
        assert_eq!(node_of(origin - 512 - 256 - 32, origin, step), -2);
    }

    /// An archive that holds no tile the cell reads leaves the cell exactly as a bake without a
    /// reference would — `None`, not a map of zeroes.
    #[test]
    fn a_reference_that_covers_nothing_produces_no_map() {
        let root = std::env::temp_dir().join(format!("obc-dem-empty-archive-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("index.json"),
            br#"{"schema": 1, "step_log2": 6, "tile_log2": 16, "sources": {}, "tiles": {}}"#,
        )
        .unwrap();
        let archive = ReferenceArchive::open(&root).unwrap();
        assert!(archive.is_empty());

        let native = |_: i32, _: i32| 1000i16;
        assert!(LiftMap::bake(0, 0, 9, 19, native, &archive).unwrap().map.is_none());
        // A pairing OBCT does not permit is refused before any tile is read.
        assert!(LiftMap::bake(0, 0, 9, 12, native, &archive).is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
