//! The records of a cell: its candidates with the data that `decide` reads, listed per bin, so that
//! a reader gives any point the overlay's label by the overlay's own decision, near and far.
//!
//! A block is little-endian words; coordinates are frame units from the cell's origin. A change of
//! the layout changes `MAGIC`.
//!
//! | Words | Holds |
//! | --- | --- |
//! | head | `HEAD` words: `MAGIC`; the counts of candidates, labels and lines; the offsets of the candidates, labels, lines, samples, extras and bin headers; the cell's row of zoom 16 |
//! | rules | `heights::RULES` words: the rule values of the heights (`HeightRules::words`) |
//! | candidates | `CAND` words each: kind, key, glob order, label, line, then offset and count of its discs, beyond cuts and parts in the extras |
//! | labels | `LABEL` words each: surface, owner kind, rank, height rule; owner id; material line; rail line of a crossing; 4 host lines; 4 other arm lines |
//! | lines | `LINE` words each: way, finish code with flags, first sample, sample count, cut, fill, reach (m), class rank, shoulder, verge (m), return grade, half-width (m), the kind of each end, the steepest fill and cut |
//! | samples | `SAMPLE` words each: x, y, half-width (units), metres along, glob order, height, bank, grade, terrain at the reach right and left, mitre direction x, y, health radius left and right (units) |
//! | extras | discs (`DISC` words), beyond cuts (a cut and a label), parts (a way and two cuts, each with a flag) |
//! | bins | `BINS`² headers of `HEADER` words, then per bin its candidate items, segments, height segments and height discs |
//!
//! Floats are f32 bits. A side is right or left of the line's direction in frame axes (y south).
//! A candidate item is its index with its winding number at the bin's reference point (its middle
//! plus half a unit) + 128 << 16, its run count, then per run a vertex count and the vertices (x, y)
//! of a ring's consecutive edges that meet the bin grown by `MARGIN_M`. The vertices are the rings'
//! own, so two cells hold the same edges. A segment item `line | sample << 16` is the centre line
//! from that sample to the next.

pub mod check;

use std::collections::BTreeMap;

use super::geom::P;
use super::points::Circle;
use super::stroke::union;
use super::{candidates, snap, Cand, Glob, Label, Owner, Rules, Surface, WayLine};
use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use crate::graph::WayGraph;
use crate::sections::Finish;
use crate::CellInput;

/// "OBR" and the layout's version.
pub const MAGIC: u32 = 0x4f42_5203;
/// Bins per side of the cell square: 4,096 frame units, about 6.5 m.
pub const BINS: i32 = 64;
pub const BIN_UNITS: i32 = CELL_UNITS as i32 / BINS;
/// A bin lists every candidate edge within this of its square: the reach of the anti-aliasing and
/// of the edge lines.
pub const MARGIN_M: f64 = 3.0;
/// A bin lists every centre-line segment within this past the way's half-width: the paint reads
/// the metres across and along of the label's way.
pub const LINE_PAD_M: f64 = 3.0;
/// The heights read a line's segments and the discs within its slope reach and this past a bin.
pub const HEIGHT_PAD_M: f64 = 1.0;
pub const HEAD: usize = 11;
pub const CAND: usize = 11;
pub const LABEL: usize = 12;
pub const LINE: usize = 16;
pub const SAMPLE: usize = 14;
/// Centre, radius, the two feet, the shoulder ring at each foot (m), the line at each foot.
pub const DISC: usize = 11;
pub const BEYOND: usize = 5;
pub const PART: usize = 11;
/// Offset, candidate items << 16 | segment items, height segment items << 16 | height disc items.
pub const HEADER: usize = 3;

/// Candidate kinds.
pub const WAY: u32 = 0;
pub const POINT: u32 = 1;
pub const REGION: u32 = 2;
pub const BAND: u32 = 3;
pub const FILL: u32 = 4;
/// The paved outline: the union of the paving candidates, for the distance to the road's edge. It
/// takes no part in the decision.
pub const OUTLINE: u32 = 5;

/// Owner kinds.
pub const TERRAIN: u32 = 0;
pub const WAY_OWNER: u32 = 1;
pub const JUNCTION: u32 = 2;
pub const PLATFORM: u32 = 3;

/// A line flag above the finish code's byte.
pub const RAIL: u32 = 1 << 8;

/// End kinds of a line: a free (round) end, an arm of a junction or joint, a level change.
pub const FREE: u32 = 0;
pub const MEETS: u32 = 1;
pub const LEVEL: u32 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordStats {
    pub cands: u32,
    pub labels: u32,
    pub lines: u32,
    pub samples: u32,
    pub bins_used: u32,
    pub bin_items: u32,
    pub bin_edges: u32,
    /// The most words of one bin.
    pub max_bin_words: u32,
}

pub struct Records {
    pub cell: CellKey,
    pub words: Vec<u32>,
    pub stats: RecordStats,
}

/// The records of one cell and level. `slopes = false` leaves out the slope layer, as `build`.
pub fn records(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8, slopes: bool) -> Records {
    let c = candidates(graph, input, rules, cell, level, if slopes { 4 } else { 3 });
    encode(cell, graph, input, rules, &c.lines, &c.cands)
}

