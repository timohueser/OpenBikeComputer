//! Runs: a run fetches and builds a plan, as many steps at a time as the machine allows. It keeps
//! its status in `runs/<id>/status.json` of the store, and its log in `runs/<id>/log`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::plan::Plan;
use super::{build_step, order, prepare, reusable, Built, Receipt, Shape, Step};
use crate::date;
use crate::fetch::{self, http::Http};
use crate::release::Output;
use crate::sources::Source;
use crate::store::{Lock, Store};

/// How many steps run at a time.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub jobs: usize,
    /// The sum of the estimated peaks of the steps that run together, or `None` for no limit.
    pub memory_bytes: Option<u64>,
}

impl Limits {
    /// One step per core, and the physical memory of the machine.
    pub fn machine() -> Self {
        Self { jobs: std::thread::available_parallelism().map_or(1, |n| n.get()), memory_bytes: memory() }
    }
}

#[cfg(unix)]
fn memory() -> Option<u64> {
    // SAFETY: sysconf reads a system value.
    let (pages, size) = unsafe { (libc::sysconf(libc::_SC_PHYS_PAGES), libc::sysconf(libc::_SC_PAGESIZE)) };
    (pages > 0 && size > 0).then(|| pages as u64 * size as u64)
}

#[cfg(not(unix))]
fn memory() -> Option<u64> {
    None
}

/// What a run uses besides its steps and its plan.
#[derive(Clone, Copy)]
pub struct Context<'a> {
    pub store: &'a Store,
    /// The repository root: a command starts in it.
    pub root: &'a Path,
    /// The sources of `data/sources.toml`: a fetch finds its source here.
    pub sources: &'a [Source],
    pub http: &'a Http,
    /// The outputs that the update keeps: a step that reads one reads its files in the release.
    pub kept: &'a super::plan::Kept,
    pub limits: Limits,
}
/// What a run records: its log gets a line, and its status changes.
#[derive(Debug, Clone)]
pub enum Event {
    Phase {
        phase: Phase,
    },
    /// The switch of the target to `release`.
    Switched {
        release: String,
    },
    FetchStarted {
        source: String,
        version: String,
        params: Vec<(String, String)>,
    },
    FetchFinished {
        source: String,
        version: String,
        params: Vec<(String, String)>,
        resolved: String,
        acquisition: Option<serde_json::Value>,
        /// The size of the files that the fetch gave, downloaded or found in the store.
        bytes: u64,
        wall_ms: u64,
    },
    FetchFailed {
        source: String,
        version: String,
        params: Vec<(String, String)>,
        error: String,
        acquisition: Option<serde_json::Value>,
    },
    StepStarted {
        step: String,
    },
    StepFinished {
        step: String,
        reused: bool,
        receipt: Box<Receipt>,
    },
    StepFailed {
        step: String,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepare,
    Download,
    Build,
    Verify,
    Upload,
    Switch,
    Collect,
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = serde_json::to_value(self).map_err(|_| std::fmt::Error)?;
        f.write_str(name.as_str().unwrap_or_default())
    }
}

/// The status file and the log file of a run.
const STATUS: &str = "status.json";
const LOG: &str = "log";
/// The file whose presence stops a run: `obc data runs RUN --stop` writes it.
const STOP: &str = "stop";

/// How often a run writes its status at most while events come.
const SAVE: Duration = Duration::from_secs(1);

/// A run that this process runs. The process that holds the lock `run-<id>` is the process of the
/// run, so a reader knows that a run whose status says `running` still runs. Its store stays in
/// use until finish or drop.
pub struct Run {
    dir: PathBuf,
    status: Status,
    log: File,
    start: Instant,
    /// When the run wrote its status last, and whether it changed since.
    saved: Instant,
    unsaved: bool,
    /// The index in `status.layers` and the leaf of each step that the plan bakes.
    planned: HashMap<String, (usize, Option<String>)>,
    /// Set when another run took the lock of the target of this run.
    stop: Arc<AtomicBool>,
    /// Whether another process reserved the run, and this one runs it without a terminal.
    detached: bool,
    _lock: Lock,
    _using: Lock,
}

/// Reserve a run of the command `args`, the arguments after `obc data`: its id is its start time
/// `YYYY-MM-DD-HHMMSS`, with a suffix `-N` when another run has it. The folder `runs/<id>/`
/// reserves it, and its status says `starting` until a process takes the run.
pub fn reserve(store: &Store, args: &[String]) -> Result<String, String> {
    let started = date::timestamp(date::now());
    let base = format!("{}-{}", &started[..10], started[11..19].replace(':', ""));
    crate::store::durable_directory(&store.root().join("runs"))?;
    for n in 1.. {
        let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
        let dir = store.run(&id);
        match fs::create_dir(&dir) {
            Ok(()) => {
                let pid = std::process::id();
                let status = Status { id: id.clone(), args: args.to_vec(), pid, started, ..Status::default() };
                write_status(&dir, &status)?;
                return Ok(id);
            }
            Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("{}: {e}", dir.display())),
        }
    }
    unreachable!("the ids never run out")
}

/// Write `status` into the folder `dir` of its run, with one rename.
fn write_status(dir: &Path, status: &Status) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(status).map_err(|e| e.to_string())?;
    let (path, part) = (dir.join(STATUS), dir.join(format!(".{STATUS}.part")));
    fs::write(&part, bytes).and_then(|()| fs::rename(&part, &path)).map_err(|e| format!("{}: {e}", path.display()))
}

impl Run {
    /// Start a run of the command `args`, the arguments after `obc data`, in this process.
    pub fn create(store: &Store, args: impl IntoIterator<Item = impl Into<String>>) -> Result<Run, String> {
        let args: Vec<String> = args.into_iter().map(Into::into).collect();
        let id = reserve(store, &args)?;
        Run::start(store, &id, false)
    }

    /// Start the run `id` that another process reserved, in this process.
    pub fn attach(store: &Store, id: &str) -> Result<Run, String> {
        Run::start(store, id, true)
    }

    fn start(store: &Store, id: &str, detached: bool) -> Result<Run, String> {
        check_id(id)?;
        let using = store.using()?;
        let lock = store.lock(&format!("run-{id}"))?;
        let dir = store.run(id);
        let reserved = status(store, id)?;
        if reserved.state != State::Starting {
            return Err(format!("run {id} started before"));
        }
        let log = OpenOptions::new().create(true).append(true).open(dir.join(LOG));
        let log = log.map_err(|e| format!("{}: {e}", dir.join(LOG).display()))?;
        let status = Status { pid: std::process::id(), state: State::Running, ..reserved };
        let now = Instant::now();
        let mut run = Run {
            dir,
            status,
            log,
            start: now,
            saved: now,
            unsaved: false,
            planned: HashMap::new(),
            stop: Arc::new(AtomicBool::new(false)),
            detached,
            _lock: lock,
            _using: using,
        };
        run.line(&format!("started `obc data {}`", run.status.args.join(" ")))?;
        run.save()?;
        Ok(run)
    }

    pub fn id(&self) -> &str {
        &self.status.id
    }

    /// Whether another process reserved the run: its standard error is the log of the run.
    pub fn detached(&self) -> bool {
        self.detached
    }

