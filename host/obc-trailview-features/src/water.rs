//! The water levels: each lake at one level, and the rivers and streams as networks whose levels
//! never rise downstream. `specs/trailview-features.md` says what the levels promise.
//!
//! The terrain is f32, and the statistics of a lake run in f32 on it, as numpy runs them.

use std::collections::HashMap;

use rayon::prelude::*;
use rstar::primitives::{GeomWithData, Rectangle};
use rstar::{RTree, AABB};

use crate::{metres, Part, Terrain};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A river or a stream.
    River,
    Lake,
    /// A waterfall, a dam or a dam point: the solver only reads it.
    Mark,
}

/// How a lake finds its level, from its tags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Natural,
    Pond,
    Reservoir,
}

#[derive(Clone, Debug)]
pub struct Feature {
    pub kind: Kind,
    pub level: Level,
    /// The half-width of a waterway in metres.
    pub half: f64,
    /// The `ele` tag, NaN without one.
    pub ele: f64,
    pub parts: Vec<Part>,
}

/// Per feature, per part: the heights of its points (NaN where unknown), or None.
pub type Heights = Vec<Vec<Option<Vec<f64>>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub lakes: usize,
    pub lakes_spilling: usize,
    pub river_parts: usize,
    pub turned: usize,
}

impl std::ops::AddAssign for Metrics {
    fn add_assign(&mut self, other: Self) {
        self.lakes += other.lakes;
        self.lakes_spilling += other.lakes_spilling;
        self.river_parts += other.river_parts;
        self.turned += other.turned;
    }
}

/// A river observes the terrain every STEP_M along it, at its lowest within ±(w/2 + BESIDE_M)
/// across it, at the ACROSS points.
const STEP_M: f64 = 5.0;
const BESIDE_M: f64 = 2.0;
const ACROSS: [f64; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];
/// The fit holds z + LEAST_GRADE × s from rising downstream, so that a pool still falls a little and
/// the smoothing does not stair-step it.
const LEAST_GRADE: f64 = 0.0005;
/// The smoothing: a triangle kernel of this half-width.
const SMOOTH_M: f64 = 25.0;
/// A way turns round when its terrain rises by more than this from end to end: OSM draws it uphill.
const UPHILL_M: f64 = 5.0;
/// A sample in a lake, or this near its shore, holds the lake's level with the weight PIN.
const SHORE_M: f64 = 2.0;
const PIN: f64 = 1000.0;
/// A fall, a weir or a dam this near a sample splits the smoothing there.
const FALL_M: f64 = 3.0;
/// Lakes: the median of the interior holds where FLAT_SHARE of it lies within FLAT_M of it; else a
/// shore quantile, INSET_M inside. An `ele` tag within ELE_M of the level wins.
const FLAT_M: f32 = 0.3;
const FLAT_SHARE: f64 = 0.8;
const INSET_M: f64 = 2.0;
const SHORE_QUANTILE: f64 = 15.0;
const ELE_M: f32 = 1.5;
/// A lake spills where its shore, away from its rivers, lies this far below its level.
const SPILL_M: f64 = 0.5;
const RIVER_NEAR_M: f64 = 30.0;
/// A lake reads the terrain at no more than this many points, and at most at the pixels of the
/// deepest zoom of the terrain.
const LAKE_POINTS: f64 = 250_000.0;
const TILE: f64 = 512.0;

type Key = [u64; 2];

fn key(point: [f64; 2]) -> Key {
    [point[0].to_bits(), point[1].to_bits()]
}

/// The levels of `features`, each network apart on the threads of the current rayon pool. A river
/// part that OSM draws uphill turns round in place; `turned` lists them as (feature, part).
pub fn solve_by_network(features: &mut [Feature], terrain: &dyn Terrain) -> (Heights, Vec<(usize, usize)>, Metrics) {
    let mut found = groups(features);
    let size =
        |group: &Vec<usize>| -> usize { group.iter().flat_map(|&i| &features[i].parts).map(|p| p.points.len()).sum() };
    found.sort_by_key(|group| std::cmp::Reverse(size(group)));
    let marks = Marks::new(features);
    let shared: &[Feature] = features;
    let solved: Vec<Solved> =
        found.par_iter().with_max_len(1).map(|group| solve_group(shared, group, &marks, terrain)).collect();
    let mut heights: Heights = features.iter().map(|f| vec![None; f.parts.len()]).collect();
    let (mut turned, mut metrics) = (Vec::new(), Metrics::default());
    for group in solved {
        for (index, z) in group.heights {
            heights[index] = z;
        }
        turned.extend(group.turned);
        metrics += group.metrics;
    }
    turned.sort_unstable();
    for &(f, p) in &turned {
        features[f].parts[p].points.reverse();
    }
    (heights, turned, metrics)
}

/// The indices of the features that solve apart from the others, each list in their order: each
/// network of rivers and streams that share a point, with each lake that comes within
/// 4 × RIVER_NEAR_M of it and the networks of those lakes; each other lake alone. A level depends on
/// its whole network, a lake on the rivers that it pins and that `spills` looks at, at most
/// 2 × RIVER_NEAR_M from it.
pub fn groups(features: &[Feature]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..features.len()).collect();
    fn root(parent: &mut [usize], mut i: usize) -> usize {
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    let mut first: HashMap<Key, usize> = HashMap::new();
    let (mut boxes, mut owners) = (Vec::new(), Vec::new());
    for (index, feature) in features.iter().enumerate().filter(|(_, f)| f.kind == Kind::River) {
        for part in &feature.parts {
            for &point in &part.points {
                let at = *first.entry(key(point)).or_insert(index);
                let (a, b) = (root(&mut parent, at), root(&mut parent, index));
                parent[a] = b;
            }
            if !part.points.is_empty() {
                boxes.push(bounds([part.points.as_slice()]));
                owners.push(index);
            }
        }
    }
    let boxes = Boxes::new(&boxes);
    for (index, feature) in features.iter().enumerate().filter(|(_, f)| f.kind == Kind::Lake) {
        if feature.parts.iter().all(|p| p.points.is_empty()) {
            continue;
        }
        let b = bounds(feature.parts.iter().map(|p| p.points.as_slice()));
        let margin = 4.0 * RIVER_NEAR_M / metres(b[1]);
        for i in boxes.overlapping([b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin]) {
            let (a, b) = (root(&mut parent, owners[i]), root(&mut parent, index));
            parent[a] = b;
        }
    }
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for (index, feature) in features.iter().enumerate() {
        if feature.kind == Kind::Mark {
            continue;
        }
        let r = root(&mut parent, index);
        let at = *slot.entry(r).or_insert_with(|| {
            out.push(Vec::new());
            out.len() - 1
        });
        out[at].push(index);
    }
    out
}

