//! The numbers of one route of the route bench.
use super::net::Net;
use crate::source::Tags;
use crate::traffic::{self, Protection};
use planner_router::model::Point;

/// The length before and after a junction that gives the heading there.
const HEADING_M: f64 = 15.0;
const TURN_DEGREES: f64 = 45.0;
/// A route that is on the same name, ref or route relation again within this length left it and
/// joined it again.
const REJOIN_M: f64 = 2_000.0;
/// A main road with a tagged limit up to this is slow.
const SLOW_KMH: f64 = 50.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measures {
    pub length_m: f64,
    pub ascent_m: f64,
    pub turns: u32,
    /// Leave-and-rejoin events.
    pub events: u32,
    /// Signal nodes at the junctions inside the route, not at its two ends.
    pub signals: u32,
    /// On a main road with a limit over `SLOW_KMH` or no tagged limit, and no cycle track.
    pub fast_main_m: f64,
    /// On a main road with a tagged limit up to `SLOW_KMH`.
    pub slow_main_m: f64,
    /// On a signed route of the activity of the profile.
    pub signed_m: f64,
    /// By `Surface`, from `Unknown`.
    pub surface_m: [f64; 6],
}

impl Measures {
    pub fn add(&mut self, other: &Measures) {
        self.length_m += other.length_m;
        self.ascent_m += other.ascent_m;
        self.turns += other.turns;
        self.events += other.events;
        self.signals += other.signals;
        self.fast_main_m += other.fast_main_m;
        self.slow_main_m += other.slow_main_m;
        self.signed_m += other.signed_m;
        for (sum, metres) in self.surface_m.iter_mut().zip(other.surface_m) {
            *sum += metres;
        }
    }

    /// The measures against the length: a count for each km or 100 km, a length as a percentage.
    pub fn rates(&self) -> Vec<(&'static str, f64)> {
        let km = self.length_m / 1000.0;
        let percent = |metres: f64| metres / km / 10.0;
        let surfaces = ["unknown surface %", "paved %", "compacted %", "gravel %", "dirt %", "rough %"];
        let mut rates = vec![
            ("turns per km", f64::from(self.turns) / km),
            ("leave-and-rejoin events per 100 km", 100.0 * f64::from(self.events) / km),
            ("signal nodes per 100 km", 100.0 * f64::from(self.signals) / km),
            ("fast main road %", percent(self.fast_main_m)),
            ("slow main road %", percent(self.slow_main_m)),
            ("signed route %", percent(self.signed_m)),
            ("ascent m per km", self.ascent_m / km),
        ];
        rates.extend(surfaces.into_iter().zip(self.surface_m.map(percent)));
        rates
    }
}

/// The name, the ref and the signed routes of a road.
type Label<'a> = (Option<&'a str>, Option<&'a str>, &'a [i64]);

fn shares(a: &Label, b: &Label) -> bool {
    a.0.is_some() && a.0 == b.0 || a.1.is_some() && a.1 == b.1 || a.2.iter().any(|route| b.2.contains(route))
}

/// The tagged limit in the direction of travel in km/h, as `traffic::stress` reads it.
fn limit(tags: &Tags, reversed: bool) -> Option<f64> {
    let direction = if reversed { "maxspeed:backward" } else { "maxspeed:forward" };
    [direction, "maxspeed"].into_iter().find_map(|key| traffic::speed(tags.get(key)?))
}

/// Whether the route turns from road `before` onto road `after`: a change of heading over
/// `TURN_DEGREES` at a junction of three or more road ends.
fn turns(net: &Net, before: u32, after: u32) -> bool {
    let roads = &net.graph.roads;
    let (arrival, departure) = (roads.shape(before as usize), roads.shape(after as usize));
    let (end, start) = (arrival[arrival.len() - 1], departure[0]);
    let back = arrival.iter().rev().skip(1).find(|point| point.distance(end) >= HEADING_M).unwrap_or(&arrival[0]);
    let ahead = departure.iter().skip(1).find(|point| point.distance(start) >= HEADING_M);
    // East and north from `a` to `b`, in microdegrees of latitude.
    let vector = |a: Point, b: Point| {
        (f64::from(b.lon - a.lon) * (f64::from(a.lat) * 1e-6).to_radians().cos(), f64::from(b.lat - a.lat))
    };
    let (u, v) = (vector(*back, end), vector(start, *ahead.unwrap_or(&departure[departure.len() - 1])));
    let lengths = u.0.hypot(u.1) * v.0.hypot(v.1);
    net.ends[roads.to(before as usize) as usize] >= 3
        && lengths > 0.0
        && ((u.0 * v.0 + u.1 * v.1) / lengths).clamp(-1.0, 1.0).acos().to_degrees() > TURN_DEGREES
}

/// The measures of the route `roads`.
pub fn measure(net: &Net, roads: &[u32]) -> Measures {
    let (graph, all) = (net.graph, &net.graph.roads);
    let row = |i: usize| all.way_row(roads[i] as usize);
    let ways: Vec<_> = (0..roads.len()).map(|i| graph.osm.ways.row(row(i) as usize)).collect();
    let mut measures = Measures::default();
    let (mut labels, mut starts) = (Vec::<Label>::new(), Vec::new());
    for (i, (&road, way)) in roads.iter().zip(&ways).enumerate() {
        let (id, length, reversed) = (road as usize, net.length(road), all.reversed(road as usize));
        starts.push(measures.length_m);
        measures.length_m += length;
        measures.ascent_m += f64::from(all.ascent_m.values()[id]);
        if let Some(&next) = roads.get(i + 1) {
            measures.signals += u32::from(graph.signals.values()[all.to(id) as usize] != 0);
            measures.turns += u32::from(turns(net, road, next));
        }
        if all.class(id) == 2 {
            let track = traffic::protection(&way.tags, reversed, way.facts.country()) == Protection::Track;
            match limit(&way.tags, reversed) {
                Some(limit) if limit <= SLOW_KMH => measures.slow_main_m += length,
                _ if !track => measures.fast_main_m += length,
                _ => {}
            }
        }
        let text = |key: &str| way.tags.get(key).filter(|value| !value.is_empty());
        let routes = net.routes.get(&row(i)).map_or(&[][..], Vec::as_slice);
        if !routes.is_empty() {
            measures.signed_m += length;
        }
        measures.surface_m[all.surface(id) as usize] += length;
        labels.push((text("name"), text("ref"), routes));
    }
    // An event: the next road shares nothing with road `i`, and a road that starts within
    // `REJOIN_M` of that leave shares again. A road with no name, no ref and no route shares
    // nothing, so it starts no event.
    for i in 0..roads.len().saturating_sub(1) {
        let mut ahead = (i + 2..roads.len()).take_while(|&j| starts[j] - starts[i + 1] <= REJOIN_M);
        let left = !shares(&labels[i], &labels[i + 1]);
        measures.events += u32::from(left && ahead.any(|j| shares(&labels[i], &labels[j])));
    }
    measures
}