    /// Record `event`: a line in the log, and the status, which the run writes at most each second
    /// while events come.
    pub fn record(&mut self, event: &Event) -> Result<(), String> {
        let line = self.apply(event);
        self.line(&line)?;
        self.unsaved = true;
        // A fetch can take long: its status shows that it runs.
        let now = matches!(event, Event::Phase { .. } | Event::Switched { .. } | Event::FetchStarted { .. });
        if now || self.saved.elapsed() >= SAVE {
            self.save()?;
        }
        Ok(())
    }

    /// Write the status when it changed since the run wrote it.
    pub fn flush(&mut self) -> Result<(), String> {
        match self.unsaved {
            true => self.save(),
            false => Ok(()),
        }
    }

    fn save(&mut self) -> Result<(), String> {
        write_status(&self.dir, &self.status)?;
        (self.saved, self.unsaved) = (Instant::now(), false);
        Ok(())
    }

    /// The status file of the run.
    pub fn status_file(&self) -> PathBuf {
        self.dir.join(STATUS)
    }

    fn line(&mut self, text: &str) -> Result<(), String> {
        let at = date::timestamp(date::now());
        let line = format!("{} {text}\n", &at[11..19]);
        self.log.write_all(line.as_bytes()).map_err(|e| format!("run {}: {e}", self.status.id))
    }

    /// Apply `event` to the status, and give its line of the log.
    fn apply(&mut self, event: &Event) -> String {
        let status = &mut self.status;
        match event.clone() {
            Event::Phase { phase } => {
                if let Some(last) = status.phases.last_mut() {
                    last.state = State::Ok;
                }
                status.phases.push(PhaseState { phase, state: State::Running, started: date::timestamp(date::now()) });
                format!("phase {phase}")
            }
            Event::Switched { release } => {
                let line = format!("switched to release {release}");
                status.switched.push(release);
                line
            }
            Event::FetchStarted { source, version, params } => {
                let line = format!("{} fetch started", fetched(&source, &version, &params));
                status.fetch(source, version, params);
                line
            }
            Event::FetchFinished { source, version, params, resolved, acquisition, bytes: size, wall_ms } => {
                let line = format!(
                    "{} fetched {resolved} in {}, {}",
                    fetched(&source, &version, &params),
                    duration(wall_ms),
                    bytes(size)
                );
                status.bytes_fetched += size;
                let fetch = status.fetch(source, version, params);
                (fetch.bytes, fetch.wall_ms, fetch.resolved) = (Some(size), Some(wall_ms), Some(resolved));
                fetch.acquisition = acquisition;
                line
            }
            Event::FetchFailed { source, version, params, error, acquisition } => {
                let line = format!("{} fetch failed: {error}", fetched(&source, &version, &params));
                let fetch = status.fetch(source, version, params);
                (fetch.error, fetch.acquisition) = (Some(error), acquisition);
                line
            }
            Event::StepStarted { step } => {
                let line = format!("{step} started");
                if let Some((layer, leaf)) = self.planned.get(&step) {
                    status.layers[*layer].leaves_baking.extend(leaf.clone());
                }
                status.step(&step);
                line
            }
            Event::StepFinished { step, reused, receipt } => {
                let line = match reused {
                    true => format!("{step} reused"),
                    false => format!("{step} built in {}, {}", duration(receipt.wall_ms), bytes(receipt.bytes_out)),
                };
                if !reused {
                    status.bytes_built += receipt.bytes_out;
                }
                if let Some((layer, leaf)) = self.planned.get(&step) {
                    let layer = &mut status.layers[*layer];
                    layer.done += 1;
                    if let Some(leaf) = leaf {
                        layer.leaves_baking.retain(|baking| baking != leaf);
                        layer.leaves_done.push(leaf.clone());
                    }
                }
                let run_step = status.step(&step);
                run_step.layer = receipt.layer.clone();
                (run_step.reused, run_step.wall_ms, run_step.bytes_out) =
                    (reused, Some(receipt.wall_ms), Some(receipt.bytes_out));
                run_step.peak_rss_bytes = receipt.peak_rss_bytes;
                run_step.reads = receipt.inputs.iter().map(|input| input.name.clone()).collect();
                line
            }
            Event::StepFailed { step, error } => {
                let line = format!("{step} failed: {error}");
                if let Some((layer, Some(leaf))) = self.planned.get(&step) {
                    status.layers[*layer].leaves_baking.retain(|baking| baking != leaf);
                }
                status.step(&step).error = Some(error);
                line
            }
        }
    }

    /// The layers of the steps that `plan` bakes, with the leaves of a tiled layer.
    fn plan_layers(&mut self, plan: &Plan, steps: &[&Step]) -> Result<(), String> {
        for build in plan.builds() {
            let Some(step) = steps.iter().find(|step| step.name == build.step) else { continue };
            if self.planned.contains_key(&step.name) {
                continue;
            }
            let layers = &mut self.status.layers;
            let at = layers.iter().position(|layer| layer.layer == step.layer.name).unwrap_or_else(|| {
                layers.push(LayerProgress { layer: step.layer.name.into(), ..LayerProgress::default() });
                layers.len() - 1
            });
            layers[at].total += 1;
            let leaf = super::leaf(&step.name).filter(|_| step.layer.shape == Shape::Tiled).map(str::to_string);
            self.planned.insert(step.name.clone(), (at, leaf));
        }
        self.save()
    }

    pub fn check_stop(&self, store: &Store) -> Result<(), String> {
        if self.stop.load(Ordering::Relaxed) {
            return Err("the run stopped: another run took the lock of its target".into());
        }
        if self.stopping() {
            return Err("the run stopped on a signal or `obc data runs RUN --stop`; it keeps what it baked".into());
        }
        crate::store::check_free(store.root(), 0)
    }

    /// Whether a signal or `obc data runs RUN --stop` stops the run.
    pub fn stopping(&self) -> bool {
        signalled() || self.dir.join(STOP).exists()
    }

