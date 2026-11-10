//! The crossings of line water by ways on the ground, as candidates of the face slice: the zones where the
//! water does not show, at a culvert, a pipe in stones or a crossing at grade, at a culvert chain's open end
//! and where a stream ends at a way. A zone is ground of the slopes over every line, the water's included, so
//! it meets the water's end at the water's level, as the banks meet its sides. Each zone is a function of
//! global data within its reach, so two cells build it alike.

#[cfg(test)]
mod tests;
mod zones;

use std::collections::BTreeMap;

use crate::faces::geom::P;
use crate::faces::stroke::{pieces_along, Rings};
use crate::faces::{Rules, WayLine, SHOULDER};
use crate::frame::{metres_per_unit, CellKey};
use crate::graph::WayGraph;
use crate::sections::{Reach, HALF_MAX_M};
use crate::CellInput;

/// A zone reads the water this far along from its crossing: it holds a road's whole slope band (at most 15 m
/// from its curve) down to a crossing angle of about 37°.
pub const ZONE_ALONG_M: f64 = 25.0;
/// A zone reads its way this far along from its crossing: `ZONE_ALONG_M` and a road's slope reach.
pub const ZONE_READ_M: f64 = 35.0;
/// A culvert chain's open end reads its chain this far for the way over it: a road's slope band (at most
/// 15 m from its curve) and a join's 3 m.
pub const CHAIN_READ_M: f64 = 18.0;
/// A zone lies within this of its seed: `ZONE_ALONG_M` along the water, and the water's band
/// (at most `HALF_MAX_M` and its bank) and a join's gap.
pub const REACH_M: f64 = ZONE_ALONG_M + 20.0;
/// Faces are final this far past the cell: the heights to 32.5 m, and a height reads the crossings' faces
/// within the water term of its reach, 15.5 m.
const FINAL_M: f64 = 48.0;
/// A zone: its rings (cell units), its way (the owner and the material of its label; none for a chain's
/// open end that no crossing names, owned by the terrain) and its seed's global place, which names it.
#[derive(Clone, Debug, PartialEq)]
pub struct Zone {
    pub rings: Rings,
    pub way: Option<u32>,
    pub seed: [i64; 2],
}

/// What the constructions decided over their seeds near the cell.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CrossingStats {
    pub zones: u32,
    pub chain_ends: u32,
    pub water_ends: u32,
    pub joins: u32,
}

impl CrossingStats {
    pub fn counters(&self) -> [(&'static str, u32); 4] {
        [("zones", self.zones), ("chain_ends", self.chain_ends), ("water_ends", self.water_ends), ("joins", self.joins)]
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WaterCrossings {
    pub zones: Vec<Zone>,
    pub stats: CrossingStats,
}

/// The crossings' rows in the reach table, from the greatest `mouth_bank_m` and `mouth_join_m`: what each
/// decision reads, where its result changes faces, and `FINAL_M`.
pub fn reach_rows(bank_m: f64, join_m: f64) -> Vec<Reach> {
    let s = HALF_MAX_M + bank_m;
    let chain = |name: &str, terms: &[(&str, f64)]| Reach {
        name: name.into(),
        owner: "water_crossings",
        chain: true,
        terms: terms.iter().map(|&(what, m)| (what.to_string(), m)).collect(),
    };
    let reads = ("ZONE_READ_M: its way, water and terrain", ZONE_READ_M);
    let last = ("FINAL_M", FINAL_M);
    vec![
        chain("crossing zone", &[reads, ("ZONE_ALONG_M and the water's band", ZONE_ALONG_M + s), last]),
        chain(
            "crossing join",
            &[
                ("a zone's seed from the gap's middle", ZONE_ALONG_M + join_m / 2.0),
                reads,
                ("the gap", join_m / 2.0 + s),
                last,
            ],
        ),
        chain(
            "crossing chain end",
            &[
                ("CHAIN_READ_M and the water's band", CHAIN_READ_M.max(s + 15.0)),
                ("the chain's band", 2.0 * s + 15.0),
                last,
            ],
        ),
    ]
}

/// The zones whose seeds lie in the box `bx` (cell units), over the lines of the faces.
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

    /// The band of the ways that a walk of `ZONE_READ_M` each way from sample `at` of `way` passes, out to their
    /// shoulders' outer edge and `d` metres on: the strokes of the faces' own lines over the box `bx` (units)
    /// grown by that offset, as rings that may overlap (the booleans that read them take their union,
    /// NonZero). None where a way of the walk has no line of the faces.
    fn band(&self, way: u32, at: u32, d: f64, bx: [f64; 4]) -> Option<Rings> {
        let g = self.graph;
        let walks = [-1, 1].map(|dir| g.walk(way, at, dir, ZONE_READ_M, false).swap_remove(0));
        let mut ways: Vec<u32> = walks.iter().flatten().map(|p| p.way).collect();
        ways.sort_unstable();
        ways.dedup();
        let r = (HALF_MAX_M + d) / self.mpu(g.samples[at as usize].pos.map(f64::from));
        // A segment whose box meets the grown box: a long segment may cross it with both ends outside.
        let near = |a: P, b: P| {
            let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
            let (lo, hi) = (lo.map(f64::from), hi.map(f64::from));
            hi[0] >= bx[0] - r && lo[0] <= bx[2] + r && hi[1] >= bx[1] - r && lo[1] <= bx[3] + r
        };
        let mut out = Vec::new();
        for w in ways {
            let l = &self.lines[*self.of_way.get(&w)?];
            let n = l.p.len().saturating_sub(1);
            let mut k = 0;
            while k < n {
                let Some(from) = (k..n).find(|&i| near(l.p[i], l.p[i + 1])) else { break };
                let to = (from..n).find(|&i| !near(l.p[i], l.p[i + 1])).unwrap_or(n);
                let run = from.saturating_sub(1)..(to + 2).min(l.p.len());
                let off: Vec<f64> =
                    run.clone().map(|i| l.bands[i][SHOULDER] + d / self.mpu(l.p[i].map(f64::from))).collect();
                if run.len() >= 2 {
                    pieces_along(&l.p[run], &off, &[], [&[], &[]], [false, false], false, &mut out);
                }
                k = to;
            }
        }
        Some(out)
    }
}

/// The box of rings (units).
fn bounds(rings: &Rings) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for p in rings.iter().flatten() {
        b = [
            b[0].min(f64::from(p[0])),
            b[1].min(f64::from(p[1])),
            b[2].max(f64::from(p[0])),
            b[3].max(f64::from(p[1])),
        ];
    }
    b
}

/// The distance (units) of q from segment ab, and the share of its foot.
fn to_segment(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let l2 = ab[0] * ab[0] + ab[1] * ab[1];
    let f = if l2 > 0.0 { (((q[0] - a[0]) * ab[0] + (q[1] - a[1]) * ab[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (libm::hypot(q[0] - a[0] - f * ab[0], q[1] - a[1] - f * ab[1]), f)
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