/// The levels of one group of `groups`, or of every river and lake when the features are one group:
/// per feature of the group its heights, the parts that turned round, and the metrics.
struct Solved {
    heights: Vec<(usize, Vec<Option<Vec<f64>>>)>,
    turned: Vec<(usize, usize)>,
    metrics: Metrics,
}

/// The points of the falls, weirs and dams.
struct Marks(RTree<[f64; 2]>);

impl Marks {
    fn new(features: &[Feature]) -> Self {
        let points = features
            .iter()
            .filter(|f| f.kind == Kind::Mark)
            .flat_map(|f| &f.parts)
            .flat_map(|p| p.points.iter().copied());
        Self(RTree::bulk_load(points.collect()))
    }
}

struct Lake<'a> {
    rings: Vec<&'a [[f64; 2]]>,
    bounds: [f64; 4],
    level: f64,
}

fn solve_group(features: &[Feature], group: &[usize], marks: &Marks, terrain: &dyn Terrain) -> Solved {
    let lakes: Vec<(usize, Lake)> = group
        .par_iter()
        .filter(|&&i| features[i].kind == Kind::Lake)
        .map(|&index| {
            let feature = &features[index];
            let rings: Vec<&[[f64; 2]]> = feature.parts.iter().map(|p| p.points.as_slice()).collect();
            let level = lake_level(feature, terrain);
            (index, Lake { bounds: bounds(rings.iter().copied()), rings, level })
        })
        .collect();
    let parts: Vec<(usize, usize)> = group
        .iter()
        .filter(|&&i| features[i].kind == Kind::River)
        .flat_map(|&i| {
            (0..features[i].parts.len()).filter(move |&n| features[i].parts[n].points.len() >= 2).map(move |n| (i, n))
        })
        .collect();
    let solved: Vec<(Way, bool)> = parts
        .par_iter()
        .map(|&(index, n)| {
            let (feature, part) = (&features[index], &features[index].parts[n]);
            let mut way = Way::new(part.points.clone(), feature.half, terrain);
            let uphill = way.uphill();
            if uphill {
                let mut points = part.points.clone();
                points.reverse();
                way = Way::new(points, feature.half, terrain);
            }
            way.feature = index;
            way.part = n;
            (way, uphill)
        })
        .collect();
    let turned: Vec<(usize, usize)> =
        solved.iter().filter(|(_, uphill)| *uphill).map(|(w, _)| (w.feature, w.part)).collect();
    let mut ways: Vec<Way> = solved.into_iter().map(|(way, _)| way).collect();
    pin(&mut ways, &lakes);
    let boxes = Boxes::new(&ways.iter().map(|w| w.bounds).collect::<Vec<_>>());
    let levels = network(&ways, marks);
    let mut heights: HashMap<usize, Vec<Option<Vec<f64>>>> =
        group.iter().map(|&i| (i, vec![None; features[i].parts.len()])).collect();
    for (index, lake) in &lakes {
        let level = if lake.level.is_finite() { lake.level } else { f64::NAN };
        heights.insert(*index, features[*index].parts.iter().map(|p| Some(vec![level; p.points.len()])).collect());
    }
    for (way, z) in ways.iter().zip(&levels) {
        heights.get_mut(&way.feature).unwrap()[way.part] = Some(way.vertices.iter().map(|&v| z[v]).collect());
    }
    let mut spilling = 0;
    for (_, lake) in &lakes {
        let margin = 2.0 * RIVER_NEAR_M / metres(lake.bounds[1]);
        let b = lake.bounds;
        let near: Vec<&Way> = boxes
            .overlapping([b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin])
            .into_iter()
            .map(|i| &ways[i])
            .collect();
        spilling += (lake.level.is_finite() && spills(lake.level, &lake.rings, terrain, &near)) as usize;
    }
    let metrics =
        Metrics { lakes: lakes.len(), lakes_spilling: spilling, river_parts: ways.len(), turned: turned.len() };
    let mut heights: Vec<_> = heights.into_iter().collect();
    heights.sort_unstable_by_key(|(i, _)| *i);
    Solved { heights, turned, metrics }
}

/// A river part: its points (world coordinates, y down) in the direction of its flow, the samples
/// along it every STEP_M with the lowest terrain across it, and their weights.
struct Way {
    points: Vec<[f64; 2]>,
    xy: Vec<[f64; 2]>,
    along: Vec<f64>,
    /// The sample of each point.
    vertices: Vec<usize>,
    bounds: [f64; 4],
    low: Vec<f64>,
    weight: Vec<f64>,
    feature: usize,
    part: usize,
}

impl Way {
    fn new(points: Vec<[f64; 2]>, half: f64, terrain: &dyn Terrain) -> Self {
        let d: Vec<[f64; 2]> = points.windows(2).map(|w| [w[1][0] - w[0][0], w[1][1] - w[0][1]]).collect();
        let lengths: Vec<f64> =
            points.windows(2).zip(&d).map(|(w, d)| d[0].hypot(d[1]) * metres((w[0][1] + w[1][1]) / 2.0)).collect();
        let steps: Vec<usize> = lengths.iter().map(|l| (l / STEP_M).ceil().max(1.0) as usize).collect();
        let mut sample = Vec::new();
        for (k, &n) in steps.iter().enumerate() {
            sample.extend((0..n).map(|i| (k, i as f64 / n as f64)));
        }
        sample.push((d.len() - 1, 1.0));
        let mut start = vec![0.0];
        for l in &lengths {
            start.push(start.last().unwrap() + l);
        }
        let xy: Vec<[f64; 2]> =
            sample.iter().map(|&(k, t)| [points[k][0] + t * d[k][0], points[k][1] + t * d[k][1]]).collect();
        let along = sample.iter().map(|&(k, t)| start[k] + t * lengths[k]).collect();
        let mut vertices = vec![0];
        for n in &steps {
            vertices.push(vertices.last().unwrap() + n);
        }
        let normal: Vec<[f64; 2]> = d
            .iter()
            .map(|d| {
                let norm = d[0].hypot(d[1]).max(1e-15);
                [-d[1] / norm, d[0] / norm]
            })
            .collect();
        let mut low = vec![f64::NAN; xy.len()];
        for o in ACROSS {
            let o = o * (half + BESIDE_M);
            let across: Vec<[f64; 2]> = xy
                .iter()
                .zip(&sample)
                .map(|(p, &(k, _))| {
                    let s = o / metres(p[1]);
                    [p[0] + normal[k][0] * s, p[1] + normal[k][1] * s]
                })
                .collect();
            for (low, h) in low.iter_mut().zip(terrain.heights(&across)) {
                if h < *low || low.is_nan() {
                    *low = h;
                }
            }
        }
        let weight = low.iter().map(|h| if h.is_finite() { 1.0 } else { 0.0 }).collect();
        let bounds = bounds([xy.as_slice()]);
        Self { points, xy, along, vertices, bounds, low, weight, feature: 0, part: 0 }
    }

