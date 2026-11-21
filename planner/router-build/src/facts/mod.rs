//! The facts of each catalog route for the search of the Explore page: the car parks, the lifts
//! and the places near it, its shares of paved length, big road, canopy and water, its snow
//! window, and for a one-way route the lift between its ends and the time back. The section "Facts files" of
//! `specs/route-catalog.md` defines the files.

pub mod geometry;
pub mod places;
pub mod shares;
pub mod snow;

use std::collections::{BTreeMap, BTreeSet};

use obc_data::pmtiles::Archive;
use planner_router::model::Pace;
use planner_router::shape::P;
use rayon::prelude::*;
use serde_json::{json, Map, Value};

use crate::source::Id;
use geometry::{Line, Plane};
use places::Places;
use shares::{percent, Roads, Shares, SAMPLE_M};
use snow::{Snow, Steps, Window};

/// The version of the facts files.
pub const FORMAT: u32 = 1;
/// Car parks and lifts count within this distance of a route.
pub const REACH_M: f64 = 3_000.0;
/// A peak, a hut, a lake or a waterfall counts within this distance of a route.
pub const PLACE_M: f64 = 150.0;
/// The ends of a one-way route that lie within this distance of the two stations of a lift give it
/// the lift shapes.
pub const LIFT_END_M: f64 = 300.0;
/// A route keeps this many car parks of each size class, and this many lifts.
pub const NEAREST: usize = 5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Parking,
    Lift,
    Peak,
    /// A hut with food: an alpine hut, or a restaurant, a cafe or a beer garden outside a town.
    Hut,
    Lake,
    Waterfall,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Parking => "parking",
            Kind::Lift => "lift",
            Kind::Peak => "peak",
            Kind::Hut => "hut",
            Kind::Lake => "lake",
            Kind::Waterfall => "waterfall",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Large,
    Small,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lift {
    pub valley: P,
    /// The car park of the valley station.
    pub parking: Option<Id>,
}

/// An OSM object that a route can pass or start from.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub id: Id,
    pub kind: Kind,
    pub name: Option<String>,
    /// The `ele` tag.
    pub ele_m: Option<i32>,
    /// The node; the top station of a lift; a point inside an area.
    pub at: P,
    /// Car parks only.
    pub size: Option<Size>,
    /// Lifts only.
    pub lift: Option<Lift>,
    /// The outer and inner rings of an area; empty for a point.
    pub rings: Vec<Vec<P>>,
}

impl Place {
    /// The distance from `line` to the place: to its point, or to the edge of its area and 0 when the
    /// line enters it. Infinite for an area whose edge is farther than `reach_m` from the box of
    /// the line.
    pub fn distance(&self, line: &Line, reach_m: f64) -> f64 {
        match self.rings.is_empty() {
            true => line.to_point(self.at),
            false => line.to_area(&self.rings, reach_m),
        }
    }
}

/// A route record of the catalog cells.
pub struct Route {
    pub id: i64,
    pub kind: String,
    pub looped: bool,
    pub line: Vec<P>,
    pub length_m: f64,
    pub moving_s: f64,
    pub profile: Option<Vec<Option<f64>>>,
}

