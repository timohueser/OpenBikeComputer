//! The water levels: each lake at one level, and the rivers and streams as networks whose levels
//! never rise downstream and lie in a channel under their banks. `specs/trailview-features.md` says
//! what the levels promise.

use std::collections::HashMap;

use obc_ground_plan::graph::{water_grades, water_level, water_spans, water_tangents, WaterCurve};
use rayon::prelude::*;

use crate::geometry::{arange, area, bounds, key, percentile, Boxes};
use crate::{metres, Part, Terrain};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A river or a stream.
    River,
    Lake,
    /// A waterfall, a dam or a dam point: it has no level.
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
    /// The half-width of a waterway in metres, and its bank top: `freeboard` metres over its level, `bank`
    /// metres past its waterline.
    pub half: f64,
    pub bank: f64,
    pub freeboard: f64,
    /// The reader's curve of its spans, on which the vertices that its levels add lie.
    pub curve: Curve,
    /// The `ele` tag, NaN without one.
    pub ele: f64,
    pub parts: Vec<Part>,
}

/// The rules of the curve that the ground plan draws through the vertices of a waterway
/// (`obc_ground_plan::graph::water_spans`): per span a cubic Hermite, or the chord where a vertex turns by
/// more than `turn_max_deg`, where the curve leaves the chord by more than `off_max_m`, or where it folds
/// back.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Curve {
    pub turn_max_deg: f64,
    pub off_max_m: f64,
}

/// Per feature, per part: the heights of its points (NaN where unknown), or None.
pub type Heights = Vec<Vec<Option<Vec<f64>>>>;

/// A river part's points in the direction of its flow with the vertices that its levels add (`kept`),
/// and their levels.
pub type Line = (Vec<[f64; 2]>, Vec<f64>);

/// Per feature, per part: its `Line` for a river, None for every other part.
pub type Kept = Vec<Vec<Option<Line>>>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub lakes: usize,
    pub lakes_spilling: usize,
    pub river_parts: usize,
    pub turned: usize,
    /// The vertices that the levels add to the rivers and streams (`kept`).
    pub added: usize,
}

impl std::ops::AddAssign for Metrics {
    fn add_assign(&mut self, other: Self) {
        self.lakes += other.lakes;
        self.lakes_spilling += other.lakes_spilling;
        self.river_parts += other.river_parts;
        self.turned += other.turned;
        self.added += other.added;
    }
}

/// A river observes the terrain every STEP_M along it, at its lowest within ±(w/2 + BESIDE_M)
/// across it, at the ACROSS points.
const STEP_M: f64 = 5.0;
/// A waterway keeps a sample as a vertex where the level that the ground plan draws between the vertices
/// around it leaves the sample's level by more than this: half the least freeboard (a stream's 0.4 m), so
/// the water between two vertices stays under its banks.
const LEVEL_TOL_M: f64 = 0.2;
const BESIDE_M: f64 = 2.0;
const ACROSS: [f64; 5] = [-1.0, -0.5, 0.0, 0.5, 1.0];
/// A way turns round when its terrain rises by more than this from end to end: OSM draws it uphill.
const UPHILL_M: f64 = 5.0;
/// A sample in a lake, or this near its shore, holds the lake's level with the weight PIN.
const SHORE_M: f64 = 2.0;
const PIN: f64 = 1000.0;
/// Lakes: the median of the interior holds where FLAT_SHARE of it lies within FLAT_M of it; else a
/// shore quantile, INSET_M inside. An `ele` tag within ELE_M of the level wins.
const FLAT_M: f64 = 0.3;
const FLAT_SHARE: f64 = 0.8;
const INSET_M: f64 = 2.0;
const SHORE_QUANTILE: f64 = 15.0;
const ELE_M: f64 = 1.5;
/// A lake spills where its shore, away from its rivers, lies this far below its level.
const SPILL_M: f64 = 0.5;
const RIVER_NEAR_M: f64 = 30.0;
/// A lake reads the terrain at no more than this many points, and at most at the pixels of the
/// deepest zoom of the terrain.
const LAKE_POINTS: f64 = 250_000.0;
const TILE: f64 = 512.0;

type Key = [u64; 2];

