//! The pairs that two checkouts share: the pairs file, the routes of the compare pairs, and the
//! join of two runs for the blind comparison.
use super::measures::measure;
use super::net::Net;
use super::panel::{junctions, sample, Class, Pair, Spec};
use crate::traffic::{self, Protection};
use planner_router::model::Point;
use planner_router::shape::{self, Near, P};
use rayon::prelude::*;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

/// The pairs of the blind comparison: 40 pairs, a third each within 10 % of 6, 12 and 22 km.
pub const COMPARE: &[Class] = &[(5.4, 6.6, 14), (10.8, 13.2, 13), (19.8, 24.2, 13)];
/// The tolerance of a stored line.
const LINE_M: f64 = 3.0;
/// The margin of the overlay around the pairs in microdegrees: about 1 km.
const MARGIN: i32 = 12_000;
/// Two routes are the same when each has at most this share of its length farther than
/// `shape::NEAR_M` from the other.
const SAME: f64 = 0.10;

/// The start and the end of a pair, each as latitude and longitude in degrees.
pub type Ends = [[f64; 2]; 2];
/// The sets of pairs of a pairs file, by their names.
pub type Sets = BTreeMap<String, Vec<Ends>>;

/// The sets of the file `path`, which has each set of `specs`. Where the file is missing, `net`
/// draws the pairs of each set and the file gets them as coordinates. An existing file is only
/// read, so that each run with it has the same pairs.
pub fn sets(path: &Path, net: &Net, specs: &[(&str, Spec)]) -> Result<Sets, String> {
    let named = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
    if !path.exists() {
        let place = |junction: u32| {
            let point = net.graph.points.values()[junction as usize];
            [f64::from(point.lat) / 1e6, f64::from(point.lon) / 1e6]
        };
        let mut drawn = Sets::new();
        for (name, spec) in specs {
            let pairs = sample(net, spec)?.iter().map(|pair| [place(pair.from), place(pair.to)]).collect();
            drawn.insert(name.to_string(), pairs);
        }
        std::fs::write(path, serde_json::to_string(&drawn).expect("numbers")).map_err(|e| named(&e))?;
    }
    let sets: Sets = serde_json::from_slice(&std::fs::read(path).map_err(|e| named(&e))?).map_err(|e| named(&e))?;
    match specs.iter().find(|(name, _)| !sets.contains_key(*name)) {
        Some((name, _)) => Err(named(&format!("no set `{name}`"))),
        None => Ok(sets),
    }
}

/// The junctions of `pairs` for the profile of `net`: each end snaps to the nearest junction where
/// a road with the snap bit of the profile arrives.
pub fn snap(net: &Net, pairs: &[Ends]) -> Result<Vec<Pair>, String> {
    let (points, candidates) = (net.graph.points.values(), junctions(net, None));
    let nearest = |[lat, lon]: [f64; 2]| {
        let at = Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: 0.0 };
        let apart = |junction: &u32| at.distance(points[*junction as usize]);
        candidates.iter().copied().min_by(|a, b| apart(a).total_cmp(&apart(b))).ok_or("the profile has no junction")
    };
    let snap = |&[from, to]: &Ends| -> Result<Pair, String> { Ok(Pair { from: nearest(from)?, to: nearest(to)? }) };
    pairs.par_iter().map(snap).collect()
}

/// The line of `roads` within `LINE_M`, as latitudes and longitudes in degrees.
fn stored(net: &Net, roads: &[u32]) -> Vec<[f64; 2]> {
    let line = net.line(roads);
    let line = shape::simplify(&line, &[0, line.len() - 1], LINE_M).0;
    line.iter().map(|p| [f64::from(p[1]) / 1e6, f64::from(p[0]) / 1e6]).collect()
}

