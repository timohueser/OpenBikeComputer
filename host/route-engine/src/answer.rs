//! The route answer that clients decode, as `specs/route-api.md` specifies it.
use crate::{
    model::{Totals, NO_ELEVATION, PUSH},
    router::{Piece, Response, Route},
};
use serde::Serialize;
use serde_json::{json, Value};

pub fn answer(response: &Response) -> Value {
    json!({ "routes": response.routes.iter().map(route).collect::<Vec<_>>() })
}

fn route(route: &Route) -> Value {
    let mut previous = [0i64; 2];
    let coordinates: Vec<i64> = route
        .points
        .iter()
        .flat_map(|p| {
            let current = [p.lon as i64, p.lat as i64];
            let delta = [current[0] - previous[0], current[1] - previous[1]];
            previous = current;
            delta
        })
        .collect();
    let mut height = 0;
    let elevation: Vec<Option<i64>> = route
        .points
        .iter()
        .map(|p| (p.elevation != NO_ELEVATION).then(|| delta(&mut height, p.elevation as f64, 10.0)))
        .collect();
    let mut time = 0;
    let elapsed: Vec<i64> = route.elapsed.iter().map(|&seconds| delta(&mut time, seconds, 1.0)).collect();
    let mut value = json!({
        "id": route.id,
        "reason": route.reason,
        "package": route.package,
        "profile": route.profile,
        "coordinates_udeg": coordinates,
        "elevation_dm": elevation,
        "elapsed_s": elapsed,
        "edges": edges(route),
        "legs": route.legs.iter().map(|leg| json!({
            "from_index": leg.from_index,
            "to_index": leg.to_index,
            "start": leg.start.id(),
            "end": leg.end.id(),
            "totals": totals(&leg.totals),
        })).collect::<Vec<_>>(),
        "snap_truncated": route.snap_truncated,
        "totals": totals(&route.totals()),
    });
    if let Some(via) = route.via {
        value["via"] = json!(via);
    }
    value
}

/// The run channels; a route of one point has no edges and no channels.
fn edges(route: &Route) -> Value {
    if route.points.len() < 2 {
        return json!({});
    }
    json!({
        "surfaces": runs(route, |p| p.surface),
        "pushing": runs(route, |p| p.mode == PUSH),
        "closures": runs(route, |p| &p.closures),
        "sac_scale": runs(route, |p| p.sac_scale),
    })
}

/// One fact of each edge in runs: each value with the number of consecutive edges it covers.
fn runs<'a, T: PartialEq + Serialize>(route: &'a Route, fact: impl Fn(&'a Piece) -> T) -> Value {
    let mut runs: Vec<(T, usize)> = Vec::new();
    let mut previous = 0;
    for piece in route.pieces() {
        let edges = piece.end - previous;
        previous = piece.end;
        if edges == 0 {
            continue;
        }
        let value = fact(piece);
        match runs.last_mut() {
            Some((last, length)) if *last == value => *length += edges,
            _ => runs.push((value, edges)),
        }
    }
    json!(runs)
}

fn totals(totals: &Totals) -> Value {
    json!({
        "distance_m": totals.distance_m,
        "ascent_m": totals.ascent_m,
        "seconds": totals.seconds.round() as u64,
        "surface_m": totals.surface_m,
        "unknown_elevation_m": totals.unknown_elevation_m,
        "pushing_m": totals.pushing_m,
    })
}

/// Rounds the absolute value before the difference, so rounding errors do not add up along the route.
fn delta(previous: &mut i64, value: f64, scale: f64) -> i64 {
    let current = (value * scale).round() as i64;
    let delta = current - *previous;
    *previous = current;
    delta
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::{Point, Surface, BIKE},
        router::Leg,
        snap::Position,
    };

    fn value<T: serde::de::DeserializeOwned>(value: &Value) -> T {
        serde_json::from_value(value.clone()).unwrap()
    }

    /// A route of the vector. It has one value per edge in each channel and becomes one piece per
    /// edge, so equal neighbours must join into one run.
    fn source(source: &Value) -> Route {
        let position = |p: &Value| Position { road: value(&p["road"]), fraction: value(&p["fraction"]) };
        let geometry: Vec<[f64; 2]> = value(&source["geometry"]);
        let elevation: Vec<Option<f32>> = value(&source["elevation"]);
        let points = geometry
            .iter()
            .zip(elevation)
            .map(|(&[lon, lat], height)| Point {
                lon: (lon * 1e6).round() as i32,
                lat: (lat * 1e6).round() as i32,
                elevation: height.unwrap_or(NO_ELEVATION),
            })
            .collect();
        let edges = &source["edges"];
        let piece = |end: usize| Piece {
            road: 0,
            from: 0.0,
            to: 1.0,
            end,
            surface: value::<Surface>(&edges["surfaces"][end - 1]),
            mode: if edges["pushing"][end - 1] == true { PUSH } else { BIKE },
            closures: value(&edges["closures"][end - 1]),
            sac_scale: value(&edges["sac_scale"][end - 1]),
            mtb_scale: None,
        };
        let legs = source["legs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|leg| {
                let (from_index, to_index) = (value(&leg["from_index"]), value(&leg["to_index"]));
                Leg {
                    start: position(&leg["start"]),
                    end: position(&leg["end"]),
                    from_index,
                    to_index,
                    totals: value(&leg["totals"]),
                    pieces: (from_index + 1..=to_index).map(piece).collect(),
                }
            })
            .collect();
        Route {
            id: value(&source["id"]),
            reason: value::<String>(&source["reason"]).leak(),
            package: value(&source["package"]),
            profile: value(&source["profile"]),
            cost: 0,
            points,
            elapsed: value(&source["elapsed"]),
            legs,
            snap_truncated: value(&source["snap_truncated"]),
            via: value(&source["via"]),
        }
    }

    #[test]
    fn encodes_the_shared_vector() {
        let vector: Value = serde_json::from_str(include_str!("../../../specs/vectors/route-answer.json")).unwrap();
        let routes = vector["routes"].as_array().unwrap().iter().map(source).collect();
        assert_eq!(answer(&Response { routes }), vector["answer"]);
    }
}
