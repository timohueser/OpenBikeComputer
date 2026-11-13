//! Routes one request set on two routing folders and prints a line for each request:
//! `compare OLD NEW COUNT SEED [options]`, `compare NEW COUNT SEED --new-only [options]`, or
//! `compare --areas INSTALL`. OLD is format 9 and NEW an install of format 10 (`cells install`) of
//! the same bake; with `--layouts`, OLD and NEW are installs of two layouts of one bake. COUNT is
//! the requests of each profile. Exits with 1 on a difference, with 2 when the folders have other
//! profiles.
//!
//! Options: `--min-km K` and `--max-km K`, the band of the later points of a request from its
//! first; `--profiles a,b` (else all); `--new-only`, NEW alone and no check; `--repeat-new N`, NEW
//! routes each request N times and the last run counts; `--layouts`, where NEW pins the candidate
//! at the coordinate of the pin of OLD, and skips a request without one; `--workers N`, with `--new-only`, N threads with
//! a router each over one opened install, which take the requests in turn; `--cold`, NEW opened
//! again with its files out of the page cache before each request; `--no-announce`, a silent host;
//! `--json FILE`, the line of each request as JSON. `--areas INSTALL` prints the table for the
//! download level of `compare::areas`.
//!
//! "ms" is the time of `routes` and `answer::answer`. With `--cold`, NEW routes once alone first:
//! "cold open ms" is the open after the eviction, and "cold ms", "cold faults" and "cold read KB"
//! are those of that route; "new ms" is then a later, warm route, which reuses the snaps of the
//! first. The other columns of NEW are the sums of the searches of its last run: the time of the
//! searches and of the unpacking, the settled labels by the level of their expansion (level 0
//! first), the table values read, the roads of the unpacked legs, the labels that the unpacking
//! settled, and the most bytes that one search held. "old settled" is the labels that the searches
//! of OLD settled. A thread samples `RssAnon` every 10 ms; the summary gives it after the open and
//! at its peak, and the times leave out the first request of OLD of each profile.
use planner_router::{
    compare::{self, Outcome, Routed, KINDS},
    data::RoutingData,
    host::Maps,
    model::{Point, NO_ELEVATION},
    snap::Policy,
    Error, Opened, Request, Router,
};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Mutex,
    },
    time::{Duration, Instant},
};

const USAGE: &str = "Usage: compare OLD NEW COUNT SEED | NEW COUNT SEED --new-only | --areas INSTALL, with \
                     [--min-km K] [--max-km K] [--profiles a,b] [--repeat-new N] [--layouts] [--workers N] [--cold] \
                     [--no-announce] [--json FILE]";

/// The columns of a printed line: JSON pointers into the line of a request.
const COLUMNS: [&str; 22] = [
    "/profile",
    "/kind",
    "/legs_km",
    "/old/outcome",
    "/old/cost",
    "/old/ms",
    "/old/settled",
    "/new/outcome",
    "/new/cost",
    "/new/ms",
    "/new/search_ms",
    "/new/unpack_ms",
    "/new/levels",
    "/new/values",
    "/new/roads",
    "/new/unpack_settled",
    "/new/peak_kb",
    "/cold/open_ms",
    "/cold/ms",
    "/cold/faults",
    "/cold/read_kb",
    "/check",
];

#[derive(Default)]
struct Options {
    min_km: f64,
    max_km: f64,
    profiles: Option<Vec<String>>,
    new_only: bool,
    repeat_new: usize,
    layouts: bool,
    workers: Option<usize>,
    cold: bool,
    silent: bool,
    json: Option<PathBuf>,
    areas: Option<PathBuf>,
}

/// Opens an install; with `silent`, on a host whose `announce` does nothing.
fn open_cells(path: &Path, silent: bool) -> Result<Router<Opened>, Error> {
    Ok(Router::new(Opened::Cells(planner_router::open::cells(path, silent)?), usize::MAX))
}

/// The first number after `name` in `/proc/self/FILE`; 0 off Linux.
fn proc_field(file: &str, name: &str) -> u64 {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string(format!("/proc/self/{file}")).unwrap_or_default();
        let line = text.lines().find_map(|line| line.strip_prefix(name)).unwrap_or_default();
        line.split_whitespace().next().and_then(|value| value.parse().ok()).unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (file, name);
        0
    }
}

/// The anonymous resident memory of this process, in kB.
fn rss_anon_kb() -> u64 {
    proc_field("status", "RssAnon:")
}

/// The bytes that this process made the disk read.
fn read_bytes() -> u64 {
    proc_field("io", "read_bytes:")
}

