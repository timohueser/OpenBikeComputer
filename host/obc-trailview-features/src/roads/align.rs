//! The alignment of the ways of a solve: the plan as a smooth curve, and per vertex what a reader
//! needs to draw it.
//!
//! Ways that run on from each other chain into a stroke. The samples of a stroke, about every
//! SAMPLE_M, fit its line within the `plan_tol_m` of their class by a third-difference penalty. A
//! junction, a vertex of OSM on a bridge or in a tunnel, the end of an approach and the ends of a
//! stroke stay where they are. The tangent, the banking and the widening come from the fit, the grade
//! from the heights of the solve, which are smooth on its vertices. Between two kept vertices the
//! curve is the cubic Hermite curve of `specs/trailview-features.md`, so a way keeps few of them.

use std::collections::{HashMap, HashSet};

use super::area::area_of;
use super::curve::{arc, bessel, hermite, lengths, penalised, smooth_along, tangents, LEAST_STEP_M};
use super::network::{dot, hypot, sub, Network};
use super::{Classes, Way, SAMPLE_M};
use crate::tiles::{rounded_grade, rounded_tangent};
use crate::{metres, Part, Plan};

/// A chord of a way leaves its curve by at most this: a tile keeps a segment by its chord, against a
/// buffer of 40 m.
const KEEP_CHORD_M: f64 = 2.0;
/// The smoothing length of the plan is PLAN_LENGTH times its tolerance.
const PLAN_LENGTH: f64 = 25.0;
/// A round of the fit holds a sample ten times harder while it leaves its line by more than the
/// tolerance of its class.
const ROUNDS: usize = 3;
const HELD: f64 = 1e6;
/// Of the side friction, the share that the banking takes: e = V² / (127 R) · BANK_SHARE.
const BANK_SHARE: f64 = 0.5;
/// A bend tighter than WIDEN_R_M widens, by all of the class's `widen_m` from WIDEN_FULL_R_M on.
const WIDEN_R_M: f64 = 25.0;
const WIDEN_FULL_R_M: f64 = 10.0;
/// Banking and widening fade in from CALM_M past a junction, a bridge or a tunnel, over CALM_RAMP_M.
const CALM_M: f64 = 14.0;
const CALM_RAMP_M: f64 = 18.0;
/// The half-lengths of the triangle filters of the banking, the widening and the curvature.
const RUNOFF_M: f64 = 22.0;
const WIDEN_SMOOTH_M: f64 = 12.0;
const CURVE_SMOOTH_M: f64 = 6.0;
/// Two ends join into a stroke within 60° of straight on.
const JOIN_COS: f64 = -0.5;
/// The vertex after a junction ends the approach of a side road when it lies this close past it.
const APPROACH_SLACK_M: f64 = 0.5;
/// The longest chord of a `polyline`.
const CHORD_M: f64 = 10.0;

/// Two ways of a stroke where they meet, each as (way, at its last vertex).
pub(crate) type Join = [(usize, bool); 2];
/// An end of a way of a whole region: ((feature, part), at its last vertex).
type End = ((usize, usize), bool);

/// Per way, what runs on from its first and its last vertex: (way, at its last vertex). Of the ends of
/// ways at a point, the two of the best class within 60° of straight on join, also across a change of
/// class, where the through roads of the solve end. The solve asks which road a junction serves; this
/// asks where one line runs on as one curve.
fn joins(net: &Network) -> Vec<[Option<(usize, bool)>; 2]> {
    let ways = net.way_first.len();
    let last_occurrence = |w: usize| net.osm_first.get(w + 1).map_or(net.osm_xy.len(), |&o| o) - 1;
    let mut ends: HashMap<usize, Vec<(usize, bool, [f64; 2])>> = HashMap::new();
    for w in 0..ways {
        for at_last in [false, true] {
            let (o, inner) = if at_last {
                (last_occurrence(w), last_occurrence(w) - 1)
            } else {
                (net.osm_first[w], net.osm_first[w] + 1)
            };
            if net.junction[net.osm_var[o]] {
                let d = sub(net.osm_xy[inner], net.osm_xy[o]);
                let norm = hypot(d).max(1e-300);
                ends.entry(net.osm_var[o]).or_default().push((w, at_last, [d[0] / norm, d[1] / norm]));
            }
        }
    }
    let mut out = vec![[None, None]; ways];
    for meeting in ends.values() {
        let mut best: Option<(f64, usize, usize)> = None;
        for (i, a) in meeting.iter().enumerate() {
            for (k, b) in meeting.iter().enumerate().skip(i + 1) {
                let straight = dot(a.2, b.2);
                if a.0 == b.0 || straight > JOIN_COS {
                    continue;
                }
                let rank = net.rule[a.0].max(net.rule[b.0]) as f64 * 4.0 + straight;
                if best.is_none_or(|(score, _, _)| rank < score) {
                    best = Some((rank, i, k));
                }
            }
        }
        if let Some((_, i, k)) = best {
            let (a, b) = (meeting[i], meeting[k]);
            out[a.0][usize::from(a.1)] = Some((b.0, b.1));
            out[b.0][usize::from(b.1)] = Some((a.0, a.1));
        }
    }
    out
}

