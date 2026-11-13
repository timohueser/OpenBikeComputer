//! Scratch measurement of #3302 (not committed): the data of an install of format 10 (`Region`)
//! against format 9 (`Selection`) of the same bake, the first reads cold and warm, and RssAnon.
use planner_router::{
    data::RoutingData,
    host::Maps,
    model::{Point, Road, BIKE, FOOT, PUSH},
    package::Endpoint,
    region::Region,
    snap::{Candidates, Policy},
    Error,
};
use std::{fs, path::Path, time::Instant};

fn rss_anon() -> u64 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("RssAnon:")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse::<u64>().unwrap() * 1024
}

fn xorshift(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn open(install: &Path) -> Region<Maps> {
    let directory = fs::read(install.join("directory.bin")).unwrap();
    let info = fs::read(install.join("package.json")).unwrap();
    let marker = fs::read(install.join("install.bin")).unwrap();
    Region::open(Maps::new(install, &directory).unwrap(), directory, info, &marker).unwrap()
}

fn departures(endpoint: &Endpoint) -> Vec<(u32, u64)> {
    let mut list: Vec<_> = endpoint.departures.iter().map(|d| (d.state, d.penalty)).collect();
    list.sort_unstable();
    list
}

fn candidates(found: &Result<Candidates, Error>) -> String {
    match found {
        Err(e) => format!("error {e}"),
        Ok(found) => {
            let list: Vec<_> = found
                .retained
                .iter()
                .map(|c| {
                    (
                        c.position.road,
                        c.position.fraction.to_bits(),
                        c.projected.lat,
                        c.projected.lon,
                        c.snap_distance_m.to_bits(),
                    )
                })
                .collect();
            format!("{} {list:?}", found.truncated)
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (old_dir, install, mode) = (Path::new(&args[1]), Path::new(&args[2]), args[3].as_str());
    let step: u32 = args.get(4).map_or(7, |s| s.parse().unwrap());
    let base = rss_anon();
    match mode {
        "compare" => {
            let old = planner_router::open(old_dir).unwrap();
            let roads = old.package().manifest().roads;
            let before = rss_anon();
            let new = open(install);
            let opened = rss_anon();
            println!("RssAnon: base {base}, format 9 open {before}, + Region open {opened}");
            assert_eq!((old.region(), old.bounds()), (new.region(), new.bounds()));
            assert_eq!(old.profiles(), new.profiles());
            let fields = |road: &Road| {
                let shape: Vec<_> = road.shape.iter().map(|p| (p.lat, p.lon, p.elevation.to_bits())).collect();
                (shape, road.surface, road.access, road.difficulty, road.hiking_difficulty, road.way, road.length_m)
            };
            let start = Instant::now();
            let mut closures = 0u64;
            for road in 0..roads {
                assert_eq!(old.with_road(road, fields).unwrap(), new.with_road(road, fields).unwrap(), "road {road}");
                for mode in [BIKE, FOOT, PUSH] {
                    let found = old.closing(road, mode).unwrap();
                    closures += u64::from(found.is_some());
                    assert_eq!(found, new.closing(road, mode).unwrap(), "closures of {road}");
                }
            }
            println!(
                "roads: {roads} equal (shape, fields, closures {closures}), {:.1} s",
                start.elapsed().as_secs_f64()
            );
            let (mut times, mut turns, mut calls) = ([0f64; 2], 0u64, 0u64);
            for profile in old.profiles() {
                for road in (0..roads).step_by(step as usize) {
                    let t = Instant::now();
                    let a = old.endpoint(profile, road).unwrap();
                    times[0] += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let b = new.endpoint(profile, road).unwrap();
                    times[1] += t.elapsed().as_secs_f64();
                    assert_eq!(a.cost, b.cost, "cost {road} {profile}");
                    assert_eq!(departures(&a), departures(&b), "turns {road} {profile}");
                    turns += a.departures.len() as u64;
                    calls += 1;
                }
            }
            println!(
                "endpoints: {calls} equal (each {step}th road, 10 profiles), {turns} turns; format 9 {:.1} s, format 10 {:.1} s",
                times[0], times[1]
            );
            let mut state = 0x1234_5678u64;
            let profiles = old.profiles();
            let (mut snaps, mut truncated, mut errors, mut times) = (0u64, 0u64, 0u64, [0f64; 2]);
            for k in 0..20_000u64 {
                let road = (xorshift(&mut state) % u64::from(roads)) as u32;
                let point = old.with_road(road, |r| r.shape[r.shape.len() / 2]).unwrap();
                let jitter = |s: &mut u64| (xorshift(s) % 4_001) as i32 - 2_000;
                let point = if k % 4 == 0 {
                    Point { lat: point.lat + 5 * jitter(&mut state), lon: point.lon + 5 * jitter(&mut state), ..point }
                } else {
                    point
                };
                let profile = profiles[k as usize % profiles.len()];
                for radius_m in [250.0, 1_000.0] {
                    let policy = Policy { radius_m, ..Policy::default() };
                    let t = Instant::now();
                    let a = old.snap(point, profile, policy);
                    times[0] += t.elapsed().as_secs_f64();
                    let t = Instant::now();
                    let b = new.snap(point, profile, policy);
                    times[1] += t.elapsed().as_secs_f64();
                    let (a, b) = (candidates(&a), candidates(&b));
                    assert_eq!(a, b, "snap {point:?} {radius_m} {profile}");
                    snaps += 1;
                    truncated += u64::from(a.starts_with("true"));
                    errors += u64::from(a.starts_with("error"));
                }
            }
            println!(
                "snaps: {snaps} equal ({truncated} truncated, {errors} errors); format 9 {:.1} s, format 10 {:.1} s",
                times[0], times[1]
            );
            let fork = new.fork();
            fork.with_road(0, |_| ()).unwrap();
            println!("RssAnon at the end {}; a fork with one read {}", rss_anon(), rss_anon());
        }
        "first" => {
            // The first reads of a request point: snap with 250 m, then the endpoint of its candidates.
            let old = planner_router::open(old_dir).unwrap();
            let roads = old.package().manifest().roads;
            let mut state = 0xABCDEFu64;
            let points: Vec<(Point, &str)> = (0..20)
                .map(|k| {
                    let road = (xorshift(&mut state) % u64::from(roads)) as u32;
                    (old.with_road(road, |r| r.shape[0]).unwrap(), ["touring", "mtb", "hiking", "road"][k % 4])
                })
                .collect();
            drop(old);
            for cold in [true, false] {
                let mut times = Vec::new();
                for &(point, profile) in &points {
                    if cold {
                        Maps::evict(install).unwrap();
                    }
                    let t = Instant::now();
                    let region = open(install);
                    let opened = t.elapsed().as_secs_f64();
                    let found = region.snap(point, profile, Policy::default()).unwrap();
                    for c in &found.retained {
                        region.endpoint(profile, c.position.road).unwrap();
                    }
                    times.push((opened * 1e3, t.elapsed().as_secs_f64() * 1e3));
                }
                times.sort_by(|a, b| a.1.total_cmp(&b.1));
                println!(
                    "{}: open and first point, ms: median {:.2} (open {:.2}), max {:.2}",
                    if cold { "cold" } else { "warm" },
                    times[10].1,
                    times[10].0,
                    times[19].1
                );
            }
            println!("RssAnon {} (base {base})", rss_anon());
        }
        _ => panic!("mode"),
    }
}