/// The major page faults of this process (field 12 of `/proc/self/stat`); 0 off Linux.
fn major_faults() -> u64 {
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
        let fields: Vec<&str> = stat.rsplit_once(')').map_or("", |(_, rest)| rest).split_whitespace().collect();
        fields.get(9).and_then(|field| field.parse().ok()).unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    0
}

fn point([lon, lat]: [f64; 2]) -> Point {
    Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION }
}

fn ms(time: Duration) -> f64 {
    time.as_secs_f64() * 1e3
}

/// The straight km of each leg.
fn legs_km(request: &Request) -> Vec<f64> {
    let km = request.points.windows(2).map(|pair| point(pair[0]).distance(point(pair[1])) / 1000.0);
    km.map(|km| (km * 10.0).round() / 10.0).collect()
}

/// The outcome, the cost of the first route, the time and the settled labels of one side.
fn side(routed: &Routed, settled: usize) -> Value {
    let (outcome, cost) = match &routed.result {
        Ok(response) => ("ok".to_string(), response.routes.first().map(|route| route.cost)),
        Err(error) => (compare::error_kind(error), None),
    };
    json!({ "outcome": outcome, "cost": cost, "ms": ms(routed.time), "settled": settled })
}

/// Routes `request` on OLD once, when there is one, and `new_request` on NEW `repeat_new` times;
/// gives the line of the request and, with OLD, the check.
fn measure(
    old: Option<&mut Router<Opened>>,
    new: &mut Router<Opened>,
    [request, new_request]: [&Request; 2],
    options: &Options,
) -> (Value, Option<Result<Outcome, String>>) {
    let old = old.map(|old| {
        old.package().take_stats();
        let routed = compare::route(old, request);
        let settled = old.package().take_stats().settled;
        (old, routed, settled)
    });
    let mut routed = compare::route(new, new_request);
    for _ in 1..options.repeat_new {
        new.package().take_stats();
        routed = compare::route(new, new_request);
    }
    let stats = new.package().take_stats();
    let top = stats.levels.iter().rposition(|&count| count > 0).unwrap_or(0);
    let mut line = json!({
        "profile": request.profile,
        "request": request,
        "legs_km": legs_km(request),
        "new": side(&routed, stats.settled),
    });
    let fields = json!({
        "search_ms": stats.search_ns as f64 / 1e6,
        "unpack_ms": stats.unpack_ns as f64 / 1e6,
        "levels": stats.levels[..=top],
        "values": stats.values,
        "roads": stats.roads,
        "unpack_settled": stats.unpack_settled,
        "peak_kb": stats.peak_bytes / 1000,
    });
    line["new"].as_object_mut().expect("an object").extend(fields.as_object().expect("an object").clone());
    let Some((old, old_routed, settled)) = old else { return (line, None) };
    line["old"] = side(&old_routed, settled);
    let checked = compare::verdict(old, new, &old_routed, &routed, options.layouts);
    line["check"] = json!(match &checked {
        Ok(outcome) => format!("{outcome:?}"),
        Err(difference) => format!("Difference: {difference}"),
    });
    (line, Some(checked))
}

/// A value of a printed line: `-` when absent, a list joined by `/`.
fn text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => "-".into(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) if number.is_f64() => format!("{:.2}", number.as_f64().unwrap_or_default()),
        Some(Value::Array(items)) => items.iter().map(|item| text(Some(item))).collect::<Vec<_>>().join("/"),
        Some(other) => other.to_string(),
    }
}

/// The counts and the samples of a run.
#[derive(Default)]
struct Totals {
    requests: usize,
    differences: usize,
    ties: usize,
    skipped: usize,
    pinned: usize,
    old_ms: Vec<f64>,
    new_ms: Vec<f64>,
}

impl Totals {
    /// Prints `line`, writes it to `json`, and counts it.
    fn add(
        &mut self,
        line: &Value,
        checked: Option<Result<Outcome, String>>,
        first: bool,
        json: &mut Option<std::fs::File>,
    ) {
        println!("{}", COLUMNS.iter().map(|column| text(line.pointer(column))).collect::<Vec<_>>().join(", "));
        if let Some(file) = json {
            let _ = writeln!(file, "{line}");
        }
        self.requests += 1;
        match checked {
            Some(Err(_)) => {
                self.differences += 1;
                println!("difference: {}", line["request"]);
            }
            Some(Ok(Outcome::Tie)) => self.ties += 1,
            Some(Ok(Outcome::Skipped)) => self.skipped += 1,
            _ => {}
        }
        if let Some(old) = line.pointer("/old/ms").and_then(Value::as_f64).filter(|_| !first) {
            self.old_ms.push(old);
        }
        self.new_ms.extend(line.pointer("/new/ms").and_then(Value::as_f64));
    }
}

