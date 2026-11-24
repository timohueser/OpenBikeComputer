//! The sidewalks of the roads: per side of each segment of a stroke, whether it has one.
//!
//! A side can have a sidewalk on a way on the ground whose class has a `sidewalk_m`. OSM's tags decide
//! first; a side without a tag has one where TOWN_BUILDINGS buildings stand within TOWN_M of the road. A
//! tight bend and the footprint of another way take it off. Only a tag, a class without a sidewalk and a tight
//! bend stop a sidewalk for good: a gap under GAP_M closes, a piece that ends within JUNCTION_M of a
//! junction runs on to it, and a piece that leaves a junction and stops within JUNCTION_M of it goes. Once
//! every stroke of a solve has its sides, a run on one side that is shorter than PIECE_M and stops at both
//! ends goes (`drop_short`).

use std::collections::{HashMap, HashSet};

use rstar::primitives::Rectangle;
use rstar::RTree;

use super::align::Curve;
use super::network::{dot, hypot, sub, Network};
use super::rows::{root, union, UntaggedCrossing};
use super::{Classes, River};
use crate::geometry::{key, Boxes};
use crate::kinds::SIDEWALK_MAX_M;
use crate::{metres, WALK_LEFT, WALK_RIGHT};

/// One building beside a road is a house in the country, a second one makes it a street. A building
/// counts by the box of its outer ring, both sides of the road alike.
const TOWN_BUILDINGS: usize = 2;
const TOWN_M: f64 = 25.0;
const GAP_M: f64 = 40.0;
const PIECE_M: f64 = 50.0;
const JUNCTION_M: f64 = 15.0;
/// Another way does not count as a footprint within this many metres of a point where it meets the road, as
/// JOIN_M of bins.ts: the corner shapes the mouth there.
const JOIN_M: f64 = 12.0;

/// Metres of sidewalk by the rule that decides them: kept by a tag or by buildings, and taken by a bend,
/// by another way's footprint and by a piece too short.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Walks {
    pub tagged_m: f64,
    pub built_m: f64,
    pub bend_m: f64,
    pub footprint_m: f64,
    pub short_m: f64,
}

impl Walks {
    pub fn add(&mut self, other: &Walks) {
        self.tagged_m += other.tagged_m;
        self.built_m += other.built_m;
        self.bend_m += other.bend_m;
        self.footprint_m += other.footprint_m;
        self.short_m += other.short_m;
    }
}

/// A way of a stroke: its samples from `first` to `last` in the order of the stroke, and whether it runs
/// with the stroke.
pub(crate) struct Piece {
    pub way: usize,
    pub forward: bool,
    pub first: usize,
    pub last: usize,
}

/// A footprint on the ground: a segment, its half-width, its way, None for a waterway, per end whether it is
/// a culvert's mouth there, where a waterway's channel ends, and the pipes of untagged culverts under it as
/// (point, metres along the water that the pipe runs each way).
struct Footprint {
    p: [f64; 2],
    q: [f64; 2],
    half: f64,
    way: Option<usize>,
    mouth: [bool; 2],
    pipes: Vec<([f64; 2], f64)>,
}

/// What the sidewalks of a solve read besides the strokes.
pub(crate) struct Town<'a> {
    buildings: Option<&'a RTree<Rectangle<[f64; 2]>>>,
    footprints: Vec<Footprint>,
    boxes: Boxes,
    /// Per way, the variables and points of its OSM vertices that other ways share.
    joins: Vec<Vec<(usize, [f64; 2])>>,
    /// Per variable of OSM: whether three segments of ways or more leave it, a junction.
    pub junction: Vec<bool>,
}

