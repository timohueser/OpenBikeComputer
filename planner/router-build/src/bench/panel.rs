//! The random pairs of junctions of the route bench, and the pooled measures of their routes.
use super::measures::{measure, Measures};
use super::net::Net;
use rayon::prelude::*;

/// The least and the largest straight-line distance of a pair in km, and the count of such pairs.
pub type Class = (f64, f64, usize);

/// 100 pairs each within 10 % of 5, 15 and 40 km.
pub const PANEL: &[Class] = &[(4.5, 5.5, 100), (13.5, 16.5, 100), (36.0, 44.0, 100)];
pub const TOWN: &[Class] = &[(3.0, 8.0, 100)];
const SEED: u64 = 3836;
/// The draws of one class before `sample` gives up.
const TRIES: u64 = 50_000_000;

pub struct Spec {
    pub classes: &'static [Class],
    /// West, south, east and north in degrees: both ends of a pair lie inside.
    pub inside: Option<[f64; 4]>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pair {
    pub from: u32,
    pub to: u32,
}

/// SplitMix64.
fn random(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The junctions inside `inside` where a road with the snap bit of the profile arrives, in rising
/// order.
pub fn junctions(net: &Net, inside: Option<[f64; 4]>) -> Vec<u32> {
    let within = |junction: u32| {
        let point = net.graph.points.values()[junction as usize];
        let (lon, lat) = (f64::from(point.lon) * 1e-6, f64::from(point.lat) * 1e-6);
        inside.is_none_or(|[west, south, east, north]| lon >= west && lon <= east && lat >= south && lat <= north)
    };
    let roads = (0..net.graph.roads.len() as u32).filter(|&road| net.snappable(road));
    let mut junctions: Vec<u32> = roads.map(|road| net.to(road)).filter(|&junction| within(junction)).collect();
    junctions.sort_unstable();
    junctions.dedup();
    junctions
}

/// The pairs of `spec`, the same for each call on the same graph: random junctions of `junctions`
/// whose distance fits a class and that have a route.
pub fn sample(net: &Net, spec: &Spec) -> Result<Vec<Pair>, String> {
    let (junctions, points) = (junctions(net, spec.inside), net.graph.points.values());
    if junctions.len() < 2 {
        return Err("the box of the pairs has no two junctions".into());
    }
    let (mut state, mut pairs) = (SEED, Vec::new());
    for &(least_km, most_km, count) in spec.classes {
        let (start, mut tries) = (pairs.len(), 0);
        while pairs.len() < start + count {
            tries += 1;
            if tries > TRIES {
                return Err(format!("the box has no {count} pairs of {least_km} to {most_km} km"));
            }
            let mut pick = || junctions[(random(&mut state) % junctions.len() as u64) as usize];
            let (from, to) = (pick(), pick());
            let km = points[from as usize].distance(points[to as usize]) / 1000.0;
            if km >= least_km && km <= most_km && net.route(from, to)?.is_some() {
                pairs.push(Pair { from, to });
            }
        }
    }
    Ok(pairs)
}

pub struct Panel {
    pub routes: usize,
    /// The sum of the measures of the routes.
    pub total: Measures,
    /// The length of a route against its shortest route: the median, the 95th percentile and the
    /// largest.
    pub detour: [f64; 3],
}

/// The pooled measures of the routes of `pairs`. Each pair has a route.
pub fn panel(net: &Net, pairs: &[Pair]) -> Result<Panel, String> {
    let routes = pairs.par_iter().map(|pair| {
        let route = net.route(pair.from, pair.to)?.ok_or("a pair of the panel has no route")?;
        let shortest = net.shortest(pair.from, pair.to)?.ok_or("a pair of the panel has no shortest route")?;
        Ok((measure(net, &route), shortest.iter().map(|&road| net.length(road)).sum::<f64>()))
    });
    let routes = routes.collect::<Result<Vec<_>, String>>()?;
    let mut total = Measures::default();
    routes.iter().for_each(|(measures, _)| total.add(measures));
    let mut detours: Vec<f64> = routes.iter().map(|(measures, shortest)| measures.length_m / shortest).collect();
    detours.sort_by(f64::total_cmp);
    let quantile = |q: f64| detours[((detours.len() - 1) as f64 * q).round() as usize];
    Ok(Panel { routes: routes.len(), total, detour: [quantile(0.5), quantile(0.95), quantile(1.0)] })
}