    /// Whether the terrain along the way rises by more than UPHILL_M: a straight fit to its lows.
    fn uphill(&self) -> bool {
        let known: Vec<(f64, f64)> =
            self.along.iter().zip(&self.low).filter(|(_, h)| h.is_finite()).map(|(&s, &h)| (s, h)).collect();
        let length = *self.along.last().unwrap();
        if known.len() < 2 || length <= 0.0 {
            return false;
        }
        let n = known.len() as f64;
        let (ms, mh) = (known.iter().map(|k| k.0).sum::<f64>() / n, known.iter().map(|k| k.1).sum::<f64>() / n);
        let sxy: f64 = known.iter().map(|(s, h)| (s - ms) * (h - mh)).sum();
        let sxx: f64 = known.iter().map(|(s, _)| (s - ms) * (s - ms)).sum();
        sxx > 0.0 && sxy / sxx * length > UPHILL_M
    }
}

/// A sample in a lake, or within SHORE_M of its shore, holds the lake's level with the weight PIN.
fn pin(ways: &mut [Way], lakes: &[(usize, Lake)]) {
    let lakes: Vec<([f64; 4], f64, Edges)> = lakes
        .par_iter()
        .filter(|(_, lake)| lake.level.is_finite())
        .map(|(_, lake)| {
            let margin = SHORE_M / metres(lake.bounds[1]);
            let b = lake.bounds;
            ([b[0] - margin, b[1] - margin, b[2] + margin, b[3] + margin], lake.level, Edges::new(&lake.rings))
        })
        .collect();
    let boxes = Boxes::new(&lakes.iter().map(|(b, _, _)| *b).collect::<Vec<_>>());
    ways.par_iter_mut().for_each(|way| {
        for k in boxes.overlapping(way.bounds) {
            let (b, level, edges) = &lakes[k];
            for s in 0..way.xy.len() {
                let [x, y] = way.xy[s];
                if !(x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]) {
                    continue;
                }
                if edges.contains(x, y) || edges.distance(x, y, SHORE_M / metres(y)) * metres(y) < SHORE_M {
                    way.low[s] = *level;
                    way.weight[s] = PIN;
                }
            }
        }
    });
}

/// The levels of every sample of the ways. The ways split into reaches at the points that they
/// share; the reaches are solved downstream in turn, each held under the levels that flow into its
/// start, and all reaches at a junction then take one level there. A loop of reaches has no order of
/// flow: its reaches go from the highest start down (a spanning DAG), and a reach that flows back
/// into a junction solved before it keeps its own end there, a step down to it.
fn network(ways: &[Way], marks: &Marks) -> Vec<Vec<f64>> {
    let mut counts: HashMap<Key, usize> = HashMap::new();
    for way in ways {
        for &point in &way.points {
            *counts.entry(key(point)).or_default() += 1;
        }
    }
    // (way, first sample, last sample, first point, last point), and per junction in the order of
    // its first reach the reaches into it and out of it.
    let mut reaches: Vec<(usize, usize, usize, Key, Key)> = Vec::new();
    let mut slot: HashMap<Key, usize> = HashMap::new();
    let mut junctions: Vec<(Vec<usize>, Vec<usize>)> = Vec::new();
    // The junctions of the start and of the end of each reach.
    let mut at = Vec::new();
    for (w, way) in ways.iter().enumerate() {
        let last = way.points.len() - 1;
        let cuts: Vec<usize> = std::iter::once(0)
            .chain((1..last).filter(|&i| counts[&key(way.points[i])] > 1))
            .chain(std::iter::once(last))
            .collect();
        for pair in cuts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (first, end) = (key(way.points[a]), key(way.points[b]));
            reaches.push((w, way.vertices[a], way.vertices[b], first, end));
            let mut junction = |point: Key| {
                *slot.entry(point).or_insert_with(|| {
                    junctions.push((Vec::new(), Vec::new()));
                    junctions.len() - 1
                })
            };
            let start = junction(first);
            at.push((start, junction(end)));
        }
    }
    for (r, &(start, stop)) in at.iter().enumerate() {
        junctions[start].1.push(r);
        junctions[stop].0.push(r);
    }
    let mut levels: Vec<Vec<f64>> = ways.iter().map(|w| vec![f64::NAN; w.xy.len()]).collect();
    let mut ends: HashMap<Key, Vec<f64>> = HashMap::new();
    let mut rank = vec![usize::MAX; reaches.len()];
    let mut solved = vec![(f64::NAN, f64::NAN); reaches.len()];
    let after: Vec<&[usize]> = at.iter().map(|&(_, stop)| junctions[stop].1.as_slice()).collect();
    let start = |r: usize| ways[reaches[r].0].low[reaches[r].1];
    let mut ranked = 0;
    for mut component in components(&after).into_iter().rev() {
        let order = |r: usize| if start(r).is_finite() { -start(r) } else { f64::INFINITY };
        component.sort_by(|&p, &q| order(p).partial_cmp(&order(q)).unwrap());
        for r in component {
            let (w, a, b, first, end) = reaches[r];
            let cap = ends.get(&first).map_or(f64::INFINITY, |v| v.iter().copied().fold(f64::INFINITY, f64::min));
            let z = reach_levels(&ways[w], a, b, cap, marks);
            levels[w][a..=b].copy_from_slice(&z);
            rank[r] = ranked;
            ranked += 1;
            solved[r] = (z[0], z[z.len() - 1]);
            if z[z.len() - 1].is_finite() {
                ends.entry(end).or_default().push(z[z.len() - 1]);
            }
        }
    }
    // One level at a junction: the highest start that flows out of it, held under every level that
    // flows into it before them; where nothing flows out, the lowest level that flows in.
    for (into, out) in &junctions {
        let first = out.iter().map(|&r| rank[r]).min().unwrap_or(usize::MAX);
        let ins: Vec<f64> =
            into.iter().filter(|&&r| rank[r] < first).map(|&r| solved[r].1).filter(|v| v.is_finite()).collect();
        let starts: Vec<f64> = out.iter().map(|&r| solved[r].0).filter(|v| v.is_finite()).collect();
        if ins.is_empty() && starts.is_empty() {
            continue;
        }
        let top = starts.iter().copied().reduce(f64::max).unwrap_or(f64::INFINITY);
        let level = ins.iter().copied().fold(top, f64::min);
        for &r in into.iter().filter(|&&r| rank[r] < first) {
            let z = &mut levels[reaches[r].0][reaches[r].2];
            if level < *z {
                *z = level;
            }
        }
        for &r in out {
            levels[reaches[r].0][reaches[r].1] = level;
        }
    }
    levels
}

