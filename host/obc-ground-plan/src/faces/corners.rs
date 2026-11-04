//! The inner radius rule: every concave corner of the carriageway union, and every arc of it tighter
//! than the inner radius of its class, is a corner. Each corner gets the closing of the union with
//! that radius, kept in the corner's zone, split between the two ways along the line from the corner
//! through the round's far point. Only the union's own outline is read, so two cells find the same
//! corners; ways that meet at no graph point in the zone are never joined.

use std::collections::BTreeMap;

use super::geom::{cross, P};
use super::stroke::{boolean, closing, dilate, disc, shapes, union, Rings, Rule};
use super::WayLine;

/// One arc step of the strokes, and a little: a concave vertex that turns more is a corner.
const STEP: f64 = 10.5 * std::f64::consts::PI / 180.0;
/// A corner's zone reaches this many radii past its own vertices.
const ZONE: f64 = 2.5;
/// A round that does not end inside its zone tries again this much smaller.
const SHRINK: f64 = 0.6;
/// A round is grown this many units into the carriageway, so its only edge is its arc.
const GROW: f64 = 3.0;

/// A part of a round: its rings, a point inside the carriageway that it belongs to, and that
/// carriageway's line and sample (for its bands).
pub struct Fill {
    pub rings: Rings,
    pub probe: P,
    pub line: usize,
    pub at: usize,
}

/// The segments of the lines by grid cells of `CELL` units, larger than the widest half-width.
struct Near<'a> {
    lines: &'a [WayLine<'a>],
    grid: BTreeMap<(i32, i32), Vec<(usize, usize)>>,
}

const CELL: i32 = 1 << 14;

