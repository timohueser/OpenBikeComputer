//! The floor of a tube between its openings: the crown's shell lies a slab under each way on top, within the grade of
//! the class, and the section gets lower where the grade does not allow that.

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

/// The floor of a tube: per vertex its metres, the road solver's height, the floor and the inside height; per way
/// over it the most metres that the shell of the crown rises over its underside, and the vertex there.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Floor {
    pub s: Vec<f64>,
    pub z: Vec<f64>,
    pub f: Vec<f64>,
    pub h: Vec<f64>,
    pub over: Vec<Option<(f64, usize)>>,
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
/// each way on top there (its index and underside), `caps` whether that way lowers the floor; `grade` is the grade of
/// the class, `height` and `least` the inside height and the least height of the group, `shell` the shell of the crown.
pub(crate) fn floor(
    (s, z): (&[f64], &[f64]),
    (under, caps): (&[Vec<(usize, f64)>], &[bool]),
    grade: f64,
    (height, least): (f64, f64),
    shell: f64,
) -> Floor {
    let n = s.len();
    // The cap: the floor under which the crown's shell clears the underside of each way on top of the vertex.
    let cap: Vec<f64> = (under.iter())
        .map(|u| u.iter().filter(|w| caps[w.0]).map(|w| w.1).fold(f64::INFINITY, f64::min) - shell - height)
        .collect();
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
    let h: Vec<f64> =
        (0..n).map(|i| if f[i] > cap[i] { (height - (f[i] - cap[i])).max(least).min(height) } else { height }).collect();
    let mut over: Vec<Option<(f64, usize)>> = vec![None; caps.len()];
    for (i, u) in under.iter().enumerate() {
        for &(w, under) in u {
            let m = f[i] + h[i] + shell - under;
            if over[w].is_none_or(|o| m > o.0) {
                over[w] = Some((m, i));
            }
        }
    }
    Floor { s: s.to_vec(), z: z.to_vec(), f, h, over }
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

/// Whether two boxes meet.
fn meet(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

/// The ways that can lie over the lines (`reach`: the half-width of each line with its shell): every way with heights
/// but a gallery, with its curve (`roads::polyline`), and per line the ways near it.
fn overs(lines: &[(Vec<[f64; 2]>, f64)], input: &Input) -> (Vec<Over>, Vec<Vec<usize>>) {
    let sections = &PlanRules::embedded().sections;
    let index = Boxes::new(&lines.iter().map(|(ps, reach)| grown(ps, *reach)).collect::<Vec<_>>());
    let (mut out, mut near) = (Vec::new(), vec![Vec::new(); lines.len()]);
    for (w, (kind, tags, parts)) in input.ways.iter().enumerate() {
        if *kind == "gallery" {
            continue;
        }
        let outer = input.widths.way(kind, tags, false).outer_m();
        for part in parts.iter().filter(|p| p.z.is_some() && p.points.len() >= 2) {
            let curve = roads::polyline(part);
            let lines = index.overlapping(grown(&curve.points, outer));
            if lines.is_empty() {
                continue;
            }
            for l in lines {
                near[l].push(out.len());
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
    (out, near)
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
    let (overs, near_of) = overs(&boxes, input);
    let mut out: Vec<Option<Floor>> = vec![None; tubes.len()];
    let (mut lowered, mut low, mut at_least, mut lows) = (0, 0, 0, Vec::new());
    let (mut tops, mut clashes, mut tube_clashes) = (0, Vec::new(), Vec::new());
    for (((&t, points), (ps, _)), near) in with.iter().zip(&lines).zip(&boxes).zip(&near_of) {
        let tube = &tubes[t];
        let reach_of = |o: &Over| o.outer + half[t] + b.shell_m;
        let ids: Vec<usize> = tube.ids.iter().map(|&id| id as usize).collect();
        let near: Vec<&Over> = near.iter().map(|&k| &overs[k]).filter(|o| !(o.tunnel && ids.contains(&o.way))).collect();
        // The vertices: the curve's, and per way over it the ends of the stretch within its reach and the foot of
        // each point of its curve there.
        let mut extra: Vec<f64> = Vec::new();
        for o in &near {
            let reach = reach_of(o);
            for j in 0..ps.len() - 1 {
                let chord = grown(&ps[j..j + 2], reach);
                let ds = points[j + 1].s - points[j].s;
                for w in o.points.windows(2).filter(|w| meet(bounds(*w), chord)) {
                    if let Some(run) = within(ps[j], ps[j + 1], (w[0], w[1]), reach) {
                        extra.extend(run.iter().filter(|&&u| u > 0.0 && u < 1.0).map(|&u| points[j].s + u * ds));
                    }
                }
                for &q in o.points.iter().filter(|&&q| meet([q[0], q[1], q[0], q[1]], chord)) {
                    let (m, u) = nearest(q, ps[j], ps[j + 1]);
                    if m <= reach && u > 0.0 && u < 1.0 {
                        extra.push(points[j].s + u * ds);
                    }
                }
            }
        }
        let mut s: Vec<f64> = points.iter().map(|q| q.s).collect();
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
        // Per vertex each way on top: within its reach, its height at the nearest point over the road solver's height
        // of the tube by more than the at-grade band of the two.
        let own_band = sections.way("tunnel", &input.tunnels[&tube.widest].0, false).snap_height_m;
        let mut under: Vec<Vec<(usize, f64)>> = vec![Vec::new(); s.len()];
        for (c, o) in near.iter().enumerate() {
            let reach = reach_of(o);
            let area = grown(&o.points, reach);
            for (i, &p) in xy.iter().enumerate().filter(|(_, p)| meet([p[0], p[1], p[0], p[1]], area)) {
                let (m, k, u) = (0..o.points.len() - 1)
                    .map(|k| {
                        let (m, u) = nearest(p, o.points[k], o.points[k + 1]);
                        (m, k, u)
                    })
                    .fold((f64::INFINITY, 0, 0.0), |a, x| if x.0 < a.0 { x } else { a });
                let height_there = o.z[k] + u * (o.z[k + 1] - o.z[k]);
                if m <= reach + MERGE_M && height_there - z[i] > o.band.min(own_band) {
                    under[i].push((c, height_there - o.depth));
                }
            }
        }
        let caps: Vec<bool> = near.iter().map(|o| !o.tunnel).collect();
        let least = input.rules.groups[tube.group].least_height_m;
        let rank = input.class_names.iter().position(|c| *c == tube.class);
        let grade = rank.map(|r| input.classes.rows[r].grade).filter(|g| g.is_finite()).unwrap_or(b.grade_default);
        let fl = floor((&s, &z), (&under, &caps), grade, (height[t], least), b.shell_m);
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
        for (c, o) in near.iter().enumerate() {
            let Some((m, i)) = fl.over[c] else { continue };
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