/// The cycling facts of the map for the compare page: the lines of the ways around `pairs`, by
/// kind. A cycle path is a cycleway, or a path or footway with `bicycle=designated`. A walking
/// profile has only the ways of its signed routes. The two directions of a piece of a way give
/// one line.
pub fn overlay(net: &Net, pairs: &[Pair]) -> Value {
    let (graph, roads, points) = (net.graph, &net.graph.roads, net.graph.points.values());
    let ends: Vec<_> = pairs.iter().flat_map(|pair| [points[pair.from as usize], points[pair.to as usize]]).collect();
    let span = |values: Vec<i32>| {
        values.iter().min().map_or(0, |least| least - MARGIN)..=values.iter().max().map_or(0, |most| most + MARGIN)
    };
    let (lats, lons) = (span(ends.iter().map(|p| p.lat).collect()), span(ends.iter().map(|p| p.lon).collect()));
    let inside =
        |junction: u32| lats.contains(&points[junction as usize].lat) && lons.contains(&points[junction as usize].lon);
    let (mut lines, mut seen) = ([const { Vec::new() }; 3], HashSet::new());
    for road in 0..roads.len() {
        let (row, from, to) = (roads.way_row(road), roads.from(road), roads.to(road));
        let piece = (row, roads.length_m.values()[road], from.min(to), from.max(to));
        if !(inside(from) || inside(to)) || !seen.insert(piece) {
            continue;
        }
        let way = graph.osm.ways.row(row as usize);
        let designated =
            matches!(way.tags.get("highway"), Some("path" | "footway")) && way.tags.contains("bicycle", "designated");
        let protection = |reversed| traffic::protection(&way.tags, reversed, way.facts.country());
        let kinds = [
            !net.profile.walking && (roads.class(road) == 0 || designated),
            !net.profile.walking && (protection(false) != Protection::None || protection(true) != Protection::None),
            net.routes.contains_key(&row),
        ];
        for (lines, _) in lines.iter_mut().zip(kinds).filter(|(_, kind)| *kind) {
            lines.push(stored(net, &[road as u32]));
        }
    }
    let [paths, lanes, signed] = lines;
    json!({"cycle path": paths, "cycle lane or track": lanes, "signed route": signed})
}

/// The route of each pair with the profile of `net`: its line as latitudes and longitudes, its
/// km, its ascent and its measures, or `null` where the pair has no route.
pub fn routes(net: &Net, pairs: &[Pair]) -> Result<Value, String> {
    let route = |pair: &Pair| -> Result<Value, String> {
        let roads = net.route(pair.from, pair.to)?.filter(|roads| !roads.is_empty());
        let Some(roads) = roads else { return Ok(Value::Null) };
        let measures = measure(net, &roads);
        let rates: serde_json::Map<_, _> =
            measures.rates().into_iter().map(|(name, rate)| (name.to_string(), json!(super::round(rate, 2)))).collect();
        Ok(json!({
            "line": stored(net, &roads),
            "km": super::round(measures.length_m / 1000.0, 2),
            "ascent_m": measures.ascent_m,
            "measures": rates,
        }))
    };
    pairs.par_iter().map(route).collect::<Result<Vec<Value>, String>>().map(Value::Array)
}

/// The line of a route of `routes`, in microdegrees of longitude and latitude.
fn line(route: &Value) -> Vec<P> {
    let degrees = |value: &Value| (value.as_f64().unwrap_or(0.0) * 1e6).round() as i32;
    route["line"].as_array().into_iter().flatten().map(|point| [degrees(&point[1]), degrees(&point[0])]).collect()
}

/// The data of the compare page for two runs of the same pairs, `{"pairs": …, "routes": …}` each,
/// with the names `names`: for each profile of both runs, the pairs with a route in both runs
/// whose two routes are not the same, and the overlay of the first run. Also returns the count
/// of pairs with no route in a run.
pub fn compare(runs: [&Value; 2], names: [&str; 2]) -> Result<(Value, usize), String> {
    if runs[0]["pairs"] != runs[1]["pairs"] {
        return Err("the two runs have different pairs".into());
    }
    let pairs = runs[0]["pairs"].as_array().ok_or("a run has no pairs")?;
    let (mut sets, mut missing) = (Vec::new(), 0);
    for (profile, first) in runs[0]["routes"].as_object().ok_or("a run has no routes")? {
        let Some(second) = runs[1]["routes"].get(profile) else { continue };
        let mut kept = Vec::new();
        for (i, ends) in pairs.iter().enumerate() {
            let routes = [&first[i], &second[i]];
            if routes.iter().any(|route| route.is_null()) {
                missing += 1;
                continue;
            }
            let lines = routes.map(line);
            let same = |a: usize| shape::far(&lines[a], &Near::new(&lines[1 - a])) <= SAME * shape::length(&lines[a]);
            if !(same(0) && same(1)) {
                kept.push(json!({"id": i + 1, "ends": ends, "routes": routes}));
            }
        }
        sets.push(json!({"profile": profile, "pairs": kept, "overlay": runs[0]["overlay"][profile]}));
    }
    if sets.is_empty() {
        return Err("the two runs have no profile in common".into());
    }
    Ok((json!({"names": names, "sets": sets}), missing))
}

/// The compare page with `data` in place of its data marker.
pub fn page(data: &Value) -> String {
    // `<` as an escape, so that no text of the data ends the script element.
    include_str!("compare.html").replace("/*DATA*/null", &data.to_string().replace('<', "\\u003c"))
}
