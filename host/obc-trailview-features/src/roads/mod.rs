//! The road solver: a height on each vertex of the roads, paths, railways, tunnels and galleries,
//! from the master terrain, by the rules per class of `data/trailview/roads.json`.
//!
//! `solve_by_area` runs it in phases. Phase 0 finds the culverts: a short bridge that lies on the
//! terrain is a way on the ground over its water. Phase A gathers per area the chains of nodes and their
//! facts: what each chain reads at its ends, what it crosses and what passes over it. Phase B
//! (`heights`) solves each chain once per bake, after every chain that it reads, into one store of node
//! heights. A way on the ground lies within a band of the terrain and no steeper than the grade of its
//! class but where the terrain asks more. Ways that share a node share its height, and a side road leaves
//! a junction on the surface of the through road. A road, a track or a railway on the ground lies over the
//! cover of a culvert part or of narrow water that it crosses. Tracks side by side, as OSM maps each track
//! of a double line, lie at one height, and so do roads and paths whose carriageways overlap. A bridge or a
//! tunnel runs between its ends, which the ways that it meets fix, and bends as little as it can: a deck
//! rises over what it crosses and a tunnel sinks under what crosses over it and under the terrain, no
//! more steeply than 1:1. Of two bridges that cross, the one of the higher layer passes over the other, at
//! one layer the better class, and of one class the later one. Phase C (`solve`) takes every height from
//! the store and fits the alignment of the ways (`align`): a fitted plan, and the vertices that hold it.

mod align;
#[cfg(test)]
mod align_tests;
mod area;
mod chain;
mod chains;
mod curve;
mod fit;
mod heights;
mod network;
mod rows;
mod sidewalks;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashSet};

use rstar::RTree;

pub use align::polyline;
pub use area::{context, solve_by_area};
use chains::Names;
use fit::Rows;
use heights::Heights;
pub use heights::{GroundMetrics, Join, Short};
use network::Network;
use sidewalks::Town;
pub use sidewalks::Walks;

use crate::geometry::{key, percentile};
use crate::structures::{BORE_M, BORE_PATH_M, BORE_RAIL_M};
use crate::{Plan, Terrain};

const SAMPLE_M: f64 = 2.0;
/// A way has a node at each of its distinct OSM vertices and so many even cuts between two that no
/// piece is longer than NODE_M (`network::pieces`).
pub(crate) const NODE_M: f64 = SAMPLE_M;
/// A way on the ground that joins a structure inside it hangs on it for this far from the join; the
/// census gate of the bake leaves out this far of the way from it.
pub const JOINT_M: f64 = 10.0;
/// A side road lies on the surface of the through road for this far from their junction; a path
/// that meets a path, for APPROACH_PATH_M.
const APPROACH_M: f64 = 4.0;
const APPROACH_PATH_M: f64 = 1.5;
/// A way without a grade limit leaves a road with one as a ramp: for RAMP_M from the junction its
/// grade differs from the terrain's under it by at most RAMP_GRADE, the owner's choice for a path.
/// The ramps are solved last, with all else fixed, so a ramp never moves a road. Where the terrain
/// is unknown, a way has no ramp.
const RAMP_M: f64 = 15.0;
const RAMP_GRADE: f64 = 0.25;
const HUBER_M: f64 = 1.0;
/// A bridge shorter than CULVERT_M that lies no higher than CULVERT_RISE_M over the terrain is a
/// road over a culvert.
const CULVERT_M: f64 = 8.0;
const CULVERT_RISE_M: f64 = 1.0;
/// The least height of a deck's road over the water, the road, the path and the track that it
/// crosses: the clearance under the deck and the deck's depth (DECK_M for a road, DECK_PATH_M for a path).
pub const CLEAR_WATER_M: f64 = 1.5;
pub const CLEAR_ROAD_M: f64 = 4.5;
pub const CLEAR_PATH_M: f64 = 2.5;
pub const CLEAR_RAIL_M: f64 = 6.0;
pub const DECK_M: f64 = 1.0;
pub const DECK_PATH_M: f64 = 0.4;
/// A road or a path over a tunnel lies at least this far over its bore.
pub const COVER_M: f64 = 1.0;
/// A track within PARALLEL_M beside another, or a road or path whose carriageway overlaps another's,
/// and within PARALLEL_COS of its direction, lies at its height, unless the terrain under the two
/// differs by more than PARALLEL_RISE_M within PARALLEL_FREE_M along it: a ramp or a flyover, whose
/// climb the smoothing of a track spreads that far.
const PARALLEL_M: f64 = 6.0;
const PARALLEL_COS: f64 = 0.9;
const PARALLEL_RISE_M: f64 = 0.5;
const PARALLEL_FREE_M: f64 = 100.0;
/// The cell of the grid of the crossings in world units, about 100 m.
const GRID_UNITS: f64 = 2.5e-6;

