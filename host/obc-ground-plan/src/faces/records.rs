//! The far sampling of the one road model: a cell's candidates as records that a shader evaluates per
//! point with the overlay's own `decide`. The overlay slices the same candidates into faces, so the
//! label of a point agrees near and far by construction. Prototype bytes, not a format.
//!
//! A cell block is little-endian words. Coordinates are frame units from the cell's origin.
//!
//! | Words | Holds |
//! | --- | --- |
//! | 0..HEAD | `MAGIC`, counts and offsets (`Head`) |
//! | cands | `CAND` words each: kind, key, glob, label, line, discs, beyond, parts |
//! | labels | `LABEL` words each: surface, owner kind, rank, rule; owner id; material line; crossing line; 4 host lines; 4 other arm lines |
//! | lines | `LINE` words each: way, finish code with flags, first sample, samples, cut, fill, reach (f32), class rank, shoulder, verge, return grade, half-width (f32 m), end kinds (0 free, 1 meets, 2 level change), steepest fill and cut (f32) |
//! | samples | `SAMPLE` words each: x, y, half-width in units (f32), metres along (f32), glob, height, bank, grade (f32), terrain at the reach left and right (f32, NaN unknown) |
//! | extras | discs (`DISC`), beyond (cut, label), parts (way, two cuts with flags) |
//! | bins | `BINS`² headers of `HEADER` words, then the items: candidates, segments, height segments, discs |
//!
//! A bin item of a candidate is its index with its winding number at the bin's reference point and its
//! edge count, then its edges clipped to the bin grown by `margin`, as two i16 pairs about the bin's
//! middle. A segment item names a line and a sample: the centre line from it to the next.

use super::geom::P;
use super::{candidates, Cand, Label, Owner, Surface, WayLine};
use crate::frame::{metres_per_unit, CellKey, CELL_UNITS};
use crate::graph::WayGraph;
use crate::sections::Finish;
use crate::CellInput;

use super::Rules;

pub const MAGIC: u32 = 0x4f42_4d31;
/// Bins per side of the cell square: 4,096 frame units, about 6.5 m.
pub const BINS: i32 = 64;
pub const BIN_UNITS: i32 = CELL_UNITS as i32 / BINS;
/// A bin lists every candidate edge within this of its square: the reach of the anti-aliasing and
/// of the edge lines.
pub const MARGIN_M: f64 = 3.0;
/// A bin lists every centre-line segment within this past the way's half-width: the paint reads
/// the metres across and along of the label's way.
pub const LINE_PAD_M: f64 = 3.0;
pub const HEAD: usize = 12;
pub const CAND: usize = 12;
pub const LABEL: usize = 12;
pub const LINE: usize = 16;
pub const SAMPLE: usize = 10;
/// Words of a disc: centre, radius, the two feet, the shoulder ring at each foot, the two arm lines.
pub const DISC: usize = 11;
/// Words of a bin header: offset, candidate items << 16 | segment items, height segment items << 16 | disc items.
pub const HEADER: usize = 3;
/// The heights read a line's segments and the discs within its slope reach and this past a bin.
pub const HEIGHT_PAD_M: f64 = 1.0;

/// Candidate kinds.
pub const WAY: u32 = 0;
pub const POINT: u32 = 1;
pub const REGION: u32 = 2;
pub const BAND: u32 = 3;
pub const FILL: u32 = 4;
/// The paved outline: the union of the paved candidates, for the distance to the road's edge. It takes
/// no part in the decision.
pub const OUTLINE: u32 = 5;

/// Line flags above the finish code's byte.
pub const MARKED: u32 = 1 << 8;
pub const RAIL: u32 = 1 << 9;

/// Owner kinds.
pub const TERRAIN: u32 = 0;
pub const WAY_OWNER: u32 = 1;
pub const JUNCTION: u32 = 2;

#[derive(Clone, Debug, Default)]
pub struct RecordStats {
    pub cands: u32,
    pub labels: u32,
    pub lines: u32,
    pub samples: u32,
    pub rings_vertices: u32,
    pub bin_items: u32,
    pub bin_edges: u32,
    pub bins_used: u32,
    pub words: u32,
    /// The most items and edges of one bin.
    pub max_bin_words: u32,
}

pub struct Records {
    pub cell: CellKey,
    pub words: Vec<u32>,
    pub stats: RecordStats,
}

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// The finish code of the viewer's looks: `Finish` in order from 1.
fn code(finish: Finish) -> u32 {
    Finish::ALL.iter().position(|&x| x == finish).map_or(0, |i| i as u32 + 1)
}

/// The classes that wear lines, as `marked` of bins.ts.
const LINED: [&str; 10] = [
    "motorway",
    "motorway_link",
    "trunk",
    "trunk_link",
    "primary",
    "primary_link",
    "secondary",
    "secondary_link",
    "tertiary",
    "tertiary_link",
];