/// The levels of `features`, each network apart on the threads of the current rayon pool: at the
/// vertices of the map, and along the rivers with the vertices that their levels add (`Kept`). A river
/// part that OSM draws uphill turns round in place; `turned` lists them as (feature, part).
pub fn solve_by_network(
    features: &mut [Feature],
    terrain: &dyn Terrain,
) -> (Heights, Kept, Vec<(usize, usize)>, Metrics) {
    let mut found = groups(features);
    let size =
        |group: &Vec<usize>| -> usize { group.iter().flat_map(|&i| &features[i].parts).map(|p| p.points.len()).sum() };
    found.sort_by_key(|group| std::cmp::Reverse(size(group)));
    let shared: &[Feature] = features;
    let solved: Vec<Solved> =
        found.par_iter().with_max_len(1).map(|group| solve_group(shared, group, terrain)).collect();
    let mut heights: Heights = features.iter().map(|f| vec![None; f.parts.len()]).collect();
    let mut kept: Kept = features.iter().map(|f| vec![None; f.parts.len()]).collect();
    let (mut turned, mut metrics) = (Vec::new(), Metrics::default());
    for group in solved {
        for (index, z) in group.heights {
            heights[index] = z;
        }
        for ((f, p), points) in group.kept {
            kept[f][p] = Some(points);
        }
        turned.extend(group.turned);
        metrics += group.metrics;
    }
    turned.sort_unstable();
    for &(f, p) in &turned {
        features[f].parts[p].points.reverse();
    }
    (heights, kept, turned, metrics)
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
                boxes.push(bounds(&part.points));
                owners.push(index);
            }
        }
    }
    let boxes = Boxes::new(&boxes);
    for (index, feature) in features.iter().enumerate().filter(|(_, f)| f.kind == Kind::Lake) {
        if feature.parts.iter().all(|p| p.points.is_empty()) {
            continue;
        }
        let b = bounds(feature.parts.iter().flat_map(|p| &p.points));
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
    kept: Vec<((usize, usize), Line)>,
    turned: Vec<(usize, usize)>,
    metrics: Metrics,
}

struct Lake<'a> {
    rings: Vec<&'a [[f64; 2]]>,
    bounds: [f64; 4],
    level: f64,
}

