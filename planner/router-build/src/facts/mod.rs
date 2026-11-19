//! The search facts of the routes of the catalog: the car parks and the lift top stations near
//! each route, the places that it passes, its highest point and its shares of paved length, big
//! road, canopy and water. `specs/route-catalog.md`, section "Facts files", gives the files.

pub mod osm;
pub mod shares;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use planner_router::geometry::{self, METRES_PER_UDEG};
use planner_router::model::Point;
use planner_router::shape::P;
use rayon::prelude::*;
use serde_json::{json, Map, Value};

use crate::catalog::decode;
use osm::Features;
use shares::Shares;

/// A car park or the top station of a lift is in an entry within this distance of the route.
const REACH_M: f64 = 3_000.0;
const PLACE_M: f64 = 150.0;
const VALLEY_PARKING_M: f64 = 300.0;
/// The distance of the samples along the plan route, as `profile_m` has them.
const SAMPLE_M: f64 = 100.0;

/// A route record of the catalog with a line.
pub struct Route {
    pub id: i64,
    pub cells: Vec<String>,
    pub line: Vec<P>,
    /// The points of `line` at the shares of the length that the entries of `profile_m` have.
    pub samples: Vec<P>,
    pub highest_m: Option<i64>,
}

/// The route records of `catalog` that have a line of two or more points.
pub fn routes(catalog: &Value) -> Result<Vec<Route>, String> {
    let records = catalog["routes"].as_array().ok_or("The route catalog has no routes")?;
    let routes = records.iter().filter_map(|record| {
        let line = decode(&record["line_udeg"]);
        let length_m = record["length_m"].as_f64()?;
        (line.len() >= 2).then(|| Route {
            id: record["id"].as_i64().expect("a record has an id"),
            cells: record["cells"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect(),
            samples: samples(&line, length_m),
            highest_m: record["profile_m"].as_array().into_iter().flatten().filter_map(Value::as_i64).max(),
            line,
        })
    });
    Ok(routes.collect())
}

fn point(p: P) -> Point {
    Point { lon: p[0], lat: p[1], elevation: 0.0 }
}

/// The points of `line` at each 100 m of `length_m` and at its end, each at its share of the
/// length, as a reader takes the profile at a vertex.
pub(crate) fn samples(line: &[P], length_m: f64) -> Vec<P> {
    let points: Vec<Point> = line.iter().map(|&p| point(p)).collect();
    let along = geometry::cumulative(&points);
    let total = along[along.len() - 1];
    let count = (length_m / SAMPLE_M).floor() as usize + 2;
    let at = |i: usize| (i as f64 * SAMPLE_M).min(length_m) / length_m.max(f64::MIN_POSITIVE) * total;
    (0..count).map(|i| geometry::at(&points, &along, at(i))).map(|p| [p.lon, p.lat]).collect()
}

/// Segments in cells of about `cell_m` square, to find those near a point or a segment. A point
/// is a segment of length 0.
pub(crate) struct Grid {
    /// The size of a cell in microdegrees of longitude and of latitude.
    size: [f64; 2],
    cells: HashMap<(i32, i32), Vec<u32>>,
    items: Vec<([P; 2], u32)>,
}

impl Grid {
    /// `lat` is the latitude farthest from the equator of the items, in degrees, where a cell
    /// is narrowest.
    pub fn new(cell_m: f64, lat: f64) -> Self {
        let size = cell_m / METRES_PER_UDEG;
        Self { size: [size / lat.to_radians().cos().max(0.01), size], cells: HashMap::new(), items: Vec::new() }
    }

    fn keys(&self, a: P, b: P, grow_m: f64) -> impl Iterator<Item = (i32, i32)> {
        let grow = grow_m / METRES_PER_UDEG;
        let pad = [grow * self.size[0] / self.size[1], grow];
        let size = self.size;
        let range = move |axis: usize| {
            let key = |v: f64| (v / size[axis]).floor() as i32;
            key(a[axis].min(b[axis]) as f64 - pad[axis])..=key(a[axis].max(b[axis]) as f64 + pad[axis])
        };
        let ys = range(1);
        range(0).flat_map(move |x| ys.clone().map(move |y| (x, y)))
    }

    pub fn insert(&mut self, a: P, b: P, owner: u32) {
        let item = self.items.len() as u32;
        self.items.push(([a, b], owner));
        for key in self.keys(a, b, 0.0).collect::<Vec<_>>() {
            self.cells.entry(key).or_default().push(item);
        }
    }

    /// Calls `found` with the owner of each item within `reach_m` of the segment `a`–`b`, and its
    /// distance in metres. The item or the segment is a point. An item can come more than once.
    pub fn near(&self, a: P, b: P, reach_m: f64, mut found: impl FnMut(u32, f64)) {
        let to = |p: P, [s, t]: [P; 2]| geometry::project(point(p), point(s), point(t)).1;
        for key in self.keys(a, b, reach_m) {
            for &item in self.cells.get(&key).into_iter().flatten() {
                let (segment, owner) = self.items[item as usize];
                let metres = to(a, segment).min(to(b, segment)).min(to(segment[0], [a, b])).min(to(segment[1], [a, b]));
                if metres <= reach_m {
                    found(owner, metres);
                }
            }
        }
    }

    /// The owners within `reach_m` of `p`, each with its least distance in metres.
    fn nearest(&self, p: P, reach_m: f64) -> BTreeMap<u32, f64> {
        let mut nearest = BTreeMap::new();
        self.near(p, p, reach_m, |owner, metres| {
            let least = nearest.entry(owner).or_insert(metres);
            *least = least.min(metres);
        });
        nearest
    }
}

/// The counts of the facts that the step reports.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub routes: usize,
    pub with_car_park_1km: usize,
    pub with_canopy: usize,
    pub with_place: usize,
    pub car_parks: usize,
    pub lifts: usize,
    pub places: usize,
    /// The samples of all routes, those that are paved, and those paved by the class of a highway
    /// without `surface` (`shares::Samples::paved`).
    pub samples: usize,
    pub paved_samples: usize,
    pub paved_by_class_samples: usize,
    /// Of the car parks with an area and `capacity`: those that the area makes large, and those of
    /// them with 50 spaces or more.
    pub large_by_area: usize,
    pub large_by_area_with_50: usize,
}

