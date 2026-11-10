//! The driver: it cuts the targets into jobs (strips of columns, cut into runs of rows), runs each job
//! in a worker process under its own memory cap, and appends each line to `census.jsonl`. A worker that
//! panics, passes the time limit of a stage or is killed ends: the driver records the cell that it was
//! on with the reason, and gives the rest of the job to a new worker that skips that cell. A run
//! resumes from `census.jsonl`, and it writes `summary.json` at its end.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use obc_ground_plan::frame::CellKey;
use obc_ground_plan::probe::FIELDS;
use serde_json::{json, Map, Value};

use crate::source::{Place, Source};
use crate::{centre, Job};

pub struct Options {
    pub workers: usize,
    /// `MemoryMax` of each worker's scope, or None: no cap (tests).
    pub memory: Option<String>,
    /// The time that one stage of one cell may take.
    pub timeout: Duration,
    pub strip: u32,
    pub rows: u32,
    pub limit: Option<usize>,
    pub cells: Option<Vec<CellKey>>,
    pub bounds: Option<[f64; 4]>,
    /// The run stops when the PC has less memory available.
    pub min_available_gb: f64,
}

/// The states of a cell that the driver records: the worker ended on it.
const FAILED: [&str; 4] = ["panic", "timeout", "memory", "crash"];
/// A worker that ends this often without a line stops the run: the job cannot start.
const STARTS: u32 = 3;
/// Cells that each list of the summary names at most.
const TOP: usize = 20;

struct Running {
    child: Child,
    job: Job,
    /// The cell and stage that it began last, and when, until its line.
    current: Option<(CellKey, String, Instant)>,
    panic: Option<Value>,
    fatal: Option<String>,
    timed_out: bool,
    lines: usize,
}

fn cell_of(v: &Value) -> Option<CellKey> {
    let c = v.as_array()?;
    Some(CellKey { x: c.first()?.as_u64()? as u32, y: c.get(1)?.as_u64()? as u32 })
}

/// MemAvailable of `/proc/meminfo` in GB.
fn available_gb() -> f64 {
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let kb =
        text.lines().find(|l| l.starts_with("MemAvailable:")).and_then(|l| l.split_whitespace().nth(1)?.parse().ok());
    kb.map_or(f64::INFINITY, |kb: f64| kb / (1 << 20) as f64)
}

fn load() -> String {
    std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim().to_string()
}

/// The lines of `census.jsonl`, the last one of each cell.
fn read_lines(path: &Path) -> BTreeMap<CellKey, Value> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok());
    lines.filter_map(|v| Some((cell_of(&v["cell"])?, v))).collect()
}

/// The jobs of the targets: per strip of `strip` columns, runs of `rows` rows, each by row, then column.
fn jobs(mut targets: Vec<CellKey>, options: &Options) -> VecDeque<Vec<CellKey>> {
    let (strip, rows) = (options.strip.max(1), options.rows.max(1));
    targets.sort_by_key(|k| (k.x / strip, k.y / rows, k.y, k.x));
    if let Some(limit) = options.limit {
        targets.truncate(limit);
    }
    let mut out: VecDeque<Vec<CellKey>> = VecDeque::new();
    for k in targets {
        match out.back_mut() {
            Some(job) if (job[0].x / strip, job[0].y / rows) == (k.x / strip, k.y / rows) => job.push(k),
            _ => out.push_back(vec![k]),
        }
    }
    out
}

fn spawn(
    job: Job,
    worker_args: &[String],
    options: &Options,
    log: &File,
    id: usize,
    tx: &mpsc::Sender<(usize, Option<String>)>,
) -> Result<Running, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut command = match &options.memory {
        Some(max) => {
            let mut c = Command::new("systemd-run");
            c.args(["--user", "--scope", "--quiet", "-p", &format!("MemoryMax={max}"), "-p", "MemorySwapMax=0"]);
            c.arg(&exe);
            c
        }
        None => Command::new(&exe),
    };
    command.arg("worker").args(worker_args);
    command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(log.try_clone().map_err(|e| e.to_string())?);
    let mut child = command.spawn().map_err(|e| format!("cannot start a worker: {e}"))?;
    let mut stdin = child.stdin.take().expect("a piped stdin");
    writeln!(stdin, "{}", job.to_line()).map_err(|e| format!("the job of a worker: {e}"))?;
    drop(stdin);
    let stdout = child.stdout.take().expect("a piped stdout");
    let tx = tx.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send((id, Some(line))).is_err() {
                return;
            }
        }
        let _ = tx.send((id, None));
    });
    Ok(Running { child, job, current: None, panic: None, fatal: None, timed_out: false, lines: 0 })
}