impl<'a> Town<'a> {
    /// The pipe of an untagged culvert runs under its road and the road's sidewalks: as far along the
    /// water from the crossing as the road's outer half-width, its widening and the channel's half-width
    /// reach at the angle of the crossing.
    pub fn new(
        net: &Network,
        (rivers, culverts): (&[&River], &[&River]),
        (untagged, classes): (&[UntaggedCrossing], &Classes),
        buildings: Option<&'a RTree<Rectangle<[f64; 2]>>>,
    ) -> Self {
        let mouths: HashSet<[u64; 2]> = culverts.iter().flat_map(|c| &c.points).map(|&p| key(p)).collect();
        let mut footprints: Vec<Footprint> = Vec::new();
        for (s, &w) in net.segment_way.iter().enumerate().filter(|&(_, &w)| !net.free[w]) {
            let v = net.segment_start[s];
            let (p, q) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
            footprints.push(Footprint { p, q, half: net.half[w], way: Some(w), mouth: [false; 2], pipes: Vec::new() });
        }
        for (r, river) in rivers.iter().enumerate() {
            let mouth = |point: &[f64; 2]| mouths.contains(&key(*point));
            let pipes: Vec<([f64; 2], f64)> = untagged
                .iter()
                .filter(|c| c.river == r)
                .map(|c| {
                    let reach = net.outer[c.way] + classes.rows[net.rule[c.way]].widen_m + river.half;
                    (c.point, reach / c.sin)
                })
                .collect();
            let channel = |p: &[[f64; 2]]| {
                let near = |&&(at, reach): &&([f64; 2], f64)| distance(at, p[0], p[1]) * metres(at[1]) < reach;
                Footprint {
                    p: p[0],
                    q: p[1],
                    half: river.half,
                    way: None,
                    mouth: [mouth(&p[0]), mouth(&p[1])],
                    pipes: pipes.iter().filter(near).copied().collect(),
                }
            };
            footprints.extend(river.points.windows(2).map(channel));
        }
        let boxes: Vec<[f64; 4]> = footprints
            .iter()
            .map(|&Footprint { p, q, half, .. }| {
                let r = (half + SIDEWALK_MAX_M / 2.0) / metres(p[1]);
                [p[0].min(q[0]) - r, p[1].min(q[1]) - r, p[0].max(q[0]) + r, p[1].max(q[1]) + r]
            })
            .collect();
        let mut joins = vec![Vec::new(); net.way_first.len()];
        let mut out = vec![0u8; net.osm_count];
        for (o, &var) in net.osm_var.iter().enumerate() {
            let w = net.osm_way[o];
            let end = net.osm_first[w] == o || net.osm_first.get(w + 1).map_or(net.osm_xy.len(), |&f| f) == o + 1;
            out[var] = out[var].saturating_add(if end { 1 } else { 2 });
            if net.junction[var] {
                joins[w].push((var, net.osm_xy[o]));
            }
        }
        let junction = out.iter().map(|&c| c >= 3).collect();
        Town { buildings, footprints, boxes: Boxes::new(&boxes), joins, junction }
    }