/// The strongly connected components of the graph whose node r leads to the nodes after[r], each
/// after every component that it leads to (Tarjan).
fn components(after: &[&[usize]]) -> Vec<Vec<usize>> {
    let n = after.len();
    let (mut index, mut low) = (vec![usize::MAX; n], vec![0; n]);
    let (mut stack, mut on, mut out) = (Vec::new(), vec![false; n], Vec::new());
    let mut counter = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        index[root] = counter;
        low[root] = counter;
        counter += 1;
        stack.push(root);
        on[root] = true;
        let mut work = vec![(root, 0)];
        while let Some(&(node, position)) = work.last() {
            let mut next = position;
            let mut descended = false;
            while next < after[node].len() {
                let child = after[node][next];
                next += 1;
                if index[child] == usize::MAX {
                    index[child] = counter;
                    low[child] = counter;
                    counter += 1;
                    stack.push(child);
                    on[child] = true;
                    work.last_mut().unwrap().1 = next;
                    work.push((child, 0));
                    descended = true;
                    break;
                }
                if on[child] {
                    low[node] = low[node].min(index[child]);
                }
            }
            if descended {
                continue;
            }
            work.pop();
            if let Some(&(parent, _)) = work.last() {
                low[parent] = low[parent].min(low[node]);
            }
            if low[node] == index[node] {
                let mut component = Vec::new();
                while component.last() != Some(&node) {
                    let member = stack.pop().unwrap();
                    on[member] = false;
                    component.push(member);
                }
                out.push(component);
            }
        }
    }
    out
}

/// The levels of the samples a..=b of a way: the fit that never rises to the lows plus LEAST_GRADE
/// per metre, under the cap, smoothed apart at falls, weirs and dams.
fn reach_levels(way: &Way, a: usize, b: usize, cap: f64, marks: &Marks) -> Vec<f64> {
    let (low, weight, along) = (&way.low[a..=b], &way.weight[a..=b], &way.along[a..=b]);
    let known: Vec<usize> = (0..low.len()).filter(|&i| weight[i] > 0.0).collect();
    if known.is_empty() {
        return vec![f64::NAN; low.len()];
    }
    let s: Vec<f64> = along.iter().map(|v| v - along[0]).collect();
    let sk: Vec<f64> = known.iter().map(|&i| s[i]).collect();
    let y: Vec<f64> = known.iter().map(|&i| low[i] + LEAST_GRADE * s[i]).collect();
    let w: Vec<f64> = known.iter().map(|&i| weight[i]).collect();
    let fit: Vec<f64> = falling(&y, &w).iter().zip(&sk).map(|(f, s)| f - LEAST_GRADE * s).collect();
    let mut z: Vec<f64> = s.iter().map(|&x| interp(x, &sk, &fit)).collect();
    let mut least = f64::INFINITY;
    for v in &mut z {
        least = least.min(v.min(cap));
        *v = least;
    }
    let mut splits = Vec::new();
    if marks.0.size() > 0 {
        let xy = &way.xy[a..=b];
        let [x0, y0, x1, y1] = bounds([xy]);
        let envelope = AABB::from_corners([x0 - 1e-5, y0 - 1e-5], [x1 + 1e-5, y1 + 1e-5]);
        let near: Vec<[f64; 2]> = marks.0.locate_in_envelope(&envelope).copied().collect();
        for (i, p) in xy.iter().enumerate().skip(1) {
            if near.iter().any(|&[x, y]| (p[0] - x).hypot(p[1] - y) * metres(y) < FALL_M) {
                splits.push(i);
            }
        }
    }
    smoothed(&z, &splits)
}

/// The weighted least-squares fit to y that never rises (pool-adjacent violators).
fn falling(y: &[f64], w: &[f64]) -> Vec<f64> {
    let (mut means, mut weights, mut counts): (Vec<f64>, Vec<f64>, Vec<usize>) = (Vec::new(), Vec::new(), Vec::new());
    for (&value, &weight) in y.iter().zip(w) {
        means.push(value);
        weights.push(weight);
        counts.push(1);
        while means.len() > 1 && means[means.len() - 2] < means[means.len() - 1] {
            let n = means.len();
            let weight = weights[n - 2] + weights[n - 1];
            means[n - 2] = (means[n - 2] * weights[n - 2] + means[n - 1] * weights[n - 1]) / weight;
            weights[n - 2] = weight;
            counts[n - 2] += counts[n - 1];
            means.pop();
            weights.pop();
            counts.pop();
        }
    }
    means.iter().zip(&counts).flat_map(|(&m, &c)| std::iter::repeat_n(m, c)).collect()
}

/// z under a triangle kernel of SMOOTH_M, with each end held: a positive kernel keeps a profile that
/// never rises. The pieces between splits are smoothed apart.
fn smoothed(z: &[f64], splits: &[usize]) -> Vec<f64> {
    let k = (SMOOTH_M / STEP_M).floor() as usize;
    let mut kernel: Vec<f64> = (0..2 * k + 1).map(|i| 1.0 - (i as f64 - k as f64).abs() / (k + 1) as f64).collect();
    let total = numpy_sum(&kernel);
    kernel.iter_mut().for_each(|v| *v /= total);
    let mut bounds = vec![0];
    bounds.extend_from_slice(splits);
    bounds.push(z.len());
    let mut out = Vec::with_capacity(z.len());
    for pair in bounds.windows(2).filter(|pair| pair[1] > pair[0]) {
        let piece = &z[pair[0]..pair[1]];
        let at = |i: usize| piece[i.saturating_sub(k).min(piece.len() - 1)];
        for i in 0..piece.len() {
            let mut sum = 0.0;
            for (j, weight) in kernel.iter().rev().enumerate() {
                sum += at(i + j) * weight;
            }
            out.push(sum);
        }
    }
    out
}