fn solve_group(features: &[Feature], group: &[usize], terrain: &dyn Terrain) -> Solved {
    let lakes: Vec<(usize, Lake)> = group
        .par_iter()
        .filter(|&&i| features[i].kind == Kind::Lake)
        .map(|&index| {
            let feature = &features[index];
            let rings: Vec<&[[f64; 2]]> = feature.parts.iter().map(|p| p.points.as_slice()).collect();
            let level = lake_level(feature, terrain);
            (index, Lake { bounds: bounds(rings.iter().copied().flatten()), rings, level })
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
            let mut way = Way::new(part.points.clone(), feature, terrain);
            let uphill = way.uphill();
            if uphill {
                let mut points = part.points.clone();
                points.reverse();
                way = Way::new(points, feature, terrain);
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
    let levels = network(&ways);
    let mut heights: HashMap<usize, Vec<Option<Vec<f64>>>> =
        group.iter().map(|&i| (i, vec![None; features[i].parts.len()])).collect();
    for (index, lake) in &lakes {
        let level = if lake.level.is_finite() { lake.level } else { f64::NAN };
        heights.insert(*index, features[*index].parts.iter().map(|p| Some(vec![level; p.points.len()])).collect());
    }
    let (mut kept_parts, mut added) = (Vec::with_capacity(ways.len()), 0);
    for (way, z) in ways.iter().zip(&levels) {
        heights.get_mut(&way.feature).unwrap()[way.part] = Some(way.vertices.iter().map(|&v| z[v]).collect());
        let curves = curves(&way.points, features[way.feature].curve);
        let samples = kept(way, z, &curves);
        added += samples.len() - way.vertices.len();
        let points = samples.iter().map(|&i| way.point(i, &curves)).collect();
        kept_parts.push(((way.feature, way.part), (points, samples.iter().map(|&i| z[i]).collect())));
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
        Metrics { lakes: lakes.len(), lakes_spilling: spilling, river_parts: ways.len(), turned: turned.len(), added };
    let mut heights: Vec<_> = heights.into_iter().collect();
    heights.sort_unstable_by_key(|(i, _)| *i);
    Solved { heights, kept: kept_parts, turned, metrics }
}

/// Metres per world unit along the chord from a to b.
fn chord_metres(a: [f64; 2], b: [f64; 2]) -> f64 {
    metres((a[1] + b[1]) / 2.0)
}

/// Per span of `points` the curve that the ground plan draws (`water_spans`), from the span's start.
fn curves(points: &[[f64; 2]], curve: Curve) -> Vec<WaterCurve> {
    let tangents = water_tangents(points, curve.turn_max_deg);
    let shapes = water_spans(points, &tangents, chord_metres, curve.off_max_m);
    let chord = |w: &[[f64; 2]]| [w[1][0] - w[0][0], w[1][1] - w[0][1]];
    points.windows(2).zip(shapes).map(|(w, shape)| WaterCurve::new(chord(w), shape)).collect()
}

/// The samples of a way that it keeps as vertices: its own vertices, and the samples that the level drawn
/// between the kept ones (`water_level` with `water_grades`, at the sample's share of its span) leaves by
/// more than LEVEL_TOL_M: in each round the farthest of each such span, until none is left. So the levels
/// hold along the line, not only at the vertices of the map. A kept vertex changes the grades of its
/// neighbours, so the rounds test every span again.
fn kept(way: &Way, z: &[f64], curves: &[WaterCurve]) -> Vec<usize> {
    let arc_m: Vec<f64> =
        curves.iter().zip(way.points.windows(2)).map(|(c, w)| c.length() * chord_metres(w[0], w[1])).collect();
    let level = |i: usize| z[i].is_finite().then_some(z[i]);
    let mut out = way.vertices.clone();
    loop {
        let points: Vec<[f64; 2]> = out.iter().map(|&i| way.point(i, curves)).collect();
        let grades = water_grades(&points, &out.iter().map(|&i| level(i)).collect::<Vec<_>>(), chord_metres);
        let mut more = Vec::new();
        for (j, pair) in out.windows(2).enumerate() {
            let (a, b) = (pair[0], pair[1]);
            let (sa, sb) = (way.along[a], way.along[b]);
            if b <= a + 1 || !(z[a].is_finite() && z[b].is_finite()) || sb <= sa {
                continue;
            }
            let k = way.span_of(a);
            let len = (sb - sa) / (way.along[way.vertices[k + 1]] - way.along[way.vertices[k]]) * arc_m[k];
            let drawn = |i: usize| {
                water_level([z[a], z[b]], [grades[j], grades[j + 1]], len, (way.along[i] - sa) / (sb - sa)).0
            };
            let worst = (a + 1..b)
                .filter(|&i| z[i].is_finite())
                .map(|i| (i, (z[i] - drawn(i)).abs()))
                .max_by(|x, y| x.1.total_cmp(&y.1));
            more.extend(worst.filter(|w| w.1 > LEVEL_TOL_M).map(|w| w.0));
        }
        if more.is_empty() {
            return out;
        }
        out.extend(more);
        out.sort_unstable();
    }
}

/// A river part: its points (world coordinates, y down) in the direction of its flow, the samples
/// along it every STEP_M with the lowest terrain across it, their weights, the most level that keeps
/// the freeboard under the lower bank top (NaN where the terrain is unknown), and how far its bank
/// top lies from its middle.
struct Way {
    points: Vec<[f64; 2]>,
    xy: Vec<[f64; 2]>,
    along: Vec<f64>,
    /// The sample of each point.
    vertices: Vec<usize>,
    bounds: [f64; 4],
    low: Vec<f64>,
    weight: Vec<f64>,
    channel: Vec<f64>,
    banks: f64,
    feature: usize,
    part: usize,
}

impl Way {
    fn new(points: Vec<[f64; 2]>, feature: &Feature, terrain: &dyn Terrain) -> Self {
        let half = feature.half;
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
        let beside = |o: f64| -> Vec<f64> {
            let across: Vec<[f64; 2]> = xy
                .iter()
                .zip(&sample)
                .map(|(p, &(k, _))| {
                    let s = o / metres(p[1]);
                    [p[0] + normal[k][0] * s, p[1] + normal[k][1] * s]
                })
                .collect();
            terrain.heights(&across)
        };
        let lowest = |offsets: &[f64]| {
            let mut low = vec![f64::NAN; xy.len()];
            for &o in offsets {
                for (low, h) in low.iter_mut().zip(beside(o)) {
                    if h < *low || low.is_nan() {
                        *low = h;
                    }
                }
            }
            low
        };
        let low = lowest(&ACROSS.map(|o| o * (half + BESIDE_M)));
        let banks = half + feature.bank;
        let channel = lowest(&[-banks, banks]).iter().map(|h| h - feature.freeboard).collect();
        let weight = low.iter().map(|h| if h.is_finite() { 1.0 } else { 0.0 }).collect();
        let bounds = bounds(&xy);
        Self { points, xy, along, vertices, bounds, low, weight, channel, banks, feature: 0, part: 0 }
    }

    /// The span of the map that sample i starts or lies in.
    fn span_of(&self, i: usize) -> usize {
        self.vertices.partition_point(|&v| v <= i) - 1
    }

    /// The point of sample i: a vertex of the map keeps its own point, which other features share. Another
    /// sample lies on the curve of its span (`curves`) at its share of the span's length: on the chord, it
    /// would pull a curved span to its chord.
    fn point(&self, i: usize, curves: &[WaterCurve]) -> [f64; 2] {
        if let Ok(j) = self.vertices.binary_search(&i) {
            return self.points[j];
        }
        let k = self.span_of(i);
        let (a, b) = (self.along[self.vertices[k]], self.along[self.vertices[k + 1]]);
        let q = curves[k].at_share((self.along[i] - a) / (b - a));
        [self.points[k][0] + q[0], self.points[k][1] + q[1]]
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

/// A sample in a lake, or within SHORE_M of its shore, holds the lake's level with the weight PIN,
/// but not the samples that a way starts with there: water that flows out of a lake, over a dam or a
/// sill, takes its levels from its own fit and channel. A way wholly in a lake keeps them all.
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
        let mut held = vec![None; way.xy.len()];
        for k in boxes.overlapping(way.bounds) {
            let (b, level, edges) = &lakes[k];
            for (&[x, y], held) in way.xy.iter().zip(held.iter_mut()) {
                if !(x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]) {
                    continue;
                }
                if edges.contains(x, y) || edges.distance(x, y, SHORE_M / metres(y)) * metres(y) < SHORE_M {
                    *held = Some(*level);
                }
            }
        }
        let start = held.iter().position(Option::is_none).unwrap_or(0);
        for (s, level) in held.into_iter().enumerate().skip(start) {
            if let Some(level) = level {
                way.low[s] = level;
                way.weight[s] = PIN;
            }
        }
    });
}

/// The levels of every sample of the ways. The ways split into reaches at the points that they
/// share; the reaches are solved downstream in turn, each held under the levels that flow into its
/// start, and all reaches at a junction then take one level there. A loop of reaches has no order of
/// flow: its reaches go from the highest start down (a spanning DAG), and a reach that flows back
/// into a junction solved before it keeps its own end there, a step down to it.
fn network(ways: &[Way]) -> Vec<Vec<f64>> {
    // Per point, how many ways have it, and the farthest bank top of those ways from it.
    let mut counts: HashMap<Key, usize> = HashMap::new();
    let mut banks: HashMap<Key, f64> = HashMap::new();
    for way in ways {
        for &point in &way.points {
            *counts.entry(key(point)).or_default() += 1;
            let far = banks.entry(key(point)).or_insert(0.0);
            *far = far.max(way.banks);
        }
    }
    let skip = |point: Key| if counts[&point] > 1 { banks[&point] } else { 0.0 };
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
            let z = reach_levels(&ways[w], (a, b), cap, (skip(first), skip(end)));
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

/// The levels of the samples a..=b of a way: the highest levels that never rise downstream, lie under
/// the cap, and lie at each sample under its low and its channel, so that the water lies in a channel
/// under its banks at its own line. Within `skip` metres of a junction at its (start, end), the bank
/// tops lie in the other waterway's bed and do not count, nor do they at a sample in a lake. A sample
/// with no known terrain takes the level before it, or after it at the start. A reach never goes
/// under a lake that it flows into.
fn reach_levels(way: &Way, (a, b): (usize, usize), cap: f64, (start, end): (f64, f64)) -> Vec<f64> {
    let (low, channel, weight, along) = (&way.low[a..=b], &way.channel[a..=b], &way.weight[a..=b], &way.along[a..=b]);
    let total = along[along.len() - 1] - along[0];
    let banks = |i: usize| {
        let s = along[i] - along[0];
        if s < start || total - s < end || weight[i] >= PIN {
            f64::NAN
        } else {
            channel[i]
        }
    };
    let under: Vec<f64> = (0..low.len()).map(|i| low[i].min(banks(i))).collect();
    let Some(first) = under.iter().copied().find(|v| v.is_finite()) else { return vec![f64::NAN; low.len()] };
    // The level of a lake that the reach flows into: a pinned sample after a free one. A reach that
    // flows out of a lake starts pinned and takes no such floor.
    let free = weight.iter().position(|&w| w < PIN).unwrap_or(under.len());
    let mut lake = vec![f64::NEG_INFINITY; under.len()];
    for i in (0..under.len()).rev() {
        let below = lake.get(i + 1).copied().unwrap_or(f64::NEG_INFINITY);
        lake[i] = if weight[i] >= PIN && i > free { below.max(low[i]) } else { below };
    }
    let mut least = cap.min(first);
    (0..under.len())
        .map(|i| {
            if under[i].is_finite() {
                least = least.min(under[i]);
            }
            least.max(lake[i])
        })
        .collect()
}

/// The level of a lake: the median of its interior where it is flat; else the SHORE_QUANTILE of its
/// shore, INSET_M inside. A pond takes its lowest shore, a reservoir its shore at the outline, the
/// full level. An `ele` tag within ELE_M wins. NaN where the terrain knows none of it.
fn lake_level(lake: &Feature, terrain: &dyn Terrain) -> f64 {
    let rings: Vec<&[[f64; 2]]> = lake.parts.iter().map(|p| p.points.as_slice()).collect();
    let finite = |h: Vec<f64>| -> Vec<f64> { h.into_iter().filter(|v| v.is_finite()).collect() };
    let median = |mut h: Vec<f64>| percentile(&mut h, 50.0).unwrap_or(f64::NAN);
    let mut shore = finite(terrain.heights(&ring_points(&rings, STEP_M, INSET_M)));
    let level = match lake.level {
        Level::Pond => shore.iter().copied().reduce(f64::min).unwrap_or(f64::NAN),
        Level::Reservoir => median(finite(terrain.heights(&ring_points(&rings, STEP_M, 0.0)))),
        Level::Natural => {
            let h = finite(terrain.heights(&interior(&lake.parts, terrain.top())));
            let middle = median(h.clone());
            let flat = h.iter().filter(|&&v| (v - middle).abs() <= FLAT_M).count();
            if !h.is_empty() && flat as f64 / h.len() as f64 >= FLAT_SHARE {
                middle
            } else {
                percentile(&mut shore, SHORE_QUANTILE).unwrap_or(middle)
            }
        }
    };
    if (lake.ele - level).abs() <= ELE_M {
        lake.ele
    } else {
        level
    }
}

/// The points of a grid inside the rings of the parts, at the pixels of the deepest zoom or
/// LAKE_POINTS points at most, row by row.
fn interior(parts: &[Part], top: u32) -> Vec<[f64; 2]> {
    let rings: Vec<&[[f64; 2]]> = parts.iter().map(|p| p.points.as_slice()).collect();
    let [x0, y0, x1, y1] = bounds(rings.iter().copied().flatten());
    let area = area(parts);
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

/// Whether a lake's shore lies below its level by more than SPILL_M, INSET_M outside it and away
/// from its rivers.
fn spills(level: f64, rings: &[&[[f64; 2]]], terrain: &dyn Terrain, rivers: &[&Way]) -> bool {
    let outside = ring_points(rings, STEP_M, -INSET_M);
    if outside.is_empty() {
        return false;
    }
    let below = level - SPILL_M;
    let watched: Vec<[f64; 2]> = rivers
        .iter()
        .flat_map(|way| {
            let step = (way.points.len() / 64).max(1);
            way.points.iter().step_by(step).chain(way.points.last()).copied()
        })
        .collect();
    terrain
        .heights(&outside)
        .iter()
        .zip(&outside)
        .any(|(&h, p)| h < below && watched.iter().all(|&[x, y]| (p[0] - x).hypot(p[1] - y) * metres(y) > RIVER_NEAR_M))
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
            bank: 0.0,
            freeboard: 0.0,
            curve: Curve { turn_max_deg: 60.0, off_max_m: 2.0 },
            ele: f64::NAN,
            parts: vec![Part { outer: kind == Kind::Lake, points, z: None, plan: None }],
        }
    }

    fn river(points: Vec<[f64; 2]>) -> Feature {
        Feature { half: 5.0, ..feature(Kind::River, points) }
    }

    /// The levels of all rivers and lakes of `features` in one solve.
    fn solve(features: &mut [Feature], terrain: &dyn Terrain) -> (Heights, Metrics) {
        let group: Vec<usize> = (0..features.len()).filter(|&i| features[i].kind != Kind::Mark).collect();
        let solved = solve_group(features, &group, terrain);
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

    /// The kept lines of the first part of each river in one solve.
    fn lines_of(features: &[Feature], terrain: &dyn Terrain) -> Vec<Line> {
        let group: Vec<usize> = (0..features.len()).filter(|&i| features[i].kind == Kind::River).collect();
        let mut kept = solve_group(features, &group, terrain).kept;
        kept.sort_unstable_by_key(|k| k.0);
        kept.into_iter().map(|(_, line)| line).collect()
    }

    /// The kept points (metres east of the origin) and levels of the first part of each river in one solve.
    fn kept_of(features: &[Feature], terrain: &dyn Terrain) -> Vec<(Vec<f64>, Vec<f64>)> {
        let (x0, _, scale) = origin();
        let lines = lines_of(features, terrain);
        lines.into_iter().map(|(p, z)| (p.iter().map(|p| (p[0] - x0) * scale).collect(), z)).collect()
    }

    /// The points of the curve through `points`, 64 per span, and its end.
    fn drawn(points: &[[f64; 2]]) -> Vec<[f64; 2]> {
        let spans = curves(points, Curve { turn_max_deg: 60.0, off_max_m: 2.0 });
        let at = |(c, p): (&WaterCurve, &[f64; 2])| -> Vec<[f64; 2]> {
            (0..64).map(|j| c.at_share(f64::from(j) / 64.0)).map(|q| [p[0] + q[0], p[1] + q[1]]).collect()
        };
        spans.iter().zip(points).flat_map(at).chain(points.last().copied()).collect()
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
    fn a_long_span_keeps_vertices_where_the_drawn_level_leaves_the_terrain_and_a_plane_keeps_none() {
        // A stream with two vertices 400 m apart: the meadow falls 40 m over its first 100 m, then 4 m. On flat
        // ground across it, its level is the terrain less its freeboard.
        let fall = |e: f64| if e < 100.0 { 1000.0 - 0.4 * e } else { 960.0 - 0.0133 * (e - 100.0) };
        let stream = Feature { half: 0.5, bank: 0.6, freeboard: 0.4, ..river(vec![at(0.0, 0.0), at(400.0, 0.0)]) };
        let (east, z) = kept_of(std::slice::from_ref(&stream), &terrain(move |e, _| fall(e))).remove(0);
        assert!(east.len() > 2 && east.len() < 10, "{east:?}");
        assert!(falls(&z));
        // The map's vertices keep their levels: the roads read those.
        let mut map = vec![stream.clone()];
        let at_map = levels(&solve(&mut map, &terrain(move |e, _| fall(e))).0, 0);
        assert_eq!([z[0], z[z.len() - 1]], [at_map[0], at_map[1]]);
        // Every 5 m the level that the ground plan draws between the kept vertices lies within the tolerance
        // of the level there.
        let points: Vec<[f64; 2]> = east.iter().map(|&e| [e, 0.0]).collect();
        let grades = water_grades(&points, &z.iter().map(|&z| Some(z)).collect::<Vec<_>>(), |_, _| 1.0);
        for e in (0..=400).step_by(5).map(f64::from) {
            let k = east.partition_point(|&x| x <= e).clamp(1, east.len() - 1);
            let len = east[k] - east[k - 1];
            let level = water_level([z[k - 1], z[k]], [grades[k - 1], grades[k]], len, (e - east[k - 1]) / len).0;
            assert!((level - (fall(e) - 0.4)).abs() <= LEVEL_TOL_M + 0.01, "{e} m: {level} over {}", fall(e));
        }
        let plane = kept_of(&[stream], &terrain(|e, _| 1000.0 - 0.1 * e)).remove(0);
        assert_eq!(plane.0.len(), 2);
    }

    #[test]
    fn the_added_vertices_lie_on_the_curve_that_the_reader_draws_through_the_map_s_vertices() {
        // A bend of 400 m and a corner of 90° over the meadow that falls steeply, then gently.
        let fall =
            |e: f64, s: f64| if e + s < 100.0 { 1000.0 - 0.4 * (e + s) } else { 960.0 - 0.0133 * (e + s - 100.0) };
        let map = vec![at(0.0, 0.0), at(200.0, 12.0), at(400.0, 0.0), at(400.0, 200.0)];
        let stream = Feature { half: 0.5, bank: 0.6, freeboard: 0.4, ..river(map.clone()) };
        let (kept, _) = lines_of(&[stream], &terrain(fall)).remove(0);
        assert!(kept.len() > map.len(), "{} vertices", kept.len());
        let (before, after) = (drawn(&map), drawn(&kept));
        let (_, _, scale) = origin();
        // The distance of each point of the new curve to the old one, a polyline of 64 chords per span.
        let to = |q: &[f64; 2], a: [f64; 2], b: [f64; 2]| {
            let (d, e) = ([b[0] - a[0], b[1] - a[1]], [q[0] - a[0], q[1] - a[1]]);
            let t = ((e[0] * d[0] + e[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(1e-30)).clamp(0.0, 1.0);
            (e[0] - t * d[0]).hypot(e[1] - t * d[1])
        };
        let off_of = |points: &[[f64; 2]]| {
            let near = |q: &[f64; 2]| before.windows(2).map(|w| to(q, w[0], w[1])).fold(f64::INFINITY, f64::min);
            points.iter().map(|q| near(q) * scale).fold(0.0, f64::max)
        };
        // The added vertices lie on the curve through the map's vertices.
        assert!(off_of(&kept) < 0.01, "a kept vertex lies {} m off the curve", off_of(&kept));
        let off = off_of(&after);
        // The reader takes the tangents at the kept vertices from their new, nearer neighbours. On a span that it
        // drew as its chord, a sub-span next to a vertex that is no corner can become a curve, and the curve
        // beside it then leaves that chord's direction: both move by up to `curve_off_max_m` (here the chord
        // after the bend's middle vertex).
        assert!(off < 2.0, "the curve moves {off} m");
    }

    #[test]
    fn a_way_drawn_uphill_turns_round() {
        let slope = terrain(|e, _| 1000.0 + 0.02 * e);
        let mut features =
            vec![Feature { half: 1.0, ..river((0..=1000).step_by(100).map(|e| at(e as f64, 0.0)).collect()) }];
        let (heights, _, turned, metrics) = solve_by_network(&mut features, &slope);
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

    #[test]
    fn a_meadow_stream_lies_in_a_channel_and_a_flat_after_a_steep_reach_does_not_lift_it() {
        // The terrain falls 10 % to 300 m, then 0.1 %; the stream runs in no channel of the terrain.
        let ground = terrain(|e, _| if e < 300.0 { 1030.0 - 0.1 * e } else { 1000.0 - 0.001 * (e - 300.0) });
        let points: Vec<[f64; 2]> = (0..=1000).step_by(50).map(|e| at(e as f64, 0.0)).collect();
        let stream = Feature { half: 0.5, bank: 0.6, freeboard: 0.4, ..river(points) };
        let z = levels(&solve(&mut [stream], &ground).0, 0);
        assert!(falls(&z));
        for (n, level) in z.iter().enumerate() {
            let e = 50.0 * n as f64;
            let banks = if e < 300.0 { 1030.0 - 0.1 * e } else { 1000.0 - 0.001 * (e - 300.0) };
            assert!(*level <= banks - 0.4 + 0.01, "{e} m: {level} over {banks}");
            assert!(*level > banks - 1.0, "{e} m: {level} sinks too far under {banks}");
        }
    }

    #[test]
    fn a_steep_stream_lies_its_freeboard_under_the_terrain_at_its_line_from_the_head_of_its_reach_on() {
        // A mountain stream that falls 50 %, then 80 %.
        let height = |e: f64| if e < 40.0 { 1176.0 - 0.5 * e } else { 1156.0 - 0.8 * (e - 40.0) };
        let ground = terrain(move |e, _| height(e));
        let points: Vec<[f64; 2]> = (0..=150).step_by(10).map(|e| at(e as f64, 0.0)).collect();
        let stream = Feature { half: 0.5, bank: 0.6, freeboard: 0.4, ..river(points) };
        let z = levels(&solve(&mut [stream], &ground).0, 0);
        assert!(falls(&z));
        for (n, level) in z.iter().enumerate() {
            let line = height(10.0 * n as f64);
            assert!((level - (line - 0.4)).abs() < 0.01, "{n}: {level} against {line}");
        }
    }

    #[test]
    fn a_stream_below_a_dam_takes_its_own_levels_and_not_the_reservoirs() {
        // A reservoir at 1900 m whose outline holds its dam; its outflow starts on the dam's crest and
        // falls 20 m down its face into a valley.
        let ground = terrain(|e, s| {
            if e < 404.0 {
                if e > 5.0 && s > 5.0 && s < 395.0 {
                    1850.0
                } else {
                    1900.0
                }
            } else {
                1880.0 - 0.05 * (e - 404.0)
            }
        });
        let outline = vec![at(0.0, 0.0), at(408.0, 0.0), at(408.0, 400.0), at(0.0, 400.0)];
        let mut features = vec![
            Feature { level: Level::Reservoir, ..feature(Kind::Lake, outline) },
            Feature {
                half: 0.5,
                bank: 0.6,
                freeboard: 0.4,
                ..river(vec![at(408.0, 200.0), at(600.0, 200.0), at(800.0, 200.0)])
            },
        ];
        let heights = solve(&mut features, &ground).0;
        let (level, z) = (levels(&heights, 0)[0], levels(&heights, 1));
        assert!(level > 1890.0);
        assert!(falls(&z) && z[0] < 1880.0, "{z:?}");
        assert!(z[2] <= 1880.0 - 0.05 * (800.0 - 404.0) - 0.4 + 0.01, "{z:?}");
    }

    #[test]
    fn a_stream_into_a_river_channel_does_not_cap_the_river() {
        // A river 10 m wide in a channel 2 m under a meadow that falls 0.1 % east; a stream from the
        // north ends on it, and its last bank tops lie in the river's channel.
        let ground = terrain(|e, s| 1000.0 - 0.001 * e - if s.abs() < 5.0 { 2.0 } else { 0.0 });
        let main =
            || Feature { bank: 2.0, freeboard: 0.8, ..river(vec![at(-400.0, 0.0), at(0.0, 0.0), at(600.0, 0.0)]) };
        let stream = Feature { half: 0.5, bank: 0.6, freeboard: 0.4, ..river(vec![at(0.0, -300.0), at(0.0, 0.0)]) };
        let alone = levels(&solve(&mut [main()], &ground).0, 0);
        let heights = solve(&mut [main(), stream], &ground).0;
        let (joined, stream) = (levels(&heights, 0), levels(&heights, 1));
        assert!(falls(&joined) && falls(&stream));
        assert_eq!(stream[1], joined[1]);
        for (a, b) in alone.iter().zip(&joined) {
            assert!((a - b).abs() < 0.05, "{alone:?} {joined:?}");
        }
    }

    #[test]
    fn a_stream_into_a_lake_keeps_the_lake_level_at_its_shore() {
        // A lake on flat ground at 1000 m; a stream comes down from the west onto its shore.
        let ground = terrain(|e, _| 1000.0 + 0.01 * (-e).max(0.0));
        let stream = Feature {
            half: 0.5,
            bank: 0.6,
            freeboard: 0.4,
            ..river(vec![at(-300.0, 200.0), at(-150.0, 200.0), at(0.0, 200.0), at(50.0, 200.0)])
        };
        let heights = solve(&mut [feature(Kind::Lake, square()), stream], &ground).0;
        let (lake, z) = (levels(&heights, 0)[0], levels(&heights, 1));
        assert_eq!(lake, 1000.0);
        assert!(falls(&z) && z.iter().all(|&v| v >= lake - 0.02), "{z:?}");
        assert!((z[3] - lake).abs() < 0.02 && z[1] < 1001.5 - 0.39, "{z:?}");
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
        let parts = [
            Part { outer: true, points: square(), z: None, plan: None },
            Part { outer: false, points: island, z: None, plan: None },
        ];
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
        let (by_network, _, turned, metrics_by_network) = pool.install(|| solve_by_network(&mut apart, &slope));
        assert_eq!(format!("{heights:?}"), format!("{by_network:?}"));
        assert_eq!((metrics, turned), (metrics_by_network, vec![(5, 0)]));
        let points = |f: &[Feature]| f.iter().map(|f| f.parts.clone()).collect::<Vec<_>>();
        assert_eq!(points(&whole), points(&apart), "a way turns round in the same place");
        assert_ne!(whole[5].parts, features[5].parts);
    }
}
