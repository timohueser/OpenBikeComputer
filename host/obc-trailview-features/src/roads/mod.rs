//! The road solver: a height on each vertex of the roads, paths, railways, tunnels and galleries,
//! from the master terrain, by the rules per class of `data/trailview/roads.json`.
//!
//! A way is sampled every SAMPLE_M metres. Its heights minimise Σ wᵢ(zᵢ − hᵢ)² + λ·Σ Δgrade² / s,
//! with Huber weights wᵢ, so that a spike or a gully of the terrain does not move the road, and
//! λ = smooth_m⁴ of its class. No segment is steeper than the grade of its class. Ways that share a
//! vertex share its height, and a side road leaves a junction on the surface of the through road. A
//! bridge or a tunnel ignores the terrain under it and runs between its ends, a tunnel in a straight
//! line; a bridge never sags under the straight line between its ends, and rises where it would pass
//! too low over the river, the road, the path or the track that it crosses; a short bridge that lies
//! on the terrain is a culvert and does not. A tunnel sinks under a road, a path or a track that
//! crosses over it, which does not move. Of two bridges that cross, the one of the higher layer
//! passes over the other, at one layer the better class, and of one class the later one. A ford lies
//! FORD_M below the water it crosses, and a road, a track or a railway on the ground lies over the cover
//! of a culvert part or of narrow water that it crosses. Tracks side by side, as OSM maps each track of a double line,
//! lie at one height: their beds are one bed. A track on a ramp beside another keeps its own height.
//! Each solve ends with the alignment of its ways (`align`): a fitted plan, and the vertices that hold it.

mod align;
#[cfg(test)]
mod align_tests;
mod area;
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
use crate::{metres, Plan, Terrain};