/// The strokes of the ways, each as its ways in order: (way, whether it runs forward).
fn strokes(links: &[[Option<(usize, bool)>; 2]]) -> Vec<Vec<(usize, bool)>> {
    let mut used = vec![false; links.len()];
    let mut out = Vec::new();
    for first in 0..links.len() {
        if used[first] {
            continue;
        }
        let (mut head, mut seen) = ((first, true), HashSet::from([first]));
        while let Some((way, at_last)) = links[head.0][usize::from(!head.1)] {
            if used[way] || !seen.insert(way) {
                break;
            }
            head = (way, at_last);
        }
        let (mut pieces, mut piece) = (Vec::new(), Some(head));
        while let Some((way, forward)) = piece.filter(|p| !used[p.0]) {
            used[way] = true;
            pieces.push((way, forward));
            piece = links[way][usize::from(forward)].map(|(next, at_last)| (next, !at_last));
        }
        out.push(pieces);
    }
    out
}

/// Per vertex of the network: whether it stays where it is, and whether it holds off banking and widening.
struct Marks {
    pin: Vec<bool>,
    calm: Vec<bool>,
}

fn marks(net: &Network, links: &[[Option<(usize, bool)>; 2]], classes: &Classes) -> Marks {
    let total = net.vertex_xy.len();
    let (mut pin, mut calm) = (vec![false; total], vec![false; total]);
    let mut at: HashMap<usize, Vec<usize>> = HashMap::new();
    for (o, &var) in net.osm_var.iter().enumerate() {
        if net.junction[var] {
            at.entry(var).or_default().push(o);
        }
    }
    let end = |o: usize| {
        let (w, v) = (net.osm_way[o], net.osm_position[o]);
        (v == net.way_first[w] || v == net.way_last[w]).then_some((w, v == net.way_last[w]))
    };
    let onward = |o: usize, other: usize| end(o).is_some_and(|(w, last)| links[w][usize::from(last)] == end(other));
    let road = |rule: usize| rule < classes.path && !classes.rows[rule].rail;
    for occurrences in at.values() {
        for &o in occurrences {
            let (v, rule) = (net.osm_position[o], net.rule[net.osm_way[o]]);
            for &other in occurrences.iter().filter(|&&other| other != o && !onward(o, other)) {
                pin[v] = true;
                let way = net.osm_way[other];
                calm[v] |= net.free[way] || road(net.rule[way]) && net.rule[way] <= rule;
            }
        }
    }
    for (o, &w) in net.osm_way.iter().enumerate() {
        if net.free[w] {
            pin[net.osm_position[o]] = true;
        }
    }
    for w in 0..net.way_first.len() {
        if net.free[w] {
            calm[net.way_first[w]..=net.way_last[w]].fill(true);
        }
    }
    // The approach of a side road lies on the surface of the through road: its end stays too.
    for (i, &o) in net.sides.occurrence.iter().enumerate() {
        let j = net.osm_position[o];
        let next = (j as i64 + net.sides.step[i]) as usize;
        let span = hypot(sub(net.vertex_xy[next], net.vertex_xy[j])) * metres(net.vertex_xy[j][1]);
        if span <= net.sides.distance[i] + APPROACH_SLACK_M {
            pin[next] = true;
            calm[next] |= calm[j];
        }
    }
    Marks { pin, calm }
}

