//! The road solver: a height on each vertex of the roads, paths, railways, tunnels and galleries,
//! from the master terrain, by the rules per class of `data/trailview/roads.json`.
//!
//! A way is sampled every SAMPLE_M metres. Its heights minimise Σ wᵢ(zᵢ − hᵢ)² + λ·Σ Δgrade² / s,
//! with Huber weights wᵢ, so that a spike or a gully of the terrain does not move the road, and
//! λ = smooth_m⁴ of its class. The solve has two stages. The first solves the ways on the ground with
//! each bridge and tunnel held on the terrain: a way on the ground lies within GROUND_M of the terrain,
//! and no segment is steeper than the grade of its class but where the terrain asks more. Ways that
//! share a vertex share its height, and a side road leaves a junction on the surface of the through
//! road. A road, a track or a railway on the ground lies over the cover of a culvert part or of narrow water that it crosses. Tracks side by side, as
//! OSM maps each track of a double line, lie at one height: their beds are one bed. So do roads and
//! paths whose carriageways overlap. A way on a ramp beside another keeps its own height.
//! The second stage solves the bridges and tunnels between their ends on the ground, which do not
//! move: so no structure moves a way on the ground. A bridge or a tunnel ignores the terrain under it
//! and bends as little as it can; a bridge never sags under the straight line between its ends, and
//! rises where it would pass too low over the river, the road, the path or the track that it crosses;
//! a short bridge that lies on the terrain is a culvert, a road on the ground. A tunnel sinks under a
//! road, a path or a track that crosses over it, and under the terrain away from its ends. Of two
//! bridges that cross, the one of the higher layer passes over the other, at one layer the better
//! class, and of one class the later one.
//! Each solve ends with the alignment of its ways (`align`): a fitted plan, and the vertices that hold it.

mod align;
#[cfg(test)]
mod align_tests;
mod area;
#[cfg(test)]
mod chain;
#[cfg(test)]
mod chains;
mod curve;
mod fit;
mod network;
mod rows;
mod sidewalks;
#[cfg(test)]
mod tests;

use std::collections::{BTreeSet, HashSet};

use rstar::RTree;

pub use align::polyline;
pub use area::{context, solve_by_area};
use fit::{fit, Block, Graph, Rows, Term};
use network::{hypot, sub, Network};
use rows::union;
use sidewalks::Town;
pub use sidewalks::Walks;

use crate::geometry::{key, percentile};
use crate::structures::{BORE_M, BORE_PATH_M, BORE_RAIL_M};
use crate::{degrees, metres, Plan, Terrain};

const SAMPLE_M: f64 = 2.0;
/// While it solves, a way has a vertex at least every VERTEX_M.
pub const VERTEX_M: f64 = 10.0;
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
/// A way on the ground lies within GROUND_M of the terrain at each sample: the terrain wins over a
/// grade. Half the census gate of the bake (`gate::GATE_M`): the rest is room for its curve.
const GROUND_M: f64 = crate::gate::GATE_M / 2.0;
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
/// Weights against one metre of terrain samples: the terrain under a bridge or tunnel that meets no
/// road, a row that pins a junction approach or a limit, and per metre of a bridge or tunnel
/// the preference for level, so that one that meets a road at one end only is level from there. A
/// held ground bound gives way to no limit: a step of 100 keeps what gives way off its bound by about
/// one hundredth of what pushes it.
const FREE: f64 = 1e-4;
const PIN: f64 = 1e8;
const GROUND: f64 = 1e10;
const LEVEL: f64 = 1e-3;
/// GMRES refines each solve to REFINE_M, in at most REFINE_STEPS steps.
const REFINE_M: f64 = 1e-6;
const REFINE_STEPS: usize = 30;
/// Rounds of the limits of a stage; the first solves the whole network, a later one only the network
/// within REGION_M of the limits whose state changed, and the bridges and tunnels it meets: the
/// terrain anchors the rest. The step of a round halves its interval BISECTIONS times.
const ROUNDS: usize = 40;
const REGION_M: f64 = 300.0;
const BISECTIONS: usize = 50;
/// The interior point method of the structures: at most STEPS steps, each towards CENTRE of the mean
/// product of slack and multiplier, and at most BOUNDARY of the way to a bound; it starts each slack at
/// SLACK_M or more and ends where that mean is under GAP and each bound holds to SETTLED_M.
const STEPS: usize = 80;
const CENTRE: f64 = 0.1;
const BOUNDARY: f64 = 0.99;
const SLACK_M: f64 = 1.0;
const GAP: f64 = 1e-6;
const SETTLED_M: f64 = 1e-4;
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
    pub rounds: usize,
    pub held: usize,
    /// The limits whose state still changed in the last round of a stage: 0 where the rounds converged.
    pub unsettled: usize,
    pub clearances: usize,
    pub clearances_low_cm: usize,
    /// The most that a deck lies under its clearance, and a pass under its margin.
    pub clearance_short_m: f64,
    pub passes: usize,
    pub passes_near_cm: usize,
    pub pass_short_m: f64,
    pub grade_excess_m: f64,
    /// Where the grade excess is largest: latitude, longitude.
    pub grade_excess_at: [f64; 2],
    pub grade_excess_over_1cm: usize,
    /// The samples that the ground holds GROUND_M off the terrain.
    pub grounded: usize,
    /// The vertices where a way on the ground meets a tunnel inside it, and takes its height.
    pub tunnel_joins: usize,
    pub residual_p99_m: f64,
    pub refinements_unconverged: usize,
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
    /// The largest lift of a lifted cover row over the first solve.
    pub cover_lift_top_m: f64,
    pub sidewalks: Walks,
}

