//! The approach to a built tunnel opening (`WayGraph::mouths`): a way that ends at the opening widens its core there to
//! the farther of the jamb lines of its hole, so its walls or banks stand on them and meet the head wall, and narrows
//! back to its own width over `approach_m` (`obc-trailview-parts`), or over the way where it is shorter.

use super::{Ctx, WayLine, LAYERS, SHOULDER, VERGE};
use crate::graph::MIN_STEP_M;

/// A line's end lies at an opening where it lies this near the opening's point (m).
const AT_MOUTH_M: f64 = 0.5;

/// Widens the cores of the lines that end at a mouth of the graph.
pub(super) fn widen(ctx: &Ctx, lines: &mut [WayLine]) {
    if ctx.graph.mouths.is_empty() {
        return;
    }
    let approach_m = obc_trailview_parts::Rules::embedded().design.approach_m;
    for line in lines.iter_mut().filter(|l| l.p.len() >= 2) {
        let n = line.p.len();
        // Per end at a mouth: the end's vertex, the widening there (units) and its length (units).
        let mut ends: Vec<(usize, f64, f64)> = Vec::new();
        for (at, next) in [(0, 1), (n - 1, n - 2)] {
            let g = ctx.global(line.p[at]).map(|v| v as f64);
            let Some(m) = ctx.graph.mouths.iter().find(|m| {
                let d = [m.at[0] - g[0], m.at[1] - g[1]];
                let away = [f64::from(line.p[next][0] - line.p[at][0]), f64::from(line.p[next][1] - line.p[at][1])];
                libm::hypot(d[0], d[1]) <= ctx.units(AT_MOUTH_M, line.p[at])
                    && away[0] * m.into[0] + away[1] * m.into[1] < 0.0
            }) else {
                continue;
            };
            let wider = ctx.units(m.jamb_m[1].max(-m.jamb_m[0]), line.p[at]) - line.bands[at][VERGE];
            if wider > 0.0 {
                ends.push((at, wider, ctx.units(approach_m, line.p[at]).min(length(&line.p))));
            }
        }
        if ends.is_empty() {
            continue;
        }
        // A vertex where each widening ends, unless one lies near.
        let gap = ctx.units(MIN_STEP_M / 2.0, line.p[0]);
        for &(at, _, reach) in &ends {
            let last = line.p.len() - 1;
            let s = along(&line.p, if at == 0 { 0 } else { last });
            let Some(i) = (0..last).find(|&i| (s[i] - reach) * (s[i + 1] - reach) < 0.0) else { continue };
            let x = (reach - s[i]) / (s[i + 1] - s[i]);
            if (s[i + 1] - s[i]) * x.min(1.0 - x) >= gap {
                insert(line, i, x);
            }
        }
        // The shoulder, at the road's height, and each layer out of it move by the widest widening at its vertex: the
        // core's edge, where a wall or a bank stands, reaches the jamb line.
        let n = line.p.len();
        let ats: Vec<usize> = ends.iter().map(|&(at, ..)| if at == 0 { 0 } else { n - 1 }).collect();
        let moved: Vec<f64> = {
            let s: Vec<Vec<f64>> = ats.iter().map(|&at| along(&line.p, at)).collect();
            (0..n)
                .map(|i| {
                    (ends.iter().zip(&s))
                        .map(|(&(_, wider, reach), s)| wider * (1.0 - s[i] / reach).max(0.0))
                        .fold(0.0, f64::max)
                })
                .collect()
        };
        for (bands, d) in line.bands.iter_mut().zip(moved) {
            for b in &mut bands[SHOULDER..LAYERS] {
                *b += d;
            }
        }
    }
}

/// The length of a line (units).
fn length(p: &[[i32; 2]]) -> f64 {
    along(p, 0)[p.len() - 1]
}

/// The distance of each vertex from vertex `at` along the line (units).
fn along(p: &[[i32; 2]], at: usize) -> Vec<f64> {
    let step = |a: [i32; 2], b: [i32; 2]| libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    let mut s = vec![0.0; p.len()];
    for i in at + 1..p.len() {
        s[i] = s[i - 1] + step(p[i - 1], p[i]);
    }
    for i in (0..at).rev() {
        s[i] = s[i + 1] + step(p[i], p[i + 1]);
    }
    s
}

/// A vertex at share x of segment i, with the segment's offsets there and its first vertex's key and walk.
fn insert(line: &mut WayLine, i: usize, x: f64) {
    let (a, b) = (line.p[i], line.p[i + 1]);
    let q = [0, 1].map(|c| a[c] + libm::round(x * f64::from(b[c] - a[c])) as i32);
    let mix = |u: f64, v: f64| u + x * (v - u);
    let bands = std::array::from_fn(|k| mix(line.bands[i][k], line.bands[i + 1][k]));
    line.p.insert(i + 1, q);
    line.h.insert(i + 1, mix(line.h[i], line.h[i + 1]));
    line.bands.insert(i + 1, bands);
    line.key.insert(i + 1, line.key[i]);
    line.walk.insert(i + 1, line.walk[i]);
    for k in line.index.iter_mut().filter(|k| **k > i) {
        *k += 1;
    }
}
