//! The floor of a tube between its openings: the crown's shell lies a slab under each way on top, within the grade of
//! the class, and the section gets lower where the grade does not allow that.

use std::collections::BTreeMap;

use obc_ground_plan::faces::Rules as PlanRules;
use serde_json::{json, Value};

use super::rooms::{hold, Outline, Place, Rooms};
use super::{distance, segment_m, Input, Point, Tube};
use crate::geometry::{bounds, interp, Boxes};
use crate::{degrees, metres, roads};

/// A way that can lie over a room: its curve with heights, the depth of its underside under the curve, its outer
/// half-width and its at-grade band; or a room over others (`Upper`), whose heights are its floor.
struct Over {
    points: Vec<[f64; 2]>,
    z: Vec<f64>,
    depth: f64,
    outer: f64,
    band: f64,
    room: Option<Upper>,
}

/// A room over others: the room, the metres along it of each point of its curve, and the rooms under it, each with
/// whether only its height goes lower (the two lie side by side) or its floor too (they cross).
struct Upper {
    room: usize,
    s: Vec<f64>,
    under: Vec<(usize, bool)>,
}

/// A way over a vertex of a room: its number among the ways over the room, its height at its nearest point (of a room
/// over this one, the road solver's: its floor where that room has one, at its metres `upper`) and its depth, the
/// metres along the chain of that point's foot and the road solver's height of the chain there, and the at-grade band
/// of the way and the room.
struct Above {
    way: usize,
    height: f64,
    upper: Option<(usize, f64)>,
    depth: f64,
    along: f64,
    base: f64,
    band: f64,
}

impl Above {
    /// Its height, with the floor of a room over this one where it has one.
    fn height(&self, out: &[Option<Floor>]) -> f64 {
        self.upper.and_then(|(r, m)| out[r].as_ref().map(|f| f.at(m).0)).unwrap_or(self.height)
    }
}

/// A room with a part where the floor pass reads it: its vertices (metres along its curve), the road solver's
/// heights and the points there, per vertex the ways over it, and those ways among `overs` in their numbers.
struct Site {
    s: Vec<f64>,
    z: Vec<f64>,
    xy: Vec<[f64; 2]>,
    above: Vec<Vec<Above>>,
    ways: Vec<usize>,
}

/// The floor of a room: per vertex its metres, the road solver's height, the floor and the inside height; of a room of
/// lanes, per lane its floor and its inside height at each vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Floor {
    pub s: Vec<f64>,
    pub z: Vec<f64>,
    pub f: Vec<f64>,
    pub h: Vec<f64>,
    pub lanes: Vec<(Vec<f64>, Vec<f64>)>,
}

impl Floor {
    /// The floor and the inside height at metres s: exact at a vertex.
    pub fn at(&self, s: f64) -> (f64, f64) {
        (interp(s, &self.s, &self.f), interp(s, &self.s, &self.h))
    }

    /// The inside height of the chord at an end of the tube (0 its first vertex, 1 its last): of a room of lanes its
    /// height at that vertex, of a lone tube the lesser of the chord's two.
    pub fn end_height(&self, end: usize) -> f64 {
        let (n, lanes) = (self.h.len(), !self.lanes.is_empty());
        match (end, lanes) {
            (0, true) => self.h[0],
            (_, true) => self.h[n - 1],
            (0, false) => self.h[0].min(self.h[1]),
            _ => self.h[n - 1].min(self.h[n - 2]),
        }
    }
}

/// A vertex nearer than this to another is that vertex, and a point this far past a reach is within it.
const MERGE_M: f64 = 0.01;

/// Per vertex the undersides of the ways on top, and the least underside of a room on top that only lowers the
/// section.
type Tops<'a> = (&'a [Vec<f64>], &'a [f64]);