/// The far level of a cell's records, for points where a pixel spans more than the band layers'
/// detail: no band layers. The band layers rank below every paving candidate, so a point that the
/// near records give no band layer's label has the same label here.
pub fn records_far(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8) -> Records {
    let c = candidates(graph, input, rules, cell, level, 0);
    encode(cell, graph, input, rules, &c.lines, &c.cands)
}

fn f(v: f64) -> u32 {
    (v as f32).to_bits()
}

fn kind(c: &Cand) -> u32 {
    if c.relabel.is_some() {
        REGION
    } else if c.fill.is_some() {
        FILL
    } else if c.glob.is_none() {
        WAY
    } else if c.label.owner == Owner::Terrain {
        BAND
    } else {
        POINT
    }
}

/// Whether a candidate paves: a carriageway's label that is no band layer and relabels nothing.
fn paves(c: &Cand) -> bool {
    c.label.surface == Surface::Carriageway && !matches!(kind(c), REGION | BAND)
}

pub(super) fn label_words(l: &Label) -> [u32; 4] {
    let (kind, id) = match l.owner {
        Owner::Terrain => (TERRAIN, 0),
        Owner::Way(w) => (WAY_OWNER, w),
        Owner::Junction(j) => (JUNCTION, j),
        Owner::Platform(p) => (PLATFORM, p),
    };
    [
        l.surface as u32 | kind << 8 | u32::from(l.rank) << 16 | (l.rule as u32) << 24,
        id,
        l.material.unwrap_or(u32::MAX),
        l.crossing.unwrap_or(u32::MAX),
    ]
}

/// Segment ab clipped to the box [x0, y0, x1, y1], if they meet.
fn clip(a: P, b: P, bx: [f64; 4]) -> Option<([f64; 2], [f64; 2])> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
    let (dx, dy) = (f64::from(b[0]) - ax, f64::from(b[1]) - ay);
    for (p, q) in [(-dx, ax - bx[0]), (dx, bx[2] - ax), (-dy, ay - bx[1]), (dy, bx[3] - ay)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    (t0 <= t1).then_some(([ax + t0 * dx, ay + t0 * dy], [ax + t1 * dx, ay + t1 * dy]))
}

/// The distance from segment ab to the box [x0, y0, x1, y1]: 0 where they meet.
fn seg_box(a: P, b: P, bx: [f64; 4]) -> f64 {
    if clip(a, b, bx).is_some() {
        return 0.0;
    }
    let (fa, fb) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
    let to_box = |p: [f64; 2]| {
        let dx = (bx[0] - p[0]).max(p[0] - bx[2]).max(0.0);
        let dy = (bx[1] - p[1]).max(p[1] - bx[3]).max(0.0);
        libm::hypot(dx, dy)
    };
    let corners = [[bx[0], bx[1]], [bx[2], bx[1]], [bx[0], bx[3]], [bx[2], bx[3]]];
    corners.into_iter().map(|q| seg_dist(fa, fb, q)).fold(to_box(fa).min(to_box(fb)), f64::min)
}

fn seg_dist(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> f64 {
    let u = [b[0] - a[0], b[1] - a[1]];
    let t = (((q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1]) / (u[0] * u[0] + u[1] * u[1]).max(1e-12)).clamp(0.0, 1.0);
    libm::hypot(q[0] - a[0] - t * u[0], q[1] - a[1] - t * u[1])
}

/// A bin's reference point less half a unit: its middle.
pub fn reference(i: i32) -> i32 {
    i * BIN_UNITS + BIN_UNITS / 2
}

/// The bin square [x0, y0, x1, y1] of column c and row r, grown by g units.
fn square(c: i32, r: i32, g: f64) -> [f64; 4] {
    let u = f64::from(BIN_UNITS);
    [f64::from(c) * u - g, f64::from(r) * u - g, f64::from(c + 1) * u + g, f64::from(r + 1) * u + g]
}

/// The bins whose square grown by g meets the box [x0, y0, x1, y1]: columns and rows, or none.
fn bins_of(bx: [f64; 4], g: f64) -> Option<([i32; 2], [i32; 2])> {
    let cell = f64::from(CELL_UNITS as i32);
    if bx[2] + g < 0.0 || bx[3] + g < 0.0 || bx[0] - g > cell || bx[1] - g > cell {
        return None;
    }
    let to = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).clamp(0, BINS - 1);
    Some(([to(bx[0] - g), to(bx[2] + g)], [to(bx[1] - g), to(bx[3] + g)]))
}

/// The windings of rings at the reference point of each bin of columns c and rows r, as `find` reads
/// it: moved by (ε, ε²) in frame axes. Per row a horizontal line through the reference points in
/// doubled units (odd, so it meets no vertex), and the signed crossings east of each point, exact.
fn windings(rings: &[Vec<P>], c: [i32; 2], r: [i32; 2]) -> Vec<i32> {
    let cols = (c[1] - c[0] + 1) as usize;
    let mut out = vec![0; cols * (r[1] - r[0] + 1) as usize];
    for row in r[0]..=r[1] {
        let y = 2 * i64::from(reference(row)) + 1;
        let mut edges: Vec<([i64; 2], [i64; 2], i32)> = Vec::new();
        for ring in rings {
            for (i, &a) in ring.iter().enumerate() {
                let b = ring[(i + 1) % ring.len()];
                let (a, b) = ([2 * i64::from(a[0]), 2 * i64::from(a[1])], [2 * i64::from(b[0]), 2 * i64::from(b[1])]);
                if (a[1] < y) != (b[1] < y) {
                    edges.push((a, b, if b[1] > a[1] { 1 } else { -1 }));
                }
            }
        }
        for col in c[0]..=c[1] {
            let x = 2 * i64::from(reference(col)) + 1;
            // East of the point: (x at y) − x has the sign of num / (b.y − a.y). On the edge, the
            // moved point lies east of it.
            let east = |&(a, b, _): &&([i64; 2], [i64; 2], i32)| {
                let num = (a[0] - x) * (b[1] - a[1]) + (b[0] - a[0]) * (y - a[1]);
                num != 0 && (num > 0) == (b[1] > a[1])
            };
            out[(row - r[0]) as usize * cols + (col - c[0]) as usize] = edges.iter().filter(east).map(|e| e.2).sum();
        }
    }
    out
}

