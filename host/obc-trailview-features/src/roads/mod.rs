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
//! FORD_M below the water it crosses. Tracks side by side, as OSM maps each track of a double line,
//! lie at one height: their beds are one bed. A track on a ramp beside another keeps its own height.

mod area;
mod fit;
mod network;
mod rows;
#[cfg(test)]
mod tests;

use std::collections::BTreeSet;

pub use area::{context, solve_by_area};
use fit::{fit, Block, Graph, Rows, Term};
use network::{hypot, sub, Network};
use rows::union;

use crate::{metres, Terrain};

const SAMPLE_M: f64 = 2.0;
/// While it solves, a way has a vertex at least every VERTEX_M; the tiles keep one only where the
/// profile bends by more than KEEP_M there.
const VERTEX_M: f64 = 10.0;
const KEEP_M: f64 = 0.2;
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
/// A road or a path over a tunnel lies at least this far over its bore, BORE_M for a road, as
/// `tools/trailview/structures.py` builds it.
pub const COVER_M: f64 = 1.0;
pub const BORE_M: f64 = 5.0;
const BORE_PATH_M: f64 = 2.6;
const BORE_RAIL_M: f64 = 6.5;
/// A track within PARALLEL_M beside another, and within PARALLEL_COS of its direction, lies at its
/// height, unless the terrain under the two differs by more than PARALLEL_RISE_M within
/// PARALLEL_FREE_M along it: a ramp or a flyover, whose climb the smoothing of a track spreads that far.
const PARALLEL_M: f64 = 6.0;
const PARALLEL_COS: f64 = 0.9;
const PARALLEL_RISE_M: f32 = 0.5;
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

/// A class of the table: its grade limit (infinite for none), its smoothing length, and whether it
/// is a track.
#[derive(Clone, Debug)]
pub struct Class {
    pub grade: f64,
    pub smooth_m: f64,
    pub rail: bool,
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
    /// Its OSM `layer`, 0 without one, and whether it has one.
    pub layer: i64,
    pub layered: bool,
    /// A way on the ground of a negative `layer` without a `tunnel` tag lies under something that the
    /// solver does not model, such as a station hall: no way on the ground passes over or under it.
    pub indoor: bool,
    pub half: f64,
    /// Its row of the class table.
    pub rule: usize,
    pub parts: Vec<Vec<[f64; 2]>>,
}

/// A solved waterway: its points and their levels, NaN where unknown.
#[derive(Clone, Debug)]
pub struct River {
    pub points: Vec<[f64; 2]>,
    pub levels: Vec<f64>,
}

/// Per feature, per part: its points with the vertices that the solver adds and their heights (NaN
/// where unknown), or None for a part that the solver leaves alone.
pub type Heights = Vec<Vec<Option<(Vec<[f64; 2]>, Vec<f64>)>>>;

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
}

impl Metrics {
    /// The metrics of two jobs: the counts add up; `rounds` and `residual_p99_m` are the largest, and so
    /// is the grade excess, with its place.
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
        ];
        for (sum, value) in sums {
            *sum += value;
        }
        self.rounds = self.rounds.max(other.rounds);
        self.residual_p99_m = self.residual_p99_m.max(other.residual_p99_m);
        if (other.grade_excess_m, other.grade_excess_at) > (self.grade_excess_m, self.grade_excess_at) {
            (self.grade_excess_m, self.grade_excess_at) = (other.grade_excess_m, other.grade_excess_at);
        }
    }
}

/// The heights of a solve; the features that are culverts; the tunnels that pass under a road or a
/// path; and the metrics.
#[derive(Debug, Default)]
pub struct Solved {
    pub heights: Heights,
    pub culverts: BTreeSet<usize>,
    pub underpasses: BTreeSet<usize>,
    pub metrics: Metrics,
}