fn label_word(l: &Label) -> [u32; 4] {
    let (kind, id) = match l.owner {
        Owner::Terrain => (TERRAIN, 0),
        Owner::Way(w) => (WAY_OWNER, w),
        Owner::Junction(j) => (JUNCTION, j),
    };
    [
        l.surface as u32 | kind << 8 | u32::from(l.rank) << 16 | (l.rule as u32) << 24,
        id,
        l.material.unwrap_or(u32::MAX),
        l.crossing.unwrap_or(u32::MAX),
    ]
}

/// The edges of rings clipped to a box [x0, y0, x1, y1], whole or in part.
fn clip(a: P, b: P, bx: [f64; 4]) -> Option<([f64; 2], [f64; 2])> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
    let (dx, dy) = (f64::from(b[0]) - ax, f64::from(b[1]) - ay);
    for (p, q) in [(-dx, ax - bx[0]), (dx, bx[2] - ax), (-dy, ay - bx[1]), (dy, bx[3] - ay)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                t0 = t0.max(r);
            } else {
                t1 = t1.min(r);
            }
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
        (dx * dx + dy * dy).sqrt()
    };
    let to_seg = |q: [f64; 2]| {
        let u = [fb[0] - fa[0], fb[1] - fa[1]];
        let t =
            (((q[0] - fa[0]) * u[0] + (q[1] - fa[1]) * u[1]) / (u[0] * u[0] + u[1] * u[1]).max(1e-12)).clamp(0.0, 1.0);
        let (dx, dy) = (q[0] - fa[0] - t * u[0], q[1] - fa[1] - t * u[1]);
        (dx * dx + dy * dy).sqrt()
    };
    let corners = [[bx[0], bx[1]], [bx[2], bx[1]], [bx[0], bx[3]], [bx[2], bx[3]]];
    corners.into_iter().map(to_seg).fold(to_box(fa).min(to_box(fb)), f64::min)
}

/// The windings of a candidate's rings at the reference point of each bin in [c0, c1] × [r0, r1]: per
/// row a horizontal line through the reference points, the crossings right of each.
fn windings(rings: &[Vec<P>], c0: i32, c1: i32, r0: i32, r1: i32) -> Vec<i32> {
    let cols = (c1 - c0 + 1) as usize;
    let mut out = vec![0; cols * (r1 - r0 + 1) as usize];
    for r in r0..=r1 {
        let q = [0i64, 2 * i64::from(reference(r)) + 1];
        let mut xs: Vec<(f64, i32)> = Vec::new();
        for ring in rings {
            for (i, &a) in ring.iter().enumerate() {
                let b = ring[(i + 1) % ring.len()];
                let (ay, by) = (2 * i64::from(a[1]), 2 * i64::from(b[1]));
                let up = ay <= q[1] && by > q[1];
                let down = by <= q[1] && ay > q[1];
                if up || down {
                    let t = (q[1] - ay) as f64 / (by - ay) as f64;
                    let x = 2.0 * f64::from(a[0]) + t * 2.0 * f64::from(b[0] - a[0]);
                    xs.push((x, if up { 1 } else { -1 }));
                }
            }
        }
        for c in c0..=c1 {
            let qx = 2.0 * f64::from(reference(c)) + 1.0;
            let w: i32 = xs.iter().filter(|&&(x, _)| x > qx).map(|&(_, s)| s).sum();
            out[((r - r0) as usize) * cols + (c - c0) as usize] = w;
        }
    }
    out
}

/// A bin's reference point, less half a unit: its middle.
pub fn reference(i: i32) -> i32 {
    i * BIN_UNITS + BIN_UNITS / 2
}

/// The records of one cell and level.
pub fn records(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8, slopes: bool) -> Records {
    let c = candidates(graph, input, rules, cell, level, slopes);
    encode(cell, graph, input, &c.lines, &c.cands)
}

/// Whether a candidate paves: a carriageway of a road or a path, a point's fan, an outline round of a
/// carriageway's label.
fn paves(c: &Cand) -> bool {
    c.relabel.is_none()
        && c.label.surface == Surface::Carriageway
        && !(c.glob.is_some() && c.label.owner == Owner::Terrain && c.fill.is_none())
}

