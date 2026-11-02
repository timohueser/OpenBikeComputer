//! The sidewalks of the roads: per side of each segment of a stroke, whether it has one.
//!
//! A side can have a sidewalk on a way on the ground whose class has a `sidewalk_m`. OSM's tags decide
//! first; a side without a tag has one where TOWN_BUILDINGS buildings stand within TOWN_M of it. Then a
//! gap under GAP_M without a tag closes. A side has none where the ground past its edge is steep, unless
//! a tag puts one there, none inside a bend too tight for it, and none on the bed of another way. A piece
//! that ends within JUNCTION_M of a junction runs on to it, and a piece under PIECE_M goes.

use rstar::RTree;

use super::align::Curve;
use super::network::{dot, hypot, sub, Network};
use super::{Classes, River};
use crate::geometry::Boxes;
use crate::kinds::SIDEWALK_MAX_M;
use crate::{metres, Terrain, WALK_LEFT, WALK_RIGHT};

const TOWN_BUILDINGS: usize = 3;
const TOWN_M: f64 = 40.0;
const GAP_M: f64 = 40.0;
const PIECE_M: f64 = 50.0;
const JUNCTION_M: f64 = 15.0;
/// The ground from STEEP_FROM_M to STEEP_TO_M past the road's edge is steep where it rises or falls by
/// more than STEEP per metre: 1 in 2.
const STEEP_FROM_M: f64 = 2.0;
const STEEP_TO_M: f64 = 6.0;
const STEEP: f64 = 0.5;
/// The radius of a bend leaves this much room inside the road's edge and its sidewalk.
const BEND_MARGIN_M: f64 = 1.0;
/// Another way does not count as a bed within this many metres of a point where it meets the road, as
/// JOIN_M of bins.ts: the corner shapes the mouth there.
const JOIN_M: f64 = 12.0;

/// Metres of sidewalk by the rule that decides them: kept by a tag or by buildings, and taken by a steep
/// side, by a bend, by another way's bed and by a piece too short.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Walks {
    pub tagged_m: f64,
    pub built_m: f64,
    pub steep_m: f64,
    pub bend_m: f64,
    pub bed_m: f64,
    pub short_m: f64,
}