impl Metrics {
    /// The metrics of two jobs: the counts add up; `rounds`, `residual_p99_m`, `cover_lift_top_m` and
    /// the shortfalls are the largest, and so is the grade excess, with its place.
    pub fn add(&mut self, other: &Metrics) {
        let sums = [
            (&mut self.ways, other.ways),
            (&mut self.variables, other.variables),
            (&mut self.known, other.known),
            (&mut self.approaches, other.approaches),
            (&mut self.culverts, other.culverts),
            (&mut self.held, other.held),
            (&mut self.unsettled, other.unsettled),
            (&mut self.clearances, other.clearances),
            (&mut self.clearances_low_cm, other.clearances_low_cm),
            (&mut self.passes, other.passes),
            (&mut self.passes_near_cm, other.passes_near_cm),
            (&mut self.grade_excess_over_1cm, other.grade_excess_over_1cm),
            (&mut self.grounded, other.grounded),
            (&mut self.tunnel_joins, other.tunnel_joins),
            (&mut self.refinements_unconverged, other.refinements_unconverged),
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
        self.rounds = self.rounds.max(other.rounds);
        self.residual_p99_m = self.residual_p99_m.max(other.residual_p99_m);
        self.cover_lift_top_m = self.cover_lift_top_m.max(other.cover_lift_top_m);
        self.clearance_short_m = self.clearance_short_m.max(other.clearance_short_m);
        self.pass_short_m = self.pass_short_m.max(other.pass_short_m);
        if (other.grade_excess_m, other.grade_excess_at) > (self.grade_excess_m, self.grade_excess_at) {
            (self.grade_excess_m, self.grade_excess_at) = (other.grade_excess_m, other.grade_excess_at);
        }
    }
}

/// The ways of a solve; the ways that run on from each other as (feature, part, at its last vertex);
/// the features that are culverts; the tunnels that pass under a road or a path; and the metrics.
#[derive(Debug, Default)]
pub struct Solved {
    pub ways: Ways,
    pub joins: Vec<[(usize, usize, bool); 2]>,
    pub culverts: BTreeSet<usize>,
    pub underpasses: BTreeSet<usize>,
    pub metrics: Metrics,
}

/// The heights, the alignment and the sidewalks of the features. `near` holds the solved waterways, for
/// the covers, the decks and the beds beside a sidewalk, and the buildings, for the sidewalks.
/// `owned(xy)` marks the OSM vertices of the area of a job of `solve_by_area`: the culverts, the tunnels
/// and the metrics count only the ways that start on one of them, and the vertices from one of them to
/// the next vertex of OSM.
pub fn solve(
    features: &[&Feature],
    terrain: &dyn Terrain,
    near: &Near,
    classes: &Classes,
    owned: Option<&dyn Fn([f64; 2]) -> bool>,
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

    // Samples: k per segment from its start, and the last vertex of each way. Ordered by terrain tile,
    // so that each tile decodes about once.
    let k: Vec<usize> = length.iter().map(|l| (l / SAMPLE_M).ceil().max(1.0) as usize).collect();
    let tile = |s: usize| {
        let p = net.vertex_xy[net.segment_start[s]];
        ((p[1] * 8192.0).floor() * 8192.0 + (p[0] * 8192.0).floor()) as i64
    };
    let mut order: Vec<usize> = (0..a.len()).collect();
    order.sort_by_key(|&s| tile(s));
    let (mut seg, mut t) = (Vec::new(), Vec::new());
    for &s in &order {
        for j in 0..k[s] {
            seg.push(s);
            t.push(j as f64 / k[s] as f64);
        }
    }
    let mut end = 0;
    for w in 0..net.way_first.len() {
        end += net.way_last[w] - net.way_first[w];
        seg.push(end - 1);
        t.push(1.0);
    }
    // A bridge or a tunnel sees no terrain, unless nothing that it meets does. A bridge's end that
    // meets no other way stands on the terrain: else the smoothness carries any rise of its deck on up.
    let mut loose = vec![false; n];
    for w in (0..net.free.len()).filter(|&w| net.free[w] && !net.tunnel[w]) {
        for v in [net.way_first[w], net.way_last[w]] {
            loose[net.vertex_var[v]] = !net.junction[net.vertex_var[v]];
        }
    }
    let at_end = |i: usize| if t[i] == 0.0 { a[seg[i]] } else { b[seg[i]] };
    let mut weight: Vec<f64> = (0..seg.len())
        .map(|i| {
            let s = seg[i];
            let seen = !free[s] || loose[at_end(i)] && (t[i] == 0.0 || t[i] == 1.0);
            length[s] / k[s] as f64 * f64::from(u8::from(seen))
        })
        .collect();
    let mut h = Vec::with_capacity(seg.len());
    for chunk in (0..seg.len()).collect::<Vec<_>>().chunks(1 << 21) {
        let points: Vec<[f64; 2]> = chunk
            .iter()
            .map(|&i| {
                let v = net.segment_start[seg[i]];
                let (pa, pb) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
                [pa[0] + t[i] * (pb[0] - pa[0]), pa[1] + t[i] * (pb[1] - pa[1])]
            })
            .collect();
        h.extend(terrain.heights(&points));
    }
    let finite: Vec<bool> = h.iter().map(|h| h.is_finite()).collect();
    for i in 0..seg.len() {
        if !finite[i] {
            weight[i] = 0.0;
        }
    }
    let touched = |w: &[f64]| {
        let mut out = vec![0.0; n];
        for i in 0..seg.len() {
            out[a[seg[i]]] += w[i] * (1.0 - t[i]);
            out[b[seg[i]]] += w[i] * t[i];
        }
        out
    };
    let mut on_free = vec![false; n];
    for s in (0..a.len()).filter(|&s| free[s]) {
        on_free[a[s]] = true;
        on_free[b[s]] = true;
    }
    // A bridge or a tunnel takes its height from the terrain that it meets, not through a road in
    // unknown terrain: the networks join only vertices with terrain samples or on a bridge or tunnel.
    let support = touched(&weight);
    let linked: Vec<bool> = (0..n).map(|v| support[v] > 0.0 || on_free[v]).collect();
    let mut component: Vec<usize> = (0..n).collect();
    for s in (0..a.len()).filter(|&s| linked[a[s]] && linked[b[s]]) {
        union(&mut component, a[s], b[s]);
    }
    let component: Vec<usize> = (0..n).map(|v| rows::root(&mut component, v)).collect();
    let supported = |support: &[f64]| {
        let mut out = vec![false; n];
        for v in (0..n).filter(|&v| support[v] > 0.0) {
            out[component[v]] = true;
        }
        out
    };
    let seen = supported(&support);
    for i in 0..seg.len() {
        if free[seg[i]] && !seen[component[a[seg[i]]]] && finite[i] {
            weight[i] = length[seg[i]] / k[seg[i]] as f64 * FREE;
        }
    }
    for value in h.iter_mut().filter(|h| !h.is_finite()) {
        *value = 0.0;
    }
    // A vertex has a height where terrain samples act on it, or where it lies on a bridge or tunnel of
    // a network that has some.
    let support = touched(&weight);
    let held_network = supported(&support);
    let known: Vec<bool> = (0..n).map(|v| support[v] > 0.0 || on_free[v] && held_network[component[v]]).collect();
    let all_known = |variables: &[usize]| variables.iter().all(|&v| known[v]);

    // Rows that pin a combination of variables to a value: the approaches of side roads and the tracks
    // side by side.
    let (approach, approached, ramp) = rows::approaches(&net, classes);
    let untagged = rows::untagged_crossings(&net, rivers, classes);
    let (beside, _) = rows::parallels(&net, classes, terrain);
    let bridge: Vec<bool> = (0..net.free.len()).map(|w| net.free[w] && !net.tunnel[w]).collect();
    // The decks over what they cross and the bridges that do not sag: each ≥ its value.
    let (above, deck_ways, _) = rows::clearances(&net, rivers, classes, terrain);
    let (sags, _) = rows::chords(&net, &bridge);
    // The covers over the culverts, tagged and untagged: each ≥ its value where its way lies on the ground.
    let (cover, cover_ways, cover_untagged) = rows::covers(&net, (&near.culverts, rivers), &untagged, classes, terrain);
    let (mut k_rows, mut k_least, mut k_way, mut k_first) = (Rows::default(), Vec::new(), Vec::new(), Vec::new());
    let mut k_untagged = Vec::new();
    for i in (0..cover.values.len()).filter(|&i| all_known(&cover.variables[i])) {
        k_rows.push(cover.variables[i].into_iter().zip(cover.coefficients[i]));
        k_least.push(cover.values[i]);
        k_way.push(cover_ways[i]);
        k_first.push(cover.variables[i][0]);
        k_untagged.push(cover_untagged[i]);
    }
    let mut c_rows = Rows::default();
    let (mut least, mut deck_way, mut deck_first) = (Vec::new(), Vec::new(), Vec::new());
    for (i, &w) in deck_ways.iter().enumerate() {
        if all_known(&above.variables[i]) {
            c_rows.push(above.variables[i].into_iter().zip(above.coefficients[i]));
            least.push(above.values[i]);
            deck_way.push(Some(w));
            deck_first.push(above.variables[i][0]);
        }
    }
    for i in 0..sags.values.len() {
        if all_known(&sags.variables[i]) {
            c_rows.push(sags.variables[i].into_iter().zip(sags.coefficients[i]));
            least.push(sags.values[i]);
            deck_way.push(None);
            deck_first.push(sags.variables[i][0]);
        }
    }
    let (under, _) = rows::passes(&net, classes);
    let held_up: Vec<bool> =
        (0..under.margin.len()).map(|i| all_known(&under.upper[i].0) && all_known(&under.lower[i].0)).collect();
    let (mut u_rows, mut l_rows) = (Rows::default(), Rows::default());
    let (mut need, mut moves, mut pass_place) = (Vec::new(), Vec::new(), Vec::new());
    let mut pass_first = Vec::new();
    for i in (0..held_up.len()).filter(|&i| held_up[i]) {
        let (upper, lower) = (under.upper[i], under.lower[i]);
        u_rows.push(upper.0.into_iter().zip(upper.1));
        l_rows.push(lower.0.into_iter().zip(lower.1));
        need.push(under.margin[i]);
        moves.push(under.moves[i]);
        pass_place.push(under.place[i]);
        pass_first.push(upper.0[0]);
    }
    let mut p_rows = Rows::default();
    let (mut values, mut counted, mut is_approach) = (Vec::new(), Vec::new(), Vec::new());
    for (k, pins) in [&approach, &beside].into_iter().enumerate() {
        let mut count = 0;
        for i in 0..pins.values.len() {
            if all_known(&pins.variables[i]) {
                p_rows.push(pins.variables[i].into_iter().zip(pins.coefficients[i]));
                values.push(pins.values[i]);
                is_approach.push(k == 0);
                count += usize::from(var_mine[pins.variables[i][0]]);
            }
        }
        counted.push(count);
    }

    // Smoothness: per vertex inside a way, the change of grade over the mean of its two segments. A
    // lower class never moves a higher one: no row ties a side to the vertex of its junction.
    let mut mouth = vec![false; total];
    for (i, &o) in net.sides.occurrence.iter().enumerate() {
        let j = net.osm_position[o];
        mouth[j] = true;
        mouth[(j as i64 + net.sides.step[i]) as usize] = true;
    }
    let smooth = |rule: usize| classes.rows[rule].smooth_m.powi(4);
    let mut d2 = Rows::default();
    let mut lam = Vec::new();
    let mut add_bend = |triple: [usize; 3], la: f64, lb: f64, smooth: f64| {
        d2.push(triple.into_iter().zip([1.0 / la, -1.0 / la - 1.0 / lb, 1.0 / lb]));
        lam.push(smooth / ((la + lb) / 2.0));
    };
    for i in 0..a.len().saturating_sub(1) {
        if net.segment_way[i] == net.segment_way[i + 1]
            && known[a[i]]
            && known[b[i]]
            && known[b[i + 1]]
            && !mouth[net.segment_start[i] + 1]
        {
            add_bend([a[i], b[i], b[i + 1]], length[i], length[i + 1], smooth(net.rule[net.segment_way[i]]));
        }
    }
    // Across a junction the through road runs on from one way into the other: the vertex next to the
    // junction on each, and the junction.
    let span = |p: usize, q: usize| hypot(sub(net.vertex_xy[p], net.vertex_xy[q])) * metres(net.vertex_xy[q][1]);
    for &[(o1, s1), (o2, s2)] in &net.pairs {
        let j = net.osm_position[o1];
        let (p, q) = ((j as i64 + s1) as usize, (net.osm_position[o2] as i64 + s2) as usize);
        let across = [net.vertex_var[p], net.vertex_var[j], net.vertex_var[q]];
        if all_known(&across) {
            add_bend(across, span(p, j), span(q, j), smooth(net.rule[net.osm_way[o1]]));
        }
    }
    // Grades: per segment of a class with a limit, z_b − z_a within ± grade × length. A bridge or a
    // tunnel has the grade that its ends give it: OSM joins shafts and ramps to tunnels that no limit
    // could meet. Nor has the approach of a side road, or a segment between two approaches: the through
    // roads give them their heights. A ramp rises at most RAMP_GRADE off the grade of the terrain.
    let mut pinned = vec![false; n];
    for variables in &approach.variables {
        pinned[variables[0]] = true;
    }
    let grade = |s: usize| classes.rows[net.rule[net.segment_way[s]]].grade;
    // The terrain's rise under each ramp, which its limit centres on; a ramp without one has no limit.
    let ramps: Vec<usize> = (0..a.len()).filter(|&s| ramp[s]).collect();
    let ends: Vec<[f64; 2]> =
        ramps.iter().flat_map(|&s| [0, 1].map(|k| net.vertex_xy[net.segment_start[s] + k])).collect();
    let ground = terrain.heights(&ends);
    let mut rise = vec![f64::NAN; a.len()];
    for (k, &s) in ramps.iter().enumerate() {
        rise[s] = ground[2 * k + 1] - ground[2 * k];
    }
    let limited: Vec<usize> = (0..a.len())
        .filter(|&s| {
            (grade(s).is_finite() && !approached[s] || rise[s].is_finite())
                && !free[s]
                && known[a[s]]
                && known[b[s]]
                && !(pinned[a[s]] && pinned[b[s]])
        })
        .collect();
    // The structures: a vertex on a bridge that is no culvert or in a tunnel, but not where one ends on
    // a way on the ground or where a bridge ends that meets no way (`inner`); and those, the ends of the
    // bridges that meet no way, and the vertices that the approach of a side road puts on the surface of
    // a structure (`moved`). A culvert is a road on the ground.
    let structure = |culvert: &[bool]| -> (Vec<bool>, Vec<bool>) {
        let (mut on, mut ground) = (vec![0u8; n], vec![false; n]);
        for s in 0..a.len() {
            if free[s] && !culvert[net.segment_way[s]] {
                on[a[s]] = on[a[s]].saturating_add(1);
                on[b[s]] = on[b[s]].saturating_add(1);
            } else {
                ground[a[s]] = true;
                ground[b[s]] = true;
            }
        }
        let inner: Vec<bool> = (0..n).map(|v| (on[v] >= 2 || on[v] == 1 && !ground[v]) && !loose[v]).collect();
        let mut moved: Vec<bool> = (0..n).map(|v| inner[v] || loose[v] && on[v] > 0).collect();
        let mut grew = true;
        while grew {
            grew = false;
            for vars in &approach.variables {
                if !moved[vars[0]] && (moved[vars[1]] || moved[vars[2]]) {
                    moved[vars[0]] = true;
                    grew = true;
                }
            }
        }
        (inner, moved)
    };
    // The first stage solves the ways on the ground with each structure vertex held on the terrain: so
    // no structure moves a way on the ground. The second solves the structures between their ends.
    let (_, moved_first) = structure(&vec![false; net.free.len()]);
    // The terrain at each variable, 0 where unknown: where each starts, and where a stage that has no
    // row of it keeps it.
    let mut z = vec![0.0; n];
    {
        let mut seen = vec![false; n];
        let picked: Vec<usize> =
            (0..total).filter(|&v| !std::mem::replace(&mut seen[net.vertex_var[v]], true)).collect();
        let heights = terrain.heights(&picked.iter().map(|&v| net.vertex_xy[v]).collect::<Vec<_>>());
        for (&v, height) in picked.iter().zip(heights) {
            z[net.vertex_var[v]] = if height.is_finite() { height } else { 0.0 };
        }
    }
    let rest = z.clone();
    let (mut level_rows, mut level_w) = (Rows::default(), Vec::new());
    for s in (0..a.len()).filter(|&s| free[s] && known[a[s]] && known[b[s]]) {
        level_rows.push([(a[s], -1.0), (b[s], 1.0)]);
        level_w.push(LEVEL / length[s].max(0.01));
    }
    let mut unknown = Rows::default();
    for v in (0..n).filter(|&v| !known[v]) {
        unknown.push([(v, 1.0)]);
    }
    let zeros = |rows: &Rows| vec![0.0; rows.len()];
    let ones = |rows: &Rows| vec![1.0; rows.len()];
    let pins = vec![PIN; p_rows.len()];
    let samples = Samples { seg: &seg, t: &t, a, b, h: &h };
    let edges: Vec<(usize, usize, f64)> =
        (0..a.len()).map(|s| (a[s], b[s], if free[s] { 1e-6 } else { length[s].max(0.01) })).collect();
    let graph = Graph::new(n, &edges);
    let mut unconverged = 0;
    // The ways on the ground, without limits, and the culverts by that solve: a short bridge whose
    // straight line between its ends lies on the terrain.
    let first = |moved: &[bool], culvert: &[bool], w: &[f64]| Stage {
        fixed: vec![
            subset(&d2, &lam, &zeros(&d2), |i| !touches(&d2, i, moved)),
            subset(&level_rows, &level_w, &zeros(&level_rows), |i| !touches(&level_rows, i, moved)),
            subset(&p_rows, &pins, &values, |i| !touches(&p_rows, i, moved)),
            subset(&unknown, &ones(&unknown), &zeros(&unknown), |i| !touches(&unknown, i, moved)),
        ],
        w: (0..seg.len()).map(|i| if free[seg[i]] && !culvert[net.segment_way[seg[i]]] { 0.0 } else { w[i] }).collect(),
        ..Default::default()
    };
    let ground_vars = |moved: &[bool]| -> Vec<bool> { moved.iter().map(|&m| !m).collect() };
    let culvert_none = vec![false; net.free.len()];
    let mut bare = first(&moved_first, &culvert_none, &weight);
    bare.keep(&samples, &ground_vars(&moved_first), &rest);
    let converged;
    (z, converged) = bare.solve(&samples, &[], &z, &on_list(&ground_vars(&moved_first)));
    unconverged += usize::from(!converged);
    let w: Vec<f64> = (0..seg.len())
        .map(|i| {
            let residual = (z[a[seg[i]]] * (1.0 - t[i]) + z[b[seg[i]]] * t[i] - h[i]).abs();
            weight[i] * (HUBER_M / residual.max(1e-9)).min(1.0)
        })
        .collect();
    let chord = chords(&net, &z);
    let culvert = culverts(&net, &chord, &seg, &t, &h, &finite);
    let (inner, moved) = structure(&culvert);
    for v in (0..n).filter(|&v| !moved[v] && moved_first[v]) {
        z[v] = chord[v];
    }
    // Per cover row, whether its way holds the cover: on the ground or a culvert, and lifted by no more
    // than its most over the first solve; whether its lift passes the most; and that lift.
    let kz = k_rows.times(&z);
    let k_lift: Vec<f64> = (0..k_least.len()).map(|i| k_least[i] - kz[i]).collect();
    let on_ground = |i: usize| !net.free[k_way[i].0] || culvert[k_way[i].0];
    let holds: Vec<bool> = (0..k_least.len()).map(|i| on_ground(i) && k_lift[i] <= k_way[i].1).collect();
    let capped: Vec<bool> = (0..k_least.len()).map(|i| on_ground(i) && k_lift[i] > k_way[i].1).collect();

    // The limits of the first stage: the grades of the segments, then the ground of the samples, the
    // covers over the culverts and the ways on the ground that pass over each other. A limit holds only
    // rows of ways on the ground: the segment of a way on the ground that ends on a structure vertex is
    // free, as are steps, a ramp or a side road that OSM joins to a deck or a tunnel inside it.
    let mut ground_stage = first(&moved, &culvert, &w);
    ground_stage.keep(&samples, &ground_vars(&moved), &rest);
    let mut kinds = Vec::new();
    for (k, &s) in limited.iter().enumerate() {
        if moved[a[s]] || moved[b[s]] {
            continue;
        }
        let (bound, centre) = match grade(s) {
            g if g.is_finite() => (g * length[s], 0.0),
            _ => (RAMP_GRADE * length[s], rise[s]),
        };
        let ramp = !grade(s).is_finite();
        ground_stage.limit([(a[s], -1.0), (b[s], 1.0)], [centre - bound, centre + bound], PIN, !ramp);
        kinds.push(if ramp { Kind::Ramp } else { Kind::Grade(k) });
    }
    for i in (0..seg.len()).filter(|&i| !free[seg[i]] && finite[i]) {
        let s = seg[i];
        if known[a[s]] && known[b[s]] && !moved[a[s]] && !moved[b[s]] {
            ground_stage.limit([(a[s], 1.0 - t[i]), (b[s], t[i])], [h[i] - GROUND_M, h[i] + GROUND_M], GROUND, true);
            kinds.push(Kind::Ground(i));
        }
    }
    for i in (0..k_least.len()).filter(|&i| holds[i] && !touches(&k_rows, i, &moved)) {
        ground_stage.limit(k_rows.row(i), [k_least[i], f64::INFINITY], PIN, true);
        kinds.push(Kind::Cover(i));
    }
    let apart = |i: usize| u_rows.row(i).chain(l_rows.row(i).map(|(v, c)| (v, -c)));
    for i in (0..need.len()).filter(|&i| moves[i] && !apart(i).any(|(v, _)| moved[v])) {
        ground_stage.limit(apart(i), [need[i], f64::INFINITY], PIN, true);
        kinds.push(Kind::Pass);
    }

    let ground = ground_vars(&moved);
    let settled = ground_stage.settle(&samples, &mut z, &ground, Some(&graph));
    let mut rounds = settled.rounds;
    let (mut unsettled, mut held) = (settled.unsettled, settled.held);
    unconverged += settled.unconverged;
    // Then the ramps, with every vertex fixed but those of the ramps' sides past their approach and
    // before any junction: a ramp bends its side and never moves the road that it leaves.
    let mut ramp_region = vec![false; n];
    for (k, kind) in kinds.iter().enumerate() {
        if matches!(kind, Kind::Ramp) {
            ground_stage.open[k] = true;
            for (v, _) in ground_stage.limits.row(k) {
                ramp_region[v] = !net.junction.get(v).copied().unwrap_or(false) && !pinned[v];
            }
        }
    }
    if ramp_region.iter().any(|&r| r) {
        let settled = ground_stage.settle(&samples, &mut z, &ramp_region, None);
        (rounds, unsettled, held) = (rounds + settled.rounds, unsettled + settled.unsettled, settled.held);
        unconverged += settled.unconverged;
    }

    // The second stage: the structures between their ends on the ground, the ways on the ground and the
    // ends of the bridges that meet no way fixed on the terrain. Each shapes itself, and its fixed ends
    // hold only its height, not its grade: its smoothing at its own vertices, its preference for level,
    // and a tunnel the straight line between the ends of its chain, which holds it as the terrain holds
    // a way on the ground. Its limits are of one side and leave room, as a deck can always rise and a
    // tunnel sink: the decks over what they cross, the bridges that do not sag, and the tunnels under
    // what crosses over them and under the terrain. From an end that does not move, a structure rises
    // or sinks to a limit no more steeply than 1:1: a limit that asks more lies where it lands, and has
    // no row (`rows::clearances` holds this for the decks).
    let moving: Vec<bool> = (0..n).map(|v| moved[v] && !loose[v]).collect();
    let (bores, bore_m) = rows::chords(&net, &net.tunnel);
    let mut b_rows = Rows::default();
    let mut b_w = Vec::new();
    for i in (0..bores.values.len()).filter(|&i| all_known(&bores.variables[i])) {
        b_rows.push(bores.variables[i].into_iter().zip(bores.coefficients[i]));
        b_w.push(bore_m[i]);
    }
    let mut structures = Stage {
        fixed: vec![
            subset(&d2, &lam, &zeros(&d2), |i| d2.row(i).nth(1).is_some_and(|(v, _)| inner[v])),
            subset(&level_rows, &level_w, &zeros(&level_rows), |i| touches(&level_rows, i, &moving)),
            subset(&p_rows, &pins, &values, |i| is_approach[i] && touches(&p_rows, i, &moving)),
            subset(&unknown, &ones(&unknown), &zeros(&unknown), |i| touches(&unknown, i, &moving)),
            subset(&b_rows, &b_w, &zeros(&b_rows), |i| touches(&b_rows, i, &moving)),
        ],
        w: (0..seg.len()).map(|i| if free[seg[i]] && !culvert[net.segment_way[seg[i]]] { w[i] } else { 0.0 }).collect(),
        ..Default::default()
    };
    structures.keep(&samples, &moving, &rest);
    for i in (0..least.len()).filter(|&i| touches(&c_rows, i, &moving)) {
        structures.limit(c_rows.row(i), [least[i], f64::INFINITY], PIN, true);
    }
    // Whether a tunnel at its vertex v, share t of the way to the next, reaches a height of `top` or
    // under from each fixed end of its chain.
    let ends = rows::chain_ends(&net, &net.tunnel);
    let reaches = |v: usize, t: f64, top: f64| match (ends[v], ends[v + 1]) {
        (Some(p), Some(q)) => (0..2).all(|k| moving[p[k].0] || z[p[k].0] - top <= (1.0 - t) * p[k].1 + t * q[k].1),
        _ => true,
    };
    let uz = u_rows.times(&z);
    for i in 0..need.len() {
        if moves[i] && apart(i).any(|(v, _)| moving[v]) {
            structures.limit(apart(i), [need[i], f64::INFINITY], PIN, true);
        } else if !moves[i]
            && l_rows.row(i).any(|(v, _)| moving[v])
            && reaches(pass_place[i].0, pass_place[i].1, uz[i] - need[i])
        {
            // A road on the ground over a tunnel keeps its height.
            structures.limit(l_rows.row(i).map(|(v, c)| (v, -c)), [need[i] - uz[i], f64::INFINITY], PIN, true);
        }
    }
    for i in (0..seg.len()).filter(|&i| net.tunnel[net.segment_way[seg[i]]] && finite[i]) {
        let (s, v) = (seg[i], net.segment_start[seg[i]]);
        if known[a[s]] && known[b[s]] && (moving[a[s]] || moving[b[s]]) && reaches(v, t[i], h[i]) {
            structures.limit([(a[s], 1.0 - t[i]), (b[s], t[i])], [f64::NEG_INFINITY, h[i]], PIN, true);
        }
    }
    let converged;
    (z, converged) = structures.solve(&samples, &[], &z, &on_list(&moving));
    unconverged += usize::from(!converged);
    let steps = structures.interior(&samples, &mut z, &moving);
    rounds += steps.rounds;
    unsettled += steps.unsettled;
    unconverged += steps.unconverged;
    let lifted: Vec<bool> = {
        let mut out = vec![false; k_least.len()];
        for (k, kind) in kinds.iter().enumerate() {
            if let Kind::Cover(i) = *kind {
                out[i] = held[k] != 0;
            }
        }
        out
    };

    // A clearance holds on the curve only where its vertices stay in the tiles.
    let mut stay = vec![false; n];
    let crossed = above.variables.iter().flatten().chain(under.upper.iter().chain(&under.lower).flat_map(|row| &row.0));
    crossed.for_each(|&v| stay[v] = true);
    let covered = cover.variables.iter().filter(|row| all_known(&row[..])).zip(&lifted);
    covered.filter(|(_, &up)| up).flat_map(|(row, _)| row).for_each(|&v| stay[v] = true);
    let shores: HashSet<[u64; 2]> = rivers.iter().flat_map(|river| &river.points).map(|&p| key(p)).collect();
    let mut water = vec![false; n];
    for (o, &xy) in net.osm_xy.iter().enumerate() {
        water[net.osm_var[o]] = shores.contains(&key(xy));
    }
    let ground: Vec<bool> = (0..culvert.len()).map(|w| !net.free[w] || culvert[w]).collect();
    // A pipe runs under the sidewalks only where its row holds the cover.
    let piped: Vec<rows::UntaggedCrossing> =
        (0..k_least.len()).filter(|&i| holds[i]).filter_map(|i| Some(untagged[k_untagged[i]?])).collect();
    let town = Town::new(&net, (rivers, &near.culverts), (&piped, classes), near.buildings, terrain);
    let (ways, joins, walks) = align::align(&net, &z, &known, &stay, &water, &ground, classes, &mine, &town);
    for (way, &(f, p)) in ways.into_iter().zip(&net.way_feature) {
        solved.ways[f][p] = Some(way);
    }
    let named = |(w, end): (usize, bool)| (net.way_feature[w].0, net.way_feature[w].1, end);
    solved.joins = joins.into_iter().map(|join| join.map(named)).collect();
    let way_mine: Vec<bool> = net.way_first.iter().map(|&v| mine[v]).collect();
    solved.culverts = (0..culvert.len()).filter(|&w| culvert[w] && way_mine[w]).map(|w| net.way_feature[w].0).collect();
    let cz = c_rows.times(&z);
    let deck_mine: Vec<bool> =
        (0..least.len()).map(|i| deck_way[i].is_some_and(|w| !culvert[w]) && var_mine[deck_first[i]]).collect();
    let (uz, lz) = (u_rows.times(&z), l_rows.times(&z));
    let pass_mine: Vec<bool> = pass_first.iter().map(|&v| var_mine[v]).collect();
    let excess: Vec<(f64, usize)> = limited
        .iter()
        .filter(|&&s| segment_mine[s] && grade(s).is_finite())
        .map(|&s| (((z[b[s]] - z[a[s]]).abs() - grade(s) * length[s]), s))
        .collect();
    let worst = excess.iter().fold(None, |best: Option<(f64, usize)>, &(e, s)| match best {
        Some((b, _)) if b >= e => best,
        _ => Some((e, s)),
    });
    let held_of = |want: &dyn Fn(Kind) -> Option<usize>, mine: &dyn Fn(usize) -> bool| {
        kinds.iter().zip(&held).filter(|&(&kind, &state)| state != 0 && want(kind).is_some_and(mine)).count()
    };
    let (mut tunnel_at, mut ground_at) = (vec![false; n], vec![false; n]);
    for s in 0..a.len() {
        let at = if net.tunnel[net.segment_way[s]] {
            &mut tunnel_at
        } else if !free[s] {
            &mut ground_at
        } else {
            continue;
        };
        at[a[s]] = true;
        at[b[s]] = true;
    }
    let mut residual: Vec<f64> = (0..seg.len())
        .filter(|&i| !free[seg[i]] && weight[i] > 0.0 && segment_mine[seg[i]])
        .map(|i| (z[a[seg[i]]] * (1.0 - t[i]) + z[b[seg[i]]] * t[i] - h[i]).abs())
        .collect();
    let count = |mask: &mut dyn Iterator<Item = bool>| mask.filter(|&m| m).count();
    solved.metrics = Metrics {
        ways: count(&mut way_mine.iter().copied()),
        variables: count(&mut var_mine.iter().copied()),
        known: count(&mut (0..n).map(|v| known[v] && var_mine[v])),
        approaches: counted[0],
        culverts: solved.culverts.len(),
        rounds,
        held: held_of(&|k| if let Kind::Grade(i) = k { Some(limited[i]) } else { None }, &|s| segment_mine[s]),
        grounded: held_of(&|k| if let Kind::Ground(i) = k { Some(seg[i]) } else { None }, &|s| segment_mine[s]),
        unsettled,
        tunnel_joins: count(&mut (0..n).map(|v| inner[v] && tunnel_at[v] && ground_at[v] && var_mine[v])),
        clearances: count(&mut deck_mine.iter().copied()),
        clearances_low_cm: count(&mut (0..least.len()).map(|i| cz[i] < least[i] - 0.01 && deck_mine[i])),
        clearance_short_m: (0..least.len()).filter(|&i| deck_mine[i]).map(|i| least[i] - cz[i]).fold(0.0, f64::max),
        passes: count(&mut pass_mine.iter().copied()),
        passes_near_cm: count(&mut (0..need.len()).map(|i| uz[i] - lz[i] < need[i] - 0.01 && pass_mine[i])),
        pass_short_m: (0..need.len()).filter(|&i| pass_mine[i]).map(|i| need[i] - uz[i] + lz[i]).fold(0.0, f64::max),
        grade_excess_m: worst.map_or(0.0, |(e, _)| e.max(0.0)),
        grade_excess_at: worst.map_or([0.0, 0.0], |(_, s)| degrees(net.vertex_xy[net.segment_start[s]])),
        grade_excess_over_1cm: excess.iter().filter(|e| e.0 > 0.01).count(),
        residual_p99_m: percentile(&mut residual, 99.0).unwrap_or(0.0),
        refinements_unconverged: unconverged,
        covers: count(&mut (0..k_least.len()).map(|i| holds[i] && var_mine[k_first[i]])),
        covers_lifted: count(&mut (0..k_least.len()).map(|i| lifted[i] && var_mine[k_first[i]])),
        covers_untagged: count(
            &mut (0..k_least.len()).map(|i| holds[i] && k_untagged[i].is_some() && var_mine[k_first[i]]),
        ),
        covers_untagged_lifted: count(
            &mut (0..k_least.len()).map(|i| lifted[i] && k_untagged[i].is_some() && var_mine[k_first[i]]),
        ),
        covers_capped: count(
            &mut (0..k_least.len()).map(|i| capped[i] && k_untagged[i].is_none() && var_mine[k_first[i]]),
        ),
        covers_untagged_capped: count(
            &mut (0..k_least.len()).map(|i| capped[i] && k_untagged[i].is_some() && var_mine[k_first[i]]),
        ),
        cover_lift_top_m: (0..k_least.len())
            .filter(|&i| lifted[i] && var_mine[k_first[i]])
            .map(|i| k_lift[i])
            .fold(0.0, f64::max),
        sidewalks: walks,
    };
    solved.underpasses = (0..held_up.len())
        .filter(|&i| held_up[i] && under.underpass[i] && way_mine[under.lower_way[i]])
        .map(|i| net.way_feature[under.lower_way[i]].0)
        .collect();
    solved
}

/// What a limit of the first stage is, for the metrics: the grade of a segment (its place in
/// `limited`), the ground at a sample, the cover of a row, a pass, or a ramp.
#[derive(Clone, Copy)]
enum Kind {
    Grade(usize),
    Ground(usize),
    Cover(usize),
    Pass,
    Ramp,
}

/// The terrain samples: per sample its segment, its place along it and the terrain there.
struct Samples<'a> {
    seg: &'a [usize],
    t: &'a [f64],
    a: &'a [usize],
    b: &'a [usize],
    h: &'a [f64],
}