fn encode(cell: CellKey, graph: &WayGraph, input: &CellInput, lines: &[WayLine], cands: &[Cand]) -> Records {
    // The paved outline, with the circles of its fans and rounds: one union, so the distance to the
    // road's edge has no seam inside the paved area.
    let paved: Vec<Vec<P>> = cands.iter().filter(|c| paves(c)).flat_map(|c| c.rings.iter().cloned()).collect();
    let outline_rings = super::stroke::union(&paved, [0, 0]);
    let outline_discs: Vec<super::points::Arc> =
        cands.iter().filter(|c| paves(c)).flat_map(|c| c.discs.iter().copied()).collect();
    let shapes: Vec<(&Vec<Vec<P>>, &[super::points::Arc])> = cands
        .iter()
        .map(|c| (&c.rings, c.discs.as_slice()))
        .chain(std::iter::once((&outline_rings, outline_discs.as_slice())))
        .collect();
    let origin = cell.origin();
    let mpu = metres_per_unit(origin.y + CELL_UNITS / 2);
    let units = |m: f64| m / mpu;
    // The global names in order: a way's at its samples, a point's at its position.
    let mut globs: Vec<[i64; 5]> =
        lines.iter().flat_map(|l| l.key.iter().copied()).chain(cands.iter().filter_map(|c| c.glob)).collect();
    globs.sort_unstable();
    globs.dedup();
    let order = |g: &[i64; 5]| globs.binary_search(g).unwrap() as u32;
    let mut labels: Vec<[u32; 4]> = Vec::new();
    // The lines whose middle the paint of a label follows: a junction's hosts, a way and the ways it
    // meets at a joint.
    let mut label_id = |l: &Label| {
        let w = label_word(l);
        match labels.iter().position(|x| *x == w) {
            Some(i) => i as u32,
            None => {
                labels.push(w);
                labels.len() as u32 - 1
            }
        }
    };
    let line_of_way = |w: u32| lines.iter().position(|l| l.way == w).map_or(u32::MAX, |i| i as u32);
    // Lines and their samples.
    let mut line_words = Vec::new();
    let mut sample_words = Vec::new();
    for line in lines {
        let way = &graph.ways[line.way as usize];
        let part = &graph.parts[way.part as usize];
        let tags = &input.features[part.feature.0 as usize].tags;
        let surface = tags.get("surface").map(String::as_str).unwrap_or("");
        let setts = ["sett", "cobblestone", "unhewn_cobblestone", "stone", "paving_stones"].contains(&surface)
            || surface.starts_with("concrete");
        let paved = matches!(line.sec.finish, Finish::Main | Finish::Road | Finish::Oneway);
        let marked = LINED.contains(&line.sec.class) && paved && !setts;
        let flags = code(line.sec.finish) | if marked { MARKED } else { 0 } | if line.rail { RAIL } else { 0 };
        line_words.extend([line.way, flags, (sample_words.len() / SAMPLE) as u32, line.p.len() as u32]);
        line_words.extend([
            f(line.sec.cut as f32),
            f(line.sec.fill as f32),
            f(line.sec.reach_m as f32),
            line.sec.rank as u32,
            f(line.sec.shoulder_m as f32),
            f(line.sec.verge_m as f32),
            f(line.sec.return_grade as f32),
            f(line.sec.half_m as f32),
        ]);
        // How each end meets the rest: 0 a free (round) end, 1 at a junction or joint (square, others go
        // on), 2 at a level change (square, nothing goes on on the ground).
        let meets = |dir: i8, st: &crate::graph::Station| {
            let key = (line.way, dir, st.span, st.u.to_bits());
            let arm = |a: &crate::graph::Arm| (a.way, a.dir, a.at.span, a.at.u.to_bits()) == key;
            graph.junctions.iter().any(|j| j.arms.iter().any(arm))
                || graph.joints.iter().any(|j| j.arms.iter().any(arm))
        };
        for (k, (dir, st)) in [(1i8, &way.from), (-1i8, &way.to)].into_iter().enumerate() {
            line_words.push(if line.round[k] {
                0
            } else if meets(dir, st) {
                1
            } else {
                2
            });
        }
        line_words.extend([f(line.sec.fill_max as f32), f(line.sec.cut_max as f32)]);
        let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
        for (i, &p) in line.p.iter().enumerate() {
            sample_words.extend([p[0] as u32, p[1] as u32, f(line.h[i] as f32), f(samples[i].along_m as f32)]);
            sample_words.push(order(&line.key[i]));
            sample_words.extend([f(samples[i].z_m as f32), f(samples[i].bank as f32), f(samples[i].grade as f32)]);
            // The plain terrain at the slope's reach on each side, left first: NaN where unknown.
            let t = samples[i].tangent;
            let len = (t[0] * t[0] + t[1] * t[1]).sqrt().max(1e-12);
            let n = [-t[1] / len, t[0] / len];
            let reach = line.bands[i][3];
            for side in [1.0, -1.0] {
                let g = crate::frame::GlobalPoint {
                    x: origin.x + i64::from(p[0]) + libm::round(side * reach * n[0]) as i64,
                    y: origin.y + i64::from(p[1]) + libm::round(side * reach * n[1]) as i64,
                };
                sample_words.push(f(input.terrain.height(g).map_or(f32::NAN, |h| h as f32)));
            }
        }
    }
    // Candidates and their extras.
    let mut cand_words = Vec::new();
    let mut extras: Vec<u32> = Vec::new();
    let cut_words = |cut: &[P; 2]| [cut[0][0] as u32, cut[0][1] as u32, cut[1][0] as u32, cut[1][1] as u32];
    let mut stats = RecordStats::default();
    // Per candidate, the labels it can give a point: its own and those past its seams.
    let mut cand_labels: Vec<Vec<u32>> = Vec::with_capacity(cands.len());
    for cand in cands {
        let kind = if cand.relabel.is_some() {
            REGION
        } else if cand.fill.is_some() {
            FILL
        } else if cand.glob.is_none() {
            WAY
        } else if cand.label.owner == Owner::Terrain {
            BAND
        } else {
            POINT
        };
        let key = u32::from(cand.key.0) << 16 | u32::from(cand.key.1) << 8 | u32::from(cand.key.2);
        let glob = cand.glob.as_ref().map_or(u32::MAX, order);
        let label = label_id(&cand.label);
        let mut given = vec![label];
        let disc_at = extras.len() as u32;
        for d in &cand.discs {
            let line = |l: usize| if l == usize::MAX { u32::MAX } else { l as u32 };
            extras.extend([d.centre[0] as u32, d.centre[1] as u32, f(d.radius as f32)]);
            extras.extend([d.feet[0][0] as u32, d.feet[0][1] as u32, d.feet[1][0] as u32, d.feet[1][1] as u32]);
            extras.extend([
                f((d.ring[0] * mpu) as f32),
                f((d.ring[1] * mpu) as f32),
                line(d.lines[0]),
                line(d.lines[1]),
            ]);
        }
        let beyond_at = extras.len() as u32;
        for (cut, l) in &cand.beyond {
            extras.extend(cut_words(cut));
            let id = label_id(l);
            extras.push(id);
            given.push(id);
        }
        cand_labels.push(given);
        let parts_at = extras.len() as u32;
        let parts = cand.relabel.as_deref().unwrap_or(&[]);
        for part in parts {
            extras.push(lines[part.line].way);
            for cut in &part.cuts {
                match cut {
                    Some(cut) => {
                        extras.push(1);
                        extras.extend(cut_words(cut));
                    }
                    None => extras.extend([0; 5]),
                }
            }
        }
        cand_words.extend([
            kind,
            key,
            glob,
            label,
            if kind == WAY { cand.line as u32 } else { u32::MAX },
            disc_at,
            cand.discs.len() as u32,
            beyond_at,
            cand.beyond.len() as u32,
            parts_at,
            parts.len() as u32,
            0,
        ]);
        stats.rings_vertices += cand.rings.iter().map(|r| r.len() as u32).sum::<u32>();
    }
    let disc_at = extras.len() as u32;
    for d in &outline_discs {
        extras.extend([d.centre[0] as u32, d.centre[1] as u32, f(d.radius as f32)]);
        extras.extend([0; 8]);
    }
    cand_words.extend([
        OUTLINE,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        u32::MAX,
        disc_at,
        outline_discs.len() as u32,
        0,
        0,
        0,
        0,
        0,
    ]);
    stats.rings_vertices += outline_rings.iter().map(|r| r.len() as u32).sum::<u32>();
    // A carriageway over a rail bed is a crossing: its label's variant with the rail as `crossing`.
    let crossings: Vec<[u32; 4]> = labels
        .iter()
        .filter(|l| l[0] & 0xff == Surface::Carriageway as u32)
        .flat_map(|l| {
            cands.iter().filter(|c| c.label.surface == Surface::RailBed).map(move |r| {
                [Surface::Crossing as u32 | (l[0] & !0xff), l[1], l[2], r.label.material.unwrap_or(u32::MAX)]
            })
        })
        .collect();
    for x in crossings {
        if !labels.contains(&x) {
            labels.push(x);
        }
    }
    // A label names its material and its rail by line, for the looks and the paint.
    for l in &mut labels {
        for k in [2, 3] {
            if l[k] != u32::MAX {
                l[k] = line_of_way(l[k]);
            }
        }
    }
    let hosts: Vec<[u32; 4]> = labels
        .iter()
        .map(|l| {
            let mut h: Vec<u32> = match (l[0] >> 8) & 0xff {
                JUNCTION => {
                    graph.junctions[l[1] as usize].arms.iter().filter(|a| a.host).map(|a| line_of_way(a.way)).collect()
                }
                WAY_OWNER => {
                    let mut v = vec![line_of_way(l[1])];
                    for j in &graph.joints {
                        if j.arms.iter().any(|a| a.way == l[1]) {
                            v.extend(j.arms.iter().filter(|a| a.way != l[1]).map(|a| line_of_way(a.way)));
                        }
                    }
                    v
                }
                _ => Vec::new(),
            };
            h.retain(|&x| x != u32::MAX);
            h.sort_unstable();
            h.dedup();
            h.resize(4, u32::MAX);
            [h[0], h[1], h[2], h[3]]
        })
        .collect();
    // The lines past whose carriageways a face meets: a junction's other arms, a way's partners at its
    // joints (the look's mouth probe paints a face past its own lines with the nearest of these).
    let guests: Vec<[u32; 4]> = labels
        .iter()
        .zip(&hosts)
        .map(|(l, h)| {
            let mut g: Vec<u32> = match (l[0] >> 8) & 0xff {
                JUNCTION => {
                    graph.junctions[l[1] as usize].arms.iter().filter(|a| !a.host).map(|a| line_of_way(a.way)).collect()
                }
                WAY_OWNER => h.iter().copied().filter(|&x| x != u32::MAX && x != l[2]).collect(),
                _ => Vec::new(),
            };
            g.retain(|&x| x != u32::MAX);
            g.sort_unstable();
            g.dedup();
            g.resize(4, u32::MAX);
            [g[0], g[1], g[2], g[3]]
        })
        .collect();
    // Bins.
    let margin = units(MARGIN_M);
    let mut items: Vec<Vec<u32>> = vec![Vec::new(); (BINS * BINS) as usize];
    let mut segs: Vec<Vec<u32>> = vec![Vec::new(); (BINS * BINS) as usize];
    let bin_box = |c: i32, r: i32| {
        [
            f64::from(c * BIN_UNITS) - margin,
            f64::from(r * BIN_UNITS) - margin,
            f64::from((c + 1) * BIN_UNITS) + margin,
            f64::from((r + 1) * BIN_UNITS) + margin,
        ]
    };
    let to_bin = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).clamp(0, BINS - 1);
    // The paved outline first, then the band layers: a point far from the paved area reads only these.
    let mut order: Vec<usize> = vec![shapes.len() - 1];
    order.extend((0..cands.len()).filter(|&c| {
        cands[c].glob.is_some()
            && cands[c].label.owner == Owner::Terrain
            && cands[c].fill.is_none()
            && cands[c].relabel.is_none()
    }));
    let rest: Vec<usize> = (0..cands.len()).filter(|c| !order.contains(c)).collect();
    order.extend(rest);
    for ci in order {
        let rings = shapes[ci].0;
        let pts = rings.iter().flatten();
        let bb = pts.fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, p| {
            [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
        });
        if bb[0] > bb[2] {
            continue;
        }
        let (c0, c1) = (to_bin(f64::from(bb[0]) - margin), to_bin(f64::from(bb[2]) + margin));
        let (r0, r1) = (to_bin(f64::from(bb[1]) - margin), to_bin(f64::from(bb[3]) + margin));
        if f64::from(bb[2]) + margin < 0.0
            || f64::from(bb[0]) - margin > f64::from(CELL_UNITS as i32)
            || f64::from(bb[3]) + margin < 0.0
            || f64::from(bb[1]) - margin > f64::from(CELL_UNITS as i32)
        {
            continue;
        }
        let wind = windings(rings, c0, c1, r0, r1);
        let cols = (c1 - c0 + 1) as usize;
        // Per bin, the clipped edges.
        let mut edges: Vec<Vec<u32>> = vec![Vec::new(); cols * (r1 - r0 + 1) as usize];
        for ring in rings {
            for (i, &a) in ring.iter().enumerate() {
                let b = ring[(i + 1) % ring.len()];
                let lo = [f64::from(a[0].min(b[0])), f64::from(a[1].min(b[1]))];
                let hi = [f64::from(a[0].max(b[0])), f64::from(a[1].max(b[1]))];
                for r in to_bin(lo[1] - margin).max(r0)..=to_bin(hi[1] + margin).min(r1) {
                    for c in to_bin(lo[0] - margin).max(c0)..=to_bin(hi[0] + margin).min(c1) {
                        let Some((p, q)) = clip(a, b, bin_box(c, r)) else { continue };
                        let m = [f64::from(reference(c)), f64::from(reference(r))];
                        let pack = |v: [f64; 2]| {
                            let x = libm::round(v[0] - m[0]) as i32 as i16 as u16 as u32;
                            let y = libm::round(v[1] - m[1]) as i32 as i16 as u16 as u32;
                            x | y << 16
                        };
                        let e = &mut edges[((r - r0) as usize) * cols + (c - c0) as usize];
                        e.extend([pack(p), pack(q)]);
                    }
                }
            }
        }
        for r in r0..=r1 {
            for c in c0..=c1 {
                let k = ((r - r0) as usize) * cols + (c - c0) as usize;
                let w = wind[k];
                if edges[k].is_empty() && w == 0 {
                    continue;
                }
                let bin = &mut items[(r * BINS + c) as usize];
                bin.push(ci as u32 | ((w.clamp(-127, 127) + 128) as u32) << 16);
                bin.push((edges[k].len() / 2) as u32);
                bin.extend(&edges[k]);
                stats.bin_items += 1;
                stats.bin_edges += (edges[k].len() / 2) as u32;
            }
        }
    }
    // Centre-line segments within their half-width and LINE_PAD_M of a bin.
    let pad = units(LINE_PAD_M);
    for (li, line) in lines.iter().enumerate() {
        for i in 0..line.p.len().saturating_sub(1) {
            let (a, b) = (line.p[i], line.p[i + 1]);
            let reach = line.h[i].max(line.h[i + 1]) + pad;
            let lo = [f64::from(a[0].min(b[0])) - reach, f64::from(a[1].min(b[1])) - reach];
            let hi = [f64::from(a[0].max(b[0])) + reach, f64::from(a[1].max(b[1])) + reach];
            if hi[0] < 0.0
                || hi[1] < 0.0
                || lo[0] > f64::from(CELL_UNITS as i32)
                || lo[1] > f64::from(CELL_UNITS as i32)
            {
                continue;
            }
            for r in to_bin(lo[1])..=to_bin(hi[1]) {
                for c in to_bin(lo[0])..=to_bin(hi[0]) {
                    // The segment's distance to the bin's square.
                    let bx = bin_box(c, r);
                    let grown = [
                        bx[0] + margin - reach,
                        bx[1] + margin - reach,
                        bx[2] - margin + reach,
                        bx[3] - margin + reach,
                    ];
                    if clip(a, b, grown).is_some() {
                        segs[(r * BINS + c) as usize].push(li as u32 | (i as u32) << 16);
                    }
                }
            }
        }
    }
    // The paint reads a label's host lines, its other lines and its own line at their segment nearest the
    // point: in every bin that a candidate's label reaches, each set's segments that can be nearest to a
    // point of the bin. A segment
    // farther from the bin than the nearest one plus the bin's diagonal is nearer to no point of it.
    let diag = f64::from(BIN_UNITS) * std::f64::consts::SQRT_2;
    let mut wanted: Vec<u32> = Vec::new();
    for b in 0..(BINS * BINS) as usize {
        let mut host_sets: Vec<[u32; 4]> = Vec::new();
        let mut at = 0;
        while at < items[b].len() {
            let ci = (items[b][at] & 0xffff) as usize;
            at += 2 + 2 * items[b][at + 1] as usize;
            // A way's stroke holds its points within its half-width of its own line, which the bin
            // lists already; its partners matter only on the faces of its joints, which are points.
            let c = cands.get(ci);
            if c.is_none_or(|c| {
                c.label.owner == Owner::Terrain || (c.glob.is_none() && c.relabel.is_none() && c.fill.is_none())
            }) {
                continue;
            }
            for &l in &cand_labels[ci] {
                let own = [labels[l as usize][2], u32::MAX, u32::MAX, u32::MAX];
                for h in [hosts[l as usize], guests[l as usize], own] {
                    if h[0] != u32::MAX && !host_sets.contains(&h) {
                        host_sets.push(h);
                    }
                }
            }
        }
        if host_sets.is_empty() {
            continue;
        }
        let (c, r) = (b as i32 % BINS, b as i32 / BINS);
        let bx = [
            f64::from(c * BIN_UNITS),
            f64::from(r * BIN_UNITS),
            f64::from((c + 1) * BIN_UNITS),
            f64::from((r + 1) * BIN_UNITS),
        ];
        for h in host_sets {
            let segments = h.iter().filter(|&&l| l != u32::MAX).flat_map(|&l| {
                let p = &lines[l as usize].p;
                (0..p.len().saturating_sub(1)).map(move |i| (l, i, seg_box(p[i], p[i + 1], bx)))
            });
            let found: Vec<(u32, usize, f64)> = segments.collect();
            let near = found.iter().map(|s| s.2).fold(f64::INFINITY, f64::min);
            wanted.extend(found.iter().filter(|s| s.2 <= near + diag).map(|&(l, i, _)| l | (i as u32) << 16));
        }
        wanted.retain(|w| !segs[b].contains(w));
        wanted.sort_unstable();
        wanted.dedup();
        segs[b].append(&mut wanted);
    }
    // The heights' segments: within the slope's reach and HEIGHT_PAD_M of a bin; and the discs whose
    // circle grown by the greatest reach does.
    let hpad = units(HEIGHT_PAD_M);
    let mut hsegs: Vec<Vec<u32>> = vec![Vec::new(); (BINS * BINS) as usize];
    let mut hdiscs: Vec<Vec<u32>> = vec![Vec::new(); (BINS * BINS) as usize];
    let square = |c: i32, r: i32| {
        [
            f64::from(c * BIN_UNITS),
            f64::from(r * BIN_UNITS),
            f64::from((c + 1) * BIN_UNITS),
            f64::from((r + 1) * BIN_UNITS),
        ]
    };
    let mut reach_max = 0.0f64;
    for (li, line) in lines.iter().enumerate() {
        for i in 0..line.p.len().saturating_sub(1) {
            let (a, b) = (line.p[i], line.p[i + 1]);
            let reach = line.bands[i][3].max(line.bands[i + 1][3]) + hpad;
            reach_max = reach_max.max(reach);
            let lo = [f64::from(a[0].min(b[0])) - reach, f64::from(a[1].min(b[1])) - reach];
            let hi = [f64::from(a[0].max(b[0])) + reach, f64::from(a[1].max(b[1])) + reach];
            if hi[0] < 0.0
                || hi[1] < 0.0
                || lo[0] > f64::from(CELL_UNITS as i32)
                || lo[1] > f64::from(CELL_UNITS as i32)
            {
                continue;
            }
            for r in to_bin(lo[1])..=to_bin(hi[1]) {
                for c in to_bin(lo[0])..=to_bin(hi[0]) {
                    let bx = square(c, r);
                    if clip(a, b, [bx[0] - reach, bx[1] - reach, bx[2] + reach, bx[3] + reach]).is_some() {
                        hsegs[(r * BINS + c) as usize].push(li as u32 | (i as u32) << 16);
                    }
                }
            }
        }
    }
    for (ci, cand) in cands.iter().enumerate() {
        for (di, d) in cand.discs.iter().enumerate() {
            let reach = d.radius + reach_max;
            let (x, y) = (f64::from(d.centre[0]), f64::from(d.centre[1]));
            if x + reach < 0.0
                || y + reach < 0.0
                || x - reach > f64::from(CELL_UNITS as i32)
                || y - reach > f64::from(CELL_UNITS as i32)
            {
                continue;
            }
            for r in to_bin(y - reach)..=to_bin(y + reach) {
                for c in to_bin(x - reach)..=to_bin(x + reach) {
                    let bx = square(c, r);
                    let (dx, dy) = ((bx[0] - x).max(x - bx[2]).max(0.0), (bx[1] - y).max(y - bx[3]).max(0.0));
                    if (dx * dx + dy * dy).sqrt() <= reach {
                        hdiscs[(r * BINS + c) as usize].push(ci as u32 | (di as u32) << 16);
                    }
                }
            }
        }
    }
    // The block.
    let mut words = vec![0u32; HEAD];
    words[0] = MAGIC;
    words[1] = shapes.len() as u32;
    words[2] = labels.len() as u32;
    words[3] = lines.len() as u32;
    words[4] = words.len() as u32;
    words.extend(&cand_words);
    words[5] = words.len() as u32;
    for ((l, h), g) in labels.iter().zip(&hosts).zip(&guests) {
        words.extend(l);
        words.extend(h);
        words.extend(g);
    }
    words[6] = words.len() as u32;
    words.extend(&line_words);
    words[7] = words.len() as u32;
    words.extend(&sample_words);
    words[8] = words.len() as u32;
    words.extend(&extras);
    words[9] = words.len() as u32;
    let heads = words.len();
    words.extend(std::iter::repeat_n(0u32, HEADER * (BINS * BINS) as usize));
    for b in 0..(BINS * BINS) as usize {
        if items[b].is_empty() && hsegs[b].is_empty() {
            continue;
        }
        let n = items[b].iter().len();
        let cand_count = {
            let mut k = 0;
            let mut at = 0;
            while at < n {
                k += 1;
                at += 2 + 2 * items[b][at + 1] as usize;
            }
            k
        };
        words[heads + HEADER * b] = words.len() as u32;
        words[heads + HEADER * b + 1] = (cand_count as u32) << 16 | segs[b].len() as u32;
        words[heads + HEADER * b + 2] = (hsegs[b].len() as u32) << 16 | hdiscs[b].len() as u32;
        let before = words.len();
        words.extend(&items[b]);
        words.extend(&segs[b]);
        words.extend(&hsegs[b]);
        words.extend(&hdiscs[b]);
        stats.bins_used += 1;
        stats.max_bin_words = stats.max_bin_words.max((words.len() - before) as u32);
    }
    words[10] = f(mpu as f32);
    stats.cands = shapes.len() as u32;
    stats.labels = labels.len() as u32;
    stats.lines = lines.len() as u32;
    stats.samples = (sample_words.len() / SAMPLE) as u32;
    stats.words = words.len() as u32;
    Records { cell, words, stats }
}