impl Walks {
    pub fn add(&mut self, other: &Walks) {
        self.tagged_m += other.tagged_m;
        self.built_m += other.built_m;
        self.steep_m += other.steep_m;
        self.bend_m += other.bend_m;
        self.bed_m += other.bed_m;
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

/// A bed on the ground: a segment, its half-width and its way, None for a waterway.
type Bed = ([f64; 2], [f64; 2], f64, Option<usize>);

/// What the sidewalks of a solve read besides the strokes.
pub(crate) struct Town<'a> {
    buildings: Option<&'a RTree<[f64; 2]>>,
    terrain: &'a dyn Terrain,
    beds: Vec<Bed>,
    boxes: Boxes,
    /// Per way, the variables and points of its OSM vertices that other ways share.
    joins: Vec<Vec<(usize, [f64; 2])>>,
    /// Per variable of OSM: whether three ways out or more leave it.
    pub junction: Vec<bool>,
}

impl<'a> Town<'a> {
    pub fn new(
        net: &Network,
        rivers: &[&River],
        buildings: Option<&'a RTree<[f64; 2]>>,
        terrain: &'a dyn Terrain,
    ) -> Self {
        let mut beds: Vec<Bed> = Vec::new();
        for (s, &w) in net.segment_way.iter().enumerate().filter(|&(_, &w)| !net.free[w]) {
            let v = net.segment_start[s];
            beds.push((net.vertex_xy[v], net.vertex_xy[v + 1], net.half[w], Some(w)));
        }
        for river in rivers {
            beds.extend(river.points.windows(2).map(|p| (p[0], p[1], river.half, None)));
        }
        let boxes: Vec<[f64; 4]> = beds
            .iter()
            .map(|&(p, q, half, _)| {
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
        Town { buildings, terrain, beds, boxes: Boxes::new(&boxes), joins, junction }
    }

    /// Per sample of a stroke, the sides of the segment from it to the next sample that have a sidewalk:
    /// WALK_LEFT and WALK_RIGHT of the stroke. `junction` and `owned` are per sample: a junction of three
    /// ways out or more, and a sample whose metres count in `walks`.
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
        let mut owner = vec![0; n.saturating_sub(1)];
        for (p, piece) in pieces.iter().enumerate() {
            owner[piece.first..piece.last].fill(p);
        }
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
            let mut on: Vec<bool> = (0..n - 1)
                .map(|i| {
                    width[i] > 0.0
                        && tag(i).unwrap_or_else(|| self.built(beside(i, half(i) + width[i] / 2.0), beside(i, 0.0)))
                })
                .collect();
            if !on.contains(&true) {
                continue;
            }
            for (a, b) in runs(&on, false) {
                let inside = a > 0 && b < n - 1;
                if inside && curve.s[b] - curve.s[a] < GAP_M && (a..b).all(|i| width[i] > 0.0 && tag(i).is_none()) {
                    on[a..b].fill(true);
                }
            }
            let free: Vec<usize> = (0..n - 1).filter(|&i| width[i] > 0.0 && tag(i) != Some(true)).collect();
            let near: Vec<[f64; 2]> = free
                .iter()
                .flat_map(|&i| [beside(i, half(i) + STEEP_FROM_M), beside(i, half(i) + STEEP_TO_M)])
                .collect();
            let ground_heights = self.terrain.heights(&near);
            let mut steep = vec![false; n - 1];
            for (k, &i) in free.iter().enumerate() {
                let rise = (ground_heights[2 * k + 1] - ground_heights[2 * k]).abs();
                steep[i] = rise > STEEP * (STEEP_TO_M - STEEP_FROM_M);
            }
            let bend = |i: usize| {
                let k = sign * curve.curvature[i];
                k > 0.0 && 1.0 / k < half(i) + width[i] + BEND_MARGIN_M
            };
            let bed = |i: usize| !self.clear(net, way(i), beside(i, half(i) + width[i] / 2.0), width[i]);
            let barred = |i: usize| width[i] == 0.0 || tag(i) == Some(false) || steep[i] || bend(i) || bed(i);
            // A sample on has a width and no tag `no`: a steep side, a bend or a bed bars it.
            for (i, on) in on.iter_mut().enumerate().filter(|(_, on)| **on) {
                if barred(i) {
                    let why = if steep[i] {
                        &mut walks.steep_m
                    } else if bend(i) {
                        &mut walks.bend_m
                    } else {
                        &mut walks.bed_m
                    };
                    count(why, i);
                    *on = false;
                }
            }
            for (a, b) in runs(&on, true) {
                let ahead = (b..n).take_while(|&j| curve.s[j] - curve.s[b] <= JUNCTION_M).find(|&j| junction[j]);
                if let Some(j) = ahead.filter(|&j| (b..j).all(|i| !barred(i))) {
                    on[b..j].fill(true);
                }
                let behind =
                    (0..=a).rev().take_while(|&j| curve.s[a] - curve.s[j] <= JUNCTION_M).find(|&j| junction[j]);
                if let Some(j) = behind.filter(|&j| (j..a).all(|i| !barred(i))) {
                    on[j..a].fill(true);
                }
            }
            for (a, b) in runs(&on, true) {
                let short = curve.s[b] - curve.s[a] < PIECE_M;
                for (i, out) in out.iter_mut().enumerate().take(b).skip(a) {
                    let why = match (short, tag(i)) {
                        (true, _) => &mut walks.short_m,
                        (false, Some(true)) => &mut walks.tagged_m,
                        (false, _) => &mut walks.built_m,
                    };
                    count(why, i);
                    *out |= if short { 0 } else { bit };
                }
            }
        }
        out
    }

    /// Whether TOWN_BUILDINGS buildings stand within TOWN_M of `point`, on its side of the way's `middle`.
    fn built(&self, point: [f64; 2], middle: [f64; 2]) -> bool {
        let Some(buildings) = self.buildings else { return false };
        let r = TOWN_M / metres(point[1]);
        let out = sub(point, middle);
        let near = buildings.locate_within_distance(point, r * r).filter(|&&b| dot(sub(b, middle), out) > 0.0);
        near.take(TOWN_BUILDINGS).count() == TOWN_BUILDINGS
    }

