//! Runs: a run fetches and builds a plan, as many steps at a time as the machine allows, and
//! writes its events as JSON lines to the store.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::plan::Plan;
use super::{build_step, order, prepare, reusable, Built, Codes, InputKind, Receipt, Run as StepRun, Step};
use crate::date;
use crate::fetch::{self, http::Http};
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
    /// The repository root: code paths are relative to it, and a command starts in it.
    pub root: &'a Path,
    /// The sources of `data/sources.toml`: a fetch finds its source here.
    pub sources: &'a [Source],
    pub http: &'a Http,
    pub copies: Option<&'a crate::input_copy::Restore<'a>>,
    pub limits: Limits,
}

/// One line of `runs/<id>.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Started {
        command: String,
        /// `YYYY-MM-DDTHH:MM:SSZ`
        at: String,
    },
    Phase {
        phase: Phase,
    },
    Published {
        mutation: Publication,
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
        /// The size of the files that the fetch gave, downloaded or found in the store.
        bytes: u64,
        wall_ms: u64,
    },
    FetchFailed {
        source: String,
        version: String,
        params: Vec<(String, String)>,
        error: String,
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
    Finished {
        ok: bool,
        error: Option<String>,
        wall_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepare,
    Build,
    Verify,
    Upload,
    Switch,
    Wait,
    Cleanup,
}

/// A remote write that acknowledged success. Verification can still fail afterward.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Publication {
    Uploaded { key: String },
    Switched { product: String, release: String },
    Removed { key: String, bytes: u64 },
}

/// A run that this process writes. The process that holds the lock of a run is the process that
/// runs it, so a reader knows that a run without `finished` still runs.
/// Its store stays in use through verification and publication, until finish or drop.
pub struct Run {
    id: String,
    file: File,
    start: Instant,
    _lock: Lock,
    _using: Lock,
    codes: super::code::Context,
    pub(crate) originals: BTreeMap<String, super::release::Layer>,
}

