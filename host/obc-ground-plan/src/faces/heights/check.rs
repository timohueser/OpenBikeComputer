//! The checks of the heights of a cell: the steps of a grid over the cell, inside a face or on a face
//! edge, the slope faces steeper than a rock face, the ripple of the carriageways, and the heights of
//! points on the cell's lines, which two cells must give alike.

use super::{height_at, height_at_sides, Block, Height, HeightRules, Kind, Step as Rule};
use crate::faces::records::{
    edge_ends, edge_line, find, label, outline_words, CAND, DISC, EDGE, JUNCTION, LABEL, LINE, PLATFORM,
};
use crate::faces::Surface;
use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use crate::terrain::Terrain;

/// A step: a jump over one frame unit of more than this.
pub const STEP_M: f64 = 0.01;
/// The ripple's grid: second differences over this.
pub const RIPPLE_M: f64 = 0.25;
/// The grid of `edge_steps` in the fixture's checks.
pub const EDGE_GRID_M: f64 = 1.0;
/// A water face of the owner rule lies at its host's own surface within this.
pub const WATER_LEVEL_M: f64 = 0.001;

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
    /// out, and the paved points whose label's material line the records lack (for a blend face, every
    /// record of its kind: it then takes the plain terrain).
    pub slots_full: Vec<[f64; 2]>,
    pub host_missing: Vec<([f64; 2], Option<u32>)>,
    /// The points of a water face of the owner rule whose height is not its host's own surface within
    /// `WATER_LEVEL_M`: the painter reads the records of one kind only, so W is exact.
    pub water_off: Vec<[f64; 2]>,
    pub steps: Vec<Step>,
    /// The second differences of the carriageway grid points along and across their line (m).
    pub ripple: [Vec<f64>; 2],
    /// Two neighbour slope points whose ground climbs more than `face_max` per metre between them where
    /// the plain terrain climbs less: the first point and the climb per metre.
    pub steep: Vec<([f64; 2], f64)>,
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

/// What `audit` finds at a point: its height and label, whether a cap left records out, whether the
/// records lack the label's material line, and whether a water face of the owner rule leaves its host's
/// own surface.
type Audit = ((Height, Option<u32>), bool, bool, bool);

