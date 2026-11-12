//! `census`: builds every zoom-16 cell of a box natively as the TrailView viewer does (the cell, then
//! its mesh with the rims of its eight neighbours), and records per cell whether the build is ok,
//! panics, hangs or grows too large, which caps it hits, the crate's own checks, and whether
//! neighbouring cells agree on their shared line. One bad cell never stops the run: the driver gives
//! jobs to worker processes, each under its own memory cap, and starts a new worker after one ends.

#[path = "../../../../obc-ground-plan/tests/fixture/mod.rs"]
mod fixture;

mod driver;
mod source;
mod worker;

use std::path::PathBuf;
use std::time::Duration;

use obc_ground_plan::frame::{CellKey, CELL_UNITS, FRAME_BITS};

use source::{parse_cell, Place};
use worker::Fail;

/// The heap in bytes, and its peak: the peak of a stage is the peak above the heap at its start.
mod heap {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

    static NOW: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(layout) };
            if !p.is_null() {
                PEAK.fetch_max(NOW.fetch_add(layout.size(), Relaxed) + layout.size(), Relaxed);
            }
            p
        }

        unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
            unsafe { System.dealloc(p, layout) };
            NOW.fetch_sub(layout.size(), Relaxed);
        }

        unsafe fn realloc(&self, p: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, layout, size) };
            if !q.is_null() {
                if size > layout.size() {
                    let grown = size - layout.size();
                    PEAK.fetch_max(NOW.fetch_add(grown, Relaxed) + grown, Relaxed);
                } else {
                    NOW.fetch_sub(layout.size() - size, Relaxed);
                }
            }
            q
        }
    }

    /// The heap now, with the peak reset to it.
    pub fn start() -> usize {
        let now = NOW.load(Relaxed);
        PEAK.store(now, Relaxed);
        now
    }

    /// The peak since `start` above its heap.
    pub fn peak(base: usize) -> usize {
        PEAK.load(Relaxed).saturating_sub(base)
    }
}

#[global_allocator]
static HEAP: heap::Counting = heap::Counting;

/// The job of a worker, one JSON line on its stdin: its targets in order and the cells that it must not
/// build (they failed before).
#[derive(Debug, Default)]
pub struct Job {
    pub targets: Vec<CellKey>,
    pub skip: Vec<CellKey>,
}

impl Job {
    pub fn to_line(&self) -> String {
        let cells = |c: &[CellKey]| c.iter().map(|k| [k.x, k.y]).collect::<Vec<_>>();
        serde_json::json!({"targets": cells(&self.targets), "skip": cells(&self.skip)}).to_string()
    }

    pub fn parse(line: &str) -> Result<Job, String> {
        let value: serde_json::Value = serde_json::from_str(line).map_err(|e| format!("the job: {e}"))?;
        let cells = |name: &str| -> Result<Vec<CellKey>, String> {
            let list: Vec<[u32; 2]> =
                serde_json::from_value(value[name].clone()).map_err(|e| format!("the job: {e}"))?;
            Ok(list.into_iter().map(|[x, y]| CellKey { x, y }).collect())
        };
        Ok(Job { targets: cells("targets")?, skip: cells("skip")? })
    }
}

/// The latitude and longitude of a cell's centre.
pub fn centre(key: CellKey) -> (f64, f64) {
    let square = (1u64 << FRAME_BITS) as f64;
    let o = key.origin();
    let (x, y) = ((o.x + CELL_UNITS / 2) as f64 / square, (o.y + CELL_UNITS / 2) as f64 / square);
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y)).sinh().atan().to_degrees();
    let round = |v: f64| (v * 1e6).round() / 1e6;
    (round(lat), round(x * 360.0 - 180.0))
}

const USAGE: &str = "census (--fixture DIR | --release DIR [--id ID]) --out DIR [--workers 3] [--memory 2G|none]
  [--timeout-s 60] [--grid-m 4] [--strip 10] [--rows 48] [--limit N] [--cells X/Y,...] [--bounds W,S,E,N]
  [--min-available-gb 2] [--levels] [--fail X/Y=panic|hang | --fail start]...";