    /// The flag that stops the run before its next fetch or step.
    pub fn stopper(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.stop)
    }

    /// Write the status and flush the log.
    pub fn sync(&mut self) -> Result<(), String> {
        self.save()?;
        self.log.sync_all().map_err(|e| format!("run {}: {e}", self.status.id))
    }

    /// Fetch the fetches of `plan` one after another, then build its builds and reuse the layers
    /// that they read. After a fetch or a step fails, no other step starts; the steps that run
    /// finish. A later run reuses every layer that this one built.
    pub fn build(&mut self, context: &Context, steps: &[Step], plan: &Plan) -> Result<Vec<Built>, String> {
        let Context { store, root, limits, .. } = *context;
        self.check_stop(store)?;
        let _using = store.using()?;
        if limits.jobs == 0 {
            return Err("the limit of jobs is 0; it must be 1 or more".into());
        }
        let ordered = order(steps)?;
        if let Some(build) = plan.builds().find(|build| !ordered.iter().any(|step| step.name == build.step)) {
            return Err(format!("the plan builds `{}`, which no step makes", build.step));
        }
        self.plan_layers(plan, &ordered)?;
        self.fetch(context, plan)?;

        // A step whose peak is not known reserves the whole memory, so it runs alone.
        let cost = |peak: Option<u64>| limits.memory_bytes.map_or(0, |memory| peak.unwrap_or(memory));
        let planned: HashMap<&str, (u64, Option<&str>, bool)> = plan
            .builds()
            .map(|build| {
                let peak = build.estimate.and_then(|estimate| estimate.peak_rss_bytes);
                (build.step.as_str(), (cost(peak), build.key.as_deref(), build.again))
            })
            .collect();
        // The planned steps and the layers that they read, in dependency order.
        let mut needed: HashSet<&str> = planned.keys().copied().collect();
        for step in ordered.iter().rev() {
            if needed.contains(step.name.as_str()) {
                needed.extend(step.reads());
            }
        }
        let position: HashMap<&str, usize> =
            ordered.iter().enumerate().map(|(i, step)| (step.name.as_str(), i)).collect();
        let mut pending: Vec<&Step> = ordered.into_iter().filter(|step| needed.contains(step.name.as_str())).collect();
        let outdated = "the plan is outdated; plan again";
        let mut done: HashMap<&str, Output> = HashMap::new();
        let mut built = Vec::new();
        let mut failure: Option<String> = None;
        let (mut running, mut reserved) = (0, 0);
        let (sender, receiver) = mpsc::channel();
        std::thread::scope(|scope| loop {
            if failure.is_none() {
                failure = self.check_stop(store).err();
            }
            let mut i = 0;
            while failure.is_none() && i < pending.len() {
                if let Err(error) = self.check_stop(store) {
                    failure = Some(error);
                    break;
                }
                let step = pending[i];
                if !step.reads().all(|name| done.contains_key(name)) {
                    i += 1;
                    continue;
                }
                let Some(&(cost, key, again)) = planned.get(step.name.as_str()) else {
                    // A layer that a planned step reads: the release keeps it, or the store has it,
                    // unless the plan is old.
                    pending.remove(i);
                    if let Some(output) = context.kept.get(&step.name) {
                        done.insert(&step.name, output.clone());
                        continue;
                    }
                    let reused = prepare(store, step, &done)
                        .and_then(|(receipt, _)| {
                            reusable(store, &receipt.key).map(|stored| {
                                stored.map(|mut stored| {
                                    stored.inputs = receipt.inputs;
                                    stored
                                })
                            })
                        })
                        .and_then(|stored| stored.ok_or(outdated.into()));
                    match reused {
                        Ok(receipt) => {
                            done.insert(&step.name, Output::new(&receipt, step));
                        }
                        Err(e) => failure = Some(format!("step `{}`: {e}", step.name)),
                    }
                    continue;
                };
                let over = limits.memory_bytes.is_some_and(|memory| reserved + cost > memory);
                if running >= limits.jobs || (running > 0 && over) {
                    i += 1;
                    continue;
                }
                pending.remove(i);
                let prepared = prepare(store, step, &done).and_then(|(receipt, request)| match key {
                    Some(key) if key != receipt.key => Err(outdated.to_string()),
                    _ => Ok((receipt, request)),
                });
                // The files of the steps that it reads; a kept step's come from the target.
                let prepared = prepared.and_then(|(receipt, request)| {
                    for file in request.layer_files.values().flatten() {
                        store.ensure(&file.sha256)?;
                    }
                    Ok((receipt, request))
                });
                let started = self.record(&Event::StepStarted { step: step.name.clone() });
                match prepared.and_then(|prepared| started.map(|()| prepared)) {
                    Ok((receipt, request)) => {
                        let sender = sender.clone();
                        scope.spawn(move || {
                            // The loop waits for every step that it started, also one that panics.
                            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                                build_step(store, root, step, receipt, request, again)
                            }));
                            let _ = sender.send((step, cost, result.unwrap_or_else(|payload| Err(panicked(payload)))));
                        });
                        running += 1;
                        reserved += cost;
                    }
                    Err(e) => self.failed(step, e, &mut failure),
                }
            }
            if running == 0 {
                break;
            }
            let (step, cost, result) = loop {
                match receiver.recv_timeout(SAVE) {
                    Ok(received) => break received,
                    // The status shows the steps that started while a long step runs.
                    Err(_) => {
                        if let Err(e) = self.flush() {
                            failure.get_or_insert(e);
                        }
                    }
                }
            };
            running -= 1;
            reserved -= cost;
            match result {
                Ok(result) => {
                    let receipt = Box::new(result.receipt.clone());
                    let event = Event::StepFinished { step: step.name.clone(), reused: result.reused, receipt };
                    if let Err(e) = self.record(&event) {
                        failure.get_or_insert(e);
                    }
                    done.insert(&step.name, Output::new(&result.receipt, step));
                    built.push(result);
                }
                Err(e) => self.failed(step, e, &mut failure),
            }
        });
        if let Some(failure) = failure {
            return Err(failure);
        }
        if let Some(step) = pending.first() {
            return Err(format!("step `{}` reads a layer that the run did not build; plan again", step.name));
        }
        built.sort_by_key(|built| position[built.receipt.step.as_str()]);
        Ok(built)
    }

    /// Fetch what `plan` fetches.
    pub fn fetch(&mut self, context: &Context, plan: &Plan) -> Result<(), String> {
        for planned in plan.fetches() {
            self.check_stop(context.store)?;
            let source = context.sources.iter().find(|known| known.id == planned.source).ok_or_else(|| {
                format!(
                    "fetch {}@{}: no source `{}` in data/sources.toml",
                    planned.source, planned.version, planned.source
                )
            })?;
            let request = fetch::Request {
                refresh: false,
                source,
                version: Some(planned.version.clone()),
                params: planned.params.clone(),
            };
            self.fetch_request(context.root, context.store, context.http, &request, &planned.files)?;
        }
        Ok(())
    }

    /// Fetch `request`, and record it. `names` names the files of a prepared version that a step
    /// reads.
    pub fn fetch_request(
        &mut self,
        root: &Path,
        store: &Store,
        http: &Http,
        request: &fetch::Request<'_>,
        names: &[String],
    ) -> Result<crate::store::Snapshot, String> {
        self.check_stop(store)?;
        let (source, version, params) = (
            request.source.id.clone(),
            request.version.clone().unwrap_or_else(|| "newest".into()),
            request.params.clone(),
        );
        self.record(&Event::FetchStarted { source: source.clone(), version: version.clone(), params: params.clone() })?;
        let start = Instant::now();
        let mut acquisition = None;
        match fetch::fetch_recorded(root, store, http, request, names, Some(&mut acquisition)) {
            Ok(snapshot) => {
                let bytes = snapshot.files.iter().map(|file| file.size).sum();
                let wall_ms = start.elapsed().as_millis() as u64;
                self.record(&Event::FetchFinished {
                    source,
                    version,
                    params,
                    resolved: snapshot.version.clone(),
                    acquisition,
                    bytes,
                    wall_ms,
                })?;
                Ok(snapshot)
            }
            Err(error) => {
                let mut failed = format!("fetch {source}@{version}: {error}");
                if let Err(journal) = self.record(&Event::FetchFailed { source, version, params, error, acquisition }) {
                    failed += &format!("; the run could not record the failure: {journal}");
                }
                Err(failed)
            }
        }
    }

    fn failed(&mut self, step: &Step, error: String, failure: &mut Option<String>) {
        let recorded = self.record(&Event::StepFailed { step: step.name.clone(), error: error.clone() });
        failure.get_or_insert(format!("step `{}`: {error}", step.name));
        if let Err(e) = recorded {
            failure.get_or_insert(e);
        }
    }

    /// End the run: `ok` without `error`, else `stopped` when a signal or `--stop` stopped it, or
    /// `failed`.
    pub fn finish(mut self, error: Option<RunError>) -> Result<(), String> {
        let state = match &error {
            None => State::Ok,
            Some(_) if self.stopping() => State::Stopped,
            Some(_) => State::Failed,
        };
        let status = &mut self.status;
        if let Some(last) = status.phases.last_mut() {
            last.state = state;
        }
        let wall_ms = self.start.elapsed().as_millis() as u64;
        (status.state, status.wall_ms) = (state, Some(wall_ms));
        status.error = error.map(|mut error| {
            let built = status.steps.iter().filter(|step| !step.reused && step.wall_ms.is_some());
            error.kept = built.map(|step| step.step.clone()).collect();
            error
        });
        let line = match &status.error {
            None => format!("finished in {}", duration(wall_ms)),
            Some(error) => format!("{}: {}", if state == State::Stopped { "stopped" } else { "failed" }, error.message),
        };
        self.line(&line)?;
        self.sync()
    }
}

