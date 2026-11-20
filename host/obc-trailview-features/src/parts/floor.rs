//! The floor of a tube between its openings: the crown's shell lies a slab under each way on top, within the grade of
//! the class, and the section gets lower where the grade does not allow that.

use std::collections::BTreeMap;

use obc_ground_plan::faces::Rules as PlanRules;
use serde_json::{json, Value};

use super::{distance, segment_m, Input, Point, Tube};
use crate::geometry::{bounds, interp, Boxes};
use crate::{degrees, metres, roads};

/// A way that can lie over a tube: its place in the ways, its curve with heights, the depth of its underside under the
/// curve, its outer half-width and its at-grade band. A way of the kind `tunnel` is counted, and it lowers no floor.
struct Over {
    way: usize,
    points: Vec<[f64; 2]>,
    z: Vec<f64>,
    depth: f64,
    outer: f64,
    band: f64,
    tunnel: bool,
}

/// A way over a vertex of a tube: its number among the ways over the tube, its height and underside at its nearest
/// point, the metres along the tube of that point's foot, and the at-grade band of the way and the tube.
struct Above {
    way: usize,
    height: f64,
    under: f64,
    along: f64,
    band: f64,
}

/// The floor of a tube: per vertex its metres, the road solver's height, the floor and the inside height.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Floor {
    pub s: Vec<f64>,
    pub z: Vec<f64>,
    pub f: Vec<f64>,
    pub h: Vec<f64>,
}

impl Floor {
    /// The floor and the inside height at metres s: exact at a vertex.
    pub fn at(&self, s: f64) -> (f64, f64) {
        (interp(s, &self.s, &self.f), interp(s, &self.s, &self.h))
    }

    /// The inside height of the chord at an end of the tube (0 its first vertex, 1 its last).
    pub fn end_height(&self, end: usize) -> f64 {
        let n = self.h.len();
        if end == 0 {
            self.h[0].min(self.h[1])
        } else {
            self.h[n - 1].min(self.h[n - 2])
        }
    }
}

/// A vertex nearer than this to another is that vertex, and a point this far past a reach is within it.
const MERGE_M: f64 = 0.01;

/// The floor of a line of metres `s` and road solver's heights `z` from opening to opening: `under` holds per vertex
/// the undersides of the ways on top there; `grade` is the grade of the class, `height` and `least` the inside height
/// and the least height of the group, `shell` the shell of the crown.
pub(crate) fn floor(
    (s, z): (&[f64], &[f64]),
    under: &[Vec<f64>],
    grade: f64,
    (height, least): (f64, f64),
    shell: f64,
) -> Floor {
    let n = s.len();
    // The cap: the floor under which the crown's shell clears the underside of each way on top of the vertex.
    let cap: Vec<f64> =
        under.iter().map(|u| u.iter().copied().fold(f64::INFINITY, f64::min) - shell - height).collect();
    // A chord may rise by the grade of the class, or by as much as the road solver's chord there.
    let rise: Vec<f64> = (1..n).map(|i| (grade * (s[i] - s[i - 1])).max((z[i] - z[i - 1]).abs())).collect();
    let mut f: Vec<f64> = (0..n).map(|i| if i == 0 || i + 1 == n { z[i] } else { z[i].min(cap[i]) }).collect();
    for i in 1..n {
        f[i] = f[i].min(f[i - 1] + rise[i - 1]);
    }
    for i in (0..n - 1).rev() {
        f[i] = f[i].min(f[i + 1] + rise[i]);
    }
    // The cones of the two openings keep the rise of each chord too, so the floor holds the heights of both.
    let mut down = 0.0;
    for i in 0..n {
        down += if i > 0 { rise[i - 1] } else { 0.0 };
        f[i] = f[i].max(z[0] - down);
    }
    let mut up = 0.0;
    for i in (0..n).rev() {
        up += if i + 1 < n { rise[i] } else { 0.0 };
        f[i] = f[i].max(z[n - 1] - up);
    }
    let h: Vec<f64> = (0..n)
        .map(|i| if f[i] > cap[i] { (height - (f[i] - cap[i])).max(least).min(height) } else { height })
        .collect();
    Floor { s: s.to_vec(), z: z.to_vec(), f, h }
}

