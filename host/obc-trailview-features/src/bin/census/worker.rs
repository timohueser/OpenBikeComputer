//! A worker: it builds the cells of its job as the viewer does and prints one JSON line per cell. It
//! walks its targets by row and keeps the cells of the last three rows, so each target's mesh reads the
//! rims of its eight neighbours and each target meets its west and north neighbours in the checks.
//! Before each stage it prints `{"begin": [x, y], "stage": …}`: the driver times that stage and names
//! the cell when the worker ends.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::time::Instant;

use obc_ground_plan::cell::{build_cell_with, CellBuild};
use obc_ground_plan::faces::check::{artefacts, feature_names, neighbours, partition};
use obc_ground_plan::faces::heights::check::caps;
use obc_ground_plan::faces::records::check::{border as records_border, outline_border};
use obc_ground_plan::faces::{window, Rules};
use obc_ground_plan::frame::CellKey;
use obc_ground_plan::mesh::{check as mesh_check, mesh_cell, CellMesh, Reads, Rim};
use obc_ground_plan::probe::{self, Probe, FIELDS};
use obc_ground_plan::terrain::Terrain;
use obc_ground_plan::tile::TileCache;
use obc_ground_plan::transport::Flat;
use serde_json::{json, Map, Value};

use crate::heap;
use crate::source::{Place, Source};
use crate::{centre, Job};

/// The zoom-13 tiles that the worker keeps parsed, as the viewer's plan worker does.
const TILES: usize = 6;
/// The step along a shared line of the records' border check, in frame units.
const BORDER_STEP: i32 = 256;
/// The faults that a line names per check.
const FIRST: usize = 3;

/// What the worker does besides the census, for the driver's test: a made-up failure of one cell, or
/// of each worker before its first cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fail {
    Panic,
    Hang,
    Start,
}

pub struct Options {
    pub place: Place,
    pub grid_m: f64,
    pub fail: Vec<(CellKey, Fail)>,
}

struct Built {
    build: CellBuild,
    terrain: Terrain,
    names: Vec<[u32; 3]>,
    rim: Rim,
    heap: usize,
    probe: Probe,
    mesh: Option<CellMesh>,
}

impl Built {
    fn face_cell(&self) -> obc_ground_plan::faces::check::Cell<'_> {
        (&self.build.faces, &self.build.graph, &self.names)
    }

    fn records_cell(&self) -> obc_ground_plan::faces::records::check::Cell<'_> {
        (&self.build.records, &self.build.graph, &self.names)
    }

    fn mesh_cell<'a>(&'a self, mesh: &'a CellMesh) -> mesh_check::Cell<'a> {
        mesh_check::Cell { key: self.build.key, mesh, words: &self.build.records, terrain: &self.terrain }
    }
}

enum Slot {
    Built(Box<Built>),
    /// None of its feature tiles exists, or the driver skips it.
    Missing,
    Error(String),
}

fn emit(line: &Value) {
    let mut out = std::io::stdout().lock();
    writeln!(out, "{line}").and_then(|()| out.flush()).expect("the driver reads the lines");
}

fn begin(key: CellKey, stage: &str, options: &Options) {
    emit(&json!({"begin": [key.x, key.y], "stage": stage}));
    match options.fail.iter().find(|f| f.0 == key && stage == "build").map(|f| f.1) {
        Some(Fail::Panic) => panic!("a made-up failure of cell {}/{}", key.x, key.y),
        Some(Fail::Hang) => loop {
            std::thread::sleep(std::time::Duration::from_secs(1));
        },
        Some(Fail::Start) | None => {}
    }
}

/// The cells about a cell, by row, then column, itself among them.
fn around(key: CellKey) -> impl Iterator<Item = CellKey> {
    (-1..=1i64).flat_map(move |dy| {
        (-1..=1i64).map(move |dx| CellKey { x: (i64::from(key.x) + dx) as u32, y: (i64::from(key.y) + dy) as u32 })
    })
}

fn probe_json(p: &Probe) -> Value {
    let fields: Map<String, Value> = FIELDS.iter().map(|f| (f.1.to_string(), json!(p.fields[f.0 as usize]))).collect();
    json!({"slots": p.slots, "quads": p.quads, "regions": p.regions, "slice_fallback": p.slice_fallback, "fields": fields})
}

fn counters<S: AsRef<str>>(list: &[(S, u32)]) -> Value {
    Value::Object(list.iter().filter(|c| c.1 > 0).map(|c| (c.0.as_ref().to_string(), json!(c.1))).collect())
}