    /// Whether a sidewalk `width` metres wide, of way `w`, with its middle at `point`, keeps off the beds
    /// of the other ways and the waterways, but near a point where such a way meets `w`.
    fn clear(&self, net: &Network, w: usize, point: [f64; 2], width: f64) -> bool {
        let scale = metres(point[1]);
        let joined = |other: usize| {
            self.joins[w].iter().any(|&(var, at)| {
                hypot(sub(at, point)) * scale < JOIN_M && self.joins[other].iter().any(|&(v, _)| v == var)
            })
        };
        self.boxes.overlapping([point[0], point[1], point[0], point[1]]).into_iter().all(|b| {
            let (p, q, half, other) = self.beds[b];
            if other.is_some_and(|o| net.way_feature[o].0 == net.way_feature[w].0 || joined(o)) {
                return true;
            }
            let d = sub(q, p);
            let t = (dot(sub(point, p), d) / dot(d, d).max(1e-300)).clamp(0.0, 1.0);
            hypot(sub([p[0] + t * d[0], p[1] + t * d[1]], point)) * scale >= half + width / 2.0
        })
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
    use super::super::{solve, Feature, Near, Solved};
    use super::*;

    /// The solve of `features` with houses at (east, north) metres.
    fn solved(place: &Place, features: &[Feature], houses: &[(f64, f64)], terrain: &dyn Terrain) -> Solved {
        let tree = RTree::bulk_load(houses.iter().map(|&(e, n)| place.at(e, n)).collect());
        let refs: Vec<&Feature> = features.iter().collect();
        solve(&refs, terrain, &Near { rivers: Vec::new(), buildings: Some(&tree) }, &table().1, None)
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
    fn three_houses_give_a_side_a_sidewalk_one_does_not_and_a_tag_decides_first() {
        let place = engelberg();
        let road = |north: f64, edit: fn(&mut Feature)| {
            way("residential", place.line(&[(0.0, north), (300.0, north)], Some(20.0)), edit)
        };
        let features = [
            road(0.0, plain),
            road(500.0, |f| f.sidewalks = [Some(false); 2]),
            road(1000.0, |f| f.sidewalks = [Some(true), None]),
        ];
        // Left of a way to the east is north. Three houses south of the first road, one north of it.
        let mut houses = vec![(150.0, -15.0), (160.0, -15.0), (170.0, -15.0), (60.0, 15.0)];
        houses.extend(row(0.0, 300.0, 485.0).into_iter().chain(row(0.0, 300.0, 515.0)));
        let solved = solved(&place, &features, &houses, &place.terrain(|_, _| 1000.0));
        assert!(pieces(&place, &solved, 0, WALK_LEFT).is_empty(), "one house gives none");
        // The middle of the sidewalk lies 3.75 m from the road's: within 40 m of a house 15 m from it for 38.4 m.
        let right = pieces(&place, &solved, 0, WALK_RIGHT);
        assert!(one(&right, (131.6, 188.4)), "{right:?}");
        assert!(pieces(&place, &solved, 1, WALK_LEFT | WALK_RIGHT).is_empty(), "a tag `separate` gives none");
        assert!(one(&pieces(&place, &solved, 2, WALK_LEFT), (0.0, 300.0)));
        assert!(pieces(&place, &solved, 2, WALK_RIGHT).is_empty());
        assert!((solved.metrics.sidewalks.tagged_m - 300.0).abs() < 1.0);
    }

    #[test]
    fn a_short_gap_closes_a_short_piece_goes_and_a_piece_runs_on_to_a_junction_near_it() {
        let place = engelberg();
        let features = [
            way("residential", place.line(&[(0.0, 0.0), (500.0, 0.0)], Some(20.0)), plain),
            way("residential", place.line(&[(0.0, 500.0), (300.0, 500.0)], Some(20.0)), plain),
            way("residential", place.line(&[(0.0, 1000.0), (300.0, 1000.0), (400.0, 1000.0)], Some(20.0)), plain),
            way("residential", place.line(&[(300.0, 1000.0), (300.0, 1100.0)], Some(20.0)), plain),
        ];
        // Two rows 30 m apart; a piece of 40 m from houses 30 m away; a piece that ends 12 m before a junction.
        let mut houses = row(100.0, 200.0, -15.0);
        houses.extend(row(267.0, 367.0, -15.0));
        houses.extend(row(100.0, 120.0, 470.0));
        houses.extend(row(100.0, 270.0, 985.0));
        let solved = solved(&place, &features, &houses, &place.terrain(|_, _| 1000.0));
        let right = pieces(&place, &solved, 0, WALK_RIGHT);
        assert!(one(&right, (81.6, 385.4)), "{right:?}");
        assert!(pieces(&place, &solved, 1, WALK_RIGHT).is_empty());
        assert!(solved.metrics.sidewalks.short_m > 35.0);
        let right = pieces(&place, &solved, 2, WALK_RIGHT);
        assert!(one(&right, (81.6, 300.0)), "{right:?}");
    }

    #[test]
    fn a_steep_side_has_no_sidewalk_unless_a_tag_puts_one_there() {
        let place = engelberg();
        // South of each road the ground climbs 1 in 1 from 4 m past its middle.
        let steep = place.terrain(|_, n| 1000.0 + (-(n - 500.0 * (n / 500.0).round()) - 4.0).max(0.0));
        let tagged = |f: &mut Feature| f.sidewalks = [None, Some(true)];
        let features = [
            way("residential", place.line(&[(0.0, 0.0), (300.0, 0.0)], Some(20.0)), plain),
            way("residential", place.line(&[(0.0, 500.0), (300.0, 500.0)], Some(20.0)), tagged),
        ];
        let houses: Vec<(f64, f64)> = [-20.0, 20.0, 480.0, 520.0].iter().flat_map(|&n| row(0.0, 300.0, n)).collect();
        let solved = solved(&place, &features, &houses, &steep);
        assert!(one(&pieces(&place, &solved, 0, WALK_LEFT), (0.0, 300.0)));
        assert!(pieces(&place, &solved, 0, WALK_RIGHT).is_empty());
        assert!((solved.metrics.sidewalks.steep_m - 300.0).abs() < 1.0);
        assert!(one(&pieces(&place, &solved, 1, WALK_RIGHT), (0.0, 300.0)));
    }
}