/// Per vertex of a line: the mitre direction (unit, the bisector of its two chords) and the health
/// radius on the left and on the right (units, 1e9 for none): the nearest crossing on that side of
/// the mitre lines of the two quads that meet at the vertex.
fn mitres(p: &[P]) -> Vec<[f64; 4]> {
    let at = |k: usize| [f64::from(p[k][0]), f64::from(p[k][1])];
    let unit = |a: [f64; 2]| {
        let l = libm::hypot(a[0], a[1]);
        if l > 0.0 {
            [a[0] / l, a[1] / l]
        } else {
            [1.0, 0.0]
        }
    };
    let n = p.len();
    let chord = |i: usize| unit([at(i + 1)[0] - at(i)[0], at(i + 1)[1] - at(i)[1]]);
    let t: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let (a, b) = ((i > 0).then(|| chord(i - 1)), (i + 1 < n).then(|| chord(i)));
            let [a, b] = [a, b].map(|v| v.unwrap_or([0.0, 0.0]));
            unit([a[0] + b[0], a[1] + b[1]])
        })
        .collect();
    // Where the mitre lines of vertices i and i + 1 cross: the signed offset along i's left normal.
    let crossing = |i: usize| {
        let (ma, mb) = ([t[i][1], -t[i][0]], [t[i + 1][1], -t[i + 1][0]]);
        let den = ma[0] * mb[1] - ma[1] * mb[0];
        if den.abs() < 1e-12 {
            return f64::INFINITY;
        }
        let ab = [at(i + 1)[0] - at(i)[0], at(i + 1)[1] - at(i)[1]];
        (ab[0] * mb[1] - ab[1] * mb[0]) / den
    };
    (0..n)
        .map(|i| {
            let rho = |side: f64| {
                let near = [(i > 0).then(|| crossing(i - 1)), (i + 1 < n).then(|| crossing(i))];
                near.into_iter().flatten().filter(|c| c * side > 0.0).fold(1e9, |r: f64, c| r.min(c * side))
            };
            [t[i][0], t[i][1], rho(1.0), rho(-1.0)]
        })
        .collect()
}

/// The finish code of the viewer's looks: `Finish` in order from 1.
fn code(finish: Finish) -> u32 {
    Finish::ALL.iter().position(|&x| x == finish).map_or(0, |i| i as u32 + 1)
}

/// The words of a line and of its samples. A taper's vertex between two graph samples takes their
/// values at its share of the segment.
fn line_words(
    line: &WayLine,
    graph: &WayGraph,
    input: &CellInput,
    order: &dyn Fn(&Glob) -> u32,
    first: u32,
) -> (Vec<u32>, Vec<u32>) {
    let way = &graph.ways[line.way as usize];
    let sec = &line.sec;
    let mut w = vec![line.way, code(sec.finish) | if line.rail { RAIL } else { 0 }, first, line.p.len() as u32];
    w.extend([f(sec.cut), f(sec.fill), f(sec.reach_m), sec.rank as u32]);
    w.extend([f(sec.shoulder_m), f(sec.verge_m), f(sec.return_grade), f(sec.half_m)]);
    let meets = |dir: i8, st: &crate::graph::Station| {
        let arm = |a: &crate::graph::Arm| a.way == line.way && a.dir == dir && a.at.span == st.span && a.at.u == st.u;
        graph.junctions.iter().any(|j| j.arms.iter().any(arm)) || graph.joints.iter().any(|j| j.arms.iter().any(arm))
    };
    for (k, (dir, st)) in [(1i8, &way.from), (-1i8, &way.to)].into_iter().enumerate() {
        w.push(if line.round[k] {
            FREE
        } else if meets(dir, st) {
            MEETS
        } else {
            LEVEL
        });
    }
    w.extend([f(sec.fill_max), f(sec.cut_max)]);
    let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
    // Per vertex: its graph sample and its share of the segment to the next one.
    let mut share = vec![(0usize, 0.0f64); line.p.len()];
    for (k, win) in line.index.windows(2).enumerate() {
        let (a, b) = (line.p[win[0]], line.p[win[1]]);
        let len = libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])).max(1e-9);
        for (j, s) in share.iter_mut().enumerate().take(win[1]).skip(win[0]) {
            let p = line.p[j];
            *s = (k, libm::hypot(f64::from(p[0] - a[0]), f64::from(p[1] - a[1])) / len);
        }
    }
    if let Some(&last) = line.index.last() {
        share[last] = (line.index.len() - 1, 0.0);
    }
    let mitres = mitres(&line.p);
    let origin = input.key.origin();
    let mut s = Vec::with_capacity(SAMPLE * line.p.len());
    for (i, &p) in line.p.iter().enumerate() {
        let (k, x) = share[i];
        let (s0, s1) = (&samples[k], &samples[(k + 1).min(samples.len() - 1)]);
        let lerp = |a: f64, b: f64| a + x * (b - a);
        s.extend([p[0] as u32, p[1] as u32, f(line.h[i]), f(lerp(s0.along_m, s1.along_m)), order(&line.key[i])]);
        s.extend([f(lerp(s0.z_m, s1.z_m)), f(lerp(s0.bank, s1.bank)), f(lerp(s0.grade, s1.grade))]);
        // The plain terrain at the slope's reach on each side: NaN where unknown.
        let t = [lerp(s0.tangent[0], s1.tangent[0]), lerp(s0.tangent[1], s1.tangent[1])];
        let len = libm::hypot(t[0], t[1]).max(1e-12);
        let (n, reach) = ([-t[1] / len, t[0] / len], line.bands[i][3]);
        for side in [1.0, -1.0] {
            let g = GlobalPoint {
                x: origin.x + i64::from(p[0]) + libm::round(side * reach * n[0]) as i64,
                y: origin.y + i64::from(p[1]) + libm::round(side * reach * n[1]) as i64,
            };
            s.push(f(input.terrain.height(g).unwrap_or(f64::NAN)));
        }
        s.extend(mitres[i].map(f));
    }
    (w, s)
}