/// numpy's pairwise sum of a contiguous float64 array.
fn numpy_sum(a: &[f64]) -> f64 {
    let n = a.len();
    if n < 8 {
        a.iter().fold(-0.0, |sum, v| sum + v)
    } else if n <= 128 {
        let mut r = [0.0; 8];
        r.copy_from_slice(&a[..8]);
        let mut i = 8;
        while i < n - n % 8 {
            for j in 0..8 {
                r[j] += a[i + j];
            }
            i += 8;
        }
        let mut sum = ((r[0] + r[1]) + (r[2] + r[3])) + ((r[4] + r[5]) + (r[6] + r[7]));
        for v in &a[i..] {
            sum += v;
        }
        sum
    } else {
        let half = n / 2;
        let half = half - half % 8;
        numpy_sum(&a[..half]) + numpy_sum(&a[half..])
    }
}

/// numpy's `interp` at x, xp non-decreasing.
fn interp(x: f64, xp: &[f64], fp: &[f64]) -> f64 {
    let n = xp.len();
    if x.is_nan() {
        return x;
    }
    if n == 1 || x < xp[0] {
        return fp[0];
    }
    if x > xp[n - 1] {
        return fp[n - 1];
    }
    let j = xp.partition_point(|&v| v <= x) - 1;
    if j == n - 1 || xp[j] == x {
        return fp[j];
    }
    let slope = (fp[j + 1] - fp[j]) / (xp[j + 1] - xp[j]);
    let z = slope * (x - xp[j]) + fp[j];
    if !z.is_nan() {
        return z;
    }
    let z = slope * (x - xp[j + 1]) + fp[j + 1];
    if z.is_nan() && fp[j] == fp[j + 1] {
        fp[j]
    } else {
        z
    }
}

/// The level of a lake: the median of its interior where it is flat; else the SHORE_QUANTILE of its
/// shore, INSET_M inside. A pond takes its lowest shore, a reservoir its shore at the outline, the
/// full level. An `ele` tag within ELE_M wins. NaN where the terrain knows none of it.
fn lake_level(lake: &Feature, terrain: &dyn Terrain) -> f64 {
    let rings: Vec<&[[f64; 2]]> = lake.parts.iter().map(|p| p.points.as_slice()).collect();
    let finite = |h: Vec<f64>| -> Vec<f32> { h.into_iter().filter(|v| v.is_finite()).map(|v| v as f32).collect() };
    let shore = finite(terrain.heights(&ring_points(&rings, STEP_M, INSET_M)));
    let level = match lake.level {
        Level::Pond => shore.iter().copied().reduce(f32::min).unwrap_or(f32::NAN),
        Level::Reservoir => median(finite(terrain.heights(&ring_points(&rings, STEP_M, 0.0)))),
        Level::Natural => {
            let h = finite(terrain.heights(&interior(&lake.parts, terrain.top())));
            let middle = median(h.clone());
            let flat = h.iter().filter(|&&v| (v - middle).abs() <= FLAT_M).count();
            if !h.is_empty() && flat as f64 / h.len() as f64 >= FLAT_SHARE {
                middle
            } else if !shore.is_empty() {
                percentile(shore, SHORE_QUANTILE)
            } else {
                middle
            }
        }
    };
    if (lake.ele as f32 - level).abs() <= ELE_M {
        lake.ele
    } else {
        level as f64
    }
}

/// numpy's median of f32 values, NaN of none.
fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// numpy's linear percentile q of f32 values, not empty.
fn percentile(mut v: Vec<f32>, q: f64) -> f32 {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    let virtual_index = (n - 1) as f64 * (q / 100.0);
    if virtual_index >= (n - 1) as f64 {
        return v[n - 1];
    }
    let previous = virtual_index.floor();
    let gamma = virtual_index - previous;
    let (a, b) = (v[previous as usize], v[previous as usize + 1]);
    let d = b - a;
    if gamma >= 0.5 {
        b - d * (1.0 - gamma) as f32
    } else {
        a + d * gamma as f32
    }
}

/// The points of a grid inside the rings of the parts, at the pixels of the deepest zoom or
/// LAKE_POINTS points at most, row by row.
fn interior(parts: &[Part], top: u32) -> Vec<[f64; 2]> {
    let rings: Vec<&[[f64; 2]]> = parts.iter().map(|p| p.points.as_slice()).collect();
    let [x0, y0, x1, y1] = bounds(rings.iter().copied());
    let mut area = 0.0;
    for part in parts {
        let r = &part.points;
        let terms: Vec<f64> =
            (0..r.len()).map(|i| r[i][0] * r[(i + 1) % r.len()][1] - r[(i + 1) % r.len()][0] * r[i][1]).collect();
        area += if part.outer { 1.0 } else { -1.0 } * numpy_sum(&terms).abs() / 2.0;
    }
    let pixel = 1.0 / ((1u64 << top) as f64 * TILE);
    let fill = (area.max(0.0) / LAKE_POINTS).sqrt();
    let step = if fill > pixel { fill } else { pixel };
    let (xs, ys) = (arange(x0 + step / 2.0, x1, step), arange(y0 + step / 2.0, y1, step));
    let edges = Edges::new(&rings);
    let mut out = Vec::new();
    for &row in &ys {
        let mut cross = edges.row(row);
        cross.sort_by(f64::total_cmp);
        let mut left = 0;
        for &x in &xs {
            while left < cross.len() && cross[left] < x {
                left += 1;
            }
            if left % 2 == 1 {
                out.push([x, row]);
            }
        }
    }
    out
}

/// numpy's `arange(start, stop, step)` of floats.
fn arange(start: f64, stop: f64, step: f64) -> Vec<f64> {
    let len = ((stop - start) / step).ceil();
    if len.is_nan() || len <= 0.0 {
        return Vec::new();
    }
    let mut out = vec![start];
    if len > 1.0 {
        let next = start + step;
        let delta = next - start;
        out.push(next);
        out.extend((2..len as usize).map(|i| start + i as f64 * delta));
    }
    out
}

