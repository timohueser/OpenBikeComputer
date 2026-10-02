//! The route answer that clients decode, as `specs/route-api.md` specifies it.
use route_engine::router::{Response, Route};
use serde_json::{json, Value};

pub(crate) fn answer(response: &Response) -> Value {
    json!({ "routes": response.routes.iter().map(route).collect::<Vec<_>>() })
}

fn route(route: &Route) -> Value {
    let mut previous = [0i64; 2];
    let coordinates: Vec<i64> = route
        .geometry
        .iter()
        .flat_map(|point| {
            let current = point.map(|degrees| (degrees * 1e6).round() as i64);
            let delta = [current[0] - previous[0], current[1] - previous[1]];
            previous = current;
            delta
        })
        .collect();
    let mut height = 0i64;
    let elevation: Vec<Option<i64>> = route
        .elevation
        .iter()
        .map(|metres| {
            metres.map(|metres| {
                let current = (metres as f64 * 10.0).round() as i64;
                let delta = current - height;
                height = current;
                delta
            })
        })
        .collect();
    let mut time = 0i64;
    let elapsed: Vec<i64> = route
        .elapsed
        .iter()
        .map(|seconds| {
            let current = seconds.round() as i64;
            let delta = current - time;
            time = current;
            delta
        })
        .collect();
    let totals = &route.totals;
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
        "legs": route.legs.iter().map(|leg| json!({ "from_index": leg.from_index, "to_index": leg.to_index })).collect::<Vec<_>>(),
        "snap_truncated": route.snap_truncated,
        "totals": {
            "distance_m": totals.distance_m,
            "ascent_m": totals.ascent_m,
            "seconds": totals.seconds.round() as u64,
            "surface_m": totals.surface_m,
            "unknown_elevation_m": totals.unknown_elevation_m,
            "pushing_m": totals.pushing_m,
        },
    })
}

fn runs<T: Copy + PartialEq>(values: &[T]) -> Vec<(T, usize)> {
    let mut runs: Vec<(T, usize)> = Vec::new();
    for &value in values {
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
    use route_engine::{
        model::Point,
        router::Leg,
        snap::{Candidate, Position},
    };

    #[test]
    fn encodes_the_shared_vector() {
        let vector: Value = serde_json::from_str(include_str!("../../../specs/vectors/route-answer.json")).unwrap();
        let source = &vector["route"];
        let attachment = Candidate {
            position: Position { road: 0, fraction: 0.0 },
            projected: Point::default(),
            snap_distance_m: 0.0,
            segment: 0,
            segment_fraction: 0.0,
        };
        let legs = source["legs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|leg| Leg {
                start_attachment: attachment.clone(),
                from_index: leg["from_index"].as_u64().unwrap() as usize,
                to_index: leg["to_index"].as_u64().unwrap() as usize,
                totals: Default::default(),
                roads: vec![],
            })
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
            elapsed: serde_json::from_value(source["elapsed"].clone()).unwrap(),
            legs,
            attachments: vec![attachment],
            snap_truncated: serde_json::from_value(source["snap_truncated"].clone()).unwrap(),
            totals: serde_json::from_value(source["totals"].clone()).unwrap(),
            warnings: vec!["Unread by clients".into()],
        };
        assert_eq!(answer(&Response { routes: vec![route] }), vector["answer"]);
    }
}