/// What the far sampling finds at a point: the label (an index of the block's labels), or none for
/// terrain, with the candidates that cover the point.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub label: Option<u32>,
    pub covered: Vec<u32>,
}

fn i16_pair(w: u32) -> [f64; 2] {
    [f64::from(w as u16 as i16), f64::from((w >> 16) as u16 as i16)]
}

fn cross2(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// The change of the winding number from c to q across edge ab: +1 where q enters the side on the
/// edge's left, half-open on both lines.
pub fn crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], q: [f64; 2]) -> i32 {
    let e = [b[0] - a[0], b[1] - a[1]];
    let (sc, sq) = (cross2(e, [c[0] - a[0], c[1] - a[1]]) > 0.0, cross2(e, [q[0] - a[0], q[1] - a[1]]) > 0.0);
    if sc == sq {
        return 0;
    }
    let s = [q[0] - c[0], q[1] - c[1]];
    if (cross2(s, [a[0] - c[0], a[1] - c[1]]) > 0.0) == (cross2(s, [b[0] - c[0], b[1] - c[1]]) > 0.0) {
        return 0;
    }
    if sq {
        1
    } else {
        -1
    }
}

/// Whether q lies past a cut: on its right, within its length.
fn past(cut: &[u32], q: [f64; 2]) -> bool {
    let a = [f64::from(cut[0] as i32), f64::from(cut[1] as i32)];
    let b = [f64::from(cut[2] as i32), f64::from(cut[3] as i32)];
    let u = [b[0] - a[0], b[1] - a[1]];
    let v = [q[0] - a[0], q[1] - a[1]];
    let along = u[0] * v[0] + u[1] * v[1];
    cross2(u, v) < 0.0 && along >= 0.0 && along <= u[0] * u[0] + u[1] * u[1]
}