/// A stage of the solve: the rows (A, w, c) of the least squares Σ w·(A·z − c)² that it always holds,
/// the weights of the terrain samples, and its limits: rows A·z that it keeps from lo to hi, where
/// open. A limit adds weight × (the distance of A·z from its band)², so it only pushes, from outside.
#[derive(Default)]
struct Stage {
    fixed: Vec<(Rows, Vec<f64>, Vec<f64>)>,
    w: Vec<f64>,
    limits: Rows,
    lo: Vec<f64>,
    hi: Vec<f64>,
    weight: Vec<f64>,
    open: Vec<bool>,
}

/// The rounds of a stage: per limit, its state at their end; their count; the limits whose state the
/// last of ROUNDS still changed; and the solves that did not converge.
struct Settled {
    held: Vec<i8>,
    rounds: usize,
    unsettled: usize,
    unconverged: usize,
}

impl Stage {
    /// A row at `rest` for each variable that `moves` marks and that no row of the stage holds.
    fn keep(&mut self, samples: &Samples, moves: &[bool], rest: &[f64]) {
        let mut held = vec![false; moves.len()];
        for (rows, w, _) in &self.fixed {
            for i in (0..rows.len()).filter(|&i| w[i] > 0.0) {
                rows.row(i).for_each(|(v, _)| held[v] = true);
            }
        }
        for i in (0..samples.seg.len()).filter(|&i| self.w[i] > 0.0) {
            held[samples.a[samples.seg[i]]] |= samples.t[i] < 1.0;
            held[samples.b[samples.seg[i]]] |= samples.t[i] > 0.0;
        }
        let mut rows = Rows::default();
        let free: Vec<usize> = (0..moves.len()).filter(|&v| moves[v] && !held[v]).collect();
        free.iter().for_each(|&v| rows.push([(v, 1.0)]));
        let c = free.iter().map(|&v| rest[v]).collect();
        self.fixed.push((rows, vec![1.0; free.len()], c));
    }

