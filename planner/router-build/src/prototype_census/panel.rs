//! PROTOTYPE. A panel: random pairs of junctions, the same for each variant, and the measures of
//! a route between them.
use super::census::Net;
use super::variant;
use crate::traffic::{self, Protection};
use planner_router::search::Workspace;
use planner_router::shape::P;
use rayon::prelude::*;

/// A route that is on the same name, ref or route relation again within this length left it and
/// joined it again.
const REJOIN_M: f64 = 2000.0;
/// The `R` of the clusters that `Measures::entered` counts, for each variant.
pub const CLUSTERS_M: u32 = 60;

/// The pairs of a panel: for each class, `count` pairs with a straight-line distance from `min_km`
/// to `max_km`, of junctions inside `inside` (west, south, east, north in degrees).
pub struct Spec {
    pub seed: u64,
    pub classes: &'static [(f64, f64, usize)],
    pub inside: Option<[f64; 4]>,
}

/// 100 pairs each at 5, 15 and 40 km, within 10 %.
pub const DISTANCES: &[(f64, f64, usize)] = &[(4.5, 5.5, 100), (13.5, 16.5, 100), (36.0, 44.0, 100)];

#[derive(Clone, Copy)]
pub struct Pair {
    pub from: u32,
    pub to: u32,
}

#[derive(Clone, Default)]
pub struct Measures {
    pub length_m: f64,
    pub turns: u32,
    pub events: u32,
    /// Signal nodes, and signalled junctions: the signal nodes without those that a road shorter
    /// than 40 m joins to the signal node before.
    pub signals: u32,
    pub signalled: u32,
    /// The clusters of `CLUSTERS_M` that the route enters; those that it enters at a signal node,
    /// where the cluster rule pays; and those that it enters at a node with no signal and then
    /// passes a signal node of, where the rule does not pay.
    pub entered: u32,
    pub paid: u32,
    pub free: u32,
    /// The clusters of R = 40 m that the route enters.
    pub entered_40: u32,
    /// Main roads (class 2) with a limit over 50 km/h or no tagged limit, and no cycle track.
    pub fast_main_m: f64,
    /// Main roads with a tagged limit of 50 km/h or less.
    pub slow_main_m: f64,
    /// Quiet roads (class 1) with a tagged limit of 80 km/h or more, and over 50 km/h.
    pub fast_quiet_m: f64,
    pub quiet_over_50_m: f64,
    /// Roads of any class with no tagged limit.
    pub no_limit_m: f64,
    pub signed_m: f64,
    /// By `Surface`, from `Unknown`.
    pub surface_m: [f64; 6],
    pub ascent_m: f64,
    pub line: Vec<P>,
}

/// SplitMix64.
pub fn random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The junctions where a road with a snap bit of `snappable` arrives, inside `inside`.
pub fn junctions(net: &Net, snappable: &[u64], inside: Option<[f64; 4]>) -> Result<Vec<u32>, String> {
    let roads = &net.graph.roads;
    let points = net.graph.points.values();
    let within = |junction: u32| {
        let point = points[junction as usize];
        let (lon, lat) = (f64::from(point.lon) * 1e-6, f64::from(point.lat) * 1e-6);
        inside.is_none_or(|[west, south, east, north]| lon >= west && lon <= east && lat >= south && lat <= north)
    };
    let mut junctions: Vec<u32> = (0..roads.len())
        .filter(|&road| snappable[road / 64] & (1 << (road % 64)) != 0)
        .map(|road| roads.to(road))
        .filter(|&junction| within(junction))
        .collect();
    junctions.sort_unstable();
    junctions.dedup();
    if junctions.len() < 2 {
        return Err("the box of a panel has no junctions".into());
    }
    Ok(junctions)
}

