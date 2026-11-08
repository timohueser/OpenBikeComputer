//! The crossings of line water by ways on the ground, as candidates of the face slice: the zone that
//! a way's fill takes from the water at a culvert or a pipe in stones, at a culvert chain's open end and
//! where a stream ends at a way; and the wet faces of a crossing at grade, where the way lies under the
//! water's level. Each is a function of global data within its reach, so two cells build it alike.

mod fords;
#[cfg(test)]
mod tests;
mod zones;

use std::collections::BTreeMap;

use crate::faces::geom::{inner_point, P};
use crate::faces::stroke::{cut, pieces_along, union, Rings};
use crate::faces::{Rules, WayLine, SHOULDER};
use crate::frame::{metres_per_unit, CellKey};
use crate::graph::{WalkPoint, WayGraph};
use crate::CellInput;

/// A zone reads the water this far along from its crossing: it holds a road's whole fill line (at most 15 m
/// from its curve) down to a crossing angle of about 37°.
pub const ZONE_ALONG_M: f64 = 25.0;
/// A zone reads its way this far along from its crossing: `ZONE_ALONG_M` and a road's slope reach.
pub const ZONE_READ_M: f64 = 35.0;
/// A zone or a wet face lies within this of its seed: `ZONE_ALONG_M` along the water, and the water's band
/// (at most `HALF_MAX_M` and its bank) and a join's gap.
pub const REACH_M: f64 = ZONE_ALONG_M + 20.0;
/// The fill line's scan step, and the room that a mouth keeps inside its way's slope band.
const SCAN_STEP_M: f64 = 0.1;
const MOUTH_ROOM_M: f64 = 0.5;
/// The gap between two pipes side by side.
const PIPE_GAP_M: f64 = 0.3;

/// A zone: its rings (cell units), its way (the owner and the material of its label; none for a chain's
/// open end that no crossing names, owned by the terrain) and its seed's global place, which names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub rings: Rings,
    pub way: Option<u32>,
    pub seed: [i64; 2],
    /// Where the water's axis leaves the zone into an open water face, and its direction.
    pub mouths: Vec<([i32; 2], [f64; 2])>,
}

/// A ford's wet face: its rings, its way (the owner) and its water (the material).
#[derive(Clone, Debug, PartialEq)]
pub struct Wet {
    pub rings: Rings,
    pub way: u32,
    pub water: u32,
    pub seed: [i64; 2],
}

/// What the constructions decided over their seeds near the cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CrossingStats {
    pub zones: u32,
    pub chain_ends: u32,
    pub water_ends: u32,
    /// Zones whose fill line no scan step reached: the zone ends at the slope band's room.
    pub zone_tall: u32,
    /// Zones whose scan or water ended before the fill line.
    pub zone_capped: u32,
    pub joins: u32,
    pub wet: u32,
    /// Crossings at grade with no part of the way under the water.
    pub ford_dry: u32,
    /// Wet faces that end at their reach while the way still lies under the water.
    pub ford_long: u32,
}

impl CrossingStats {
    pub fn counters(&self) -> [(&'static str, u32); 9] {
        [
            ("zones", self.zones),
            ("chain_ends", self.chain_ends),
            ("water_ends", self.water_ends),
            ("zone_tall", self.zone_tall),
            ("zone_capped", self.zone_capped),
            ("joins", self.joins),
            ("wet", self.wet),
            ("ford_dry", self.ford_dry),
            ("ford_long", self.ford_long),
        ]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterCrossings {
    pub zones: Vec<Zone>,
    pub wets: Vec<Wet>,
    pub stats: CrossingStats,
}

/// The zones and wet faces whose seeds lie in the box `bx` (cell units), over the lines of the faces.
pub(crate) fn build(
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    lines: &[WayLine],
    bx: [i32; 4],
) -> WaterCrossings {
    let of_way = lines.iter().enumerate().map(|(i, l)| (l.way, i)).collect();
    let ctx = Ctx { graph, input, rules, lines, of_way };
    let near = |p: P| p[0] >= bx[0] && p[0] <= bx[2] && p[1] >= bx[1] && p[1] <= bx[3];
    let mut out = WaterCrossings::default();
    out.zones = zones::zones(&ctx, &near, &mut out.stats);
    out.wets = fords::wets(&ctx, &near, &mut out.stats);
    out
}

struct Ctx<'a> {
    graph: &'a WayGraph,
    input: &'a CellInput,
    rules: &'a Rules,
    lines: &'a [WayLine<'a>],
    of_way: BTreeMap<u32, usize>,
}

impl Ctx<'_> {
    fn cell(&self) -> CellKey {
        self.graph.cell
    }