/// A class of the table: its grade limit (infinite for none), its smoothing length, whether it is a
/// track, and its alignment: the tolerance of the plan, the design speed, the most banking, the most
/// widening, and how far the curve of its kept vertices may leave the fit.
#[derive(Clone, Debug)]
pub struct Class {
    pub grade: f64,
    pub smooth_m: f64,
    pub rail: bool,
    pub plan_tol_m: f64,
    pub design_kmh: f64,
    pub bank: f64,
    pub widen_m: f64,
    pub keep_m: f64,
    /// The width of a sidewalk, 0 where the class has none; the rise per metre of the ground past the
    /// road's edge that is too steep for one; and the room that a bend leaves inside the sidewalk.
    pub sidewalk_m: f64,
    pub sidewalk_steep: f64,
    pub sidewalk_margin_m: f64,
}

/// The classes from the most important down, as the junctions rank them; a class from `path` on is
/// a path's.
#[derive(Clone, Debug)]
pub struct Classes {
    pub rows: Vec<Class>,
    pub path: usize,
}

/// A way of the solver, with the facts of its tags.
#[derive(Clone, Debug, Default)]
pub struct Feature {
    /// A bridge or a tunnel: it ignores the terrain under it.
    pub free: bool,
    pub tunnel: bool,
    /// Its OSM `layer`, 0 without one, and whether it has one.
    pub layer: i64,
    pub layered: bool,
    /// A way on the ground of a negative `layer` without a `tunnel` tag lies under something that the
    /// solver does not model, such as a station hall: no way on the ground passes over or under it.
    pub indoor: bool,
    /// The carriageway's half-width, and the outer half-width of its outermost band.
    pub half: f64,
    pub outer: f64,
    /// Per side, left and right of its direction, whether OSM tags a sidewalk there; None where the
    /// bake decides. A way that can have none has `Some(false)` on both.
    pub sidewalks: [Option<bool>; 2],
    /// A short bridge that lies on the terrain (`chains::culverts`): a way on the ground over its water.
    pub culvert: bool,
    /// Its row of the class table.
    pub rule: usize,
    pub parts: Vec<Vec<[f64; 2]>>,
}

/// A solved waterway: its points and their levels, NaN where unknown, its half-width, the cover of a
/// road or a track over it at a culvert with the most that the cover may lift that way, and whether it
/// is narrow enough that a way on the ground crosses it on a culvert without a tag.
#[derive(Clone, Debug)]
pub struct River {
    pub points: Vec<[f64; 2]>,
    pub levels: Vec<f64>,
    pub half: f64,
    pub cover: f64,
    pub lift: f64,
    pub untagged_culvert: bool,
}

/// What the ways of a region meet besides the terrain: the solved waterways, the culvert parts with the
/// levels of their water, and a point per building.
#[derive(Default)]
pub struct Surroundings {
    pub rivers: Vec<River>,
    pub culverts: Vec<River>,
    pub buildings: RTree<[f64; 2]>,
}