impl Report {
    pub fn to_json(&self) -> Value {
        json!({
            "routes": self.routes, "with_car_park_1km": self.with_car_park_1km, "with_canopy": self.with_canopy,
            "with_place": self.with_place, "car_parks": self.car_parks, "lifts": self.lifts, "places": self.places,
            "samples": self.samples, "paved_samples": self.paved_samples,
            "paved_by_class_samples": self.paved_by_class_samples, "large_by_area": self.large_by_area,
            "large_by_area_with_50": self.large_by_area_with_50,
        })
    }
}

/// The entry of each route, in the order of the routes, and each place that an entry names, by
/// its id.
pub struct Facts {
    pub entries: Vec<Map<String, Value>>,
    pub places: BTreeMap<String, Value>,
    pub report: Report,
}

/// The facts of `routes`. `height` gives the GLO-30 height at a point.
pub fn entries(
    routes: &[Route],
    features: &Features,
    shares: &[Shares],
    height: &mut dyn FnMut(P) -> Result<Option<f64>, String>,
) -> Result<Facts, String> {
    let lat = routes.iter().flat_map(|route| &route.line).map(|p| (p[1] as f64 * 1e-6).abs()).fold(0.0, f64::max);
    // A grid of cells of the reach of each search: a place searches far fewer segments than a car park.
    let grid = |cell_m: f64| {
        let mut grid = Grid::new(cell_m, lat);
        for (i, route) in routes.iter().enumerate() {
            for pair in route.line.windows(2) {
                grid.insert(pair[0], pair[1], i as u32);
            }
        }
        grid
    };
    let (lines, near_lines) = (grid(REACH_M), grid(PLACE_M));
    let mut parks = Grid::new(VALLEY_PARKING_M, lat);
    for parking in &features.parkings {
        parks.insert(parking.at, parking.at, 0);
    }
    let walk = |metres: f64| metres.round() as i64;
    let mut car_parks: Vec<Vec<(i64, P, bool)>> = vec![Vec::new(); routes.len()];
    let near: Vec<_> = features.parkings.par_iter().map(|parking| lines.nearest(parking.at, REACH_M)).collect();
    for (parking, near) in features.parkings.iter().zip(near) {
        for (route, metres) in near {
            car_parks[route as usize].push((walk(metres), parking.at, parking.large()));
        }
    }

    let mut lifts: Vec<Vec<(i64, P, P, bool)>> = vec![Vec::new(); routes.len()];
    for lift in &features.lifts {
        if lift.ends.iter().all(|&end| lines.nearest(end, REACH_M).is_empty()) {
            continue;
        }
        let mut end = |i: usize| lift.ele[i].map_or_else(|| height(lift.ends[i]), |ele| Ok(Some(ele)));
        let (Some(a), Some(b)) = (end(0)?, end(1)?) else { continue };
        let (top, valley) = match a.total_cmp(&b) {
            std::cmp::Ordering::Greater => (lift.ends[0], lift.ends[1]),
            std::cmp::Ordering::Less => (lift.ends[1], lift.ends[0]),
            std::cmp::Ordering::Equal => continue,
        };
        let parking = !parks.nearest(valley, VALLEY_PARKING_M).is_empty();
        for (route, metres) in lines.nearest(top, REACH_M) {
            lifts[route as usize].push((walk(metres), top, valley, parking));
        }
    }

    let mut passes: Vec<BTreeSet<&str>> = vec![BTreeSet::new(); routes.len()];
    let mut places = BTreeMap::new();
    for place in &features.places {
        let mut near = BTreeSet::new();
        for &p in std::iter::once(&place.at).chain(&place.outline) {
            near.extend(near_lines.nearest(p, PLACE_M).into_keys());
        }
        if near.is_empty() {
            continue;
        }
        for route in near {
            passes[route as usize].insert(&place.id);
        }
        let mut entry = json!({"kind": place.kind, "at_udeg": place.at});
        if let Some(name) = &place.name {
            entry["name"] = json!(name);
        }
        if let Some(ele) = place.ele_m {
            entry["ele_m"] = json!(ele);
        }
        places.insert(place.id.clone(), entry);
    }

    let mut report = Report { routes: routes.len(), places: places.len(), ..Report::default() };
    let mut entries = Vec::with_capacity(routes.len());
    for (i, (route, shares)) in routes.iter().zip(shares).enumerate() {
        let (parks, lifts, passes) = (&mut car_parks[i], &mut lifts[i], &passes[i]);
        parks.sort_unstable();
        lifts.sort_unstable();
        report.with_car_park_1km += usize::from(parks.first().is_some_and(|park| park.0 <= 1000));
        report.with_place += usize::from(!passes.is_empty());
        report.with_canopy += usize::from(shares.canopy.is_some());
        let mut entry = Map::new();
        let size = |large: bool| if large { "large" } else { "small" };
        let parks = parks.iter().map(|&(m, at, large)| json!({"at_udeg": at, "walk_m": m, "size": size(large)}));
        let lifts = lifts.iter().map(|&(m, top, valley, parking)| {
            json!({"top_udeg": top, "walk_m": m, "valley_udeg": valley, "valley_parking": parking})
        });
        for (key, list) in [("car_parks", parks.collect::<Vec<_>>()), ("lifts", lifts.collect())] {
            if !list.is_empty() {
                entry.insert(key.into(), Value::Array(list));
            }
        }
        if !passes.is_empty() {
            entry.insert("places".into(), json!(passes));
        }
        if let Some(highest) = route.highest_m {
            entry.insert("highest_m".into(), json!(highest));
        }
        for (key, share) in
            [("paved_pct", shares.paved), ("big_road_pct", shares.big_road), ("water_pct", shares.water)]
        {
            entry.insert(key.into(), json!(share));
        }
        if let Some(canopy) = shares.canopy {
            entry.insert("canopy_pct".into(), json!(canopy));
        }
        entries.push(entry);
    }
    report.car_parks = features.parkings.len();
    report.lifts = features.lifts.len();
    let both = features.parkings.iter().filter(|parking| parking.area_m2.is_some_and(|area| area >= 1_250.0));
    let both: Vec<_> = both.filter_map(|parking| parking.capacity).collect();
    report.large_by_area = both.len();
    report.large_by_area_with_50 = both.iter().filter(|&&spaces| spaces >= 50).count();
    Ok(Facts { entries, places, report })
}

/// The facts file of each cell of `cells`: the entry of each route that names the cell, and the
/// places that those entries name.
pub fn files(cells: &[&str], routes: &[Route], facts: &Facts) -> BTreeMap<String, Value> {
    let Facts { entries, places, .. } = facts;
    let empty = json!({"format": 1, "places": {}, "routes": {}});
    let mut files: BTreeMap<String, Value> = cells.iter().map(|&cell| (cell.to_string(), empty.clone())).collect();
    for (route, entry) in routes.iter().zip(entries) {
        for cell in &route.cells {
            let Some(file) = files.get_mut(cell) else { continue };
            for id in entry.get("places").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                file["places"][id] = places[id].clone();
            }
            file["routes"][route.id.to_string()] = Value::Object(entry.clone());
        }
    }
    files
}