/// The samples of a stroke.
#[derive(Default)]
struct Samples {
    x: Vec<f64>,
    y: Vec<f64>,
    /// Heights, linear between the vertices of the solve; NaN where unknown.
    z: Vec<f64>,
    pin: Vec<bool>,
    calm: Vec<bool>,
    /// A vertex that a way shares with a river or a stream.
    water: Vec<bool>,
    rule: Vec<usize>,
    /// The samples of the vertices of the solve.
    vertex: Vec<usize>,
}

impl Samples {
    fn push(&mut self, [x, y]: [f64; 2], z: f64, pin: bool, calm: bool, water: bool, rule: usize) {
        self.x.push(x);
        self.y.push(y);
        self.z.push(z);
        self.pin.push(pin);
        self.calm.push(calm);
        self.water.push(water);
        self.rule.push(rule);
    }
}

/// The fade from 0 to 1 away from the samples of `still`.
fn fade(s: &[f64], still: impl Fn(usize) -> bool) -> Vec<f64> {
    let n = s.len();
    let mut away = vec![f64::INFINITY; n];
    let mut last = f64::NEG_INFINITY;
    for i in 0..n {
        if still(i) {
            last = s[i];
        }
        away[i] = s[i] - last;
    }
    let mut next = f64::INFINITY;
    for i in (0..n).rev() {
        if still(i) {
            next = s[i];
        }
        away[i] = away[i].min(next - s[i]);
    }
    away.iter().map(|d| ((d - CALM_M) / CALM_RAMP_M).clamp(0.0, 1.0)).collect()
}

/// The curve of a stroke: its points, tangents, banking and widening per sample.
struct Curve {
    x: Vec<f64>,
    y: Vec<f64>,
    s: Vec<f64>,
    tangent: Vec<[f64; 2]>,
    /// Filled from the heights of the solve, after the fit.
    grade: Vec<f64>,
    bank: Vec<f64>,
    widen: Vec<f64>,
}

