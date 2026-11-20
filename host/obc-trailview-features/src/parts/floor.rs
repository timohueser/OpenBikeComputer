//! The floor of a tube between its openings: the crown's shell lies a slab under each way on top, within the grade of
//! the class, and the section gets lower where the grade does not allow that.

use std::collections::HashSet;

use serde_json::{json, Value};

use super::measure::meet;
use super::{distance, Input, Point, Tube};
use crate::geometry::{bounds, interp, key, Boxes};
use crate::{degrees, roads};

/// A way on top of a tube: the metres along the tube of its crossing and of the window where its band lies over the
/// tube's shell, its underside there, and the crossing point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Top {
    pub s: f64,
    pub window: [f64; 2],
    pub under: f64,
    pub p: [f64; 2],
}

/// The floor of a tube: per vertex its metres, the road solver's height, the floor and the inside height; per top the
/// metres that the shell of the crown rises over its underside (0 or less where it stays under).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Floor {
    pub s: Vec<f64>,
    pub z: Vec<f64>,
    pub f: Vec<f64>,
    pub h: Vec<f64>,
    pub over: Vec<f64>,
}

impl Floor {
    /// The floor and the inside height at metres s: exact at a vertex.
    pub fn at(&self, s: f64) -> (f64, f64) {
        (interp(s, &self.s, &self.f), interp(s, &self.s, &self.h))
    }
}

/// A crossing nearer than this to a vertex of a tunnel way that the way on top shares lies at that vertex; a window
/// end nearer than this to a vertex of the tube is that vertex.
const JOINED_M: f64 = 0.01;

/// The ways on top of each line (the points of a tube from opening to opening, its inside half-width): each crossing
/// of the curve of a way with heights that is not of the kind `tunnel` or `gallery`. A way that meets a tunnel way at a
/// shared vertex joins it there and does not cross on top.
pub(crate) fn tops(lines: &[(Vec<Point>, f64)], input: &Input) -> Vec<Vec<Top>> {
    let b = &input.rules.bake;
    let mut out: Vec<Vec<Top>> = vec![Vec::new(); lines.len()];
    let boxes = Boxes::new(&lines.iter().map(|(l, _)| bounds(l.iter().map(|q| &q.p))).collect::<Vec<_>>());
    let joined: HashSet<[u64; 2]> = input
        .tunnels
        .values()
        .flat_map(|(_, parts)| parts.iter().flat_map(|p| p.points.iter().map(|&q| key(q))))
        .collect();
    for (w, (kind, tags, parts)) in input.ways.iter().enumerate() {
        if *kind == "tunnel" || *kind == "gallery" {
            continue;
        }
        let outer = input.widths.way(kind, tags, false).outer_m();
        let depth = input.decks.get(&w).copied().unwrap_or(b.slab_m);
        for part in parts.iter().filter(|p| p.z.is_some()) {
            let curve = roads::polyline(part);
            let z = curve.z.as_ref().unwrap();
            let shared: Vec<[f64; 2]> = part.points.iter().copied().filter(|&q| joined.contains(&key(q))).collect();
            for t in boxes.overlapping(bounds(&curve.points)) {
                let (line, half) = &lines[t];
                let reach = outer + half + b.shell_m;
                // A crossing at a vertex of either curve is found on both chords there: it is one.
                let mut found: Vec<Top> = Vec::new();
                for (k, w2) in curve.points.windows(2).enumerate() {
                    for j in 0..line.len().saturating_sub(1) {
                        let (p, q) = (line[j], line[j + 1]);
                        let Some((u, v)) = meet(w2[0], w2[1], p.p, q.p) else { continue };
                        let x = [w2[0][0] + u * (w2[1][0] - w2[0][0]), w2[0][1] + u * (w2[1][1] - w2[0][1])];
                        if shared.iter().any(|&e| distance(x, e) < JOINED_M) {
                            continue;
                        }
                        let under = z[k] + u * (z[k + 1] - z[k]) - depth;
                        if !under.is_finite() {
                            continue;
                        }
                        let (dw, dt) = ([w2[1][0] - w2[0][0], w2[1][1] - w2[0][1]], [q.p[0] - p.p[0], q.p[1] - p.p[1]]);
                        let sine = (dw[0] * dt[1] - dw[1] * dt[0]).abs() / (dw[0].hypot(dw[1]) * dt[0].hypot(dt[1]));
                        let s = p.s + v * (q.s - p.s);
                        let half_window = reach / sine.max(b.cross_sin_min);
                        let (lo, hi) = (line[0].s, line[line.len() - 1].s);
                        let window = [(s - half_window).max(lo), (s + half_window).min(hi)];
                        if !found.iter().any(|f| distance(f.p, x) < JOINED_M) {
                            found.push(Top { s, window, under, p: x });
                        }
                    }
                }
                out[t].extend(found);
            }
        }
    }
    for tops in &mut out {
        tops.sort_by(|x, y| x.s.total_cmp(&y.s).then(x.under.total_cmp(&y.under)));
    }
    out
}