/// The label at point q (frame units from the cell's origin) of a cell block: the decision of the
/// overlay over the candidates that cover q.
pub fn find(words: &[u32], q: [f64; 2]) -> Found {
    let none = Found { label: None, covered: Vec::new() };
    let (col, row) = ((q[0] / f64::from(BIN_UNITS)).floor() as i32, (q[1] / f64::from(BIN_UNITS)).floor() as i32);
    if !(0..BINS).contains(&col) || !(0..BINS).contains(&row) {
        return none;
    }
    let heads = words[9] as usize;
    let b = (row * BINS + col) as usize;
    let (mut at, counts) = (words[heads + HEADER * b] as usize, words[heads + HEADER * b + 1]);
    let (n_cands, n_segs) = ((counts >> 16) as usize, (counts & 0xffff) as usize);
    if n_cands == 0 {
        return none;
    }
    let m = [f64::from(reference(col)), f64::from(reference(row))];
    let rq = [q[0] - m[0], q[1] - m[1]];
    let c = [0.5, 0.5];
    let cand = |ci: usize| &words[words[4] as usize + ci * CAND..][..CAND];
    let mut covered = Vec::new();
    for _ in 0..n_cands {
        let ci = (words[at] & 0xffff) as usize;
        let mut w = ((words[at] >> 16) & 0xff) as i32 - 128;
        let n = words[at + 1] as usize;
        for e in 0..n {
            let (a, bb) = (i16_pair(words[at + 2 + 2 * e]), i16_pair(words[at + 3 + 2 * e]));
            w += crossing(a, bb, c, rq);
        }
        at += 2 + 2 * n;
        let k = cand(ci);
        let extras = &words[words[8] as usize..];
        let in_disc = (0..k[6] as usize).any(|d| {
            let e = &extras[k[5] as usize + DISC * d..];
            let (dx, dy) = (q[0] - f64::from(e[0] as i32), q[1] - f64::from(e[1] as i32));
            (dx * dx + dy * dy).sqrt() < f64::from(f32::from_bits(e[2]))
        });
        if w != 0 && !in_disc {
            covered.push(ci as u32);
        }
    }
    let segs = &words[at..at + n_segs];
    let label = decide(words, &covered, q, segs);
    Found { label, covered }
}

