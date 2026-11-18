//! The step `planner/trailview-stamps` of `obc data`: `trailview-stamps.pmtiles`, the bytes that the TrailView
//! stamp worker posts for the baked nodes of each feature tile of `planner/trailview-features`. The stamp code
//! runs in Node, in processes of `builder/web/scripts/trailview-stamps.mjs`, which state its stamp version.

use std::io::{BufReader, BufWriter, Read, Write};
use std::process::{ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use obc_data::engine::Request;
use serde_json::json;

use crate::pmtiles::{self, Archive, Header};
use crate::tiles::{Tiles, MAX_ZOOM, MIN_ZOOM};

/// The stamp process, from the repository root, where a step starts, with its V8 heap capped.
const STAMP: [&str; 3] = ["node", "--max-old-space-size=1024", "builder/web/scripts/trailview-stamps.mjs"];
/// The memory that a stamp process may take: the capped heap, the decoded tiles and the meshes outside the heap.
const PROCESS_BYTES: usize = 3 << 29;
/// Stamp processes at most.
const PROCESSES: usize = 8;
/// The time that a stamp process may take for a tile, and for its start, which builds the bake module.
const TILE_TIMEOUT: Duration = Duration::from_secs(300);

/// The stamp version and the gzip bytes of the nodes by tile id.
type Stamped = (String, Vec<(u64, Vec<u8>)>);

pub fn step(request: &Request) -> Result<(), String> {
    let options = &request.options;
    let bounds: [f64; 4] =
        serde_json::from_value(options["bounds"].clone()).map_err(|_| "option `bounds` is not four numbers")?;
    let attribution = options["attribution"].as_str().ok_or("option `attribution` is not a string")?;
    let features = request
        .layers
        .get("planner/trailview-features")
        .and_then(|files| files.get("trailview-features.pmtiles"))
        .ok_or("the step has no input `planner/trailview-features`")?;
    let archive = Archive::open(features)?;
    // A feature tile holds what comes within one tile of the region.
    let region = Tiles::new(bounds);
    let tiles: Vec<(u8, u32, u32)> = (MIN_ZOOM..=MAX_ZOOM)
        .flat_map(|z| {
            let [c0, r0, c1, r1] = region.span(z);
            let side = 1i64 << z;
            let columns = (c0 - 1).max(0)..=(c1 + 1).min(side - 1);
            columns.flat_map(move |x| ((r0 - 1).max(0)..=(r1 + 1).min(side - 1)).map(move |y| (z, x as u32, y as u32)))
        })
        .collect();
    // The process of a step gets the memory that `obc data` reserves for it in its environment.
    let reserved = request.memory_bytes.or_else(|| std::env::var("OBC_DATA_MEMORY_BYTES").ok()?.parse().ok());
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let processes =
        (crate::step::free_memory(reserved) / PROCESS_BYTES).min(cores).min(PROCESSES).min(tiles.len()).max(1);
    let (queue, stop, first) = (Mutex::new(tiles.into_iter()), AtomicBool::new(false), Mutex::new(None));
    let made = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..processes)
            .map(|_| {
                scope.spawn(|| {
                    let made = stamp(&archive, &queue, &stop);
                    // The first process that fails stops the others, and its error is the step's.
                    if let Err(error) = &made {
                        first.lock().expect("an unpoisoned error").get_or_insert_with(|| error.clone());
                        stop.store(true, Ordering::Relaxed);
                    }
                    made.ok()
                })
            })
            .collect();
        workers.into_iter().map(|worker| worker.join().ok().flatten()).collect::<Vec<_>>()
    });
    if let Some(error) = first.into_inner().expect("an unpoisoned error") {
        return Err(error);
    }
    let made = made.into_iter().collect::<Option<Vec<_>>>().ok_or("a stamp thread panicked")?;
    let version = made[0].0.clone();
    if made.iter().any(|(other, _)| *other != version) {
        return Err("the stamp processes state different stamp versions".into());
    }
    let mut nodes: Vec<(u64, Vec<u8>)> = made.into_iter().flat_map(|(_, nodes)| nodes).collect();
    nodes.sort_by_key(|node| node.0);
    let [west, south, east, north] = bounds;
    let e7 = |value: f64| (value * 1e7).round_ties_even() as i32;
    let header = Header {
        tile_type: 0,
        tile_compression: 2,
        bounds_e7: [e7(west), e7(south), e7(east), e7(north)],
        center_zoom: MAX_ZOOM,
        center_e7: [e7((west + east) / 2.0), e7((south + north) / 2.0)],
    };
    let metadata = json!({"stamp_version": version, "attribution": attribution, "bounds": bounds});
    let output = request.output.join("trailview-stamps.pmtiles");
    let bytes = pmtiles::write(&output, &nodes, &header, &metadata.to_string())?;
    let metrics = json!({"nodes": nodes.len(), "processes": processes, "bytes": bytes, "stamp_version": version});
    std::fs::write(&request.metrics, metrics.to_string()).map_err(|e| e.to_string())
}