/// Set by SIGINT or SIGTERM.
static SIGNALLED: AtomicBool = AtomicBool::new(false);

/// Whether SIGINT or SIGTERM came: the run of this process stops before its next fetch, step or
/// phase.
pub fn signalled() -> bool {
    SIGNALLED.load(Ordering::Relaxed)
}

/// Stop the run of this process on SIGINT or SIGTERM, so that it ends as a failed run and frees
/// what it holds. A second signal ends the process at once.
pub fn stop_on_signals() {
    #[cfg(unix)]
    // SAFETY: the handler only stores atomics, and calls signal and raise, which are
    // async-signal-safe.
    unsafe {
        libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
    }
}

#[cfg(unix)]
extern "C" fn on_signal(signal: libc::c_int) {
    if SIGNALLED.swap(true, Ordering::Relaxed) {
        // SAFETY: see `stop_on_signals`.
        unsafe {
            libc::signal(signal, libc::SIG_DFL);
            libc::raise(signal);
        }
    }
    crate::fetch::http::stop();
}

fn panicked(payload: Box<dyn std::any::Any + Send>) -> String {
    let message = payload.downcast_ref::<&str>().map(|text| text.to_string());
    let message = message.or_else(|| payload.downcast_ref::<String>().cloned());
    format!("it panicked: {}", message.unwrap_or_default())
}

/// Whether `id` has the form of a run id: `YYYY-MM-DD-HHMMSS`, and `-N` for a later run in the
/// same second.
pub fn check_id(id: &str) -> Result<(), String> {
    let (time, suffix) = id.split_at(id.len().min(17));
    let time_ok = time.len() == 17
        && time.bytes().enumerate().all(|(i, b)| if [4, 7, 10].contains(&i) { b == b'-' } else { b.is_ascii_digit() });
    let digits = |n: &str| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit());
    if time_ok && (suffix.is_empty() || suffix.strip_prefix('-').is_some_and(digits)) {
        return Ok(());
    }
    Err(format!("`{id}` is not a run id: YYYY-MM-DD-HHMMSS"))
}

/// `runs/<id>/status.json`: a run as far as it went. The process of the run writes it. A reader
/// takes fields that it does not know, and gives a missing one its default, so a status of another
/// build of `obc data` reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct Status {
    pub id: String,
    /// The arguments after `obc data` that started the run.
    pub args: Vec<String>,
    /// The process of the run: while it starts, the process that reserved it, and then the
    /// process that it starts.
    pub pid: u32,
    pub state: State,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub started: String,
    /// `null` until it ends.
    pub wall_ms: Option<u64>,
    /// Each phase that it entered, in order.
    pub phases: Vec<PhaseState>,
    /// Each layer whose steps it bakes, in the order of the plan.
    pub layers: Vec<LayerProgress>,
    pub fetches: Vec<RunFetch>,
    /// In the order they started.
    pub steps: Vec<RunStep>,
    /// The releases that the run switched the target to.
    pub switched: Vec<String>,
    /// The size of the files of its fetches.
    pub bytes_fetched: u64,
    /// The size of the layers that it built; a reused layer is not counted.
    pub bytes_built: u64,
    /// `null` unless it failed or stopped.
    pub error: Option<RunError>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// Reserved: no process took it yet.
    #[default]
    Starting,
    Running,
    Ok,
    /// It failed, or its process ended before it finished.
    Failed,
    /// A signal or `obc data runs RUN --stop` stopped it.
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct PhaseState {
    pub phase: Phase,
    pub state: State,
    /// When it began, `YYYY-MM-DDTHH:MM:SSZ`.
    pub started: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct LayerProgress {
    pub layer: String,
    /// The steps of the layer that the run bakes, and those that finished.
    pub total: usize,
    pub done: usize,
    /// The leaves `<i>-<j>` of a tiled layer whose steps finished, and those whose steps bake now.
    pub leaves_done: Vec<String>,
    pub leaves_baking: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
pub struct RunError {
    /// The code of the [error](#errors).
    pub code: String,
    pub message: String,
    pub fix: String,
    /// The steps whose layers the run built: the store keeps them, and the next update reuses
    /// them.
    #[serde(default)]
    pub kept: Vec<String>,
}

impl RunError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, fix: impl Into<String>) -> Self {
        RunError { code: code.into(), message: message.into(), fix: fix.into(), kept: Vec::new() }
    }
}

/// The code of a run that failed without an error of its command.
pub(crate) const RUN_FAILED: &str = "run_failed";

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct RunFetch {
    pub source: String,
    pub version: String,
    pub resolved: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acquisition: Option<serde_json::Value>,
    pub params: Vec<(String, String)>,
    /// `None` while it runs, and when it failed.
    pub bytes: Option<u64>,
    pub wall_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default)]
pub struct RunStep {
    pub step: String,
    /// Empty until it finishes.
    pub layer: String,
    pub reused: bool,
    /// `null` while it runs, and when it failed.
    pub wall_ms: Option<u64>,
    pub peak_rss_bytes: Option<u64>,
    pub bytes_out: Option<u64>,
    /// The inputs that it read.
    pub reads: Vec<String>,
    pub error: Option<String>,
    /// The steps of this run that read it. `runs RUN` gives it; the status file has none.
    pub users: Vec<String>,
    /// Its wall time in the newest earlier run that built it. `runs RUN` gives it.
    pub last_wall_ms: Option<u64>,
}

impl Status {
    /// The fetch of `source@version` with `params`: a version fetched with two sets of params is
    /// two fetches.
    fn fetch(&mut self, source: String, version: String, params: Vec<(String, String)>) -> &mut RunFetch {
        let same = |fetch: &RunFetch| (&fetch.source, &fetch.version, &fetch.params) == (&source, &version, &params);
        let at = self.fetches.iter().position(same).unwrap_or_else(|| {
            self.fetches.push(RunFetch { source, version, params, ..RunFetch::default() });
            self.fetches.len() - 1
        });
        &mut self.fetches[at]
    }

    fn step(&mut self, name: &str) -> &mut RunStep {
        let at = self.steps.iter().position(|step| step.step == name).unwrap_or_else(|| {
            self.steps.push(RunStep { step: name.into(), ..RunStep::default() });
            self.steps.len() - 1
        });
        &mut self.steps[at]
    }
}

