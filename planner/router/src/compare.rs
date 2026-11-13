//! The request set and the check that compare two engines on the two formats of one bake.
use crate::{
    data::RoutingData,
    model::{Point, NO_ELEVATION},
    router::Response,
    snap::{Candidates, Policy, REACH_M},
    Control, Request, Router,
};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

/// The kinds of request, by `index % 8`.
pub const KINDS: [&str; 8] =
    ["points", "junctions", "via-junction", "turnaround", "pinned", "one-road", "one-road-back", "alternatives"];

/// The most distance of a request with alternatives.
const ALTERNATIVES_M: f64 = 40_000.0;

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn coordinate(point: Point) -> [f64; 2] {
    [f64::from(point.lon) * 1e-6, f64::from(point.lat) * 1e-6]
}

/// The point at `fraction` of the length of `shape`.
fn along(shape: &[Point], fraction: f64) -> Point {
    let mut rest = shape.windows(2).map(|pair| pair[0].distance(pair[1])).sum::<f64>() * fraction;
    for pair in shape.windows(2) {
        let length = pair[0].distance(pair[1]);
        if rest <= length && length > 0.0 {
            let t = rest / length;
            let at = |a: i32, b: i32| a + (f64::from(b - a) * t).round() as i32;
            return Point { lat: at(pair[0].lat, pair[1].lat), lon: at(pair[0].lon, pair[1].lon), ..pair[0] };
        }
        rest -= length;
    }
    shape[shape.len() - 1]
}

/// The candidates of `point` as the router snaps it: within 250 m, else within `REACH_M`. `None`
/// when it has none or the data has no roads there.
fn snap<P: RoutingData>(data: &P, point: Point, profile: &str) -> Option<Candidates> {
    let found = data.snap(point, profile, Policy::default()).ok()?;
    let found = if found.retained.is_empty() {
        data.snap(point, profile, Policy { radius_m: REACH_M, ..Policy::default() }).ok()?
    } else {
        found
    };
    (!found.retained.is_empty()).then_some(found)
}

/// `count` requests of `profile`. A fixed xorshift from `seed` draws the points in the bounds; a
/// point counts when it snaps. Request `i` has the kind `KINDS[i % 8]`, and its later spots lie
/// within `max_m` of its first. The list is shorter when 100 tries for each request do not give
/// `count`, as for a profile that snaps nowhere in the bounds.
pub fn requests<P: RoutingData>(
    router: &Router<P>,
    profile: &str,
    count: usize,
    seed: u64,
    max_m: f64,
) -> Vec<Request> {
    let data = router.package();
    let [west, south, east, north] = data.bounds();
    let mut state = seed.max(1);
    let mut uniform = move || (xorshift(&mut state) >> 11) as f64 / (1u64 << 53) as f64;
    let mut requests = Vec::with_capacity(count);
    for _ in 0..count.saturating_mul(100) {
        if requests.len() == count {
            break;
        }
        let kind = requests.len() % 8;
        let max_m = if kind == 7 { max_m.min(ALTERNATIVES_M) } else { max_m };
        // A spot: a drawn point that snaps, and its nearest road.
        let mut spot = |around: Option<Point>| {
            for _ in 0..1_000 {
                let point = match around {
                    None => [west + uniform() * (east - west), south + uniform() * (north - south)],
                    Some(a) => {
                        let lat = f64::from(a.lat) * 1e-6;
                        let dlat = max_m / 111_320.0;
                        let dlon = dlat / lat.to_radians().cos().max(0.01);
                        [f64::from(a.lon) * 1e-6 + (2.0 * uniform() - 1.0) * dlon, lat + (2.0 * uniform() - 1.0) * dlat]
                    }
                };
                if !(west..=east).contains(&point[0]) || !(south..=north).contains(&point[1]) {
                    continue;
                }
                let point = Point {
                    lat: (point[1] * 1e6).round() as i32,
                    lon: (point[0] * 1e6).round() as i32,
                    elevation: NO_ELEVATION,
                };
                if around.is_some_and(|a| a.distance(point) > max_m) {
                    continue;
                }
                if let Some(found) = snap(data, point, profile) {
                    return Some((point, found));
                }
            }
            None
        };
        let Some((a, found_a)) = spot(None) else { continue };
        let Some((_, found_b)) = spot(Some(a)) else { continue };
        let Some((_, found_m)) = spot(Some(a)) else { continue };
        let road = |found: &Candidates| found.retained[0].position.road;
        let shape = |found: &Candidates| data.with_road(road(found), |road| road.shape.clone()).ok();
        let (Some(shape_a), Some(shape_b), Some(shape_m)) = (shape(&found_a), shape(&found_b), shape(&found_m)) else {
            continue;
        };
        let projected = |found: &Candidates| found.retained[0].projected;
        let end = |shape: &Vec<Point>| shape[shape.len() - 1];
        let (pa, pb, pm) = (projected(&found_a), projected(&found_b), projected(&found_m));
        let points = match kind {
            1 => vec![end(&shape_a), end(&shape_b)],
            2 => vec![pa, end(&shape_m), pb],
            3 => {
                let Some((_, found_n)) = spot(Some(a)) else { continue };
                vec![pa, pm, projected(&found_n), pb]
            }
            5 => vec![along(&shape_a, 0.25), along(&shape_a, 0.75)],
            6 => vec![along(&shape_a, 0.75), along(&shape_a, 0.25)],
            _ => vec![pa, pb],
        };
        let mut request =
            json!({ "points": points.iter().map(|&p| coordinate(p)).collect::<Vec<_>>(), "profile": profile });
        match kind {
            3 => request["turnarounds"] = json!([1]),
            4 => {
                // The last candidate of each point, as the router snaps the point of the request.
                let (Some(first), Some(last)) = (snap(data, points[0], profile), snap(data, points[1], profile)) else {
                    continue;
                };
                let pin = |found: &Candidates| found.retained[found.retained.len() - 1].position.id();
                request["start_position"] = json!(pin(&first));
                request["end_position"] = json!(pin(&last));
            }
            7 => request["alternatives"] = json!(true),
            _ => {}
        }
        requests.push(serde_json::from_value(request).expect("a request of known fields"));
    }
    requests
}