    fn limit(&mut self, row: impl IntoIterator<Item = (usize, f64)>, [lo, hi]: [f64; 2], weight: f64, open: bool) {
        self.limits.push(row);
        self.lo.push(lo);
        self.hi.push(hi);
        self.weight.push(weight);
        self.open.push(open);
    }

    /// Per limit: 1 past its top, −1 under its bottom, else 0.
    fn state(&self, z: &[f64]) -> Vec<i8> {
        let gz = self.limits.times(z);
        (0..gz.len())
            .map(|i| match gz[i] {
                _ if !self.open[i] => 0,
                g if g > self.hi[i] => 1,
                g if g < self.lo[i] => -1,
                _ => 0,
            })
            .collect()
    }

    /// The least squares with each limit that `held` marks at the bound that it passes, the variables
    /// `moved` moving from z.
    fn solve(&self, samples: &Samples, held: &[i8], z: &[f64], moved: &[usize]) -> (Vec<f64>, bool) {
        if moved.is_empty() {
            return (z.to_vec(), true);
        }
        let on: Vec<bool> = (0..self.lo.len()).map(|i| held.get(i).is_some_and(|&x| x != 0)).collect();
        let rows = self.limits.select(&on);
        let c: Vec<f64> =
            (0..on.len()).filter(|&i| on[i]).map(|i| if held[i] > 0 { self.hi[i] } else { self.lo[i] }).collect();
        let w: Vec<f64> = (0..on.len()).filter(|&i| on[i]).map(|i| self.weight[i]).collect();
        let mut terms: Vec<Term> =
            self.fixed.iter().map(|(rows, w, c)| Term { block: Block::Rows(rows), w, c }).collect();
        let (seg, t, a, b) = (samples.seg, samples.t, samples.a, samples.b);
        terms.push(Term { block: Block::Samples { seg, t, a, b }, w: &self.w, c: samples.h });
        terms.push(Term { block: Block::Rows(&rows), w: &w, c: &c });
        fit(&terms, z.len(), Some((z, moved)))
    }