    /// Metres per frame unit at a point.
    fn mpu(&self, p: [f64; 2]) -> f64 {
        metres_per_unit(self.cell().origin().y + libm::round(p[1]) as i64)
    }

    fn global(&self, p: P) -> [i64; 2] {
        let o = self.cell().origin();
        [o.x + i64::from(p[0]), o.y + i64::from(p[1])]
    }

    fn terrain(&self, p: [f64; 2]) -> f64 {
        let g = self.global([libm::round(p[0]) as i32, libm::round(p[1]) as i32]);
        self.input.terrain.height(crate::frame::GlobalPoint { x: g[0], y: g[1] }).unwrap_or(f64::NAN)
    }

    fn feature(&self, way: u32) -> &crate::tile::Feature {
        let part = &self.graph.parts[self.graph.ways[way as usize].part as usize];
        &self.input.features[part.feature.0 as usize]
    }

    fn way_section(&self, way: u32) -> crate::sections::WaySection<'_> {
        let f = self.feature(way);
        self.rules.sections.way(&f.kind, &f.tags, false)
    }

    fn water_section(&self, way: u32) -> crate::sections::WaterSection {
        let f = self.feature(way);
        self.rules.sections.water(&f.kind, &f.tags)
    }

    /// The band of the ways of `line` (a walk along them) out to their shoulders' outer edge and `d` metres
    /// on, `d[0]` on the left, `d[1]` on the right: the strokes of the faces' own lines, within `reach_m`
    /// of the seed `at`. The lesser side is the stroke of its offset; the greater adds the part of its
    /// stroke on its side. None where a way of the walk has no line of the faces.
    fn band(&self, line: &Axis, at: P, d: [f64; 2], reach_m: f64) -> Option<Rings> {
        let mut ways: Vec<u32> = line.points.iter().map(|p| p.way).collect();
        ways.sort_unstable();
        ways.dedup();
        let r = reach_m / self.mpu(at.map(f64::from));
        let near = |p: P| (f64::from(p[0] - at[0])).abs() <= r && (f64::from(p[1] - at[1])).abs() <= r;
        let mut runs: Vec<(&WayLine, std::ops::Range<usize>)> = Vec::new();
        for w in ways {
            let l = &self.lines[*self.of_way.get(&w)?];
            let mut k = 0;
            while k < l.p.len() {
                let from = (k..l.p.len()).find(|&i| near(l.p[i]));
                let Some(from) = from else { break };
                let to = (from..l.p.len()).find(|&i| !near(l.p[i])).unwrap_or(l.p.len());
                runs.push((l, from.saturating_sub(1)..(to + 1).min(l.p.len())));
                k = to;
            }
        }
        let strokes = |m: f64| {
            let mut out = Vec::new();
            for (l, run) in &runs {
                let p = &l.p[run.clone()];
                let off: Vec<f64> =
                    run.clone().map(|i| l.bands[i][SHOULDER] + m / self.mpu(l.p[i].map(f64::from))).collect();
                if p.len() >= 2 {
                    pieces_along(p, &off, &[], [&[], &[]], [false, false], false, &mut out);
                }
            }
            union(&out, at)
        };
        let (lo, hi) = (d[0].min(d[1]), d[0].max(d[1]));
        let inner = strokes(lo);
        if hi <= lo {
            return Some(inner);
        }
        // The greater side: the pieces of its stroke that the lines cut off on that side.
        let side = if d[0] > d[1] { 1.0 } else { -1.0 };
        let cuts: Vec<[P; 2]> =
            runs.iter().flat_map(|(l, run)| l.p[run.clone()].windows(2).map(|w| [w[0], w[1]])).collect();
        let mut out = inner;
        for piece in cut(&strokes(hi), &cuts, at) {
            let Some(q) = inner_point(&piece) else { continue };
            let q = [q[0] as f64 / 2.0, q[1] as f64 / 2.0];
            if signed(line, q, self.mpu(q)).0 * side > 0.0 {
                out.extend(piece);
            }
        }
        Some(union(&out, at))
    }
}

/// The signed metres of q from a line (+ on its left), and the segment of its foot.
fn signed(line: &Axis, q: [f64; 2], mpu: f64) -> (f64, usize) {
    let mut best = (f64::INFINITY, 0.0, 0);
    for k in 0..line.points.len().saturating_sub(1) {
        let (a, b) = (line.pos(k), line.pos(k + 1));
        let (d, _) = to_segment(q, a, b);
        if d < best.0 {
            let n = left(a, b);
            let side = if (q[0] - a[0]) * n[0] + (q[1] - a[1]) * n[1] >= 0.0 { 1.0 } else { -1.0 };
            best = (d, side, k);
        }
    }
    (best.1 * best.0 * mpu, best.2)
}

