//! Routes one request set on two packages and prints each request whose cost or error differs:
//! `compare OLD NEW COUNT SEED`. A request runs from the middle of one road of OLD to the middle of
//! another, at most 75 km away. When more candidates are near a point than the snap keeps
//! (`snap_truncated`), the snap cuts them by distance, then by road ID: so only where the last kept
//! and the first cut candidate are at the same distance can other road IDs keep other candidates.
//! A difference is a fault unless a point of its request has such a tie. Exits with 1 on a fault,
//! with 2 when the packages have other profiles.
use planner_router::{
    data::RoutingData,
    model::{Point, NO_ELEVATION},
    snap::Policy,
    Control, Error, Request, Router,
};
use std::path::Path;

/// The point at half the length of `shape`.
fn middle(shape: &[Point]) -> Point {
    let mut rest = shape.windows(2).map(|pair| pair[0].distance(pair[1])).sum::<f64>() / 2.0;
    for pair in shape.windows(2) {
        let length = pair[0].distance(pair[1]);
        if rest <= length && length > 0.0 {
            let t = rest / length;
            let along = |a: i32, b: i32| a + (f64::from(b - a) * t).round() as i32;
            return Point { lat: along(pair[0].lat, pair[1].lat), lon: along(pair[0].lon, pair[1].lon), ..pair[0] };
        }
        rest -= length;
    }
    shape[shape.len() - 1]
}

/// The name of the kind of `error`, without its details.
fn kind(error: &Error) -> String {
    format!("{error:?}").split('(').next().unwrap_or_default().to_string()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let [_, old, new, count, seed] = &args[..] else { return Err("Usage: compare OLD NEW COUNT SEED".into()) };
    let (count, mut state): (usize, u64) = (count.parse()?, seed.parse()?);
    if state == 0 {
        return Err("SEED is a number above 0".into());
    }
    let (old, new) = (planner_router::open(Path::new(old))?, planner_router::open(Path::new(new))?);
    let profiles: Vec<String> = old.profiles().into_iter().map(str::to_string).collect();
    if new.profiles() != old.profiles() {
        eprintln!("The packages have other profiles: {:?} and {:?}", old.profiles(), new.profiles());
        std::process::exit(2);
    }
    let roads = u64::from(old.package().manifest().roads);
    let mut requests = Vec::new();
    while requests.len() < count {
        let [a, b] = [0; 2].map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            old.with_road((state % roads) as u32, |road| middle(&road.shape))
        });
        let (a, b) = (a?, b?);
        if a.distance(b) <= 75_000.0 {
            requests.push([a, b].map(|p| [f64::from(p.lon) * 1e-6, f64::from(p.lat) * 1e-6]));
        }
    }
    // Whether the snap of `point` on `package` cuts between two candidates at the same distance.
    let tie = |package: &planner_router::data::Selection<_>, [lon, lat]: [f64; 2], profile: &str| {
        let point = Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION };
        let kept = Policy::default().max_candidates;
        let found = package.snap(point, profile, Policy { max_candidates: kept + 1, ..Policy::default() })?;
        let distance = |i: usize| found.retained.get(i).map(|candidate| candidate.snap_distance_m);
        Ok::<_, Error>(distance(kept).is_some() && distance(kept - 1) == distance(kept))
    };
    let (old_budget, new_budget) = (old.default_budget(), new.default_budget());
    let mut routers = [Router::new(old.fork(), old_budget), Router::new(new.fork(), new_budget)];
    let (mut truncated, mut differences, mut truncated_differences, mut ties) = (0, 0, 0, 0);
    for profile in &profiles {
        for points in &requests {
            let request: Request = serde_json::from_value(serde_json::json!({"points": points, "profile": profile}))?;
            let answers = routers.each_mut().map(|router| router.routes(&request, &Control::default()));
            let cut =
                answers.iter().any(|answer| answer.as_ref().is_ok_and(|response| response.routes[0].snap_truncated));
            truncated += usize::from(cut);
            let [old_cost, new_cost] = answers.map(|answer| match answer {
                Ok(response) => response.routes[0].cost.to_string(),
                Err(error) => kind(&error),
            });
            if old_cost == new_cost {
                continue;
            }
            if !cut {
                println!("{profile}, {points:?}, {old_cost}, {new_cost}");
                differences += 1;
                continue;
            }
            let mut at_tie = false;
            for &point in points {
                at_tie |= tie(&old, point, profile)? || tie(&new, point, profile)?;
            }
            println!("truncated, tie at the cut {at_tie}: {profile}, {points:?}, {old_cost}, {new_cost}");
            truncated_differences += 1;
            ties += usize::from(at_tie);
        }
    }
    println!(
        "{} requests, {differences} differences; {truncated} truncated, {truncated_differences} differences, {ties} of them at a tie",
        profiles.len() * requests.len()
    );
    std::process::exit(i32::from(differences + truncated_differences > ties));
}