/// What the ways of one solve meet: the solved waterways and the culvert parts near them, and the points
/// of the buildings.
#[derive(Default)]
pub struct Near<'a> {
    pub rivers: Vec<&'a River>,
    pub culverts: Vec<&'a River>,
    pub buildings: Option<&'a RTree<[f64; 2]>>,
}

/// A way after the solve: its kept vertices on the fitted curve, their heights (NaN where unknown) and
/// alignment, and the index of the OSM vertex that each follows, which names the job that owns it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Way {
    pub points: Vec<[f64; 2]>,
    pub z: Vec<f64>,
    pub plan: Vec<Plan>,
    pub osm: Vec<usize>,
}

impl Way {
    fn add(&mut self, other: &Way, i: usize) {
        self.points.push(other.points[i]);
        self.z.push(other.z[i]);
        self.plan.push(other.plan[i]);
        self.osm.push(other.osm[i]);
    }
}

/// Per feature, per part: its way, or None for a part that the solver leaves alone.
pub type Ways = Vec<Vec<Option<Way>>>;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Metrics {
    pub ways: usize,
    pub variables: usize,
    pub known: usize,
    pub approaches: usize,
    pub culverts: usize,
    pub clearances: usize,
    pub clearances_low_cm: usize,
    /// The most that a deck lies under its clearance, and a pass under its margin.
    pub clearance_short_m: f64,
    pub passes: usize,
    pub passes_near_cm: usize,
    pub pass_short_m: f64,
    /// The vertices where a way on the ground meets a tunnel inside it, and takes its height.
    pub tunnel_joins: usize,
    pub residual_p99_m: f64,
    pub covers: usize,
    pub covers_lifted: usize,
    /// The cover rows of untagged culverts, and those that lift their way.
    pub covers_untagged: usize,
    pub covers_untagged_lifted: usize,
    /// Tagged cover rows of a way on the ground or a culvert whose lift would pass the most of their
    /// water: the way keeps its height.
    pub covers_capped: usize,
    /// The same for the cover rows of untagged culverts.
    pub covers_untagged_capped: usize,
    /// The largest lift of a lifted cover row over the plain solve of its chain.
    pub cover_lift_top_m: f64,
    pub sidewalks: Walks,
}

impl Metrics {
    /// The metrics of two jobs: the counts add up; `residual_p99_m`, `cover_lift_top_m` and the shortfalls
    /// are the largest.
    pub fn add(&mut self, other: &Metrics) {
        let sums = [
            (&mut self.ways, other.ways),
            (&mut self.variables, other.variables),
            (&mut self.known, other.known),
            (&mut self.approaches, other.approaches),
            (&mut self.culverts, other.culverts),
            (&mut self.clearances, other.clearances),
            (&mut self.clearances_low_cm, other.clearances_low_cm),
            (&mut self.passes, other.passes),
            (&mut self.passes_near_cm, other.passes_near_cm),
            (&mut self.tunnel_joins, other.tunnel_joins),
            (&mut self.covers, other.covers),
            (&mut self.covers_lifted, other.covers_lifted),
            (&mut self.covers_untagged, other.covers_untagged),
            (&mut self.covers_untagged_lifted, other.covers_untagged_lifted),
            (&mut self.covers_capped, other.covers_capped),
            (&mut self.covers_untagged_capped, other.covers_untagged_capped),
        ];
        for (sum, value) in sums {
            *sum += value;
        }
        self.sidewalks.add(&other.sidewalks);
        self.cover_lift_top_m = self.cover_lift_top_m.max(other.cover_lift_top_m);
        self.residual_p99_m = self.residual_p99_m.max(other.residual_p99_m);
        self.clearance_short_m = self.clearance_short_m.max(other.clearance_short_m);
        self.pass_short_m = self.pass_short_m.max(other.pass_short_m);
    }
}