/// A line through a crossing: the walks from a sample both ways (the one backward reversed), as points in
/// cell units with signed metres along.
#[derive(Clone, Debug)]
struct Axis {
    points: Vec<WalkPoint>,
}

impl Axis {
    /// The walks from sample `at` of `way` back and on, each `max_m` long, through the joints.
    fn through(g: &WayGraph, way: u32, at: u32, max_m: f64) -> Axis {
        let mut back = g.walk(way, at, -1, max_m, false).swap_remove(0);
        back.reverse();
        for p in &mut back {
            p.m = -p.m;
        }
        let on = g.walk(way, at, 1, max_m, false).swap_remove(0);
        back.extend(on.into_iter().skip(1));
        Axis { points: back }
    }

    fn pos(&self, i: usize) -> [f64; 2] {
        let p = self.points[i].pos;
        [f64::from(p[0]), f64::from(p[1])]
    }

    /// The water level at metres m along: the samples' own on the ground; in a culvert, which has none, the
    /// line between the nearest known levels on both sides, else the one known. NaN with none known.
    fn level(&self, m: f64) -> f64 {
        let known = |p: &&WalkPoint| !p.z.is_nan();
        let before = self.points.iter().rev().filter(known).find(|p| p.m <= m);
        let after = self.points.iter().filter(known).find(|p| p.m >= m);
        match (before, after) {
            (Some(a), Some(b)) if b.m > a.m => a.z + (m - a.m) / (b.m - a.m) * (b.z - a.z),
            (Some(a), _) => a.z,
            (_, Some(b)) => b.z,
            _ => f64::NAN,
        }
    }

    /// The metres along of the point of the axis nearest q.
    fn foot(&self, q: [f64; 2]) -> f64 {
        let mut best = (f64::INFINITY, 0.0);
        for k in 0..self.points.len().saturating_sub(1) {
            let (a, b) = (self.pos(k), self.pos(k + 1));
            let (d, f) = to_segment(q, a, b);
            if d < best.0 {
                best = (d, self.points[k].m + f * (self.points[k + 1].m - self.points[k].m));
            }
        }
        best.1
    }
}

/// The distance (units) of q from segment ab, and the share of its foot.
fn to_segment(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let f = if l2 > 0.0 { (((q[0] - a[0]) * ab[0] + (q[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (libm::hypot(q[0] - a[0] - f * ab[0], q[1] - a[1] - f * ab[1]), f)
}

/// The unit normal of a b on the left of the heights (+ across: x east, y south, so −cross(b − a, q − a)
/// is positive there).
fn left(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    let (x, y) = (b[0] - a[0], b[1] - a[1]);
    let l = libm::hypot(x, y);
    if l > 0.0 {
        [y / l, -x / l]
    } else {
        [0.0, 0.0]
    }
}

/// The normal at point k of a polyline: the mean of its segments' normals.
fn normal_at(pos: &dyn Fn(usize) -> [f64; 2], n: usize, k: usize) -> [f64; 2] {
    let a = if k > 0 { left(pos(k - 1), pos(k)) } else { [0.0, 0.0] };
    let b = if k + 1 < n { left(pos(k), pos(k + 1)) } else { [0.0, 0.0] };
    let m = [a[0] + b[0], a[1] + b[1]];
    let l = libm::hypot(m[0], m[1]);
    if l > 0.0 {
        [m[0] / l, m[1] / l]
    } else {
        a
    }
}

/// The stroke of a polyline (units) with a half-width per point, square ends or round ones, as the band
/// layers are stroked: every point an integer point.
fn stroke(points: &[[f64; 2]], half: &[f64], round: [bool; 2]) -> Rings {
    let p: Vec<P> = points.iter().map(|q| [libm::round(q[0]) as i32, libm::round(q[1]) as i32]).collect();
    let mut keep: Vec<usize> = Vec::with_capacity(p.len());
    for k in 0..p.len() {
        if keep.last().is_none_or(|&j| p[j] != p[k]) {
            keep.push(k);
        }
    }
    let (p, d): (Vec<P>, Vec<f64>) = keep.iter().map(|&k| (p[k], half[k])).unzip();
    let mut out = Vec::new();
    if p.len() >= 2 {
        pieces_along(&p, &d, &[], [&[], &[]], round, false, &mut out);
    }
    out
}