/// The nodes of the feature tiles that one stamp process takes from `queue` until it is empty or `stop`
/// is set, in the frames of `trailview-stamps.mjs`. A process that takes longer than TILE_TIMEOUT, or
/// that runs while `stop` is set, is killed.
fn stamp(
    archive: &Archive,
    queue: &Mutex<impl Iterator<Item = (u8, u32, u32)>>,
    stop: &AtomicBool,
) -> Result<Stamped, String> {
    let mut child = Command::new(STAMP[0])
        .args(&STAMP[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| format!("{}: {e}", STAMP.join(" ")))?;
    let input = BufWriter::new(child.stdin.take().expect("a piped input"));
    let output = BufReader::new(child.stdout.take().expect("a piped output"));
    let (child, since, done, late) =
        (Mutex::new(child), Mutex::new(Instant::now()), AtomicBool::new(false), AtomicBool::new(false));
    let made = std::thread::scope(|scope| {
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                let over = since.lock().expect("an unpoisoned clock").elapsed() > TILE_TIMEOUT;
                if over || stop.load(Ordering::Relaxed) {
                    late.store(over, Ordering::Relaxed);
                    let _ = child.lock().expect("an unpoisoned child").kill();
                    break;
                }
            }
        });
        let made = frames(archive, queue, stop, input, output, &since);
        done.store(true, Ordering::Relaxed);
        made
    });
    let status = child.into_inner().expect("an unpoisoned child").wait().map_err(|e| e.to_string())?;
    if late.load(Ordering::Relaxed) {
        return Err(format!("a stamp process took more than {} s for a tile", TILE_TIMEOUT.as_secs()));
    }
    let made = made?;
    if !status.success() {
        return Err(format!("the stamp process ended with {status}"));
    }
    Ok(made)
}

/// The frames of one stamp process: its version, then a tile out and its nodes back, until the queue is
/// empty or `stop` is set. `since` is when the current tile started.
fn frames(
    archive: &Archive,
    queue: &Mutex<impl Iterator<Item = (u8, u32, u32)>>,
    stop: &AtomicBool,
    mut input: BufWriter<std::process::ChildStdin>,
    mut output: BufReader<ChildStdout>,
    since: &Mutex<Instant>,
) -> Result<Stamped, String> {
    let ended = |e: std::io::Error| format!("the stamp process ended ({e}); its errors are above");
    let length = read_u32(&mut output).map_err(ended)?;
    let mut version = vec![0; length as usize];
    output.read_exact(&mut version).map_err(ended)?;
    let version = String::from_utf8(version).map_err(|e| e.to_string())?;
    let mut nodes = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        let Some((z, x, y)) = queue.lock().expect("an unpoisoned queue").next() else { break };
        let Some(body) = archive.get(z, x, y)? else { continue };
        *since.lock().expect("an unpoisoned clock") = Instant::now();
        input.write_all(&head(z, x, y, body.len())).and_then(|()| input.write_all(&body)).map_err(ended)?;
        input.flush().map_err(ended)?;
        for _ in 0..read_u32(&mut output).map_err(ended)? {
            let mut place = [0u8; 13];
            output.read_exact(&mut place).map_err(ended)?;
            let word = |at: usize| u32::from_le_bytes(place[at..at + 4].try_into().expect("four bytes"));
            let mut bytes = vec![0; word(9) as usize];
            output.read_exact(&mut bytes).map_err(ended)?;
            nodes.push((pmtiles::tile_id(place[0], word(1), word(5)), pmtiles::gzip(&bytes)));
        }
    }
    drop(input);
    if stop.load(Ordering::Relaxed) {
        return Err("another stamp process failed".into());
    }
    Ok((version, nodes))
}

fn head(z: u8, x: u32, y: u32, length: usize) -> Vec<u8> {
    let mut out = vec![z];
    for word in [x, y, length as u32] {
        out.extend(word.to_le_bytes());
    }
    out
}

fn read_u32(output: &mut BufReader<ChildStdout>) -> std::io::Result<u32> {
    let mut word = [0u8; 4];
    output.read_exact(&mut word)?;
    Ok(u32::from_le_bytes(word))
}