fn disc_words(d: &Circle, origin_y: i64) -> [u32; DISC] {
    let m = |k: usize| d.ring[k] * metres_per_unit(origin_y + i64::from(d.feet[k][1]));
    let line = |l: usize| if l == usize::MAX { u32::MAX } else { l as u32 };
    [
        d.centre[0] as u32,
        d.centre[1] as u32,
        f(d.radius),
        d.feet[0][0] as u32,
        d.feet[0][1] as u32,
        d.feet[1][0] as u32,
        d.feet[1][1] as u32,
        f(m(0)),
        f(m(1)),
        line(d.lines[0]),
        line(d.lines[1]),
    ]
}

/// The words of a cut as the slice holds it: its ends snapped as the overlay's `past` reads them.
fn cut_words(cut: &[P; 2]) -> [u32; 4] {
    let (a, b) = (snap(cut[0]), snap(cut[1]));
    [a[0] as u32, a[1] as u32, b[0] as u32, b[1] as u32]
}

fn encode(
    cell: CellKey,
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    lines: &[WayLine],
    cands: &[Cand],
) -> Records {
    let origin = cell.origin();
    let mpu = metres_per_unit(origin.y + CELL_UNITS / 2);
    let units = |m: f64| m / mpu;
    // The paved outline with the circles of its fans and rounds: one union, so the distance to the
    // road's edge has no seam inside the paved area.
    let paved: Vec<Vec<P>> = cands.iter().filter(|c| paves(c)).flat_map(|c| c.rings.iter().cloned()).collect();
    let outline = union(&paved, [0, 0]);
    let outline_discs: Vec<&Circle> = cands.iter().filter(|c| paves(c)).flat_map(|c| &c.discs).collect();
    // The global names in order: a way's at its samples, a point's at its position.
    let mut globs: Vec<Glob> =
        lines.iter().flat_map(|l| l.key.iter().copied()).chain(cands.iter().filter_map(|c| c.glob)).collect();
    globs.sort_unstable();
    globs.dedup();
    let order = |g: &Glob| globs.binary_search(g).unwrap() as u32;
    let mut labels: Vec<[u32; 4]> = Vec::new();
    let mut label_id = |l: &Label| {
        let w = label_words(l);
        labels.iter().position(|x| *x == w).unwrap_or_else(|| {
            labels.push(w);
            labels.len() - 1
        }) as u32
    };
    let mut lines_w = Vec::new();
    let mut samples_w = Vec::new();
    for line in lines {
        let (l, s) = line_words(line, graph, input, &order, (samples_w.len() / SAMPLE) as u32);
        lines_w.extend(l);
        samples_w.extend(s);
    }
    let mut cands_w = Vec::new();
    let mut extras: Vec<u32> = Vec::new();
    // Per candidate, the labels it can give a point: its own and those past its seams.
    let mut given: Vec<Vec<u32>> = Vec::with_capacity(cands.len());
    for c in cands {
        let k = kind(c);
        let own = label_id(&c.label);
        let discs = extras.len() as u32;
        extras.extend(c.discs.iter().flat_map(|d| disc_words(d, origin.y)));
        let beyond = extras.len() as u32;
        let mut ids = vec![own];
        for (cut, l) in &c.beyond {
            let id = label_id(l);
            extras.extend(cut_words(cut));
            extras.push(id);
            ids.push(id);
        }
        given.push(ids);
        let parts_at = extras.len() as u32;
        let parts = c.relabel.as_deref().unwrap_or(&[]);
        for part in parts {
            extras.push(lines[part.line].way);
            for cut in &part.cuts {
                extras.push(u32::from(cut.is_some()));
                extras.extend(cut.as_ref().map_or([0; 4], cut_words));
            }
        }
        let key = u32::from(c.key.0) << 16 | u32::from(c.key.1) << 8 | u32::from(c.key.2);
        cands_w.extend([k, key, c.glob.as_ref().map_or(u32::MAX, order), own]);
        cands_w.push(if k == WAY { c.line as u32 } else { u32::MAX });
        cands_w.extend([discs, c.discs.len() as u32, beyond, c.beyond.len() as u32, parts_at, parts.len() as u32]);
    }
    let discs = extras.len() as u32;
    extras.extend(outline_discs.iter().flat_map(|d| disc_words(d, origin.y)));
    cands_w.extend([OUTLINE, u32::MAX, u32::MAX, u32::MAX, u32::MAX, discs, outline_discs.len() as u32, 0, 0, 0, 0]);
    // A carriageway over a rail bed is a crossing: its label's variant with the rail as `crossing`.
    let rails: Vec<u32> = (cands.iter())
        .filter(|c| c.label.surface == Surface::RailBed)
        .map(|c| c.label.material.unwrap_or(u32::MAX))
        .collect();
    let crossings: Vec<[u32; 4]> = (labels.iter())
        .filter(|l| l[0] & 0xff == Surface::Carriageway as u32)
        .flat_map(|l| rails.iter().map(move |&r| [Surface::Crossing as u32 | (l[0] & !0xff), l[1], l[2], r]))
        .collect();
    for x in crossings {
        if !labels.contains(&x) {
            labels.push(x);
        }
    }
    // A label names its material and its rail by line.
    let of_way: BTreeMap<u32, u32> = lines.iter().enumerate().map(|(i, l)| (l.way, i as u32)).collect();
    let line_of = |w: u32| of_way.get(&w).copied().unwrap_or(u32::MAX);
    for l in &mut labels {
        for k in [2, 3] {
            if l[k] != u32::MAX {
                l[k] = line_of(l[k]);
            }
        }
    }
    let four = |mut v: Vec<u32>| {
        v.retain(|&x| x != u32::MAX);
        v.sort_unstable();
        v.dedup();
        v.resize(4, u32::MAX);
        [v[0], v[1], v[2], v[3]]
    };
    // The lines whose frame the paint of a label follows: a junction's hosts; a way and the ways it
    // meets at a joint. And its other lines: a junction's guests; a way's partners at its joints.
    let joints_of = |w: u32| graph.joints.iter().filter(move |j| j.arms.iter().any(|a| a.way == w));
    let hosts: Vec<[u32; 4]> = (labels.iter())
        .map(|l| match (l[0] >> 8) & 0xff {
            JUNCTION => {
                four(graph.junctions[l[1] as usize].arms.iter().filter(|a| a.host).map(|a| line_of(a.way)).collect())
            }
            WAY_OWNER => {
                let partners = joints_of(l[1]).flat_map(|j| j.arms.iter().filter(|a| a.way != l[1]));
                four(std::iter::once(line_of(l[1])).chain(partners.map(|a| line_of(a.way))).collect())
            }
            _ => four(Vec::new()),
        })
        .collect();
    let others: Vec<[u32; 4]> = (labels.iter().zip(&hosts))
        .map(|(l, h)| match (l[0] >> 8) & 0xff {
            JUNCTION => {
                four(graph.junctions[l[1] as usize].arms.iter().filter(|a| !a.host).map(|a| line_of(a.way)).collect())
            }
            WAY_OWNER => four(h.iter().copied().filter(|&x| x != l[2]).collect()),
            _ => four(Vec::new()),
        })
        .collect();
    // The shapes in bin order: the paved outline first, then the band layers, then the rest, so a
    // point far from the paved area reads only these first.
    let shapes: Vec<&Vec<Vec<P>>> = cands.iter().map(|c| &c.rings).chain(std::iter::once(&outline)).collect();
    let mut by_order = vec![cands.len()];
    by_order.extend((0..cands.len()).filter(|&c| kind(&cands[c]) == BAND));
    by_order.extend((0..cands.len()).filter(|&c| kind(&cands[c]) != BAND));
    let n_bins = (BINS * BINS) as usize;
    let margin = units(MARGIN_M);
    let mut items: Vec<Vec<u32>> = vec![Vec::new(); n_bins];
    let mut n_items = vec![0u32; n_bins];
    let mut stats = RecordStats::default();
    for ci in by_order {
        let rings = shapes[ci];
        let bb = rings.iter().flatten().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            [b[0].min(f64::from(p[0])), b[1].min(f64::from(p[1])), b[2].max(f64::from(p[0])), b[3].max(f64::from(p[1]))]
        });
        let Some((c, r)) = bins_of(bb, margin).filter(|_| bb[0] <= bb[2]) else { continue };
        let wind = windings(rings, c, r);
        let cols = (c[1] - c[0] + 1) as usize;
        // Per bin, the edges (ring, index) that meet its square grown by the margin and a unit.
        let mut edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); wind.len()];
        for (ri, ring) in rings.iter().enumerate() {
            for (i, &a) in ring.iter().enumerate() {
                let b = ring[(i + 1) % ring.len()];
                let eb = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
                let Some((ec, er)) = bins_of(eb, margin + 1.0) else { continue };
                for row in er[0].max(r[0])..=er[1].min(r[1]) {
                    for col in ec[0].max(c[0])..=ec[1].min(c[1]) {
                        if clip(a, b, square(col, row, margin + 1.0)).is_some() {
                            edges[(row - r[0]) as usize * cols + (col - c[0]) as usize].push((ri, i));
                        }
                    }
                }
            }
        }
        for row in r[0]..=r[1] {
            for col in c[0]..=c[1] {
                let k = (row - r[0]) as usize * cols + (col - c[0]) as usize;
                if edges[k].is_empty() && wind[k] == 0 {
                    continue;
                }
                // The edges as runs of a ring's consecutive vertices.
                let mut runs: Vec<Vec<P>> = Vec::new();
                let mut last: Option<(usize, usize)> = None;
                for &(ri, i) in &edges[k] {
                    let ring = &rings[ri];
                    if last != Some((ri, i.wrapping_sub(1))) {
                        runs.push(vec![ring[i]]);
                    }
                    runs.last_mut().unwrap().push(ring[(i + 1) % ring.len()]);
                    last = Some((ri, i));
                }
                let b = (row * BINS + col) as usize;
                items[b].extend([ci as u32 | ((wind[k].clamp(-127, 127) + 128) as u32) << 16, runs.len() as u32]);
                for run in &runs {
                    items[b].push(run.len() as u32);
                    items[b].extend(run.iter().flat_map(|v| [v[0] as u32, v[1] as u32]));
                }
                n_items[b] += 1;
                stats.bin_items += 1;
                stats.bin_edges += edges[k].len() as u32;
            }
        }
    }
    // Centre-line segments within their half-width and LINE_PAD_M of a bin.
    let mut segs: Vec<Vec<u32>> = vec![Vec::new(); n_bins];
    let pad = units(LINE_PAD_M);
    let seg_box_of = |a: P, b: P| [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
    for (li, line) in lines.iter().enumerate() {
        for i in 0..line.p.len().saturating_sub(1) {
            let (a, b) = (line.p[i], line.p[i + 1]);
            let reach = line.h[i].max(line.h[i + 1]) + pad;
            let Some((c, r)) = bins_of(seg_box_of(a, b), reach) else { continue };
            for row in r[0]..=r[1] {
                for col in c[0]..=c[1] {
                    if clip(a, b, square(col, row, reach)).is_some() {
                        segs[(row * BINS + col) as usize].push(li as u32 | (i as u32) << 16);
                    }
                }
            }
        }
    }
    // The paint reads a label's host lines, its other lines and its material line at their segment
    // nearest the point: in every bin that a point's, region's or round's label reaches, each set's
    // segments that can be nearest to a point of the bin. A segment farther from the bin than the
    // set's nearest one plus the bin's diagonal is nearer to no point of it.
    let diag = f64::from(BIN_UNITS) * std::f64::consts::SQRT_2;
    for b in 0..n_bins {
        let mut sets: Vec<[u32; 4]> = Vec::new();
        let mut at = 0;
        while at < items[b].len() {
            let ci = (items[b][at] & 0xffff) as usize;
            at = skip_item(&items[b], at);
            if !cands.get(ci).is_some_and(|c| c.label.owner != Owner::Terrain && kind(c) != WAY) {
                continue;
            }
            for &l in &given[ci] {
                let own = [labels[l as usize][2], u32::MAX, u32::MAX, u32::MAX];
                for h in [hosts[l as usize], others[l as usize], own] {
                    if h[0] != u32::MAX && !sets.contains(&h) {
                        sets.push(h);
                    }
                }
            }
        }
        let (col, row) = (b as i32 % BINS, b as i32 / BINS);
        let bx = square(col, row, 0.0);
        let mut wanted: Vec<u32> = Vec::new();
        for h in sets {
            let found: Vec<(u32, usize, f64)> = (h.iter().filter(|&&l| l != u32::MAX))
                .flat_map(|&l| {
                    let p = &lines[l as usize].p;
                    (0..p.len().saturating_sub(1)).map(move |i| (l, i, seg_box(p[i], p[i + 1], bx)))
                })
                .collect();
            let near = found.iter().map(|s| s.2).fold(f64::INFINITY, f64::min);
            wanted.extend(found.iter().filter(|s| s.2 <= near + diag).map(|&(l, i, _)| l | (i as u32) << 16));
        }
        wanted.retain(|w| !segs[b].contains(w));
        wanted.sort_unstable();
        wanted.dedup();
        segs[b].append(&mut wanted);
    }
    // The heights' segments within the slope's reach and HEIGHT_PAD_M of a bin, and the discs whose
    // circle grown by the greatest such reach meets a bin.
    let mut hsegs: Vec<Vec<u32>> = vec![Vec::new(); n_bins];
    let mut hdiscs: Vec<Vec<u32>> = vec![Vec::new(); n_bins];
    let hpad = units(HEIGHT_PAD_M);
    let mut reach_max = 0.0f64;
    for (li, line) in lines.iter().enumerate() {
        for i in 0..line.p.len().saturating_sub(1) {
            let (a, b) = (line.p[i], line.p[i + 1]);
            let reach = line.bands[i][3].max(line.bands[i + 1][3]) + hpad;
            reach_max = reach_max.max(reach);
            let Some((c, r)) = bins_of(seg_box_of(a, b), reach) else { continue };
            for row in r[0]..=r[1] {
                for col in c[0]..=c[1] {
                    if clip(a, b, square(col, row, reach)).is_some() {
                        hsegs[(row * BINS + col) as usize].push(li as u32 | (i as u32) << 16);
                    }
                }
            }
        }
    }
    for (ci, cand) in cands.iter().enumerate() {
        for (di, d) in cand.discs.iter().enumerate() {
            let (x, y, reach) = (f64::from(d.centre[0]), f64::from(d.centre[1]), d.radius + reach_max);
            let Some((c, r)) = bins_of([x, y, x, y], reach) else { continue };
            for row in r[0]..=r[1] {
                for col in c[0]..=c[1] {
                    let bx = square(col, row, 0.0);
                    let (dx, dy) = ((bx[0] - x).max(x - bx[2]).max(0.0), (bx[1] - y).max(y - bx[3]).max(0.0));
                    if libm::hypot(dx, dy) <= reach {
                        hdiscs[(row * BINS + col) as usize].push(ci as u32 | (di as u32) << 16);
                    }
                }
            }
        }
    }
    let mut words = vec![0u32; HEAD];
    words[0] = MAGIC;
    words[1] = shapes.len() as u32;
    words[2] = labels.len() as u32;
    words[3] = lines.len() as u32;
    words.extend(rules.plan.heights.words());
    words[4] = words.len() as u32;
    words.extend(&cands_w);
    words[5] = words.len() as u32;
    for ((l, h), o) in labels.iter().zip(&hosts).zip(&others) {
        words.extend(l.iter().chain(h).chain(o));
    }
    words[6] = words.len() as u32;
    words.extend(&lines_w);
    words[7] = words.len() as u32;
    words.extend(&samples_w);
    words[8] = words.len() as u32;
    words.extend(&extras);
    words[9] = words.len() as u32;
    words[10] = (origin.y / CELL_UNITS) as u32;
    let heads = words.len();
    words.resize(heads + HEADER * n_bins, 0);
    for b in 0..n_bins {
        if items[b].is_empty() && segs[b].is_empty() && hsegs[b].is_empty() && hdiscs[b].is_empty() {
            continue;
        }
        let before = words.len();
        words[heads + HEADER * b] = before as u32;
        words[heads + HEADER * b + 1] = n_items[b] << 16 | segs[b].len() as u32;
        words[heads + HEADER * b + 2] = (hsegs[b].len() as u32) << 16 | hdiscs[b].len() as u32;
        words.extend(items[b].iter().chain(&segs[b]).chain(&hsegs[b]).chain(&hdiscs[b]));
        stats.bins_used += 1;
        stats.max_bin_words = stats.max_bin_words.max((words.len() - before) as u32);
    }
    stats.cands = shapes.len() as u32;
    stats.labels = labels.len() as u32;
    stats.lines = lines.len() as u32;
    stats.samples = (samples_w.len() / SAMPLE) as u32;
    Records { cell, words, stats }
}