const SAMPLE_M: f64 = 2.0;
/// While it solves, a way has a vertex at least every VERTEX_M.
const VERTEX_M: f64 = 10.0;
/// A side road lies on the surface of the through road for this far from their junction; a path
/// that meets a path, for APPROACH_PATH_M.
const APPROACH_M: f64 = 4.0;
const APPROACH_PATH_M: f64 = 1.5;
const HUBER_M: f64 = 1.0;
pub const FORD_M: f64 = 0.2;
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
/// A track within PARALLEL_M beside another, and within PARALLEL_COS of its direction, lies at its
/// height, unless the terrain under the two differs by more than PARALLEL_RISE_M within
/// PARALLEL_FREE_M along it: a ramp or a flyover, whose climb the smoothing of a track spreads that far.
const PARALLEL_M: f64 = 6.0;
const PARALLEL_COS: f64 = 0.9;
const PARALLEL_RISE_M: f64 = 0.5;
const PARALLEL_FREE_M: f64 = 100.0;
/// Weights against one metre of terrain samples: the terrain under a bridge or tunnel that meets no
/// road, a row that pins a junction approach, a ford or a slope limit, and per metre of a bridge or
/// tunnel the preference for level, so that one that meets a road at one end only is level from there.
const FREE: f64 = 1e-4;
const PIN: f64 = 1e8;
const LEVEL: f64 = 1e-3;
/// GMRES refines each solve to REFINE_M, in at most REFINE_STEPS steps.
const REFINE_M: f64 = 1e-6;
const REFINE_STEPS: usize = 30;
/// Solves of the whole network, the first ones with the Huber weights of the solve before. Then
/// rounds of the slope limits solve only the network within REGION_M of the segments whose limit
/// changed, and the bridges and tunnels it meets: the terrain anchors the rest.
const SOLVES: usize = 2;
const ROUNDS: usize = 40;
const REGION_M: f64 = 300.0;
/// The grade excess in metres that a segment may have before it holds its limit.
const TOLERANCE_M: f64 = 1e-3;
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
    pub ford: bool,
    /// A path over stepping stones: no ford, it keeps its height.
    pub stones: bool,
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
    pub fords: usize,
    pub culverts: usize,
    pub rounds: usize,
    pub held: usize,
    pub clearances: usize,
    pub clearances_low_cm: usize,
    pub passes: usize,
    pub passes_near_cm: usize,
    pub grade_excess_m: f64,
    /// Where the grade excess is largest: latitude, longitude.
    pub grade_excess_at: [f64; 2],
    pub grade_excess_over_1cm: usize,
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
    /// The metrics of two jobs: the counts add up; `rounds`, `residual_p99_m` and `cover_lift_top_m` are
    /// the largest, and so is the grade excess, with its place.
    pub fn add(&mut self, other: &Metrics) {
        let sums = [
            (&mut self.ways, other.ways),
            (&mut self.variables, other.variables),
            (&mut self.known, other.known),
            (&mut self.approaches, other.approaches),
            (&mut self.fords, other.fords),
            (&mut self.culverts, other.culverts),
            (&mut self.held, other.held),
            (&mut self.clearances, other.clearances),
            (&mut self.clearances_low_cm, other.clearances_low_cm),
            (&mut self.passes, other.passes),
            (&mut self.passes_near_cm, other.passes_near_cm),
            (&mut self.grade_excess_over_1cm, other.grade_excess_over_1cm),
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
/// the fords, the decks and the beds beside a sidewalk, and the buildings, for the sidewalks.
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

    // Rows that pin a combination of variables to a value: the approaches of side roads, the fords,
    // the bores, the tracks side by side.
    let (approach, approached) = rows::approaches(&net);
    let fords = rows::fords(&net, rivers, classes);
    let untagged = rows::untagged_crossings(&net, rivers, classes);
    let bores = rows::chains(&net, &net.tunnel);
    let beside = rows::parallels(&net, classes, terrain);
    let bridge: Vec<bool> = (0..net.free.len()).map(|w| net.free[w] && !net.tunnel[w]).collect();
    // The decks over what they cross and the bridges that do not sag: each ≥ its value.
    let (above, deck_ways) = rows::clearances(&net, rivers, classes, terrain);
    let sags = rows::chains(&net, &bridge);
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
    // Per row, whether its way holds the cover: on the ground or a culvert, and lifted by no more than its
    // most; whether the cover lifts it; whether its lift passes the most; and its lift over the first solve.
    let (mut holds, mut lifted) = (vec![false; k_least.len()], vec![false; k_least.len()]);
    let (mut capped, mut k_lift) = (vec![false; k_least.len()], vec![0.0; k_least.len()]);
    // The culverts by the first solve: the cover that lifts one keeps it a culvert.
    let mut culvert = vec![false; net.free.len()];
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
    let under = rows::passes(&net, classes);
    let held_up: Vec<bool> =
        (0..under.margin.len()).map(|i| all_known(&under.upper[i].0) && all_known(&under.lower[i].0)).collect();
    let (mut u_rows, mut l_rows, mut apart) = (Rows::default(), Rows::default(), Rows::default());
    let (mut need, mut moves) = (Vec::new(), Vec::new());
    let mut pass_first = Vec::new();
    for i in (0..held_up.len()).filter(|&i| held_up[i]) {
        let (upper, lower) = (under.upper[i], under.lower[i]);
        u_rows.push(upper.0.into_iter().zip(upper.1));
        l_rows.push(lower.0.into_iter().zip(lower.1));
        let lifted = upper.0.into_iter().zip(upper.1).filter(|_| under.moves[i]);
        apart.push(lifted.chain(lower.0.into_iter().zip(lower.1.map(|c| -c))));
        need.push(under.margin[i]);
        moves.push(under.moves[i]);
        pass_first.push(upper.0[0]);
    }
    let mut p_rows = Rows::default();
    let (mut values, mut counted) = (Vec::new(), Vec::new());
    for pins in [&approach, &fords, &bores, &beside] {
        let mut count = 0;
        for i in 0..pins.values.len() {
            if all_known(&pins.variables[i]) {
                p_rows.push(pins.variables[i].into_iter().zip(pins.coefficients[i]));
                values.push(pins.values[i]);
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
    // roads give them their heights.
    let mut pinned = vec![false; n];
    for variables in &approach.variables {
        pinned[variables[0]] = true;
    }
    let grade = |s: usize| classes.rows[net.rule[net.segment_way[s]]].grade;
    let limited: Vec<usize> = (0..a.len())
        .filter(|&s| {
            grade(s).is_finite()
                && !free[s]
                && known[a[s]]
                && known[b[s]]
                && !approached[s]
                && !(pinned[a[s]] && pinned[b[s]])
        })
        .collect();
    let mut g_rows = Rows::default();
    for &s in &limited {
        g_rows.push([(a[s], -1.0), (b[s], 1.0)]);
    }
    let bound: Vec<f64> = limited.iter().map(|&s| grade(s) * length[s]).collect();
    let (mut level_rows, mut level_w) = (Rows::default(), Vec::new());
    for s in (0..a.len()).filter(|&s| free[s] && known[a[s]] && known[b[s]]) {
        level_rows.push([(a[s], -1.0), (b[s], 1.0)]);
        level_w.push(LEVEL / length[s].max(0.01));
    }
    let mut unknown = Rows::default();
    for v in (0..n).filter(|&v| !known[v]) {
        unknown.push([(v, 1.0)]);
    }
    let zeros = vec![0.0; unknown.len()];
    // The rows (A, w, c) of the least squares Σ w·(A·z − c)² that every solve holds.
    let mut fixed =
        vec![level(d2, lam), level(level_rows, level_w), held_at(p_rows, PIN, values), held_at(unknown, 1.0, zeros)];
    let mut unconverged = 0;
    let solve = |fixed: &[(Rows, Vec<f64>, Vec<f64>)], w: &[f64], held: &[f64], from: Option<(&[f64], &[usize])>| {
        let on: Vec<bool> = held.iter().map(|&x| x != 0.0).collect();
        let g_on = g_rows.select(&on);
        let g_c: Vec<f64> = (0..held.len()).filter(|&i| on[i]).map(|i| held[i] * bound[i]).collect();
        let g_w = vec![PIN; g_c.len()];
        let mut terms: Vec<Term> = fixed.iter().map(|(rows, w, c)| Term { block: Block::Rows(rows), w, c }).collect();
        terms.push(Term { block: Block::Samples { seg: &seg, t: &t, a, b }, w, c: &h });
        terms.push(Term { block: Block::Rows(&g_on), w: &g_w, c: &g_c });
        fit(&terms, n, from)
    };
    // The slope limits after a solve: a segment past its limit holds it, a held one that the solve
    // would make less steep lets go. Without releases the rounds end: the held segments only grow.
    let update = |held: &[f64], z: &[f64], release: bool| -> Vec<f64> {
        let gz = g_rows.times(z);
        (0..held.len())
            .map(|i| {
                let mut out = if release && held[i] * gz[i] < bound[i] { 0.0 } else { held[i] };
                if gz[i] > bound[i] + TOLERANCE_M {
                    out = 1.0;
                }
                if gz[i] < -bound[i] - TOLERANCE_M {
                    out = -1.0;
                }
                out
            })
            .collect()
    };
    let deck = |rows: &Rows, z: &[f64]| rows.times(z);
    let mut held = vec![0.0; limited.len()];
    let mut before = held.clone();
    let mut w = weight.clone();
    let mut z = Vec::new();
    for solves in 0..SOLVES {
        let converged;
        (z, converged) = solve(&fixed, &w, &held, None);
        unconverged += usize::from(!converged);
        if solves < SOLVES - 1 {
            w = (0..seg.len())
                .map(|i| {
                    let residual = (z[a[seg[i]]] * (1.0 - t[i]) + z[b[seg[i]]] * t[i] - h[i]).abs();
                    weight[i] * (HUBER_M / residual.max(1e-9)).min(1.0)
                })
                .collect();
            // A short bridge that lies on the terrain is a culvert. Any other deck that passes too low
            // holds its least height from the next solve on.
            culvert = culverts(&net, &z, &seg, &t, &h, &finite);
            let cz = deck(&c_rows, &z);
            let low: Vec<bool> = (0..least.len())
                .map(|i| cz[i] < least[i] - TOLERANCE_M && !deck_way[i].is_some_and(|w| culvert[w]))
                .collect();
            let low_rows = c_rows.select(&low);
            let low_least: Vec<f64> = (0..least.len()).filter(|&i| low[i]).map(|i| least[i]).collect();
            fixed.push(held_at(low_rows, PIN, low_least));
            // So does a road or a track over a culvert, and a culvert's bridge of one, that lies too low.
            let kz = deck(&k_rows, &z);
            k_lift = (0..k_least.len()).map(|i| k_least[i] - kz[i]).collect();
            let ground = |i: usize| !net.free[k_way[i].0] || culvert[k_way[i].0];
            holds = (0..k_least.len()).map(|i| ground(i) && k_lift[i] <= k_way[i].1).collect();
            capped = (0..k_least.len()).map(|i| ground(i) && k_lift[i] > k_way[i].1).collect();
            lifted = (0..k_least.len()).map(|i| holds[i] && kz[i] < k_least[i] - TOLERANCE_M).collect();
            let k_least: Vec<f64> = (0..k_least.len()).filter(|&i| lifted[i]).map(|i| k_least[i]).collect();
            fixed.push(held_at(k_rows.select(&lifted), PIN, k_least));
            // Ways that pass too near hold the margin from the next solve on; a tunnel under a road holds
            // the road's height less the margin.
            let (uz, lz) = (u_rows.times(&z), l_rows.times(&z));
            let near: Vec<bool> = (0..need.len()).map(|i| uz[i] - lz[i] < need[i] - TOLERANCE_M).collect();
            let target: Vec<f64> = (0..need.len())
                .filter(|&i| near[i])
                .map(|i| if moves[i] { need[i] } else { need[i] - uz[i] })
                .collect();
            let near_rows = apart.select(&near);
            fixed.push(held_at(near_rows, PIN, target));
        }
        let next = update(&held, &z, true);
        before = std::mem::replace(&mut held, next);
    }
    // Rounds of the slope limits near the segments whose limit changed, the rest of the network
    // fixed. Nothing anchors a bridge or a tunnel but its ends: a region takes it whole.
    let edges: Vec<(usize, usize, f64)> =
        (0..a.len()).map(|s| (a[s], b[s], if free[s] { 1e-6 } else { length[s].max(0.01) })).collect();
    let graph = Graph::new(n, &edges);
    let mut rounds = 0;
    while rounds < ROUNDS && held != before {
        rounds += 1;
        let sources: BTreeSet<usize> =
            (0..held.len()).filter(|&i| held[i] != before[i]).flat_map(|i| [a[limited[i]], b[limited[i]]]).collect();
        let distance = graph.distances(sources.into_iter().map(|v| (v, 0.0)), REGION_M);
        let region: Vec<usize> = (0..n).filter(|&v| distance[v].is_finite()).collect();
        let converged;
        (z, converged) = solve(&fixed, &w, &held, Some((&z, &region)));
        unconverged += usize::from(!converged);
        let next = update(&held, &z, rounds < ROUNDS / 2);
        before = std::mem::replace(&mut held, next);
    }

    // A clearance and a ford hold on the curve only where their vertices stay in the tiles.
    let mut stay = vec![false; n];
    let crossed = above.variables.iter().flatten().chain(under.upper.iter().chain(&under.lower).flat_map(|row| &row.0));
    fords.variables.iter().flatten().chain(crossed).for_each(|&v| stay[v] = true);
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
    let cz = deck(&c_rows, &z);
    let deck_mine: Vec<bool> =
        (0..least.len()).map(|i| deck_way[i].is_some_and(|w| !culvert[w]) && var_mine[deck_first[i]]).collect();
    let (uz, lz) = (u_rows.times(&z), l_rows.times(&z));
    let pass_mine: Vec<bool> = pass_first.iter().map(|&v| var_mine[v]).collect();
    let gz = g_rows.times(&z);
    let excess: Vec<(f64, usize)> = (0..limited.len())
        .filter(|&i| segment_mine[limited[i]])
        .map(|i| (gz[i].abs() - bound[i], limited[i]))
        .collect();
    let worst = excess.iter().fold(None, |best: Option<(f64, usize)>, &(e, s)| match best {
        Some((b, _)) if b >= e => best,
        _ => Some((e, s)),
    });
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
        fords: counted[1],
        culverts: solved.culverts.len(),
        rounds,
        held: count(&mut (0..limited.len()).map(|i| held[i] != 0.0 && segment_mine[limited[i]])),
        clearances: count(&mut deck_mine.iter().copied()),
        clearances_low_cm: count(&mut (0..least.len()).map(|i| cz[i] < least[i] - 0.01 && deck_mine[i])),
        passes: count(&mut pass_mine.iter().copied()),
        passes_near_cm: count(&mut (0..need.len()).map(|i| uz[i] - lz[i] < need[i] - 0.01 && pass_mine[i])),
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

/// Rows of their weights against 0.
fn level(rows: Rows, w: Vec<f64>) -> (Rows, Vec<f64>, Vec<f64>) {
    let c = vec![0.0; rows.len()];
    (rows, w, c)
}

/// Rows of one weight against their values.
fn held_at(rows: Rows, w: f64, c: Vec<f64>) -> (Rows, Vec<f64>, Vec<f64>) {
    (rows, vec![w; c.len()], c)
}

/// Latitude and longitude of a world point.
fn degrees([x, y]: [f64; 2]) -> [f64; 2] {
    [(std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees(), x * 360.0 - 180.0]
}

/// Per way, whether it is a culvert: a bridge shorter than CULVERT_M, with the deck no more than
/// CULVERT_RISE_M above the terrain under it.
fn culverts(net: &Network, z: &[f64], seg: &[usize], t: &[f64], h: &[f64], finite: &[bool]) -> Vec<bool> {
    let ways = net.free.len();
    let mut way_length = vec![0.0; ways];
    for (s, &w) in net.segment_way.iter().enumerate() {
        way_length[w] += net.length[s];
    }
    let short: Vec<bool> = (0..ways).map(|w| net.free[w] && !net.tunnel[w] && way_length[w] < CULVERT_M).collect();
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