/// The height at q with the label of `find` there, and the faults there (`Audit`).
fn audit(words: &[u32], terrain: &Terrain, cell: CellKey, q: [f64; 2]) -> Option<Audit> {
    let t = terrain_at(terrain, cell, q)?;
    let label = find(words, q);
    let rule = label.map_or(0, |id| self::label(words, id)[0] >> 24);
    if rule == 0 {
        return Some(((height_at(words, q, label, t), label), false, false, false));
    }
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    if rule == 1 {
        return Some(((height_at(words, q, label, t), label), super::groups(&b, q, &p).1, false, false));
    }
    let material = super::material(words, label);
    let items = super::bin_items(&b, q)?;
    let present = super::present(&b, q, &p, [material, u32::MAX], &items);
    let h = super::by_label(&b, q, &present.recs, items.blend, label, t, &p).0;
    // A host face misses its host; a blend face has none, and misses every record of its kind.
    let missing = match rule {
        3 => !present.recs.iter().any(|r| r.water == b.water(material)),
        _ => material != u32::MAX && !present.recs.iter().any(|r| r.line == material),
    };
    let full = present.full || (rule == 3 && super::regions_full(words, q, label.unwrap_or(u32::MAX), &p));
    let water = label.is_some_and(|id| self::label(words, id)[0] & 0xff == Surface::Water as u32) && rule == 2;
    let host = present.recs.iter().find(|r| r.line == material);
    let off = water && host.is_some_and(|r| (h.h - r.z).abs() > WATER_LEVEL_M);
    Some(((h, label), full, missing, off))
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

/// `scan`'s grid points of `grid_m` over the cell with a known terrain, those where a cap left records
/// out (`Scan::slots_full`) and those whose label's material line the records lack
/// (`Scan::host_missing`), without the steps: the census reads the caps of each cell of a region.
pub fn caps(words: &[u32], terrain: &Terrain, cell: CellKey, grid_m: f64) -> [u64; 3] {
    let step = grid_m / metres_per_unit(cell.origin().y + CELL_UNITS / 2);
    let n = (CELL_UNITS as f64 / step).ceil() as usize;
    let mut out = [0; 3];
    for k in 0..n * n {
        let q = [((k % n) as f64 + 0.5) * step, ((k / n) as f64 + 0.5) * step];
        if q[0] >= CELL_UNITS as f64 || q[1] >= CELL_UNITS as f64 {
            continue;
        }
        if let Some((_, full, missing, _)) = audit(words, terrain, cell, q) {
            out = [out[0] + 1, out[1] + u64::from(full), out[2] + u64::from(missing)];
        }
    }
    out
}

/// The steps of a grid of `grid_m` over the cell, and the ripple at its carriageway points. Between
/// two neighbours whose heights differ by more than `STEP_M`, the span halves, keeping the half whose
/// heights change more, while that half keeps most of the change; a jump that holds down to one frame
/// unit is a step. A step on a face edge is named when both sides at one point give its size by a rule.
pub fn scan(words: &[u32], terrain: &Terrain, cell: CellKey, grid_m: f64) -> Scan {
    let rules = HeightRules::of(words);
    let face_max = rules.face_max;
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
            if a.3 {
                out.water_off.push(q);
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
            let (mut a, mut b) = (point(i, j), point(i + di, j + dj));
            let climb = |ga: f64, gb: f64| (gb - ga).abs() / grid_m;
            if ha.0.kind == Kind::Slope && hb.0.kind == Kind::Slope && climb(ha.0.h, hb.0.h) > face_max {
                if let (Some(ta), Some(tb)) = (terrain_at(terrain, cell, a), terrain_at(terrain, cell, b)) {
                    if climb(ta, tb) < face_max {
                        out.steep.push((a, climb(ha.0.h, hb.0.h)));
                    }
                }
            }
            if (hb.0.h - ha.0.h).abs() <= STEP_M {
                continue;
            }
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

/// The steps at the label edges of a grid of `grid_m` over the cell, read by both sides at one point:
/// between two neighbours with two labels, the span halves to one frame unit by the labels alone, and
/// `height_at_sides` gives the step there. So a step beside a slope that falls the same way counts,
/// which `scan`'s halving by height can miss. The steps over `STEP_M` with no rule (`Unnamed`, or
/// `Apart` beside a face that GP6's rules do not own yet).
pub fn edge_steps(words: &[u32], terrain: &Terrain, cell: CellKey, grid_m: f64) -> Vec<Step> {
    let step = grid_m / metres_per_unit(cell.origin().y + CELL_UNITS / 2);
    let n = (CELL_UNITS as f64 / step) as usize;
    let point = |i: usize, j: usize| [(i as f64 + 0.5) * step, (j as f64 + 0.5) * step];
    let labels: Vec<Option<u32>> = (0..n * n).map(|k| find(words, point(k % n, k / n))).collect();
    let mut out = Vec::new();
    for k in 0..n * n {
        let (i, j) = (k % n, k / n);
        for nb in [(i + 1 < n).then_some(k + 1), (j + 1 < n).then_some(k + n)].into_iter().flatten() {
            if labels[k] == labels[nb] {
                continue;
            }
            let (mut a, mut b, mut lb) = (point(i, j), point(nb % n, nb / n), labels[nb]);
            while libm::hypot(b[0] - a[0], b[1] - a[1]) > 1.0 {
                let m = [(a[0] + b[0]) / 2.0, (a[1] + b[1]) / 2.0];
                match find(words, m) {
                    l if l == labels[k] => a = m,
                    l => (b, lb) = (m, l),
                }
            }
            let Some(t) = terrain_at(terrain, cell, a) else { continue };
            let sides = height_at_sides(words, a, [labels[k], lb], t);
            let dh = sides[1].h - sides[0].h;
            if dh.abs() <= STEP_M || sides[0].step != Rule::None {
                continue;
            }
            let class = if apart(words, labels[k]) || apart(words, lb) { Class::Apart } else { Class::Unnamed };
            out.push(Step { a, b, dh, labels: [labels[k], lb], class, sides });
        }
    }
    out
}

/// The points of `wall_steps` along an outline edge lie this far apart, at most.
pub const WALL_TOP_M: f64 = 0.25;
/// A step this high beside a wall class's road draws as a wall (`WALL_STEP_M` of `plan/piece.wgsl`).
pub const WALL_M: f64 = 0.5;

/// The outline edges of a cell's records that lie in the cell by their middle: their ends (frame units), their
/// unit direction, their host line and their turn at b (`Edge::turn`), in the records' order.
fn cell_edges(words: &[u32]) -> impl Iterator<Item = ([f64; 2], [f64; 2], [f64; 2], u32, f64)> + '_ {
    let inside = |v: f64| (0.0..CELL_UNITS as f64).contains(&v);
    let block = Block::at(words, [0.0, 0.0]);
    outline_words(words).chunks(EDGE).enumerate().filter_map(move |(k, e)| {
        let (a, b) = edge_ends(e);
        let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
        let len = libm::hypot(b[0] - a[0], b[1] - a[1]);
        let u = [(b[0] - a[0]) / len, (b[1] - a[1]) / len];
        (inside((a[0] + b[0]) / 2.0) && inside((a[1] + b[1]) / 2.0))
            .then(|| (a, b, u, edge_line(e), block.edge(k as u32).turn[1]))
    })
}

/// The wall-top steps of a cell: along each outline edge of a wall class that lies in the cell by its middle,
/// points up to `WALL_TOP_M` apart (edges under 0.1 m left out); two neighbours on one no-room wall at least
/// `WALL_M` high whose wall height changes by more than `grade` per metre: the first point and that change.
pub fn wall_steps(words: &[u32], terrain: &Terrain, cell: CellKey, grade: f64) -> Vec<([f64; 2], f64)> {
    let b = Block::at(words, [0.0, 0.0]);
    let mut out = Vec::new();
    for (a, z, u, line, _) in cell_edges(words) {
        let len = libm::hypot(z[0] - a[0], z[1] - a[1]) * b.mpu;
        if len < 0.1 || !b.line(line as usize).wall {
            continue;
        }
        let (n, steps) = ([u[1], -u[0]], (len / WALL_TOP_M).ceil() as usize);
        let wall = |k: usize| {
            let q = [a[0] + (z[0] - a[0]) * k as f64 / steps as f64, a[1] + (z[1] - a[1]) * k as f64 / steps as f64];
            let labels = [-2.0, 2.0].map(|o| find(words, [q[0] + o * n[0], q[1] + o * n[1]]));
            let s = height_at_sides(words, q, labels, terrain_at(terrain, cell, q)?);
            let w = s[1].h - s[0].h;
            (s[1].step == Rule::NoRoom && w.abs() >= WALL_M).then_some((q, w))
        };
        let walls: Vec<_> = (0..=steps).map(wall).collect();
        for pair in walls.windows(2) {
            if let [Some((q, w0)), Some((_, w1))] = pair {
                let climb = (w1 - w0).abs() * steps as f64 / len;
                if climb > grade {
                    out.push((*q, climb));
                }
            }
        }
    }
    out
}

/// `slope_steps` reads the ground this many metres out from the rim, and these metres along it to each side.
pub const SLOPE_OUT_M: [f64; 3] = [1.0, 2.0, 4.0];
pub const SLOPE_SIDE_M: [f64; 2] = [0.25, 0.05];
/// The slope steps of a cell (the shading bands, a groove or a ridge): at each end in the cell where the
/// record's turn goes on to the next edge on a straight rim, the ground's grade along the rim over each of
/// `SLOPE_SIDE_M` before and after the end, at each of `SLOPE_OUT_M` out where all three points lie on a slope;
/// where the grade changes by more than `grade`: the end and the greatest change. The rim is straight where
/// the turn is within what a shift of each whole-unit end by one unit across the rim gives (2 / length per
/// edge, in units): a cut at the bin grid rounds its point and a split floors its middle.
pub fn slope_steps(words: &[u32], terrain: &Terrain, cell: CellKey, grade: f64) -> Vec<([f64; 2], f64)> {
    let mpu = Block::at(words, [0.0, 0.0]).mpu;
    let edges: Vec<_> = cell_edges(words).collect();
    let mut out = Vec::new();
    for pair in edges.windows(2) {
        let [(a0, v, u, l0, turn), (a1, z, _, l1, _)] = [pair[0], pair[1]];
        let rounding = 2.0 / libm::hypot(v[0] - a0[0], v[1] - a0[1]) + 2.0 / libm::hypot(z[0] - a1[0], z[1] - a1[1]);
        if v != a1 || l0 != l1 || turn.is_nan() || turn.abs() > rounding {
            continue;
        }
        let n = [u[1], -u[0]];
        let ground = |out: f64, side: f64| {
            let q = [v[0] + (n[0] * out + u[0] * side) / mpu, v[1] + (n[1] * out + u[1] * side) / mpu];
            at(words, terrain, cell, q).filter(|(h, _)| h.kind == Kind::Slope).map(|(h, _)| h.h)
        };
        let change = (SLOPE_OUT_M.iter().flat_map(|&o| SLOPE_SIDE_M.map(|d| (o, d))))
            .filter_map(|(o, d)| {
                let (l, c, r) = (ground(o, -d)?, ground(o, 0.0)?, ground(o, d)?);
                Some(((r - c) - (c - l)).abs() / d)
            })
            .fold(0.0, f64::max);
        if change > grade {
            out.push((v, change));
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
    let Some(items) = super::bin_items(&b, q) else { return false };
    items.discs.iter().any(|&item| {
        let cand = &words[words[4] as usize + CAND * (item & 0xffff) as usize..][..CAND];
        if cand[3] == u32::MAX || (label(words, cand[3])[0] >> 8) & 0xff != JUNCTION {
            return false;
        }
        let e = &words[words[8] as usize + cand[5] as usize + DISC * (item >> 16) as usize..][..DISC];
        let feather = [e[3], e[4]]
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
    let Some(items) = super::bin_items(b, q) else { return [1.0, 0.0] };
    let mut best = (f64::INFINITY, [1.0, 0.0]);
    for &item in items.segs {
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

/// The records present at q, the host lines of the outline near it and both sides' causes, for the
/// debug tool's `--why`.
pub fn explain(words: &[u32], q: [f64; 2], labels: [Option<u32>; 2], t: f64) -> String {
    use super::{by_label, groups, material, paved, present, slope_edge, Block, HeightRules};
    let b = Block::at(words, q);
    let p = HeightRules::of(words);
    let mut s = format!("q {q:?} t {t:.3} mpu {:.6} slope edge {:.3} m\n", b.mpu, slope_edge(&b, q));
    let items = super::bin_items(&b, q);
    let recs = items.as_ref().map_or(Vec::new(), |i| present(&b, q, &p, labels.map(|l| material(words, l)), i).recs);
    for (k, r) in recs.iter().enumerate() {
        let ln = b.line(r.line as usize);
        let ends = [0, 1].map(|e| b.past_end(&ln, e, q));
        let segs: Vec<u32> = items
            .as_ref()
            .map_or(Vec::new(), |i| i.segs.iter().filter(|&&w| w & 0xffff == r.line).map(|w| w >> 16).collect());
        s += &format!(
            "  rec {k}: line {} segs {segs:?} of {} class {} rail {} level ends past {ends:.3?} z {:.3} d {:.3} feather {} paved {:.3}\n",
            r.line, ln.count, r.class, r.rail, r.z, r.d, r.feather, paved(&recs, k, &p)
        );
    }
    for g in groups(&b, q, &p).0 {
        s += &format!(
            "  outline of line {} class {} rail {}: fill {:.3} cut {:.3} steep {:.3} d {:.3} toe {:.3} fade {:.3}\n",
            g.line, g.class, g.rail, g.fill, g.cut, g.steep, g.d, g.toe, g.fade
        );
    }
    let blend = items.as_ref().map_or(&[][..], |i| i.blend);
    for l in labels {
        let (h, c) = by_label(&b, q, &recs, blend, l, t, &p);
        let rule = l.map_or(0, |id| words[words[5] as usize + LABEL * id as usize] >> 24);
        s += &format!("  label {l:?} rule {rule}: {h:?} by {:?}\n", c.by);
    }
    // A region's edges within its fade: the face beyond (its host line) and the metres to it.
    for l in labels.into_iter().flatten() {
        for e in crate::faces::records::blend_edges(words, q).filter(|e| e.region == l) {
            let d = super::to_segment(q, e.a.map(f64::from), e.b.map(f64::from)) * b.mpu;
            if d < p.region_fade_m {
                let material = e.other.map_or(u32::MAX, |o| label(words, o)[2]);
                s += &format!("  edge of {l} to {:?} material {material}: {d:.3} m\n", e.other);
            }
        }
    }
    let sides = height_at_sides(words, q, labels, t);
    s += &format!("  step {:?}\n", sides[0].step);
    s
}