/// The fit of the samples, with the design of `classes`. A widening moves the curve toward the inside
/// of a bend by half of it, so its outer edge keeps its line, but not at a pin, where both sides widen.
fn fitted(samples: &Samples, classes: &Classes) -> Curve {
    let n = samples.x.len();
    let (x0, y0) = (samples.x[0], samples.y[0]);
    let target =
        [samples.x.iter().map(|x| x - x0).collect::<Vec<_>>(), samples.y.iter().map(|y| y - y0).collect::<Vec<_>>()];
    let s = lengths(&samples.x, &samples.y);
    let tolerance: Vec<f64> = samples.rule.iter().map(|&r| classes.rows[r].plan_tol_m).collect();
    let lambda: Vec<f64> = tolerance.iter().map(|t| (PLAN_LENGTH * t / std::f64::consts::TAU).powi(6)).collect();
    let share: Vec<f64> = (0..n)
        .map(|i| {
            let before = if i > 0 { s[i] - s[i - 1] } else { 0.0 };
            let after = if i + 1 < n { s[i + 1] - s[i] } else { 0.0 };
            ((before + after) / 2.0).max(LEAST_STEP_M)
        })
        .collect();
    let pin = |i: usize| samples.pin[i] || i == 0 || i + 1 == n;
    let mut held = vec![1.0; n];
    let mut fit = [Vec::new(), Vec::new()];
    for round in 0..=ROUNDS {
        let w: Vec<f64> = (0..n).map(|i| if pin(i) { HELD } else { share[i] * held[i] }).collect();
        fit = penalised([&target[0], &target[1]], &w, &lambda, &s);
        let mut again = false;
        for i in (0..n).filter(|&i| round < ROUNDS && !pin(i)) {
            let off = [fit[0][i] - target[0][i], fit[1][i] - target[1][i]];
            if hypot(off) * metres(samples.y[i]) > tolerance[i] {
                held[i] *= 10.0;
                again = true;
            }
        }
        if !again {
            break;
        }
    }
    let [mut x, mut y] = fit;
    for i in 0..n {
        let (px, py) = if pin(i) { (samples.x[i], samples.y[i]) } else { (x[i] + x0, y[i] + y0) };
        (x[i], y[i]) = (px, py);
    }
    let mut s = lengths(&x, &y);
    let mut tangent = tangents(&x, &y, &s);
    // The heading, unwound, and its rate: the curvature, positive to the left. y runs south, so a left
    // turn turns the heading down.
    let mut heading = vec![0.0; n];
    for i in 0..n {
        let a = tangent[i][1].atan2(tangent[i][0]);
        heading[i] = if i == 0 {
            a
        } else {
            let tau = std::f64::consts::TAU;
            heading[i - 1] + (a - heading[i - 1] + 1.5 * tau).rem_euclid(tau) - tau / 2.0
        };
    }
    let curvature: Vec<f64> = bessel(&heading, &s).iter().map(|k| -k).collect();
    let curvature = smooth_along(&curvature, &s, CURVE_SMOOTH_M);
    let calm = fade(&s, |i| samples.calm[i]);
    let keeps = fade(&s, |i| samples.calm[i] || pin(i));
    let design: Vec<_> = samples.rule.iter().map(|&r| &classes.rows[r]).collect();
    let bank: Vec<f64> = (0..n)
        .map(|i| {
            let k = curvature[i];
            -k.signum() * design[i].bank.min(design[i].design_kmh.powi(2) * k.abs() / 127.0 * BANK_SHARE)
        })
        .collect();
    let bank = smooth_along(&bank, &s, RUNOFF_M);
    let bank: Vec<f64> = (0..n).map(|i| bank[i] * calm[i]).collect();
    let widen: Vec<f64> = (0..n)
        .map(|i| {
            let (k, r) = (curvature[i], 1.0 / curvature[i].abs().max(1e-12));
            k.signum() * design[i].widen_m * ((WIDEN_R_M - r) / (WIDEN_R_M - WIDEN_FULL_R_M)).clamp(0.0, 1.0)
        })
        .collect();
    let signed = smooth_along(&widen, &s, WIDEN_SMOOTH_M);
    let signed: Vec<f64> = (0..n).map(|i| signed[i] * calm[i]).collect();
    let shift: Vec<f64> = (0..n).map(|i| signed[i] / 2.0 * keeps[i]).collect();
    if shift.iter().any(|&w| w != 0.0) {
        for i in 0..n {
            // Left of the tangent is (ty, −tx).
            let unit = shift[i] / metres(y[i]);
            x[i] += tangent[i][1] * unit;
            y[i] -= tangent[i][0] * unit;
        }
        s = lengths(&x, &y);
        tangent = tangents(&x, &y, &s);
    }
    let widen = (0..n).map(|i| signed[i].abs() - shift[i].abs()).collect();
    Curve { x, y, s, tangent, grade: vec![0.0; n], bank, widen }
}