/// The words of label `id` of a block.
pub fn label(words: &[u32], id: u32) -> &[u32] {
    &words[words[5] as usize + LABEL * id as usize..][..LABEL]
}

fn cand(words: &[u32], ci: u32) -> &[u32] {
    &words[words[4] as usize + CAND * ci as usize..][..CAND]
}

/// The word after the candidate item at `at`.
pub(crate) fn skip_item(words: &[u32], mut at: usize) -> usize {
    let runs = words[at + 1];
    at += 2;
    for _ in 0..runs {
        at += 1 + 2 * words[at] as usize;
    }
    at
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Whether point p moved by (ε, ε²) lies left of the line through a along e.
fn left(e: [f64; 2], a: [f64; 2], p: [f64; 2]) -> bool {
    let s = cross(e, [p[0] - a[0], p[1] - a[1]]);
    if s != 0.0 {
        return s > 0.0;
    }
    e[1] < 0.0 || (e[1] == 0.0 && e[0] > 0.0)
}

/// The change of the winding number from c to q, both moved by (ε, ε²), across edge ab: +1 where q
/// enters the side on the edge's left. In doubled units.
fn crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], q: [f64; 2]) -> i32 {
    let e = [b[0] - a[0], b[1] - a[1]];
    let (sc, sq) = (left(e, a, c), left(e, a, q));
    if sc == sq {
        return 0;
    }
    // The ends of the edge against the moved segment: v − (c + (ε, ε²)) across d.
    let d = [q[0] - c[0], q[1] - c[1]];
    let side = |v: [f64; 2]| {
        let s = cross(d, [v[0] - c[0], v[1] - c[1]]);
        if s != 0.0 {
            s > 0.0
        } else {
            d[1] > 0.0 || (d[1] == 0.0 && d[0] < 0.0)
        }
    };
    if side(a) == side(b) {
        0
    } else if sq {
        1
    } else {
        -1
    }
}

