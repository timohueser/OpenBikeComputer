//! Where two tubes overlap (Tunnel parts in the spec, "overlapping tubes"): the distance from a point to the axis of
//! each tube near it, square to its chord, with the foot between the tube's two ends.

use std::collections::HashMap;

use super::floor::spaced;
use super::Tube;
use crate::metres;

/// A chord of a tube's axis: its tube, its ends in world units, their floors and metres along the chain.
struct Seg {
    tube: usize,
    a: [f64; 2],
    b: [f64; 2],
    z: [f64; 2],
    s: [f64; 2],
}

/// The axes of all tubes by cells of `CELL_M`.
pub(crate) struct Axes {
    segs: Vec<Seg>,
    grid: HashMap<(i64, i64), Vec<usize>>,
    cell: f64,
    len: Vec<f64>,
}

/// The side of a cell: wider than the farthest reach of a test (two halves and their shells).
const CELL_M: f64 = 32.0;

/// Another tube's axis near a point: the tube, the metres to it, its floor and its metres along there.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Foot {
    pub tube: usize,
    pub d: f64,
    pub z: f64,
    pub s: f64,
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

    /// The axis of each tube but `own` within `reach` metres of p, nearest per tube; a foot beyond a tube's end does not
    /// count.
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
                    let first = seg.s[0] <= 1e-9;
                    let last = seg.s[1] >= self.len[seg.tube] - 1e-9;
                    if (u < 0.0 && first) || (u > 1.0 && last) {
                        continue;
                    }
                    let u = u.clamp(0.0, 1.0);
                    let dist = (q[0] - u * d[0]).hypot(q[1] - u * d[1]);
                    if dist > reach || best.get(&seg.tube).is_some_and(|f| f.d <= dist) {
                        continue;
                    }
                    let z = seg.z[0] + u * (seg.z[1] - seg.z[0]);
                    let s = seg.s[0] + u * (seg.s[1] - seg.s[0]);
                    best.insert(seg.tube, Foot { tube: seg.tube, d: dist, z, s });
                }
            }
        }
        let mut out: Vec<Foot> = best.into_values().collect();
        out.sort_by_key(|f| f.tube);
        out
    }
}

/// Two tubes of heights `ha` and `hb` whose floors lie `dz` apart are at one level.
pub(crate) fn level(dz: f64, ha: f64, hb: f64) -> bool {
    dz.abs() <= (ha + hb) / 2.0
}

/// Per tube with a part, its vertices between its openings: `base`, then where its shell starts or stops overlapping
/// the shell of another such tube at one level (read every metre, each place found to a centimetre), spaced by `gap`
/// (a vertex nearer than that to another is that vertex). `spans` gives each tube's metres from opening to opening,
/// `floor` its floor and inside height at metres along.
pub(crate) fn meeting_vertices(
    axes: &Axes,
    tubes: &[Tube],
    (spans, half): (&[Option<[f64; 2]>], &[f64]),
    floor: &dyn Fn(usize, f64) -> (f64, f64),
    (base, shell, gap): (&[Vec<f64>], f64, f64),
) -> Vec<Vec<f64>> {
    let reach = 2.0 * half.iter().fold(0.0f64, |a, &h| a.max(h)) + 2.0 * shell;
    let within = |t: usize, s: f64| spans[t].is_some_and(|sp| s >= sp[0] && s <= sp[1]);
    // The tubes that tube t meets at metres s along.
    let meets = |t: usize, s: f64| -> Vec<usize> {
        let q = tubes[t].curve.at(s).0;
        let (z, h) = floor(t, s);
        (axes.near(q.p, t, reach).into_iter())
            .filter(|f| within(f.tube, f.s))
            .filter(|f| {
                let (zo, ho) = floor(f.tube, f.s);
                f.d < half[t] + half[f.tube] + 2.0 * shell && level(zo - z, h, ho)
            })
            .map(|f| f.tube)
            .collect()
    };
    (0..tubes.len())
        .map(|t| {
            let Some([s0, s1]) = spans[t] else { return spaced(&[], base[t].clone(), gap) };
            let steps = (s1 - s0).ceil().max(1.0) as usize;
            let at = |k: usize| s0 + (s1 - s0) * k as f64 / steps as f64;
            let mut out = Vec::new();
            let mut before = meets(t, s0);
            for k in 1..=steps {
                let now = meets(t, at(k));
                for &o in before.iter().filter(|o| !now.contains(o)).chain(now.iter().filter(|o| !before.contains(o))) {
                    // Halves to the place where o starts or stops meeting t.
                    let (mut lo, mut hi) = (at(k - 1), at(k));
                    let was = before.contains(&o);
                    for _ in 0..7 {
                        let mid = (lo + hi) / 2.0;
                        if meets(t, mid).contains(&o) == was {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    out.push((lo + hi) / 2.0);
                }
                before = now;
            }
            out.sort_by(f64::total_cmp);
            spaced(&out, base[t].clone(), gap)
        })
        .collect()
}