/// A run, as `obc data runs` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub struct Summary {
    pub id: String,
    /// The arguments after `obc data` that started the run.
    pub args: Vec<String>,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub started: String,
    pub state: State,
    /// `null` until it ends.
    pub wall_ms: Option<u64>,
    pub bytes_fetched: u64,
    pub bytes_built: u64,
}

/// A run as `obc data runs RUN` gives it: its status, with the users and the earlier times of its
/// steps, and the end of its log.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct View {
    #[serde(flatten)]
    pub status: Status,
    /// The last lines of `runs/<id>/log`.
    pub log: Vec<String>,
}

/// How many lines of the log a [`View`] gives.
const TAIL: usize = 50;

/// The progress of each layer in the status file `file` of a run; none when it cannot be read.
pub fn progress(file: &Path) -> Vec<crate::lock::Progress> {
    let status = fs::read(file).ok().and_then(|bytes| serde_json::from_slice::<Status>(&bytes).ok());
    let layers = status.map(|status| status.layers).unwrap_or_default();
    layers
        .into_iter()
        .map(|layer| crate::lock::Progress { layer: layer.layer, done: layer.done, total: layer.total })
        .collect()
}

/// The status of run `id`. A run whose process ended before it finished has failed.
pub fn status(store: &Store, id: &str) -> Result<Status, String> {
    check_id(id)?;
    let dir = store.run(id);
    if !dir.is_dir() {
        return Err(format!("no run `{id}`"));
    }
    // The lock before the status: a run that ends after this has written its last status.
    let running = store.is_locked(&format!("run-{id}"))?;
    let read = fs::read(dir.join(STATUS)).map_err(|e| e.to_string());
    let mut status: Status = match read.and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string())) {
        Ok(status) => status,
        // Its process is not known (pid 0): `stop` signals no process of it.
        Err(e) => {
            let message = format!("run {id}: {STATUS} cannot be read: {e}; its log tells why");
            let fix = "Read its log: `obc data runs RUN`.";
            return Ok(Status {
                id: id.into(),
                state: if running { State::Running } else { State::Failed },
                error: (!running).then(|| RunError::new(RUN_FAILED, message, fix)),
                ..Status::default()
            });
        }
    };
    let died = match status.state {
        State::Running => Some("its process ended before it finished"),
        State::Starting if !alive(status.pid) => Some("its process never started"),
        _ => None,
    };
    if let Some(message) = died.filter(|_| !running) {
        status.state = State::Failed;
        status.error.get_or_insert_with(|| RunError::new(RUN_FAILED, message, "Run its command again."));
    }
    Ok(status)
}

/// Whether the process `pid` runs.
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    if let Ok(pid @ 1..) = libc::pid_t::try_from(pid) {
        // SAFETY: signal 0 only checks that the process exists.
        let found = unsafe { libc::kill(pid, 0) } == 0;
        return found || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
    }
    false
}

/// Every run in the store, newest first.
pub fn list(store: &Store) -> Result<Vec<Summary>, String> {
    let summary = |status: Status| Summary {
        id: status.id,
        args: status.args,
        started: status.started,
        state: status.state,
        wall_ms: status.wall_ms,
        bytes_fetched: status.bytes_fetched,
        bytes_built: status.bytes_built,
    };
    Ok(all(store)?.into_iter().rev().map(summary).collect())
}

/// Run `id`, with the users of each step, its time in an earlier run, and the end of its log.
pub fn view(store: &Store, id: &str) -> Result<View, String> {
    let runs = all(store)?;
    let at = runs.iter().position(|run| run.id == id).ok_or_else(|| format!("no run `{id}`"))?;
    let mut status = runs[at].clone();
    let names: HashSet<&str> = runs[at].steps.iter().map(|step| step.step.as_str()).collect();
    let mut users: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for step in &runs[at].steps {
        for read in step.reads.iter().filter(|read| names.contains(read.as_str())) {
            users.entry(read).or_default().push(step.step.clone());
        }
    }
    for step in &mut status.steps {
        step.users = users.get(step.step.as_str()).cloned().unwrap_or_default();
        step.last_wall_ms = runs[..at]
            .iter()
            .rev()
            .find_map(|run| run.steps.iter().find(|earlier| earlier.step == step.step && !earlier.reused)?.wall_ms);
    }
    Ok(View { status, log: tail(&store.run(id).join(LOG)) })
}

/// The last [`TAIL`] lines of the log at `path`, from its last bytes only. The output of a step
/// can be any bytes.
fn tail(path: &Path) -> Vec<String> {
    let Ok(mut file) = File::open(path) else { return Vec::new() };
    let length = file.seek(SeekFrom::End(0)).unwrap_or(0);
    let from = length.saturating_sub(TAIL as u64 * 400);
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(from)).and_then(|_| file.read_to_end(&mut bytes)).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    // A read from the middle of the file starts in the middle of a line.
    let lines: Vec<&str> = text.lines().skip(usize::from(from > 0)).collect();
    lines[lines.len().saturating_sub(TAIL)..].iter().map(|line| line.to_string()).collect()
}

/// Every run, oldest first.
fn all(store: &Store) -> Result<Vec<Status>, String> {
    let dir = store.root().join("runs");
    let mut ids: Vec<String> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|id| check_id(id).is_ok())
            .collect(),
        Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    ids.sort_by_key(|id| {
        let (time, suffix) = id.split_at(17);
        (time.to_string(), suffix.trim_start_matches('-').parse::<u32>().unwrap_or(1))
    });
    ids.iter().map(|id| status(store, id)).collect()
}