/// The metres from q to the segment a-b, and the share along it of the nearest point.
fn nearest(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, f64) {
    let d = [b[0] - a[0], b[1] - a[1]];
    let l2 = d[0] * d[0] + d[1] * d[1];
    let t = if l2 > 0.0 { (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (distance(q, [a[0] + t * d[0], a[1] + t * d[1]]), t)
}

/// The shares along p-q that lie within `reach` metres of the segment a-b: one run, because the metres to a segment
/// are convex along a line.
fn within(p: [f64; 2], q: [f64; 2], (a, b): ([f64; 2], [f64; 2]), reach: f64) -> Option<[f64; 2]> {
    let at = |t: f64| segment_m([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])], a, b);
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..40 {
        let (m1, m2) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if at(m1) <= at(m2) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    let best = (lo + hi) / 2.0;
    if at(best) > reach {
        return None;
    }
    let edge = |mut inside: f64, mut outside: f64| {
        if at(outside) <= reach {
            return outside;
        }
        for _ in 0..40 {
            let mid = (inside + outside) / 2.0;
            if at(mid) <= reach {
                inside = mid;
            } else {
                outside = mid;
            }
        }
        inside
    };
    Some([edge(best, 0.0), edge(best, 1.0)])
}

/// The box of the points grown by `m` metres.
fn grown(points: &[[f64; 2]], m: f64) -> [f64; 4] {
    let b = bounds(points);
    let u = m / metres((b[1] + b[3]) / 2.0);
    [b[0] - u, b[1] - u, b[2] + u, b[3] + u]
}

/// The ways that can lie over the lines (`reach`: the half-width of each line with its shell): every way with heights
/// but a gallery, with its curve (`roads::polyline`).
fn overs(lines: &[(Vec<[f64; 2]>, f64)], input: &Input) -> Vec<Over> {
    let sections = &PlanRules::embedded().sections;
    let index = Boxes::new(&lines.iter().map(|(ps, reach)| grown(ps, *reach)).collect::<Vec<_>>());
    let mut out = Vec::new();
    for (w, (kind, tags, parts)) in input.ways.iter().enumerate() {
        if *kind == "gallery" {
            continue;
        }
        let outer = input.widths.way(kind, tags, false).outer_m();
        for part in parts.iter().filter(|p| p.z.is_some() && p.points.len() >= 2) {
            let curve = roads::polyline(part);
            if index.overlapping(grown(&curve.points, outer)).is_empty() {
                continue;
            }
            out.push(Over {
                way: w,
                points: curve.points,
                z: curve.z.unwrap(),
                depth: input.decks.get(&w).copied().unwrap_or(input.rules.bake.slab_m),
                outer,
                band: sections.way(kind, tags, false).snap_height_m,
                tunnel: *kind == "tunnel",
            });
        }
    }
    out
}

/// The floors of the tubes with a part (`spans`: the metres of their two openings; `half` and `height` their inside
/// half-width and height), and their counts.
pub(crate) fn floors(
    tubes: &[Tube],
    spans: &[Option<[f64; 2]>],
    (half, height): (&[f64], &[f64]),
    input: &Input,
) -> (Vec<Option<Floor>>, Value) {
    let b = &input.rules.bake;
    let sections = &PlanRules::embedded().sections;
    let with: Vec<usize> = (0..tubes.len()).filter(|&t| spans[t].is_some()).collect();
    let lines: Vec<Vec<Point>> = with.iter().map(|&t| line(&tubes[t], spans[t].unwrap())).collect();
    let boxes: Vec<(Vec<[f64; 2]>, f64)> =
        (with.iter().zip(&lines)).map(|(&t, l)| (l.iter().map(|q| q.p).collect(), half[t] + b.shell_m)).collect();
    let overs = overs(&boxes, input);
    // Each segment of a way over a line, its box grown by the way's outer half-width.
    let segments: Vec<(usize, usize)> =
        (overs.iter().enumerate()).flat_map(|(o, w)| (0..w.points.len() - 1).map(move |k| (o, k))).collect();
    let index = Boxes::new(
        &segments.iter().map(|&(o, k)| grown(&overs[o].points[k..k + 2], overs[o].outer)).collect::<Vec<_>>(),
    );
    let mut out: Vec<Option<Floor>> = vec![None; tubes.len()];
    let (mut lowered, mut low, mut at_least, mut lows) = (0, 0, 0, Vec::new());
    let (mut tops, mut clashes, mut tube_clashes) = (0, Vec::new(), Vec::new());
    for ((&t, points), (ps, shell_half)) in with.iter().zip(&lines).zip(&boxes) {
        let tube = &tubes[t];
        let reach_of = |o: &Over| o.outer + shell_half;
        let ids: Vec<usize> = tube.ids.iter().map(|&id| id as usize).collect();
        // The segments of the ways near a box, but those of the tube's own ways.
        let near = |area: [f64; 4]| {
            (index.overlapping(area).into_iter())
                .map(|i| segments[i])
                .filter(|&(o, _)| !(overs[o].tunnel && ids.contains(&overs[o].way)))
        };
        // The vertices: the curve's, and the ends of each stretch of the tube within the reach of a way that can lower
        // the floor.
        let mut runs: BTreeMap<usize, Vec<[f64; 2]>> = BTreeMap::new();
        let mut extra: Vec<f64> = Vec::new();
        for j in 0..ps.len() - 1 {
            let ds = points[j + 1].s - points[j].s;
            for (o, k) in near(grown(&ps[j..j + 2], *shell_half)).filter(|&(o, _)| !overs[o].tunnel) {
                let w = &overs[o];
                let reach = reach_of(w);
                if let Some([a, e]) = within(ps[j], ps[j + 1], (w.points[k], w.points[k + 1]), reach) {
                    runs.entry(o).or_default().push([points[j].s + a * ds, points[j].s + e * ds]);
                }
            }
        }
        for mut list in runs.into_values() {
            list.sort_by(|x, y| x[0].total_cmp(&y[0]));
            let mut run = list[0];
            for r in list.into_iter().skip(1).chain([[f64::INFINITY; 2]]) {
                if r[0] <= run[1] + MERGE_M {
                    run[1] = run[1].max(r[1]);
                } else {
                    extra.extend(run);
                    run = r;
                }
            }
        }
        let (s0, s1) = (points[0].s, points[points.len() - 1].s);
        let mut s: Vec<f64> = points.iter().map(|q| q.s).collect();
        extra.retain(|&e| e > s0 && e < s1);
        extra.sort_by(f64::total_cmp);
        for e in extra {
            let at = s.partition_point(|&x| x < e);
            let close = |k: usize| s.get(k).is_some_and(|&x| (x - e).abs() < MERGE_M);
            if !close(at) && !(at > 0 && close(at - 1)) {
                s.insert(at, e);
            }
        }
        let (ls, lz): (Vec<f64>, Vec<f64>) = points.iter().map(|q| (q.s, q.z)).unzip();
        let z: Vec<f64> = s.iter().map(|&x| interp(x, &ls, &lz)).collect();
        let xy: Vec<[f64; 2]> = s.iter().map(|&x| tube.curve.at(x).0.p).collect();
        // The metres along the tube of the foot of a point on it, which lies within twice the reach of the vertex that
        // the point is near.
        let foot = |q: [f64; 2], (at, reach): (f64, f64)| {
            let first = points.partition_point(|v| v.s < at - 2.0 * reach).saturating_sub(1);
            let last = points.partition_point(|v| v.s <= at + 2.0 * reach).min(ps.len() - 1);
            let (_, j, u) = (first..last.max(first + 1))
                .map(|j| {
                    let (m, u) = nearest(q, ps[j], ps[j + 1]);
                    (m, j, u)
                })
                .fold((f64::INFINITY, 0, 0.0), |a, x| if x.0 < a.0 { x } else { a });
            points[j].s + u * (points[j + 1].s - points[j].s)
        };
        // Per vertex each way over it: within its reach, with its nearest point over the tube between the openings;
        // its height and underside there, the metres of the foot and the at-grade band of the two.
        let own_band = sections.way("tunnel", &input.tunnels[&tube.widest].0, false).snap_height_m;
        let mut local: BTreeMap<usize, usize> = BTreeMap::new();
        let mut above: Vec<Vec<Above>> = (0..s.len()).map(|_| Vec::new()).collect();
        for (i, &p) in xy.iter().enumerate() {
            // Per way its nearest segment: metres, segment and share along it.
            let mut best: BTreeMap<usize, (f64, usize, f64)> = BTreeMap::new();
            for (o, k) in near(grown(&[p], *shell_half)) {
                let (m, u) = nearest(p, overs[o].points[k], overs[o].points[k + 1]);
                let e = best.entry(o).or_insert((f64::INFINITY, 0, 0.0));
                if m < e.0 {
                    *e = (m, k, u);
                }
            }
            for (o, (m, k, u)) in best {
                let w = &overs[o];
                if m > reach_of(w) + MERGE_M {
                    continue;
                }
                let (a, e) = (w.points[k], w.points[k + 1]);
                let along = foot([a[0] + u * (e[0] - a[0]), a[1] + u * (e[1] - a[1])], (s[i], reach_of(w)));
                if along > s0 + MERGE_M && along < s1 - MERGE_M {
                    let c = local.len();
                    let height = w.z[k] + u * (w.z[k + 1] - w.z[k]);
                    let (way, under, band) = (*local.entry(o).or_insert(c), height - w.depth, w.band.min(own_band));
                    above[i].push(Above { way, height, under, along, band });
                }
            }
        }
        let ways: Vec<&Over> = {
            let mut v: Vec<(usize, usize)> = local.iter().map(|(&o, &c)| (c, o)).collect();
            v.sort_unstable();
            v.into_iter().map(|(_, o)| &overs[o]).collect()
        };
        let least = input.rules.groups[tube.group].least_height_m;
        let rank = input.class_names.iter().position(|c| *c == tube.class);
        let grade = rank.map(|r| input.classes.rows[r].grade).filter(|g| g.is_finite()).unwrap_or(b.grade_default);
        // A way lies on top where it lies over the road solver's height of the tube by more than the at-grade band;
        // a way of the kind `tunnel` lowers no floor.
        let caps: Vec<Vec<f64>> = (above.iter())
            .map(|v| {
                let on_top = v.iter().filter(|a| !ways[a.way].tunnel && a.height - interp(a.along, &s, &z) > a.band);
                on_top.map(|a| a.under).collect()
            })
            .collect();
        let fl = floor((&s, &z), &caps, grade, (height[t], least), b.shell_m);
        // Over the floor as it is, per way the most metres that the shell of the crown rises over its underside where
        // it lies over the floor by more than the band, and the vertex there.
        let mut over: Vec<Option<(f64, usize)>> = vec![None; ways.len()];
        for (i, v) in above.iter().enumerate() {
            for a in v.iter().filter(|a| a.height - interp(a.along, &fl.s, &fl.f) > a.band) {
                let m = fl.f[i] + fl.h[i] + b.shell_m - a.under;
                if over[a.way].is_none_or(|o| m > o.0) {
                    over[a.way] = Some((m, i));
                }
            }
        }
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
                    lows.push((height[t] - fl.h[r], xy[r], &tube.class));
                    run = None;
                }
                _ => {}
            }
        }
        for (c, o) in ways.iter().enumerate() {
            let Some((m, i)) = over[c] else { continue };
            tops += usize::from(!o.tunnel);
            if m > 0.005 {
                let list = if o.tunnel { &mut tube_clashes } else { &mut clashes };
                list.push((m, xy[i], &tube.class));
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
        "tops": tops,
        "tubes_lowered": lowered,
        "low_stretches": low,
        "low_stretches_at_least_height": at_least,
        "lowest_stretches": places(lows, -1.0),
        "floor_clashes": clashes.len(),
        "floor_clash_places": places(clashes, 1.0),
        "tube_clashes": tube_clashes.len(),
        "tube_clash_places": places(tube_clashes, 1.0),
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