    /// Rounds of the limits from z, the variables that `moves` marks moving. A round holds each limit
    /// that z passes at the bound that it passes, solves, and steps from z towards that solve as far as
    /// the least squares with the limits falls: it falls each round, so the rounds cannot cycle, and
    /// where no limit changes they end at its least, which does not depend on their path. The first
    /// round solves all; with a graph, each later one the network within REGION_M of the limits that
    /// changed and of a step that stopped short.
    fn settle(&self, samples: &Samples, z: &mut [f64], moves: &[bool], graph: Option<&Graph>) -> Settled {
        let all: Vec<usize> = (0..z.len()).filter(|&v| moves[v]).collect();
        let mut used = vec![0i8; self.lo.len()];
        let mut held = self.state(z);
        let (mut rounds, mut unconverged, mut short) = (0, 0, Vec::new());
        while rounds < ROUNDS {
            let changed: Vec<usize> = (0..held.len()).filter(|&i| held[i] != used[i]).collect();
            if changed.is_empty() && rounds > 0 {
                break;
            }
            let region: Vec<usize> = match graph {
                Some(graph) if rounds > 0 => {
                    let rows = changed.iter().flat_map(|&i| self.limits.row(i).map(|(v, _)| v));
                    let distance = graph.distances(rows.chain(short.iter().copied()).map(|v| (v, 0.0)), REGION_M);
                    all.iter().copied().filter(|&v| distance[v].is_finite()).collect()
                }
                _ => all.clone(),
            };
            rounds += 1;
            let (next, converged) = self.solve(samples, &held, z, &region);
            unconverged += usize::from(!converged);
            let d: Vec<f64> = next.iter().zip(z.iter()).map(|(p, q)| p - q).collect();
            let alpha = self.step(samples, z, &d);
            for &v in &region {
                z[v] += alpha * d[v];
            }
            short = if alpha < 1.0 { region } else { Vec::new() };
            used = std::mem::replace(&mut held, self.state(z));
        }
        let unsettled = (0..held.len()).filter(|&i| held[i] != used[i]).count();
        Settled { held, rounds, unsettled, unconverged }
    }