/// Follow run `id` until it ends, or until SIGINT or SIGTERM come: give `each` the lines that its
/// log gets and its status, when one of them changed. The status when it ended, or `None` after a
/// signal: the run continues.
pub fn follow(
    store: &Store,
    id: &str,
    mut each: impl FnMut(&str, &Status) -> Result<(), String>,
) -> Result<Option<Status>, String> {
    let path = store.run(id).join(LOG);
    let mut log = File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut last: Option<Status> = None;
    loop {
        let running = store.is_locked(&format!("run-{id}"))?;
        let mut bytes = Vec::new();
        log.read_to_end(&mut bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        // A part of a line waits for its end.
        let complete = bytes.iter().rposition(|byte| *byte == b'\n').map_or(0, |end| end + 1);
        log.seek(SeekFrom::Current(complete as i64 - bytes.len() as i64)).map_err(|e| e.to_string())?;
        let status = status(store, id)?;
        if complete > 0 || last.as_ref() != Some(&status) {
            each(&String::from_utf8_lossy(&bytes[..complete]), &status)?;
        }
        if !running && status.state != State::Starting {
            return Ok(Some(status));
        }
        last = Some(status);
        if signalled() {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The variable that names the run of a process that [`detach`] started.
pub const RUN: &str = "OBC_DATA_RUN";

/// Start the command `args`, the arguments after `obc data`, as a run in a process of its own: a
/// session without a terminal, so closing the terminal does not stop it. It writes its output to
/// its log. Gives the id of the run once its process wrote its status.
pub fn detach(store: &Store, args: &[String]) -> Result<String, String> {
    let id = reserve(store, args)?;
    let dir = store.run(&id);
    let log = OpenOptions::new().create(true).append(true).open(dir.join(LOG));
    let log = log.map_err(|e| format!("{}: {e}", dir.join(LOG).display()))?;
    // On Linux, the image of this process, also when its file was replaced since: the run does the
    // review that this process made.
    #[cfg(target_os = "linux")]
    let program = PathBuf::from("/proc/self/exe");
    #[cfg(not(target_os = "linux"))]
    let program = std::env::current_exe().map_err(|e| format!("the program of `obc data`: {e}"))?;
    let mut command = std::process::Command::new(program);
    command.args(args).env(RUN, &id).stdin(std::process::Stdio::null());
    command.stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    #[cfg(unix)]
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut command, || match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
    // The lock of the run keeps its process waiting until its status names it.
    let lock = store.lock(&format!("run-{id}"))?;
    let starting = |pid: u32, error: Option<String>| -> Result<(), String> {
        let reserved = status(store, &id)?;
        // A status that left `starting` is the one of the run.
        if reserved.state != State::Starting {
            return Ok(());
        }
        let failed =
            error.map(|message| RunError::new(RUN_FAILED, message, format!("Read its log: `obc data runs {id}`.")));
        let state = if failed.is_some() { State::Failed } else { State::Starting };
        write_status(&dir, &Status { pid, state, error: failed, ..reserved })
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            starting(0, Some(format!("its process did not start: {e}")))?;
            return Err(format!("run {id} did not start: {e}"));
        }
    };
    starting(child.id(), None)?;
    drop(lock);
    loop {
        if status(store, &id)?.state != State::Starting {
            return Ok(id);
        }
        if let Some(exit) = child.try_wait().map_err(|e| format!("run {id}: {e}"))? {
            if status(store, &id)?.state != State::Starting {
                return Ok(id);
            }
            starting(child.id(), Some(format!("its process ended before it started the run ({exit})")))?;
            return Err(format!("run {id} did not start ({exit}); `obc data runs {id}` shows its log"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop run `id`: write its stop file, and send SIGTERM to the process group of its process, once.
/// It frees what it holds, and keeps what it baked. A run that starts, or whose process is not
/// known because its status cannot be read, gets only the stop file.
pub fn stop(store: &Store, id: &str) -> Result<Status, String> {
    let status = status(store, id)?;
    if !matches!(status.state, State::Starting | State::Running) {
        return Err(format!("run {id} does not run"));
    }
    // The process of a run that starts can be the one that reserved it.
    let signal = status.state == State::Running;
    let path = store.run(id).join(STOP);
    // A second signal would end the process at once, and leave its lock held.
    if path.exists() {
        return Ok(status);
    }
    fs::write(&path, "").map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    if let Some(pid @ 1..) = libc::pid_t::try_from(status.pid).ok().filter(|_| signal) {
        // SAFETY: kill sends a signal. The lock of the run says that its process still runs. A
        // detached run leads its process group; a run in a terminal may not.
        unsafe {
            if libc::kill(-pid, libc::SIGTERM) != 0 {
                libc::kill(pid, libc::SIGTERM);
            }
        }
    }
    Ok(status)
}

/// `SOURCE@VERSION NAME=VALUE…`
pub fn fetched(source: &str, version: &str, params: &[(String, String)]) -> String {
    let params = params.iter().map(|(name, value)| format!(" {name}={value}"));
    format!("{source}@{version}{}", params.collect::<String>())
}

pub fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0 => format!("{ms} ms"),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

pub fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} kB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::Estimate;
    use crate::engine::tests::{fixture, pipeline, snapshot, step, summary, write, JOIN};
    use crate::engine::{Input, Request, Run as StepRun};
    use crate::store::Requested;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The log of run `id`, without the time of each line.
    fn logged(store: &Store, id: &str) -> Vec<String> {
        let text = std::fs::read_to_string(store.run(id).join(LOG)).unwrap();
        text.lines().map(|line| line[9..].to_string()).collect()
    }

    #[test]
    fn a_run_records_its_status_and_log_and_a_later_run_resumes_after_a_failed_step() {
        let fixture = fixture("runs-resume");
        let join = fixture.root().join("join.py");
        std::fs::write(&join, "raise SystemExit(3)\n").unwrap();
        let err = fixture.build(&pipeline()).unwrap_err();
        assert!(err.starts_with("step `test/join`: `python3 join.py` failed"), "{err}");
        let failed = list(&fixture.store).unwrap().remove(0);
        let lines = logged(&fixture.store, &failed.id);
        assert_eq!(lines[..2], ["started `obc data build test`", "test/upper started"]);
        assert!(lines[2].starts_with("test/upper built in ") && lines[2].ends_with(", 5 B"), "{lines:?}");
        assert!(lines[4].starts_with("test/join failed: "), "{lines:?}");
        let status = status(&fixture.store, &failed.id).unwrap();
        let progress = (&status.layers[0].layer, status.layers[0].done, status.layers[0].total);
        assert_eq!(progress, (&"test".to_string(), 1, 3), "test/upper of three steps");
        assert_eq!(status.error.unwrap().kept, ["test/upper"], "the store keeps what it built");

        std::fs::write(&join, JOIN).unwrap();
        let built = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&built), [("test/join", false), ("test/count", false)], "test/upper is in the store");
        let mut steps = pipeline();
        steps[1].options = serde_json::json!({"note": "the same bytes"});
        fixture.build(&steps).unwrap();

        let runs = list(&fixture.store).unwrap();
        let states: Vec<_> = runs.iter().map(|run| (run.args.join(" "), run.state)).collect();
        let build = || "build test".to_string();
        assert_eq!(states, [(build(), State::Ok), (build(), State::Ok), (build(), State::Failed)]);
        assert_eq!(runs[2], failed);
        assert_eq!(runs[1].bytes_built, 10 + 1, "joined.txt and count/lines.txt");
        assert_eq!(runs[0].bytes_built, 10, "test/count is reused");

        let resumed = view(&fixture.store, &runs[1].id).unwrap().status;
        let steps: Vec<(&str, &[String])> =
            resumed.steps.iter().map(|s| (s.step.as_str(), s.users.as_slice())).collect();
        assert_eq!(steps, [("test/join", &["test/count".to_string()][..]), ("test/count", &[][..])]);
        let last = view(&fixture.store, &runs[0].id).unwrap();
        assert_eq!(last.status.steps[0].last_wall_ms, resumed.steps[0].wall_ms);
        assert_eq!(last.log.last().map(|line| &line[9..18]), Some("finished "));
        let error = view(&fixture.store, &failed.id).unwrap().status.steps[1].error.clone();
        assert_eq!(error.as_deref(), Some(&err["step `test/join`: ".len()..]));
    }

    fn named(request: &Request) -> Result<(), String> {
        std::fs::write(request.output.join("out.txt"), &request.step).map_err(|e| e.to_string())
    }

    #[test]
    fn a_run_shows_the_leaves_of_a_tiled_layer_that_bake_and_that_finished() {
        static TILED: crate::engine::Layer = crate::engine::Layer { shape: Shape::Tiled, ..crate::engine::tests::TEST };
        let fixture = fixture("runs-leaves");
        let mut steps: Vec<Step> = ["test/0001-0002", "test/0001-0003"]
            .map(|name| step(name, vec![snapshot("head", "1", &[])], "out.txt", StepRun::Rust(named)))
            .into();
        steps.iter_mut().for_each(|step| step.layer = &TILED);
        let plan = fixture.plan(&steps).unwrap();
        let mut run = Run::create(&fixture.store, ["build", "test"]).unwrap();
        run.plan_layers(&plan, &steps.iter().collect::<Vec<_>>()).unwrap();
        for step in ["test/0001-0002", "test/0001-0003"] {
            run.record(&Event::StepStarted { step: step.into() }).unwrap();
        }
        run.record(&Event::StepFailed { step: "test/0001-0002".into(), error: "no".into() }).unwrap();
        run.flush().unwrap();
        let baking = &status(&fixture.store, run.id()).unwrap().layers[0];
        assert_eq!((baking.done, baking.total, &baking.leaves_baking[..]), (0, 2, &["0001-0003".to_string()][..]));

        fixture.run(&steps, &plan, Limits::machine()).unwrap();
        let built = &status(&fixture.store, &list(&fixture.store).unwrap()[0].id).unwrap().layers[0];
        assert_eq!(
            (built.done, &built.leaves_done[..], built.leaves_baking.len()),
            (2, &["0001-0002".to_string(), "0001-0003".to_string()][..], 0)
        );
    }

    #[test]
    fn a_stopped_run_stops_before_its_next_step_and_ends_as_stopped() {
        let fixture = fixture("runs-stop");
        let run = Run::create(&fixture.store, ["update", "local"]).unwrap();
        let id = run.id().to_string();
        assert!(run.check_stop(&fixture.store).is_ok());
        std::fs::write(fixture.store.run(&id).join(STOP), "").unwrap();
        let stopped = run.check_stop(&fixture.store).unwrap_err();
        assert!(stopped.contains("--stop"), "{stopped}");
        run.finish(Some(RunError::new(RUN_FAILED, stopped, "Run it again."))).unwrap();
        let status = status(&fixture.store, &id).unwrap();
        assert_eq!(status.state, State::Stopped);
        assert!(stop(&fixture.store, &id).unwrap_err().contains("does not run"));
    }

    #[test]
    fn a_reserved_run_shows_as_starting_and_a_stop_signals_no_process_of_it() {
        let fixture = fixture("runs-reserved");
        let id = reserve(&fixture.store, &["update".into(), "local".into()]).unwrap();
        assert_eq!(list(&fixture.store).unwrap()[0].state, State::Starting);
        // Its process is the one that reserved it, this test: only the stop file stops the run.
        stop(&fixture.store, &id).unwrap();
        let run = Run::attach(&fixture.store, &id).unwrap();
        assert_eq!(status(&fixture.store, &id).unwrap().args, ["update", "local"]);
        assert!(run.check_stop(&fixture.store).is_err());

        let gone = reserve(&fixture.store, &[]).unwrap();
        let mut exited = std::process::Command::new("true").spawn().unwrap();
        exited.wait().unwrap();
        let status = Status { pid: exited.id(), ..status(&fixture.store, &gone).unwrap() };
        write_status(&fixture.store.run(&gone), &status).unwrap();
        let never = super::status(&fixture.store, &gone).unwrap();
        assert_eq!((never.state, never.error.unwrap().message.as_str()), (State::Failed, "its process never started"));
    }

    #[cfg(unix)]
    #[test]
    fn a_stop_sends_sigterm_to_the_process_group_of_the_run() {
        use std::os::unix::process::{CommandExt, ExitStatusExt};
        let fixture = fixture("runs-stop-signal");
        let mut sleep = std::process::Command::new("sleep");
        sleep.arg("30");
        // SAFETY: setsid is async-signal-safe.
        unsafe { sleep.pre_exec(|| (libc::setsid() != -1).then_some(()).ok_or_else(std::io::Error::last_os_error)) };
        let mut child = sleep.spawn().unwrap();
        let mut run = Run::create(&fixture.store, ["update", "local"]).unwrap();
        run.status.pid = child.id();
        run.save().unwrap();
        stop(&fixture.store, run.id()).unwrap();
        assert_eq!(child.wait().unwrap().signal(), Some(libc::SIGTERM));
        assert!(run.check_stop(&fixture.store).is_err(), "the stop file stops it");
    }

    #[test]
    fn a_run_keeps_old_inputs_and_outputs_until_it_finishes() {
        use crate::store::gc;

        let fixture = fixture("run-gc-phases");
        let steps = [step(
            "test/upper",
            vec![snapshot("head", "1", &["head.txt"])],
            "upper.txt",
            StepRun::Rust(crate::engine::tests::upper),
        )];
        let plan = fixture.plan(&steps).unwrap();
        let mut run = Run::create(&fixture.store, ["apply", "test"]).unwrap();
        let (root, http) = (fixture.root(), Http::new());
        let context = Context {
            store: &fixture.store,
            root: &root,
            sources: &[],
            http: &http,
            kept: &crate::engine::plan::Kept::new(),
            limits: Limits { jobs: 1, memory_bytes: None },
        };
        let built = run.build(&context, &steps, &plan).unwrap();
        let output = &built[0].receipt.files[0];
        fixture.fetched_version("head", "2", "head.txt", b"newer head\n");
        let mut newest = fixture.store.snapshot("head", "2").unwrap().unwrap();
        newest.files.iter_mut().for_each(|file| file.retrieved = "2026-10-06T00:00:00Z".into());
        fixture.store.put_snapshot(&newest).unwrap();
        let roots = gc::Roots::default();
        let cleanup = gc::plan(&fixture.store, &roots).unwrap();
        assert!(cleanup.snapshots.contains(&"head@1".into()));
        assert!(cleanup.objects.iter().any(|(sha256, _)| sha256 == &output.sha256));

        run.record(&Event::Phase { phase: Phase::Verify }).unwrap();
        assert!(gc::apply(&fixture.store, &roots, &cleanup).unwrap().is_none());
        assert!(fixture.store.snapshot("head", "1").unwrap().is_some());
        let object = fixture.store.object(&output.sha256);
        assert_eq!(std::fs::read(&object).unwrap(), b"HEAD\n");
        run.record(&Event::Phase { phase: Phase::Upload }).unwrap();
        assert!(gc::apply(&fixture.store, &roots, &cleanup).unwrap().is_none());

        let id = run.id().to_string();
        run.finish(None).unwrap();
        let read = status(&fixture.store, &id).unwrap();
        assert_eq!(read.state, State::Ok);
        let phases: Vec<_> = read.phases.iter().map(|phase| (phase.phase, phase.state)).collect();
        assert_eq!(phases, [(Phase::Verify, State::Ok), (Phase::Upload, State::Ok)]);
        assert!(gc::apply(&fixture.store, &roots, &cleanup).unwrap().is_some());
        assert!(fixture.store.snapshot("head", "1").unwrap().is_none());
        assert!(!object.exists());
    }

    #[test]
    fn a_run_whose_process_ends_before_it_finishes_has_failed() {
        let fixture = fixture("runs-interrupted");
        let run = Run::create(&fixture.store, ["build", "test"]).unwrap();
        assert_eq!(list(&fixture.store).unwrap()[0].state, State::Running);
        let id = run.id().to_string();
        drop(run);
        assert_eq!(list(&fixture.store).unwrap()[0].state, State::Failed);
        let ended = follow(&fixture.store, &id, |_, _| Ok(())).unwrap().unwrap();
        assert_eq!(ended.error.unwrap().message, "its process ended before it finished");

        std::fs::create_dir(fixture.store.run("2026-01-01-000000")).unwrap();
        let empty = list(&fixture.store).unwrap().pop().unwrap();
        assert_eq!((empty.id.as_str(), empty.state), ("2026-01-01-000000", State::Failed));
        assert_eq!(check_id("../x").unwrap_err(), "`../x` is not a run id: YYYY-MM-DD-HHMMSS");
        assert!(status(&fixture.store, "2026-01-01-000000-x").is_err());
    }

    #[test]
    fn a_run_refuses_an_outdated_plan() {
        let fixture = fixture("runs-outdated");
        let plan = fixture.plan(&pipeline()).unwrap();
        let mut steps = pipeline();
        steps[2].options = serde_json::json!({"case": "upper"});
        let err = fixture.run(&steps, &plan, Limits::machine()).unwrap_err();
        assert_eq!(err, "step `test/upper`: the plan is outdated; plan again");
        let none = Limits { jobs: 0, memory_bytes: None };
        assert!(fixture.run(&pipeline(), &plan, none).unwrap_err().contains("the limit of jobs is 0"));
    }

    const SLEEP: &str = "import json, os, sys, time
request = json.load(sys.stdin)
start = time.time()
time.sleep(0.3)
open(os.path.join(request['output'], 'out.txt'), 'w').write(f'{start} {time.time()}')
";

    /// Whether two independent command steps with these estimated peaks ran at the same time.
    fn together(name: &str, peaks: [Option<u64>; 2], memory_bytes: u64) -> bool {
        let fixture = fixture(name);
        write(&fixture.root().join("sleep.py"), SLEEP);
        let command = || StepRun::Command(vec!["python3".into(), "sleep.py".into()]);
        let steps: Vec<Step> =
            ["test/a", "test/b"].map(|name| step(name, vec![snapshot("head", "1", &[])], "out.txt", command())).into();
        let mut plan = fixture.plan(&steps).unwrap();
        for (build, peak) in plan.groups.iter_mut().flat_map(|group| &mut group.builds).zip(peaks) {
            build.estimate = Some(Estimate { wall_ms: 0, bytes_out: 0, client_bytes: 0, peak_rss_bytes: peak });
        }
        let built = fixture.run(&steps, &plan, Limits { jobs: 4, memory_bytes: Some(memory_bytes) }).unwrap();
        let spans: Vec<(f64, f64)> = built
            .iter()
            .map(|built| {
                let text = std::fs::read_to_string(fixture.store.object(&built.receipt.files[0].sha256)).unwrap();
                let (start, end) = text.split_once(' ').unwrap();
                (start.parse().unwrap(), end.parse().unwrap())
            })
            .collect();
        spans[0].0 < spans[1].1 && spans[1].0 < spans[0].1
    }

    #[test]
    fn the_steps_that_run_together_fit_in_the_memory_budget() {
        assert!(together("runs-memory-fits", [Some(600), Some(600)], 1200));
        assert!(!together("runs-memory-over", [Some(600), Some(600)], 1000));
        assert!(!together("runs-memory-alone", [Some(1500), Some(100)], 1000), "a step over the budget runs alone");
        assert!(!together("runs-memory-unknown", [None, Some(100)], 1000), "a step with no estimate runs alone");
    }

    static RUNNING: AtomicUsize = AtomicUsize::new(0);
    static MOST: AtomicUsize = AtomicUsize::new(0);

    fn tracked(request: &Request) -> Result<(), String> {
        let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
        MOST.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        std::fs::write(request.output.join("out.txt"), &request.step).map_err(|e| e.to_string())
    }

    #[test]
    fn independent_rust_steps_run_in_parallel() {
        let fixture = fixture("runs-in-process");
        let steps: Vec<Step> = ["test/a", "test/b"]
            .map(|name| step(name, vec![snapshot("head", "1", &[])], "out.txt", StepRun::Rust(tracked)))
            .into();
        let plan = fixture.plan(&steps).unwrap();
        fixture.run(&steps, &plan, Limits { jobs: 4, memory_bytes: None }).unwrap();
        assert_eq!(MOST.load(Ordering::SeqCst), 2);
    }

    fn first_file(request: &Request) -> Result<(), String> {
        let file = request.snapshots["land"].values().next().ok_or("no file")?;
        std::fs::copy(file, request.output.join("copy.bin")).map(|_| ()).map_err(|e| e.to_string())
    }

    #[test]
    fn a_run_fetches_the_files_that_the_params_of_an_input_give() {
        let (url, log) = crate::fetch::tests::serve(|_, _| crate::fetch::tests::whole(b"tile a"));
        let land = crate::fetch::tests::source(&url.replace("file.bin", "{tile}-{version}.bin"), "release");
        let fixture = fixture("runs-fetch");
        fixture.with_sources(std::slice::from_ref(&land));
        let tile = Input::Snapshot {
            source: "land".into(),
            version: "v1".into(),
            params: vec![("tile".into(), "a".into())],
            files: Vec::new(),
        };
        let mut steps = [step("test/copy", vec![tile], "copy.bin", StepRun::Rust(first_file))];
        let plan = fixture.plan(&steps).unwrap();
        let fetch = &plan.groups[0].fetches[0];
        assert_eq!(
            (fetch.files.len(), plan.groups[0].builds[0].key.as_deref()),
            (0, None),
            "the store cannot name them"
        );

        let mut run = Run::create(&fixture.store, ["build", "test"]).unwrap();
        let sources = [land];
        let http = crate::fetch::tests::quick();
        let context = Context {
            store: &fixture.store,
            root: &fixture.root(),
            sources: &sources,
            http: &http,
            kept: &crate::engine::plan::Kept::new(),
            limits: Limits::machine(),
        };
        let built = run.build(&context, &steps, &plan).unwrap();
        run.finish(None).unwrap();
        assert_eq!(
            built[0].receipt.inputs[0].digest,
            crate::engine::digest([("a-v1.bin", sha256_of(b"tile a").as_str())])
        );
        let runs = list(&fixture.store).unwrap();
        let lines = logged(&fixture.store, &runs[0].id);
        assert_eq!(lines[1], "land@v1 tile=a fetch started");
        assert!(lines[2].starts_with("land@v1 tile=a fetched v1 in ") && lines[2].ends_with(", 6 B"), "{lines:?}");
        assert_eq!(runs[0].bytes_fetched, 6);
        assert_eq!(fixture.plan(&steps).unwrap().builds().count(), 0, "the store knows the files of the params");
        assert_eq!(log.lock().unwrap().len(), 1);

        // A fetch that gave no file: its input reads no file, not every file of the version.
        let none = Requested { version: "v1".into(), params: vec![("tile".into(), "z".into())], files: Vec::new() };
        fixture.store.put_requested("land", &none).unwrap();
        let Input::Snapshot { params, .. } = &mut steps[0].inputs[0] else { unreachable!() };
        *params = none.params.clone();
        let plan = fixture.plan(&steps).unwrap();
        assert_eq!((plan.groups[0].fetches.len(), plan.groups[0].builds[0].key.is_some()), (0, true));
    }

    fn sha256_of(bytes: &[u8]) -> String {
        crate::store::sha256_hex(bytes)
    }
}