impl Run {
    /// Start a run with the id `YYYY-MM-DD-HHMMSS`, and a suffix `-N` when that id is taken.
    pub fn create(store: &Store, command: &str) -> Result<Run, String> {
        let using = store.using()?;
        let at = date::timestamp(date::now());
        let base = format!("{}-{}", &at[..10], at[11..19].replace(':', ""));
        for n in 1.. {
            let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
            let Some(lock) = store.try_lock(&format!("run-{id}"))? else { continue };
            let path = store.run(&id);
            crate::store::durable_directory(path.parent().expect("a run file has a directory"))?;
            let file = match OpenOptions::new().append(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            let mut run = Run {
                id,
                file,
                start: Instant::now(),
                _lock: lock,
                _using: using,
                codes: Default::default(),
                originals: BTreeMap::new(),
            };
            run.record(&Event::Started { command: command.into(), at })?;
            run.sync()?;
            File::open(path.parent().unwrap()).and_then(|directory| directory.sync_all()).map_err(|e| e.to_string())?;
            return Ok(run);
        }
        unreachable!("the ids never run out")
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn record(&mut self, event: &Event) -> Result<(), String> {
        let mut line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        line.push('\n');
        self.file.write_all(line.as_bytes()).map_err(|e| format!("run {}: {e}", self.id))
    }

    pub fn check_stop(&self, store: &Store) -> Result<(), String> {
        crate::store::check_free(store.root(), 0)?;
        if crate::operation::stopped(store, self.id())? {
            return Err("stopped after the current work".into());
        }
        Ok(())
    }

    /// Flush the journal.
    pub fn sync(&self) -> Result<(), String> {
        self.file.sync_all().map_err(|e| format!("run {}: {e}", self.id))
    }

    /// Continue the unfinished run `id` in this process: the detached worker of an operation.
    pub fn attach(store: &Store, id: &str, prefix: &[Event]) -> Result<Self, String> {
        check_id(id)?;
        if !matches!(prefix.first(), Some(Event::Started { .. }))
            || prefix.iter().any(|event| matches!(event, Event::Finished { .. } | Event::Published { .. }))
        {
            return Err("the run is not unfinished".into());
        }
        let using = store.using()?;
        let lock = store.try_lock(&format!("run-{id}"))?.ok_or("the originating run still has an owner")?;
        let path = store.run(id);
        crate::store::durable_directory(path.parent().unwrap())?;
        if path.exists() {
            let existing = events(store, id)?;
            if !existing.starts_with(prefix) || existing.iter().any(|event| matches!(event, Event::Finished { .. })) {
                return Err("the run journal differs from its start".into());
            }
        }
        let file = OpenOptions::new().append(true).create(true).open(&path).map_err(|e| e.to_string())?;
        let elapsed = match prefix.first() {
            Some(Event::Started { at, .. }) => {
                date::seconds(at).map(|at| Duration::from_secs(date::now().saturating_sub(at)))
            }
            _ => None,
        };
        let start = elapsed.and_then(|elapsed| Instant::now().checked_sub(elapsed)).unwrap_or_else(Instant::now);
        let mut run = Self {
            id: id.into(),
            file,
            start,
            _lock: lock,
            _using: using,
            codes: Default::default(),
            originals: BTreeMap::new(),
        };
        if run.file.metadata().map_err(|e| e.to_string())?.len() == 0 {
            for event in prefix {
                run.record(event)?;
            }
        }
        run.sync()?;
        File::open(path.parent().unwrap()).and_then(|directory| directory.sync_all()).map_err(|e| e.to_string())?;
        Ok(run)
    }

    /// Use only original layers returned by checked portable reuse for this operation.
    pub fn reuse_layers(&mut self, layers: &BTreeMap<String, super::release::Layer>) {
        self.originals = layers.clone();
    }

    /// Fetch the fetches of `plan` one after another, then build its builds and reuse the layers
    /// that they read. After a fetch or a step fails, no other step starts; the steps that run
    /// finish. A later run reuses every layer that this one built.
    pub fn build(&mut self, context: &Context, steps: &[Step], plan: &Plan) -> Result<Vec<Built>, String> {
        let Context { store, root, limits, .. } = *context;
        self.check_stop(store)?;
        crate::worker::check(root)?;
        let _using = store.using()?;
        if limits.jobs == 0 {
            return Err("the limit of jobs is 0; it must be 1 or more".into());
        }
        let estimated = plan
            .builds()
            .filter_map(|build| build.estimate)
            .fold(0u64, |total, estimate| total.saturating_add(estimate.bytes_out));
        let estimated = plan.fetches().iter().filter_map(|fetch| fetch.bytes).fold(estimated, u64::saturating_add);
        crate::store::check_free(store.root(), estimated)?;
        let ordered = order(steps)?;
        if let Some(build) = plan.builds().find(|build| !ordered.iter().any(|step| step.name == build.step)) {
            return Err(format!("the plan builds `{}`, which no step makes", build.step));
        }
        self.fetch(context, plan)?;
        crate::worker::check(root)?;

        // A step whose peak is not known reserves the whole memory, so it runs alone.
        let cost = |peak: Option<u64>| limits.memory_bytes.map_or(0, |memory| peak.unwrap_or(memory));
        let planned: HashMap<&str, (u64, Option<&str>)> = plan
            .builds()
            .map(|build| {
                let peak = build.estimate.and_then(|estimate| estimate.peak_rss_bytes);
                (build.step.as_str(), (cost(peak), build.key.as_deref()))
            })
            .collect();
        // The planned steps and the layers that they read, in dependency order.
        let mut needed: HashSet<&str> = planned.keys().copied().collect();
        for step in ordered.iter().rev() {
            if needed.contains(step.name.as_str()) && !self.originals.contains_key(&step.name) {
                needed.extend(step.layers());
            }
        }
        let position: HashMap<&str, usize> =
            ordered.iter().enumerate().map(|(i, step)| (step.name.as_str(), i)).collect();
        let mut pending: Vec<&Step> = ordered.into_iter().filter(|step| needed.contains(step.name.as_str())).collect();
        let mut codes = Codes { context: std::mem::take(&mut self.codes), ..Default::default() };
        for step in pending.iter().filter(|step| !self.originals.contains_key(&step.name)) {
            let (hash, files) = codes.get(root, &step.code).map_err(|e| format!("step `{}`: {e}", step.name))?;
            store.put_code(hash, &files.files)?;
            store.put_producer(hash, &super::release::Producer::from(files))?;
        }
        crate::worker::check(root)?;
        let checks = std::sync::Mutex::new(std::mem::take(&mut codes.context));

        let outdated = "the plan is outdated; plan again";
        let mut done: HashMap<&str, super::release::Layer> = HashMap::new();
        let mut built = Vec::new();
        let mut failure: Option<String> = None;
        let (mut running, mut reserved, mut rust_running) = (0, 0, false);
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
                if let Some(original) = self.originals.get(&step.name) {
                    pending.remove(i);
                    done.insert(&step.name, original.clone());
                    continue;
                }
                if !step.layers().all(|name| done.contains_key(name)) {
                    i += 1;
                    continue;
                }
                let code = &codes.get(root, &step.code).expect("hashed above").0;
                let Some(&(cost, key)) = planned.get(step.name.as_str()) else {
                    // A layer that a planned step reads: the store has it, unless the plan is old.
                    pending.remove(i);
                    let reused = prepare(store, step, &done, code)
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
                            done.insert(&step.name, super::release::Layer::new(&receipt, step));
                        }
                        Err(e) => failure = Some(format!("step `{}`: {e}", step.name)),
                    }
                    continue;
                };
                // In-process steps measure the whole process, so only one runs at a time.
                let rust = matches!(step.run, StepRun::Rust(_));
                let over = limits.memory_bytes.is_some_and(|memory| reserved + cost > memory);
                if running >= limits.jobs || (running > 0 && over) || (rust && rust_running) {
                    i += 1;
                    continue;
                }
                pending.remove(i);
                let prepared = prepare(store, step, &done, code).and_then(|(receipt, request)| match key {
                    Some(key) if key != receipt.key => Err(outdated.to_string()),
                    _ => Ok((receipt, request)),
                });
                let started = self.record(&Event::StepStarted { step: step.name.clone() });
                match prepared.and_then(|prepared| started.map(|()| prepared)) {
                    Ok((receipt, request)) => {
                        let sender = sender.clone();
                        let checks = &checks;
                        scope.spawn(move || {
                            // The loop waits for every step that it started, also one that panics.
                            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                                build_step(store, root, step, receipt, request, checks)
                            }));
                            let _ = sender.send((step, cost, result.unwrap_or_else(|payload| Err(panicked(payload)))));
                        });
                        running += 1;
                        reserved += cost;
                        rust_running |= rust;
                    }
                    Err(e) => self.failed(step, e, &mut failure),
                }
            }
            if running == 0 {
                break;
            }
            let (step, cost, result) = receiver.recv().expect("a running step sends its result");
            running -= 1;
            reserved -= cost;
            rust_running &= !matches!(step.run, StepRun::Rust(_));
            match result {
                Ok(result) => {
                    let receipt = Box::new(result.receipt.clone());
                    let event = Event::StepFinished { step: step.name.clone(), reused: result.reused, receipt };
                    if let Err(e) = self.record(&event) {
                        failure.get_or_insert(e);
                    }
                    done.insert(&step.name, super::release::Layer::new(&result.receipt, step));
                    built.push(result);
                }
                Err(e) => self.failed(step, e, &mut failure),
            }
        });
        self.codes = checks.into_inner().unwrap_or_default();
        if let Some(failure) = failure {
            return Err(failure);
        }
        if let Some(step) = pending.first() {
            return Err(format!("step `{}` reads a layer that the run did not build; plan again", step.name));
        }
        built.sort_by_key(|built| position[built.receipt.step.as_str()]);
        crate::worker::check(root)?;
        Ok(built)
    }

    fn fetch(&mut self, context: &Context, plan: &Plan) -> Result<(), String> {
        for planned in plan.fetches() {
            self.check_stop(context.store)?;
            let source = context.sources.iter().find(|known| known.id == planned.source).ok_or_else(|| {
                format!(
                    "fetch {}@{}: no source `{}` in data/sources.toml",
                    planned.source, planned.version, planned.source
                )
            })?;
            let request =
                fetch::Request { source, version: Some(planned.version.clone()), params: planned.params.clone() };
            self.fetch_request(context.root, context.store, context.http, context.copies, &request, &planned.files)?;
        }
        Ok(())
    }

    /// Record preparation and execution fetches through the same journal boundary.
    pub fn fetch_request(
        &mut self,
        root: &Path,
        store: &Store,
        http: &Http,
        copies: Option<&crate::input_copy::Restore<'_>>,
        request: &fetch::Request<'_>,
        files: &[String],
    ) -> Result<crate::store::Snapshot, String> {
        self.check_stop(store)?;
        let (source, version, params) = (
            request.source.id.clone(),
            request.version.clone().unwrap_or_else(|| "newest".into()),
            request.params.clone(),
        );
        self.record(&Event::FetchStarted { source: source.clone(), version: version.clone(), params: params.clone() })?;
        let start = Instant::now();
        match crate::input_copy::fetch_checked(root, store, http, copies, request, files, Some(&mut self.codes)) {
            Ok(snapshot) => {
                let bytes = snapshot.files.iter().map(|file| file.size).sum();
                let wall_ms = start.elapsed().as_millis() as u64;
                self.record(&Event::FetchFinished {
                    source,
                    version,
                    params,
                    resolved: snapshot.version.clone(),
                    bytes,
                    wall_ms,
                })?;
                Ok(snapshot)
            }
            Err(error) => {
                let mut failed = format!("fetch {source}@{version}: {error}");
                if let Err(journal) = self.record(&Event::FetchFailed { source, version, params, error }) {
                    failed += &format!("; the run journal could not record the failure: {journal}");
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

    pub fn finish(mut self, error: Option<&str>) -> Result<(), String> {
        let wall_ms = self.start.elapsed().as_millis() as u64;
        self.record(&Event::Finished { ok: error.is_none(), error: error.map(str::to_string), wall_ms })?;
        self.sync()
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Running,
    Ok,
    /// It failed, or its process ended before it finished.
    Failed,
}

/// A run, as `obc data runs` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub id: String,
    pub command: String,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub started: String,
    pub outcome: Outcome,
    /// `None` until it finishes.
    pub wall_ms: Option<u64>,
    /// The size of the files of its fetches.
    pub bytes_fetched: u64,
    /// The size of the layers that it built; a reused layer is not counted.
    pub bytes_built: u64,
}

/// A run with its fetches and steps, as `obc data runs RUN` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Details {
    #[serde(flatten)]
    pub summary: Summary,
    pub error: Option<String>,
    pub phase: Option<Phase>,
    pub published: Vec<Publication>,
    pub fetches: Vec<RunFetch>,
    /// In the order they started.
    pub steps: Vec<RunStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunFetch {
    pub source: String,
    pub version: String,
    pub resolved: Option<String>,
    pub params: Vec<(String, String)>,
    /// `None` while it runs, and when it failed.
    pub bytes: Option<u64>,
    pub wall_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunStep {
    pub step: String,
    pub reused: bool,
    /// `None` while the step runs, and when it failed.
    pub receipt: Option<Receipt>,
    pub error: Option<String>,
    /// The layers of this run that read it.
    pub users: Vec<String>,
    /// Its wall time in the newest earlier run that built it.
    pub last_wall_ms: Option<u64>,
}

/// The events of a run, as far as they are written.
pub fn events(store: &Store, id: &str) -> Result<Vec<Event>, String> {
    let mut events = Vec::new();
    Reader::open(store, id)?.read(|event| {
        events.push(event.clone());
        Ok(())
    })?;
    Ok(events)
}

/// Give each event of a run to `each`, and wait for more until the run ends.
pub fn follow(store: &Store, id: &str, mut each: impl FnMut(&Event) -> Result<(), String>) -> Result<(), String> {
    let mut reader = Reader::open(store, id)?;
    loop {
        if reader.read(&mut each)? {
            return Ok(());
        }
        if !store.is_locked(&format!("run-{id}"))? {
            // Its process ended without `finished`; read what it wrote before it ended.
            reader.read(&mut each)?;
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Every run in the store, newest first.
pub fn list(store: &Store) -> Result<Vec<Summary>, String> {
    Ok(all(store)?.into_iter().rev().map(|run| run.summary).collect())
}

pub fn details(store: &Store, id: &str) -> Result<Details, String> {
    let runs = all(store)?;
    let at = runs.iter().position(|run| run.summary.id == id).ok_or_else(|| format!("no run `{id}`"))?;
    Ok(with_history(&runs, at))
}

/// What `details` gives for every run, newest first, from one read of the runs.
pub fn all_details(store: &Store) -> Result<Vec<Details>, String> {
    let runs = all(store)?;
    Ok((0..runs.len()).rev().map(|at| with_history(&runs, at)).collect())
}

/// Run `at` of `runs`, oldest first, with the users of each step and its time in an earlier run.
fn with_history(runs: &[Details], at: usize) -> Details {
    let mut details = runs[at].clone();
    let mut users: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for receipt in runs[at].steps.iter().filter_map(|step| step.receipt.as_ref()) {
        for input in receipt.inputs.iter().filter(|input| input.kind == InputKind::Layer) {
            users.entry(&input.name).or_default().push(receipt.step.clone());
        }
    }
    for step in &mut details.steps {
        step.users = users.get(step.step.as_str()).cloned().unwrap_or_default();
        step.last_wall_ms = runs[..at].iter().rev().find_map(|run| {
            let earlier = run.steps.iter().find(|earlier| earlier.step == step.step && !earlier.reused)?;
            earlier.receipt.as_ref().map(|receipt| receipt.wall_ms)
        });
    }
    details
}

/// Every run, oldest first.
fn all(store: &Store) -> Result<Vec<Details>, String> {
    let dir = store.root().join("runs");
    let mut ids: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok()?.strip_suffix(".jsonl").map(str::to_string))
            .filter(|id| check_id(id).is_ok())
            .collect(),
        Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    ids.sort_by_key(|id| {
        let (time, suffix) = id.split_at(17);
        (time.to_string(), suffix.trim_start_matches('-').parse::<u32>().unwrap_or(1))
    });
    ids.into_iter().map(|id| read_run(store, id)).collect()
}

/// A run file that cannot be read is a run that failed, unless its process still runs.
fn read_run(store: &Store, id: String) -> Result<Details, String> {
    // The lock before the events: a run that ends after this has its `finished` in the file.
    let running = store.is_locked(&format!("run-{id}"))?;
    let mut run = blank(&id, running);
    let events = match events(store, &id) {
        Ok(events) => events,
        Err(e) => {
            run.error = Some(e);
            return Ok(run);
        }
    };
    Ok(from_events(id, events, running))
}

fn blank(id: &str, running: bool) -> Details {
    Details {
        summary: Summary {
            id: id.into(),
            command: String::new(),
            started: String::new(),
            outcome: if running { Outcome::Running } else { Outcome::Failed },
            wall_ms: None,
            bytes_fetched: 0,
            bytes_built: 0,
        },
        error: None,
        phase: None,
        published: Vec::new(),
        fetches: Vec::new(),
        steps: Vec::new(),
    }
}

fn from_events(id: String, events: Vec<Event>, running: bool) -> Details {
    let mut run = blank(&id, running);
    let Some(Event::Started { command, at }) = events.first() else {
        if !running {
            run.error = Some(format!("run {id}: it does not start with `started`"));
        }
        return run;
    };
    (run.summary.command, run.summary.started) = (command.clone(), at.clone());
    // A version fetched with two sets of params is two fetches.
    let fetch = |fetches: &mut Vec<RunFetch>, source: String, version: String, params: Vec<(String, String)>| {
        let same = |fetch: &RunFetch| (&fetch.source, &fetch.version, &fetch.params) == (&source, &version, &params);
        fetches.iter().position(same).unwrap_or_else(|| {
            fetches.push(RunFetch { source, version, params, resolved: None, bytes: None, wall_ms: None, error: None });
            fetches.len() - 1
        })
    };
    let step = |steps: &mut Vec<RunStep>, name: &str| -> usize {
        steps.iter().position(|step| step.step == name).unwrap_or_else(|| {
            let blank = RunStep {
                step: name.into(),
                reused: false,
                receipt: None,
                error: None,
                users: Vec::new(),
                last_wall_ms: None,
            };
            steps.push(blank);
            steps.len() - 1
        })
    };
    for event in events {
        match event {
            Event::Started { .. } => {}
            Event::Phase { phase } => run.phase = Some(phase),
            Event::Published { mutation } => run.published.push(mutation),
            Event::FetchStarted { source, version, params } => {
                fetch(&mut run.fetches, source, version, params);
            }
            Event::FetchFinished { source, version, params, resolved, bytes, wall_ms } => {
                run.summary.bytes_fetched += bytes;
                let i = fetch(&mut run.fetches, source, version, params);
                (run.fetches[i].bytes, run.fetches[i].wall_ms) = (Some(bytes), Some(wall_ms));
                run.fetches[i].resolved = Some(resolved);
            }
            Event::FetchFailed { source, version, params, error } => {
                let i = fetch(&mut run.fetches, source, version, params);
                run.fetches[i].error = Some(error);
            }
            Event::StepStarted { step: name } => {
                step(&mut run.steps, &name);
            }
            Event::StepFinished { step: name, reused, receipt } => {
                if !reused {
                    run.summary.bytes_built += receipt.bytes_out;
                }
                let i = step(&mut run.steps, &name);
                (run.steps[i].reused, run.steps[i].receipt) = (reused, Some(*receipt));
            }
            Event::StepFailed { step: name, error } => {
                let i = step(&mut run.steps, &name);
                run.steps[i].error = Some(error);
            }
            Event::Finished { ok, error, wall_ms } => {
                run.summary.outcome = if ok { Outcome::Ok } else { Outcome::Failed };
                run.summary.wall_ms = Some(wall_ms);
                run.error = error;
            }
        }
    }
    run
}

/// Reads the complete lines of a run file, and keeps its place for the lines that follow.
struct Reader {
    file: File,
    id: String,
    buffer: Vec<u8>,
}

impl Reader {
    fn open(store: &Store, id: &str) -> Result<Self, String> {
        check_id(id)?;
        let file = File::open(store.run(id)).map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("no run `{id}`"),
            _ => format!("run {id}: {e}"),
        })?;
        Ok(Self { file, id: id.into(), buffer: Vec::new() })
    }

    /// Give each new complete line to `each`. True when the run finished.
    fn read(&mut self, mut each: impl FnMut(&Event) -> Result<(), String>) -> Result<bool, String> {
        self.file.read_to_end(&mut self.buffer).map_err(|e| format!("run {}: {e}", self.id))?;
        while let Some(end) = self.buffer.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            let event: Event = serde_json::from_slice(&line).map_err(|e| format!("run {}: {e}", self.id))?;
            each(&event)?;
            if matches!(event, Event::Finished { .. }) {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::plan::Estimate;
    use crate::engine::tests::{fixture, pipeline, snapshot, step, steps_crate, summary, write, JOIN};
    use crate::engine::{Code, Input, Request};
    use crate::store::Requested;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Each event as `event` or `event step`.
    fn trace(events: &[Event]) -> Vec<String> {
        let name = |event: &Event| serde_json::to_value(event).unwrap()["event"].as_str().unwrap().to_string();
        events
            .iter()
            .map(|event| match event {
                Event::StepStarted { step } | Event::StepFinished { step, .. } | Event::StepFailed { step, .. } => {
                    format!("{} {step}", name(event))
                }
                Event::FetchStarted { source, version, .. } | Event::FetchFinished { source, version, .. } => {
                    format!("{} {source}@{version}", name(event))
                }
                _ => name(event),
            })
            .collect()
    }

    fn followed(store: &Store, id: &str) -> Vec<Event> {
        let mut events = Vec::new();
        follow(store, id, |event| {
            events.push(event.clone());
            Ok(())
        })
        .unwrap();
        events
    }

    #[test]
    fn a_run_records_its_events_in_order_and_a_later_run_resumes_after_a_failed_step() {
        let fixture = fixture("runs-resume");
        let join = fixture.root().join("join.py");
        std::fs::write(&join, "raise SystemExit(3)\n").unwrap();
        let err = fixture.build(&pipeline()).unwrap_err();
        assert!(err.starts_with("step `test/join`: `python3 join.py` failed"), "{err}");
        let failed = list(&fixture.store).unwrap().remove(0);
        assert_eq!(
            trace(&events(&fixture.store, &failed.id).unwrap()),
            [
                "started",
                "step_started test/upper",
                "step_finished test/upper",
                "step_started test/join",
                "step_failed test/join",
                "finished"
            ]
        );

        std::fs::write(&join, JOIN).unwrap();
        let built = fixture.build(&pipeline()).unwrap();
        assert_eq!(summary(&built), [("test/join", false), ("test/count", false)], "test/upper is in the store");
        std::fs::write(&join, format!("# The same bytes.\n{JOIN}")).unwrap();
        fixture.build(&pipeline()).unwrap();

        let runs = list(&fixture.store).unwrap();
        let outcomes: Vec<_> = runs.iter().map(|run| (run.command.as_str(), run.outcome)).collect();
        assert_eq!(
            outcomes,
            [("build test", Outcome::Ok), ("build test", Outcome::Ok), ("build test", Outcome::Failed)]
        );
        assert_eq!(runs[2], failed);
        assert_eq!(runs[1].bytes_built, 10 + 1, "joined.txt and count/lines.txt");
        assert_eq!(runs[0].bytes_built, 10, "test/count is reused");

        let resumed = details(&fixture.store, &runs[1].id).unwrap();
        let steps: Vec<(&str, &[String])> =
            resumed.steps.iter().map(|s| (s.step.as_str(), s.users.as_slice())).collect();
        assert_eq!(steps, [("test/join", &["test/count".to_string()][..]), ("test/count", &[][..])]);
        let last = details(&fixture.store, &runs[0].id).unwrap();
        assert_eq!(last.steps[0].last_wall_ms, resumed.steps[0].receipt.as_ref().map(|receipt| receipt.wall_ms));
        let error = details(&fixture.store, &failed.id).unwrap().steps[1].error.clone();
        assert_eq!(error.as_deref(), Some(&err["step `test/join`: ".len()..]));
        assert_eq!(followed(&fixture.store, &failed.id), events(&fixture.store, &failed.id).unwrap());
    }

    #[test]
    fn a_run_keeps_old_inputs_and_outputs_until_it_finishes() {
        use crate::store::gc;

        let fixture = fixture("run-gc-phases");
        let steps = [step(
            "test/upper",
            vec![snapshot("head", "1", &["head.txt"])],
            steps_crate(),
            "upper.txt",
            StepRun::Rust(crate::engine::tests::upper),
        )];
        let plan = fixture.plan(&steps).unwrap();
        let mut run = Run::create(&fixture.store, "apply test").unwrap();
        let (root, http) = (fixture.root(), Http::new());
        let context = Context {
            store: &fixture.store,
            root: &root,
            sources: &[],
            http: &http,
            copies: None,
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
        let read = details(&fixture.store, &id).unwrap();
        assert_eq!(read.summary.outcome, Outcome::Ok);
        assert!(gc::apply(&fixture.store, &roots, &cleanup).unwrap().is_some());
        assert!(fixture.store.snapshot("head", "1").unwrap().is_none());
        assert!(!object.exists());
    }

    #[test]
    fn a_run_whose_process_ends_before_it_finishes_has_failed() {
        let fixture = fixture("runs-interrupted");
        let run = Run::create(&fixture.store, "build test").unwrap();
        assert_eq!(list(&fixture.store).unwrap()[0].outcome, Outcome::Running);
        let id = run.id().to_string();
        drop(run);
        assert_eq!(list(&fixture.store).unwrap()[0].outcome, Outcome::Failed);
        assert_eq!(trace(&followed(&fixture.store, &id)), ["started"]);

        std::fs::write(fixture.store.run("2026-01-01-000000"), "").unwrap();
        let empty = list(&fixture.store).unwrap().pop().unwrap();
        assert_eq!((empty.id.as_str(), empty.outcome), ("2026-01-01-000000", Outcome::Failed));
        assert_eq!(check_id("../x").unwrap_err(), "`../x` is not a run id: YYYY-MM-DD-HHMMSS");
        assert!(events(&fixture.store, "2026-01-01-000000-x").is_err());
    }

    #[test]
    fn a_run_refuses_an_outdated_plan() {
        let fixture = fixture("runs-outdated");
        let plan = fixture.plan(&pipeline()).unwrap();
        write(&fixture.root().join("steps/src/lib.rs"), "// A comment.\n");
        let err = fixture.run(&pipeline(), &plan, Limits::machine()).unwrap_err();
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
        let sleep = || Code { paths: vec!["sleep.py".into()], crates: Vec::new(), ..Default::default() };
        let command = || StepRun::Command(vec!["python3".into(), "sleep.py".into()]);
        let steps: Vec<Step> = ["test/a", "test/b"]
            .map(|name| step(name, vec![snapshot("head", "1", &[])], sleep(), "out.txt", command()))
            .into();
        let mut plan = fixture.plan(&steps).unwrap();
        for (build, peak) in plan.groups.iter_mut().flat_map(|group| &mut group.builds).zip(peaks) {
            build.estimate = Some(Estimate { wall_ms: 0, bytes_out: 0, peak_rss_bytes: peak });
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
    fn two_steps_in_the_process_never_run_at_the_same_time() {
        let fixture = fixture("runs-in-process");
        let steps: Vec<Step> = ["test/a", "test/b"]
            .map(|name| step(name, vec![snapshot("head", "1", &[])], steps_crate(), "out.txt", StepRun::Rust(tracked)))
            .into();
        let plan = fixture.plan(&steps).unwrap();
        fixture.run(&steps, &plan, Limits { jobs: 4, memory_bytes: None }).unwrap();
        assert_eq!(MOST.load(Ordering::SeqCst), 1);
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
        fixture.with_acquisition();
        fixture.with_sources(std::slice::from_ref(&land));
        let tile = Input::Snapshot {
            source: "land".into(),
            version: "v1".into(),
            params: vec![("tile".into(), "a".into())],
            files: Vec::new(),
        };
        let mut steps = [step("test/copy", vec![tile], steps_crate(), "copy.bin", StepRun::Rust(first_file))];
        let plan = fixture.plan(&steps).unwrap();
        let fetch = &plan.groups[0].fetches[0];
        assert_eq!(
            (fetch.files.len(), plan.groups[0].builds[0].key.as_deref()),
            (0, None),
            "the store cannot name them"
        );

        let mut run = Run::create(&fixture.store, "build test").unwrap();
        let sources = [land];
        let http = crate::fetch::tests::quick();
        let context = Context {
            store: &fixture.store,
            root: &fixture.root(),
            sources: &sources,
            http: &http,
            copies: None,
            limits: Limits::machine(),
        };
        let built = run.build(&context, &steps, &plan).unwrap();
        run.finish(None).unwrap();
        assert_eq!(
            built[0].receipt.inputs[0].digest,
            crate::engine::digest([("a-v1.bin", sha256_of(b"tile a").as_str())])
        );
        let runs = list(&fixture.store).unwrap();
        assert_eq!(
            trace(&events(&fixture.store, &runs[0].id).unwrap()),
            [
                "started",
                "fetch_started land@v1",
                "fetch_finished land@v1",
                "step_started test/copy",
                "step_finished test/copy",
                "finished"
            ]
        );
        assert_eq!(runs[0].bytes_fetched, 6);
        assert_eq!(fixture.plan(&steps).unwrap().groups, [], "the store knows the files of the params");
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