    /// The least squares with its open limits as hard bounds, the variables that `moves` marks moving
    /// from z: a primal-dual interior point method. It reaches the least from any start, without rounds
    /// of limits that hold and let go. The limits must leave room. It ends where a step moves no height
    /// by SETTLED_M. Its rounds are its steps; a bound that is still passed by more than SETTLED_M after
    /// STEPS is unsettled.
    fn interior(&self, samples: &Samples, z: &mut [f64], moves: &[bool]) -> Settled {
        let moved = on_list(moves);
        // One bound per side of an open limit: σ·(G·z) − d = s ≥ 0, with its multiplier λ ≥ 0.
        let (mut rows, mut d) = (Rows::default(), Vec::new());
        for j in (0..self.lo.len()).filter(|&j| self.open[j]) {
            for (sigma, bound) in [(1.0, self.lo[j]), (-1.0, -self.hi[j])] {
                if bound.is_finite() {
                    rows.push(self.limits.row(j).map(|(v, c)| (v, sigma * c)));
                    d.push(bound);
                }
            }
        }
        let m = d.len();
        let mut settled = Settled { held: vec![0; self.lo.len()], rounds: 0, unsettled: 0, unconverged: 0 };
        if m == 0 || moved.is_empty() {
            return settled;
        }
        let mut g = rows.times(z);
        let mut s: Vec<f64> = (0..m).map(|j| (g[j] - d[j]).max(SLACK_M)).collect();
        let mut lambda = vec![1.0; m];
        let mut moving = f64::INFINITY;
        while settled.rounds < STEPS {
            let gap = s.iter().zip(&lambda).map(|(s, l)| s * l).sum::<f64>() / m as f64;
            let rp: Vec<f64> = (0..m).map(|j| g[j] - s[j] - d[j]).collect();
            if gap < GAP && moving < SETTLED_M && rp.iter().all(|r| r.abs() < SETTLED_M) {
                break;
            }
            settled.rounds += 1;
            let mu = CENTRE * gap;
            let weight: Vec<f64> = (0..m).map(|j| lambda[j] / s[j]).collect();
            let target: Vec<f64> = (0..m).map(|j| d[j] + s[j] + mu / lambda[j]).collect();
            let mut terms: Vec<Term> =
                self.fixed.iter().map(|(rows, w, c)| Term { block: Block::Rows(rows), w, c }).collect();
            let (seg, t, a, b) = (samples.seg, samples.t, samples.a, samples.b);
            terms.push(Term { block: Block::Samples { seg, t, a, b }, w: &self.w, c: samples.h });
            terms.push(Term { block: Block::Rows(&rows), w: &weight, c: &target });
            let (next, converged) = fit(&terms, z.len(), Some((z, &moved)));
            settled.unconverged += usize::from(!converged);
            let dz: Vec<f64> = next.iter().zip(z.iter()).map(|(p, q)| p - q).collect();
            let cdz = rows.times(&dz);
            let ds: Vec<f64> = (0..m).map(|j| rp[j] + cdz[j]).collect();
            let dl: Vec<f64> = (0..m).map(|j| (mu - lambda[j] * s[j] - lambda[j] * ds[j]) / s[j]).collect();
            let reach = |x: &[f64], dx: &[f64]| {
                (0..m).filter(|&j| dx[j] < 0.0).map(|j| -x[j] / dx[j]).fold(1.0 / BOUNDARY, f64::min) * BOUNDARY
            };
            // One step for both: the stationarity couples the heights and the multipliers.
            let step = reach(&s, &ds).min(reach(&lambda, &dl));
            moving = 0.0;
            for &v in &moved {
                z[v] += step * dz[v];
                moving = f64::max(moving, (step * dz[v]).abs());
            }
            for j in 0..m {
                s[j] += step * ds[j];
                lambda[j] += step * dl[j];
            }
            g = rows.times(z);
        }
        settled.unsettled = (0..m).filter(|&j| g[j] - d[j] < -SETTLED_M).count();
        settled
    }