/// The floor of a line of metres `s` and road solver's heights `z` from opening to opening: `under` holds per vertex
/// the undersides of the ways on top there, `ceiling` the least underside of a room on top that only lowers the
/// section; `grade` is the grade of the class, `height` and `least` the inside height and the least height of the
/// group, `shell` the shell of the crown.
pub(crate) fn floor(
    (s, z): (&[f64], &[f64]),
    (under, ceiling): Tops,
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
        .map(|i| {
            let h = if f[i] > cap[i] { (height - (f[i] - cap[i])).max(least).min(height) } else { height };
            h.min((ceiling[i] - shell - f[i]).max(least))
        })
        .collect();
    Floor { s: s.to_vec(), z: z.to_vec(), f, h, lanes: Vec::new() }
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
/// but a gallery and a tunnel (a room over another is an `Upper`), with its curve (`roads::polyline`).
fn overs(lines: &[(Vec<[f64; 2]>, f64)], input: &Input) -> Vec<Over> {
    let sections = &PlanRules::embedded().sections;
    let index = Boxes::new(&lines.iter().map(|(ps, reach)| grown(ps, *reach)).collect::<Vec<_>>());
    let mut out = Vec::new();
    for (w, (kind, tags, parts)) in input.ways.iter().enumerate() {
        if *kind == "gallery" || *kind == "tunnel" {
            continue;
        }
        let outer = input.widths.way(kind, tags, false).outer_m();
        for part in parts.iter().filter(|p| p.z.is_some() && p.points.len() >= 2) {
            let curve = roads::polyline(part);
            if index.overlapping(grown(&curve.points, outer)).is_empty() {
                continue;
            }
            out.push(Over {
                points: curve.points,
                z: curve.z.unwrap(),
                depth: input.decks.get(&w).copied().unwrap_or(input.rules.bake.slab_m),
                outer,
                band: sections.way(kind, tags, false).snap_height_m,
                room: None,
            });
        }
    }
    out
}

/// The floors of the rooms with a part (`spans`: the metres of their two openings; `half` and `height` their inside
/// half-width and height), and their counts. The ways on top are read over a room's width. The floor rule runs once
/// per arm from opening to opening, at the vertices of each room that it runs in, under the ways on top there; a room's
/// floor is the lowest of its lanes'. A room over another in a run of over and under (`Rooms::over`) is a way on top of
/// it, its underside its floor less `slab_m`: the arms are solved from the highest down.
pub(crate) fn floors(
    rooms: &Rooms,
    (spans, boxed): (&[Option<[f64; 2]>], &[bool]),
    (half, height): (&[f64], &[f64]),
    input: &Input,
) -> (Vec<Option<Floor>>, Value) {
    let tubes = &rooms.tubes;
    let b = &input.rules.bake;
    let sections = &PlanRules::embedded().sections;
    let with: Vec<usize> = (0..tubes.len()).filter(|&t| spans[t].is_some()).collect();
    let lines: Vec<Vec<Point>> = with.iter().map(|&t| line(&tubes[t], spans[t].unwrap())).collect();
    let boxes: Vec<(Vec<[f64; 2]>, f64)> =
        (with.iter().zip(&lines)).map(|(&t, l)| (l.iter().map(|q| q.p).collect(), half[t] + b.shell_m)).collect();
    let mut overs = overs(&boxes, input);
    // The rooms over others, each once, as ways on top.
    let mut uppers: BTreeMap<usize, Vec<(usize, bool)>> = BTreeMap::new();
    for &(u, t, height_only) in &rooms.over {
        if spans[u].is_some() && spans[t].is_some() {
            uppers.entry(u).or_default().push((t, height_only));
        }
    }
    for (&u, under) in &uppers {
        let points = line(&tubes[u], spans[u].unwrap());
        overs.push(Over {
            points: points.iter().map(|q| q.p).collect(),
            z: points.iter().map(|q| q.z).collect(),
            depth: b.slab_m,
            outer: half[u] + b.shell_m,
            band: 0.0,
            room: Some(Upper { room: u, s: points.iter().map(|q| q.s).collect(), under: under.clone() }),
        });
    }
    // Each segment of a way over a line, its box grown by the way's outer half-width.
    let segments: Vec<(usize, usize)> =
        (overs.iter().enumerate()).flat_map(|(o, w)| (0..w.points.len() - 1).map(move |k| (o, k))).collect();
    let index = Boxes::new(
        &segments.iter().map(|&(o, k)| grown(&overs[o].points[k..k + 2], overs[o].outer)).collect::<Vec<_>>(),
    );
    // Per room with a part, its vertices and the ways over them.
    let site_of = |k: usize| -> Site {
        let t = with[k];
        let (points, (ps, shell_half)) = (&lines[k], &boxes[k]);
        let tube = &tubes[t];
        let reach_of = |o: &Over| o.outer + shell_half;
        // The segments of the ways near a box, and of the rooms over this one.
        let on = |o: usize| overs[o].room.as_ref().is_none_or(|u| u.under.iter().any(|x| x.0 == t));
        let near = |area: [f64; 4]| {
            let rooms = (overs.len() - uppers.len()..overs.len())
                .flat_map(|o| (0..overs[o].points.len() - 1).map(move |k| (o, k)));
            (index.overlapping(area).into_iter()).map(|i| segments[i]).chain(rooms).filter(move |&(o, _)| on(o))
        };
        // The vertices: the curve's, and the ends of each stretch of the tube within the reach of a way that can lower
        // the floor.
        let mut runs: BTreeMap<usize, Vec<[f64; 2]>> = BTreeMap::new();
        // The ends of the reaches, each with its way.
        let mut extra: Vec<(f64, usize)> = Vec::new();
        for j in 0..ps.len() - 1 {
            let ds = points[j + 1].s - points[j].s;
            for (o, k) in near(grown(&ps[j..j + 2], *shell_half)) {
                let w = &overs[o];
                let reach = reach_of(w);
                if let Some([a, e]) = within(ps[j], ps[j + 1], (w.points[k], w.points[k + 1]), reach) {
                    runs.entry(o).or_default().push([points[j].s + a * ds, points[j].s + e * ds]);
                }
            }
        }
        for (o, mut list) in runs {
            list.sort_by(|x, y| x[0].total_cmp(&y[0]));
            let mut run = list[0];
            for r in list.into_iter().skip(1).chain([[f64::INFINITY; 2]]) {
                if r[0] <= run[1] + MERGE_M {
                    run[1] = run[1].max(r[1]);
                } else {
                    extra.extend(run.map(|x| (x, o)));
                    run = r;
                }
            }
        }
        let (s0, s1) = (points[0].s, points[points.len() - 1].s);
        // The openings and the joins of the ways stay; a point of the curve, then a point of a reach, nearer than
        // `vertex_gap_m` to a vertex is that vertex.
        let joins = tube.curve.stretches.iter().map(|(_, ps)| ps[0].s).filter(|&x| x > s0 && x < s1);
        let fixed: Vec<f64> = [s0, s1].into_iter().chain(joins).collect();
        let curve: Vec<f64> = points.iter().map(|q| q.s).collect();
        extra.retain(|&(e, _)| e > s0 && e < s1);
        extra.sort_by(|x, y| x.0.total_cmp(&y.0));
        let ends: Vec<f64> = extra.iter().map(|e| e.0).collect();
        let mut s = spaced(&ends, spaced(&curve, fixed, b.vertex_gap_m), b.vertex_gap_m);
        // Behind a seam no vertex lies within `clear` of the hole, so each ring lies wholly behind the line.
        let holes = [s0, s1].map(|x| tube.curve.at(x).0.p);
        let clear = rooms.clear[t];
        let off = |x: f64, e: usize| clear[e].is_none_or(|c| distance(tube.curve.at(x).0.p, holes[e]) >= c);
        s.retain(|&x| x == s0 || x == s1 || (off(x, 0) && off(x, 1)));
        // A room of one lane at a seam has a vertex between its ends: behind the line the part of its other end stops
        // there (`boxes::stops`).
        if s.len() == 2 && rooms.lanes[t].len() < 2 && (0..2).any(|e| rooms.seam_at(t, e)) {
            s.insert(1, (s0 + s1) / 2.0);
        }
        // Each end of a reach that is no vertex is taken by its nearest vertex, which reads that way within its reach
        // and the metres to that end: by the triangle inequality the way lies there within that.
        let mut took: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        for &(e, o) in &extra {
            let at = s.partition_point(|&x| x < e);
            let near = [at.checked_sub(1), (at < s.len()).then_some(at)].into_iter().flatten();
            if let Some(i) = near.min_by(|&x, &y| (s[x] - e).abs().total_cmp(&(s[y] - e).abs())) {
                let m = took.entry((i, o)).or_insert(0.0);
                *m = m.max(distance(tube.curve.at(s[i]).0.p, tube.curve.at(e).0.p));
            }
        }
        // Per vertex the most metres to an end of a reach that it took.
        let more: Vec<f64> =
            (0..s.len()).map(|i| took.range((i, 0)..(i + 1, 0)).map(|(_, &m)| m).fold(0.0, f64::max)).collect();
        let (ls, lz): (Vec<f64>, Vec<f64>) = points.iter().map(|q| (q.s, q.z)).unzip();
        let z: Vec<f64> = s.iter().map(|&x| interp(x, &ls, &lz)).collect();
        let xy: Vec<[f64; 2]> = s.iter().map(|&x| tube.curve.at(x).0.p).collect();
        // The foot of a point on the whole chain, past the openings to its ends: its metres and the road solver's
        // height there. It lies within twice the reach of the vertex that the point is near.
        let chain = line(tube, [0.0, tube.curve.len]);
        let foot = |q: [f64; 2], (at, reach): (f64, f64)| {
            let first = chain.partition_point(|v| v.s < at - 2.0 * reach).saturating_sub(1);
            let last = chain.partition_point(|v| v.s <= at + 2.0 * reach).min(chain.len() - 1);
            let (_, j, u) = (first..last.max(first + 1))
                .map(|j| {
                    let (m, u) = nearest(q, chain[j].p, chain[j + 1].p);
                    (m, j, u)
                })
                .fold((f64::INFINITY, 0, 0.0), |a, x| if x.0 < a.0 { x } else { a });
            let (c, d) = (chain[j], chain[j + 1]);
            (c.s + u * (d.s - c.s), c.z + u * (d.z - c.z))
        };
        // Per vertex each way over it: within its reach, with the foot of its nearest point within the openings
        // grown by its outer half-width; its height and depth there, the foot and the at-grade band of the two.
        let own_band = sections.way("tunnel", &input.tunnels[&tube.widest].0, false).snap_height_m;
        let mut local: BTreeMap<usize, usize> = BTreeMap::new();
        let mut above: Vec<Vec<Above>> = (0..s.len()).map(|_| Vec::new()).collect();
        for (i, &p) in xy.iter().enumerate() {
            // Per way its nearest segment: metres, segment and share along it.
            let mut best: BTreeMap<usize, (f64, usize, f64)> = BTreeMap::new();
            for (o, k) in near(grown(&[p], *shell_half + more[i])) {
                let (m, u) = nearest(p, overs[o].points[k], overs[o].points[k + 1]);
                let e = best.entry(o).or_insert((f64::INFINITY, 0, 0.0));
                if m < e.0 {
                    *e = (m, k, u);
                }
            }
            for (o, (m, k, u)) in best {
                let w = &overs[o];
                let reach = reach_of(w) + took.get(&(i, o)).copied().unwrap_or(0.0);
                if m > reach + MERGE_M {
                    continue;
                }
                let (a, e) = (w.points[k], w.points[k + 1]);
                let (along, base) = foot([a[0] + u * (e[0] - a[0]), a[1] + u * (e[1] - a[1])], (s[i], reach));
                if along >= s0 - w.outer - MERGE_M && along <= s1 + w.outer + MERGE_M {
                    let c = local.len();
                    let way = *local.entry(o).or_insert(c);
                    let upper = w.room.as_ref().map(|r| (r.room, r.s[k] + u * (r.s[k + 1] - r.s[k])));
                    let height = w.z[k] + u * (w.z[k + 1] - w.z[k]);
                    let band = w.band.min(own_band);
                    above[i].push(Above { way, height, upper, depth: w.depth, along, base, band });
                }
            }
        }
        let mut ways: Vec<(usize, usize)> = local.iter().map(|(&o, &c)| (c, o)).collect();
        ways.sort_unstable();
        Site { s, z, xy, above, ways: ways.into_iter().map(|(_, o)| o).collect() }
    };
    let sites: Vec<Site> = (0..with.len()).map(site_of).collect();
    let site = |t: usize| &sites[with.iter().position(|&w| w == t).unwrap()];
    // A tube's grade of its class and its group's least height.
    let rule = |t: &Tube| {
        let rank = input.class_names.iter().position(|c| *c == t.class);
        let grade = rank.map(|r| input.classes.rows[r].grade).filter(|g| g.is_finite()).unwrap_or(b.grade_default);
        (grade, input.rules.groups[t.group].least_height_m)
    };
    // Per vertex of room t, the undersides of the ways on top, and the least underside of a room on top beside it,
    // which only lowers its section: a way lies on top where it lies over the road solver's height of the room by more
    // than the at-grade band.
    let tops = |t: usize, out: &[Option<Floor>]| -> (Vec<Vec<f64>>, Vec<f64>) {
        let site = site(t);
        let beside = |a: &Above| overs[site.ways[a.way]].room.as_ref().is_some_and(|u| u.under.contains(&(t, true)));
        let caps = (site.above.iter())
            .map(|v| {
                let on = v.iter().filter(|a| !beside(a) && a.height(out) - a.base > a.band);
                on.map(|a| a.height(out) - a.depth).collect()
            })
            .collect();
        let ceilings = (site.above.iter())
            .map(|v| v.iter().filter(|a| beside(a)).map(|a| a.height(out) - a.depth).fold(f64::INFINITY, f64::min))
            .collect();
        (caps, ceilings)
    };
    // The metres along its arm of each lane of room t at the room's metres s.
    let lane_metres = |t: usize, s: f64| -> Vec<f64> {
        if rooms.lanes[t].len() < 2 {
            vec![rooms.from[t] + s]
        } else {
            rooms.places(t, s).iter().map(|p| p.s).collect()
        }
    };
    let mut lanes_of: Vec<Vec<(usize, usize)>> = vec![Vec::new(); rooms.arms.len()];
    for &t in &with {
        for (k, l) in rooms.lanes[t].iter().enumerate() {
            lanes_of[l.arm].push((t, k));
        }
    }
    // From the highest arm down: an arm after each arm of each room over a room that it runs in, so the floors of those
    // rooms are known; arms over each other in turn in their order. A room's floor comes with its last arm's.
    let mut above: Vec<Vec<usize>> = vec![Vec::new(); rooms.arms.len()];
    for (&u, under) in &uppers {
        for x in under.iter().flat_map(|&(t, _)| rooms.lanes[t].iter().map(|l| l.arm)) {
            above[x].extend(rooms.lanes[u].iter().map(|l| l.arm).filter(|&y| y != x));
        }
    }
    let arms: Vec<usize> = (0..rooms.arms.len()).filter(|&x| !lanes_of[x].is_empty()).collect();
    let (mut order, mut placed) = (Vec::new(), vec![false; rooms.arms.len()]);
    while order.len() < arms.len() {
        let next = (arms.iter().copied())
            .find(|&x| !placed[x] && above[x].iter().all(|&y| placed[y]))
            .or_else(|| arms.iter().copied().find(|&x| !placed[x]))
            .unwrap();
        placed[next] = true;
        order.push(next);
    }
    // The floor of an arm: the rule at the vertices of each room that it runs in, under the ways on top there; a
    // vertex of two rooms (on a seam's line) once, under the ways of both.
    let arm_floor = |arm: usize, out: &[Option<Floor>]| -> Floor {
        let mut samples: Vec<(f64, Vec<f64>, f64)> = Vec::new();
        for &(t, k) in &lanes_of[arm] {
            let (caps, ceilings) = tops(t, out);
            for ((&s, cap), ceiling) in site(t).s.iter().zip(caps).zip(ceilings) {
                samples.push((lane_metres(t, s)[k], cap, ceiling));
            }
        }
        samples.sort_by(|x, y| x.0.total_cmp(&y.0));
        let mut merged: Vec<(f64, Vec<f64>, f64)> = Vec::new();
        for (m, cap, ceiling) in samples {
            match merged.last_mut() {
                Some(last) if m - last.0 < MERGE_M => {
                    last.1.extend(cap);
                    last.2 = last.2.min(ceiling);
                }
                _ => merged.push((m, cap, ceiling)),
            }
        }
        let a = &rooms.arms[arm];
        let s: Vec<f64> = merged.iter().map(|x| x.0).collect();
        let z: Vec<f64> = s.iter().map(|&m| a.curve.at(m).0.z).collect();
        let (caps, ceilings): (Vec<Vec<f64>>, Vec<f64>) = merged.into_iter().map(|x| (x.1, x.2)).unzip();
        let (grade, least) = rule(a);
        floor((&s, &z), (&caps, &ceilings), grade, (a.height, least), b.shell_m)
    };
    let mut out: Vec<Option<Floor>> = vec![None; tubes.len()];
    let mut arm_floors: Vec<Option<Floor>> = vec![None; rooms.arms.len()];
    let (mut lowered, mut low, mut at_least, mut lows) = (0, 0, 0, Vec::new());
    let (mut tops_count, mut clashes, mut tube_clashes) = (0, Vec::new(), Vec::new());
    for &x in &order {
        arm_floors[x] = Some(arm_floor(x, &out));
        let ready = |t: usize| out[t].is_none() && rooms.lanes[t].iter().all(|l| arm_floors[l.arm].is_some());
        let rooms_now: Vec<usize> = lanes_of[x].iter().map(|l| l.0).filter(|&t| ready(t)).collect();
        for t in rooms_now {
            let (site, tube) = (site(t), &tubes[t]);
            let (s0, s1) = (site.s[0], site.s[site.s.len() - 1]);
            // Each lane's floor, its inside height and the road solver's height at the room's vertices.
            let lanes: Vec<LaneFloor> = (rooms.lanes[t].iter().enumerate())
                .map(|(k, l)| {
                    let (a, f) = (&rooms.arms[l.arm], arm_floors[l.arm].as_ref().unwrap());
                    let at: Vec<f64> = site.s.iter().map(|&s| lane_metres(t, s)[k]).collect();
                    let (fl, h): (Vec<f64>, Vec<f64>) = at.iter().map(|&m| f.at(m)).unzip();
                    [fl, h, at.iter().map(|&m| a.curve.at(m).0.z).collect()]
                })
                .collect();
            let fl = if let [[f, h, _]] = &lanes[..] {
                Floor { s: site.s.clone(), z: site.z.clone(), f: f.clone(), h: h.clone(), lanes: Vec::new() }
            } else {
                let leasts: Vec<f64> = rooms.lanes[t].iter().map(|l| rule(&rooms.arms[l.arm]).1).collect();
                lanes_floor(rooms, (t, !boxed[t]), (&site.s, &lanes, &leasts), tops(t, &out), input)
            };
            // Over the floor as it is (past an opening, the chain), per way the most metres that the shell of the crown
            // rises over its underside where it lies over the floor by more than the band, and the vertex there.
            let ground =
                |a: &Above| if a.along >= s0 && a.along <= s1 { interp(a.along, &fl.s, &fl.f) } else { a.base };
            let mut over: Vec<Option<(f64, usize)>> = vec![None; site.ways.len()];
            for (i, v) in site.above.iter().enumerate() {
                for a in v.iter().filter(|a| a.height(&out) - ground(a) > a.band) {
                    let m = fl.f[i] + fl.h[i] + b.shell_m - (a.height(&out) - a.depth);
                    if over[a.way].is_none_or(|o| m > o.0) {
                        over[a.way] = Some((m, i));
                    }
                }
            }
            lowered += usize::from(fl.f.iter().zip(&fl.z).any(|(f, z)| *f < z - 0.005));
            // A low stretch of a lone tube: a run of chords (the lesser height of their two vertices) under the tube's
            // height, and its lowest vertex. A room of lanes is as high as its lanes need at each vertex.
            let chords: Vec<f64> =
                if fl.lanes.is_empty() { fl.h.windows(2).map(|w| w[0].min(w[1])).collect() } else { Vec::new() };
            let mut run: Option<usize> = None;
            let least = rule(tube).1;
            for k in 0..=chords.len() {
                match (run, chords.get(k).filter(|&&h| h < height[t] - 0.005)) {
                    (None, Some(_)) => run = Some(if fl.h[k] <= fl.h[k + 1] { k } else { k + 1 }),
                    (Some(r), Some(_)) if fl.h[k + 1] < fl.h[r] => run = Some(k + 1),
                    (Some(r), None) => {
                        low += 1;
                        at_least += usize::from(fl.h[r] <= least + 0.005);
                        lows.push((height[t] - fl.h[r], site.xy[r], &tube.class));
                        run = None;
                    }
                    _ => {}
                }
            }
            for (c, &o) in site.ways.iter().enumerate() {
                let Some((m, i)) = over[c] else { continue };
                tops_count += usize::from(overs[o].room.is_none());
                if m > 0.005 {
                    let list = if overs[o].room.is_some() { &mut tube_clashes } else { &mut clashes };
                    list.push((m, site.xy[i], &tube.class));
                }
            }
            out[t] = Some(fl);
        }
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
        "tops": tops_count,
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

/// A lane's floor, its inside height and the road solver's height at the vertices of its room.
type LaneFloor = [Vec<f64>; 3];

/// The floor of room r of lanes (`round` or of box section) at its vertices `s`, of the floors of its lanes there (per
/// lane its floor, its inside height and the road solver's height, and its group's least height) under the undersides
/// of the ways on top: its floor the lowest of its lanes'. At a vertex the room is as high as its lanes need there;
/// where its crown's shell would rise over a way on top, its lanes are lower by as much, each to its least height, and
/// the room is as high as the ways on top let it be, at least as high as its lowered lanes need.
fn lanes_floor(
    rooms: &Rooms,
    (r, round): (usize, bool),
    (s, lanes, leasts): (&[f64], &[LaneFloor], &[f64]),
    under: (Vec<Vec<f64>>, Vec<f64>),
    input: &Input,
) -> Floor {
    let design = &obc_trailview_parts::Rules::embedded().design;
    let shell = input.rules.bake.shell_m;
    let places: Vec<Vec<Place>> = s.iter().map(|&m| rooms.places(r, m)).collect();
    let n = s.len();
    let low = |k: usize| (0..n).map(|i| lanes.iter().map(|l| l[k][i]).fold(f64::INFINITY, f64::min)).collect();
    let (f, z): (Vec<f64>, Vec<f64>) = (low(0), low(2));
    let (segments, arch) = (design.arch_segments * lanes.len(), design.arch_segments);
    let hold_at = |i: usize, heights: &[f64]| {
        let outlines: Vec<Outline> = (places[i].iter().zip(lanes).zip(heights))
            .map(|((p, l), &h)| (p.off, p.half, l[0][i] - f[i], h, arch))
            .collect();
        hold(round, rooms.tubes[r].half, design.spring, &outlines, segments)
    };
    let mut heights: Vec<Vec<f64>> = lanes.iter().map(|l| l[1].clone()).collect();
    let h: Vec<f64> = (0..n)
        .map(|i| {
            let need = hold_at(i, &lanes.iter().map(|l| l[1][i]).collect::<Vec<f64>>());
            let top = under.0[i].iter().copied().fold(under.1[i], f64::min) - shell - f[i];
            let over = need - top;
            if over <= 0.0 {
                return need;
            }
            for (k, least) in leasts.iter().enumerate() {
                heights[k][i] = (lanes[k][1][i] - over).max(*least).min(lanes[k][1][i]);
            }
            hold_at(i, &heights.iter().map(|l| l[i]).collect::<Vec<f64>>()).max(top)
        })
        .collect();
    Floor { s: s.to_vec(), z, f, h, lanes: lanes.iter().map(|l| l[0].clone()).zip(heights).collect() }
}

/// The metres of `kept`, sorted, with each of `more` in its order that lies `gap` or farther from every kept one.
pub(crate) fn spaced(more: &[f64], mut kept: Vec<f64>, gap: f64) -> Vec<f64> {
    kept.sort_by(f64::total_cmp);
    kept.dedup();
    for &x in more {
        let at = kept.partition_point(|&k| k < x);
        let far = |k: usize| kept.get(k).is_none_or(|&y| (y - x).abs() >= gap);
        if far(at) && (at == 0 || far(at - 1)) {
            kept.insert(at, x);
        }
    }
    kept
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