fn main() {
    let worker = std::env::args().nth(1).as_deref() == Some("worker");
    if let Err(e) = run(worker) {
        if worker {
            println!("{}", serde_json::json!({"fatal": e}));
        }
        eprintln!("census: {e}");
        std::process::exit(2);
    }
}

fn run(worker: bool) -> Result<(), String> {
    let mut args = std::env::args().skip(1 + usize::from(worker));
    let (mut fixture, mut release, mut id, mut out) = (None::<PathBuf>, None::<PathBuf>, None, None::<PathBuf>);
    let mut options = driver::Options {
        workers: 3,
        memory: Some("2G".into()),
        timeout: Duration::from_secs(60),
        strip: 10,
        rows: 48,
        limit: None,
        cells: None,
        bounds: None,
        min_available_gb: 2.0,
    };
    let (mut grid_m, mut fail, mut levels) = (4.0, Vec::new(), false);
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or_else(|| format!("{arg} needs a value\n{USAGE}"));
        let number = |v: String| v.parse::<f64>().map_err(|_| format!("{arg}: a number"));
        match arg.as_str() {
            "--fixture" => fixture = Some(value()?.into()),
            "--release" => release = Some(value()?.into()),
            "--id" => id = Some(value()?),
            "--out" => out = Some(value()?.into()),
            "--workers" => options.workers = number(value()?)? as usize,
            "--memory" => options.memory = Some(value()?).filter(|m| m != "none"),
            "--timeout-s" => options.timeout = Duration::from_secs_f64(number(value()?)?),
            "--grid-m" => grid_m = number(value()?)?,
            "--levels" => levels = true,
            "--strip" => options.strip = number(value()?)? as u32,
            "--rows" => options.rows = number(value()?)? as u32,
            "--limit" => options.limit = Some(number(value()?)? as usize),
            "--min-available-gb" => options.min_available_gb = number(value()?)?,
            "--cells" => options.cells = Some(value()?.split(',').map(parse_cell).collect::<Result<_, _>>()?),
            "--bounds" => {
                let v: Vec<f64> = value()?
                    .split(',')
                    .map(|v| v.parse().map_err(|_| "--bounds: W,S,E,N"))
                    .collect::<Result<_, _>>()?;
                options.bounds = Some(v.try_into().map_err(|_| "--bounds: W,S,E,N")?);
            }
            "--fail" => {
                let v = value()?;
                if v == "start" {
                    fail.push((CellKey { x: 0, y: 0 }, Fail::Start));
                    continue;
                }
                let (cell, how) = v.split_once('=').ok_or("--fail X/Y=panic|hang")?;
                let how = match how {
                    "panic" => Fail::Panic,
                    "hang" => Fail::Hang,
                    _ => return Err("--fail X/Y=panic|hang".into()),
                };
                fail.push((parse_cell(cell)?, how));
            }
            _ => return Err(format!("unknown option {arg}\n{USAGE}")),
        }
    }
    let place = match (fixture, release) {
        (Some(dir), None) => Place::Fixture(dir),
        (None, Some(dir)) => Place::Release(dir, id),
        _ => return Err(format!("give --fixture or --release\n{USAGE}")),
    };
    if worker {
        return worker::run(worker::Options { place, grid_m, fail, levels });
    }
    let out = out.ok_or_else(|| format!("give --out\n{USAGE}"))?;
    let mut worker_args = place.args();
    worker_args.extend(["--grid-m".into(), grid_m.to_string()]);
    if levels {
        worker_args.push("--levels".into());
    }
    for (cell, how) in &fail {
        worker_args.push("--fail".into());
        worker_args.push(match how {
            Fail::Panic => format!("{}/{}=panic", cell.x, cell.y),
            Fail::Hang => format!("{}/{}=hang", cell.x, cell.y),
            Fail::Start => "start".into(),
        });
    }
    driver::run(&place, &out, &options, &worker_args)
}