/// Whether q lies past a cut: on its right, within its length, as the overlay's `past`.
fn past(cut: &[u32], q: [f64; 2]) -> bool {
    let [a0, a1, b0, b1] = [0, 1, 2, 3].map(|k| f64::from(cut[k] as i32));
    let (u, v) = ([b0 - a0, b1 - a1], [q[0] - a0, q[1] - a1]);
    let along = u[0] * v[0] + u[1] * v[1];
    cross(u, v) < 0.0 && (0.0..=u[0] * u[0] + u[1] * u[1]).contains(&along)
}

/// The label of a block at point q of the closed cell square (frame units from the cell's origin),
/// none for terrain: the overlay's decision over the candidates that cover q. A point on an edge
/// reads as moved by (ε, ε²) in frame axes, so every cell breaks a tie alike; the tests are exact
/// for q on a grid of 1/1024 unit. A fan or round leaves out the disc of its circle: its arc is the
/// true circle where the overlay's is its chords.
pub fn find(words: &[u32], q: [f64; 2]) -> Option<u32> {
    let cell = f64::from(CELL_UNITS as i32);
    if !(0.0..=cell).contains(&q[0]) || !(0.0..=cell).contains(&q[1]) {
        return None;
    }
    let bin = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).min(BINS - 1);
    let (col, row) = (bin(q[0]), bin(q[1]));
    let head = words[9] as usize + HEADER * (row * BINS + col) as usize;
    let (mut at, n_cands, n_segs) = (words[head] as usize, (words[head + 1] >> 16) as usize, words[head + 1] & 0xffff);
    let c = [2.0 * f64::from(reference(col)) + 1.0, 2.0 * f64::from(reference(row)) + 1.0];
    let q2 = [2.0 * q[0], 2.0 * q[1]];
    let extras = &words[words[8] as usize..];
    let mut covered = Vec::new();
    for _ in 0..n_cands {
        let ci = words[at] & 0xffff;
        let mut w = ((words[at] >> 16) & 0xff) as i32 - 128;
        let runs = words[at + 1];
        at += 2;
        for _ in 0..runs {
            let n = words[at] as usize;
            let v = |k: usize| {
                [2.0 * f64::from(words[at + 1 + 2 * k] as i32), 2.0 * f64::from(words[at + 2 + 2 * k] as i32)]
            };
            for k in 0..n.saturating_sub(1) {
                w += crossing(v(k), v(k + 1), c, q2);
            }
            at += 1 + 2 * n;
        }
        let k = cand(words, ci);
        let in_disc = (0..k[6] as usize).any(|d| {
            let e = &extras[k[5] as usize + DISC * d..];
            libm::hypot(q[0] - f64::from(e[0] as i32), q[1] - f64::from(e[1] as i32)) < f64::from(f32::from_bits(e[2]))
        });
        if w != 0 && !in_disc {
            covered.push(ci);
        }
    }
    decide(words, &covered, q, &words[at..at + n_segs as usize])
}

