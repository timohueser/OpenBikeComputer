//! Runs: a run builds the builds of a plan, as many at a time as the machine allows, and writes
//! its events as JSON lines to the store.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::plan::{Plan, Removal, Switch};
use super::{build_step, order, prepare, reusable, Built, Codes, InputKind, Receipt, Step};
use crate::date;
use crate::store::{Lock, Store};

/// How many steps run at a time.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub jobs: usize,
    /// The sum of the estimated peaks of the steps that run together, or `None` for no limit. A
    /// step that alone is over it runs alone.
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

/// One line of `runs/<id>.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Started {
        command: String,
        /// `YYYY-MM-DDTHH:MM:SSZ`
        at: String,
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
    Switched(Switch),
    Removed(Removal),
    Finished {
        ok: bool,
        error: Option<String>,
        wall_ms: u64,
    },
}

/// A run that this process writes. While it lives, it holds the lock of the run, so a reader
/// knows that a run without `finished` still runs.
pub struct Run {
    id: String,
    file: File,
    start: Instant,
    _lock: Lock,
}

impl Run {
    /// Start a run with the id `YYYY-MM-DD-HHMMSS`, and a suffix `-N` when that id is taken.
    pub fn create(store: &Store, command: &str) -> Result<Run, String> {
        let now = date::now();
        let at = date::timestamp(now);
        let base = format!("{}-{}", &at[..10], at[11..19].replace(':', ""));
        for n in 1.. {
            let id = if n == 1 { base.clone() } else { format!("{base}-{n}") };
            let Some(lock) = store.try_lock(&format!("run-{id}"))? else { continue };
            let path = store.run(&id);
            std::fs::create_dir_all(path.parent().expect("a run file has a directory"))
                .map_err(|e| format!("{}: {e}", path.display()))?;
            let file = match OpenOptions::new().append(true).create_new(true).open(&path) {
                Ok(file) => file,
                Err(e) if e.kind() == ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            };
            let mut run = Run { id, file, start: Instant::now(), _lock: lock };
            run.record(&Event::Started { command: command.into(), at })?;
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

    /// Build the builds of `plan`, and reuse the layers that they read. After a step fails, no
    /// other step starts; the steps that run finish. A later run reuses every layer that this
    /// one built.
    pub fn build(
        &mut self,
        store: &Store,
        root: &Path,
        steps: &[Step],
        plan: &Plan,
        limits: Limits,
    ) -> Result<Vec<Built>, String> {
        let estimates: HashMap<&str, u64> = plan
            .builds()
            .map(|build| (build.step.as_str(), build.estimate.and_then(|e| e.peak_rss_bytes).unwrap_or(0)))
            .collect();
        let ordered = order(steps)?;
        if let Some(build) = plan.builds().find(|build| !ordered.iter().any(|step| step.name == build.step)) {
            return Err(format!("the plan builds `{}`, which no step makes", build.step));
        }
        // The planned steps and the layers that they read, in dependency order.
        let mut needed: HashSet<&str> = estimates.keys().copied().collect();
        for step in ordered.iter().rev() {
            if needed.contains(step.name.as_str()) {
                needed.extend(step.layers());
            }
        }
        let position: HashMap<&str, usize> =
            ordered.iter().enumerate().map(|(i, step)| (step.name.as_str(), i)).collect();
        let mut pending: Vec<&Step> = ordered.into_iter().filter(|step| needed.contains(step.name.as_str())).collect();
        let mut codes = Codes::default();
        for step in &pending {
            let (hash, files) = codes.get(root, &step.code).map_err(|e| format!("step `{}`: {e}", step.name))?;
            store.put_code(hash, files)?;
        }

        let mut done: HashMap<&str, Receipt> = HashMap::new();
        let mut built = Vec::new();
        let mut failure: Option<String> = None;
        let (mut running, mut reserved) = (0, 0);
        let (sender, receiver) = mpsc::channel();
        std::thread::scope(|scope| {
            loop {
                let mut i = 0;
                while failure.is_none() && i < pending.len() {
                    let step = pending[i];
                    if !step.layers().all(|name| done.contains_key(name)) {
                        i += 1;
                        continue;
                    }
                    let code = &codes.get(root, &step.code).expect("hashed above").0;
                    let Some(&estimate) = estimates.get(step.name.as_str()) else {
                        // A layer that a planned step reads: the store has it, or the plan is old.
                        pending.remove(i);
                        let reused = prepare(store, step, &done, code)
                            .and_then(|(receipt, _)| reusable(store, &receipt.key))
                            .and_then(|stored| stored.ok_or("the store has no layer for its key; plan again".into()));
                        match reused {
                            Ok(receipt) => {
                                done.insert(&step.name, receipt);
                            }
                            Err(e) => failure = Some(format!("step `{}`: {e}", step.name)),
                        }
                        continue;
                    };
                    let over = limits.memory_bytes.is_some_and(|memory| reserved + estimate > memory);
                    if running >= limits.jobs || (running > 0 && over) {
                        i += 1;
                        continue;
                    }
                    pending.remove(i);
                    let prepared = prepare(store, step, &done, code);
                    let started = self.record(&Event::StepStarted { step: step.name.clone() });
                    match prepared.and_then(|prepared| started.map(|()| prepared)) {
                        Ok((receipt, request)) => {
                            let sender = sender.clone();
                            scope.spawn(move || {
                                // The loop waits for every step that it started, also one that panics.
                                let result = panic::catch_unwind(AssertUnwindSafe(|| {
                                    build_step(store, root, step, receipt, request)
                                }));
                                let result = result.unwrap_or_else(|_| Err("it panicked".into()));
                                let _ = sender.send((step, estimate, result));
                            });
                            running += 1;
                            reserved += estimate;
                        }
                        Err(e) => self.failed(step, e, &mut failure),
                    }
                }
                if running == 0 {
                    break;
                }
                let (step, estimate, result) = receiver.recv().expect("a running step sends its result");
                running -= 1;
                reserved -= estimate;
                match result {
                    Ok(result) => {
                        let event = Event::StepFinished {
                            step: step.name.clone(),
                            reused: result.reused,
                            receipt: Box::new(result.receipt.clone()),
                        };
                        if let Err(e) = self.record(&event) {
                            failure.get_or_insert(e);
                        }
                        done.insert(&step.name, result.receipt.clone());
                        built.push(result);
                    }
                    Err(e) => self.failed(step, e, &mut failure),
                }
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

    fn failed(&mut self, step: &Step, error: String, failure: &mut Option<String>) {
        let recorded = self.record(&Event::StepFailed { step: step.name.clone(), error: error.clone() });
        failure.get_or_insert(format!("step `{}`: {error}", step.name));
        if let Err(e) = recorded {
            failure.get_or_insert(e);
        }
    }

    /// Record the end of the run.
    pub fn finish(mut self, error: Option<&str>) -> Result<(), String> {
        let wall_ms = self.start.elapsed().as_millis() as u64;
        self.record(&Event::Finished { ok: error.is_none(), error: error.map(str::to_string), wall_ms })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Running,
    Ok,
    /// It failed, or its process ended before it finished.
    Failed,
}

/// A run, as `obc data runs` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Summary {
    pub id: String,
    pub command: String,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub started: String,
    pub outcome: Outcome,
    /// `None` until it finishes.
    pub wall_ms: Option<u64>,
    /// The size of the layers that it built; a reused layer is not counted.
    pub bytes_built: u64,
    pub bytes_removed: u64,
}

/// A run with its steps, as `obc data runs RUN` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Details {
    #[serde(flatten)]
    pub summary: Summary,
    pub error: Option<String>,
    /// In the order they started.
    pub steps: Vec<RunStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
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
    let mut reader = Reader::open(store, id)?;
    reader.read(|event| {
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
        if store.try_lock(&format!("run-{id}"))?.is_some() {
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
    Ok(details)
}

/// Every run, oldest first.
fn all(store: &Store) -> Result<Vec<Details>, String> {
    let dir = store.root().join("runs");
    let mut ids: Vec<String> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok()?.strip_suffix(".jsonl").map(str::to_string))
            .collect(),
        Err(e) if e.kind() == ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    // An id is its start time and, for the second run in that second, a suffix `-N`.
    ids.sort_by_key(|id| {
        let (time, suffix) = id.split_at(id.len().min(17));
        (time.to_string(), suffix.trim_start_matches('-').parse::<u32>().unwrap_or(1))
    });
    ids.into_iter().map(|id| read_run(store, id)).collect()
}

fn read_run(store: &Store, id: String) -> Result<Details, String> {
    let events = events(store, &id)?;
    let Some(Event::Started { command, at }) = events.first().cloned() else {
        return Err(format!("run {id}: it does not start with `started`"));
    };
    let mut run = Details {
        summary: Summary {
            id,
            command,
            started: at,
            outcome: Outcome::Running,
            wall_ms: None,
            bytes_built: 0,
            bytes_removed: 0,
        },
        error: None,
        steps: Vec::new(),
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
            Event::Started { .. } | Event::Switched(_) => {}
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
            Event::Removed(removal) => run.summary.bytes_removed += removal.bytes,
            Event::Finished { ok, error, wall_ms } => {
                run.summary.outcome = if ok { Outcome::Ok } else { Outcome::Failed };
                run.summary.wall_ms = Some(wall_ms);
                run.error = error;
            }
        }
    }
    if run.summary.outcome == Outcome::Running && store.try_lock(&format!("run-{}", run.summary.id))?.is_some() {
        run.summary.outcome = Outcome::Failed;
    }
    Ok(run)
}

/// Reads the complete lines of a run file, and keeps its place for the lines that follow.
struct Reader {
    file: File,
    id: String,
    buffer: String,
}

impl Reader {
    fn open(store: &Store, id: &str) -> Result<Self, String> {
        let file = File::open(store.run(id)).map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("no run `{id}`"),
            _ => format!("run {id}: {e}"),
        })?;
        Ok(Self { file, id: id.into(), buffer: String::new() })
    }

    /// Give each new complete line to `each`. True when the run finished.
    fn read(&mut self, mut each: impl FnMut(&Event) -> Result<(), String>) -> Result<bool, String> {
        self.file.read_to_string(&mut self.buffer).map_err(|e| format!("run {}: {e}", self.id))?;
        while let Some(end) = self.buffer.find('\n') {
            let line: String = self.buffer.drain(..=end).collect();
            let event: Event = serde_json::from_str(&line).map_err(|e| format!("run {}: {e}", self.id))?;
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
    use crate::engine::tests::{fixture, pipeline, snapshot, step, steps_crate, summary, JOIN};
    use crate::engine::{Request, Run as StepRun};
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
                _ => name(event),
            })
            .collect()
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
        assert_eq!(
            details(&fixture.store, &failed.id).unwrap().steps[1].error.as_deref(),
            Some(&err["step `test/join`: ".len()..])
        );

        let mut followed = Vec::new();
        follow(&fixture.store, &failed.id, |event| {
            followed.push(event.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(followed, events(&fixture.store, &failed.id).unwrap());
    }

    #[test]
    fn a_run_whose_process_ends_before_it_finishes_has_failed() {
        let fixture = fixture("runs-interrupted");
        let run = Run::create(&fixture.store, "build test").unwrap();
        assert_eq!(list(&fixture.store).unwrap()[0].outcome, Outcome::Running);
        let id = run.id().to_string();
        drop(run);
        assert_eq!(list(&fixture.store).unwrap()[0].outcome, Outcome::Failed);
        let mut followed = Vec::new();
        follow(&fixture.store, &id, |event| {
            followed.push(event.clone());
            Ok(())
        })
        .unwrap();
        assert_eq!(trace(&followed), ["started"]);
    }

    static RUNNING: AtomicUsize = AtomicUsize::new(0);
    static MOST: AtomicUsize = AtomicUsize::new(0);

    fn tracked(request: &Request) -> Result<(), String> {
        let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
        MOST.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(300));
        RUNNING.fetch_sub(1, Ordering::SeqCst);
        std::fs::write(request.output.join("out.txt"), &request.step).map_err(|e| e.to_string())
    }

    /// The most steps that ran at a time when each of two independent steps has the estimated peak.
    fn together(name: &str, peaks: [u64; 2], memory_bytes: u64) -> usize {
        let fixture = fixture(name);
        let steps: Vec<Step> = ["test/a", "test/b"]
            .map(|name| step(name, vec![snapshot("head", "1", &[])], steps_crate(), "out.txt", StepRun::Rust(tracked)))
            .into();
        let mut plan = fixture.plan(&steps).unwrap();
        for (build, peak) in plan.groups.iter_mut().flat_map(|group| &mut group.builds).zip(peaks) {
            build.estimate = Some(Estimate { wall_ms: 0, bytes_out: 0, peak_rss_bytes: Some(peak) });
        }
        MOST.store(0, Ordering::SeqCst);
        let built = fixture.run(&steps, &plan, Limits { jobs: 4, memory_bytes: Some(memory_bytes) }).unwrap();
        assert_eq!(summary(&built), [("test/a", false), ("test/b", false)]);
        MOST.load(Ordering::SeqCst)
    }

    #[test]
    fn the_steps_that_run_together_fit_in_the_memory_budget() {
        assert_eq!(together("runs-memory-fits", [600, 600], 1200), 2);
        assert_eq!(together("runs-memory-over", [600, 600], 1000), 1);
        assert_eq!(together("runs-memory-alone", [1500, 100], 1000), 1, "a step over the budget runs alone");
    }
}
