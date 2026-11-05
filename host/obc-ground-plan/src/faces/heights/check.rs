//! The checks of the heights of a cell: the steps of a grid over the cell, inside a face or on a face
//! edge, the ripple of the carriageways, and the heights of points on the cell's lines, which two
//! cells must give alike.

use super::{height_at, height_at_sides, Block, Height, HeightRules, Kind, Step as Rule};
use crate::faces::records::{find, label, CAND, DISC, JUNCTION, LABEL, LINE, PLATFORM};
use crate::faces::Surface;
use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use crate::terrain::Terrain;

/// A step: a jump over one frame unit of more than this.
pub const STEP_M: f64 = 0.01;
/// The ripple's grid: second differences over this.
pub const RIPPLE_M: f64 = 0.25;

/// Where a step stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Inside one face.
    Inside,
    /// On a face edge, by a rule of its size.
    Named(Rule),
    /// On a face edge, with no rule of its size: a fault.
    Unnamed,
    /// On the edge of a face that GP6's rules do not own yet, with no rule of its size: a band face
    /// with no material line, or a platform.
    Apart,
}

/// A step between two grid points: where it stands (within a frame unit) and the two sides there.
#[derive(Clone, Debug)]
pub struct Step {
    /// The two points about the jump, frame units from the cell's origin.
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub dh: f64,
    /// The labels at a and b.
    pub labels: [Option<u32>; 2],
    pub class: Class,
    /// The heights of both labels at a.
    pub sides: [Height; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub points: u64,
    /// The grid points where a cap of `SLOTS` lines, `QUADS` quads or `REGIONS` regions left records
    /// out, and the paved points whose label's material line the records lack.
    pub slots_full: Vec<[f64; 2]>,
    pub host_missing: Vec<([f64; 2], Option<u32>)>,
    pub steps: Vec<Step>,
    /// The second differences of the carriageway grid points along and across their line (m).
    pub ripple: [Vec<f64>; 2],
}

/// The plain terrain at q, frame units from the cell's origin: at the nearest frame unit.
pub fn terrain_at(terrain: &Terrain, cell: CellKey, q: [f64; 2]) -> Option<f64> {
    let o = cell.origin();
    terrain.height(GlobalPoint { x: o.x + q[0].round() as i64, y: o.y + q[1].round() as i64 })
}

/// The height at q with the label of `find` there.
pub fn at(words: &[u32], terrain: &Terrain, cell: CellKey, q: [f64; 2]) -> Option<(Height, Option<u32>)> {
    let t = terrain_at(terrain, cell, q)?;
    let label = find(words, q);
    Some((height_at(words, q, label, t), label))
}

/// The height at q with the label of `find` there, whether a cap left records out, and whether the
/// records lack the label's material line.
fn audit(words: &[u32], terrain: &Terrain, cell: CellKey, q: [f64; 2]) -> Option<((Height, Option<u32>), bool, bool)> {
    let t = terrain_at(terrain, cell, q)?;
    let label = find(words, q);
    let rule = label.map_or(0, |id| self::label(words, id)[0] >> 24);
    if rule == 0 {
        return Some(((height_at(words, q, label, t), label), false, false));
    }
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let material = super::material(words, label);
    let present = super::present(&b, q, &p, [material, u32::MAX]);
    let h = super::by_label(&b, q, &present.recs, label, t, &p).0;
    // A blend face has no host: its material line need not be present.
    let missing = (rule == 2 || rule == 4) && material != u32::MAX && !present.recs.iter().any(|r| r.line == material);
    let full = present.full || (rule == 3 && super::regions_full(words, q, label.unwrap_or(u32::MAX), &p));
    Some(((h, label), full, missing))
}

/// Whether a label's face is one that GP6's rules do not own yet: a paved band face with no material
/// line, or a platform.
fn apart(words: &[u32], id: Option<u32>) -> bool {
    id.is_some_and(|id| {
        let l = label(words, id);
        let owner = (l[0] >> 8) & 0xff;
        owner == PLATFORM || (l[0] >> 24 >= 2 && l[2] == u32::MAX)
    })
}

/// The steps of a grid of `grid_m` over the cell, and the ripple at its carriageway points. Between
/// two neighbours whose heights differ by more than `STEP_M`, the span halves, keeping the half whose
/// heights change more, while that half keeps most of the change; a jump that holds down to one frame
/// unit is a step. A step on a face edge is named when both sides at one point give its size by a rule.
pub fn scan(words: &[u32], terrain: &Terrain, cell: CellKey, grid_m: f64) -> Scan {
    let mpu = metres_per_unit(cell.origin().y + CELL_UNITS / 2);
    let step = grid_m / mpu;
    let n = (CELL_UNITS as f64 / step).ceil() as usize;
    let point = |i: usize, j: usize| [(i as f64 + 0.5) * step, (j as f64 + 0.5) * step];
    let mut out = Scan { points: (n * n) as u64, ..Scan::default() };
    let grid: Vec<Option<(Height, Option<u32>)>> = (0..n * n)
        .map(|k| {
            let q = point(k % n, k / n);
            let a = (q[0] < CELL_UNITS as f64 && q[1] < CELL_UNITS as f64)
                .then(|| audit(words, terrain, cell, q))
                .flatten()?;
            if a.1 {
                out.slots_full.push(q);
            }
            if a.2 {
                out.host_missing.push((q, a.0 .1));
            }
            Some(a.0)
        })
        .collect();
    for k in 0..n * n {
        let (i, j) = (k % n, k / n);
        if let Some(r) = grid[k].and_then(|g| ripple(words, terrain, cell, point(i, j), g)) {
            out.ripple[0].push(r[0]);
            out.ripple[1].push(r[1]);
        }
        for (nb, di, dj) in
            [(i + 1 < n).then_some((k + 1, 1, 0)), (j + 1 < n).then_some((k + n, 0, 1))].into_iter().flatten()
        {
            let (Some(ha), Some(hb)) = (grid[k], grid[nb]) else { continue };
            if (hb.0.h - ha.0.h).abs() <= STEP_M {
                continue;
            }
            let (mut a, mut b) = (point(i, j), point(i + di, j + dj));
            let (mut va, mut vb) = (ha, hb);
            let found = loop {
                let d = (vb.0.h - va.0.h).abs();
                if d <= STEP_M {
                    break false;
                }
                if libm::hypot(b[0] - a[0], b[1] - a[1]) <= 1.0 {
                    break true;
                }
                let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                let Some(vm) = at(words, terrain, cell, m) else { break false };
                let (d1, d2) = ((vm.0.h - va.0.h).abs(), (vb.0.h - vm.0.h).abs());
                if d1.max(d2) < 0.75 * d {
                    break false;
                }
                if d1 >= d2 {
                    (b, vb) = (m, vm);
                } else {
                    (a, va) = (m, vm);
                }
            };
            if !found {
                continue;
            }
            let t = terrain_at(terrain, cell, a).unwrap_or(f64::NAN);
            let sides = height_at_sides(words, a, [va.1, vb.1], t);
            let dh = vb.0.h - va.0.h;
            // The sides at one point give the step: else the field jumps between a and b.
            let same = ((sides[1].h - sides[0].h) - dh).abs() <= STEP_M;
            let class = if va.1 == vb.1 && ha.1 == hb.1 {
                Class::Inside
            } else if same && sides[0].step != Rule::None {
                Class::Named(sides[0].step)
            } else if apart(words, va.1) || apart(words, vb.1) {
                Class::Apart
            } else {
                Class::Unnamed
            };
            out.steps.push(Step { a, b, dh, labels: [va.1, vb.1], class, sides });
        }
    }
    out
}

/// The second differences of h at a carriageway point c over `RIPPLE_M` along and across its line
/// (NaN where a neighbour lies in another face), or None for a point that the ripple gate leaves out:
/// no carriageway of a way's own face, or within a feather of a junction's circle (a junction ramp).
pub fn ripple(
    words: &[u32],
    terrain: &Terrain,
    cell: CellKey,
    c: [f64; 2],
    h: (Height, Option<u32>),
) -> Option<[f64; 2]> {
    let id = h.1?;
    let l = label(words, id);
    let surface = l[0] & 0xff;
    let carriageway = surface == Surface::Carriageway as u32 || surface == Surface::Crossing as u32;
    if !carriageway || l[0] >> 24 != 2 || l[2] == u32::MAX || near_junction(words, c) {
        return None;
    }
    let b = Block::at(words, c);
    let u = direction(&b, l[2] as usize, c);
    let s = RIPPLE_M / b.mpu;
    let mut out = [f64::NAN; 2];
    for (k, d) in [u, [-u[1], u[0]]].into_iter().enumerate() {
        let p = at(words, terrain, cell, [c[0] + d[0] * s, c[1] + d[1] * s]);
        let m = at(words, terrain, cell, [c[0] - d[0] * s, c[1] - d[1] * s]);
        if let (Some(p), Some(m)) = (p, m) {
            if p.1 == h.1 && m.1 == h.1 {
                out[k] = p.0.h - 2.0 * h.0.h + m.0.h;
            }
        }
    }
    Some(out)
}

/// Whether q lies within a feather of the circle of a junction's fan (its arms' greater feather).
fn near_junction(words: &[u32], q: [f64; 2]) -> bool {
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let Some((_, _, ds, nd)) = super::bin_items(&b, q) else { return false };
    (0..nd).any(|k| {
        let item = words[ds + k];
        let cand = &words[words[4] as usize + CAND * (item & 0xffff) as usize..][..CAND];
        if cand[3] == u32::MAX || (label(words, cand[3])[0] >> 8) & 0xff != JUNCTION {
            return false;
        }
        let e = &words[words[8] as usize + cand[5] as usize + DISC * (item >> 16) as usize..][..DISC];
        let feather = [e[9], e[10]]
            .iter()
            .filter(|&&l| l != u32::MAX)
            .map(|&l| b.line(l as usize).reach.max(p.feather_min_m))
            .fold(0.0, f64::max);
        let r = f64::from(f32::from_bits(e[2])) * b.mpu;
        let d = libm::hypot(q[0] - f64::from(e[0] as i32), q[1] - f64::from(e[1] as i32)) * b.mpu;
        d <= r + feather
    })
}

/// The unit direction (frame axes) of line l's segment nearest q among the segments of q's bin.
fn direction(b: &Block, l: usize, q: [f64; 2]) -> [f64; 2] {
    let ln = b.line(l);
    let Some((hs, nh, _, _)) = super::bin_items(b, q) else { return [1.0, 0.0] };
    let mut best = (f64::INFINITY, [1.0, 0.0]);
    for k in 0..nh {
        let item = b.w[hs + k];
        if (item & 0xffff) as usize != l {
            continue;
        }
        let i = (item >> 16) as usize;
        let (pa, pb) = (b.position(&ln, i), b.position(&ln, i + 1));
        let ab = [pb[0] - pa[0], pb[1] - pa[1]];
        let l2 = ab[0] * ab[0] + ab[1] * ab[1];
        if l2 <= 0.0 {
            continue;
        }
        let f = (((q[0] - pa[0]) * ab[0] + (q[1] - pa[1]) * ab[1]) / l2).clamp(0.0, 1.0);
        let d = libm::hypot(q[0] - pa[0] - f * ab[0], q[1] - pa[1] - f * ab[1]);
        if d < best.0 {
            best = (d, [ab[0] / l2.sqrt(), ab[1] / l2.sqrt()]);
        }
    }
    best.1
}

/// A point on a cell line as one cell reads it: the line (0 for x = const, 1 for y = const), its global
/// coordinate and the global coordinate along it.
pub type BorderKey = (u8, i64, i64);

/// A border point's height and terrain bits, and its side's name.
#[derive(Clone, Debug, PartialEq)]
pub struct BorderValue {
    pub h: u64,
    pub t: u64,
    pub name: String,
}

/// The heights of points on the four lines of a cell, every `step` units along each: both cells of a
/// line read the same global points.
pub fn border(words: &[u32], terrain: &Terrain, cell: CellKey, step: i64) -> Vec<(BorderKey, BorderValue)> {
    let o = cell.origin();
    let mut out = Vec::new();
    for along in (step / 2..CELL_UNITS).step_by(step as usize) {
        for (axis, at) in [(0u8, 0), (0, CELL_UNITS), (1, 0), (1, CELL_UNITS)] {
            let q = if axis == 0 { [at as f64, along as f64] } else { [along as f64, at as f64] };
            let Some((h, _)) = self::at(words, terrain, cell, q) else { continue };
            let t = terrain_at(terrain, cell, q).unwrap_or(f64::NAN);
            let (line, g) = if axis == 0 { (o.x + at, o.y + along) } else { (o.y + at, o.x + along) };
            out.push(((axis, line, g), BorderValue { h: h.h.to_bits(), t: t.to_bits(), name: side_name(words, &h) }));
        }
    }
    out
}

/// A side as its kind and the class rank of its owner line.
pub fn side_name(words: &[u32], h: &Height) -> String {
    let kind = match h.kind {
        Kind::Terrain => "terrain",
        Kind::Slope => "slope",
        Kind::Paved => "paved",
    };
    if h.owner == u32::MAX {
        return kind.to_string();
    }
    let class = words[words[6] as usize + LINE * h.owner as usize + 7];
    format!("{kind}:{class}")
}

/// The records present at q and both sides' causes, for the debug tool's `--why`.
pub fn explain(words: &[u32], q: [f64; 2], labels: [Option<u32>; 2], t: f64) -> String {
    use super::{by_label, cones, material, paved, present, slope_edge, Block, HeightRules};
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let recs = present(&b, q, &p, labels.map(|l| material(words, l))).recs;
    let mut s = format!("q {q:?} t {t:.3} mpu {:.6} slope edge {:.3} m\n", b.mpu, slope_edge(&b, q));
    for (k, r) in recs.iter().enumerate() {
        let base = paved(&recs, k, &p);
        let (fill, cut, wf, wc, sf, sc) = cones(base, r.reach_t, r.d.min(r.out), r, &p);
        let ln = b.line(r.line as usize);
        let ends = [0, 1].map(|e| b.past_end(&ln, e, q));
        let segs: Vec<u32> = super::bin_items(&b, q).map_or(Vec::new(), |(hs, nh, _, _)| {
            (0..nh).map(|k| words[hs + k]).filter(|w| w & 0xffff == r.line).map(|w| w >> 16).collect()
        });
        s += &format!(
            "  rec {k}: line {} segs {segs:?} of {} class {} rail {} level ends past {ends:.3?} z {:.3} d {:.3} out {:.3} feather {} reach {} reach_t {:.3} | base {:.3} fill {:.3} cut {:.3} walls {:.3}/{:.3} slopes {:.2}/{:.2}\n",
            r.line, ln.count, r.class, r.rail, r.z, r.d, r.out, r.feather, r.reach, r.reach_t, base, fill, cut, wf, wc, sf, sc
        );
    }
    for l in labels {
        let (h, c) = by_label(&b, q, &recs, l, t, &p);
        let rule = l.map_or(0, |id| words[words[5] as usize + LABEL * id as usize] >> 24);
        s += &format!("  label {l:?} rule {rule}: {h:?} by {:?} cut {}\n", c.by, c.cut);
    }
    // A region's edges within its fade: the face beyond (its host line) and the metres to it.
    for l in labels.into_iter().flatten() {
        for e in crate::faces::records::blend_edges(words, q).filter(|e| e.region == l) {
            let d = super::to_segment(q, e.a.map(f64::from), e.b.map(f64::from)) * b.mpu;
            if d < p.region_fade_m {
                let host = e.other.map_or(u32::MAX, |o| super::host_line(words, o));
                s += &format!("  edge of {l} to {:?} host {host}: {d:.3} m\n", e.other);
            }
        }
    }
    let sides = height_at_sides(words, q, labels, t);
    s += &format!("  step {:?}\n", sides[0].step);
    s
}