impl Route {
    /// The route of a record of a cell file; None for a record with no `line_udeg`.
    pub fn read(record: &Value) -> Result<Option<Self>, String> {
        if record.get("line_udeg").is_none() {
            return Ok(None);
        }
        let id = record["id"].as_i64().ok_or("a record has no id")?;
        let line = crate::catalog::decode(&record["line_udeg"]);
        if line.is_empty() {
            return Err(format!("the route {id} has an empty line"));
        }
        let profile = record.get("profile_m").and_then(Value::as_array);
        Ok(Some(Route {
            id,
            kind: record["kind"].as_str().unwrap_or_default().to_string(),
            looped: record["loop"].as_bool().unwrap_or(false),
            line,
            length_m: record["length_m"].as_f64().unwrap_or(0.0),
            moving_s: record["moving_s"].as_f64().unwrap_or(0.0),
            profile: profile.map(|heights| heights.iter().map(Value::as_f64).collect()),
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    Start,
    End,
}

/// The facts of one route. Each list holds (place, whole metres to the route), by distance and
/// then by place.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub parking: Vec<(Id, u32)>,
    pub lifts: Vec<(Id, u32)>,
    pub places: Vec<(Id, u32)>,
    pub shares: Shares,
    /// None without a snow archive.
    pub snow: Option<Snow>,
    /// One-way routes only: the lift whose valley station lies at one end of the line and whose
    /// top station at the other, and the end at the valley.
    pub lift_ends: Option<(Id, End)>,
    /// One-way routes only.
    pub back_s: Option<u64>,
}

impl Entry {
    /// The places that the entry names.
    fn named(&self) -> impl Iterator<Item = Id> + '_ {
        let lists = self.parking.iter().chain(&self.lifts).chain(&self.places).map(|(id, _)| *id);
        lists.chain(self.lift_ends.map(|(id, _)| id))
    }

    fn json(&self) -> Value {
        let list =
            |list: &[(Id, u32)]| json!(list.iter().map(|(id, m)| json!([id.to_string(), m])).collect::<Vec<_>>());
        let shares = &self.shares;
        let mut entry = json!({
            "paved_pct": percent(shares.paved, shares.samples),
            "big_road_pct": percent(shares.big_road, shares.samples),
            "water_pct": percent(shares.water, shares.samples),
        });
        for (key, values) in [("parking", &self.parking), ("lifts", &self.lifts), ("places", &self.places)] {
            if !values.is_empty() {
                entry[key] = list(values);
            }
        }
        if let Some(canopy) = shares.canopy_pct() {
            entry["canopy_pct"] = json!(canopy);
        }
        match self.snow.map(|snow| snow.window) {
            Some(Window::Free(first, last)) => entry["snow_free"] = json!([first, last]),
            Some(Window::Never) => entry["snow_free"] = json!([]),
            _ => {}
        }
        if let Some((id, end)) = self.lift_ends {
            entry["lift_ends"] = json!([id.to_string(), if end == End::Start { "start" } else { "end" }]);
        }
        if let Some(back_s) = self.back_s {
            entry["back_s"] = json!(back_s);
        }
        entry
    }
}

fn place_json(place: &Place) -> Value {
    let mut object = json!({"kind": place.kind.name(), "at_udeg": place.at});
    if let Some(name) = &place.name {
        object["name"] = json!(name);
    }
    if let Some(ele) = place.ele_m {
        object["ele_m"] = json!(ele);
    }
    if let Some(size) = place.size {
        object["size"] = json!(if size == Size::Large { "large" } else { "small" });
    }
    if let Some(lift) = &place.lift {
        object["valley_udeg"] = json!(lift.valley);
        if let Some(parking) = lift.parking {
            object["parking"] = json!(parking.to_string());
        }
    }
    object
}

/// The `n` first of `list`, by distance and then by place.
fn nearest(mut list: Vec<(Id, u32)>, n: usize) -> Vec<(Id, u32)> {
    list.sort_by_key(|&(id, metres)| (metres, id));
    list.truncate(n);
    list
}

/// The lift whose valley station lies within `LIFT_END_M` of one end of the line and whose top
/// station within it of the other end, and the end at the valley. Of several, the one with the
/// smallest sum of the two distances, then the smallest id.
pub fn lift_ends<'p>(line: &[P], lifts: impl Iterator<Item = &'p Place>) -> Option<(Id, End)> {
    let (start, end) = (line[0], line[line.len() - 1]);
    let plane = Plane::at(start[1]);
    let mut fits = Vec::new();
    for lift in lifts {
        let Some(stations) = &lift.lift else { continue };
        for (valley, top, at) in [(start, end, End::Start), (end, start, End::End)] {
            let (to_valley, to_top) = (plane.distance(valley, stations.valley), plane.distance(top, lift.at));
            if to_valley <= LIFT_END_M && to_top <= LIFT_END_M {
                fits.push((to_valley + to_top, lift.id, at));
            }
        }
    }
    fits.into_iter().min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1))).map(|(_, id, at)| (id, at))
}

/// The moving time of the route in the other direction: `moving_s` times the time of the profile
/// read backwards over the time of the profile, both at the pace of its kind. A height of `null`
/// counts as no rise. Without a profile, `moving_s`.
pub fn back_s(route: &Route) -> u64 {
    let back = route.moving_s.round() as u64;
    let Some(profile) = &route.profile else { return back };
    let (walk, mtb) = (matches!(route.kind.as_str(), "hiking" | "foot"), route.kind == "mtb");
    let pace = Pace::default();
    let (mut forward, mut backward) = (0.0, 0.0);
    for (i, heights) in profile.windows(2).enumerate() {
        let step = (route.length_m - i as f64 * SAMPLE_M).clamp(0.0, SAMPLE_M);
        let rise = match heights {
            [Some(a), Some(b)] => b - a,
            _ => 0.0,
        };
        forward += pace.seconds(step, rise, walk, mtb);
        backward += pace.seconds(step, -rise, walk, mtb);
    }
    match forward > 0.0 {
        true => (route.moving_s * backward / forward).round() as u64,
        false => back,
    }
}