/// The positions in `list`, samples of one way, of the vertices that it keeps: each of `stops`, and
/// from each kept vertex the farthest one whose curve holds the samples between them within `keep_m`
/// in the plan and in height, and whose chord holds them within KEEP_CHORD_M and between its ends. The
/// test reads the tangents and the grades as a tile rounds them: over a long chord that moves the
/// curve by metres. `sign` turns the tangents and the grades of a way against its stroke.
fn kept(list: &[usize], stops: &[usize], curve: &Curve, z: &[f64], keep_m: f64, sign: f64) -> Vec<usize> {
    let holds = |p: usize, q: usize| {
        let (a, b) = (list[p], list[q]);
        let scale = metres(curve.y[a]);
        let chord = [(curve.x[b] - curve.x[a]) * scale, (curve.y[b] - curve.y[a]) * scale];
        let length = hypot(chord);
        let tangent = |i: usize| {
            let heading = rounded_tangent((sign * curve.tangent[i][1]).atan2(sign * curve.tangent[i][0]));
            [heading.cos(), heading.sin()]
        };
        let (ta, tb) = (tangent(a), tangent(b));
        let (ga, gb) = (rounded_grade(sign * curve.grade[a]), rounded_grade(sign * curve.grade[b]));
        (p + 1..q).all(|k| {
            let i = list[k];
            let off = [(curve.x[i] - curve.x[a]) * scale, (curve.y[i] - curve.y[a]) * scale];
            let u = (curve.s[i] - curve.s[a]) / (curve.s[b] - curve.s[a]);
            let [h00, h10, h01, h11] = hermite(u);
            let on = [
                h10 * ta[0] * length + h01 * chord[0] + h11 * tb[0] * length,
                h10 * ta[1] * length + h01 * chord[1] + h11 * tb[1] * length,
            ];
            let off_curve = (on[0] - off[0]) * curve.tangent[i][1] - (on[1] - off[1]) * curve.tangent[i][0];
            let height = h00 * z[a] + h10 * ga * length + h01 * z[b] + h11 * gb * length;
            let off_chord = chord[0] * off[1] - chord[1] * off[0];
            let on_chord = chord[0] * off[0] + chord[1] * off[1];
            let off_height = (z[i] - height).abs();
            off_curve.abs() <= keep_m
                && off_chord.abs() <= KEEP_CHORD_M * length
                && (-keep_m * length..=(length + keep_m) * length).contains(&on_chord)
                && (off_height.is_nan() || off_height <= keep_m)
        })
    };
    let mut keep = vec![stops[0]];
    for stop in stops.windows(2) {
        let (mut p, end) = (stop[0], stop[1]);
        while p < end {
            let (mut reach, mut step) = (p + 1, 1);
            while reach + step <= end && holds(p, reach + step) {
                reach += step;
                step *= 2;
            }
            while step > 1 {
                step /= 2;
                if reach + step <= end && holds(p, reach + step) {
                    reach += step;
                }
            }
            keep.push(reach);
            p = reach;
        }
    }
    keep
}

