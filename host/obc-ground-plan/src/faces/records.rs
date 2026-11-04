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
//! | labels | `LABEL` words each: surface, owner kind, rank, flags; owner id; material line; crossing way |
//! | lines | `LINE` words each: way, finish code with flags, first sample, samples, cut, fill, reach (f32), class rank |
//! | samples | `SAMPLE` words each: x, y, half-width in units (f32), metres along (f32), glob, height and bank (f32) |
//! | extras | discs (x, y, radius f32), beyond (cut, label), parts (way, two cuts with flags) |
//! | bins | `BINS`² headers (offset, cand items << 16 | seg items), then the items |
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
pub const LABEL: usize = 4;
pub const LINE: usize = 8;
pub const SAMPLE: usize = 7;

/// Candidate kinds.
pub const WAY: u32 = 0;
pub const POINT: u32 = 1;
pub const REGION: u32 = 2;
pub const BAND: u32 = 3;
pub const FILL: u32 = 4;

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
    (t0 <= t1).then(|| ([ax + t0 * dx, ay + t0 * dy], [ax + t1 * dx, ay + t1 * dy]))
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

fn encode(cell: CellKey, graph: &WayGraph, input: &CellInput, lines: &[WayLine], cands: &[Cand]) -> Records {
    let origin = cell.origin();
    let units = |m: f64| m / metres_per_unit(origin.y + CELL_UNITS / 2);
    // The global names in order: a way's at its samples, a point's at its position.
    let mut globs: Vec<[i64; 5]> =
        lines.iter().flat_map(|l| l.key.iter().copied()).chain(cands.iter().filter_map(|c| c.glob)).collect();
    globs.sort_unstable();
    globs.dedup();
    let order = |g: &[i64; 5]| globs.binary_search(g).unwrap() as u32;
    let mut labels: Vec<[u32; 4]> = Vec::new();
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
        ]);
        let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
        for (i, &p) in line.p.iter().enumerate() {
            sample_words.extend([p[0] as u32, p[1] as u32, f(line.h[i] as f32), f(samples[i].along_m as f32)]);
            sample_words.push(order(&line.key[i]));
            sample_words.extend([f(samples[i].z_m as f32), f(samples[i].bank as f32)]);
        }
    }
    // Candidates and their extras.
    let mut cand_words = Vec::new();
    let mut extras: Vec<u32> = Vec::new();
    let cut_words = |cut: &[P; 2]| [cut[0][0] as u32, cut[0][1] as u32, cut[1][0] as u32, cut[1][1] as u32];
    let mut stats = RecordStats::default();
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
        let disc_at = extras.len() as u32;
        for &(p, r) in &cand.discs {
            extras.extend([p[0] as u32, p[1] as u32, f(r as f32)]);
        }
        let beyond_at = extras.len() as u32;
        for (cut, l) in &cand.beyond {
            extras.extend(cut_words(cut));
            extras.push(label_id(l));
        }
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
    for (ci, cand) in cands.iter().enumerate() {
        let pts = cand.rings.iter().flatten();
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
        let wind = windings(&cand.rings, c0, c1, r0, r1);
        let cols = (c1 - c0 + 1) as usize;
        // Per bin, the clipped edges.
        let mut edges: Vec<Vec<u32>> = vec![Vec::new(); cols * (r1 - r0 + 1) as usize];
        for ring in &cand.rings {
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
    // The block.
    let mut words = vec![0u32; HEAD];
    words[0] = MAGIC;
    words[1] = cands.len() as u32;
    words[2] = labels.len() as u32;
    words[3] = lines.len() as u32;
    words[4] = words.len() as u32;
    words.extend(&cand_words);
    words[5] = words.len() as u32;
    words.extend(labels.iter().flatten());
    words[6] = words.len() as u32;
    words.extend(&line_words);
    words[7] = words.len() as u32;
    words.extend(&sample_words);
    words[8] = words.len() as u32;
    words.extend(&extras);
    words[9] = words.len() as u32;
    let heads = words.len();
    words.extend(std::iter::repeat_n(0u32, (2 * BINS * BINS) as usize));
    for b in 0..(BINS * BINS) as usize {
        if items[b].is_empty() {
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
        words[heads + 2 * b] = words.len() as u32;
        words[heads + 2 * b + 1] = (cand_count as u32) << 16 | segs[b].len() as u32;
        let before = words.len();
        words.extend(&items[b]);
        words.extend(&segs[b]);
        stats.bins_used += 1;
        stats.max_bin_words = stats.max_bin_words.max((words.len() - before) as u32);
    }
    words[10] = f(metres_per_unit(origin.y + CELL_UNITS / 2) as f32);
    stats.cands = cands.len() as u32;
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
    let (mut at, counts) = (words[heads + 2 * b] as usize, words[heads + 2 * b + 1]);
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
            let e = &extras[k[5] as usize + 3 * d..];
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
    let rest: Vec<u32> = covered.iter().copied().filter(|&c| cand(c)[0] != REGION).collect();
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
            if let Some(x) = (0..n as u32).find(|&x| label(x) == want) {
                id = x;
            }
        }
    }
    Some(id)
}