/// The archive values of the samples of a route: the forest and the snowy steps of each sample,
/// each None without its archive.
#[derive(Clone, Copy, Default)]
struct Layers<'a> {
    forest: Option<&'a [Option<bool>]>,
    snow: Option<&'a [Option<Steps>]>,
}

/// The entry of a route with its samples.
fn entry(route: &Route, samples: &[P], places: &Places, roads: &Roads, layers: Layers) -> Entry {
    let line = Line::new(&route.line);
    let metres = |place: &Place, reach_m: f64| {
        let distance = place.distance(&line, reach_m);
        (distance <= reach_m).then(|| (place.id, distance.round() as u32))
    };
    let (mut large, mut small, mut lifts) = (Vec::new(), Vec::new(), Vec::new());
    let mut stations = Vec::new();
    for place in places.near(&route.line, REACH_M) {
        let Some(found) = metres(place, REACH_M) else { continue };
        match (place.kind, place.size) {
            (Kind::Parking, Some(Size::Large)) => large.push(found),
            (Kind::Parking, _) => small.push(found),
            (Kind::Lift, _) => {
                lifts.push(found);
                stations.push(place);
            }
            _ => {}
        }
    }
    let nearby = places.near(&route.line, PLACE_M).filter(|place| !matches!(place.kind, Kind::Parking | Kind::Lift));
    let mut parking = nearest(large, NEAREST);
    parking.extend(nearest(small, NEAREST));
    let one_way = !route.looped;
    Entry {
        parking: nearest(parking, 2 * NEAREST),
        lifts: nearest(lifts, NEAREST),
        places: nearest(nearby.filter_map(|place| metres(place, PLACE_M)).collect(), usize::MAX),
        shares: Shares::new(samples, roads, places, layers.forest),
        snow: layers.snow.map(snow::window),
        lift_ends: one_way.then(|| lift_ends(&route.line, stations.into_iter())).flatten(),
        back_s: one_way.then(|| back_s(route)),
    }
}

/// The entry of each route, with the canopy and the `snow-seasons` archive when the release has
/// them.
pub fn entries(
    routes: &[Route],
    places: &Places,
    roads: &Roads,
    [canopy, snow]: [Option<&Archive>; 2],
) -> Result<BTreeMap<i64, Entry>, String> {
    let samples: Vec<Vec<P>> = routes.par_iter().map(|route| shares::samples(&route.line, route.length_m)).collect();
    let all = samples.concat();
    let forest = canopy.map(|archive| shares::forest(archive, &all)).transpose()?;
    let snow = snow.map(|archive| snow::read(archive, &all)).transpose()?;
    drop(all);
    let mut starts = vec![0];
    starts.extend(samples.iter().scan(0, |start, samples| {
        *start += samples.len();
        Some(*start)
    }));
    let layers = |i: usize| {
        let range = starts[i]..starts[i + 1];
        Layers {
            forest: forest.as_ref().map(|forest| &forest[range.clone()]),
            snow: snow.as_ref().map(|snow| &snow[range]),
        }
    };
    let entries = routes.par_iter().enumerate();
    Ok(entries.map(|(i, route)| (route.id, entry(route, &samples[i], places, roads, layers(i)))).collect())
}

/// The facts file of each cell, from the ids of the routes of its catalog file. Its places hold
/// every place that its entries name, and the car park of each lift among them.
pub fn files(
    cells: &BTreeMap<String, Vec<i64>>,
    entries: &BTreeMap<i64, Entry>,
    places: &Places,
) -> BTreeMap<String, Value> {
    let file = |ids: &Vec<i64>| {
        let entries: Vec<(i64, &Entry)> = ids.iter().filter_map(|id| Some((*id, entries.get(id)?))).collect();
        let mut named: BTreeSet<Id> = entries.iter().flat_map(|(_, entry)| entry.named()).collect();
        let parks: Vec<Id> = named.iter().filter_map(|&id| places.get(id)?.lift.as_ref()?.parking).collect();
        named.extend(parks);
        let named = named.into_iter().filter_map(|id| Some((id.to_string(), place_json(places.get(id)?))));
        let routes = entries.iter().map(|(id, entry)| (id.to_string(), entry.json()));
        json!({"format": FORMAT, "places": named.collect::<Map<_, _>>(), "routes": routes.collect::<Map<_, _>>()})
    };
    cells.iter().map(|(cell, ids)| (cell.clone(), file(ids))).collect()
}