/// Per way, its kept vertices and alignment, and the joins of the strokes at the vertices of `mine`.
/// `heights` and `known` are per variable of the solve. `stay` holds the variables whose height a row
/// fixes, and `water` those that a way shares with a river or a stream.
pub(crate) fn align(
    net: &Network,
    heights: &[f64],
    known: &[bool],
    stay: &[bool],
    water: &[bool],
    classes: &Classes,
    mine: &[bool],
) -> (Vec<Way>, Vec<Join>) {
    let links = joins(net);
    let mut marks = marks(net, &links, classes);
    for (pin, &var) in marks.pin.iter_mut().zip(&net.vertex_var) {
        *pin |= stay[var] || water[var];
    }
    let height = |v: usize| if known[net.vertex_var[v]] { heights[net.vertex_var[v]] } else { f64::NAN };
    let mut ways = vec![Way::default(); links.len()];
    for pieces in strokes(&links) {
        let mut samples = Samples::default();
        // Per piece, the samples of its way in the order of the stroke, with whether each is of OSM.
        let mut lists: Vec<Vec<(usize, bool)>> = Vec::with_capacity(pieces.len());
        for &(w, forward) in &pieces {
            let rule = net.rule[w];
            let free = net.free[w];
            let first = samples.x.len().saturating_sub(1);
            let mut list = Vec::new();
            let order: Vec<usize> = if forward {
                (net.way_first[w]..=net.way_last[w]).collect()
            } else {
                (net.way_first[w]..=net.way_last[w]).rev().collect()
            };
            for (j, &v) in order.iter().enumerate() {
                let (xy, z, pin, calm) = (net.vertex_xy[v], height(v), marks.pin[v], marks.calm[v]);
                let wet = water[net.vertex_var[v]];
                if j == 0 && !samples.x.is_empty() {
                    samples.pin[first] |= pin;
                    samples.calm[first] |= calm;
                    samples.rule[first] = samples.rule[first].min(rule);
                    list.push((first, true));
                    continue;
                }
                if j > 0 {
                    let u = order[j - 1];
                    let (a, b) = (net.vertex_xy[u], height(u));
                    let d = sub(xy, a);
                    let steps = (hypot(d) * metres(a[1] + d[1] / 2.0) / SAMPLE_M).round().max(1.0) as usize;
                    for i in 1..steps {
                        let t = i as f64 / steps as f64;
                        list.push((samples.x.len(), false));
                        samples.push([a[0] + d[0] * t, a[1] + d[1] * t], b + (z - b) * t, false, free, false, rule);
                    }
                }
                samples.vertex.push(samples.x.len());
                list.push((samples.x.len(), net.vertex_osm[v]));
                samples.push(xy, z, pin, calm, wet, rule);
            }
            if !forward {
                list.reverse();
            }
            lists.push(list);
        }
        let mut curve = fitted(&samples, classes);
        let along: Vec<f64> = samples.vertex.iter().map(|&i| curve.s[i]).collect();
        let at: Vec<f64> = samples.vertex.iter().map(|&i| samples.z[i]).collect();
        let slope: Vec<f64> = bessel(&at, &along).iter().map(|g| if g.is_finite() { *g } else { 0.0 }).collect();
        for pair in 0..samples.vertex.len().saturating_sub(1) {
            let (i, j) = (samples.vertex[pair], samples.vertex[pair + 1]);
            for k in i..=j {
                let t = (curve.s[k] - curve.s[i]) / (curve.s[j] - curve.s[i]).max(1e-9);
                curve.grade[k] = slope[pair] + (slope[pair + 1] - slope[pair]) * t;
            }
        }
        for (&(w, forward), list) in pieces.iter().zip(&lists) {
            let order: Vec<usize> = list.iter().map(|l| l.0).collect();
            let unknown = |k: usize| samples.z[order[k]].is_nan();
            // Each job keeps its part of a way alone: a stop at the first vertex of OSM in each area lets
            // the job that owns a chord test it, whatever the job past its end keeps.
            let area = |&(i, _): &(usize, bool)| area_of([samples.x[i], samples.y[i]]);
            let osm: Vec<&(usize, bool)> = list.iter().filter(|l| l.1).collect();
            let entries: HashSet<usize> = osm.windows(2).filter(|p| area(p[0]) != area(p[1])).map(|p| p[1].0).collect();
            let stops: Vec<usize> = (0..order.len())
                .filter(|&k| {
                    k == 0
                        || k + 1 == order.len()
                        || samples.pin[order[k]]
                        || entries.contains(&order[k])
                        || unknown(k - 1) != unknown(k)
                        || unknown(k + 1) != unknown(k)
                })
                .collect();
            // A piece against the stroke turns its tangent, its grade and its left side about.
            let sign = if forward { 1.0 } else { -1.0 };
            let keep = kept(&order, &stops, &curve, &samples.z, classes.rows[net.rule[w]].keep_m, sign);
            let mut osm = 0;
            let mut way = Way::default();
            let mut next = keep.iter().peekable();
            for (k, &(i, of_osm)) in list.iter().enumerate() {
                osm += usize::from(of_osm && k > 0);
                if next.peek() == Some(&&k) {
                    next.next();
                    let [tx, ty] = curve.tangent[i];
                    way.points.push([curve.x[i], curve.y[i]]);
                    way.z.push(samples.z[i]);
                    way.plan.push(Plan {
                        tangent: (sign * ty).atan2(sign * tx),
                        grade: sign * curve.grade[i],
                        bank: sign * curve.bank[i],
                        widen: curve.widen[i],
                        along: 0.0,
                        whole: samples.water[i],
                    });
                    way.osm.push(osm);
                }
            }
            ways[w] = way;
        }
    }
    let mut joined = Vec::new();
    for (w, ends) in links.iter().enumerate() {
        for (end, link) in ends.iter().enumerate() {
            if let Some(&(other, at_last)) = link.as_ref() {
                let vertex = if end == 1 { net.way_last[w] } else { net.way_first[w] };
                if (w, end == 1) < (other, at_last) && mine[vertex] {
                    joined.push([(w, end == 1), (other, at_last)]);
                }
            }
        }
    }
    (ways, joined)
}

