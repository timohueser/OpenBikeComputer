//! Routes one request set on the two formats of one bake and prints each request:
//! `compare OLD NEW COUNT SEED [--max-km K] [--profile P] [--cold] [--no-announce]`. OLD is format
//! 9 and NEW an install of format 10 (`cells install`); COUNT is the requests of each profile, or of
//! `P` only. `--cold` opens NEW again with its files out of the page cache before each request, and
//! prints the time and the major page faults of that first route of NEW; `--no-announce` makes its
//! host silent. Exits with 1 on a difference, with 2 when the packages have other profiles.
//!
//! "old ms" and "new ms" are the time of the whole request. With `--cold`, "new ms" is a second,
//! warm route that reuses the snaps of the first, so it leaves out the snap time. The other columns
//! of NEW are the sums of the searches of the request: the time of the searches and of the
//! unpacking, the settled labels by the level of their expansion (level 0 first), the table values
//! read, the roads of the unpacked legs, the labels that the unpacking settled, and the most bytes
//! that one search held.
use planner_router::{
    compare::{self, Outcome, KINDS},
    data::RoutingData,
    host::Maps,
    model::{Point, NO_ELEVATION},
    snap::Policy,
    Control, Error, Opened, Request, Router,
};
use std::{path::Path, time::Instant};

/// Opens NEW; with `silent`, on a host whose `announce` does nothing.
fn open_new(path: &Path, silent: bool) -> Result<Router<Opened>, Error> {
    Ok(Router::new(Opened::Cells(planner_router::open::cells(path, silent)?), usize::MAX))
}

/// The major page faults of this process (field 12 of `/proc/self/stat`).
fn major_faults() -> u64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let fields: Vec<&str> = stat.rsplit_once(')').map_or("", |(_, rest)| rest).split_whitespace().collect();
    fields.get(9).and_then(|field| field.parse().ok()).unwrap_or(0)
}

fn point([lon, lat]: [f64; 2]) -> Point {
    Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION }
}

/// The greatest distance from the first point of `request`, in km.
fn straight_km(request: &Request) -> f64 {
    let first = point(request.points[0]);
    request.points.iter().map(|&p| first.distance(point(p))).fold(0.0, f64::max) / 1000.0
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let usage = "Usage: compare OLD NEW COUNT SEED [--max-km K] [--profile P] [--cold] [--no-announce]";
    let [_, old_path, new_path, count, seed, options @ ..] = &args[..] else { return Err(usage.into()) };
    let (count, seed): (usize, u64) = (count.parse()?, seed.parse()?);
    if seed == 0 {
        return Err("SEED is a number above 0".into());
    }
    let (mut max_km, mut only, mut cold, mut silent) = (75.0, None, false, false);
    let mut options = options.iter();
    while let Some(option) = options.next() {
        match option.as_str() {
            "--max-km" => max_km = options.next().ok_or(usage)?.parse()?,
            "--profile" => only = Some(options.next().ok_or(usage)?.clone()),
            "--cold" => cold = true,
            "--no-announce" => silent = true,
            _ => return Err(usage.into()),
        }
    }
    let new_path = Path::new(new_path);
    let mut old = Router::new(planner_router::open(Path::new(old_path))?, usize::MAX);
    let mut new = Some(open_new(new_path, silent)?);
    let profiles: Vec<String> = old.package().profiles().into_iter().map(str::to_string).collect();
    let new_profiles: Vec<String> = new.iter().flat_map(|new| new.package().profiles()).map(str::to_string).collect();
    if new_profiles != profiles {
        eprintln!("The packages have other profiles: {profiles:?} and {new_profiles:?}");
        std::process::exit(2);
    }
    let started = Instant::now();
    let (mut requests_run, mut differences, mut ties) = (0, 0, 0);
    println!(
        "profile, kind, km, old ms, old settled, new ms, search ms, unpack ms, settled by level, values, roads, \
         unpack settled, peak KB, cold ms, cold faults, outcome"
    );
    for profile in profiles.iter().filter(|profile| only.as_ref().is_none_or(|only| only == *profile)) {
        // The decode of a profile of format 9 is not a time of its searches.
        old.package().warm(profile)?;
        let requests = compare::requests(&old, profile, count, seed, max_km * 1000.0);
        if requests.len() < count {
            println!("{profile}: only {} of {count} requests, because few points snap", requests.len());
        }
        let points: Vec<Point> = requests.iter().flat_map(|request| request.points.iter().map(|&p| point(p))).collect();
        let mut snap_us = [0.0; 2];
        for (side, data) in [old.package(), new.as_ref().ok_or(usage)?.package()].into_iter().enumerate() {
            let started = Instant::now();
            for &point in &points {
                let _ = data.snap(point, profile, Policy::default());
            }
            snap_us[side] = started.elapsed().as_secs_f64() * 1e6 / points.len().max(1) as f64;
        }
        println!("{profile}: snap of {} points: old {:.1} µs, new {:.1} µs each", points.len(), snap_us[0], snap_us[1]);
        for (k, request) in requests.iter().enumerate() {
            if cold {
                drop(new.take());
                Maps::evict(new_path)?;
                new = Some(open_new(new_path, silent)?);
            }
            let router = new.as_mut().ok_or(usage)?;
            // The first route of NEW alone, so that its time and faults are those of the cold files.
            let (mut cold_ms, mut faults) = (0.0, 0);
            if cold {
                let (started, before) = (Instant::now(), major_faults());
                let _ = router.routes(request, &Control::default());
                (cold_ms, faults) = (started.elapsed().as_secs_f64() * 1e3, major_faults() - before);
            }
            old.package().take_stats();
            router.package().take_stats();
            let checked = compare::check(&mut old, router, request);
            let stats = [old.package().take_stats(), router.package().take_stats()];
            requests_run += 1;
            let (outcome, times) = match checked {
                Ok((outcome, times)) => (format!("{outcome:?}"), times),
                Err(difference) => {
                    differences += 1;
                    println!("difference: {}", serde_json::to_string(request)?);
                    (format!("Difference: {difference}"), Default::default())
                }
            };
            ties += usize::from(outcome == format!("{:?}", Outcome::Tie));
            let [old_ms, new_ms] = times.map(|time| time.as_secs_f64() * 1e3);
            let new = stats[1];
            let levels: Vec<String> = new.levels.iter().map(usize::to_string).collect();
            let top = new.levels.iter().rposition(|&count| count > 0).unwrap_or(0);
            println!(
                "{profile}, {}, {:.1}, {old_ms:.1}, {}, {new_ms:.1}, {:.2}, {:.2}, {}, {}, {}, {}, {:.0}, {cold_ms:.1}, \
                 {faults}, {outcome}",
                KINDS[k % KINDS.len()],
                straight_km(request),
                stats[0].settled,
                new.search_ns as f64 / 1e6,
                new.unpack_ns as f64 / 1e6,
                levels[..=top].join("/"),
                new.values,
                new.roads,
                new.unpack_settled,
                new.peak_bytes as f64 / 1e3
            );
        }
    }
    println!(
        "{requests_run} requests, {differences} differences, {ties} ties, in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    std::process::exit(i32::from(differences > 0));
}