    /// Per sample of a stroke, the sides of the segment from it to the next sample that have a sidewalk:
    /// WALK_LEFT and WALK_RIGHT of the stroke, before `drop_short`. `junction` and `owned` are per sample: a
    /// junction of three segments or more, and a sample whose metres count in `walks`.
    #[allow(clippy::too_many_arguments)]
    pub fn sidewalks(
        &self,
        net: &Network,
        classes: &Classes,
        ground: &[bool],
        curve: &Curve,
        pieces: &[Piece],
        junction: &[bool],
        owned: &[bool],
        walks: &mut Walks,
    ) -> Vec<u8> {
        let n = curve.s.len();
        let mut out = vec![0; n];
        if n < 2 {
            return out;
        }
        let owner = owners(pieces, n);
        let width: Vec<f64> = owner
            .iter()
            .map(|&p| {
                let w = pieces[p].way;
                if ground[w] {
                    classes.rows[net.rule[w]].sidewalk_m
                } else {
                    0.0
                }
            })
            .collect();
        if width.iter().all(|&w| w == 0.0) {
            return out;
        }
        let way = |i: usize| pieces[owner[i]].way;
        let rule = |i: usize| &classes.rows[net.rule[way(i)]];
        let half = |i: usize| net.half[way(i)] + curve.widen[i];
        let metres_of = |i: usize| curve.s[i + 1] - curve.s[i];
        let count = |walks: &mut f64, i: usize| {
            if owned[i] {
                *walks += metres_of(i);
            }
        };
        for (side, bit) in [(0, WALK_LEFT), (1, WALK_RIGHT)] {
            let sign = if side == 0 { 1.0 } else { -1.0 };
            // Left of the tangent is (ty, −tx): y runs south.
            let beside = |i: usize, d: f64| {
                let ([tx, ty], unit) = (curve.tangent[i], sign * d / metres(curve.y[i]));
                [curve.x[i] + ty * unit, curve.y[i] - tx * unit]
            };
            let tag = |i: usize| {
                let piece = &pieces[owner[i]];
                net.sidewalks[piece.way][if piece.forward { side } else { 1 - side }]
            };
            let mut on: Vec<bool> =
                (0..n - 1).map(|i| width[i] > 0.0 && tag(i).unwrap_or_else(|| self.built(beside(i, 0.0)))).collect();
            if !on.contains(&true) {
                continue;
            }
            let bend = |i: usize| {
                let k = sign * curve.curvature[i];
                k > 0.0 && 1.0 / k < half(i) + width[i] + rule(i).sidewalk_margin_m
            };
            let covered = |i: usize| !self.clear(net, way(i), beside(i, half(i) + width[i] / 2.0), width[i]);
            for (i, on) in on.iter_mut().enumerate().filter(|(_, on)| **on) {
                if bend(i) || covered(i) {
                    count(if bend(i) { &mut walks.bend_m } else { &mut walks.footprint_m }, i);
                    *on = false;
                }
            }
            // A sidewalk crosses another way's footprint and a stretch without buildings to its next piece or to a
            // junction; inside a tight bend it would fold over itself.
            let stops = |i: usize| width[i] == 0.0 || tag(i) == Some(false) || bend(i);
            for (a, b) in runs(&on, false) {
                if a > 0 && b < n - 1 && curve.s[b] - curve.s[a] < GAP_M && !(a..b).any(stops) {
                    on[a..b].fill(true);
                }
            }
            for (a, b) in runs(&on, true) {
                let ahead = (b..n).take_while(|&j| curve.s[j] - curve.s[b] <= JUNCTION_M).find(|&j| junction[j]);
                if let Some(j) = ahead.filter(|&j| !(b..j).any(stops)) {
                    on[b..j].fill(true);
                }
                let behind =
                    (0..=a).rev().take_while(|&j| curve.s[a] - curve.s[j] <= JUNCTION_M).find(|&j| junction[j]);
                if let Some(j) = behind.filter(|&j| !(j..a).any(stops)) {
                    on[j..a].fill(true);
                }
            }
            // A stub joins a junction to an end in the open, inside the stroke and off a junction. An end at the
            // stroke's end is neither: the sidewalk may carry on into the next stroke there.
            let open = |e: usize| e > 0 && e < n - 1 && !junction[e];
            let stub = |a: usize, b: usize| (junction[a] && open(b)) || (open(a) && junction[b]);
            for (a, b) in runs(&on, true) {
                if stub(a, b) && curve.s[b] - curve.s[a] < JUNCTION_M {
                    (a..b).for_each(|i| count(&mut walks.short_m, i));
                    on[a..b].fill(false);
                }
            }
            for (out, _) in out.iter_mut().zip(&on).filter(|(_, &on)| on) {
                *out |= bit;
            }
        }
        out
    }

    /// Whether TOWN_BUILDINGS buildings stand within TOWN_M of `point`.
    fn built(&self, point: [f64; 2]) -> bool {
        let Some(buildings) = self.buildings else { return false };
        let r = TOWN_M / metres(point[1]);
        buildings.locate_within_distance(point, r * r).take(TOWN_BUILDINGS).count() == TOWN_BUILDINGS
    }

    /// Whether a sidewalk `width` metres wide, of way `w`, with its middle at `point`, keeps off the footprints
    /// of the other ways and the waterways, but near a point where such a way meets `w`.
    fn clear(&self, net: &Network, w: usize, point: [f64; 2], width: f64) -> bool {
        let scale = metres(point[1]);
        let joined = |other: usize| {
            self.joins[w].iter().any(|&(var, at)| {
                hypot(sub(at, point)) * scale < JOIN_M && self.joins[other].iter().any(|&(v, _)| v == var)
            })
        };
        self.boxes.overlapping([point[0], point[1], point[0], point[1]]).into_iter().all(|b| {
            let Footprint { p, q, half, way, mouth, ref pipes } = self.footprints[b];
            if way.is_some_and(|o| net.way_feature[o].0 == net.way_feature[w].0 || joined(o)) {
                return true;
            }
            let (d, reach) = (sub(q, p), half + width / 2.0);
            let t = (dot(sub(point, p), d) / dot(d, d).max(1e-300)).clamp(0.0, 1.0);
            let foot = [p[0] + t * d[0], p[1] + t * d[1]];
            // An open waterway's channel ends at a culvert's mouth: the pipe runs on under the road and its sidewalks.
            let at_mouth = [p, q].iter().zip(mouth).any(|(&end, m)| m && hypot(sub(foot, end)) * scale < reach);
            let piped = pipes.iter().any(|&(at, length)| hypot(sub(foot, at)) * scale < length);
            at_mouth || piped || hypot(sub(foot, point)) * scale >= reach
        })
    }
}