/// The pairs of `spec`, from the junctions where a road with a snap bit of `snappable` arrives.
/// Each pair has a route in `net`.
pub fn sample(net: &Net, snappable: &[u64], spec: &Spec) -> Result<Vec<Pair>, String> {
    let junctions = junctions(net, snappable, spec.inside)?;
    let points = net.graph.points.values();
    let (mut state, mut workspace, mut pairs) = (spec.seed, Workspace::default(), Vec::new());
    for &(min_km, max_km, count) in spec.classes {
        let (mut found, mut tries) = (0, 0u64);
        while found < count {
            tries += 1;
            if tries > 50_000_000 {
                return Err(format!("the panel finds no pair from {min_km} to {max_km} km"));
            }
            let mut pick = || junctions[(random(&mut state) % junctions.len() as u64) as usize];
            let (from, to) = (pick(), pick());
            let apart = points[from as usize].distance(points[to as usize]) / 1000.0;
            if apart < min_km || apart > max_km || net.route_in(&mut workspace, from, to)?.is_none() {
                continue;
            }
            pairs.push(Pair { from, to });
            found += 1;
        }
    }
    Ok(pairs)
}

/// The roads of the cheapest route of each pair in `net`.
pub fn routes(net: &Net, pairs: &[Pair]) -> Result<Vec<Vec<u32>>, String> {
    let routes = pairs.par_iter().map_init(Workspace::default, |workspace, pair| {
        let (path, _) = net.route_in(workspace, pair.from, pair.to)?.ok_or("a pair of the panel has no route")?;
        Ok(path[1..].to_vec())
    });
    routes.collect()
}

pub fn measure(net: &Net, roads: &[u32]) -> Measures {
    let (graph, context) = (net.graph, variant::context());
    let all = &graph.roads;
    let mut measures = Measures::default();
    let (mut labels, mut starts) = (Vec::with_capacity(roads.len()), Vec::with_capacity(roads.len()));
    // The route is in a cluster that it entered at a node with no signal, and has passed no signal node of it.
    let (clusters, mut unpaid) = (context.clusters(CLUSTERS_M), false);
    for (i, &road) in roads.iter().enumerate() {
        let id = road as usize;
        let way = graph.osm.ways.row(all.way_row(id) as usize);
        let (length, reversed) = (net.length(road), all.reversed(id));
        starts.push(measures.length_m);
        measures.length_m += length;
        if let Some(&next) = roads.get(i + 1) {
            measures.signals += u32::from(graph.signals.values()[all.to(id) as usize] != 0);
            measures.signalled += u32::from(variant::signalled(graph, road));
            measures.entered_40 += u32::from(context.clusters(40).enters(graph, road));
            let signal = graph.signals.values()[all.to(id) as usize] != 0;
            if clusters.enters(graph, road) {
                measures.entered += 1;
                measures.paid += u32::from(signal);
                unpaid = !signal;
            } else if clusters.of(all.to(id)) == 0 {
                unpaid = false;
            } else if unpaid && signal {
                measures.free += 1;
                unpaid = false;
            }
            measures.turns += u32::from(net.is_turn(road, next));
        }
        let speed = variant::speed_limit(&way.tags, reversed);
        if speed.is_none() {
            measures.no_limit_m += length;
        }
        if all.class(id) == 1 && speed.is_some_and(|speed| speed >= 80.0) {
            measures.fast_quiet_m += length;
        }
        if all.class(id) == 1 && speed.is_some_and(|speed| speed > 50.0) {
            measures.quiet_over_50_m += length;
        }
        if all.class(id) == 2 {
            let track = traffic::protection(&way.tags, reversed, way.facts.country()) == Protection::Track;
            match speed {
                Some(speed) if speed <= 50.0 => measures.slow_main_m += length,
                _ if !track => measures.fast_main_m += length,
                _ => {}
            }
        }
        if way.facts.ranks[usize::from(reversed)] != 0 {
            measures.signed_m += length;
        }
        measures.surface_m[all.surface(id) as usize] += length;
        measures.ascent_m += f64::from(all.ascent_m.values()[id]);
        labels.push(context.labels(all.way_row(id), net.profile.walking));
    }
    // A leave: the next road shares no name, ref or route with this one. A rejoin: a later road
    // within `REJOIN_M` shares one.
    let shared = |a: &[u64], b: &[u64]| a.iter().any(|label| b.contains(label));
    for i in 0..roads.len().saturating_sub(1) {
        if labels[i].is_empty() || shared(&labels[i], &labels[i + 1]) {
            continue;
        }
        let mut ahead = (i + 2..roads.len()).take_while(|&j| starts[j] - starts[i + 1] <= REJOIN_M);
        measures.events += u32::from(ahead.any(|j| shared(&labels[i], &labels[j])));
    }
    measures.line = net.line(roads);
    measures
}
