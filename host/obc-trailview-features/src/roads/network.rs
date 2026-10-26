//! The ways of a solve as arrays: its variables, vertices and segments.

use super::{Classes, Feature, APPROACH_M, APPROACH_PATH_M, VERTEX_M};
use crate::metres;

/// The ways as arrays. An OSM vertex is an occurrence of a point in a way; ways that share a point
/// share its variable. Between two occurrences a way gets vertices so that no segment is longer
/// than VERTEX_M, and one at the approach distance of a side road. `vertex_*` lists every vertex of
/// every way in order; `a`, `b` are the variables of each segment between two vertices of a way.
pub(crate) struct Network {
    /// Per way: its feature and part.
    pub way_feature: Vec<(usize, usize)>,
    pub rule: Vec<usize>,
    pub free: Vec<bool>,
    pub tunnel: Vec<bool>,
    pub ford: Vec<bool>,
    pub layer: Vec<i64>,
    pub layered: Vec<bool>,
    pub indoor: Vec<bool>,
    pub half: Vec<f64>,
    pub osm_xy: Vec<[f64; 2]>,
    pub osm_way: Vec<usize>,
    pub osm_first: Vec<usize>,
    pub osm_var: Vec<usize>,
    pub osm_count: usize,
    /// Per OSM variable: whether two occurrences or more share it.
    pub junction: Vec<bool>,
    pub sides: Sides,
    /// The through roads of two ways: (occurrence, step) of each.
    pub pairs: Vec<[(usize, i64); 2]>,
    pub osm_position: Vec<usize>,
    pub vertex_xy: Vec<[f64; 2]>,
    pub vertex_var: Vec<usize>,
    pub vertex_osm: Vec<bool>,
    pub vertex_junction: Vec<bool>,
    pub vertex_fixed: Vec<bool>,
    /// The OSM segment of each vertex, None for the last vertex of a way, and its place along it.
    pub vertex_segment: Vec<Option<usize>>,
    pub vertex_t: Vec<f64>,
    pub count: usize,
    pub way_first: Vec<usize>,
    pub way_last: Vec<usize>,
    pub segment_start: Vec<usize>,
    pub a: Vec<usize>,
    pub b: Vec<usize>,
    pub segment_way: Vec<usize>,
    pub length: Vec<f64>,
}

/// The side directions of the junctions: per side its occurrence, step and approach metres, and the
/// through directions of its junction as (occurrence, step), the second one None where there is one.
#[derive(Default)]
pub(crate) struct Sides {
    pub occurrence: Vec<usize>,
    pub step: Vec<i64>,
    pub distance: Vec<f64>,
    pub through: Vec<[Option<(usize, i64)>; 2]>,
}

/// The points of a part without a vertex that repeats the one before it: it has no length to solve along.
pub(crate) fn distinct(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::with_capacity(points.len());
    for &point in points {
        if out.last() != Some(&point) {
            out.push(point);
        }
    }
    out
}

/// Per point, its rank among the distinct points sorted by x, then y; and their count.
pub(crate) fn unique(points: &[[f64; 2]]) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..points.len()).collect();
    order.sort_by(|&i, &k| points[i].partial_cmp(&points[k]).unwrap());
    let mut rank = vec![0; points.len()];
    let mut count = 0;
    for (n, &i) in order.iter().enumerate() {
        if n > 0 && points[i] != points[order[n - 1]] {
            count += 1;
        }
        rank[i] = count;
    }
    (rank, if points.is_empty() { 0 } else { count + 1 })
}

pub(crate) fn hypot(d: [f64; 2]) -> f64 {
    d[0].hypot(d[1])
}

pub(crate) fn sub(p: [f64; 2], q: [f64; 2]) -> [f64; 2] {
    [p[0] - q[0], p[1] - q[1]]
}

pub(crate) fn dot(p: [f64; 2], q: [f64; 2]) -> f64 {
    p[0] * q[0] + p[1] * q[1]
}

