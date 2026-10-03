//! The route answer that clients decode, as `specs/route-api.md` specifies it.
use crate::{
    model::Totals,
    router::{Response, Route},
};
use serde_json::{json, Value};

pub fn answer(response: &Response) -> Value {
    json!({ "routes": response.routes.iter().map(route).collect::<Vec<_>>() })
}

fn route(route: &Route) -> Value {
    let mut previous = [0i64; 2];
    let coordinates: Vec<i64> = route
        .geometry
        .iter()
        .flat_map(|&[lon, lat]| [delta(&mut previous[0], lon, 1e6), delta(&mut previous[1], lat, 1e6)])
        .collect();
    let mut height = 0;
    let elevation: Vec<Option<i64>> =
        route.elevation.iter().map(|metres| metres.map(|metres| delta(&mut height, metres as f64, 10.0))).collect();
    let mut time = 0;
    let elapsed: Vec<i64> = route.elapsed.iter().map(|&seconds| delta(&mut time, seconds, 1.0)).collect();
    json!({
        "id": route.id,
        "reason": route.reason,
        "package": route.package,
        "profile": route.profile,
        "coordinates_udeg": coordinates,
        "elevation_dm": elevation,
        "elapsed_s": elapsed,
        "surfaces": runs(&route.surfaces),
        "pushing": runs(&route.pushing),
        "closures": runs(&route.closures),
        "legs": route.legs.iter().zip(&route.attachments[1..]).map(|(leg, end)| json!({
            "from_index": leg.from_index,
            "to_index": leg.to_index,
            "start": leg.start_attachment.position.id(),
            "end": end.position.id(),
            "totals": totals(&leg.totals),
        })).collect::<Vec<_>>(),
        "snap_truncated": route.snap_truncated,
        "totals": totals(&route.totals),
    })
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

fn runs<T: PartialEq>(values: &[T]) -> Vec<(&T, usize)> {
    let mut runs: Vec<(&T, usize)> = Vec::new();
    for value in values {
        match runs.last_mut() {
            Some((last, length)) if *last == value => *length += 1,
            _ => runs.push((value, 1)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        model::Point,
        router::Leg,
        snap::{Candidate, Position},
    };

    #[test]
    fn encodes_the_shared_vector() {
        let vector: Value = serde_json::from_str(include_str!("../../../specs/vectors/route-answer.json")).unwrap();
        let source = &vector["route"];
        let attachment = |position: &Value| Candidate {
            position: Position {
                road: position["road"].as_u64().unwrap() as u32,
                fraction: position["fraction"].as_f64().unwrap(),
            },
            projected: Point::default(),
            snap_distance_m: 0.0,
            segment: 0,
            segment_fraction: 0.0,
        };
        let source_legs = source["legs"].as_array().unwrap();
        let legs = source_legs
            .iter()
            .map(|leg| Leg {
                start_attachment: attachment(&leg["start"]),
                from_index: leg["from_index"].as_u64().unwrap() as usize,
                to_index: leg["to_index"].as_u64().unwrap() as usize,
                totals: serde_json::from_value(leg["totals"].clone()).unwrap(),
                roads: vec![],
            })
            .collect();
        let attachments = std::iter::once(attachment(&source_legs[0]["start"]))
            .chain(source_legs.iter().map(|leg| attachment(&leg["end"])))
            .collect();
        assert_eq!(source["reason"], "primary");
        let route = Route {
            id: serde_json::from_value(source["id"].clone()).unwrap(),
            reason: "primary",
            package: serde_json::from_value(source["package"].clone()).unwrap(),
            profile: serde_json::from_value(source["profile"].clone()).unwrap(),
            cost: 0,
            geometry: serde_json::from_value(source["geometry"].clone()).unwrap(),
            elevation: serde_json::from_value(source["elevation"].clone()).unwrap(),
            surfaces: serde_json::from_value(source["surfaces"].clone()).unwrap(),
            pushing: serde_json::from_value(source["pushing"].clone()).unwrap(),
            closures: serde_json::from_value(source["closures"].clone()).unwrap(),
            elapsed: serde_json::from_value(source["elapsed"].clone()).unwrap(),
            legs,
            attachments,
            snap_truncated: serde_json::from_value(source["snap_truncated"].clone()).unwrap(),
            totals: serde_json::from_value(source["totals"].clone()).unwrap(),
        };
        assert_eq!(answer(&Response { routes: vec![route] }), vector["answer"]);
    }
}