/// A way as a polyline of its curve, at most `CHORD_M` metres between its vertices: for what stands
/// beside it. The alignment stays with the vertices of the way, so the polyline has none.
pub fn polyline(part: &Part) -> Part {
    let (Some(plan), Some(z)) = (&part.plan, &part.z) else { return part.clone() };
    let mut out = Part { outer: part.outer, points: vec![part.points[0]], z: Some(vec![z[0]]), plan: None };
    let heights = out.z.as_mut().unwrap();
    for k in 1..part.points.len() {
        let (a, b) = (part.points[k - 1], part.points[k]);
        let scale = metres((a[1] + b[1]) / 2.0);
        let chord = [(b[0] - a[0]) * scale, (b[1] - a[1]) * scale];
        let length = hypot(chord);
        let tangent = |p: &Plan| [p.tangent.cos(), p.tangent.sin()];
        let (t0, t1) = (tangent(&plan[k - 1]), tangent(&plan[k]));
        let steps = (length / CHORD_M).ceil().max(1.0) as usize;
        for step in 1..steps {
            let [h00, h10, h01, h11] = hermite(step as f64 / steps as f64);
            let along = |axis: usize| h10 * t0[axis] * length + h01 * chord[axis] + h11 * t1[axis] * length;
            out.points.push([a[0] + along(0) / scale, a[1] + along(1) / scale]);
            heights.push(h00 * z[k - 1] + h10 * plan[k - 1].grade * length + h01 * z[k] + h11 * plan[k].grade * length);
        }
        // The end itself: a way's ends meet other ways, and a height next to an unknown one stays known.
        out.points.push(b);
        heights.push(z[k]);
    }
    out
}

/// Set the metres along their strokes on the kept vertices of the ways: the length of the curve from
/// the first way of each stroke. `joins` names each way as (feature, part, at its last vertex).
pub(crate) fn measure(ways: &mut [Vec<Option<Way>>], joins: &[[(usize, usize, bool); 2]]) {
    let mut links: HashMap<(usize, usize), [Option<End>; 2]> = HashMap::new();
    for &[(f, p, end), (g, q, other)] in joins {
        links.entry((f, p)).or_default()[usize::from(end)] = Some(((g, q), other));
        links.entry((g, q)).or_default()[usize::from(other)] = Some(((f, p), end));
    }
    let length = |way: &Way, k: usize| {
        let (a, b) = (way.points[k - 1], way.points[k]);
        let scale = metres((a[1] + b[1]) / 2.0);
        let t = |k: usize| [way.plan[k].tangent.cos(), way.plan[k].tangent.sin()];
        arc([(b[0] - a[0]) * scale, (b[1] - a[1]) * scale], t(k - 1), t(k))
    };
    let mut done: HashSet<(usize, usize)> = HashSet::new();
    let order: Vec<(usize, usize)> = (0..ways.len()).flat_map(|f| (0..ways[f].len()).map(move |p| (f, p))).collect();
    for first in order {
        if done.contains(&first) || !ways[first.0][first.1].as_ref().is_some_and(|w| !w.points.is_empty()) {
            continue;
        }
        let entry = |way: (usize, usize), forward: bool| links.get(&way).and_then(|ends| ends[usize::from(!forward)]);
        let (mut head, mut forward, mut seen) = (first, true, HashSet::from([first]));
        while let Some((way, at_last)) = entry(head, forward) {
            if done.contains(&way) || !seen.insert(way) {
                break;
            }
            (head, forward) = (way, at_last);
        }
        let mut total = 0.0;
        let mut piece = Some((head, forward));
        while let Some((id, forward)) = piece.filter(|p| !done.contains(&p.0)) {
            done.insert(id);
            let way = ways[id.0][id.1].as_mut().unwrap();
            let steps: Vec<f64> = (1..way.points.len()).map(|k| length(way, k)).collect();
            let full: f64 = steps.iter().sum();
            let mut along = if forward { total } else { total + full };
            for (plan, step) in way.plan.iter_mut().zip(steps.iter().chain([&0.0])) {
                plan.along = along;
                along += if forward { *step } else { -*step };
            }
            total += full;
            piece = links
                .get(&id)
                .and_then(|ends| ends[usize::from(forward)])
                .filter(|(next, _)| ways[next.0][next.1].is_some())
                .map(|(next, at_last)| (next, !at_last));
        }
    }
}