impl Network {
    pub fn new(features: &[&Feature], classes: &Classes) -> Option<Network> {
        let mut way_feature = Vec::new();
        let mut xys = Vec::new();
        for (f, feature) in features.iter().enumerate() {
            for (p, points) in feature.parts.iter().enumerate() {
                let xy = distinct(points);
                if xy.len() >= 2 {
                    way_feature.push((f, p));
                    xys.push(xy);
                }
            }
        }
        if xys.is_empty() {
            return None;
        }
        let of =
            |get: &dyn Fn(&Feature) -> bool| way_feature.iter().map(|&(f, _)| get(features[f])).collect::<Vec<_>>();
        let (free, tunnel, ford) = (of(&|f| f.free), of(&|f| f.tunnel), of(&|f| f.ford));
        let (layered, indoor) = (of(&|f| f.layered), of(&|f| f.indoor));
        let rule: Vec<usize> = way_feature.iter().map(|&(f, _)| features[f].rule).collect();
        let layer: Vec<i64> = way_feature.iter().map(|&(f, _)| features[f].layer).collect();
        let half: Vec<f64> = way_feature.iter().map(|&(f, _)| features[f].half).collect();
        let counts: Vec<usize> = xys.iter().map(Vec::len).collect();
        let osm_xy: Vec<[f64; 2]> = xys.concat();
        let osm_way: Vec<usize> = counts.iter().enumerate().flat_map(|(w, &c)| std::iter::repeat_n(w, c)).collect();
        let mut osm_first = Vec::with_capacity(counts.len());
        let mut total = 0;
        for &c in &counts {
            osm_first.push(total);
            total += c;
        }
        let (osm_var, osm_count) = unique(&osm_xy);
        let mut last = vec![false; osm_xy.len()];
        for (w, &c) in counts.iter().enumerate() {
            last[osm_first[w] + c - 1] = true;
        }
        // OSM segments, from occurrence sa to sa + 1.
        let sa: Vec<usize> = (0..osm_xy.len()).filter(|&i| !last[i]).collect();
        let d: Vec<[f64; 2]> = sa.iter().map(|&s| sub(osm_xy[s + 1], osm_xy[s])).collect();
        let lengths: Vec<f64> =
            sa.iter().zip(&d).map(|(&s, d)| hypot(*d) * metres(osm_xy[s][1] + d[1] / 2.0)).collect();
        let mut segment_of = vec![usize::MAX; osm_xy.len()];
        for (n, &s) in sa.iter().enumerate() {
            segment_of[s] = n;
        }
        let mut occurrences = vec![0usize; osm_count];
        for &v in &osm_var {
            occurrences[v] += 1;
        }
        let junction: Vec<bool> = occurrences.iter().map(|&c| c >= 2).collect();
        let mut net = Network {
            way_feature,
            rule,
            free,
            tunnel,
            ford,
            layer,
            layered,
            indoor,
            half,
            osm_xy,
            osm_way,
            osm_first,
            osm_var,
            osm_count,
            junction,
            sides: Sides::default(),
            pairs: Vec::new(),
            osm_position: Vec::new(),
            vertex_xy: Vec::new(),
            vertex_var: Vec::new(),
            vertex_osm: Vec::new(),
            vertex_junction: Vec::new(),
            vertex_fixed: Vec::new(),
            vertex_segment: Vec::new(),
            vertex_t: Vec::new(),
            count: 0,
            way_first: Vec::new(),
            way_last: Vec::new(),
            segment_start: Vec::new(),
            a: Vec::new(),
            b: Vec::new(),
            segment_way: Vec::new(),
            length: Vec::new(),
        };
        net.junction_sides(&last, classes);
        // The cuts of each OSM segment: evenly spaced, and at the approach distance of a side road.
        // An approach cut within a metre of the one before it on its segment goes, and so does a
        // regular cut within a metre of an approach cut.
        let mut approach: Vec<(usize, f64)> = Vec::new();
        for i in 0..net.sides.occurrence.len() {
            let (occurrence, step, distance) = (net.sides.occurrence[i], net.sides.step[i], net.sides.distance[i]);
            let s = segment_of[if step == 1 { occurrence } else { occurrence - 1 }];
            if lengths[s] > distance + 1.0 {
                approach.push((s, if step == 1 { distance / lengths[s] } else { 1.0 - distance / lengths[s] }));
            }
        }
        approach.sort_by(|p, q| p.partial_cmp(q).unwrap());
        let mut kept: Vec<(usize, f64)> = Vec::with_capacity(approach.len());
        for (n, &(s, t)) in approach.iter().enumerate() {
            if n == 0 || !(approach[n - 1].0 == s && (t - approach[n - 1].1) * lengths[s] < 1.0) {
                kept.push((s, t));
            }
        }
        // Per OSM segment, its cuts in order: (t, whether at an approach distance).
        let mut cuts: Vec<Vec<(f64, bool)>> = vec![Vec::new(); sa.len()];
        let mut at = 0;
        for (s, cut) in cuts.iter_mut().enumerate() {
            let begin = at;
            while at < kept.len() && kept[at].0 == s {
                at += 1;
            }
            let approaches = &kept[begin..at];
            let regular = (lengths[s] / VERTEX_M).ceil().max(1.0) as usize;
            for k in 1..regular {
                let t = k as f64 / regular as f64;
                if !approaches.iter().any(|&(_, other)| (t - other).abs() * lengths[s] <= 1.0) {
                    cut.push((t, false));
                }
            }
            cut.extend(approaches.iter().map(|&(_, t)| (t, true)));
            cut.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap());
        }
        let cut_count: usize = cuts.iter().map(Vec::len).sum();
        let total = net.osm_xy.len() + cut_count;
        net.osm_position = Vec::with_capacity(net.osm_xy.len());
        net.vertex_xy = Vec::with_capacity(total);
        net.vertex_var = Vec::with_capacity(total);
        net.vertex_osm = Vec::with_capacity(total);
        net.vertex_fixed = Vec::with_capacity(total);
        net.vertex_segment = Vec::with_capacity(total);
        net.vertex_t = Vec::with_capacity(total);
        let mut next_var = net.osm_count;
        for (o, &s) in segment_of.iter().enumerate() {
            net.osm_position.push(net.vertex_xy.len());
            net.vertex_xy.push(net.osm_xy[o]);
            net.vertex_var.push(net.osm_var[o]);
            net.vertex_osm.push(true);
            net.vertex_fixed.push(true);
            net.vertex_segment.push((s != usize::MAX).then_some(s));
            net.vertex_t.push(0.0);
            if s != usize::MAX {
                for &(t, at_approach) in &cuts[s] {
                    let (p, d) = (net.osm_xy[o], d[s]);
                    net.vertex_xy.push([p[0] + t * d[0], p[1] + t * d[1]]);
                    net.vertex_var.push(next_var);
                    next_var += 1;
                    net.vertex_osm.push(false);
                    net.vertex_fixed.push(at_approach);
                    net.vertex_segment.push(Some(s));
                    net.vertex_t.push(t);
                }
            }
        }
        net.vertex_junction = (0..total).map(|v| net.vertex_osm[v] && net.junction[net.vertex_var[v]]).collect();
        net.count = next_var;
        net.way_first = net.osm_first.iter().map(|&o| net.osm_position[o]).collect();
        net.way_last = (0..counts.len()).map(|w| net.osm_position[net.osm_first[w] + counts[w] - 1]).collect();
        for w in 0..counts.len() {
            for v in net.way_first[w]..net.way_last[w] {
                net.segment_start.push(v);
                net.a.push(net.vertex_var[v]);
                net.b.push(net.vertex_var[v + 1]);
                net.segment_way.push(w);
                let (pa, pb) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
                net.length.push(hypot(sub(pb, pa)) * metres((pa[1] + pb[1]) / 2.0));
            }
        }
        Some(net)
    }

    /// The side directions of the junctions and the through road of each, and the through roads of
    /// two ways: the grade runs smoothly on across the junction, where OSM splits a way. The through
    /// road is the best class at the junction, and of two ways of that class, the pair that runs most
    /// nearly straight on.
    fn junction_sides(&mut self, last: &[bool], classes: &Classes) {
        // (occurrence, step, direction): first every step forward, then every step back.
        let mut rows: Vec<(usize, i64, [f64; 2])> = Vec::new();
        let at: Vec<usize> = (0..self.osm_xy.len()).filter(|&o| self.junction[self.osm_var[o]]).collect();
        let first: std::collections::HashSet<usize> = self.osm_first.iter().copied().collect();
        for step in [1i64, -1] {
            for &o in &at {
                if (step == 1 && last[o]) || (step == -1 && first.contains(&o)) {
                    continue;
                }
                let d = sub(self.osm_xy[(o as i64 + step) as usize], self.osm_xy[o]);
                let norm = hypot(d).max(1e-300);
                rows.push((o, step, [d[0] / norm, d[1] / norm]));
            }
        }
        let rank: Vec<usize> = rows.iter().map(|r| self.rule[self.osm_way[r.0]]).collect();
        let mut order: Vec<usize> = (0..rows.len()).collect();
        order.sort_by_key(|&i| (self.osm_var[rows[i].0], rank[i]));
        let mut s = 0;
        while s < order.len() {
            let var = self.osm_var[rows[order[s]].0];
            let mut e = s;
            while e < order.len() && self.osm_var[rows[order[e]].0] == var {
                e += 1;
            }
            let group = &order[s..e];
            let best = rank[group[0]];
            let top: Vec<usize> = group.iter().copied().filter(|&i| rank[i] == best).collect();
            let through: Vec<usize> = if top.len() >= 2 {
                let mut pick = (f64::INFINITY, usize::MAX, usize::MAX);
                for (n, &i) in top.iter().enumerate() {
                    for &k in &top[n + 1..] {
                        let candidate = (dot(rows[i].2, rows[k].2), i, k);
                        if candidate.partial_cmp(&pick) == Some(std::cmp::Ordering::Less) {
                            pick = candidate;
                        }
                    }
                }
                vec![pick.1, pick.2]
            } else {
                vec![top[0]]
            };
            let distance = if best >= classes.path { APPROACH_PATH_M } else { APPROACH_M };
            let t1 = (rows[through[0]].0, rows[through[0]].1);
            let t2 = through.get(1).map(|&i| (rows[i].0, rows[i].1));
            if let Some(t2) = t2 {
                if t1.0 != t2.0 {
                    self.pairs.push([t1, t2]);
                }
            }
            for &i in group {
                if !through.contains(&i) {
                    self.sides.occurrence.push(rows[i].0);
                    self.sides.step.push(rows[i].1);
                    self.sides.distance.push(distance);
                    self.sides.through.push([Some(t1), t2]);
                }
            }
            s = e;
        }
    }
}