/// Whether a lake's shore lies below its level by more than SPILL_M, INSET_M outside it and away
/// from its rivers.
fn spills(level: f64, rings: &[&[[f64; 2]]], terrain: &dyn Terrain, rivers: &[&Way]) -> bool {
    let outside = ring_points(rings, STEP_M, -INSET_M);
    if outside.is_empty() {
        return false;
    }
    let below = (level - SPILL_M) as f32;
    let watched: Vec<[f64; 2]> = rivers
        .iter()
        .flat_map(|way| {
            let step = (way.points.len() / 64).max(1);
            way.points.iter().step_by(step).chain(way.points.last()).copied()
        })
        .collect();
    terrain.heights(&outside).iter().zip(&outside).any(|(&h, p)| {
        (h as f32) < below && watched.iter().all(|&[x, y]| (p[0] - x).hypot(p[1] - y) * metres(y) > RIVER_NEAR_M)
    })
}

/// Points every step_m along the rings, moved offset_m inside the area (negative: outside).
fn ring_points(rings: &[&[[f64; 2]]], step_m: f64, offset_m: f64) -> Vec<[f64; 2]> {
    let (mut points, mut normals) = (Vec::new(), Vec::new());
    for ring in rings {
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let n = (d[0].hypot(d[1]) * metres((a[1] + b[1]) / 2.0) / step_m).ceil().max(1.0) as usize;
            let norm = d[0].hypot(d[1]).max(1e-15);
            for j in 0..n {
                let t = (j as f64 + 0.5) / n as f64;
                points.push([a[0] + t * d[0], a[1] + t * d[1]]);
                normals.push([(a[1] - b[1]) / norm, (b[0] - a[0]) / norm]);
            }
        }
    }
    let edges = Edges::new(rings);
    points
        .iter()
        .zip(&normals)
        .map(|(p, n)| {
            let step = offset_m.abs() / metres(p[1]);
            let probe = [p[0] + n[0] * step / 100.0, p[1] + n[1] * step / 100.0];
            let sign = if edges.contains(probe[0], probe[1]) == (offset_m >= 0.0) { 1.0 } else { -1.0 };
            [p[0] + sign * n[0] * step, p[1] + sign * n[1] * step]
        })
        .collect()
}

/// The edges (ax, ay, bx, by) of the rings of at least 3 points, and their boxes.
struct Edges {
    edges: Vec<[f64; 4]>,
    boxes: Boxes,
    right: f64,
}

impl Edges {
    fn new(rings: &[&[[f64; 2]]]) -> Self {
        let edges: Vec<[f64; 4]> = rings
            .iter()
            .filter(|r| r.len() >= 3)
            .flat_map(|r| (0..r.len()).map(|i| [r[i][0], r[i][1], r[(i + 1) % r.len()][0], r[(i + 1) % r.len()][1]]))
            .collect();
        let boxes: Vec<[f64; 4]> =
            edges.iter().map(|&[ax, ay, bx, by]| [ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]).collect();
        let right = boxes.iter().map(|b| b[2]).fold(f64::NEG_INFINITY, f64::max);
        Edges { edges, boxes: Boxes::new(&boxes), right }
    }

    /// The edges whose boxes meet the box b, in order.
    fn meeting(&self, b: [f64; 4]) -> impl Iterator<Item = &[f64; 4]> {
        self.boxes.overlapping(b).into_iter().map(|i| &self.edges[i])
    }

    /// Whether a point lies inside the edges, even-odd. An edge that the ray to the right crosses has
    /// its box on the ray, give or take the round-off of the crossing.
    fn contains(&self, x: f64, y: f64) -> bool {
        self.meeting([x - 1e-9, y, self.right, y])
            .filter(|&&[ax, ay, bx, by]| (ay > y) != (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay))
            .count()
            % 2
            == 1
    }

    /// The distance from a point to the nearest edge in world units, infinite past `reach` and more.
    fn distance(&self, x: f64, y: f64, reach: f64) -> f64 {
        let r = 2.0 * reach;
        self.meeting([x - r, y - r, x + r, y + r]).fold(f64::INFINITY, |best, &[ax, ay, bx, by]| {
            let (dx, dy) = (bx - ax, by - ay);
            let t = (((x - ax) * dx + (y - ay) * dy) / (dx * dx + dy * dy).max(1e-30)).clamp(0.0, 1.0);
            best.min((x - ax - t * dx).hypot(y - ay - t * dy))
        })
    }

    /// The x of each crossing of an edge with the row y, unsorted.
    fn row(&self, y: f64) -> Vec<f64> {
        self.meeting([f64::NEG_INFINITY, y, self.right, y])
            .filter(|e| (e[1] <= y) != (e[3] <= y))
            .map(|&[ax, ay, bx, by]| ax + (y - ay) * (bx - ax) / (by - ay))
            .collect()
    }
}

/// The box (x0, y0, x1, y1) of the points of lines.
fn bounds<'a>(lines: impl IntoIterator<Item = &'a [[f64; 2]]>) -> [f64; 4] {
    let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for &[x, y] in lines.into_iter().flatten() {
        b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
    }
    b
}

/// Boxes (x0, y0, x1, y1) to find the ones that meet a box, edges included.
struct Boxes(RTree<GeomWithData<Rectangle<[f64; 2]>, usize>>);

impl Boxes {
    fn new(boxes: &[[f64; 4]]) -> Self {
        let items = boxes
            .iter()
            .enumerate()
            .map(|(i, b)| GeomWithData::new(Rectangle::from_corners([b[0], b[1]], [b[2], b[3]]), i));
        Self(RTree::bulk_load(items.collect()))
    }

