//! The checks of the heights of a cell: the steps of a grid over the cell, inside a face or on a face
//! edge, and the heights of points on the cell's lines, which two cells must give alike.

use super::{height_at, height_at_sides, Height, Kind};
use crate::faces::records::{find, LINE};
use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use crate::terrain::Terrain;

/// A step: a jump over one frame unit of more than this.
pub const STEP_M: f64 = 0.01;

/// A step between two grid points: where it stands (within a frame unit) and the two sides there.
#[derive(Clone, Debug)]
pub struct Step {
    /// The two points about the jump, frame units from the cell's origin.
    pub a: [f64; 2],
    pub b: [f64; 2],
    pub dh: f64,
    /// The labels at a and b, and whether they differ: a step on a face edge.
    pub labels: [Option<u32>; 2],
    pub edge: bool,
    /// The heights of both labels at a.
    pub sides: [Height; 2],
}

#[derive(Clone, Debug, Default)]
pub struct Scan {
    pub points: u64,
    pub steps: Vec<Step>,
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

/// The steps of a grid of `grid_m` over the cell: between two neighbours whose heights differ by more
/// than `STEP_M`, the span halves, keeping the half whose heights change more, while that half keeps
/// most of the change; a jump that holds down to one frame unit is a step.
pub fn scan(words: &[u32], terrain: &Terrain, cell: CellKey, grid_m: f64) -> Scan {
    let mpu = metres_per_unit(cell.origin().y + CELL_UNITS / 2);
    let step = grid_m / mpu;
    let n = (CELL_UNITS as f64 / step).ceil() as usize;
    let point = |i: usize, j: usize| [(i as f64 + 0.5) * step, (j as f64 + 0.5) * step];
    let grid: Vec<Option<(Height, Option<u32>)>> = (0..n * n)
        .map(|k| {
            let q = point(k % n, k / n);
            (q[0] < CELL_UNITS as f64 && q[1] < CELL_UNITS as f64).then(|| at(words, terrain, cell, q)).flatten()
        })
        .collect();
    let mut steps = Vec::new();
    for k in 0..n * n {
        let (i, j) = (k % n, k / n);
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
            if found {
                let t = terrain_at(terrain, cell, a).unwrap_or(f64::NAN);
                let sides = height_at_sides(words, a, [va.1, vb.1], t);
                let edge = va.1 != vb.1 || ha.1 != hb.1;
                steps.push(Step { a, b, dh: vb.0.h - va.0.h, labels: [va.1, vb.1], edge, sides });
            }
        }
    }
    Scan { points: (n * n) as u64, steps }
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