/// The median, the P90 and the largest of `times`.
fn spread(mut times: Vec<f64>) -> String {
    times.sort_by(f64::total_cmp);
    let at = |q| compare::quantile(&times, q);
    format!("median {:.1} ms, P90 {:.1} ms, largest {:.1} ms", at(0.5), at(0.9), at(1.0))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut options = Options { max_km: 75.0, repeat_new: 1, ..Options::default() };
    let mut positional = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(USAGE);
        match arg.as_str() {
            "--min-km" => options.min_km = value()?.parse()?,
            "--max-km" => options.max_km = value()?.parse()?,
            "--profiles" => options.profiles = Some(value()?.split(',').map(str::to_string).collect()),
            "--new-only" => options.new_only = true,
            "--repeat-new" => options.repeat_new = value()?.parse()?,
            "--layouts" => options.layouts = true,
            "--workers" => options.workers = Some(value()?.parse()?),
            "--cold" => options.cold = true,
            "--no-announce" => options.silent = true,
            "--json" => options.json = Some(value()?.into()),
            "--areas" => options.areas = Some(value()?.into()),
            option if option.starts_with("--") => return Err(USAGE.into()),
            _ => positional.push(arg),
        }
    }
    if let Some(folder) = &options.areas {
        return print_areas(folder);
    }
    let (old_path, new_path, count, seed) = match (options.new_only, &positional[..]) {
        (false, [old, new, count, seed]) => (Some(old), new, count, seed),
        (true, [new, count, seed]) => (None, new, count, seed),
        _ => return Err(USAGE.into()),
    };
    let (count, seed): (usize, u64) = (count.parse()?, seed.parse()?);
    if seed == 0 || options.repeat_new == 0 || options.workers == Some(0) {
        return Err("SEED, N of --repeat-new and N of --workers are numbers above 0".into());
    }
    if options.workers.is_some() && (!options.new_only || options.cold) || options.layouts && options.new_only {
        return Err("--workers needs --new-only and no --cold; --layouts needs OLD".into());
    }
    let new_path = Path::new(new_path);
    let mut old = match old_path {
        Some(path) if options.layouts => Some(open_cells(Path::new(path), options.silent)?),
        Some(path) => Some(Router::new(planner_router::open(Path::new(path))?, usize::MAX)),
        None => None,
    };
    let mut new = Some(open_cells(new_path, options.silent)?);
    let profiles: Vec<String> =
        new.as_ref().ok_or(USAGE)?.package().profiles().into_iter().map(str::to_string).collect();
    if let Some(old) = &old {
        let old_profiles: Vec<String> = old.package().profiles().into_iter().map(str::to_string).collect();
        if old_profiles != profiles {
            eprintln!("The folders have other profiles: {old_profiles:?} and {profiles:?}");
            std::process::exit(2);
        }
    }
    let profiles = match &options.profiles {
        None => profiles,
        Some(list) => match list.iter().find(|name| !profiles.contains(name)) {
            Some(missing) => return Err(format!("The folders have no profile {missing}").into()),
            None => list.clone(),
        },
    };
    let mut json = options.json.as_ref().map(std::fs::File::create).transpose()?;
    let idle_kb = rss_anon_kb();
    let (peak_kb, stop) = (AtomicU64::new(idle_kb), AtomicBool::new(false));
    let started = Instant::now();
    let mut totals = Totals::default();
    std::thread::scope(|scope| -> Result<(), Box<dyn std::error::Error>> {
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                peak_kb.fetch_max(rss_anon_kb(), Ordering::Relaxed);
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        let run = (|| -> Result<(), Box<dyn std::error::Error>> {
            println!("{}", COLUMNS.map(|column| column[1..].replace(['/', '_'], " ")).join(", "));
            for profile in &profiles {
                if let Some(old) = &old {
                    // The decode of a profile of format 9 is not a time of its searches.
                    old.package().warm(profile)?;
                }
                let (min_m, max_m) = (options.min_km * 1000.0, options.max_km * 1000.0);
                let requests = match &old {
                    Some(old) => compare::requests(old, profile, count, seed, min_m, max_m),
                    None => compare::requests(new.as_ref().ok_or(USAGE)?, profile, count, seed, min_m, max_m),
                };
                if requests.len() < count {
                    println!("{profile}: only {} of {count} requests, because few points snap", requests.len());
                }
                let points: Vec<Point> = requests.iter().flat_map(|r| r.points.iter().map(|&p| point(p))).collect();
                let mut snap_us = Vec::new();
                for data in old.iter().chain(new.as_ref()).map(Router::package) {
                    let started = Instant::now();
                    for &point in &points {
                        let _ = data.snap(point, profile, Policy::default());
                    }
                    snap_us
                        .push(format!("{:.1} µs", started.elapsed().as_secs_f64() * 1e6 / points.len().max(1) as f64));
                }
                println!("{profile}: snap of {} points: {} each (OLD first)", points.len(), snap_us.join(", "));
                let kind = |k: usize| KINDS[compare::kind(k, min_m)];
                if let Some(workers) = options.workers {
                    let (next, lines) = (AtomicUsize::new(0), Mutex::new(Vec::new()));
                    std::thread::scope(|scope| {
                        for _ in 0..workers {
                            let fork = new.as_ref().map(|new| new.package().fork());
                            let Some(mut router) = fork.map(|fork| Router::new(fork, usize::MAX)) else { break };
                            let (next, lines, requests, options) = (&next, &lines, &requests, &options);
                            scope.spawn(move || loop {
                                let k = next.fetch_add(1, Ordering::Relaxed);
                                let Some(request) = requests.get(k) else { break };
                                let (line, _) = measure(None, &mut router, [request; 2], options);
                                lines.lock().expect("a line list").push((k, line));
                            });
                        }
                    });
                    let mut lines = lines.into_inner().expect("a line list");
                    lines.sort_by_key(|(k, _)| *k);
                    for (k, mut line) in lines {
                        line["kind"] = json!(kind(k));
                        totals.add(&line, None, false, &mut json);
                    }
                    continue;
                }
                let mut first = true;
                for (k, request) in requests.iter().enumerate() {
                    // A pin names a road of OLD; on another layout, the candidate at its coordinate.
                    let moved = match (&old, &new) {
                        (Some(old), Some(new)) if options.layouts => {
                            compare::repin(old.package(), new.package(), request)
                        }
                        _ => Some(request.clone()),
                    };
                    let Some(moved) = moved else {
                        totals.pinned += 1;
                        continue;
                    };
                    let mut cold = Value::Null;
                    if options.cold {
                        drop(new.take());
                        Maps::evict(new_path)?;
                        let opened = Instant::now();
                        let router = new.insert(open_cells(new_path, options.silent)?);
                        let open_ms = ms(opened.elapsed());
                        let (routed, faults, read) = (Instant::now(), major_faults(), read_bytes());
                        let _ = router.routes(&moved, &planner_router::Control::default());
                        cold = json!({
                            "open_ms": open_ms,
                            "ms": ms(routed.elapsed()),
                            "faults": major_faults() - faults,
                            "read_kb": (read_bytes() - read) / 1000,
                        });
                    }
                    let (mut line, checked) =
                        measure(old.as_mut(), new.as_mut().ok_or(USAGE)?, [request, &moved], &options);
                    line["kind"] = json!(kind(k));
                    line["cold"] = cold;
                    totals.add(&line, checked, first, &mut json);
                    first = false;
                }
            }
            Ok(())
        })();
        stop.store(true, Ordering::Relaxed);
        run
    })?;
    let workers = options.workers.unwrap_or(1) as u64;
    let peak_kb = peak_kb.into_inner();
    if old.is_some() {
        println!("old: {}", spread(totals.old_ms));
    }
    println!("new: {}", spread(totals.new_ms));
    println!(
        "RssAnon: {:.1} MB after the open, peak {:.1} MB, (peak - open) / {workers} workers = {:.1} MB",
        idle_kb as f64 / 1e3,
        peak_kb as f64 / 1e3,
        peak_kb.saturating_sub(idle_kb) as f64 / 1e3 / workers as f64
    );
    println!(
        "{} requests, {} differences, {} ties, {} skipped for 8 candidates, {} pins without a candidate, in {:.1} s",
        totals.requests,
        totals.differences,
        totals.ties,
        totals.skipped,
        totals.pinned,
        started.elapsed().as_secs_f64()
    );
    std::process::exit(i32::from(totals.differences > 0));
}

fn print_areas(folder: &Path) -> Result<(), Box<dyn std::error::Error>> {
    println!(
        "level, cells, roads median, roads max, objects, object KB median, object KB max, world files, world \
         directory MB, tiles, tile median, tile P90, blocks, block median, block P90"
    );
    for row in compare::areas(folder)? {
        println!(
            "{}, {}, {}, {}, {}, {:.0}, {:.0}, {:.0}, {:.1}, {}, {:.2}, {:.2}, {}, {:.2}, {:.2}",
            row.level,
            row.cells,
            row.roads[0],
            row.roads[1],
            row.objects,
            row.object_bytes[0] as f64 / 1e3,
            row.object_bytes[1] as f64 / 1e3,
            row.world_files,
            row.world_directory / 1e6,
            row.tiles.0,
            row.tiles.1,
            row.tiles.2,
            row.blocks.0,
            row.blocks.1,
            row.blocks.2
        );
    }
    Ok(())
}