/// The ways of a solve; the ways that run on from each other as (feature, part, at its last vertex);
/// the features that are culverts; the tunnels that pass under a road or a path; and the metrics.
#[derive(Debug, Default)]
pub struct Solved {
    pub ways: Ways,
    /// The heights of the ground: phase B, once per bake.
    pub ground: GroundMetrics,
    pub joins: Vec<[(usize, usize, bool); 2]>,
    pub culverts: BTreeSet<usize>,
    pub underpasses: BTreeSet<usize>,
    pub metrics: Metrics,
    /// Per phase A, B, C of `solve_by_area`: its seconds, and the most bytes that the process has held by
    /// its end.
    pub phases: [(f64, usize); 3],
}

/// The heights of the ground of a bake, as one job reads them: the store of phase B and the number in
/// the bake of each feature of the job.
pub(crate) struct Ground<'a> {
    pub heights: &'a Heights,
    pub feature_of: &'a [usize],
}

/// Phase C of one job: the height of every way from the store of phase B, and the alignment and the
/// sidewalks of the features. `near` holds the solved waterways, for the decks and the beds beside a
/// sidewalk, and the buildings, for the sidewalks. `owned(xy)` marks the OSM vertices of the area of a
/// job of `solve_by_area`: the tunnels and the metrics count only the ways that start on one of them, and
/// the vertices from one of them to the next vertex of OSM.
pub(crate) fn solve(
    features: &[&Feature],
    terrain: &dyn Terrain,
    near: &Near,
    classes: &Classes,
    owned: Option<&dyn Fn([f64; 2]) -> bool>,
    ground: &Ground,
) -> Solved {
    let rivers = &near.rivers;
    let mut solved =
        Solved { ways: features.iter().map(|f| vec![None; f.parts.len()]).collect(), ..Default::default() };
    let Some(net) = Network::new(features, classes) else { return solved };
    let (n, a, b, length) = (net.count, &net.a, &net.b, &net.length);
    let total = net.vertex_xy.len();
    let mine: Vec<bool> = match owned {
        None => vec![true; total],
        Some(owned) => {
            let mut osm = 0;
            (0..total)
                .map(|v| {
                    if net.vertex_osm[v] {
                        osm = v;
                    }
                    owned(net.vertex_xy[osm])
                })
                .collect()
        }
    };
    let mut var_mine = vec![false; n];
    for v in 0..total {
        var_mine[net.vertex_var[v]] = mine[v];
    }
    let segment_mine: Vec<bool> = net.segment_start.iter().map(|&v| mine[v]).collect();
    let free: Vec<bool> = net.segment_way.iter().map(|&w| net.free[w]).collect();

    // A vertex has the height of its node in the store; one without is unknown, such as a way's node at a
    // join out of reach, where the structure that it joins has one. A variable has the height of its
    // vertices that have one. A bound holds on the curve only where its vertices stay in the tiles, and so
    // does a crossing.
    let names = Names::new(&net, ground.feature_of);
    let at: Vec<f64> = (0..total).map(|v| ground.heights.at(names.node(v))).collect();
    let (mut z, mut stay) = (vec![f64::NAN; n], vec![false; n]);
    for v in 0..total {
        if at[v].is_finite() {
            z[net.vertex_var[v]] = at[v];
        }
        stay[net.vertex_var[v]] |= ground.heights.held(names.node(v));
    }
    let known: Vec<bool> = z.iter().map(|z| z.is_finite()).collect();
    let all_known = |variables: &[usize]| variables.iter().all(|&v| known[v]);

    let (approach, _, _, _) = rows::approaches(&net, &net.sides, classes);
    let approaches = (0..approach.values.len())
        .filter(|&i| all_known(&approach.variables[i]) && var_mine[approach.variables[i][0]])
        .count();
    let untagged = rows::untagged_crossings(&net, rivers, classes);
    let (above, _, _) = rows::clearances(&net, rivers, classes);
    let (cover, cover_ways, cover_untagged, cover_first) =
        rows::covers(&net, (&near.culverts, rivers), &untagged, classes, terrain);
    let on_ground_way = |i: usize| !net.free[cover_ways[i].0];
    let covered: Vec<bool> = (0..cover.values.len())
        .map(|i| on_ground_way(i) && ground.heights.covered(names.node(cover_first[i])))
        .collect();
    let (mut c_rows, mut least, mut deck_first) = (Rows::default(), Vec::new(), Vec::new());
    for i in (0..above.values.len()).filter(|&i| all_known(&above.variables[i])) {
        c_rows.push(above.variables[i].into_iter().zip(above.coefficients[i]));
        least.push(above.values[i]);
        deck_first.push(above.variables[i][0]);
    }
    let (under, _) = rows::passes(&net, classes);
    let held_up: Vec<bool> =
        (0..under.margin.len()).map(|i| all_known(&under.upper[i].0) && all_known(&under.lower[i].0)).collect();
    let (mut u_rows, mut l_rows) = (Rows::default(), Rows::default());
    let (mut need, mut pass_first) = (Vec::new(), Vec::new());
    for i in (0..held_up.len()).filter(|&i| held_up[i]) {
        let (upper, lower) = (under.upper[i], under.lower[i]);
        u_rows.push(upper.0.into_iter().zip(upper.1));
        l_rows.push(lower.0.into_iter().zip(lower.1));
        need.push(under.margin[i]);
        pass_first.push(upper.0[0]);
    }
    let crossed = above.variables.iter().flatten().chain(under.upper.iter().chain(&under.lower).flat_map(|row| &row.0));
    crossed.for_each(|&v| stay[v] = true);
    for (_, row) in cover.variables.iter().enumerate().filter(|&(i, _)| covered[i]) {
        row.iter().for_each(|&v| stay[v] = true);
    }

    let shores: HashSet<[u64; 2]> = rivers.iter().flat_map(|river| &river.points).map(|&p| key(p)).collect();
    let mut water = vec![false; n];
    for (o, &xy) in net.osm_xy.iter().enumerate() {
        water[net.osm_var[o]] = shores.contains(&key(xy));
    }
    let earth: Vec<bool> = net.free.iter().map(|&f| !f).collect();
    // A pipe runs under the sidewalks only where its row holds the cover.
    let piped: Vec<rows::UntaggedCrossing> =
        (0..cover.values.len()).filter(|&i| covered[i]).filter_map(|i| Some(untagged[cover_untagged[i]?])).collect();
    let town = Town::new(&net, (rivers, &near.culverts), (&piped, classes), near.buildings, terrain);
    let (ways, joins, walks) = align::align(&net, &at, &stay, &water, &earth, classes, &mine, &town);
    for (way, &(f, p)) in ways.into_iter().zip(&net.way_feature) {
        solved.ways[f][p] = Some(way);
    }
    let named = |(w, end): (usize, bool)| (net.way_feature[w].0, net.way_feature[w].1, end);
    solved.joins = joins.into_iter().map(|join| join.map(named)).collect();

    let way_mine: Vec<bool> = net.way_first.iter().map(|&v| mine[v]).collect();
    let cz = c_rows.times(&z);
    let deck_mine: Vec<bool> = deck_first.iter().map(|&v| var_mine[v]).collect();
    let (uz, lz) = (u_rows.times(&z), l_rows.times(&z));
    let pass_mine: Vec<bool> = pass_first.iter().map(|&v| var_mine[v]).collect();
    // A way on the ground meets a tunnel inside it where it meets two of its segments.
    let (mut inside, mut tunnel_at, mut ground_at) = (vec![0u8; n], vec![false; n], vec![false; n]);
    for s in 0..a.len() {
        for v in [a[s], b[s]] {
            if free[s] {
                inside[v] = inside[v].saturating_add(1);
                tunnel_at[v] |= net.tunnel[net.segment_way[s]];
            } else {
                ground_at[v] = true;
            }
        }
    }
    // The ways on the ground against the terrain every SAMPLE_M, in the order of the terrain tiles, so that
    // each tile decodes about once.
    let tile = |s: usize| {
        let p = net.vertex_xy[net.segment_start[s]];
        ((p[1] * 8192.0).floor() * 8192.0 + (p[0] * 8192.0).floor()) as i64
    };
    let mut sampled: Vec<usize> =
        (0..a.len()).filter(|&s| !free[s] && segment_mine[s] && known[a[s]] && known[b[s]]).collect();
    sampled.sort_by_key(|&s| tile(s));
    let (mut points, mut along) = (Vec::new(), Vec::new());
    for s in sampled {
        let k = (length[s] / SAMPLE_M).ceil().max(1.0) as usize;
        let (p, q) = (net.vertex_xy[net.segment_start[s]], net.vertex_xy[net.segment_start[s] + 1]);
        for j in 0..k {
            let t = j as f64 / k as f64;
            points.push([p[0] + t * (q[0] - p[0]), p[1] + t * (q[1] - p[1])]);
            along.push(z[a[s]] * (1.0 - t) + z[b[s]] * t);
        }
    }
    let mut residual: Vec<f64> = terrain
        .heights(&points)
        .into_iter()
        .zip(along)
        .filter(|(h, _)| h.is_finite())
        .map(|(h, z)| (z - h).abs())
        .collect();
    let count = |mask: &mut dyn Iterator<Item = bool>| mask.filter(|&m| m).count();
    let rows_of =
        |want: &dyn Fn(usize) -> bool| count(&mut (0..cover.values.len()).map(|i| want(i) && mine[cover_first[i]]));
    let held_cover = |i: usize| covered[i] && ground.heights.held(names.node(cover_first[i]));
    solved.metrics = Metrics {
        ways: count(&mut way_mine.iter().copied()),
        variables: count(&mut var_mine.iter().copied()),
        known: count(&mut (0..n).map(|v| known[v] && var_mine[v])),
        approaches,
        culverts: 0,
        tunnel_joins: count(&mut (0..n).map(|v| inside[v] >= 2 && tunnel_at[v] && ground_at[v] && var_mine[v])),
        clearances: count(&mut deck_mine.iter().copied()),
        clearances_low_cm: count(&mut (0..least.len()).map(|i| cz[i] < least[i] - 0.01 && deck_mine[i])),
        clearance_short_m: (0..least.len()).filter(|&i| deck_mine[i]).map(|i| least[i] - cz[i]).fold(0.0, f64::max),
        passes: count(&mut pass_mine.iter().copied()),
        passes_near_cm: count(&mut (0..need.len()).map(|i| uz[i] - lz[i] < need[i] - 0.01 && pass_mine[i])),
        pass_short_m: (0..need.len()).filter(|&i| pass_mine[i]).map(|i| need[i] - uz[i] + lz[i]).fold(0.0, f64::max),
        residual_p99_m: percentile(&mut residual, 99.0).unwrap_or(0.0),
        covers: rows_of(&|i| covered[i]),
        covers_lifted: rows_of(&held_cover),
        covers_untagged: rows_of(&|i| covered[i] && cover_untagged[i].is_some()),
        covers_untagged_lifted: rows_of(&|i| held_cover(i) && cover_untagged[i].is_some()),
        covers_capped: rows_of(&|i| on_ground_way(i) && !covered[i] && cover_untagged[i].is_none()),
        covers_untagged_capped: rows_of(&|i| on_ground_way(i) && !covered[i] && cover_untagged[i].is_some()),
        cover_lift_top_m: (0..cover.values.len())
            .filter(|&i| held_cover(i) && mine[cover_first[i]])
            .map(|i| ground.heights.lift(names.node(cover_first[i])))
            .fold(0.0, f64::max),
        sidewalks: walks,
    };
    solved.underpasses = (0..held_up.len())
        .filter(|&i| held_up[i] && under.underpass[i] && way_mine[under.lower_way[i]])
        .map(|i| net.way_feature[under.lower_way[i]].0)
        .collect();
    solved
}