pub fn run(place: &Place, out: &Path, options: &Options, worker_args: &[String]) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let census = out.join("census.jsonl");
    let started = (Instant::now(), load(), available_gb());
    let source = Source::open(place)?;
    let mut targets = match &options.cells {
        Some(cells) => cells.clone(),
        None => source.cells()?,
    };
    drop(source);
    if let Some(bounds) = options.bounds {
        let inside: BTreeSet<CellKey> = crate::source::cells_meeting(bounds).into_iter().collect();
        targets.retain(|k| inside.contains(k));
    }
    let all: BTreeSet<CellKey> = targets.iter().copied().collect();
    let before = read_lines(&census);
    let mut done: BTreeSet<CellKey> = before.keys().copied().collect();
    let mut skip: BTreeSet<CellKey> =
        (before.iter()).filter(|(_, v)| FAILED.contains(&v["state"].as_str().unwrap_or(""))).map(|(k, _)| *k).collect();
    targets.retain(|k| !done.contains(k));
    let mut queue = jobs(targets, options);
    let todo: usize = queue.iter().map(Vec::len).sum();
    eprintln!("census: {} cells, {} done before, {todo} to build in {} jobs", all.len(), before.len(), queue.len());
    let mut file = OpenOptions::new().create(true).append(true).open(&census).map_err(|e| e.to_string())?;
    let log = OpenOptions::new().create(true).append(true).open(out.join("workers.log")).map_err(|e| e.to_string())?;
    let (tx, rx) = mpsc::channel();
    let mut running: BTreeMap<usize, Running> = BTreeMap::new();
    let (mut next_id, mut failed_starts, mut stopped, mut built) = (0, 0, None::<String>, 0usize);
    let record = |file: &mut File, line: &Value| -> Result<(), String> {
        writeln!(file, "{line}").and_then(|()| file.flush()).map_err(|e| e.to_string())
    };
    loop {
        while stopped.is_none() && running.len() < options.workers.max(1) {
            let Some(job) = queue.pop_front() else { break };
            let targets: Vec<CellKey> = job.into_iter().filter(|k| !done.contains(k) && !skip.contains(k)).collect();
            if targets.is_empty() {
                continue;
            }
            let job = Job { targets, skip: skip.iter().copied().collect() };
            running.insert(next_id, spawn(job, worker_args, options, &log, next_id, &tx)?);
            next_id += 1;
        }
        if running.is_empty() {
            break;
        }
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok((id, Some(text))) => {
                let Some(w) = running.get_mut(&id) else { continue };
                let Ok(v) = serde_json::from_str::<Value>(&text) else { continue };
                w.lines += 1;
                if let Some(cell) = cell_of(&v["begin"]) {
                    w.current = Some((cell, v["stage"].as_str().unwrap_or("").to_string(), Instant::now()));
                } else if let Some(cell) = cell_of(&v["cell"]) {
                    record(&mut file, &v)?;
                    done.insert(cell);
                    w.current = None;
                    built += 1;
                    if built % 50 == 0 {
                        eprintln!("census: {built} of {todo} built, {:.1} GB available", available_gb());
                    }
                } else if v.get("panic").is_some() {
                    w.panic = Some(v["panic"].clone());
                } else if let Some(fatal) = v["fatal"].as_str() {
                    w.fatal = Some(fatal.to_string());
                }
            }
            Ok((id, None)) => {
                let Some(mut w) = running.remove(&id) else { continue };
                let status = w.child.wait().map_err(|e| e.to_string())?;
                let left: Vec<CellKey> = w.job.targets.iter().copied().filter(|k| !done.contains(k)).collect();
                if left.is_empty() || stopped.is_some() {
                    continue;
                }
                let Some((cell, stage, since)) = w.current.take() else {
                    // It ended before its first cell, or between cells.
                    failed_starts += u32::from(w.lines == 0);
                    if failed_starts >= STARTS {
                        stopped = Some(format!("a worker ended with {status}: {}", w.fatal.unwrap_or_default()));
                        for other in running.values_mut() {
                            let _ = other.child.kill();
                        }
                        continue;
                    }
                    queue.push_front(left);
                    continue;
                };
                let (lat, lon) = centre(cell);
                let mut line = json!({"cell": [cell.x, cell.y], "lat": lat, "lon": lon, "stage": stage,
                    "stage_ms": since.elapsed().as_secs_f64() * 1e3, "context": !all.contains(&cell)});
                if let Some(fatal) = &w.fatal {
                    line["error"] = json!(fatal);
                }
                line["state"] = json!(if w.timed_out {
                    "timeout"
                } else if w.panic.is_some() {
                    "panic"
                } else if status.signal() == Some(9) {
                    "memory"
                } else {
                    "crash"
                });
                if let Some(panic) = w.panic {
                    line["panic"] = panic;
                }
                line["exit"] = json!(status.to_string());
                if !done.contains(&cell) {
                    record(&mut file, &line)?;
                }
                eprintln!("census: cell {}/{} ({lat}, {lon}): {} in {stage}", cell.x, cell.y, line["state"]);
                done.insert(cell);
                skip.insert(cell);
                queue.push_front(left.into_iter().filter(|k| *k != cell).collect());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        for w in running.values_mut() {
            if !w.timed_out && w.current.as_ref().is_some_and(|c| c.2.elapsed() > options.timeout) {
                w.timed_out = true;
                let _ = w.child.kill();
            }
        }
        let free = available_gb();
        if stopped.is_none() && free < options.min_available_gb {
            stopped = Some(format!("the PC has {free:.1} GB available, under {} GB", options.min_available_gb));
            eprintln!("census: stop: {}", stopped.as_ref().unwrap());
            for w in running.values_mut() {
                let _ = w.child.kill();
            }
        }
    }
    let run = json!({
        "wall_s": started.0.elapsed().as_secs_f64(), "built": built, "workers": options.workers,
        "memory_max": options.memory, "timeout_s": options.timeout.as_secs_f64(),
        "load_start": started.1, "load_end": load(), "available_gb_start": started.2, "available_gb_end": available_gb(),
        "stopped": stopped,
    });
    let summary = summary(&read_lines(&census), &all, run);
    let text = serde_json::to_string_pretty(&summary).expect("JSON");
    std::fs::write(out.join("summary.json"), text).map_err(|e| e.to_string())?;
    eprintln!("census: {} -> {}", census.display(), out.join("summary.json").display());
    match stopped {
        Some(why) => Err(format!("stopped: {why}; run again to resume")),
        None => Ok(()),
    }
}

fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

fn brief(k: &CellKey, v: &Value, extra: &[&str]) -> Value {
    let mut out = json!({"cell": format!("{}/{}", k.x, k.y), "lat": v["lat"], "lon": v["lon"], "state": v["state"]});
    for &e in extra {
        out[e] = v[e].clone();
    }
    out
}

/// The sum of a counter over the lines, and the cells where it is not 0.
fn tally(lines: &BTreeMap<CellKey, Value>, get: impl Fn(&Value) -> f64) -> Value {
    let values: Vec<f64> = lines.values().map(get).collect();
    json!({"total": values.iter().sum::<f64>(), "cells": values.iter().filter(|&&v| v > 0.0).count()})
}

fn summary(everything: &BTreeMap<CellKey, Value>, all: &BTreeSet<CellKey>, run: Value) -> Value {
    let lines: BTreeMap<CellKey, Value> =
        everything.iter().filter(|(k, _)| all.contains(k)).map(|(k, v)| (*k, v.clone())).collect();
    let mut states: BTreeMap<String, usize> = BTreeMap::new();
    for v in lines.values() {
        *states.entry(v["state"].as_str().unwrap_or("?").to_string()).or_default() += 1;
    }
    let ok: Vec<(&CellKey, &Value)> =
        lines.iter().filter(|(_, v)| v.get("ms").is_some() && v["digest"].is_string()).collect();
    let total_ms = |v: &Value| ["input", "graph", "faces", "mesh"].iter().map(|s| num(&v["ms"][s])).sum::<f64>();
    let top = |key: &dyn Fn(&Value) -> f64, extra: &[&str]| -> Vec<Value> {
        let mut sorted = ok.clone();
        sorted.sort_by(|a, b| key(b.1).total_cmp(&key(a.1)));
        sorted.iter().take(TOP).map(|(k, v)| brief(k, v, extra)).collect()
    };
    let mut caps = Map::new();
    for cap in ["slots", "quads", "regions", "slice_fallback"] {
        caps.insert(format!("build.{cap}"), tally(&lines, |v| num(&v["caps"][cap])));
        caps.insert(format!("audit.{cap}"), tally(&lines, |v| num(&v["audit"][cap])));
    }
    for k in ["capped", "host_missing"] {
        caps.insert(format!("audit.{k}_points"), tally(&lines, |v| num(&v["audit"][k])));
    }
    for k in ["outside_d", "unlabelled", "split_edges"] {
        caps.insert(format!("mesh.{k}"), tally(&lines, |v| num(&v["mesh"][k])));
    }
    let mut fields = Map::new();
    for (field, name) in FIELDS {
        let most =
            lines.iter().max_by(|a, b| num(&a.1["caps"]["fields"][name]).total_cmp(&num(&b.1["caps"]["fields"][name])));
        let over = lines.values().filter(|v| num(&v["caps"]["fields"][name]) > field.limit() as f64).count();
        fields.insert(
            name.into(),
            json!({"limit": field.limit(), "cells_over": over,
                   "largest": most.map(|(k, v)| json!({"cell": format!("{}/{}", k.x, k.y), "lat": v["lat"], "lon": v["lon"], "value": v["caps"]["fields"][name]}))}),
        );
    }
    caps.insert("fields".into(), Value::Object(fields));
    let mut counters: BTreeMap<String, (f64, usize)> = BTreeMap::new();
    for v in lines.values() {
        for group in ["graph", "face_stats"] {
            for (name, n) in v[group].as_object().into_iter().flatten() {
                let c = counters.entry(format!("{group}.{name}")).or_default();
                (c.0, c.1) = (c.0 + num(n), c.1 + 1);
            }
        }
    }
    let checks = json!({
        "partition_faults": tally(&lines, |v| num(&v["checks"]["partition"])),
        "artefacts": tally(&lines, |v| num(&v["checks"]["artefacts"])),
        "mesh_validity": tally(&lines, |v| num(&v["mesh"]["validity"])),
        "mesh_labels_ground_wrong": tally(&lines, |v| num(&v["mesh"]["labels"]["ground_wrong"])),
        "mesh_labels_sharp_wrong": tally(&lines, |v| num(&v["mesh"]["labels"]["sharp_wrong"])),
        "first": lines.iter().filter(|(_, v)| v["checks"]["first"].as_array().is_some_and(|f| !f.is_empty()))
            .take(TOP).map(|(k, v)| json!({"cell": format!("{}/{}", k.x, k.y), "lat": v["lat"], "lon": v["lon"], "first": v["checks"]["first"]}))
            .collect::<Vec<_>>(),
    });
    let (mut pairs, mut mesh_pairs, mut differ) = (0, 0, Vec::new());
    let mut by_check: BTreeMap<&str, usize> = BTreeMap::new();
    for (k, v) in &lines {
        for (side, b) in v["borders"].as_object().into_iter().flatten() {
            pairs += 1;
            mesh_pairs += usize::from(!b["mesh"].is_null());
            let bad: Vec<&str> =
                ["faces", "records", "outline", "mesh"].into_iter().filter(|c| num(&b[c]) > 0.0).collect();
            for c in &bad {
                *by_check.entry(c).or_default() += 1;
            }
            if !bad.is_empty() {
                differ.push(json!({"cell": format!("{}/{}", k.x, k.y), "side": side, "lat": v["lat"], "lon": v["lon"],
                    "faces": b["faces"], "records": b["records"], "outline": b["outline"], "mesh": b["mesh"], "first": b["first"]}));
            }
        }
    }
    // A neighbour outside the targets that failed as a mesh's context counts too.
    let failed: Vec<Value> = (everything.iter())
        .filter(|(_, v)| !matches!(v["state"].as_str(), Some("ok" | "empty")))
        .map(|(k, v)| brief(k, v, &["context", "stage", "stage_ms", "panic", "error", "mesh", "exit"]))
        .collect();
    let peak_rss_kb = lines.values().map(|v| num(&v["rss_kb"])).fold(0.0, f64::max);
    json!({
        "cells": all.len(),
        "recorded": lines.len(),
        "states": states,
        "run": run,
        "peak_worker_rss_mb": peak_rss_kb / 1024.0,
        "caps": caps,
        "counters": counters.into_iter().map(|(k, (t, c))| (k, json!({"total": t, "cells": c}))).collect::<Map<_, _>>(),
        "checks": checks,
        "borders": {"pairs": pairs, "mesh_checked": mesh_pairs, "differ_by_check": by_check, "differ": differ},
        "failed": failed,
        "slowest": top(&total_ms, &["ms"]),
        "largest_records": top(&|v| num(&v["words"]), &["words"]),
        "largest_heap": top(&|v| num(&v["heap"]).max(num(&v["heap_mesh"])), &["heap", "heap_mesh"]),
    })
}