/// The distance from `point` to the segment p-q, in world units.
fn distance(point: [f64; 2], p: [f64; 2], q: [f64; 2]) -> f64 {
    let d = sub(q, p);
    let t = (dot(sub(point, p), d) / dot(d, d).max(1e-300)).clamp(0.0, 1.0);
    hypot(sub(point, [p[0] + t * d[0], p[1] + t * d[1]]))
}

/// Per segment of a stroke of `n` samples, its piece.
fn owners(pieces: &[Piece], n: usize) -> Vec<usize> {
    let mut owner = vec![0; n.saturating_sub(1)];
    for (p, piece) in pieces.iter().enumerate() {
        owner[piece.first..piece.last].fill(p);
    }
    owner
}

/// A way out of a point: its heading, and the runs of sidewalk on its left and its right.
type WayOut = (f64, [Option<usize>; 2]);

/// What the length rule reads of a stroke with a sidewalk, and its sides, which it changes.
pub(crate) struct Walking<'a> {
    pub curve: &'a Curve,
    pub pieces: &'a [Piece],
    /// Per sample, its variable of OSM where it has one, and whether the job owns it.
    pub vars: &'a [Option<usize>],
    pub owned: &'a [bool],
    pub sides: &'a mut Vec<u8>,
}

/// Per point of OSM, the ways out of it whose class has a `sidewalk_m`.
pub(crate) type WaysOut = HashMap<usize, Vec<WayOut>>;

/// Add the ways out of the points of a stroke to `ways_out`, with the run of each side of the segment that
/// leaves the point, from `run_of`, None for a stroke with no sidewalk.
pub(crate) fn add_ways_out(
    ways_out: &mut WaysOut,
    (net, classes): (&Network, &Classes),
    (curve, pieces, vars): (&Curve, &[Piece], &[Option<usize>]),
    run_of: Option<&[Vec<Option<usize>>; 2]>,
) {
    let n = curve.s.len();
    let owner = owners(pieces, n);
    let can = |segment: usize| classes.rows[net.rule[pieces[owner[segment]].way]].sidewalk_m > 0.0;
    let run = |side: usize, segment: usize| run_of.and_then(|of| of[side][segment]);
    for (i, var) in vars.iter().enumerate().filter_map(|(i, v)| Some((i, (*v)?))) {
        let [tx, ty] = curve.tangent[i];
        if i + 1 < n && can(i) {
            ways_out.entry(var).or_default().push((ty.atan2(tx), [run(0, i), run(1, i)]));
        }
        if i > 0 && can(i - 1) {
            ways_out.entry(var).or_default().push(((-ty).atan2(-tx), [run(1, i - 1), run(0, i - 1)]));
        }
    }
}

