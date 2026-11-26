//! PROTOTYPE. The panel: random pairs of junctions, the same for each variant, and the measures of
//! a route between them.
use super::census::Net;
use super::variant;
use crate::traffic::{self, Protection};
use planner_router::search::Workspace;
use planner_router::shape::P;
use rayon::prelude::*;

/// The straight-line distances of the pairs; a pair lies within 10 % of its distance.
pub const DISTANCES_KM: [f64; 3] = [5.0, 15.0, 40.0];
pub const PAIRS_EACH: usize = 100;
const SEED: u64 = 3836;
/// A route that is on the same name, ref or cycle route again within this length left it and
/// joined it again.
const REJOIN_M: f64 = 2000.0;

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
    pub signals: u32,
    /// Main roads (class 2) with a limit over 50 km/h or no tagged limit, and no cycle track.
    pub fast_main_m: f64,
    /// Main roads with a tagged limit of 50 km/h or less.
    pub slow_main_m: f64,
    pub signed_m: f64,
    /// By `Surface`, from `Unknown`.
    pub surface_m: [f64; 6],
    pub ascent_m: f64,
    pub line: Vec<P>,
}

/// SplitMix64.
fn random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// `PAIRS_EACH` pairs for each distance, from the junctions where a road with a snap bit of
/// `snappable` arrives. Each pair has a route in `net`.
pub fn sample(net: &Net, snappable: &[u64]) -> Result<Vec<Pair>, String> {
    let roads = &net.graph.roads;
    let mut junctions: Vec<u32> = (0..roads.len())
        .filter(|&road| snappable[road / 64] & (1 << (road % 64)) != 0)
        .map(|road| roads.to(road))
        .collect();
    junctions.sort_unstable();
    junctions.dedup();
    let points = net.graph.points.values();
    let (mut state, mut workspace, mut pairs) = (SEED, Workspace::default(), Vec::new());
    for distance in DISTANCES_KM {
        let mut found = 0;
        while found < PAIRS_EACH {
            let mut pick = || junctions[(random(&mut state) % junctions.len() as u64) as usize];
            let (from, to) = (pick(), pick());
            let apart = points[from as usize].distance(points[to as usize]) / 1000.0;
            if (apart - distance).abs() > 0.1 * distance || net.route_in(&mut workspace, from, to)?.is_none() {
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
    for (i, &road) in roads.iter().enumerate() {
        let id = road as usize;
        let way = graph.osm.ways.row(all.way_row(id) as usize);
        let (length, reversed) = (net.length(road), all.reversed(id));
        starts.push(measures.length_m);
        measures.length_m += length;
        if let Some(&next) = roads.get(i + 1) {
            measures.signals += u32::from(graph.signals.values()[all.to(id) as usize] != 0);
            measures.turns += u32::from(net.is_turn(road, next));
        }
        if all.class(id) == 2 {
            let track = traffic::protection(&way.tags, reversed, way.facts.country()) == Protection::Track;
            match variant::speed_limit(&way.tags, reversed) {
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
        labels.push(context.labels(all.way_row(id)));
    }
    // A leave: the next road shares no name, ref or cycle route with this one. A rejoin: a later
    // road within `REJOIN_M` shares one.
    let shared = |a: &[u64], b: &[u64]| a.iter().any(|label| b.contains(label));
    for i in 0..roads.len().saturating_sub(1) {
        if labels[i].is_empty() || shared(&labels[i], &labels[i + 1]) {
            continue;
        }
        let ahead = (i + 2..roads.len()).take_while(|&j| starts[j] - starts[i + 1] <= REJOIN_M);
        measures.events += u32::from(ahead.into_iter().any(|j| shared(&labels[i], &labels[j])));
    }
    measures.line = net.line(roads);
    measures
}