    /// The step along d from z, from 0 to 1, to the least of the least squares with the limits on that
    /// line. On a line it is convex: its slope rises, and a bisection finds where it turns.
    fn step(&self, samples: &Samples, z: &[f64], d: &[f64]) -> f64 {
        let (mut p, mut q) = (0.0, 0.0);
        let mut add = |r: f64, s: f64, w: f64| {
            p += w * r * s;
            q += w * s * s;
        };
        for (rows, w, c) in &self.fixed {
            let (rz, rd) = (rows.times(z), rows.times(d));
            for i in (0..rz.len()).filter(|&i| rd[i] != 0.0) {
                add(rz[i] - c[i], rd[i], w[i]);
            }
        }
        for i in 0..samples.seg.len() {
            let (va, vb, t) = (samples.a[samples.seg[i]], samples.b[samples.seg[i]], samples.t[i]);
            let s = d[va] * (1.0 - t) + d[vb] * t;
            if s != 0.0 {
                add(z[va] * (1.0 - t) + z[vb] * t - samples.h[i], s, self.w[i]);
            }
        }
        let (gz, gd) = (self.limits.times(z), self.limits.times(d));
        let moving: Vec<usize> = (0..gz.len()).filter(|&i| self.open[i] && gd[i] != 0.0).collect();
        let slope = |alpha: f64| {
            moving.iter().fold(p + alpha * q, |sum, &i| {
                let g = gz[i] + alpha * gd[i];
                let past = if g > self.hi[i] {
                    g - self.hi[i]
                } else if g < self.lo[i] {
                    g - self.lo[i]
                } else {
                    0.0
                };
                sum + self.weight[i] * gd[i] * past
            })
        };
        if slope(1.0) <= 0.0 {
            return 1.0;
        }
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..BISECTIONS {
            let middle = 0.5 * (low + high);
            if slope(middle) > 0.0 {
                high = middle;
            } else {
                low = middle;
            }
        }
        low
    }
}