/// The glob order of a line at q: of its sample nearest q among the bin's segments.
fn way_glob(words: &[u32], line: u32, q: [f64; 2], segs: &[u32]) -> u32 {
    let first = words[words[6] as usize + LINE * line as usize + 2] as usize;
    let mut best = (f64::INFINITY, u32::MAX);
    for &s in segs.iter().filter(|&&s| s & 0xffff == line) {
        for i in [(s >> 16) as usize, (s >> 16) as usize + 1] {
            let w = &words[words[7] as usize + SAMPLE * (first + i)..];
            let (dx, dy) = (q[0] - f64::from(w[0] as i32), q[1] - f64::from(w[1] as i32));
            if dx * dx + dy * dy < best.0 {
                best = (dx * dx + dy * dy, w[4]);
            }
        }
    }
    best.1
}

fn decide(words: &[u32], covered: &[u32], q: [f64; 2], segs: &[u32]) -> Option<u32> {
    let extras = &words[words[8] as usize..];
    let glob = |ci: u32| {
        let k = cand(words, ci);
        if k[0] == WAY {
            way_glob(words, k[4], q, segs)
        } else {
            k[2]
        }
    };
    let rest: Vec<u32> = covered.iter().copied().filter(|&c| !matches!(cand(words, c)[0], REGION | OUTLINE)).collect();
    let best = rest.iter().map(|&c| cand(words, c)[1]).min()?;
    let win = rest.iter().copied().filter(|&c| cand(words, c)[1] == best).min_by_key(|&c| glob(c))?;
    let k = cand(words, win);
    if k[0] == FILL {
        return Some(k[3]);
    }
    let across = (0..k[8] as usize).map(|b| &extras[k[7] as usize + BEYOND * b..][..BEYOND]).find(|e| past(e, q));
    let mut id = across.map_or(k[3], |e| e[4]);
    let l = label(words, id);
    if (l[0] >> 8) & 0xff == WAY_OWNER && across.is_none() {
        let w = l[1];
        let outside = |p: &[u32]| !(p[1] == 1 && past(&p[2..6], q)) && !(p[6] == 1 && past(&p[7..11], q));
        let region = (covered.iter().copied())
            .filter(|&c| cand(words, c)[0] == REGION)
            .filter(|&c| {
                let r = cand(words, c);
                let parts = (0..r[10] as usize).map(|p| &extras[r[9] as usize + PART * p..][..PART]);
                let mine: Vec<&[u32]> = parts.filter(|p| p[0] == w).collect();
                !mine.is_empty() && mine.iter().all(|p| outside(p))
            })
            .min_by_key(|&c| (cand(words, c)[1], cand(words, c)[2]));
        if let Some(r) = region {
            id = cand(words, r)[3];
        }
    }
    let l = label(words, id);
    let rail = rest.iter().find(|&&c| label(words, cand(words, c)[3])[0] & 0xff == Surface::RailBed as u32);
    if let (true, Some(&r)) = (l[0] & 0xff == Surface::Carriageway as u32, rail) {
        let want = [Surface::Crossing as u32 | (l[0] & !0xff), l[1], l[2], label(words, cand(words, r)[3])[2]];
        if let Some(x) = (0..words[2]).find(|&x| label(words, x)[..4] == want) {
            id = x;
        }
    }
    Some(id)
}