/// The floor of a line of (metres, height) from opening to opening under its tops: `g` the steepest grade, `height`
/// and `least` the inside height and the least height of its group, `shell` the shell of the crown.
pub(crate) fn floor(line: &[(f64, f64)], tops: &[Top], g: f64, (height, least): (f64, f64), shell: f64) -> Floor {
    let (ls, lz): (Vec<f64>, Vec<f64>) = line.iter().copied().unzip();
    let mut s: Vec<f64> = ls.clone();
    let mut ends: Vec<f64> = tops.iter().flat_map(|t| t.window).collect();
    ends.sort_by(f64::total_cmp);
    for e in ends {
        let at = s.partition_point(|&x| x < e);
        let near = |k: usize| s.get(k).is_some_and(|&x| (x - e).abs() < JOINED_M);
        if !near(at) && !(at > 0 && near(at - 1)) {
            s.insert(at, e);
        }
    }
    let n = s.len();
    let z: Vec<f64> = s.iter().map(|&x| interp(x, &ls, &lz)).collect();
    let holds = |t: &Top, x: f64| t.window[0] - JOINED_M <= x && x <= t.window[1] + JOINED_M;
    // The cap: the floor under which the crown's shell clears the underside of each top whose window holds the vertex.
    let cap: Vec<f64> = (s.iter())
        .map(|&x| tops.iter().filter(|t| holds(t, x)).map(|t| t.under).fold(f64::INFINITY, f64::min) - shell - height)
        .collect();
    let mut f: Vec<f64> = (0..n).map(|i| if i == 0 || i + 1 == n { z[i] } else { z[i].min(cap[i]) }).collect();
    for i in 1..n {
        f[i] = f[i].min(f[i - 1] + g * (s[i] - s[i - 1]));
    }
    for i in (0..n.saturating_sub(1)).rev() {
        f[i] = f[i].min(f[i + 1] + g * (s[i + 1] - s[i]));
    }
    // The cones of the two openings keep the grade too, so the floor holds the heights of both.
    for i in 0..n {
        f[i] = f[i].max(z[0] - g * (s[i] - s[0])).max(z[n - 1] - g * (s[n - 1] - s[i]));
    }
    let h: Vec<f64> = (0..n)
        .map(|i| if f[i] > cap[i] { (height - (f[i] - cap[i])).max(least).min(height) } else { height })
        .collect();
    let over = (tops.iter())
        .map(|t| {
            let within = (0..n).filter(|&i| holds(t, s[i]));
            within.map(|i| f[i] + h[i] + shell - t.under).fold(f64::NEG_INFINITY, f64::max)
        })
        .collect();
    Floor { s, z, f, h, over }
}