/// The rows of (A, w, c) that `keep` marks.
fn subset(rows: &Rows, w: &[f64], c: &[f64], keep: impl Fn(usize) -> bool) -> (Rows, Vec<f64>, Vec<f64>) {
    let mask: Vec<bool> = (0..rows.len()).map(keep).collect();
    let pick = |x: &[f64]| (0..x.len()).filter(|&i| mask[i]).map(|i| x[i]).collect();
    (rows.select(&mask), pick(w), pick(c))
}

/// Whether row i of `rows` holds a variable that `mask` marks.
fn touches(rows: &Rows, i: usize, mask: &[bool]) -> bool {
    rows.row(i).any(|(v, _)| mask[v])
}

fn on_list(mask: &[bool]) -> Vec<usize> {
    (0..mask.len()).filter(|&v| mask[v]).collect()
}

/// Per variable, its height in z; inside a bridge, the straight line between the heights of its ends
/// in z, by its share of the bridge's length.
fn chords(net: &Network, z: &[f64]) -> Vec<f64> {
    let mut out = z.to_vec();
    for w in (0..net.free.len()).filter(|&w| net.free[w] && !net.tunnel[w]) {
        let (first, last) = (net.way_first[w], net.way_last[w]);
        let mut along = vec![0.0];
        for v in first..last {
            along.push(along[along.len() - 1] + hypot(sub(net.vertex_xy[v + 1], net.vertex_xy[v])));
        }
        let total = along[along.len() - 1].max(1e-300);
        let (za, zb) = (z[net.vertex_var[first]], z[net.vertex_var[last]]);
        for v in first + 1..last {
            out[net.vertex_var[v]] = za + (zb - za) * along[v - first] / total;
        }
    }
    out
}

/// Per way, whether it is a culvert: a bridge shorter than CULVERT_M between ways on the ground, no
/// other bridge or tunnel at its ends, with the deck no more than CULVERT_RISE_M above the terrain
/// under it.
fn culverts(net: &Network, z: &[f64], seg: &[usize], t: &[f64], h: &[f64], finite: &[bool]) -> Vec<bool> {
    let ways = net.free.len();
    let mut way_length = vec![0.0; ways];
    for (s, &w) in net.segment_way.iter().enumerate() {
        way_length[w] += net.length[s];
    }
    let mut structures = vec![0u8; net.count];
    for o in (0..net.osm_xy.len()).filter(|&o| net.free[net.osm_way[o]]) {
        structures[net.osm_var[o]] = structures[net.osm_var[o]].saturating_add(1);
    }
    let alone = |w: usize| [net.way_first[w], net.way_last[w]].iter().all(|&v| structures[net.vertex_var[v]] == 1);
    let short: Vec<bool> =
        (0..ways).map(|w| net.free[w] && !net.tunnel[w] && way_length[w] < CULVERT_M && alone(w)).collect();
    let mut rise = vec![f64::NEG_INFINITY; ways];
    for i in 0..seg.len() {
        let w = net.segment_way[seg[i]];
        if short[w] && finite[i] {
            let deck = z[net.a[seg[i]]] * (1.0 - t[i]) + z[net.b[seg[i]]] * t[i];
            rise[w] = rise[w].max(deck - h[i]);
        }
    }
    (0..ways).map(|w| short[w] && rise[w] > f64::NEG_INFINITY && rise[w] <= CULVERT_RISE_M).collect()
}