/// Drop each piece of sidewalk under PIECE_M of `strokes`, and count the metres of the owned samples in
/// `walks`. A piece is a run on one side, followed across each point where it carries on into another way's
/// sidewalk on the same side: of the ways out of the point whose class has a `sidewalk_m`, in the order of
/// their headings, the right side of one and the left side of the next face the same corner. `ways_out`
/// holds those of the strokes with no sidewalk.
pub(crate) fn drop_short(
    net: &Network,
    classes: &Classes,
    strokes: &mut [Walking],
    mut ways_out: WaysOut,
    walks: &mut Walks,
) {
    // The runs (stroke, bit, a, b), and per stroke and side the run of each segment.
    let (mut found, mut run_of) = (Vec::new(), Vec::new());
    for (k, stroke) in strokes.iter().enumerate() {
        let mut of = [vec![None; stroke.sides.len()], vec![None; stroke.sides.len()]];
        for (side, bit) in [(0, WALK_LEFT), (1, WALK_RIGHT)] {
            let on: Vec<bool> = stroke.sides[..stroke.sides.len() - 1].iter().map(|&b| b & bit != 0).collect();
            for (a, b) in runs(&on, true) {
                of[side][a..b].fill(Some(found.len()));
                found.push((k, bit, a, b));
            }
        }
        add_ways_out(&mut ways_out, (net, classes), (stroke.curve, stroke.pieces, stroke.vars), Some(&of));
        run_of.push(of);
    }
    let mut parent: Vec<usize> = (0..found.len()).collect();
    for out in ways_out.values_mut().filter(|out| out.len() > 1) {
        // Left of a heading lies toward the lower angle: y runs south.
        out.sort_by(|p, q| p.0.total_cmp(&q.0));
        for j in 0..out.len() {
            if let (Some(a), Some(b)) = (out[j].1[1], out[(j + 1) % out.len()].1[0]) {
                union(&mut parent, a, b);
            }
        }
    }
    let mut length: HashMap<usize, f64> = HashMap::new();
    for (r, &(k, _, a, b)) in found.iter().enumerate() {
        let s = &strokes[k].curve.s;
        *length.entry(root(&mut parent, r)).or_default() += s[b] - s[a];
    }
    let owner: Vec<Vec<usize>> = strokes.iter().map(|stroke| owners(stroke.pieces, stroke.sides.len())).collect();
    for (r, &(k, bit, a, b)) in found.iter().enumerate() {
        let short = length[&root(&mut parent, r)] < PIECE_M;
        let stroke = &mut strokes[k];
        let side = usize::from(bit == WALK_RIGHT);
        for i in (a..b).filter(|&i| stroke.owned[i]) {
            let piece = &stroke.pieces[owner[k][i]];
            let tagged = net.sidewalks[piece.way][if piece.forward { side } else { 1 - side }] == Some(true);
            *match (short, tagged) {
                (true, _) => &mut walks.short_m,
                (false, true) => &mut walks.tagged_m,
                (false, false) => &mut walks.built_m,
            } += stroke.curve.s[i + 1] - stroke.curve.s[i];
        }
        if short {
            stroke.sides[a..b].iter_mut().for_each(|b| *b &= !bit);
        }
    }
}