impl<'a> Near<'a> {
    fn new(lines: &'a [WayLine<'a>]) -> Near<'a> {
        let mut grid: BTreeMap<(i32, i32), Vec<(usize, usize)>> = BTreeMap::new();
        for (l, line) in lines.iter().enumerate() {
            for i in 0..line.p.len().saturating_sub(1) {
                let (a, b) = (line.p[i], line.p[i + 1]);
                for x in a[0].min(b[0]).div_euclid(CELL)..=a[0].max(b[0]).div_euclid(CELL) {
                    for y in a[1].min(b[1]).div_euclid(CELL)..=a[1].max(b[1]).div_euclid(CELL) {
                        grid.entry((x, y)).or_default().push((l, i));
                    }
                }
            }
        }
        Near { lines, grid }
    }

    /// The line whose carriageway edge lies nearest q (q's distance past that edge), and its sample.
    fn at(&self, q: P) -> Option<(usize, usize, f64)> {
        let mut best: Option<(usize, usize, f64)> = None;
        let (cx, cy) = (q[0].div_euclid(CELL), q[1].div_euclid(CELL));
        for x in cx - 1..=cx + 1 {
            for y in cy - 1..=cy + 1 {
                for &(l, i) in self.grid.get(&(x, y)).into_iter().flatten() {
                    let line = &self.lines[l];
                    let (a, b) = (line.p[i], line.p[i + 1]);
                    let (dx, dy) = (f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
                    let len2 = dx * dx + dy * dy;
                    let t = if len2 > 0.0 {
                        ((f64::from(q[0] - a[0]) * dx + f64::from(q[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let (ex, ey) =
                        (f64::from(a[0]) + t * dx - f64::from(q[0]), f64::from(a[1]) + t * dy - f64::from(q[1]));
                    let off = (ex * ex + ey * ey).sqrt() - (line.h[i] + t * (line.h[i + 1] - line.h[i]));
                    let cand = (l, if t < 0.5 { i } else { i + 1 }, off);
                    if best.is_none_or(|b| (off, l, i) < (b.2, b.0, b.1)) {
                        best = Some(cand);
                    }
                }
            }
        }
        best
    }
}

fn dist(a: P, b: P) -> f64 {
    (f64::from(b[0] - a[0]).powi(2) + f64::from(b[1] - a[1]).powi(2)).sqrt()
}

/// The point `len` units back (or on) along a ring from vertex i.
fn walk(ring: &[P], i: usize, len: f64, back: bool) -> P {
    let n = ring.len();
    let mut s = 0.0;
    let mut k = i;
    loop {
        let j = if back { (k + n - 1) % n } else { (k + 1) % n };
        let d = dist(ring[k], ring[j]);
        if s + d >= len || j == i {
            let g = if d > 0.0 { ((len - s) / d).clamp(0.0, 1.0) } else { 0.0 };
            let (a, b) = (ring[k], ring[j]);
            return [
                a[0] + libm::round(g * f64::from(b[0] - a[0])) as i32,
                a[1] + libm::round(g * f64::from(b[1] - a[1])) as i32,
            ];
        }
        s += d;
        k = j;
    }
}

/// The rounds of the corners of `held`, the carriageway union. `r_in(line, sample)` is the inner
/// radius in units there; `meet(a, b, at, zone)` says whether lines a and b are arms of one graph
/// point within `zone` units of `at`.
pub fn fills(
    held: &Rings,
    lines: &[WayLine],
    r_in: &dyn Fn(usize, usize) -> f64,
    meet: &dyn Fn(usize, usize, P, f64) -> bool,
    min_r: f64,
) -> Vec<Fill> {
    let near_line = Near::new(lines);
    let nearest = |_: &[WayLine], q: P| near_line.at(q);
    // The corner vertices: (ring, vertex, turn, radius in units).
    let mut corners: Vec<(usize, usize, f64, f64)> = Vec::new();
    for (ri, ring) in held.iter().enumerate() {
        let n = ring.len();
        for i in 0..n {
            let (u, v, w) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            if cross(u, v, w) >= 0 {
                continue;
            }
            let (a, b) =
                ([f64::from(v[0] - u[0]), f64::from(v[1] - u[1])], [f64::from(w[0] - v[0]), f64::from(w[1] - v[1])]);
            let turn = libm::atan2((a[0] * b[1] - a[1] * b[0]).abs(), a[0] * b[0] + a[1] * b[1]);
            let Some((l, s, _)) = nearest(lines, v) else { continue };
            let r = r_in(l, s);
            if r <= 0.0 {
                continue;
            }
            let short = dist(u, v).min(dist(v, w));
            if turn > STEP || short / (2.0 * libm::tan(turn / 2.0)) < 0.9 * r {
                corners.push((ri, i, turn, r));
            }
        }
    }
    // The sharpest corner vertex (then the least position) leads a corner of all vertices within
    // its radius; then the next one left. A corner never chains along a narrow gap.
    let pos = |i: usize| held[corners[i].0][corners[i].1];
    let mut order: Vec<usize> = (0..corners.len()).collect();
    order.sort_by(|&i, &j| {
        corners[j].2.total_cmp(&corners[i].2).then((pos(i)[1], pos(i)[0]).cmp(&(pos(j)[1], pos(j)[0])))
    });
    let mut taken = vec![false; corners.len()];
    let mut out = Vec::new();
    for &top in &order {
        if taken[top] {
            continue;
        }
        let group: Vec<usize> =
            (0..corners.len()).filter(|&i| !taken[i] && dist(pos(top), pos(i)) <= corners[top].3).collect();
        for &i in &group {
            taken[i] = true;
        }
        let (ri, vi, _, _) = corners[top];
        let x = held[ri][vi];
        let r_in = group.iter().map(|&i| corners[i].3).fold(f64::INFINITY, f64::min);
        let spread = group.iter().map(|&i| dist(x, held[corners[i].0][corners[i].1])).fold(0.0, f64::max);
        // The two sides of the corner: the ways a radius back and on along the outline.
        let sides = [walk(&held[ri], vi, r_in, true), walk(&held[ri], vi, r_in, false)];
        let (Some(a), Some(b)) = (nearest(lines, sides[0]), nearest(lines, sides[1])) else { continue };
        if a.0 != b.0 && !meet(a.0, b.0, x, spread + ZONE * r_in) {
            continue;
        }
        // The largest radius up to the inner radius whose round ends inside its zone: land narrower
        // than 2r keeps its own end, a smaller round.
        let mut r = r_in;
        let mut found = None;
        while r >= min_r {
            let zone = spread + ZONE * r;
            let near = boolean(held, &[disc(x, zone + 2.0 * r)], Rule::Intersect, x);
            let closed = closing(&near, r, x);
            let added = boolean(&boolean(&closed, held, Rule::Difference, x), &[disc(x, zone)], Rule::Intersect, x);
            // Only the part at this corner: the one that meets its vertex.
            let added: Rings = shapes(&added, &[], Rule::Subject, x)
                .into_iter()
                .filter(|part| part.iter().flatten().any(|&q| dist(x, q) <= 2.0 * GROW))
                .flatten()
                .collect();
            if !added.iter().flatten().any(|&q| dist(x, q) >= zone - 2.0 * GROW) {
                found = Some((r, zone, closed, added));
                break;
            }
            r *= SHRINK;
        }
        let Some((r, zone, closed, added)) = found else { continue };
        if added.is_empty() {
            continue;
        }
        let grown = boolean(&dilate(&added, GROW, x), &closed, Rule::Intersect, x);
        // Split along the line from the corner through the round's far point: each half goes to
        // the carriageway on its side.
        let far = added.iter().flatten().copied().max_by(|&p, &q| dist(x, p).total_cmp(&dist(x, q))).unwrap();
        let d = [f64::from(far[0] - x[0]), f64::from(far[1] - x[1])];
        let len = (d[0] * d[0] + d[1] * d[1]).sqrt();
        if len == 0.0 {
            continue;
        }
        let (u, nrm) = ([d[0] / len, d[1] / len], [-d[1] / len, d[0] / len]);
        let big = 2.0 * (zone + r);
        let at = |s: f64, t: f64| {
            [x[0] + libm::round(s * u[0] + t * nrm[0]) as i32, x[1] + libm::round(s * u[1] + t * nrm[1]) as i32]
        };
        for side in [1.0, -1.0] {
            let half = vec![at(-big, 0.0), at(big, 0.0), at(big, side * big), at(-big, side * big)];
            for part in shapes(&grown, &[half], Rule::Intersect, x) {
                // The probe: a vertex of the part inside the carriageway, nearest the corner.
                let inside: Vec<P> =
                    part.iter().flatten().copied().filter(|&q| nearest(lines, q).is_some_and(|n| n.2 < -1.0)).collect();
                let Some(&probe) = inside.iter().min_by(|&&p, &&q| dist(x, p).total_cmp(&dist(x, q)).then(p.cmp(&q)))
                else {
                    continue;
                };
                let (line, at, _) = nearest(lines, probe).unwrap();
                out.push(Fill { rings: union(&part, x), probe, line, at });
            }
        }
    }
    out
}