/// The floors of the tubes with a part (`spans`: the metres of their two openings), and their counts.
pub(crate) fn floors(
    tubes: &[Tube],
    spans: &[Option<[f64; 2]>],
    (half, height): (&[f64], &[f64]),
    input: &Input,
) -> (Vec<Option<Floor>>, Value) {
    let b = &input.rules.bake;
    let with: Vec<usize> = (0..tubes.len()).filter(|&t| spans[t].is_some()).collect();
    let lines: Vec<(Vec<Point>, f64)> = (with.iter()).map(|&t| (line(&tubes[t], spans[t].unwrap()), half[t])).collect();
    let tops = tops(&lines, input);
    let mut out: Vec<Option<Floor>> = vec![None; tubes.len()];
    let (mut lowered, mut low, mut at_least, mut lows, mut clashes) = (0, 0, 0, Vec::new(), Vec::new());
    for ((&t, (points, _)), tops) in with.iter().zip(&lines).zip(&tops) {
        let tube = &tubes[t];
        let least = input.rules.groups[tube.group].least_height_m;
        let rank = input.class_names.iter().position(|c| *c == tube.class);
        let class_grade =
            rank.map(|r| input.classes.rows[r].grade).filter(|g| g.is_finite()).unwrap_or(b.grade_default);
        let steepest = (points.windows(2))
            .filter(|w| w[1].s > w[0].s)
            .map(|w| (w[1].z - w[0].z).abs() / (w[1].s - w[0].s))
            .fold(0.0, f64::max);
        let line: Vec<(f64, f64)> = points.iter().map(|q| (q.s, q.z)).collect();
        let fl = floor(&line, tops, class_grade.max(steepest), (height[t], least), b.shell_m);
        lowered += usize::from(fl.f.iter().zip(&fl.z).any(|(f, z)| *f < z - 0.005));
        // A low stretch: a run of chords (the lesser height of their two vertices) under the tube's height, and its
        // lowest vertex.
        let chords: Vec<f64> = fl.h.windows(2).map(|w| w[0].min(w[1])).collect();
        let mut run: Option<usize> = None;
        for k in 0..=chords.len() {
            match (run, chords.get(k).filter(|&&h| h < height[t] - 0.005)) {
                (None, Some(_)) => run = Some(if fl.h[k] <= fl.h[k + 1] { k } else { k + 1 }),
                (Some(r), Some(_)) if fl.h[k + 1] < fl.h[r] => run = Some(k + 1),
                (Some(r), None) => {
                    low += 1;
                    at_least += usize::from(fl.h[r] <= least + 0.005);
                    lows.push((height[t] - fl.h[r], tube.curve.at(fl.s[r]).0.p, &tube.class));
                    run = None;
                }
                _ => {}
            }
        }
        for (top, &over) in tops.iter().zip(&fl.over) {
            if over > 0.005 {
                clashes.push((over, top.p, &tube.class));
            }
        }
        out[t] = Some(fl);
    }
    let places = |mut list: Vec<(f64, [f64; 2], &String)>, sign: f64| {
        list.sort_by(|x, y| y.0.total_cmp(&x.0));
        let shown = list.iter().take(10).map(|(m, p, class)| {
            let [lat, lon] = degrees(*p);
            format!("{lat:.5},{lon:.5} {class} {:+.2} m", sign * m)
        });
        shown.collect::<Vec<_>>().join("; ")
    };
    let metrics = json!({
        "floors": with.len(),
        "tops": tops.iter().map(Vec::len).sum::<usize>(),
        "tubes_lowered": lowered,
        "low_stretches": low,
        "low_stretches_at_least_height": at_least,
        "lowest_stretches": places(lows, -1.0),
        "floor_clashes": clashes.len(),
        "floor_clash_places": places(clashes, 1.0),
    });
    (out, metrics)
}

/// The points of a tube from metres a to b along its chain: a point at each end and the points of its curve between,
/// each metre once.
pub(crate) fn line(tube: &Tube, [a, b]: [f64; 2]) -> Vec<Point> {
    let mut out = vec![tube.curve.at(a).0];
    let inner = tube.curve.stretches.iter().flat_map(|(_, ps)| ps).filter(|q| q.s > a && q.s < b);
    for q in inner {
        if out.last().is_some_and(|l| l.s < q.s) {
            out.push(*q);
        }
    }
    out.push(tube.curve.at(b).0);
    out
}