/// How the two engines answered a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Same,
    /// A route has other roads at the same cost, and the old data costs the new roads exactly.
    Tie,
}

/// The answer of a route without the name of its package.
fn answers(response: &Response) -> Vec<Value> {
    let answer = crate::answer::answer(response);
    let mut routes = answer["routes"].as_array().cloned().unwrap_or_default();
    for route in &mut routes {
        if let Some(route) = route.as_object_mut() {
            route.remove("package");
        }
    }
    routes
}

/// Routes `request` on both routers and compares the answers; also gives the time of each side.
/// A difference is the `Err`.
pub fn check<A: RoutingData, B: RoutingData>(
    old: &mut Router<A>,
    new: &mut Router<B>,
    request: &Request,
) -> Result<(Outcome, [Duration; 2]), String> {
    let timed = |route: &mut dyn FnMut() -> crate::Result<Response>| {
        let started = Instant::now();
        let result = route();
        (result, started.elapsed())
    };
    let (a, old_time) = timed(&mut || old.routes(request, &Control::default()));
    let (b, new_time) = timed(&mut || new.routes(request, &Control::default()));
    let times = [old_time, new_time];
    let (a, b) = match (a, b) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(a), Err(b)) if std::mem::discriminant(&a) == std::mem::discriminant(&b) => {
            return Ok((Outcome::Same, times))
        }
        (a, b) => return Err(format!("old {:?}, new {:?}", a.map(|r| r.routes.len()), b.map(|r| r.routes.len()))),
    };
    let (answers_a, answers_b) = (answers(&a), answers(&b));
    for (k, (x, y)) in a.routes.iter().zip(&b.routes).enumerate() {
        let position = |p: &crate::snap::Position| (p.road, p.fraction.to_bits());
        let ends = |route: &crate::Route| -> Vec<_> {
            route.legs.iter().map(|leg| (position(&leg.start), position(&leg.end))).collect()
        };
        if (x.reason, x.cost, x.snap_truncated, ends(x)) != (y.reason, y.cost, y.snap_truncated, ends(y)) {
            return Err(format!(
                "route {k}: old {} {} {:?}, new {} {} {:?}",
                x.reason,
                x.cost,
                ends(x),
                y.reason,
                y.cost,
                ends(y)
            ));
        }
        let pieces = |route: &crate::Route| -> Vec<_> {
            route.pieces().map(|piece| (piece.road, piece.from.to_bits(), piece.to.to_bits())).collect()
        };
        if pieces(x) != pieces(y) {
            return match old.cost(y, &y.profile) {
                Ok(Some(cost)) if cost == y.cost => Ok((Outcome::Tie, times)),
                cost => Err(format!("route {k}: other roads at cost {}, which the old data costs {cost:?}", y.cost)),
            };
        }
        if answers_a.get(k) != answers_b.get(k) {
            return Err(format!("route {k}: the answers differ"));
        }
    }
    if a.routes.len() != b.routes.len() {
        return Err(format!("old {} routes, new {}", a.routes.len(), b.routes.len()));
    }
    Ok((Outcome::Same, times))
}
