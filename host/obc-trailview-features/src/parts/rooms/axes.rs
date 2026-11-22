//! The axes of the arms or the rooms by cells: per point, the nearest foot on each other axis near it.

use std::collections::HashMap;

use super::super::Tube;
use crate::metres;

/// A chord of an axis: its tube, its ends in world units, their heights and metres along the tube.
struct Seg {
    tube: usize,
    a: [f64; 2],
    b: [f64; 2],
    z: [f64; 2],
    s: [f64; 2],
}

/// The axes of all arms or rooms by cells of `CELL_M`.
pub(crate) struct Axes {
    segs: Vec<Seg>,
    grid: HashMap<(i64, i64), Vec<usize>>,
    cell: f64,
    len: Vec<f64>,
}

/// The side of a cell: wider than the farthest reach of a test (two halves and a wall).
const CELL_M: f64 = 32.0;

/// Another axis near a point: its tube, the metres to it, its height and its metres along there, its unit direction
/// there in world units, and whether the foot lies past an end of the axis, so that the end is the nearest point.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Foot {
    pub tube: usize,
    pub d: f64,
    pub z: f64,
    pub s: f64,
    pub dir: [f64; 2],
    pub past: bool,
}

impl Axes {
    pub fn new(tubes: &[Tube]) -> Axes {
        let mut segs = Vec::new();
        for (t, tube) in tubes.iter().enumerate() {
            let points: Vec<_> = tube.curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).collect();
            for w in points.windows(2) {
                if w[1].s > w[0].s {
                    segs.push(Seg { tube: t, a: w[0].p, b: w[1].p, z: [w[0].z, w[1].z], s: [w[0].s, w[1].s] });
                }
            }
        }
        let cell = segs.first().map_or(1.0, |s| CELL_M / metres(s.a[1]));
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let at = |v: f64| (v / cell).floor() as i64;
        for (i, s) in segs.iter().enumerate() {
            for x in at(s.a[0].min(s.b[0]))..=at(s.a[0].max(s.b[0])) {
                for y in at(s.a[1].min(s.b[1]))..=at(s.a[1].max(s.b[1])) {
                    grid.entry((x, y)).or_default().push(i);
                }
            }
        }
        Axes { segs, grid, cell, len: tubes.iter().map(|t| t.curve.len).collect() }
    }

    /// The axis of each tube but `own` within `reach` metres of p, nearest per tube.
    pub fn near(&self, p: [f64; 2], own: usize, reach: f64) -> Vec<Foot> {
        let m = metres(p[1]);
        let (cx, cy) = ((p[0] / self.cell).floor() as i64, (p[1] / self.cell).floor() as i64);
        // The cells round p as far as the reach goes.
        let k = (reach / m / self.cell).ceil().max(1.0) as i64;
        let mut best: HashMap<usize, Foot> = HashMap::new();
        for x in cx - k..=cx + k {
            for y in cy - k..=cy + k {
                for &i in self.grid.get(&(x, y)).into_iter().flatten() {
                    let seg = &self.segs[i];
                    if seg.tube == own {
                        continue;
                    }
                    let d = [(seg.b[0] - seg.a[0]) * m, (seg.b[1] - seg.a[1]) * m];
                    let q = [(p[0] - seg.a[0]) * m, (p[1] - seg.a[1]) * m];
                    let l2 = d[0] * d[0] + d[1] * d[1];
                    let u = (q[0] * d[0] + q[1] * d[1]) / l2;
                    let past = (u < 0.0 && seg.s[0] <= 1e-9) || (u > 1.0 && seg.s[1] >= self.len[seg.tube] - 1e-9);
                    let u = u.clamp(0.0, 1.0);
                    let dist = (q[0] - u * d[0]).hypot(q[1] - u * d[1]);
                    if dist > reach || best.get(&seg.tube).is_some_and(|f| f.d <= dist) {
                        continue;
                    }
                    let z = seg.z[0] + u * (seg.z[1] - seg.z[0]);
                    let s = seg.s[0] + u * (seg.s[1] - seg.s[0]);
                    let l = l2.sqrt();
                    best.insert(seg.tube, Foot { tube: seg.tube, d: dist, z, s, dir: [d[0] / l, d[1] / l], past });
                }
            }
        }
        let mut out: Vec<Foot> = best.into_values().collect();
        out.sort_by_key(|f| f.tube);
        out
    }
}