/// The heights of the features, with vertices added where the profile bends between two of OSM.
/// `rivers` are the solved waterways, for the fords and the decks. `owned(xy)` marks the OSM vertices
/// of the area of a job of `solve_by_area`: the culverts, the tunnels and the metrics count only the
/// ways that start on one of them, and the vertices from one of them to the next vertex of OSM.
pub fn solve(
    features: &[&Feature],
    terrain: &dyn Terrain,
    rivers: &[&River],
    classes: &Classes,
    owned: Option<&dyn Fn([f64; 2]) -> bool>,
) -> Solved {
    let mut solved =
        Solved { heights: features.iter().map(|f| vec![None; f.parts.len()]).collect(), ..Default::default() };
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
    let fords = rows::fords(&net, rivers);
    let bores = rows::chains(&net, &net.tunnel);
    let beside = rows::parallels(&net, classes, terrain);
    let bridge: Vec<bool> = (0..net.free.len()).map(|w| net.free[w] && !net.tunnel[w]).collect();
    // The decks over what they cross and the bridges that do not sag: each ≥ its value.
    let (above, deck_ways) = rows::clearances(&net, rivers, classes, terrain);
    let sags = rows::chains(&net, &bridge);
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
            let culvert = culverts(&net, &z, &seg, &t, &h, &finite);
            let cz = deck(&c_rows, &z);
            let low: Vec<bool> = (0..least.len())
                .map(|i| cz[i] < least[i] - TOLERANCE_M && !deck_way[i].is_some_and(|w| culvert[w]))
                .collect();
            let low_rows = c_rows.select(&low);
            let low_least: Vec<f64> = (0..least.len()).filter(|&i| low[i]).map(|i| least[i]).collect();
            fixed.push(held_at(low_rows, PIN, low_least));
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

    let culvert = culverts(&net, &z, &seg, &t, &h, &finite);
    let keep = kept(&net, &z, &known);
    for (w, &(f, p)) in net.way_feature.iter().enumerate() {
        let span = net.way_first[w]..=net.way_last[w];
        let vertices: Vec<usize> = span.filter(|&v| keep[v]).collect();
        let points = vertices.iter().map(|&v| net.vertex_xy[v]).collect();
        let heights =
            vertices.iter().map(|&v| if known[net.vertex_var[v]] { z[net.vertex_var[v]] } else { f64::NAN }).collect();
        solved.heights[f][p] = Some((points, heights));
    }
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
        residual_p99_m: percentile(&mut residual, 99.0),
        refinements_unconverged: unconverged,
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

/// The q-th percentile with linear interpolation, 0 for no values.
fn percentile(values: &mut [f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = q / 100.0 * (values.len() - 1) as f64;
    let (lo, hi) = (at.floor() as usize, at.ceil() as usize);
    values[lo] + (values[hi] - values[lo]) * (at - lo as f64)
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

/// Per vertex, whether the tiles keep it: each vertex of OSM and at an approach distance, and an
/// added vertex where the profile leaves the straight line between its neighbours by more than
/// KEEP_M (Douglas-Peucker in the plane of distance and height, over each OSM segment).
fn kept(net: &Network, z: &[f64], known: &[bool]) -> Vec<bool> {
    let mut keep = net.vertex_fixed.clone();
    let zv: Vec<f64> = net.vertex_var.iter().map(|&v| if known[v] { z[v] } else { f64::NAN }).collect();
    let mut v = 0;
    while v < net.vertex_xy.len() {
        if net.vertex_osm[v] {
            v += 1;
            continue;
        }
        // The added vertices of one OSM segment, between the vertices i and j about them.
        let (i, segment) = (v - 1, net.vertex_segment[v]);
        while v < net.vertex_xy.len() && !net.vertex_osm[v] && net.vertex_segment[v] == segment {
            v += 1;
        }
        let j = v;
        let worst = (i + 1..j)
            .map(|k| {
                let off = (zv[k] - (zv[i] + (zv[j] - zv[i]) * net.vertex_t[k])).abs();
                if off.is_finite() {
                    off
                } else {
                    f64::INFINITY
                }
            })
            .fold(f64::NEG_INFINITY, f64::max);
        if worst <= KEEP_M {
            continue;
        }
        if !zv[i..=j].iter().all(|z| z.is_finite()) {
            keep[i..=j].fill(true);
            continue;
        }
        let mut stops = vec![i];
        stops.extend((i + 1..j).filter(|&k| net.vertex_fixed[k]));
        stops.push(j);
        let mut spans: Vec<(usize, usize)> = stops.windows(2).map(|s| (s[0], s[1])).collect();
        while let Some((p, q)) = spans.pop() {
            if q - p < 2 {
                continue;
            }
            let tp = if p != i { net.vertex_t[p] } else { 0.0 };
            let tq = if q != j { net.vertex_t[q] } else { 1.0 };
            let mut far = (p + 1, f64::NEG_INFINITY);
            for k in p + 1..q {
                let off = (zv[k] - (zv[p] + (zv[q] - zv[p]) * (net.vertex_t[k] - tp) / (tq - tp))).abs();
                if off > far.1 {
                    far = (k, off);
                }
            }
            if far.1 > KEEP_M {
                keep[far.0] = true;
                spans.push((p, far.0));
                spans.push((far.0, q));
            }
        }
    }
    keep
}