    /// The numbers of the boxes that meet `b`, in order.
    fn overlapping(&self, b: [f64; 4]) -> Vec<usize> {
        let envelope = AABB::from_corners([b[0], b[1]], [b[2], b[3]]);
        let mut out: Vec<usize> = self.0.locate_in_envelope_intersecting(&envelope).map(|item| item.data).collect();
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point at Engelberg in world coordinates, and the metres of a world unit there.
    fn origin() -> (f64, f64, f64) {
        let x0 = (8.4 + 180.0) / 360.0;
        let y0 =
            0.5 - (std::f64::consts::FRAC_PI_4 + 46.8f64.to_radians() / 2.0).tan().ln() / (2.0 * std::f64::consts::PI);
        (x0, y0, metres(y0))
    }

    /// The world point `east` and `south` metres from the origin.
    fn at(east: f64, south: f64) -> [f64; 2] {
        let (x0, y0, scale) = origin();
        [x0 + east / scale, y0 + south / scale]
    }

    /// f32 heights from a function of metres east and south of the origin, as the terrain has them.
    fn terrain(height: impl Fn(f64, f64) -> f64 + Sync) -> impl Fn(f64, f64) -> f64 + Sync {
        let (x0, y0, scale) = origin();
        move |x, y| height((x - x0) * scale, (y - y0) * scale) as f32 as f64
    }

    fn feature(kind: Kind, points: Vec<[f64; 2]>) -> Feature {
        Feature {
            kind,
            level: Level::Natural,
            half: 1.0,
            ele: f64::NAN,
            parts: vec![Part { outer: kind == Kind::Lake, points }],
        }
    }

    fn river(points: Vec<[f64; 2]>) -> Feature {
        Feature { half: 5.0, ..feature(Kind::River, points) }
    }

    /// The levels of all rivers and lakes of `features` in one solve.
    fn solve(features: &mut [Feature], terrain: &dyn Terrain) -> (Heights, Metrics) {
        let group: Vec<usize> = (0..features.len()).filter(|&i| features[i].kind != Kind::Mark).collect();
        let solved = solve_group(features, &group, &Marks::new(features), terrain);
        let mut heights: Heights = features.iter().map(|f| vec![None; f.parts.len()]).collect();
        for (index, z) in solved.heights {
            heights[index] = z;
        }
        for &(f, p) in &solved.turned {
            features[f].parts[p].points.reverse();
        }
        (heights, solved.metrics)
    }

    fn levels(heights: &Heights, feature: usize) -> Vec<f64> {
        heights[feature][0].clone().unwrap()
    }

    fn falls(z: &[f64]) -> bool {
        z.windows(2).all(|w| w[1] <= w[0] + 1e-9)
    }

    /// Noise of about `sigma` metres, the same in each run.
    fn noise(seed: u64, sigma: f64) -> Vec<f64> {
        let mut state = seed;
        (0..4000)
            .map(|_| {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ((state >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 3.4 * sigma
            })
            .collect()
    }

    #[test]
    fn a_river_in_a_noisy_v_valley_falls_with_the_valley_and_stays_at_its_bottom() {
        let noise = noise(1, 0.5);
        let valley =
            terrain(move |e, s| 1000.0 - 0.05 * e + 0.3 * s.abs() + noise[((e / 0.5) as i64).clamp(0, 3999) as usize]);
        let points: Vec<[f64; 2]> =
            (0..=1800).step_by(60).map(|e| at(e as f64, 3.0 * (e as f64 / 40.0).sin())).collect();
        let mut features = vec![river(points)];
        let z = levels(&solve(&mut features, &valley).0, 0);
        assert!(falls(&z));
        let bottom = (0..=1800).step_by(60).map(|e| 1000.0 - 0.05 * e as f64);
        assert!(z.iter().zip(bottom).all(|(a, b)| (a - b).abs() < 2.0));
    }

    #[test]
    fn a_way_drawn_uphill_turns_round() {
        let slope = terrain(|e, _| 1000.0 + 0.02 * e);
        let mut features =
            vec![Feature { half: 1.0, ..river((0..=1000).step_by(100).map(|e| at(e as f64, 0.0)).collect()) }];
        let (heights, turned, metrics) = solve_by_network(&mut features, &slope);
        assert_eq!(features[0].parts[0].points[0], at(1000.0, 0.0), "the way runs downstream now");
        assert!(falls(&levels(&heights, 0)));
        assert_eq!((turned, metrics.turned), (vec![(0, 0)], 1));
    }

    #[test]
    fn a_confluence_has_one_level_and_nothing_rises_through_it() {
        // Two streams from the north-west and the south-west meet at the origin; the river goes on east.
        let plain = terrain(|e, s| 900.0 - 0.01 * e + 0.02 * s.abs());
        let mut features = vec![
            Feature { half: 1.0, ..river(vec![at(-400.0, -300.0), at(-200.0, -150.0), at(0.0, 0.0)]) },
            Feature { half: 1.0, ..river(vec![at(-400.0, 300.0), at(-200.0, 150.0), at(0.0, 0.0)]) },
            river(vec![at(0.0, 0.0), at(300.0, 0.0), at(600.0, 0.0)]),
        ];
        let heights = solve(&mut features, &plain).0;
        let (a, b, c) = (levels(&heights, 0), levels(&heights, 1), levels(&heights, 2));
        assert!(falls(&a) && falls(&b) && falls(&c));
        assert_eq!((a[2], b[2]), (c[0], c[0]));
    }

    #[test]
    fn a_branch_off_a_way_that_runs_on_does_not_rise() {
        // A river runs east through the origin and drops 3 m 15 m past it, so its smoothing lowers its
        // level at the origin after the reach before it; a canal branches off north there on the flat.
        let ground = terrain(|e, s| if e > 15.0 { 900.0 - 0.01 * e - 3.0 } else { 900.0 - 0.01 * e - 0.002 * s.abs() });
        let mut features = vec![
            river([-400.0, -200.0, 0.0, 200.0, 400.0].map(|e| at(e, 0.0)).to_vec()),
            river(vec![at(0.0, 0.0), at(0.0, -60.0), at(0.0, -120.0)]),
        ];
        let heights = solve(&mut features, &ground).0;
        let (r, c) = (levels(&heights, 0), levels(&heights, 1));
        assert!(falls(&r) && falls(&c));
        assert_eq!(c[0], r[2]);
    }

    #[test]
    fn a_loop_of_channels_keeps_its_fall_and_nothing_rises_through_it_in_any_order() {
        // A loop P → Q → P around an island, with Q its low point: "back" is drawn up a slope too gentle
        // to turn, and so is an outflow from Q.
        let ground = terrain(|e, _| 1000.0 - 0.01 * e.min(400.0) + 0.01 * (e - 400.0).max(0.0));
        let ways = [
            ("there", vec![at(0.0, 0.0), at(200.0, 50.0), at(400.0, 0.0)]),
            ("back", vec![at(400.0, 0.0), at(200.0, -50.0), at(0.0, 0.0)]),
            ("out", vec![at(400.0, 0.0), at(600.0, 0.0), at(800.0, 0.0)]),
        ];
        let mut solved = Vec::new();
        for order in [[2, 0, 1], [0, 1, 2]] {
            let mut features: Vec<Feature> = order.iter().map(|&i| river(ways[i].1.clone())).collect();
            let heights = solve(&mut features, &ground).0;
            let mut z: Vec<(&str, Vec<f64>)> =
                order.iter().enumerate().map(|(n, &i)| (ways[i].0, levels(&heights, n))).collect();
            z.sort_by_key(|(name, _)| *name);
            solved.push(z);
        }
        assert_eq!(solved[0], solved[1]);
        let z: HashMap<&str, Vec<f64>> = solved.remove(0).into_iter().collect();
        assert!(z.values().all(|v| falls(v)));
        // The channel down the valley keeps its fall; the one drawn back up it stays under Q.
        assert!(z["there"][0] > z["there"][2] + 3.0);
        assert_eq!((z["back"][0], z["out"][0]), (z["there"][2], z["there"][2]));
    }

    #[test]
    fn a_fall_keeps_its_step_and_the_pools_above_and_below_stay_flat() {
        // Two flat pools, 10 m apart, with a fall between them at 500 m.
        let step = terrain(|e, _| if e < 500.0 { 1010.0 } else { 1000.0 });
        let points: Vec<[f64; 2]> = (0..=1000).step_by(50).map(|e| at(e as f64, 0.0)).collect();
        let mut features = vec![river(points), feature(Kind::Mark, vec![at(500.0, 0.0)])];
        let z = levels(&solve(&mut features, &step).0, 0);
        assert!(falls(&z));
        assert!(z[9] > 1009.5, "at 450 m");
        assert!(z[11] < 1000.5, "at 550 m");
    }

    fn square() -> Vec<[f64; 2]> {
        vec![at(0.0, 0.0), at(400.0, 0.0), at(400.0, 400.0), at(0.0, 400.0)]
    }

    fn lake_level_of(terrain: &dyn Terrain, level: Level, ele: f64) -> f64 {
        let mut features = vec![Feature { level, ele, ..feature(Kind::Lake, square()) }];
        levels(&solve(&mut features, terrain).0, 0)[0]
    }

    #[test]
    fn the_interior_of_a_lake_leaves_out_its_island() {
        // The island's ring is closed: its last vertex repeats the first.
        let island = vec![at(40.0, 40.0), at(360.0, 40.0), at(360.0, 360.0), at(40.0, 360.0), at(40.0, 40.0)];
        let ground = terrain(|e, s| if e > 40.0 && e < 360.0 && s > 40.0 && s < 360.0 { 760.0 } else { 700.0 });
        let parts = [Part { outer: true, points: square() }, Part { outer: false, points: island }];
        let h = ground.heights(&interior(&parts, ground.top()));
        assert!(!h.is_empty());
        assert!(h.iter().all(|&v| v == 700.0));
    }

    #[test]
    fn a_sloped_lake_takes_a_low_quantile_of_its_shore_and_an_ele_tag_near_it() {
        // The terrain inside a lake that a DTM did not flatten: it falls 2 % to the south-east.
        let sloped = terrain(|e, s| 708.0 - 0.01 * (e + s));
        let level = lake_level_of(&sloped, Level::Natural, f64::NAN);
        assert!(level > 700.0 && level < 702.0);
        let tagged: f64 = format!("{:.1}", level + 1.0).parse().unwrap();
        assert_eq!(lake_level_of(&sloped, Level::Natural, tagged), tagged);
        assert_eq!(lake_level_of(&sloped, Level::Natural, 690.0), level);
        assert!(lake_level_of(&sloped, Level::Pond, f64::NAN) < level);
    }

    #[test]
    fn a_dam_reservoir_takes_the_terrain_at_its_outline_and_its_outflow_stays_below_it() {
        // A reservoir held by a dam at its east side; the terrain inside has dropped with the water.
        let valley = terrain(|e, s| {
            let full = if e > 5.0 && e < 395.0 && s > 5.0 && s < 395.0 { 1850.0 } else { 1900.0 };
            full - if e >= 400.0 { 0.1 * (e - 400.0) } else { 0.0 }
        });
        let mut features = vec![
            Feature { level: Level::Reservoir, ..feature(Kind::Lake, square()) },
            feature(Kind::Mark, vec![at(400.0, 150.0), at(400.0, 250.0)]),
            river(vec![at(400.0, 200.0), at(600.0, 200.0), at(800.0, 200.0)]),
        ];
        let (heights, metrics) = solve(&mut features, &valley);
        let (level, outflow) = (levels(&heights, 0)[0], levels(&heights, 2));
        assert!(level > 1890.0, "the full level, at the outline");
        assert!(falls(&outflow));
        assert!(outflow[0] <= level);
        assert_eq!(metrics.lakes, 1);
    }

    #[test]
    fn the_networks_and_the_lakes_solve_apart_to_the_same_levels() {
        let noise = noise(2, 0.3);
        let slope = terrain(move |e, s| {
            900.0 - 0.02 * e + 0.01 * (s - 200.0).abs() + noise[((e.abs() / 0.5) as usize).min(3999)]
        });
        let square =
            |e: f64, s: f64, size: f64| vec![at(e, s), at(e + size, s), at(e + size, s + size), at(e, s + size)];
        let stream = |points: Vec<[f64; 2]>| Feature { half: 1.0, ..river(points) };
        let features = vec![
            // A confluence whose river runs through a lake, with a fall below it.
            stream(vec![at(-400.0, -300.0), at(-200.0, -150.0), at(0.0, 0.0)]),
            stream(vec![at(-400.0, 300.0), at(-200.0, 150.0), at(0.0, 0.0)]),
            river(vec![at(0.0, 0.0), at(300.0, 0.0), at(600.0, 0.0), at(900.0, 0.0)]),
            feature(Kind::Lake, square(350.0, -50.0, 100.0)),
            feature(Kind::Mark, vec![at(750.0, 0.0)]),
            // A stream drawn uphill, with a lake that does not touch it but lies near it.
            stream(vec![at(1600.0, 400.0), at(1300.0, 400.0), at(1000.0, 400.0)]),
            feature(Kind::Lake, square(1200.0, 440.0, 80.0)),
            feature(Kind::Lake, square(0.0, 1000.0, 200.0)),
        ];
        let mut found = groups(&features);
        found.sort();
        assert_eq!(found, vec![vec![0, 1, 2, 3], vec![5, 6], vec![7]]);
        let (mut whole, mut apart) = (features.clone(), features.clone());
        let (heights, metrics) = solve(&mut whole, &slope);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
        let (by_network, turned, metrics_by_network) = pool.install(|| solve_by_network(&mut apart, &slope));
        assert_eq!(format!("{heights:?}"), format!("{by_network:?}"));
        assert_eq!((metrics, turned), (metrics_by_network, vec![(5, 0)]));
        let points = |f: &[Feature]| f.iter().map(|f| f.parts.clone()).collect::<Vec<_>>();
        assert_eq!(points(&whole), points(&apart), "a way turns round in the same place");
        assert_ne!(whole[5].parts, features[5].parts);
    }
}