fn rss_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let line = status.lines().find(|l| l.starts_with("VmHWM:")).unwrap_or("");
    line.split_whitespace().nth(1).and_then(|v| v.parse().ok()).unwrap_or(0)
}

struct Worker<'a> {
    options: &'a Options,
    source: Source,
    cache: TileCache,
    rules: &'static Rules,
    cells: BTreeMap<CellKey, Slot>,
    skip: BTreeSet<CellKey>,
    start: Instant,
}

impl Worker<'_> {
    fn now(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1e3
    }

    fn ensure(&mut self, key: CellKey) -> Result<(), String> {
        if self.cells.contains_key(&key) {
            return Ok(());
        }
        if self.skip.contains(&key) {
            self.cells.insert(key, Slot::Missing);
            return Ok(());
        }
        begin(key, "build", self.options);
        let Some(request) = self.source.request(key)? else {
            self.cells.insert(key, Slot::Missing);
            return Ok(());
        };
        let bytes = request.encode();
        probe::take();
        let base = heap::start();
        let start = self.start;
        let built = build_cell_with(&bytes, &mut self.cache, self.rules, &mut || start.elapsed().as_secs_f64() * 1e3);
        let (heap, probe) = (heap::peak(base), probe::take());
        let slot = match built {
            Err(e) => Slot::Error(e),
            Ok(build) => {
                let input = request.input(&mut self.cache)?;
                let rim = Rim::of(&build.faces);
                let names = feature_names(&input);
                Slot::Built(Box::new(Built { build, terrain: input.terrain, names, rim, heap, probe, mesh: None }))
            }
        };
        self.cells.insert(key, slot);
        Ok(())
    }

    fn built(&self, key: CellKey) -> Option<&Built> {
        match self.cells.get(&key) {
            Some(Slot::Built(b)) => Some(b),
            _ => None,
        }
    }

    /// The line of a target, its mesh kept for the checks of its east and south neighbours.
    fn census(&mut self, key: CellKey) -> Value {
        let (lat, lon) = centre(key);
        let mut line = json!({"cell": [key.x, key.y], "lat": lat, "lon": lon});
        let b = match self.cells.get(&key) {
            Some(Slot::Built(b)) => b,
            Some(Slot::Error(e)) => {
                line["state"] = json!("error");
                line["error"] = json!(e);
                return line;
            }
            _ => {
                line["state"] = json!("empty");
                return line;
            }
        };
        let build = &b.build;
        begin(key, "mesh", self.options);
        let rims: Vec<Rim> = around(key).filter_map(|k| self.built(k).map(|n| n.rim.clone())).collect();
        let reads = Reads::of(&build.faces, &rims);
        let t0 = self.now();
        let base = heap::start();
        let start = self.start;
        let mesh = mesh_cell(&build.faces, &build.records, &reads, &mut || start.elapsed().as_secs_f64() * 1e3);
        let heap_mesh = heap::peak(base);
        let mesh_ms = self.now() - t0;
        begin(key, "checks", self.options);
        let t1 = self.now();
        let (mut laps, mut last) = (Map::new(), t1);
        let mut lap = |name: &str| {
            let t = start.elapsed().as_secs_f64() * 1e3;
            laps.insert(name.into(), json!(t - last));
            last = t;
        };
        let (plan, graph) = (&build.faces, &build.graph);
        let mut first: Vec<String> = Vec::new();
        let mut note = |found: Vec<String>| {
            let n = found.len();
            first.extend(found.into_iter().take(FIRST));
            n
        };
        let digest = Flat::of(build).digest(build).unwrap_or_else(|e| format!("error: {e}"));
        lap("digest");
        let faults = note(partition(plan, window(key, 0.0)).into_iter().map(|f| format!("1: {}", f.what)).collect());
        lap("partition");
        let thin = artefacts(plan, graph).len();
        lap("artefacts");
        let mesh_json = match &mesh {
            Ok(m) => {
                let validity = note(mesh_check::validity(m).into_iter().map(|f| format!("mesh: {f}")).collect());
                let labels = mesh_check::labels(m, &build.records);
                let s = &m.stats;
                json!({
                    "corners": m.corners.len(), "triangles": m.triangles.len(), "strip_triangles": m.strip_triangles,
                    "steps": m.steps.len(), "outside_d": s.outside_d, "unlabelled": s.unlabelled,
                    "split_edges": s.split_edges, "validity": validity,
                    "labels": {"sharp_wrong": labels.sharp_wrong, "ground_wrong": labels.ground_wrong, "ground_other": labels.ground_other},
                })
            }
            Err(e) => json!({"error": format!("{e:?}")}),
        };
        lap("mesh");
        probe::take();
        let [points, capped, host_missing] = caps(&build.records, &b.terrain, key, self.options.grid_m);
        let mut audit = probe_json(&probe::take());
        for (k, v) in [("grid_m", json!(self.options.grid_m)), ("points", json!(points)), ("capped", json!(capped))] {
            audit[k] = v;
        }
        audit["host_missing"] = json!(host_missing);
        lap("audit");
        let mut borders = Map::new();
        for (side, other, east) in
            [("west", CellKey { x: key.x - 1, y: key.y }, true), ("north", CellKey { x: key.x, y: key.y - 1 }, false)]
        {
            let Some(a) = self.built(other) else { continue };
            let faces = neighbours(a.face_cell(), b.face_cell(), east);
            let (_, _, records) = records_border(a.records_cell(), b.records_cell(), east, BORDER_STEP);
            let (_, [outline, _]) = outline_border((&a.build.records, other), (&build.records, key));
            let meshed = match (&a.mesh, &mesh) {
                (Some(ma), Ok(mb)) => Some(mesh_check::border(&a.mesh_cell(ma), &b.mesh_cell(mb), east).0),
                _ => None,
            };
            let mut named: Vec<String> = faces.iter().map(|f| format!("2: {}", f.what)).take(FIRST).collect();
            named.extend(records.iter().take(FIRST).map(|d| format!("records at {}: {} | {}", d.0, d.1, d.2)));
            named.extend(outline.iter().take(FIRST).map(|d| format!("outline: {d}")));
            named.extend(meshed.iter().flatten().take(FIRST).map(|d| format!("mesh: {d}")));
            borders.insert(
                side.into(),
                json!({"faces": faces.len(), "records": records.len(), "outline": outline.len(),
                       "mesh": meshed.as_ref().map(Vec::len), "first": named}),
            );
        }
        lap("borders");
        let times = &build.times;
        line["state"] = json!(if mesh.is_ok() { "ok" } else { "mesh_error" });
        line["digest"] = json!(digest);
        line["ms"] = json!({"input": times[0], "graph": times[1], "faces": times[2], "mesh": mesh_ms, "checks": self.now() - t1});
        line["check_ms"] = Value::Object(laps);
        line["heap"] = json!(b.heap);
        line["heap_mesh"] = json!(heap_mesh);
        line["rss_kb"] = json!(rss_kb());
        for (k, v) in [
            ("tiles", build.tiles),
            ("pages", build.pages),
            ("ways", graph.ways.len()),
            ("words", build.records.len()),
            ("faces", plan.faces.len()),
            ("vertices", plan.vertices.len()),
            ("edges", plan.edges.len()),
        ] {
            line[k] = json!(v);
        }
        line["mesh"] = mesh_json;
        line["caps"] = probe_json(&b.probe);
        line["audit"] = audit;
        line["graph"] = counters(&graph.stats.counters());
        line["levels"] = crate::fixture::levels_detail(graph);
        line["face_stats"] = counters(&plan.stats.counters());
        line["checks"] = json!({"partition": faults, "artefacts": thin, "first": first});
        line["borders"] = Value::Object(borders);
        if let (Ok(m), Some(Slot::Built(b))) = (mesh, self.cells.get_mut(&key)) {
            b.mesh = Some(m);
        }
        line
    }
}

pub fn run(options: Options) -> Result<(), String> {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_default();
        let at = info.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        eprintln!("census worker: panic at {at}: {message}");
        emit(&json!({"panic": {"message": message, "at": at}}));
    }));
    if options.fail.iter().any(|f| f.1 == Fail::Start) {
        return Err("a made-up failure before the first cell".into());
    }
    let mut text = String::new();
    std::io::stdin().lock().read_line(&mut text).map_err(|e| e.to_string())?;
    let job = Job::parse(&text)?;
    let mut worker = Worker {
        source: Source::open(&options.place)?,
        options: &options,
        cache: TileCache::new(TILES),
        rules: Rules::embedded(),
        cells: BTreeMap::new(),
        skip: job.skip.into_iter().collect(),
        start: Instant::now(),
    };
    for key in job.targets {
        worker.cells.retain(|k, _| k.y + 1 >= key.y);
        for n in around(key) {
            worker.ensure(n)?;
        }
        let line = worker.census(key);
        emit(&line);
    }
    Ok(())
}