/// The glob of a line at q: its sample nearest q among the segments of the bin.
fn way_glob(words: &[u32], line: u32, q: [f64; 2], segs: &[u32]) -> u32 {
    let lines = words[6] as usize;
    let samples = words[7] as usize;
    let first = words[lines + LINE * line as usize + 2] as usize;
    let mut best = (f64::INFINITY, u32::MAX);
    for &s in segs.iter().filter(|&&s| s & 0xffff == line) {
        for i in [(s >> 16) as usize, (s >> 16) as usize + 1] {
            let w = &words[samples + SAMPLE * (first + i)..];
            let (dx, dy) = (q[0] - f64::from(w[0] as i32), q[1] - f64::from(w[1] as i32));
            let d = dx * dx + dy * dy;
            if d < best.0 {
                best = (d, w[4]);
            }
        }
    }
    best.1
}

fn decide(words: &[u32], covered: &[u32], q: [f64; 2], segs: &[u32]) -> Option<u32> {
    let cand = |ci: u32| &words[words[4] as usize + ci as usize * CAND..][..CAND];
    let label = |id: u32| &words[words[5] as usize + LABEL * id as usize..][..LABEL];
    let extras = &words[words[8] as usize..];
    let glob = |ci: u32| {
        let k = cand(ci);
        if k[0] == WAY {
            way_glob(words, k[4], q, segs)
        } else {
            k[2]
        }
    };
    let rest: Vec<u32> = covered.iter().copied().filter(|&c| cand(c)[0] != REGION && cand(c)[0] != OUTLINE).collect();
    let best = rest.iter().map(|&c| cand(c)[1]).min()?;
    let rail = rest.iter().copied().find(|&c| label(cand(c)[3])[0] & 0xff == Surface::RailBed as u32);
    let win = rest.iter().copied().filter(|&c| cand(c)[1] == best).min_by_key(|&c| glob(c))?;
    let k = cand(win);
    if k[0] == FILL {
        return Some(k[3]);
    }
    let mut id = k[3];
    let mut across = false;
    for b in 0..k[8] as usize {
        let e = &extras[k[7] as usize + 5 * b..];
        if past(e, q) {
            id = e[4];
            across = true;
            break;
        }
    }
    let l = label(id);
    if (l[0] >> 8) & 0xff == WAY_OWNER && !across {
        let w = l[1];
        let region = covered
            .iter()
            .copied()
            .filter(|&c| cand(c)[0] == REGION)
            .filter(|&c| {
                let r = cand(c);
                let parts = (0..r[10] as usize).map(|p| &extras[r[9] as usize + 11 * p..][..11]);
                let mine: Vec<&[u32]> = parts.filter(|p| p[0] == w).collect();
                !mine.is_empty()
                    && mine.iter().all(|p| !(p[1] == 1 && past(&p[2..6], q)) && !(p[6] == 1 && past(&p[7..11], q)))
            })
            .min_by_key(|&c| (cand(c)[1], cand(c)[2]));
        if let Some(r) = region {
            id = cand(r)[3];
        }
    }
    let l = label(id);
    if l[0] & 0xff == Surface::Carriageway as u32 {
        if let Some(r) = rail {
            let rail_line = label(cand(r)[3])[2];
            let want = [Surface::Crossing as u32 | (l[0] & !0xff), l[1], l[2], rail_line];
            let n = words[2] as usize;
            if let Some(x) = (0..n as u32).find(|&x| label(x)[..4] == want) {
                id = x;
            }
        }
    }
    Some(id)
}