/// The figures of the entries for the metrics of the step.
pub fn report(entries: &BTreeMap<i64, Entry>) -> Value {
    let count = |test: &dyn Fn(&Entry) -> bool| entries.values().filter(|entry| test(entry)).count();
    let sum = |field: fn(&Shares) -> u32| entries.values().map(|entry| u64::from(field(&entry.shares))).sum::<u64>();
    let paved = entries.iter().filter(|(_, entry)| percent(entry.shares.paved, entry.shares.samples) > 0);
    let snow = |test: fn(&Snow) -> bool| count(&|entry| entry.snow.as_ref().is_some_and(test));
    json!({
        "routes": entries.len(),
        "parking_within_1km": count(&|entry| entry.parking.iter().any(|&(_, metres)| metres <= 1_000)),
        "with_canopy": count(&|entry| entry.shares.canopy_pct().is_some()),
        "with_place": count(&|entry| !entry.places.is_empty()),
        "with_lift_ends": count(&|entry| entry.lift_ends.is_some()),
        "with_snow_free": snow(|snow| snow.window != Window::NoData),
        "snow_never_free": snow(|snow| snow.window == Window::Never),
        "snow_no_data": snow(|snow| snow.window == Window::NoData),
        "snow_left_out": snow(|snow| snow.window != Window::NoData && snow.left_out > 0),
        "samples": sum(|shares| shares.samples),
        "paved_points": sum(|shares| shares.paved),
        "paved_untagged_points": sum(|shares| shares.untagged),
        "paved_check": paved.map(|(id, _)| *id).take(20).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::Shape;
    use crate::facts::places::tests::places;
    use crate::source::Data;

    const LAT: i32 = 47_000_000;
    type Tagged = (&'static [(&'static str, &'static str)], Shape);

    /// The longitude `metres` east of 8° E at 47° N.
    fn east(metres: f64) -> i32 {
        8_000_000 + (metres / Plane::at(LAT).xy([1, 0])[0]).round() as i32
    }

    fn route(line: Vec<P>, looped: bool) -> Route {
        Route { id: 1, kind: "hiking".into(), looped, line, length_m: 1_000.0, moving_s: 900.0, profile: None }
    }

    /// A lift from `valley` up to `top`, and its stations with their heights.
    fn lift(valley: P, top: P) -> Vec<Tagged> {
        vec![
            (&[("aerialway", "gondola")], Shape::Line(vec![valley, top])),
            (&[("aerialway", "station"), ("ele", "1000")], Shape::Point(valley)),
            (&[("aerialway", "station"), ("ele", "2000")], Shape::Point(top)),
        ]
    }

    #[test]
    fn lift_ends_names_the_valley_end() {
        let (valley, top) = ([east(0.0), LAT], [east(0.0), LAT + 30_000]);
        let places = places(lift(valley, top));
        let gondola = places.get(Id::Way(1)).unwrap();
        let ends = |line: &[P]| lift_ends(line, [gondola].into_iter());
        let (near_valley, near_top) = ([east(100.0), LAT], [east(290.0), LAT + 30_000]);
        assert_eq!(ends(&[near_valley, [east(900.0), LAT + 15_000], near_top]), Some((Id::Way(1), End::Start)));
        assert_eq!(ends(&[near_top, near_valley]), Some((Id::Way(1), End::End)));
        assert_eq!(ends(&[[east(310.0), LAT], near_top]), None);
        let osm = Data::default();
        let roads = Roads::new(&osm);
        let entry =
            |looped| entry(&route(vec![near_valley, near_top], looped), &[], &places, &roads, Layers::default());
        assert_eq!(entry(false).lift_ends, Some((Id::Way(1), End::Start)));
        assert_eq!((entry(true).lift_ends, entry(true).back_s), (None, None));
    }

    #[test]
    fn back_s_reads_the_profile_backwards() {
        // 250 m: a profile at 0, 100, 200 and 250 m.
        let with = |profile: Vec<Option<f64>>| {
            let route = Route { length_m: 250.0, profile: Some(profile), ..route(Vec::new(), false) };
            back_s(&route)
        };
        assert!(with(vec![Some(500.0), Some(520.0), Some(540.0), Some(550.0)]) < 900);
        assert!(with(vec![Some(550.0), Some(540.0), Some(520.0), Some(500.0)]) > 900);
        assert_eq!(with(vec![Some(500.0); 4]), 900);
        assert_eq!(with(vec![Some(500.0), None, Some(600.0), None]), 900);
        assert_eq!(back_s(&route(Vec::new(), false)), 900);
    }

    #[test]
    fn an_entry_keeps_five_car_parks_of_each_class_and_five_lifts() {
        // For each k, a large car park at 100 k m, a small one 50 m farther and a lift with its top
        // 20 m farther: ids 5 k − 4, 5 k − 3 and 5 k − 2.
        let mut objects: Vec<Tagged> = Vec::new();
        for k in 1..=7 {
            let metres = 100.0 * f64::from(k);
            objects.push((&[("amenity", "parking"), ("capacity", "60")], Shape::Point([east(metres), LAT])));
            objects.push((&[("amenity", "parking")], Shape::Point([east(metres + 50.0), LAT])));
            objects.extend(lift([east(metres + 2_000.0), LAT], [east(metres + 20.0), LAT]));
        }
        objects.push((&[("amenity", "parking"), ("capacity", "60")], Shape::Point([east(3_100.0), LAT])));
        let places = places(objects);
        let osm = Data::default();
        let line = vec![[east(0.0), LAT - 10_000], [east(0.0), LAT + 10_000]];
        let entry = entry(&route(line, true), &[], &places, &Roads::new(&osm), Layers::default());
        let way = |k: i64, offset: i64| Id::Way(5 * k - offset);
        let parking: Vec<(Id, u32)> =
            (1..=5).flat_map(|k| [(way(k, 4), 100 * k as u32), (way(k, 3), 100 * k as u32 + 50)]).collect();
        assert_eq!(entry.parking, parking);
        assert_eq!(entry.lifts, (1..=5).map(|k| (way(k, 2), 100 * k as u32 + 20)).collect::<Vec<_>>());
    }

    #[test]
    fn a_file_names_places_of_other_cells_and_the_car_park_of_a_lift() {
        let (valley, top) = ([east(0.0), LAT], [east(0.0), LAT + 20_000]);
        let mut objects = lift(valley, top);
        objects.push((&[("amenity", "parking")], Shape::Point([east(100.0), LAT])));
        objects.push((&[("natural", "peak"), ("name", "Pilatus")], Shape::Point([east(0.0), LAT + 900_000])));
        let places = places(objects);
        let shares = Shares { samples: 10, paved: 3, ..Shares::default() };
        let entry = |places, lifts| Entry {
            parking: vec![],
            lifts,
            places,
            shares,
            snow: None,
            lift_ends: None,
            back_s: Some(100),
        };
        let mut entries =
            BTreeMap::from([(-501, entry(vec![(Id::Way(5), 0)], vec![])), (7, entry(vec![], vec![(Id::Way(1), 40)]))]);
        entries.get_mut(&7).unwrap().snow = Some(Snow { window: Window::Never, left_out: 0 });
        let cells =
            BTreeMap::from([("9-1-1".into(), vec![-501, 7]), ("9-1-2".into(), vec![7]), ("9-1-3".into(), vec![])]);
        let files = files(&cells, &entries, &places);
        let lift = json!({"kind": "lift", "at_udeg": top, "valley_udeg": valley, "parking": "w4"});
        let parking = json!({"kind": "parking", "size": "small", "at_udeg": [east(100.0), LAT]});
        let route = json!({"lifts": [["w1", 40]], "paved_pct": 30, "big_road_pct": 0, "water_pct": 0, "snow_free": [],
                           "back_s": 100});
        assert_eq!(files["9-1-2"], json!({"format": 1, "places": {"w1": lift, "w4": parking}, "routes": {"7": route}}));
        let keys = |value: &Value| value.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(
            (keys(&files["9-1-1"]["places"]), keys(&files["9-1-1"]["routes"])),
            (vec!["w1".into(), "w4".into(), "w5".into()], vec!["-501".into(), "7".into()])
        );
        assert_eq!(files["9-1-3"], json!({"format": 1, "places": {}, "routes": {}}));
    }
}
