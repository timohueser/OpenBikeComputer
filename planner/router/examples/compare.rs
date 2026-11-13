//! Routes one request set on the two formats of one bake and prints each request:
//! `compare OLD NEW COUNT SEED [--max-km K] [--profile P] [--cold] [--no-announce]`. OLD is format
//! 9 and NEW an install of format 10 (`cells install`); COUNT is the requests of each profile, or of
//! `P` only. `--cold` opens NEW
//! again with its files out of the page cache before each request; `--no-announce` makes its host
//! silent. Exits with 1 on a difference, with 2 when the packages have other profiles.
use planner_router::{
    compare::{self, Outcome, KINDS},
    data::RoutingData,
    host::Maps,
    install,
    model::{Point, NO_ELEVATION},
    region::Region,
    snap::Policy,
    Error, Opened, Request, Router,
};
use std::{path::Path, time::Instant};

/// Opens NEW; with `silent`, on a host whose `announce` does nothing.
fn open_new(path: &Path, silent: bool) -> Result<Router<Opened>, Error> {
    let data = if silent {
        let read = |name: &str| std::fs::read(path.join(name)).map_err(|e| Error::InvalidData(e.to_string()));
        let (directory, info, marker) = (read("directory.bin")?, read("package.json")?, read(install::MARKER)?);
        let maps = Maps::new(path, &directory)?.silent();
        Opened::Cells(Region::open(maps, directory, info, &marker)?)
    } else {
        planner_router::open(path)?
    };
    Ok(Router::new(data, usize::MAX))
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
    println!("profile, kind, km, old ms, old settled, old ns/label, new ms, new settled, new ns/label, new peak MB, faults, outcome");
    for profile in profiles.iter().filter(|profile| only.as_ref().is_none_or(|only| only == *profile)) {
        let requests = compare::requests(&old, profile, count, seed, max_km * 1000.0);
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
            let faults = major_faults();
            old.package().take_stats();
            router.package().take_stats();
            let checked = compare::check(&mut old, router, request);
            let faults = major_faults() - faults;
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
            let side = |s: usize| {
                let ms = times[s].as_secs_f64() * 1e3;
                let per = times[s].as_nanos() as f64 / stats[s].settled.max(1) as f64;
                format!("{ms:.1}, {}, {per:.0}", stats[s].settled)
            };
            println!(
                "{profile}, {}, {:.1}, {}, {}, {:.1}, {faults}, {outcome}",
                KINDS[k % KINDS.len()],
                straight_km(request),
                side(0),
                side(1),
                stats[1].peak_bytes as f64 / 1e6
            );
        }
    }
    println!(
        "{requests_run} requests, {differences} differences, {ties} ties, in {:.1} s",
        started.elapsed().as_secs_f64()
    );
    std::process::exit(i32::from(differences > 0));
}
