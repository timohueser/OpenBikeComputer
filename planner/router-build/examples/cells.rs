//! Measures the package of format 10 on the layers of a bake.
//!
//! `cells write ROADS COSTS OUT [LEVEL]` writes the package of the roads and the costs layer
//! folders into OUT with zstd level LEVEL (default `cells::LEVEL`), with `cell_sizes` and `download_level` of
//! `data/planner.toml`. `cells install OUT TARGET [THREADS]` installs all cells of OUT into TARGET
//! and then checks each object. Each prints its summary, the time and the peak `RssAnon`.

use planner_router::cells::{Directory, Geometry, GEOMETRY};
use planner_router::install;
use planner_router_build::cells::{self, Info, Input};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

fn rss_anon() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let line = status.lines().find_map(|line| line.strip_prefix("RssAnon:")).unwrap_or("0 kB");
    line.trim().trim_end_matches("kB").trim().parse::<u64>().unwrap_or(0) * 1024
}

/// Runs `work`, and prints its time and the peak `RssAnon` of the process while it ran.
fn measured<T>(name: &str, work: impl FnOnce() -> T) -> T {
    let (peak, done) = (AtomicU64::new(0), AtomicBool::new(false));
    let started = Instant::now();
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                peak.fetch_max(rss_anon(), Ordering::Relaxed);
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        });
        let result = work();
        done.store(true, Ordering::Relaxed);
        result
    });
    let seconds = started.elapsed().as_secs_f64();
    eprintln!("{name}: {seconds:.1} s, peak RssAnon {} MiB", peak.into_inner() >> 20);
    result
}

/// A value of `[routing]` in `data/planner.toml`.
fn setting(key: &str) -> String {
    let text = include_str!("../../../data/planner.toml");
    let line = text.lines().find(|line| line.starts_with(&format!("{key} = "))).expect("a routing setting");
    line.split_once(" = ").unwrap().1.to_string()
}

fn km(bounds: [i32; 4]) -> [f64; 2] {
    let lat = f64::from(bounds[1] / 2 + bounds[3] / 2) * 1e-6;
    let metres = 0.111195 / 1000.0;
    [f64::from(bounds[2] - bounds[0]) * metres * lat.to_radians().cos(), f64::from(bounds[3] - bounds[1]) * metres]
}

fn meets(a: [i32; 4], b: [i32; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn spread(mut values: Vec<usize>) -> (usize, usize) {
    values.sort_unstable();
    (values.get(values.len() / 2).copied().unwrap_or(0), values.last().copied().unwrap_or(0))
}

fn write(roads: &Path, costs: &Path, out: &Path, level: i32) -> Result<(), String> {
    let (graph, topology, costs) = planner_router_build::step::layers(roads, costs)?;
    let sizes: Vec<u32> =
        setting("cell_sizes").trim_matches(['[', ']']).split(',').map(|size| size.trim().parse().unwrap()).collect();
    let download_level = setting("download_level").parse().unwrap();
    let info = Info { region: "measure".into(), bounds: [0.0; 4], source_sha256: vec![], attribution: String::new() };
    let input = Input { graph: &graph, topology: &topology, costs: &costs, sizes: &sizes, download_level, info, level };
    let summary = measured(&format!("write, zstd level {level}"), || cells::write(&input, out))?;
    println!("{}", serde_json::to_string_pretty(&summary.to_json()).unwrap());

    let directory = std::fs::read(out.join(cells::DIRECTORY)).map_err(|e| e.to_string())?;
    let directory = Directory::new(&directory).map_err(|e| e.to_string())?;
    let mut level1 = Vec::new();
    let mut downloads = Vec::new();
    for record in 0..directory.downloads() {
        let cell = directory.cell(record).map_err(|e| e.to_string())?;
        let name: String = cell.parts[GEOMETRY].object.iter().map(|b| format!("{b:02x}")).collect();
        let object = std::fs::read(out.join(name)).map_err(|e| e.to_string())?;
        let part = zstd::stream::decode_all(&object[..]).map_err(|e| e.to_string())?;
        let geometry = Geometry::new(&part).map_err(|e| e.to_string())?;
        for i in 0..geometry.cells {
            level1.push(geometry.level1(i).map_err(|e| e.to_string())?.bounds);
        }
        downloads.push(cell.bounds);
    }
    let largest = |boxes: &[[i32; 4]]| {
        let b = *boxes.iter().max_by(|a, b| km(**a)[0].max(km(**a)[1]).total_cmp(&km(**b)[0].max(km(**b)[1]))).unwrap();
        format!("{b:?}, {:.1} km × {:.1} km", km(b)[0], km(b)[1])
    };
    println!("Largest level-1 box: {}", largest(&level1));
    println!("Largest download cell box: {}", largest(&downloads));
    for radius in [250.0, 1000.0] {
        let (mut d, mut l) = (Vec::new(), Vec::new());
        for road in (0..graph.roads.len()).step_by(1000) {
            let shape = graph.roads.shape(road);
            let p = shape[shape.len() / 2];
            let lat = (radius / 0.110f64).ceil() as i32 + 2;
            let lon = (radius / (0.110 * (f64::from(p.lat) * 1e-6).to_radians().cos())).ceil() as i32 + 2;
            let window = [p.lon - lon, p.lat - lat, p.lon + lon, p.lat + lat];
            d.push(downloads.iter().filter(|&&b| meets(b, window)).count());
            l.push(level1.iter().filter(|&&b| meets(b, window)).count());
        }
        let ((dm, dl), (lm, ll)) = (spread(d), spread(l));
        println!("Snap window of {radius} m: download cells median {dm}, largest {dl}; level-1 cells median {lm}, largest {ll}");
    }
    Ok(())
}

fn install(out: &Path, target: &Path, threads: usize) -> Result<(), String> {
    let read = |name| std::fs::read(out.join(name)).map_err(|e| e.to_string());
    let (directory, info) = (read(cells::DIRECTORY)?, read(cells::PACKAGE)?);
    let fetch = |name: &str| std::fs::File::open(out.join(name));
    for (name, run) in [("install", true), ("check", false)] {
        let started = Instant::now();
        let report = measured(&format!("{name}, {threads} workers"), || match run {
            true => install::install(target, (&directory, &info), (None, None), threads, fetch),
            false => install::check(&directory, &info, threads, fetch),
        })
        .map_err(|e| e.to_string())?;
        let rate = report.bytes as f64 / started.elapsed().as_secs_f64() / threads as f64 / 1e6;
        println!("{name}: {report:?}; {rate:.0} MB/s of parts for each worker");
    }
    let files = std::fs::read_dir(target.join(install::CELLS)).map_err(|e| e.to_string())?.count();
    println!("Installed files: {files}");
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage = "usage: cells write ROADS COSTS OUT [LEVEL] | cells install OUT TARGET [THREADS]";
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["write", roads, costs, out, ..] => {
            let level = args.get(4).map_or(Ok(cells::LEVEL), |level| level.parse()).map_err(|_| usage)?;
            write(Path::new(roads), Path::new(costs), Path::new(out), level)
        }
        ["install", out, target, ..] => {
            let all = std::thread::available_parallelism().map_or(1, |n| n.get());
            let threads = args.get(3).map_or(Ok(all), |threads| threads.parse()).map_err(|_| usage)?;
            install(Path::new(out), Path::new(target), threads)
        }
        _ => Err(usage.into()),
    }
}