/// The runs [a, b) of the values equal to `value`.
fn runs(values: &[bool], value: bool) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, &v) in values.iter().chain([&!value]).enumerate() {
        match (v == value, start) {
            (true, None) => start = Some(i),
            (false, Some(a)) => {
                out.push((a, i));
                start = None;
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::{engelberg, plain, table, way, Place};
    use std::collections::BTreeSet;

    use super::super::{solve, Feature, Near, Solved};
    use super::*;
    use crate::Terrain;

    /// The solve of `features` with buildings whose boxes span (east, north) to (east, north) metres.
    fn built(place: &Place, features: &[Feature], buildings: &[[(f64, f64); 2]], terrain: &dyn Terrain) -> Solved {
        let boxes =
            buildings.iter().map(|&[(e0, n0), (e1, n1)]| Rectangle::from_corners(place.at(e0, n0), place.at(e1, n1)));
        let tree = RTree::bulk_load(boxes.collect());
        let refs: Vec<&Feature> = features.iter().collect();
        solve(&refs, terrain, &Near { buildings: Some(&tree), ..Default::default() }, &table().1, None)
    }

    /// The solve of `features` with houses at (east, north) metres.
    fn solved(place: &Place, features: &[Feature], houses: &[(f64, f64)], terrain: &dyn Terrain) -> Solved {
        let points: Vec<[(f64, f64); 2]> = houses.iter().map(|&h| [h, h]).collect();
        built(place, features, &points, terrain)
    }

    /// The runs of a solved way with a sidewalk on the side `bit`, in metres east.
    fn pieces(place: &Place, solved: &Solved, f: usize, bit: u8) -> Vec<(f64, f64)> {
        let way = solved.ways[f][0].as_ref().unwrap();
        let mut out: Vec<(f64, f64)> = Vec::new();
        for k in (0..way.points.len() - 1).filter(|&k| way.plan[k].walk & bit != 0) {
            let (a, b) = (place.east_north(way.points[k]).0, place.east_north(way.points[k + 1]).0);
            match out.last_mut() {
                Some(last) if (last.1 - a).abs() < 1e-6 => last.1 = b,
                _ => out.push((a, b)),
            }
        }
        out
    }

    /// Houses every 10 m from `from` to `to` metres east, `north` metres north.
    fn row(from: f64, to: f64, north: f64) -> Vec<(f64, f64)> {
        (0..=((to - from) / 10.0).round() as usize).map(|k| (from + 10.0 * k as f64, north)).collect()
    }

    fn one(runs: &[(f64, f64)], expected: (f64, f64)) -> bool {
        runs.len() == 1 && (runs[0].0 - expected.0).abs() < 2.5 && (runs[0].1 - expected.1).abs() < 2.5
    }

    #[test]
    fn two_buildings_give_both_sides_a_sidewalk_one_does_not_and_a_tag_decides_first() {
        let place = engelberg();
        let road = |north: f64, edit: fn(&mut Feature)| {
            way("residential", place.line(&[(0.0, north), (300.0, north)], Some(20.0)), edit)
        };
        let features = [
            road(0.0, plain),
            road(500.0, |f| f.sidewalks = [Some(false); 2]),
            road(1000.0, |f| f.sidewalks = [Some(true), None]),
            road(1500.0, plain),
        ];
        // A lone house north of the first road; a row south of it, 15 m from its middle: two houses lie within
        // 25 m of the road's middle from 90 m to 210 m east, and both sides walk there.
        let mut houses: Vec<[(f64, f64); 2]> =
            [(40.0, 15.0)].iter().chain(&row(100.0, 200.0, -15.0)).map(|&h| [h, h]).collect();
        houses.extend(row(0.0, 300.0, 485.0).into_iter().chain(row(0.0, 300.0, 515.0)).map(|h| [h, h]));
        // Two large buildings on the last road: their middles lie 30 m from it, their boxes 10 m.
        houses.extend([[(100.0, 1490.0), (160.0, 1450.0)], [(100.0, 1510.0), (160.0, 1550.0)]]);
        let solved = built(&place, &features, &houses, &place.terrain(|_, _| 1000.0));
        for bit in [WALK_LEFT, WALK_RIGHT] {
            let runs = pieces(&place, &solved, 0, bit);
            assert!(one(&runs, (90.0, 210.0)), "{runs:?}");
            let runs = pieces(&place, &solved, 3, bit);
            assert!(one(&runs, (77.1, 182.9)), "{runs:?}");
        }
        assert!(pieces(&place, &solved, 1, WALK_LEFT | WALK_RIGHT).is_empty(), "a tag `separate` gives none");
        assert!(one(&pieces(&place, &solved, 2, WALK_LEFT), (0.0, 300.0)));
        assert!(pieces(&place, &solved, 2, WALK_RIGHT).is_empty());
        assert!((solved.metrics.sidewalks.tagged_m - 300.0).abs() < 1.0);
    }

    #[test]
    fn a_short_gap_closes_a_short_piece_goes_and_a_piece_runs_on_to_a_junction_across_a_footprint_near_it() {
        let place = engelberg();
        let road = |corners: &[(f64, f64)]| way("residential", place.line(corners, Some(20.0)), plain);
        let features = [
            road(&[(0.0, 0.0), (500.0, 0.0)]),
            road(&[(0.0, 500.0), (300.0, 500.0)]),
            road(&[(0.0, 1000.0), (300.0, 1000.0), (400.0, 1000.0)]),
            road(&[(300.0, 1000.0), (300.0, 1006.0), (300.0, 1100.0)]),
            // A footway that leaves the side road 6 m from the junction: its footprint lies on the main road's north
            // sidewalk for its last 13 m.
            way("footway", place.line(&[(300.0, 1006.0), (290.0, 1006.0)], Some(5.0)), plain),
        ];
        // Two rows 40 m apart: their pieces end 20 m apart, and the gap closes. Two houses: a piece of 30 m.
        // A row that ends 10 m before the junction.
        let mut houses = row(100.0, 200.0, -15.0);
        houses.extend(row(240.0, 340.0, -15.0));
        houses.extend([(150.0, 485.0), (160.0, 485.0)]);
        houses.extend(row(100.0, 280.0, 985.0));
        let solved = solved(&place, &features, &houses, &place.terrain(|_, _| 1000.0));
        for bit in [WALK_LEFT, WALK_RIGHT] {
            let runs = pieces(&place, &solved, 0, bit);
            assert!(one(&runs, (90.0, 350.0)), "{runs:?}");
            assert!(pieces(&place, &solved, 1, bit).is_empty());
            let runs = pieces(&place, &solved, 2, bit);
            assert!(one(&runs, (90.0, 300.0)), "{runs:?}");
        }
        assert!(solved.metrics.sidewalks.short_m > 55.0);
        assert!(solved.metrics.sidewalks.footprint_m > 2.0, "the footway's footprint lies on the north sidewalk");
    }

    #[test]
    fn a_tag_no_holds_against_the_gap_rule_and_the_run_on() {
        let place = engelberg();
        let road =
            |corners: &[(f64, f64)], edit: fn(&mut Feature)| way("residential", place.line(corners, Some(20.0)), edit);
        let no = |f: &mut Feature| f.sidewalks = [Some(false); 2];
        let features = [
            road(&[(0.0, 0.0), (220.0, 0.0)], plain),
            road(&[(220.0, 0.0), (230.0, 0.0)], no),
            road(&[(230.0, 0.0), (500.0, 0.0)], plain),
            road(&[(0.0, 1000.0), (295.0, 1000.0)], plain),
            road(&[(295.0, 1000.0), (300.0, 1000.0)], no),
            road(&[(300.0, 1000.0), (400.0, 1000.0)], plain),
            road(&[(300.0, 1000.0), (300.0, 1100.0)], plain),
        ];
        // The gap of 20 m between the two rows' pieces holds a part tagged `no`; the piece that ends 10 m before
        // the junction has a part tagged `no` before it.
        let mut houses = row(100.0, 200.0, -15.0);
        houses.extend(row(240.0, 340.0, -15.0));
        houses.extend(row(100.0, 280.0, 985.0));
        let solved = solved(&place, &features, &houses, &place.terrain(|_, _| 1000.0));
        for bit in [WALK_LEFT, WALK_RIGHT] {
            assert!(one(&pieces(&place, &solved, 0, bit), (90.0, 210.0)), "{:?}", pieces(&place, &solved, 0, bit));
            assert!(pieces(&place, &solved, 1, bit).is_empty());
            assert!(one(&pieces(&place, &solved, 2, bit), (230.0, 350.0)), "{:?}", pieces(&place, &solved, 2, bit));
            assert!(one(&pieces(&place, &solved, 3, bit), (90.0, 290.0)), "{:?}", pieces(&place, &solved, 3, bit));
            assert!(pieces(&place, &solved, 4, bit).is_empty());
        }
    }

    #[test]
    fn a_sidewalk_that_leaves_a_junction_and_stops_within_15_m_goes() {
        let place = engelberg();
        let road =
            |corners: &[(f64, f64)], edit: fn(&mut Feature)| way("residential", place.line(corners, Some(10.0)), edit);
        let features = [
            road(&[(0.0, 0.0), (300.0, 0.0)], |f| f.sidewalks = [Some(true); 2]),
            road(&[(150.0, 0.0), (150.0, 10.0)], |f| f.sidewalks = [Some(true); 2]),
            road(&[(150.0, 10.0), (150.0, 100.0)], |f| f.sidewalks = [Some(false); 2]),
        ];
        let solved = solved(&place, &features, &[], &place.terrain(|_, _| 1000.0));
        let walks = |f: usize| solved.ways[f][0].as_ref().unwrap().plan.iter().map(|p| p.walk).max().unwrap();
        assert_eq!(walks(0), WALK_LEFT | WALK_RIGHT);
        assert_eq!((walks(1), walks(2)), (0, 0));
    }

    #[test]
    fn a_short_street_keeps_its_sidewalks_where_they_carry_on_into_the_streets_at_both_its_ends() {
        let place = engelberg();
        let street = |corners: &[(f64, f64)]| way("residential", place.line(corners, Some(20.0)), plain);
        let features = [
            street(&[(0.0, 0.0), (150.0, 0.0), (300.0, 0.0)]),
            street(&[(0.0, 40.0), (150.0, 40.0), (300.0, 40.0)]),
            street(&[(150.0, 0.0), (150.0, 40.0)]),
        ];
        let mut houses = row(0.0, 300.0, -15.0);
        houses.extend(row(0.0, 300.0, 55.0));
        houses.extend([135.0, 165.0].iter().flat_map(|&e| [10.0, 20.0, 30.0].map(|n| (e, n))));
        let solved = solved(&place, &features, &houses, &place.terrain(|_, _| 1000.0));
        let way = solved.ways[2][0].as_ref().unwrap();
        let last = way.plan.len() - 1;
        assert!(way.plan[..last].iter().all(|p| p.walk == WALK_LEFT | WALK_RIGHT), "{:?}", way.plan);
    }

    #[test]
    fn a_piece_sums_its_length_around_a_corner_and_not_across_the_road() {
        let place = engelberg();
        // Two ways of 30 m meet at a right angle: each tags one side.
        let corner = |north: f64, sides: [[Option<bool>; 2]; 2]| {
            let mut first = way("residential", place.line(&[(0.0, north), (30.0, north)], Some(10.0)), plain);
            let mut second = way("residential", place.line(&[(30.0, north), (30.0, north + 30.0)], Some(10.0)), plain);
            (first.sidewalks, second.sidewalks) = (sides[0], sides[1]);
            [first, second]
        };
        let (left, right) = ([Some(true), Some(false)], [Some(false), Some(true)]);
        // The left of a way east and the left of a way north face one corner: 60 m. The left of the first and
        // the right of the second lie across the road from each other: two pieces of 30 m. A street with no
        // sidewalk that leaves into the corner splits it: two pieces of 30 m. A footway there does not: 60 m.
        // The census of the viewer (`corners` and `splitsCorners` of builder/web/test-support/trailview/census.ts)
        // holds the same four cases.
        let between = way("residential", place.line(&[(30.0, 1000.0), (0.0, 1030.0)], Some(10.0)), |f| {
            f.sidewalks = [Some(false); 2]
        });
        let features: Vec<Feature> = corner(0.0, [left, left])
            .into_iter()
            .chain(corner(500.0, [left, right]))
            .chain(corner(1000.0, [left, left]))
            .chain([between])
            .chain(corner(1500.0, [left, left]))
            .chain([way("footway", place.line(&[(30.0, 1500.0), (0.0, 1530.0)], Some(10.0)), plain)])
            .collect();
        let solved = solved(&place, &features, &[], &place.terrain(|_, _| 1000.0));
        let walked = |f: usize| solved.ways[f][0].as_ref().unwrap().plan.iter().map(|p| p.walk).max().unwrap();
        assert_eq!((walked(0), walked(1)), (WALK_LEFT, WALK_LEFT));
        assert_eq!((walked(2), walked(3)), (0, 0));
        assert_eq!((walked(4), walked(5)), (0, 0));
        assert_eq!((walked(7), walked(8)), (WALK_LEFT, WALK_LEFT));
    }

    #[test]
    fn a_tag_holds_the_side_of_its_own_way_where_the_way_runs_against_its_stroke() {
        let place = engelberg();
        // The second way runs west and joins the first one's stroke: its left is the south.
        let mut against = way("residential", place.line(&[(300.0, 0.0), (150.0, 0.0)], Some(20.0)), plain);
        against.sidewalks = [Some(true), None];
        let features = [way("residential", place.line(&[(0.0, 0.0), (150.0, 0.0)], Some(20.0)), plain), against];
        let solved = solved(&place, &features, &[], &place.terrain(|_, _| 1000.0));
        let walks = |f: usize| {
            let plan = &solved.ways[f][0].as_ref().unwrap().plan;
            plan[..plan.len() - 1].iter().map(|p| p.walk).collect::<BTreeSet<u8>>()
        };
        assert_eq!(walks(1), BTreeSet::from([WALK_LEFT]));
        assert_eq!(walks(0), BTreeSet::from([0]));
    }
}
